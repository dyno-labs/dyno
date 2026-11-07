//! MP3 (MPEG-1, 2 and 2.5 Layer III): the length from the Xing or Info tag, trimmed by the LAME
//! tag's encoder delay and padding, or from a VBRI tag, or else from a walk over every frame.

use memchr::memchr_iter;

use super::{Codec, Container, Header, TRUNCATED, Unreadable, array};

const FRAME_HEADER: usize = 4;

/// The frame sync: 11 set bits, the first byte and the top 3 bits of the second.
const SYNC_BYTE: u8 = 0xFF;
const SYNC_MASK: u8 = 0xE0;

/// Version bits 01 and layer bits 00 are reserved. Layer bits 01 are Layer III.
const RESERVED_VERSION: u8 = 0b01;
const RESERVED_LAYER: u8 = 0b00;
const LAYER_III: u8 = 0b01;

/// Channel mode 3 in the last header byte.
const MONO_MODE: u8 = 0b11;

const NO_FRAME_SYNC: &str = "no frame sync";

const ID3V2_HEADER: usize = 10;
const ID3V2_FOOTER: usize = 10;
const ID3V2_FOOTER_FLAG: u8 = 0x10;
const ID3V1_TAG: usize = 128;

const XING_FRAMES_FLAG: u32 = 0x1;
const XING_BYTES_FLAG: u32 = 0x2;
const XING_TOC_FLAG: u32 = 0x4;
const XING_QUALITY_FLAG: u32 = 0x8;
const XING_TOC_BYTES: usize = 100;

/// The encoders whose tag holds the delay and padding. FFmpeg writes `Lavf` or `Lavc`.
const DELAY_TAG_ENCODERS: [&[u8; 4]; 3] = [b"LAME", b"Lavf", b"Lavc"];

/// The encoder delay and padding: 12 bits each, 21 bytes after the start of the 9-byte encoder
/// string.
const DELAY_AT: usize = 21;

/// The VBRI tag sits a fixed 32 bytes after the frame header, and holds the frame count at 14.
const VBRI_AT: usize = FRAME_HEADER + 32;
const VBRI_FRAMES_AT: usize = 14;

/// Layer III bit rates in kbit/s by index. Index 0 is free format, and 15 is not allowed.
const MPEG1_KBITS: [u32; 15] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
];
const MPEG2_KBITS: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];

pub(super) fn read(bytes: &[u8]) -> Result<Header, Unreadable> {
    let start = skip_id3v2(bytes);
    let end = end_before_id3v1(bytes);
    // An ID3v2 size can run into the ID3v1 tag, which leaves no audio between them.
    if start >= end {
        return Err(Unreadable::invalid(NO_FRAME_SYNC));
    }
    if let Some(reason) = unmodelled_stream(bytes, start) {
        return Err(Unreadable::unsupported(reason));
    }
    let (first_at, first) = first_frame(bytes, start, end)?;
    let ticks = match tagged_length(bytes, first_at, &first)? {
        Some(ticks) => ticks,
        None => walk(bytes, first_at, end, &first)? * u64::from(first.samples()),
    };
    Ok(Header {
        container: Container::Mp3,
        codec: Codec::Mp3,
        channels: first.channels(),
        sample_rate: first.sample_rate,
        ticks,
        tick_rate: first.sample_rate.into(),
        note: None,
    })
}

/// Where the audio starts: after every ID3v2 tag. 0 when there is none.
pub(super) fn skip_id3v2(bytes: &[u8]) -> usize {
    let mut at = 0;
    while bytes.get(at..at + 3) == Some(b"ID3".as_slice()) {
        let (Some(&flags), Some(size)) = (bytes.get(at + 5), array::<4>(bytes, at + 6)) else {
            break;
        };
        // A syncsafe integer: 7 bits a byte.
        let size = size
            .iter()
            .fold(0usize, |size, &byte| (size << 7) | usize::from(byte & 0x7F));
        let footer = if flags & ID3V2_FOOTER_FLAG != 0 {
            ID3V2_FOOTER
        } else {
            0
        };
        at += ID3V2_HEADER + size + footer;
    }
    at.min(bytes.len())
}

