//! Ogg Vorbis and Ogg Opus: the ident header from the first page, the length from the granule
//! position of the last page (the Ogg spec, the Vorbis I spec and RFC 7845).

use memchr::memmem;

use super::{BAD_HEADER, Codec, Container, Header, TRUNCATED, Unreadable, array};

/// What the first packet of a logical stream says.
struct Stream {
    codec: Codec,
    channels: u32,
    sample_rate: u32,
    tick_rate: u64,
    /// Samples the decoder drops at the start. Always 0 for Vorbis.
    pre_skip: u64,
}

/// One page's header.
struct Page {
    start: usize,
    /// None when no packet ends on the page (granule -1).
    granule: Option<u64>,
    serial: u32,
    /// Where the segment data starts.
    body_start: usize,
    end: usize,
}

enum PageAt {
    Whole(Page),
    PastTheEnd,
    NotAPage,
}

/// The fixed part of a page header, before the segment table.
const PAGE_HEADER: usize = 27;

/// The longest page: the fixed header, 255 lacing values and 255 segments of 255 bytes.
const MAX_PAGE: usize = PAGE_HEADER + 255 + 255 * 255;

/// Where the end scan starts, back from the end of the file. One whole page always fits.
const TAIL_WINDOW: usize = MAX_PAGE + PAGE_HEADER;

const CAPTURE_PATTERN: &[u8; 4] = b"OggS";
const GRANULE_AT: usize = 6;
const SERIAL_AT: usize = 14;
const CRC_AT: usize = 22;
const SEGMENT_COUNT_AT: usize = 26;

/// The page CRC: polynomial 0x04C11DB7, initial value 0, not reflected.
const CRC_POLYNOMIAL: u32 = 0x04C1_1DB7;
const CRC_TABLE: [u32; 256] = crc_table();

const VORBIS_IDENT: &[u8; 7] = b"\x01vorbis";
const VORBIS_CHANNELS_AT: usize = 11;
const VORBIS_RATE_AT: usize = 12;

const OPUS_HEAD: &[u8; 8] = b"OpusHead";
const OPUS_CHANNELS_AT: usize = 9;
const OPUS_PRE_SKIP_AT: usize = 10;

/// Opus always decodes at 48 kHz, and its granules count 48 kHz samples. The OpusHead field
/// `input_sample_rate` only says what the encoder was given.
const OPUS_RATE: u32 = 48_000;

pub(super) fn read(bytes: &[u8]) -> Result<Header, Unreadable> {
    let first = match Page::at(bytes, 0) {
        PageAt::Whole(page) => page,
        PageAt::PastTheEnd => return Err(Unreadable::invalid(TRUNCATED)),
        PageAt::NotAPage => return Err(Unreadable::invalid(BAD_HEADER)),
    };
    let stream = Stream::from_first_packet(first.body(bytes))?;
    let granule = last_granule(bytes, first.serial)?;
    if stream.codec == Codec::Opus && granule <= stream.pre_skip {
        return Err(Unreadable::invalid("granule not above pre-skip"));
    }
    Ok(Header {
        container: Container::Ogg,
        codec: stream.codec,
        channels: stream.channels,
        sample_rate: stream.sample_rate,
        ticks: granule - stream.pre_skip,
        tick_rate: stream.tick_rate,
        note: None,
    })
}

