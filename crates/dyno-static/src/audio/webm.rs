//! WebM and Matroska audio (RFC 9559): the length is `Duration x TimestampScale` minus the
//! track's `CodecDelay`, in microseconds. A live file has no `Duration`, so its last block gives
//! the end instead.
//!
//! `Duration` is a float. It is turned into an exact fraction, so no float arithmetic happens.

use std::collections::BTreeMap;

use super::{BAD_HEADER, Codec, Container, Header, TRUNCATED, Unreadable, array};

/// An element header: its id with the length marker kept, and its size.
struct Element {
    id: u32,
    data_start: usize,
    /// None when the size is unknown: all its bits are 1.
    size: Option<u64>,
}

/// A variable-length integer: its bytes as written, and its value with the length marker taken
/// off.
struct Vint {
    raw: u64,
    value: u64,
    all_ones: bool,
}

/// What the length needs from a segment.
struct Segment {
    timestamp_scale: u64,
    duration: Option<Fraction>,
    tracks: Vec<Track>,
    /// The last block of each track, by track number.
    last_blocks: BTreeMap<u64, LastBlock>,
}

struct Track {
    number: u64,
    kind: u64,
    codec_id: String,
    codec_delay_ns: u64,
    default_duration_ns: Option<u64>,
    channels: u64,
    sample_rate_hz: u64,
}

/// A block's timing. Times are in `TimestampScale` units, except where the name says ns.
struct LastBlock {
    cluster_timestamp: u64,
    relative_timestamp: i16,
    /// More than 1 when the block laces several frames.
    frames: u64,
    /// The block group's `BlockDuration`, if it has one.
    duration: Option<u64>,
    discard_padding_ns: i64,
}

/// A non-negative number `numerator / 2^shift`, kept exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fraction {
    numerator: u128,
    shift: u32,
}

const EBML_HEADER: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const DURATION: u32 = 0x4489;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const CODEC_DELAY: u32 = 0x56AA;
const DEFAULT_DURATION: u32 = 0x23_E383;
const AUDIO: u32 = 0xE1;
const CHANNELS: u32 = 0x9F;
const SAMPLING_FREQUENCY: u32 = 0xB5;
const CLUSTER: u32 = 0x1F43_B675;
const CLUSTER_TIMESTAMP: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;
const BLOCK_DURATION: u32 = 0x9B;
const DISCARD_PADDING: u32 = 0x75A2;

const TRACK_TYPE_VIDEO: u64 = 1;
const TRACK_TYPE_AUDIO: u64 = 2;

/// The Matroska defaults when an element is absent.
const DEFAULT_TIMESTAMP_SCALE_NS: u64 = 1_000_000;
const DEFAULT_CHANNELS: u64 = 1;
const DEFAULT_SAMPLING_FREQUENCY_HZ: u64 = 8_000;

const NS_PER_US: u64 = 1_000;
const US_PER_SECOND: u64 = 1_000_000;

/// The longest element id and the longest size, in bytes.
const MAX_ID_BYTES: usize = 4;
const MAX_SIZE_BYTES: usize = 8;

/// Bits 1 and 2 of a block's flags: 0 when the block holds one frame, unlaced.
const LACING_BITS: u8 = 0b0110;

/// The largest shift kept. Dropping the bits below `2^-63` of a tick moves the end by under 2 ns,
/// even at the largest `TimestampScale`.
const MAX_SHIFT: u32 = 63;

pub(super) fn read(bytes: &[u8]) -> Result<Header, Unreadable> {
    let ebml = Element::at(bytes, 0)
        .filter(|element| element.id == EBML_HEADER)
        .ok_or(Unreadable::invalid(BAD_HEADER))?;
    let segment_at = ebml.end(bytes.len())?;
    let segment_element = Element::at(bytes, segment_at).ok_or(Unreadable::invalid(TRUNCATED))?;
    if segment_element.id != SEGMENT {
        return Err(Unreadable::invalid(BAD_HEADER));
    }
    let segment = Segment::read(bytes, &segment_element)?;
    if segment
        .tracks
        .iter()
        .any(|track| track.kind == TRACK_TYPE_VIDEO)
    {
        return Err(Unreadable::invalid("video track"));
    }
    let track = segment
        .tracks
        .iter()
        .find(|track| track.kind == TRACK_TYPE_AUDIO)
        .ok_or(Unreadable::invalid("no audio track"))?;
    let codec = codec(&track.codec_id)?;
    let (end_ns, note) = match &segment.duration {
        Some(duration) => {
            let end = duration
                .times(segment.timestamp_scale)
                .ok_or(Unreadable::invalid(BAD_HEADER))?;
            (end, None)
        }
        None => (
            segment.end_of_last_block(track)?,
            Some("duration_from_blocks"),
        ),
    };
    let ticks = end_ns
        .rounded_quotient(track.codec_delay_ns, NS_PER_US)
        .ok_or(Unreadable::invalid(BAD_HEADER))?;
    Ok(Header {
        container: Container::Webm,
        codec,
        channels: u32::try_from(track.channels).map_err(|_| Unreadable::invalid(BAD_HEADER))?,
        sample_rate: u32::try_from(track.sample_rate_hz).unwrap_or(u32::MAX),
        ticks,
        tick_rate: US_PER_SECOND,
        note,
    })
}