/// Where the audio ends: before a trailing 128-byte ID3v1 tag, if there is one.
pub(super) fn end_before_id3v1(bytes: &[u8]) -> usize {
    let tag_start = bytes.len().saturating_sub(ID3V1_TAG);
    if bytes.len() >= ID3V1_TAG && bytes[tag_start..].starts_with(b"TAG") {
        tag_start
    } else {
        bytes.len()
    }
}

/// MPEG audio we know but do not model: Layer I and II, and free-format bit rates.
fn unmodelled_stream(bytes: &[u8], start: usize) -> Option<&'static str> {
    let header = array::<4>(bytes, start)?;
    if !synced(header) || version_bits(header) == RESERVED_VERSION {
        return None;
    }
    match layer_bits(header) {
        RESERVED_LAYER => None,
        LAYER_III => (bit_rate_index(header) == 0).then_some("free-format mp3"),
        _ => Some("mpeg layer 1 or 2"),
    }
}

/// The first frame: a valid header followed by another frame of the same stream, or by the end.
fn first_frame(bytes: &[u8], start: usize, end: usize) -> Result<(usize, Frame), Unreadable> {
    let audio = &bytes[..end];
    for offset in memchr_iter(0xFF, &audio[start..]) {
        let at = start + offset;
        let Some(frame) = Frame::at(audio, at) else {
            continue;
        };
        let next = at + frame.length;
        let followed =
            next == end || Frame::at(audio, next).is_some_and(|next| next.same_stream(&frame));
        if followed {
            return Ok((at, frame));
        }
    }
    Err(Unreadable::invalid(NO_FRAME_SYNC))
}

/// The length a Xing, Info or VBRI tag in the first frame gives. None when there is no tag.
fn tagged_length(bytes: &[u8], at: usize, first: &Frame) -> Result<Option<u64>, Unreadable> {
    let samples = u64::from(first.samples());
    if let Some(xing) = Xing::at(bytes, at + first.xing_offset()) {
        if xing
            .stream_bytes
            .is_some_and(|stream_bytes| stream_bytes > bytes.len() as u64)
        {
            return Err(Unreadable::invalid(TRUNCATED));
        }
        let Some(frames) = xing.frames else {
            return Ok(None);
        };
        let length = u64::from(frames) * samples;
        return length
            .checked_sub(xing.delay_and_padding)
            .map(Some)
            .ok_or(Unreadable::invalid(super::BAD_HEADER));
    }
    if bytes.get(at + VBRI_AT..at + VBRI_AT + 4) == Some(b"VBRI".as_slice()) {
        let frames = array::<4>(bytes, at + VBRI_AT + VBRI_FRAMES_AT)
            .ok_or(Unreadable::invalid(TRUNCATED))?;
        return Ok(Some(u64::from(u32::from_be_bytes(frames)) * samples));
    }
    Ok(None)
}

/// Counts every frame from the first to the end of the audio.
fn walk(bytes: &[u8], first_at: usize, end: usize, first: &Frame) -> Result<u64, Unreadable> {
    let mut count = 0;
    let mut at = first_at;
    while at < end {
        let frame = Frame::at(bytes, at)
            .filter(|frame| frame.same_stream(first))
            .ok_or(Unreadable::invalid("lost sync"))?;
        if at + frame.length > end {
            return Err(Unreadable::invalid(TRUNCATED));
        }
        count += 1;
        at += frame.length;
    }
    Ok(count)
}