impl Page {
    fn at(bytes: &[u8], start: usize) -> PageAt {
        if bytes.get(start..start + 4) != Some(CAPTURE_PATTERN.as_slice()) {
            return PageAt::NotAPage;
        }
        let (Some(fixed), Some(&segment_count)) = (
            bytes.get(start..start + PAGE_HEADER),
            bytes.get(start + SEGMENT_COUNT_AT),
        ) else {
            return PageAt::PastTheEnd;
        };
        // Stream structure version 0 is the only one.
        if fixed[4] != 0 {
            return PageAt::NotAPage;
        }
        let body_start = start + PAGE_HEADER + usize::from(segment_count);
        let Some(lacing) = bytes.get(start + PAGE_HEADER..body_start) else {
            return PageAt::PastTheEnd;
        };
        let end = body_start + lacing.iter().map(|&size| usize::from(size)).sum::<usize>();
        if end > bytes.len() {
            return PageAt::PastTheEnd;
        }
        let granule = array::<8>(fixed, GRANULE_AT).map(i64::from_le_bytes);
        let serial = array::<4>(fixed, SERIAL_AT).map(u32::from_le_bytes);
        PageAt::Whole(Page {
            start,
            granule: granule.and_then(|granule| u64::try_from(granule).ok()),
            serial: serial.unwrap_or_default(),
            body_start,
            end,
        })
    }

    /// The page's segment data. The first page of a stream holds exactly its ident packet.
    fn body<'a>(&self, bytes: &'a [u8]) -> &'a [u8] {
        &bytes[self.body_start..self.end]
    }

    fn crc_is_right(&self, bytes: &[u8]) -> bool {
        let page = &bytes[self.start..self.end];
        let stored = array::<4>(page, CRC_AT).map(u32::from_le_bytes);
        stored == Some(crc(page))
    }
}

impl Stream {
    fn from_first_packet(packet: &[u8]) -> Result<Stream, Unreadable> {
        let bad_header = Unreadable::invalid(BAD_HEADER);
        if packet.starts_with(VORBIS_IDENT) {
            let channels = *packet.get(VORBIS_CHANNELS_AT).ok_or(bad_header)?;
            let rate = array::<4>(packet, VORBIS_RATE_AT).ok_or(bad_header)?;
            let sample_rate = u32::from_le_bytes(rate);
            return Ok(Stream {
                codec: Codec::Vorbis,
                channels: channels.into(),
                sample_rate,
                tick_rate: sample_rate.into(),
                pre_skip: 0,
            });
        }
        if packet.starts_with(OPUS_HEAD) {
            let channels = *packet.get(OPUS_CHANNELS_AT).ok_or(bad_header)?;
            let pre_skip = array::<2>(packet, OPUS_PRE_SKIP_AT).ok_or(bad_header)?;
            return Ok(Stream {
                codec: Codec::Opus,
                channels: channels.into(),
                sample_rate: OPUS_RATE,
                tick_rate: OPUS_RATE.into(),
                pre_skip: u16::from_le_bytes(pre_skip).into(),
            });
        }
        if packet.starts_with(b"\x7fFLAC") {
            return Err(Unreadable::unsupported("flac"));
        }
        Err(Unreadable::unsupported("ogg codec"))
    }
}

/// The granule position of the last page of the logical stream `serial`.
///
/// Scans the tail of the file for capture patterns, last first. A page counts only when its CRC
/// is right and it belongs to `serial`. When the last pattern starts a page that runs past the
/// end of the file, the file is cut short. When the tail holds only other streams' pages, the
/// first stream ends before the tail and is not found.
fn last_granule(bytes: &[u8], serial: u32) -> Result<u64, Unreadable> {
    let window_start = bytes.len().saturating_sub(TAIL_WINDOW);
    let mut starts: Vec<usize> = memmem::find_iter(&bytes[window_start..], CAPTURE_PATTERN)
        .map(|offset| window_start + offset)
        .collect();
    starts.reverse();
    let mut other_stream_seen = false;
    for (index, &start) in starts.iter().enumerate() {
        let page = match Page::at(bytes, start) {
            PageAt::Whole(page) => page,
            PageAt::PastTheEnd if index == 0 => return Err(Unreadable::invalid(TRUNCATED)),
            PageAt::PastTheEnd | PageAt::NotAPage => continue,
        };
        if !page.crc_is_right(bytes) {
            continue;
        }
        if page.serial != serial {
            other_stream_seen = true;
            continue;
        }
        if let Some(granule) = page.granule {
            return Ok(granule);
        }
    }
    if other_stream_seen {
        return Err(Unreadable::unsupported("chained ogg"));
    }
    Err(Unreadable::invalid("no granule"))
}