impl Element {
    /// None when the header is cut short or is not a valid id and size.
    fn at(bytes: &[u8], at: usize) -> Option<Element> {
        let (id, id_length) = vint(bytes, at, MAX_ID_BYTES)?;
        let id = u32::try_from(id.raw).ok()?;
        let (size, size_length) = vint(bytes, at + id_length, MAX_SIZE_BYTES)?;
        Some(Element {
            id,
            data_start: at + id_length + size_length,
            size: (!size.all_ones).then_some(size.value),
        })
    }

    /// Where the element ends. It must have a known size and end by `limit`.
    fn end(&self, limit: usize) -> Result<usize, Unreadable> {
        let size = self.size.ok_or(Unreadable::invalid(BAD_HEADER))?;
        usize::try_from(size)
            .ok()
            .and_then(|size| self.data_start.checked_add(size))
            .filter(|&end| end <= limit)
            .ok_or(Unreadable::invalid(TRUNCATED))
    }
}

/// The integer at `at`, and its length. The leading zero bits of the first byte, plus 1, give the
/// length.
fn vint(bytes: &[u8], at: usize, max_length: usize) -> Option<(Vint, usize)> {
    let first = *bytes.get(at)?;
    let length = first.leading_zeros() as usize + 1;
    if length > max_length {
        return None;
    }
    let all = bytes.get(at..at + length)?;
    let raw = all
        .iter()
        .fold(0u64, |raw, &byte| (raw << 8) | u64::from(byte));
    let value_bits = 7 * length as u32;
    let value_mask = (1u64 << value_bits) - 1;
    let value = raw & value_mask;
    let vint = Vint {
        raw,
        value,
        all_ones: value == value_mask,
    };
    Some((vint, length))
}

impl Segment {
    /// Walks every child of the segment, and every child of each cluster in the same walk. A live
    /// file gives its clusters an unknown size, so a cluster ends where the next element starts.
    fn read(bytes: &[u8], element: &Element) -> Result<Segment, Unreadable> {
        let end = match element.size {
            Some(_) => element.end(bytes.len())?,
            None => bytes.len(),
        };
        let mut segment = Segment {
            timestamp_scale: DEFAULT_TIMESTAMP_SCALE_NS,
            duration: None,
            tracks: Vec::new(),
            last_blocks: BTreeMap::new(),
        };
        let mut cluster_timestamp = 0;
        let mut at = element.data_start;
        while at < end {
            let child = Element::at(&bytes[..end], at).ok_or(Unreadable::invalid(TRUNCATED))?;
            if child.id == CLUSTER {
                if child.size.is_some() {
                    child.end(end)?;
                }
                at = child.data_start;
                continue;
            }
            let child_end = child.end(end)?;
            let body = &bytes[child.data_start..child_end];
            match child.id {
                INFO => segment.read_info(body)?,
                TRACKS => segment.read_tracks(body)?,
                CLUSTER_TIMESTAMP => cluster_timestamp = uint(body),
                SIMPLE_BLOCK => segment.note_block(body, cluster_timestamp, None, 0)?,
                BLOCK_GROUP => segment.read_block_group(body, cluster_timestamp)?,
                _ => {}
            }
            at = child_end;
        }
        Ok(segment)
    }