/// One Layer III frame header.
struct Frame {
    version: Version,
    sample_rate: u32,
    mono: bool,
    /// The whole frame, header included.
    length: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Version {
    Mpeg1,
    /// MPEG-2 and MPEG-2.5: half the samples a frame, half the sample rates.
    Mpeg2,
}

impl Frame {
    fn at(bytes: &[u8], at: usize) -> Option<Frame> {
        let header = array::<4>(bytes, at)?;
        if !synced(header) || layer_bits(header) != LAYER_III {
            return None;
        }
        let (version, rates) = match version_bits(header) {
            0b11 => (Version::Mpeg1, [44_100, 48_000, 32_000]),
            0b10 => (Version::Mpeg2, [22_050, 24_000, 16_000]),
            0b00 => (Version::Mpeg2, [11_025, 12_000, 8_000]),
            _ => return None,
        };
        let kbits = match version {
            Version::Mpeg1 => MPEG1_KBITS,
            Version::Mpeg2 => MPEG2_KBITS,
        };
        let kbits = *kbits
            .get(usize::from(bit_rate_index(header)))
            .filter(|&&kbits| kbits > 0)?;
        let sample_rate = *rates.get(usize::from((header[2] >> 2) & 0b11))?;
        let padding = usize::from((header[2] >> 1) & 1);
        // Bytes a frame: samples / 8 bits x bit rate / sample rate, plus the padding byte.
        let slot_bytes = match version {
            Version::Mpeg1 => 144,
            Version::Mpeg2 => 72,
        };
        let length = (slot_bytes * kbits * 1_000 / sample_rate) as usize + padding;
        Some(Frame {
            version,
            sample_rate,
            mono: header[3] >> 6 == MONO_MODE,
            length,
        })
    }

    fn same_stream(&self, other: &Frame) -> bool {
        self.version == other.version && self.sample_rate == other.sample_rate
    }

    fn samples(&self) -> u32 {
        match self.version {
            Version::Mpeg1 => 1_152,
            Version::Mpeg2 => 576,
        }
    }

    fn channels(&self) -> u32 {
        if self.mono { 1 } else { 2 }
    }