/// The page CRC, computed with the CRC field read as zeros.
fn crc(page: &[u8]) -> u32 {
    page.iter().enumerate().fold(0, |crc, (index, &byte)| {
        let byte = if (CRC_AT..CRC_AT + 4).contains(&index) {
            0
        } else {
            byte
        };
        let table_index = usize::from((crc >> 24) as u8 ^ byte);
        (crc << 8) ^ CRC_TABLE[table_index]
    })
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut crc = (index as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ CRC_POLYNOMIAL
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERIAL: u32 = 7;

    /// One page with one packet, and a right CRC.
    fn page(serial: u32, granule: i64, packet: &[u8]) -> Vec<u8> {
        let mut bytes = CAPTURE_PATTERN.to_vec();
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&granule.to_le_bytes());
        bytes.extend_from_slice(&serial.to_le_bytes());
        bytes.extend_from_slice(&[0; 8]);
        let mut lacing = vec![255; packet.len() / 255];
        lacing.push((packet.len() % 255) as u8);
        bytes.push(lacing.len() as u8);
        bytes.extend_from_slice(&lacing);
        bytes.extend_from_slice(packet);
        let crc = crc(&bytes);
        bytes[CRC_AT..CRC_AT + 4].copy_from_slice(&crc.to_le_bytes());
        bytes
    }

    fn opus_head(channels: u8, pre_skip: u16) -> Vec<u8> {
        let mut packet = OPUS_HEAD.to_vec();
        packet.extend_from_slice(&[1, channels]);
        packet.extend_from_slice(&pre_skip.to_le_bytes());
        packet.extend_from_slice(&44_100u32.to_le_bytes());
        packet.extend_from_slice(&[0, 0, 0]);
        packet
    }

    fn vorbis_ident(channels: u8, sample_rate: u32) -> Vec<u8> {
        let mut packet = VORBIS_IDENT.to_vec();
        packet.extend_from_slice(&[0; 4]);
        packet.push(channels);
        packet.extend_from_slice(&sample_rate.to_le_bytes());
        packet.extend_from_slice(&[0; 14]);
        packet
    }

    fn file(pages: &[Vec<u8>]) -> Vec<u8> {
        pages.concat()
    }

    #[test]
    fn the_crc_matches_the_ogg_polynomial() {
        // The check value of "123456789" for CRC-32/POSIX, without its final XOR.
        assert_eq!(crc(b"123456789"), 0x89A1_897F);
    }

    #[test]
    fn vorbis_length_is_the_last_granule_at_the_ident_rate() {
        let bytes = file(&[
            page(SERIAL, 0, &vorbis_ident(2, 22_050)),
            page(SERIAL, 11_025, &[0; 40]),
        ]);
        let header = read(&bytes).unwrap();
        assert_eq!(header.codec, Codec::Vorbis);
        assert_eq!(
            (header.channels, header.ticks, header.tick_rate),
            (2, 11_025, 22_050)
        );
    }

    #[test]
    fn opus_length_is_the_last_granule_minus_pre_skip_at_48_khz() {
        let bytes = file(&[
            page(SERIAL, 0, &opus_head(2, 312)),
            page(SERIAL, 48_312, &[0; 40]),
        ]);
        let header = read(&bytes).unwrap();
        assert_eq!(header.codec, Codec::Opus);
        assert_eq!(
            (header.ticks, header.tick_rate, header.sample_rate),
            (48_000, 48_000, 48_000)
        );
    }

    #[test]
    fn an_opus_granule_not_above_pre_skip_is_invalid() {
        let bytes = file(&[
            page(SERIAL, 0, &opus_head(1, 312)),
            page(SERIAL, 312, &[0; 4]),
        ]);
        assert_eq!(
            read(&bytes),
            Err(Unreadable::invalid("granule not above pre-skip"))
        );
    }

    #[test]
    fn a_page_with_no_finished_packet_does_not_give_the_granule() {
        let bytes = file(&[
            page(SERIAL, 0, &vorbis_ident(1, 8_000)),
            page(SERIAL, 800, &[0; 4]),
            page(SERIAL, -1, &[0; 4]),
        ]);
        assert_eq!(read(&bytes).unwrap().ticks, 800);
    }

    #[test]
    fn a_page_with_a_wrong_crc_is_skipped() {
        let mut damaged = page(SERIAL, 900, &[0; 4]);
        damaged[CRC_AT] ^= 1;
        let bytes = file(&[
            page(SERIAL, 0, &vorbis_ident(1, 8_000)),
            page(SERIAL, 800, &[0; 4]),
            damaged,
        ]);
        assert_eq!(read(&bytes).unwrap().ticks, 800);
    }

    #[test]
    fn a_last_page_cut_short_is_truncated() {
        let mut bytes = file(&[
            page(SERIAL, 0, &vorbis_ident(1, 8_000)),
            page(SERIAL, 800, &[0; 40]),
        ]);
        bytes.truncate(bytes.len() - 1);
        assert_eq!(read(&bytes), Err(Unreadable::invalid(TRUNCATED)));
    }

    #[test]
    fn a_first_page_cut_short_is_truncated() {
        let mut bytes = page(SERIAL, 0, &opus_head(2, 312));
        bytes.truncate(40);
        assert_eq!(read(&bytes), Err(Unreadable::invalid(TRUNCATED)));
    }

    #[test]
    fn pages_of_a_second_stream_are_skipped() {
        let bytes = file(&[
            page(SERIAL, 0, &vorbis_ident(1, 8_000)),
            page(SERIAL, 800, &[0; 4]),
            page(SERIAL + 1, 0, &vorbis_ident(1, 8_000)),
            page(SERIAL + 1, 400, &[0; 4]),
        ]);
        assert_eq!(read(&bytes).unwrap().ticks, 800);
    }

    #[test]
    fn a_tail_with_only_a_second_stream_is_unsupported() {
        let second_stream_tail = page(SERIAL + 1, 400, &vec![0; MAX_PAGE - PAGE_HEADER - 300]);
        let bytes = file(&[
            page(SERIAL, 0, &vorbis_ident(1, 8_000)),
            page(SERIAL, 800, &[0; 4]),
            page(SERIAL + 1, 0, &vorbis_ident(1, 8_000)),
            second_stream_tail.clone(),
            second_stream_tail,
        ]);
        assert_eq!(read(&bytes), Err(Unreadable::unsupported("chained ogg")));
    }

    #[test]
    fn only_the_last_capture_pattern_can_mark_the_file_cut_short() {
        // A packet that holds the start of a page header whose lacing runs far past the end.
        let mut look_alike = CAPTURE_PATTERN.to_vec();
        look_alike.resize(SEGMENT_COUNT_AT, 0);
        look_alike.push(255);
        look_alike.resize(look_alike.len() + 255, 255);
        let bytes = file(&[
            page(SERIAL, 0, &vorbis_ident(1, 8_000)),
            page(SERIAL, 800, &look_alike),
            page(SERIAL, -1, &[0; 4]),
        ]);
        assert_eq!(read(&bytes).unwrap().ticks, 800);
    }

    #[test]
    fn ogg_flac_is_unsupported() {
        let bytes = file(&[
            page(SERIAL, 0, b"\x7fFLAC\x01\x00"),
            page(SERIAL, 800, &[0; 4]),
        ]);
        assert_eq!(read(&bytes), Err(Unreadable::unsupported("flac")));
    }
}