    fn read_info(&mut self, info: &[u8]) -> Result<(), Unreadable> {
        for child in children(info) {
            match child? {
                (TIMESTAMP_SCALE, body) => self.timestamp_scale = uint(body),
                (DURATION, body) => {
                    let duration =
                        Fraction::from_float(body).ok_or(Unreadable::invalid(BAD_HEADER))?;
                    self.duration = Some(duration);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn read_tracks(&mut self, tracks: &[u8]) -> Result<(), Unreadable> {
        for child in children(tracks) {
            if let (TRACK_ENTRY, entry) = child? {
                self.tracks.push(Track::read(entry)?);
            }
        }
        Ok(())
    }

    fn read_block_group(&mut self, group: &[u8], cluster_timestamp: u64) -> Result<(), Unreadable> {
        let mut block = None;
        let mut duration = None;
        let mut discard_padding_ns = 0;
        for child in children(group) {
            match child? {
                (BLOCK, body) => block = Some(body),
                (BLOCK_DURATION, body) => duration = Some(uint(body)),
                (DISCARD_PADDING, body) => discard_padding_ns = int(body),
                _ => {}
            }
        }
        match block {
            Some(block) => self.note_block(block, cluster_timestamp, duration, discard_padding_ns),
            None => Ok(()),
        }
    }

    /// Keeps a block's timing as its track's last block so far.
    fn note_block(
        &mut self,
        block: &[u8],
        cluster_timestamp: u64,
        duration: Option<u64>,
        discard_padding_ns: i64,
    ) -> Result<(), Unreadable> {
        let truncated = Unreadable::invalid(TRUNCATED);
        let (track, track_length) = vint(block, 0, MAX_SIZE_BYTES).ok_or(truncated)?;
        let relative = array::<2>(block, track_length).ok_or(truncated)?;
        let flags = *block.get(track_length + 2).ok_or(truncated)?;
        let frames = if flags & LACING_BITS != 0 {
            // The lace count is the number of frames minus 1.
            u64::from(*block.get(track_length + 3).ok_or(truncated)?) + 1
        } else {
            1
        };
        self.last_blocks.insert(
            track.value,
            LastBlock {
                cluster_timestamp,
                relative_timestamp: i16::from_be_bytes(relative),
                frames,
                duration,
                discard_padding_ns,
            },
        );
        Ok(())
    }

    /// Where the track's last block ends, in nanoseconds: its time plus its length.
    fn end_of_last_block(&self, track: &Track) -> Result<Fraction, Unreadable> {
        let block = self
            .last_blocks
            .get(&track.number)
            .ok_or(Unreadable::invalid("no blocks"))?;
        let scale = i128::from(self.timestamp_scale);
        let block_timestamp =
            i128::from(block.cluster_timestamp) + i128::from(block.relative_timestamp);
        let start = block_timestamp.checked_mul(scale);
        let length = match (block.duration, track.default_duration_ns) {
            (Some(duration), _) => i128::from(duration).checked_mul(scale),
            (None, Some(default_duration)) => {
                // RFC 9559: a negative DiscardPadding pads the block's start, not its end.
                let end_padding = block.discard_padding_ns.max(0);
                i128::from(default_duration)
                    .checked_mul(i128::from(block.frames))
                    .and_then(|frames| frames.checked_sub(i128::from(end_padding)))
            }
            (None, None) => return Err(Unreadable::unsupported("no block duration")),
        };
        let end = start
            .zip(length)
            .and_then(|(start, length)| start.checked_add(length))
            .and_then(|end| u128::try_from(end).ok())
            .ok_or(Unreadable::invalid(BAD_HEADER))?;
        Ok(Fraction {
            numerator: end,
            shift: 0,
        })
    }
}

/// The children of a master element with a known size. The last item is an error when a child
/// runs past the end of its parent or has an unknown size.
fn children(body: &[u8]) -> impl Iterator<Item = Result<(u32, &[u8]), Unreadable>> {
    let mut at = 0;
    std::iter::from_fn(move || {
        if at >= body.len() {
            return None;
        }
        let child = Element::at(body, at)
            .ok_or(Unreadable::invalid(TRUNCATED))
            .and_then(|element| Ok((element.id, element.data_start, element.end(body.len())?)));
        Some(match child {
            Ok((id, start, end)) => {
                at = end;
                Ok((id, &body[start..end]))
            }
            Err(why) => {
                at = body.len();
                Err(why)
            }
        })
    })
}

impl Track {
    fn read(entry: &[u8]) -> Result<Track, Unreadable> {
        let mut track = Track {
            number: 0,
            kind: 0,
            codec_id: String::new(),
            codec_delay_ns: 0,
            default_duration_ns: None,
            channels: DEFAULT_CHANNELS,
            sample_rate_hz: DEFAULT_SAMPLING_FREQUENCY_HZ,
        };
        for child in children(entry) {
            match child? {
                (TRACK_NUMBER, body) => track.number = uint(body),
                (TRACK_TYPE, body) => track.kind = uint(body),
                (CODEC_ID, body) => track.codec_id = String::from_utf8_lossy(body).into_owned(),
                (CODEC_DELAY, body) => track.codec_delay_ns = uint(body),
                (DEFAULT_DURATION, body) => track.default_duration_ns = Some(uint(body)),
                (AUDIO, audio) => track.read_audio(audio)?,
                _ => {}
            }
        }
        Ok(track)
    }

    fn read_audio(&mut self, audio: &[u8]) -> Result<(), Unreadable> {
        for child in children(audio) {
            match child? {
                (CHANNELS, body) => self.channels = uint(body),
                // The formula never reads the rate, so a bad one keeps the default.
                (SAMPLING_FREQUENCY, body) => {
                    if let Some(rate) =
                        Fraction::from_float(body).and_then(|rate| rate.rounded_quotient(0, 1))
                    {
                        self.sample_rate_hz = rate;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn codec(codec_id: &str) -> Result<Codec, Unreadable> {
    let codec = match codec_id {
        "A_OPUS" => Codec::Opus,
        "A_VORBIS" => Codec::Vorbis,
        "A_MPEG/L3" => Codec::Mp3,
        "A_PCM/INT/LIT" | "A_PCM/INT/BIG" => Codec::Pcm,
        "A_PCM/FLOAT/IEEE" => Codec::PcmFloat,
        aac if aac.starts_with("A_AAC") => Codec::Aac,
        "A_FLAC" => return Err(Unreadable::unsupported("flac")),
        _ => return Err(Unreadable::unsupported("webm codec")),
    };
    Ok(codec)
}

/// An unsigned integer element: big-endian, 0 to 8 bytes.
fn uint(body: &[u8]) -> u64 {
    body.iter()
        .take(8)
        .fold(0, |value, &byte| (value << 8) | u64::from(byte))
}

/// A signed integer element: big-endian two's complement, 0 to 8 bytes.
fn int(body: &[u8]) -> i64 {
    let length = body.len().min(8);
    if length == 0 {
        return 0;
    }
    let unsigned = uint(body);
    // Sign-extends from the element's own length.
    let unused = 64 - 8 * length as u32;
    ((unsigned << unused) as i64) >> unused
}

impl Fraction {
    /// A 4 or 8 byte IEEE float element (0 bytes is 0), exactly. None when it is negative,
    /// infinite or not a number.
    fn from_float(body: &[u8]) -> Option<Fraction> {
        let value = match body.len() {
            0 => 0.0,
            4 => f64::from(f32::from_be_bytes(array(body, 0)?)),
            8 => f64::from_be_bytes(array(body, 0)?),
            _ => return None,
        };
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        // An f64 is `significand x 2^(exponent - 1075)`, with an implicit top bit unless the
        // exponent field is 0. Converting from f32 above is exact.
        let bits = value.to_bits();
        let exponent_field = ((bits >> 52) & 0x7FF) as i32;
        let fraction_bits = bits & ((1 << 52) - 1);
        let (significand, exponent) = if exponent_field == 0 {
            (fraction_bits, -1_074)
        } else {
            (fraction_bits | (1 << 52), exponent_field - 1_075)
        };
        let mut fraction = Fraction {
            numerator: u128::from(significand),
            shift: 0,
        };
        if exponent >= 0 {
            let exponent = exponent.unsigned_abs();
            if fraction.numerator.leading_zeros() < exponent {
                return None;
            }
            fraction.numerator <<= exponent;
        } else {
            fraction.shift = exponent.unsigned_abs();
        }
        Some(fraction.trimmed())
    }

    fn times(self, factor: u64) -> Option<Fraction> {
        Some(Fraction {
            numerator: self.numerator.checked_mul(u128::from(factor))?,
            shift: self.shift,
        })
    }

    /// `round((self - minus) / divisor)`, with halves rounding up. None when the difference is
    /// negative or does not fit.
    fn rounded_quotient(&self, minus: u64, divisor: u64) -> Option<u64> {
        let minus = u128::from(minus).checked_shl(self.shift)?;
        let divisor = u128::from(divisor).checked_shl(self.shift)?;
        let difference = self.numerator.checked_sub(minus)?;
        let quotient = difference.checked_add(divisor / 2)? / divisor;
        u64::try_from(quotient).ok()
    }

    /// Drops bits finer than `2^-MAX_SHIFT`, so that the shifts in `rounded_quotient` fit in a
    /// u128.
    fn trimmed(self) -> Fraction {
        match self.shift.checked_sub(MAX_SHIFT) {
            Some(extra) if extra > 0 => Fraction {
                numerator: self.numerator >> extra.min(127),
                shift: MAX_SHIFT,
            },
            _ => self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Padding that readers skip.
    const VOID: u32 = 0xEC;

    /// An element: its id bytes, then an 8-byte size, then the body.
    fn element(id: u32, body: &[u8]) -> Vec<u8> {
        let id_bytes = id.to_be_bytes();
        let first = id_bytes.iter().position(|&byte| byte != 0).unwrap_or(3);
        let mut bytes = id_bytes[first..].to_vec();
        bytes.push(0x01);
        bytes.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        bytes.extend_from_slice(body);
        bytes
    }

    /// An element with an unknown size.
    fn open_element(id: u32) -> Vec<u8> {
        let mut bytes = id.to_be_bytes().to_vec();
        bytes.push(0x01);
        bytes.extend_from_slice(&[0xFF; 7]);
        bytes
    }

    fn uint_element(id: u32, value: u64) -> Vec<u8> {
        element(id, &value.to_be_bytes())
    }

    fn opus_track(default_duration_ns: Option<u64>) -> Vec<u8> {
        opus_track_with_audio(default_duration_ns, &uint_element(CHANNELS, 2))
    }

    fn opus_track_with_audio(default_duration_ns: Option<u64>, audio: &[u8]) -> Vec<u8> {
        let audio = element(AUDIO, audio);
        let mut entry = [
            uint_element(TRACK_NUMBER, 1),
            uint_element(TRACK_TYPE, TRACK_TYPE_AUDIO),
            element(CODEC_ID, b"A_OPUS"),
            uint_element(CODEC_DELAY, 6_500_000),
            audio,
        ]
        .concat();
        if let Some(duration) = default_duration_ns {
            entry.extend(uint_element(DEFAULT_DURATION, duration));
        }
        element(TRACKS, &element(TRACK_ENTRY, &entry))
    }

    fn simple_block(relative_timestamp: i16) -> Vec<u8> {
        let mut body = vec![0x81];
        body.extend_from_slice(&relative_timestamp.to_be_bytes());
        body.extend_from_slice(&[0x80, 0, 0]);
        element(SIMPLE_BLOCK, &body)
    }

    fn webm(segment_body: &[u8]) -> Vec<u8> {
        [
            element(EBML_HEADER, b"\x42\x82\x84webm"),
            element(SEGMENT, segment_body),
        ]
        .concat()
    }

    #[test]
    fn length_is_duration_times_scale_minus_codec_delay_in_microseconds() {
        let info = element(
            INFO,
            &[
                uint_element(TIMESTAMP_SCALE, 1_000_000),
                element(DURATION, &1_008.0f64.to_be_bytes()),
            ]
            .concat(),
        );
        let header = read(&webm(&[info, opus_track(None)].concat())).unwrap();
        assert_eq!((header.ticks, header.tick_rate), (1_001_500, 1_000_000));
        assert_eq!(
            (header.channels, header.codec, header.note),
            (2, Codec::Opus, None)
        );
    }

    #[test]
    fn a_4_byte_duration_is_read_exactly() {
        // At this scale, 0.1 as f32 minus 6.5 ms is 12,321,398.4999... us, a hair under a half.
        // f64 arithmetic gives 12,321,398.5 and so rounds up.
        let info = element(
            INFO,
            &[
                uint_element(TIMESTAMP_SCALE, 123_278_983_163),
                element(DURATION, &0.1f32.to_be_bytes()),
            ]
            .concat(),
        );
        let header = read(&webm(&[info, opus_track(None)].concat())).unwrap();
        assert_eq!(header.ticks, 12_321_398);
    }

    #[test]
    fn a_duration_shorter_than_the_codec_delay_is_invalid() {
        let info = element(INFO, &element(DURATION, &1.0f64.to_be_bytes()));
        let header = read(&webm(&[info, opus_track(None)].concat()));
        assert_eq!(header, Err(Unreadable::invalid(BAD_HEADER)));
    }

    #[test]
    fn a_live_file_ends_at_its_last_block_plus_one_default_duration() {
        let cluster = [
            open_element(CLUSTER),
            uint_element(CLUSTER_TIMESTAMP, 0),
            simple_block(0),
            simple_block(988),
        ]
        .concat();
        let second_cluster = [
            open_element(CLUSTER),
            uint_element(CLUSTER_TIMESTAMP, 1_000),
            simple_block(-12),
        ]
        .concat();
        let mut segment = open_element(SEGMENT);
        segment.extend([opus_track(Some(20_000_000)), cluster, second_cluster].concat());
        let bytes = [element(EBML_HEADER, b""), segment].concat();
        let header = read(&bytes).unwrap();
        // (1000 - 12) ms + 20 ms - 6.5 ms.
        assert_eq!(header.ticks, 1_001_500);
        assert_eq!(header.note, Some("duration_from_blocks"));
    }

    #[test]
    fn a_block_duration_beats_the_default_duration() {
        let group = element(
            BLOCK_GROUP,
            &[
                element(BLOCK, &[0x81, 0, 10, 0, 0]),
                uint_element(BLOCK_DURATION, 30),
            ]
            .concat(),
        );
        let cluster = element(
            CLUSTER,
            &[uint_element(CLUSTER_TIMESTAMP, 100), group].concat(),
        );
        let header = read(&webm(&[opus_track(Some(20_000_000)), cluster].concat())).unwrap();
        // (100 + 10 + 30) ms - 6.5 ms.
        assert_eq!(header.ticks, 133_500);
    }

    /// A live file whose last block, at 110 ms, has the given DiscardPadding.
    fn padded_last_block(discard_padding_ns: i32) -> Vec<u8> {
        let group = element(
            BLOCK_GROUP,
            &[
                element(BLOCK, &[0x81, 0, 10, 0, 0]),
                element(DISCARD_PADDING, &discard_padding_ns.to_be_bytes()),
            ]
            .concat(),
        );
        let cluster = element(
            CLUSTER,
            &[uint_element(CLUSTER_TIMESTAMP, 100), group].concat(),
        );
        webm(&[opus_track(Some(20_000_000)), cluster].concat())
    }

    #[test]
    fn discard_padding_shortens_a_last_block_of_default_duration() {
        let header = read(&padded_last_block(5_000_000)).unwrap();
        // (100 + 10 + 20 - 5) ms - 6.5 ms.
        assert_eq!(header.ticks, 118_500);
    }

    #[test]
    fn negative_discard_padding_leaves_the_end_alone() {
        let header = read(&padded_last_block(-5_000_000)).unwrap();
        // (100 + 10 + 20) ms - 6.5 ms.
        assert_eq!(header.ticks, 123_500);
    }

    #[test]
    fn a_last_block_too_late_to_count_is_invalid() {
        let cluster = element(
            CLUSTER,
            &[uint_element(CLUSTER_TIMESTAMP, u64::MAX), simple_block(0)].concat(),
        );
        let info = element(INFO, &uint_element(TIMESTAMP_SCALE, u64::MAX));
        let bytes = webm(&[info, opus_track(Some(20_000_000)), cluster].concat());
        assert_eq!(read(&bytes), Err(Unreadable::invalid(BAD_HEADER)));
    }

    #[test]
    fn a_sampling_frequency_that_is_not_a_number_keeps_the_default() {
        let audio = [
            uint_element(CHANNELS, 2),
            element(SAMPLING_FREQUENCY, &f64::NAN.to_be_bytes()),
        ]
        .concat();
        let info = element(INFO, &element(DURATION, &1_008.0f64.to_be_bytes()));
        let bytes = webm(&[info, opus_track_with_audio(None, &audio)].concat());
        let header = read(&bytes).unwrap();
        assert_eq!(header.sample_rate, 8_000);
    }

    #[test]
    fn a_segment_past_the_end_is_truncated() {
        // Every element left is whole, but the segment says more follows.
        let void = element(VOID, &[0; 4]);
        let info = element(INFO, &element(DURATION, &1_008.0f64.to_be_bytes()));
        let mut bytes = webm(&[info, opus_track(None), void.clone()].concat());
        bytes.truncate(bytes.len() - void.len());
        assert_eq!(read(&bytes), Err(Unreadable::invalid(TRUNCATED)));
    }

    #[test]
    fn a_video_track_is_invalid() {
        let video = element(
            TRACKS,
            &element(TRACK_ENTRY, &uint_element(TRACK_TYPE, TRACK_TYPE_VIDEO)),
        );
        assert_eq!(read(&webm(&video)), Err(Unreadable::invalid("video track")));
    }

    #[test]
    fn signed_integers_spread_their_sign() {
        assert_eq!(int(&[0xFF, 0xFE]), -2);
        assert_eq!(int(&[0x01, 0x00]), 256);
        assert_eq!(int(&[]), 0);
    }
}