    /// Where a Xing or Info tag starts, from the frame start: after the header and the side
    /// information. Tag readers look there even when a CRC follows the header.
    fn xing_offset(&self) -> usize {
        let side_info = match (self.version, self.mono) {
            (Version::Mpeg1, false) => 32,
            (Version::Mpeg1, true) | (Version::Mpeg2, false) => 17,
            (Version::Mpeg2, true) => 9,
        };
        FRAME_HEADER + side_info
    }
}

/// A Xing or Info tag, with the LAME tag's delay and padding when one follows.
struct Xing {
    frames: Option<u32>,
    stream_bytes: Option<u64>,
    delay_and_padding: u64,
}

impl Xing {
    fn at(bytes: &[u8], at: usize) -> Option<Xing> {
        let id = bytes.get(at..at + 4)?;
        if id != b"Xing" && id != b"Info" {
            return None;
        }
        let u32_at = |at: usize| array::<4>(bytes, at).map(u32::from_be_bytes);
        let flags = u32_at(at + 4)?;
        let has = |flag: u32| flags & flag != 0;
        // The optional fields follow the flags in this order, each only when its flag is set.
        let mut cursor = at + 8;
        let mut frames = None;
        if has(XING_FRAMES_FLAG) {
            frames = u32_at(cursor);
            cursor += 4;
        }
        let mut stream_bytes = None;
        if has(XING_BYTES_FLAG) {
            stream_bytes = u32_at(cursor).map(u64::from);
            cursor += 4;
        }
        if has(XING_TOC_FLAG) {
            cursor += XING_TOC_BYTES;
        }
        if has(XING_QUALITY_FLAG) {
            cursor += 4;
        }
        Some(Xing {
            frames,
            stream_bytes,
            delay_and_padding: delay_and_padding(bytes, cursor).unwrap_or(0),
        })
    }
}

/// The encoder delay plus padding from a LAME tag at `at`, when the encoder string names one.
fn delay_and_padding(bytes: &[u8], at: usize) -> Option<u64> {
    let encoder = array::<4>(bytes, at)?;
    if !DELAY_TAG_ENCODERS.contains(&&encoder) {
        return None;
    }
    let [high, middle, low] = array::<3>(bytes, at + DELAY_AT)?;
    let delay = (u64::from(high) << 4) | u64::from(middle >> 4);
    let padding = (u64::from(middle & 0x0F) << 8) | u64::from(low);
    Some(delay + padding)
}

fn synced(header: [u8; 4]) -> bool {
    header[0] == SYNC_BYTE && header[1] & SYNC_MASK == SYNC_MASK
}

fn version_bits(header: [u8; 4]) -> u8 {
    (header[1] >> 3) & 0b11
}

fn layer_bits(header: [u8; 4]) -> u8 {
    (header[1] >> 1) & 0b11
}

fn bit_rate_index(header: [u8; 4]) -> u8 {
    header[2] >> 4
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MPEG-1 Layer III, 128 kbit/s, 44.1 kHz, no CRC, no padding: 417 bytes and 1152 samples.
    const MPEG1_STEREO: [u8; 4] = [0xFF, 0xFB, 0x90, 0x00];
    const MPEG1_MONO: [u8; 4] = [0xFF, 0xFB, 0x90, 0xC0];
    /// As `MPEG1_STEREO`, with the protection bit 0: a CRC follows the header.
    const MPEG1_STEREO_CRC: [u8; 4] = [0xFF, 0xFA, 0x90, 0x00];
    const MPEG1_FRAME_BYTES: usize = 417;

    /// Where the tags sit in an MPEG-1 stereo frame, as the specs place them. A Xing or Info tag
    /// follows the 4-byte header and 32 bytes of side info. A VBRI tag sits 32 bytes after the
    /// header, with its frame count at 14. A LAME tag's delay sits 21 bytes into the encoder
    /// string.
    const XING_TAG_AT: usize = 36;
    const VBRI_TAG_AT: usize = 36;
    const VBRI_COUNT_AT: usize = VBRI_TAG_AT + 14;
    const LAME_DELAY_AT: usize = 21;

    fn frame(header: [u8; 4]) -> Vec<u8> {
        let mut bytes = header.to_vec();
        bytes.resize(MPEG1_FRAME_BYTES, 0);
        bytes
    }

    fn frames(header: [u8; 4], count: usize) -> Vec<u8> {
        (0..count).flat_map(|_| frame(header)).collect()
    }

    /// A first frame that holds an Info tag with a frame count and a LAME tag.
    fn info_frame(frames: u32, delay: u16, padding: u16) -> Vec<u8> {
        let mut bytes = frame(MPEG1_STEREO);
        let tag = XING_TAG_AT;
        bytes[tag..tag + 4].copy_from_slice(b"Info");
        bytes[tag + 4..tag + 8].copy_from_slice(&XING_FRAMES_FLAG.to_be_bytes());
        bytes[tag + 8..tag + 12].copy_from_slice(&frames.to_be_bytes());
        let encoder = tag + 12;
        bytes[encoder..encoder + 9].copy_from_slice(b"LAME3.100");
        let delay_at = encoder + LAME_DELAY_AT;
        bytes[delay_at] = (delay >> 4) as u8;
        bytes[delay_at + 1] = ((delay & 0x0F) << 4) as u8 | (padding >> 8) as u8;
        bytes[delay_at + 2] = padding as u8;
        bytes
    }

    #[test]
    fn without_a_tag_every_frame_is_counted() {
        let header = read(&frames(MPEG1_STEREO, 10)).unwrap();
        assert_eq!(
            (header.ticks, header.tick_rate, header.channels),
            (11_520, 44_100, 2)
        );
    }

    #[test]
    fn a_lame_tag_takes_off_the_encoder_delay_and_padding() {
        let mut bytes = info_frame(40, 576, 1_404);
        bytes.extend(frames(MPEG1_STEREO, 2));
        assert_eq!(read(&bytes).unwrap().ticks, 40 * 1_152 - 576 - 1_404);
    }

    /// A Xing tag with 40 frames and a stream byte count, then two more frames: three in all.
    fn xing_stream(header: [u8; 4], stream_bytes: u32) -> Vec<u8> {
        let mut bytes = frame(header);
        let tag = XING_TAG_AT;
        let flags = XING_FRAMES_FLAG | XING_BYTES_FLAG;
        bytes[tag..tag + 4].copy_from_slice(b"Xing");
        bytes[tag + 4..tag + 8].copy_from_slice(&flags.to_be_bytes());
        bytes[tag + 8..tag + 12].copy_from_slice(&40u32.to_be_bytes());
        bytes[tag + 12..tag + 16].copy_from_slice(&stream_bytes.to_be_bytes());
        [bytes, frames(header, 2)].concat()
    }

    const THREE_FRAMES: u32 = 3 * MPEG1_FRAME_BYTES as u32;

    #[test]
    fn a_xing_tag_gives_the_frame_count() {
        let bytes = xing_stream(MPEG1_STEREO, THREE_FRAMES);
        assert_eq!(read(&bytes).unwrap().ticks, 40 * 1_152);
    }

    #[test]
    fn a_xing_byte_count_past_the_end_is_truncated() {
        let bytes = xing_stream(MPEG1_STEREO, THREE_FRAMES + 1);
        assert_eq!(read(&bytes), Err(Unreadable::invalid(TRUNCATED)));
    }

    #[test]
    fn a_crc_does_not_move_the_xing_tag() {
        let bytes = xing_stream(MPEG1_STEREO_CRC, THREE_FRAMES);
        assert_eq!(read(&bytes).unwrap().ticks, 40 * 1_152);
    }

    #[test]
    fn a_vbri_tag_gives_the_frame_count() {
        let mut bytes = frame(MPEG1_STEREO);
        bytes[VBRI_TAG_AT..VBRI_TAG_AT + 4].copy_from_slice(b"VBRI");
        bytes[VBRI_COUNT_AT..VBRI_COUNT_AT + 4].copy_from_slice(&30u32.to_be_bytes());
        bytes.extend(frames(MPEG1_STEREO, 2));
        assert_eq!(read(&bytes).unwrap().ticks, 30 * 1_152);
    }

    #[test]
    fn mode_3_is_mono() {
        assert_eq!(read(&frames(MPEG1_MONO, 2)).unwrap().channels, 1);
    }

    #[test]
    fn id3_tags_at_either_end_are_skipped() {
        // The ID3v2 tag holds bytes that look like two frames, as cover art can.
        let look_alike = frames(MPEG1_STEREO, 2);
        let syncsafe_size = [
            0,
            0,
            (look_alike.len() >> 7) as u8,
            (look_alike.len() & 0x7F) as u8,
        ];
        let mut bytes = b"ID3\x04\0\0".to_vec();
        bytes.extend_from_slice(&syncsafe_size);
        bytes.extend(look_alike);
        bytes.extend(frames(MPEG1_STEREO, 3));
        let mut id3v1 = b"TAG".to_vec();
        id3v1.resize(ID3V1_TAG, 0);
        bytes.extend(id3v1);
        assert_eq!(read(&bytes).unwrap().ticks, 3 * 1_152);
    }

    #[test]
    fn an_id3v2_tag_that_runs_into_the_id3v1_tag_leaves_no_frame_sync() {
        let mut bytes = b"ID3\x04\0\0\x7F\x7F\x7F\x7F".to_vec();
        bytes.extend(frames(MPEG1_STEREO, 2));
        let mut id3v1 = b"TAG".to_vec();
        id3v1.resize(ID3V1_TAG, 0);
        bytes.extend(id3v1);
        assert_eq!(read(&bytes), Err(Unreadable::invalid(NO_FRAME_SYNC)));
    }

    #[test]
    fn a_frame_cut_short_is_truncated() {
        let mut bytes = frames(MPEG1_STEREO, 3);
        bytes.truncate(bytes.len() - 1);
        assert_eq!(read(&bytes), Err(Unreadable::invalid(TRUNCATED)));
    }

    #[test]
    fn junk_between_frames_loses_sync() {
        let mut bytes = frames(MPEG1_STEREO, 2);
        bytes.extend_from_slice(&[0; 7]);
        bytes.extend(frames(MPEG1_STEREO, 2));
        assert_eq!(read(&bytes), Err(Unreadable::invalid("lost sync")));
    }

    #[test]
    fn no_two_frames_in_a_row_is_no_frame_sync() {
        let mut bytes = vec![0; 100];
        bytes.extend_from_slice(&MPEG1_STEREO);
        bytes.extend_from_slice(&[0; 600]);
        assert_eq!(read(&bytes), Err(Unreadable::invalid(NO_FRAME_SYNC)));
    }

    #[test]
    fn layer_2_is_unsupported() {
        let layer_2 = [0xFF, 0xFD, 0x90, 0x00];
        assert_eq!(
            read(&frames(layer_2, 2)),
            Err(Unreadable::unsupported("mpeg layer 1 or 2"))
        );
    }
}
