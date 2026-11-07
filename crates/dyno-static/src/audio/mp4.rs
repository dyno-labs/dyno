//! AAC in MP4 or M4A: the boxes of ISO/IEC 14496-12, the `esds` of 14496-14 and the
//! AudioSpecificConfig of 14496-3. The length comes from the edit list, which removes the AAC
//! priming, or from `mdhd` when there is no edit list.

use super::{BAD_HEADER, Codec, Container, Header, TRUNCATED, Unreadable, array};

/// One box: its four-character type and its body.
struct Mp4Box<'a> {
    kind: [u8; 4],
    body: &'a [u8],
}

/// A box that runs past the end of its parent.
struct PastTheEnd;

/// What `moov` holds: the movie timescale, which edit lists use, and the first sound track.
struct Movie<'a> {
    timescale: Option<u32>,
    sound_track: Option<Track<'a>>,
}

struct Track<'a> {
    /// The sum of the edit list's segments that play media. None without an edit list.
    edit_duration: Option<u64>,
    /// Where the first segment that plays media starts, in the media timescale: the AAC priming
    /// the edit list leaves out. None without an edit list or such a segment.
    edit_start: Option<u64>,
    /// How many of the edit list's segments play media. 0 without an edit list.
    media_edits: u32,
    media_timescale: u32,
    media_duration: u64,
    entry: SampleEntry<'a>,
}

/// The first sample entry in `stsd`, such as `mp4a`.
struct SampleEntry<'a> {
    kind: [u8; 4],
    body: &'a [u8],
}

/// What the AudioSpecificConfig says.
struct AudioConfig {
    object_type: u32,
    sample_rate: u32,
    channel_configuration: u32,
}

/// Reads bits most significant first.
struct Bits<'a> {
    bytes: &'a [u8],
    /// In bits.
    at: usize,
}

const BOX_HEADER: usize = 8;
const LARGE_BOX_HEADER: usize = 16;

/// Version and flags open every full box.
const FULL_BOX_HEADER: usize = 4;

/// MPEG-4 audio, then the MPEG-2 AAC Main, LC and SSR profiles (`objectTypeIndication`).
const AAC_OBJECT_TYPES: [u8; 4] = [0x40, 0x66, 0x67, 0x68];

/// Parametric stereo: a mono core that plays as stereo.
const PS_OBJECT_TYPE: u32 = 29;
/// An escape: the object type continues in 6 more bits.
const OBJECT_TYPE_ESCAPE: u32 = 31;
/// An escape: the sample rate follows as 24 bits.
const SAMPLE_RATE_ESCAPE: usize = 15;

const SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

/// Where an audio sample entry's fields sit in its body.
const ENTRY_VERSION_AT: usize = 8;
const ENTRY_CHANNELS_AT: usize = 16;
/// A QuickTime version 2 sound description keeps a placeholder at 16, and the real channel count
/// as 32 bits here.
const ENTRY_V2_CHANNELS_AT: usize = 40;
const QUICKTIME_V2: u16 = 2;
/// Child boxes follow the fields, which QuickTime sound description versions 1 and 2 extend.
const ENTRY_CHILDREN_AT: [usize; 3] = [28, 44, 64];

const ES_DESCRIPTOR: u8 = 0x03;
const DECODER_CONFIG_DESCRIPTOR: u8 = 0x04;
const DECODER_SPECIFIC_INFO: u8 = 0x05;
/// `objectTypeIndication`, `streamType`, `bufferSizeDB`, `maxBitrate` and `avgBitrate`.
const DECODER_CONFIG_FIELDS: usize = 13;

/// ES_Descriptor flags (ISO/IEC 14496-1). Each set flag adds an optional field.
const STREAM_DEPENDENCE_FLAG: u8 = 0x80;
const URL_FLAG: u8 = 0x40;
const OCR_STREAM_FLAG: u8 = 0x20;

pub(super) fn read(bytes: &[u8]) -> Result<Header, Unreadable> {
    let (movie, track) = sound_track(bytes)?;
    let (ticks, tick_rate) = match track.edit_duration {
        Some(duration) => {
            let timescale = movie.timescale.ok_or(Unreadable::invalid(BAD_HEADER))?;
            (duration, timescale)
        }
        None => (track.media_duration, track.media_timescale),
    };
    let config = track.entry.audio_config()?;
    Ok(Header {
        container: Container::Mp4,
        codec: Codec::Aac,
        channels: config.channels(&track.entry)?,
        sample_rate: config.sample_rate,
        ticks,
        tick_rate: tick_rate.into(),
        note: None,
    })
}

/// What the edit list says about the frames a decoder gives before the playable part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LeadIn {
    /// No edit list, or none that plays media.
    Unknown,
    /// One segment plays media, from this many ticks, with this many ticks a second.
    Ticks(u64, u64),
    /// More than one segment plays media, so the playable part is not one range.
    SeveralSegments,
}

pub(super) fn lead_in(bytes: &[u8]) -> LeadIn {
    let Ok((_, track)) = sound_track(bytes) else {
        return LeadIn::Unknown;
    };
    match (track.media_edits, track.edit_start) {
        (1, Some(start)) => LeadIn::Ticks(start, track.media_timescale.into()),
        (0 | 1, _) => LeadIn::Unknown,
        _ => LeadIn::SeveralSegments,
    }
}

/// The movie and its first sound track.
fn sound_track(bytes: &[u8]) -> Result<(Movie<'_>, Track<'_>), Unreadable> {
    let mut moov = None;
    for top_box in boxes(bytes) {
        let Ok(top_box) = top_box else {
            // A file cut short before its `moov` has nothing to read, the usual end of a cut
            // upload when `moov` comes last.
            let reason = if moov.is_none() {
                "missing moov (truncated)"
            } else {
                TRUNCATED
            };
            return Err(Unreadable::invalid(reason));
        };
        if &top_box.kind == b"moov" && moov.is_none() {
            moov = Some(top_box.body);
        }
    }
    let moov = moov.ok_or(Unreadable::invalid("missing moov"))?;
    let mut movie = Movie::read(moov)?;
    let track = movie
        .sound_track
        .take()
        .ok_or(Unreadable::invalid("no soun track"))?;
    Ok((movie, track))
}

/// The boxes in `bytes`, in order. The last item is an error when a box runs past the end.
fn boxes(bytes: &[u8]) -> impl Iterator<Item = Result<Mp4Box<'_>, PastTheEnd>> {
    let mut at = 0;
    std::iter::from_fn(move || {
        if at >= bytes.len() {
            return None;
        }
        let next = box_at(bytes, at);
        at = match &next {
            Ok((_, end)) => *end,
            Err(PastTheEnd) => bytes.len(),
        };
        Some(next.map(|(next, _)| next))
    })
}

/// The box at `at` and where it ends. A size of 0 runs to the end of the parent, and a size of 1
/// means a 64-bit size follows the type.
fn box_at(bytes: &[u8], at: usize) -> Result<(Mp4Box<'_>, usize), PastTheEnd> {
    let size = array::<4>(bytes, at).ok_or(PastTheEnd)?;
    let kind = array::<4>(bytes, at + 4).ok_or(PastTheEnd)?;
    let (size, header) = match u32::from_be_bytes(size) {
        0 => ((bytes.len() - at) as u64, BOX_HEADER),
        1 => {
            let large = array::<8>(bytes, at + BOX_HEADER).ok_or(PastTheEnd)?;
            (u64::from_be_bytes(large), LARGE_BOX_HEADER)
        }
        size => (u64::from(size), BOX_HEADER),
    };
    let end = usize::try_from(size)
        .ok()
        .and_then(|size| at.checked_add(size))
        .filter(|&end| end <= bytes.len() && end >= at + header)
        .ok_or(PastTheEnd)?;
    let body = &bytes[at + header..end];
    Ok((Mp4Box { kind, body }, end))
}

impl<'a> Movie<'a> {
    fn read(moov: &'a [u8]) -> Result<Movie<'a>, Unreadable> {
        let mut movie = Movie {
            timescale: None,
            sound_track: None,
        };
        for child_box in boxes(moov) {
            let child_box = child_box.map_err(|PastTheEnd| Unreadable::invalid(TRUNCATED))?;
            match &child_box.kind {
                b"mvhd" => movie.timescale = Some(timescale_and_duration(child_box.body)?.0),
                // Fragments carry the samples, so `mdhd` and the edit list do not give the length.
                b"mvex" => return Err(Unreadable::unsupported("fragmented mp4")),
                b"trak" if movie.sound_track.is_none() => {
                    movie.sound_track = Track::read(child_box.body)?
                }
                _ => {}
            }
        }
        Ok(movie)
    }
}

impl<'a> Track<'a> {
    /// The track, when its handler is `soun`.
    fn read(trak: &'a [u8]) -> Result<Option<Track<'a>>, Unreadable> {
        let handler = descendant(trak, &[b"mdia", b"hdlr"])?
            .and_then(|hdlr| hdlr.get(FULL_BOX_HEADER + 4..FULL_BOX_HEADER + 8));
        if handler != Some(b"soun".as_slice()) {
            return Ok(None);
        }
        let mdhd = descendant(trak, &[b"mdia", b"mdhd"])?.ok_or(Unreadable::invalid(BAD_HEADER))?;
        let (media_timescale, media_duration) = timescale_and_duration(mdhd)?;
        let stsd = descendant(trak, &[b"mdia", b"minf", b"stbl", b"stsd"])?
            .ok_or(Unreadable::invalid(BAD_HEADER))?;
        let (edit_duration, edit_start, media_edits) = match descendant(trak, &[b"edts", b"elst"])?
        {
            Some(elst) => {
                let edits = media_edits(elst)?;
                (Some(edits.duration), edits.first_start, edits.count)
            }
            None => (None, None, 0),
        };
        Ok(Some(Track {
            edit_duration,
            edit_start,
            media_edits,
            media_timescale,
            media_duration,
            entry: SampleEntry::first(stsd)?,
        }))
    }
}

/// The box at the end of a path of children, such as `mdia/minf/stbl`.
fn descendant<'a>(parent: &'a [u8], path: &[&[u8; 4]]) -> Result<Option<&'a [u8]>, Unreadable> {
    let mut current = parent;
    for kind in path {
        match child(current, kind)? {
            Some(body) => current = body,
            None => return Ok(None),
        }
    }
    Ok(Some(current))
}

/// The first child of `kind`. An error when a child box runs past the end of its parent.
fn child<'a>(parent: &'a [u8], kind: &[u8; 4]) -> Result<Option<&'a [u8]>, Unreadable> {
    for child_box in boxes(parent) {
        let child_box = child_box.map_err(|PastTheEnd| Unreadable::invalid(TRUNCATED))?;
        if &child_box.kind == kind {
            return Ok(Some(child_box.body));
        }
    }
    Ok(None)
}

/// The timescale and duration of an `mvhd` or `mdhd`, which share their first fields.
fn timescale_and_duration(body: &[u8]) -> Result<(u32, u64), Unreadable> {
    let bad_header = Unreadable::invalid(BAD_HEADER);
    let version = *body.first().ok_or(bad_header)?;
    // Version 1 has 64-bit creation and modification times and duration.
    let (timescale_at, duration) = if version == 1 {
        let duration = array::<8>(body, 24).map(u64::from_be_bytes);
        (20, duration)
    } else {
        let duration = array::<4>(body, 16).map(|duration| u32::from_be_bytes(duration).into());
        (12, duration)
    };
    let timescale = array::<4>(body, timescale_at)
        .map(u32::from_be_bytes)
        .ok_or(bad_header)?;
    Ok((timescale, duration.ok_or(bad_header)?))
}

/// The entries of an edit list that play media (`media_time` 0 or more). An entry with
/// `media_time` -1 is an empty edit.
struct MediaEdits {
    /// The sum of their `segment_duration`, in the movie timescale.
    duration: u64,
    /// The first one's `media_time`, in the media timescale.
    first_start: Option<u64>,
    count: u32,
}

fn media_edits(elst: &[u8]) -> Result<MediaEdits, Unreadable> {
    let bad_header = Unreadable::invalid(BAD_HEADER);
    let version = *elst.first().ok_or(bad_header)?;
    let count = array::<4>(elst, FULL_BOX_HEADER)
        .map(u32::from_be_bytes)
        .ok_or(bad_header)?;
    let entry_size = if version == 1 { 20 } else { 12 };
    let mut edits = MediaEdits {
        duration: 0,
        first_start: None,
        count: 0,
    };
    for index in 0..count as usize {
        let at = FULL_BOX_HEADER + 4 + index * entry_size;
        let (duration, media_time) = if version == 1 {
            let duration = array::<8>(elst, at).map(u64::from_be_bytes);
            let media_time = array::<8>(elst, at + 8).map(i64::from_be_bytes);
            (duration, media_time)
        } else {
            let duration = array::<4>(elst, at).map(|bytes| u32::from_be_bytes(bytes).into());
            let media_time = array::<4>(elst, at + 4).map(|bytes| i32::from_be_bytes(bytes).into());
            (duration, media_time)
        };
        let (Some(duration), Some(media_time)) = (duration, media_time) else {
            return Err(bad_header);
        };
        if media_time >= 0 {
            edits.duration = edits.duration.saturating_add(duration);
            edits.first_start = edits.first_start.or(u64::try_from(media_time).ok());
            edits.count = edits.count.saturating_add(1);
        }
    }
    Ok(edits)
}

impl<'a> SampleEntry<'a> {
    fn first(stsd: &'a [u8]) -> Result<SampleEntry<'a>, Unreadable> {
        let entries = stsd
            .get(FULL_BOX_HEADER + 4..)
            .ok_or(Unreadable::invalid(BAD_HEADER))?;
        match boxes(entries).next() {
            Some(Ok(entry)) => Ok(SampleEntry {
                kind: entry.kind,
                body: entry.body,
            }),
            Some(Err(PastTheEnd)) => Err(Unreadable::invalid(TRUNCATED)),
            None => Err(Unreadable::invalid(BAD_HEADER)),
        }
    }

    /// The AudioSpecificConfig of an AAC entry. Other codecs are unsupported.
    fn audio_config(&self) -> Result<AudioConfig, Unreadable> {
        match &self.kind {
            b"mp4a" => {}
            b"Opus" => return Err(Unreadable::unsupported("opus in mp4")),
            b"alac" => return Err(Unreadable::unsupported("alac in mp4")),
            _ => return Err(Unreadable::unsupported("mp4 codec")),
        }
        let esds = self.esds()?.ok_or(Unreadable::invalid("no esds"))?;
        let (object_type, specific_info) =
            decoder_config(esds).ok_or(Unreadable::invalid(BAD_HEADER))?;
        if !AAC_OBJECT_TYPES.contains(&object_type) {
            return Err(Unreadable::unsupported("mp4 codec"));
        }
        AudioConfig::read(specific_info).ok_or(Unreadable::invalid(BAD_HEADER))
    }

    /// The `esds` box among the entry's children, or inside a QuickTime `wave` box.
    fn esds(&self) -> Result<Option<&'a [u8]>, Unreadable> {
        let children_at = self
            .version()
            .and_then(|version| ENTRY_CHILDREN_AT.get(usize::from(version)))
            .ok_or(Unreadable::invalid(BAD_HEADER))?;
        let children = self
            .body
            .get(*children_at..)
            .ok_or(Unreadable::invalid(BAD_HEADER))?;
        if let Some(esds) = child(children, b"esds")? {
            return Ok(Some(esds));
        }
        match child(children, b"wave")? {
            Some(wave) => child(wave, b"esds"),
            None => Ok(None),
        }
    }

    fn channel_count(&self) -> Option<u32> {
        if self.version()? == QUICKTIME_V2 {
            return array::<4>(self.body, ENTRY_V2_CHANNELS_AT).map(u32::from_be_bytes);
        }
        array::<2>(self.body, ENTRY_CHANNELS_AT).map(|count| u16::from_be_bytes(count).into())
    }

    /// The QuickTime sound description version. An ISO entry keeps 0 here.
    fn version(&self) -> Option<u16> {
        array::<2>(self.body, ENTRY_VERSION_AT).map(u16::from_be_bytes)
    }
}

/// The `objectTypeIndication` and the DecoderSpecificInfo inside an `esds` body.
fn decoder_config(esds: &[u8]) -> Option<(u8, &[u8])> {
    let (tag, es, _) = descriptor(esds.get(FULL_BOX_HEADER..)?)?;
    if tag != ES_DESCRIPTOR {
        return None;
    }
    // A 2-byte ES_ID, then the flags.
    let flags = *es.get(2)?;
    let mut at = 3;
    if flags & STREAM_DEPENDENCE_FLAG != 0 {
        at += 2;
    }
    if flags & URL_FLAG != 0 {
        at += 1 + usize::from(*es.get(at)?);
    }
    if flags & OCR_STREAM_FLAG != 0 {
        at += 2;
    }
    let mut rest = es.get(at..)?;
    while let Some((tag, body, after)) = descriptor(rest) {
        if tag == DECODER_CONFIG_DESCRIPTOR {
            let object_type = *body.first()?;
            let (tag, specific_info, _) = descriptor(body.get(DECODER_CONFIG_FIELDS..)?)?;
            return (tag == DECODER_SPECIFIC_INFO).then_some((object_type, specific_info));
        }
        rest = after;
    }
    None
}

/// One descriptor: its tag, its body and the bytes after it. The size takes 1 to 4 bytes of 7
/// bits, each but the last with its top bit set.
fn descriptor(bytes: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *bytes.first()?;
    let mut size = 0usize;
    let mut at = 1;
    loop {
        let byte = *bytes.get(at)?;
        size = (size << 7) | usize::from(byte & 0x7F);
        at += 1;
        if byte & 0x80 == 0 || at == 5 {
            break;
        }
    }
    let body = bytes.get(at..at.checked_add(size)?)?;
    Some((tag, body, &bytes[at + size..]))
}

impl AudioConfig {
    fn read(bytes: &[u8]) -> Option<AudioConfig> {
        let mut bits = Bits { bytes, at: 0 };
        let mut object_type = bits.take(5)?;
        if object_type == OBJECT_TYPE_ESCAPE {
            object_type = 32 + bits.take(6)?;
        }
        let rate_index = bits.take(4)? as usize;
        let sample_rate = if rate_index == SAMPLE_RATE_ESCAPE {
            bits.take(24)?
        } else {
            *SAMPLE_RATES.get(rate_index)?
        };
        Some(AudioConfig {
            object_type,
            sample_rate,
            channel_configuration: bits.take(4)?,
        })
    }

    fn channels(&self, entry: &SampleEntry<'_>) -> Result<u32, Unreadable> {
        match (self.object_type, self.channel_configuration) {
            (PS_OBJECT_TYPE, _) => Ok(2),
            // 0: a program config element says, and the sample entry repeats it.
            (_, 0) => entry.channel_count().ok_or(Unreadable::invalid(BAD_HEADER)),
            (_, configuration @ 1..=6) => Ok(configuration),
            // 7.1: eight channels.
            (_, 7) => Ok(8),
            _ => Err(Unreadable::unsupported("aac channel configuration")),
        }
    }
}

impl Bits<'_> {
    fn take(&mut self, count: usize) -> Option<u32> {
        let mut value = 0;
        for _ in 0..count {
            let byte = self.bytes.get(self.at / 8)?;
            let bit = (byte >> (7 - self.at % 8)) & 1;
            value = (value << 1) | u32::from(bit);
            self.at += 1;
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut bytes = ((BOX_HEADER + body.len()) as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(body);
        bytes
    }

    fn full_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        mp4_box(kind, &[&[0, 0, 0, 0], body].concat())
    }

    /// Version 0 `mvhd` or `mdhd` fields up to the duration.
    fn header_box(kind: &[u8; 4], timescale: u32, duration: u32) -> Vec<u8> {
        let mut body = vec![0; 8];
        body.extend_from_slice(&timescale.to_be_bytes());
        body.extend_from_slice(&duration.to_be_bytes());
        full_box(kind, &body)
    }

    /// An ISO `mp4a` entry whose `esds` holds an AudioSpecificConfig for AAC LC (object type 2)
    /// at 44.1 kHz (rate index 4).
    fn mp4a(channel_configuration: u8, entry_channels: u16, object_type: u8) -> Vec<u8> {
        let mut entry = vec![0; 16];
        entry.extend_from_slice(&entry_channels.to_be_bytes());
        entry.extend_from_slice(&[0; 10]);
        entry.extend(esds(channel_configuration, object_type));
        mp4_box(b"mp4a", &entry)
    }

    /// A QuickTime version 2 `mp4a` entry: 3 in the old channel field, the real count at 40.
    fn quicktime_v2_mp4a(channel_configuration: u8, entry_channels: u32) -> Vec<u8> {
        let mut entry = vec![0; 8];
        entry.extend_from_slice(&QUICKTIME_V2.to_be_bytes());
        entry.resize(ENTRY_CHANNELS_AT, 0);
        entry.extend_from_slice(&3u16.to_be_bytes());
        entry.resize(ENTRY_V2_CHANNELS_AT, 0);
        entry.extend_from_slice(&entry_channels.to_be_bytes());
        entry.resize(ENTRY_CHILDREN_AT[2], 0);
        entry.extend(esds(channel_configuration, 0x40));
        mp4_box(b"mp4a", &entry)
    }

    fn esds(channel_configuration: u8, object_type: u8) -> Vec<u8> {
        // 5 bits of object type, 4 of rate index, 4 of channel configuration, 3 of flags.
        let config = [(2 << 3) | (4 >> 1), channel_configuration << 3];
        let specific_info = [
            &[DECODER_SPECIFIC_INFO, config.len() as u8],
            config.as_slice(),
        ]
        .concat();
        let mut decoder_config = vec![object_type, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        decoder_config.extend(specific_info);
        let decoder_config = [
            &[DECODER_CONFIG_DESCRIPTOR, decoder_config.len() as u8],
            decoder_config.as_slice(),
        ]
        .concat();
        let es = [&[0, 1, 0], decoder_config.as_slice()].concat();
        let es = [&[ES_DESCRIPTOR, es.len() as u8], es.as_slice()].concat();
        full_box(b"esds", &es)
    }

    fn elst(entries: &[(u32, i32)]) -> Vec<u8> {
        let mut body = (entries.len() as u32).to_be_bytes().to_vec();
        for (duration, media_time) in entries {
            body.extend_from_slice(&duration.to_be_bytes());
            body.extend_from_slice(&media_time.to_be_bytes());
            body.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        }
        full_box(b"elst", &body)
    }

    fn trak(handler: &[u8; 4], edit_list: Option<Vec<u8>>, entry: Vec<u8>) -> Vec<u8> {
        let hdlr = full_box(b"hdlr", &[&[0; 4], handler.as_slice(), &[0; 12]].concat());
        let stsd = full_box(b"stsd", &[1u32.to_be_bytes().as_slice(), &entry].concat());
        let stbl = mp4_box(b"stbl", &stsd);
        let minf = mp4_box(b"minf", &stbl);
        let mdia = mp4_box(
            b"mdia",
            &[header_box(b"mdhd", 44_100, 45_124), hdlr, minf].concat(),
        );
        let edts = edit_list
            .map(|elst| mp4_box(b"edts", &elst))
            .unwrap_or_default();
        mp4_box(b"trak", &[edts, mdia].concat())
    }

    fn movie(traks: &[Vec<u8>]) -> Vec<u8> {
        let ftyp = mp4_box(b"ftyp", b"M4A \0\0\0\0");
        let moov = mp4_box(
            b"moov",
            &[vec![header_box(b"mvhd", 1_000, 1_000)], traks.to_vec()]
                .concat()
                .concat(),
        );
        [ftyp, moov, mp4_box(b"mdat", &[0; 32])].concat()
    }

    #[test]
    fn without_an_edit_list_the_length_is_the_media_duration() {
        let bytes = movie(&[trak(b"soun", None, mp4a(2, 2, 0x40))]);
        let header = read(&bytes).unwrap();
        assert_eq!(
            (header.channels, header.ticks, header.tick_rate),
            (2, 45_124, 44_100)
        );
        assert_eq!(header.sample_rate, 44_100);
    }

    #[test]
    fn an_edit_list_gives_the_length_in_the_movie_timescale() {
        let edits = elst(&[(200, -1), (1_000, 1_024)]);
        let bytes = movie(&[trak(b"soun", Some(edits), mp4a(1, 1, 0x40))]);
        let header = read(&bytes).unwrap();
        assert_eq!((header.ticks, header.tick_rate), (1_000, 1_000));
    }

    #[test]
    fn the_lead_in_is_the_media_edit_start_in_the_media_timescale() {
        let edits = elst(&[(200, -1), (1_000, 1_024)]);
        let bytes = movie(&[trak(b"soun", Some(edits), mp4a(1, 1, 0x40))]);
        assert_eq!(lead_in(&bytes), LeadIn::Ticks(1_024, 44_100));
        let no_edits = movie(&[trak(b"soun", None, mp4a(1, 1, 0x40))]);
        assert_eq!(lead_in(&no_edits), LeadIn::Unknown);
    }

    #[test]
    fn an_edit_list_that_plays_two_parts_of_the_media_has_no_one_lead_in() {
        let edits = elst(&[(1_000, 1_024), (500, 90_000)]);
        let bytes = movie(&[trak(b"soun", Some(edits), mp4a(1, 1, 0x40))]);
        assert_eq!(lead_in(&bytes), LeadIn::SeveralSegments);
        // The length still sums both segments.
        assert_eq!(read(&bytes).unwrap().ticks, 1_500);
    }

    #[test]
    fn the_first_sound_track_counts() {
        let bytes = movie(&[
            trak(b"vide", None, mp4a(1, 1, 0x40)),
            trak(b"soun", None, mp4a(2, 2, 0x40)),
            trak(b"soun", None, mp4a(1, 1, 0x40)),
        ]);
        assert_eq!(read(&bytes).unwrap().channels, 2);
    }

    #[test]
    fn channel_configuration_0_takes_the_sample_entry_count() {
        let bytes = movie(&[trak(b"soun", None, mp4a(0, 6, 0x40))]);
        assert_eq!(read(&bytes).unwrap().channels, 6);
    }

    #[test]
    fn a_quicktime_v2_entry_keeps_its_channel_count_at_40() {
        let bytes = movie(&[trak(b"soun", None, quicktime_v2_mp4a(0, 6))]);
        assert_eq!(read(&bytes).unwrap().channels, 6);
    }

    #[test]
    fn no_sound_track_is_invalid() {
        let bytes = movie(&[trak(b"vide", None, mp4a(1, 1, 0x40))]);
        assert_eq!(read(&bytes), Err(Unreadable::invalid("no soun track")));
    }

    #[test]
    fn mp3_in_mp4_and_other_entries_are_unsupported() {
        let mp3 = movie(&[trak(b"soun", None, mp4a(2, 2, 0x6B))]);
        let alac = movie(&[trak(b"soun", None, mp4_box(b"alac", &[0; 40]))]);
        let opus = movie(&[trak(b"soun", None, mp4_box(b"Opus", &[0; 40]))]);
        assert_eq!(read(&mp3), Err(Unreadable::unsupported("mp4 codec")));
        assert_eq!(read(&alac), Err(Unreadable::unsupported("alac in mp4")));
        assert_eq!(read(&opus), Err(Unreadable::unsupported("opus in mp4")));
    }

    fn ftyp_and_mdat() -> Vec<u8> {
        [
            mp4_box(b"ftyp", b"M4A \0\0\0\0"),
            mp4_box(b"mdat", &[0; 64]),
        ]
        .concat()
    }

    #[test]
    fn a_file_without_a_moov_is_invalid() {
        assert_eq!(
            read(&ftyp_and_mdat()),
            Err(Unreadable::invalid("missing moov"))
        );
    }

    #[test]
    fn a_file_cut_before_its_moov_is_invalid() {
        let mut bytes = ftyp_and_mdat();
        bytes.truncate(bytes.len() - 1);
        assert_eq!(
            read(&bytes),
            Err(Unreadable::invalid("missing moov (truncated)"))
        );
    }

    #[test]
    fn a_box_past_the_end_after_moov_is_truncated() {
        let mut bytes = movie(&[trak(b"soun", None, mp4a(2, 2, 0x40))]);
        bytes.truncate(bytes.len() - 1);
        assert_eq!(read(&bytes), Err(Unreadable::invalid(TRUNCATED)));
    }

    #[test]
    fn a_64_bit_size_and_a_size_of_0_both_reach_the_right_end() {
        let ftyp = mp4_box(b"ftyp", b"M4A \0\0\0\0");
        let mut large = 1u32.to_be_bytes().to_vec();
        large.extend_from_slice(b"free");
        large.extend_from_slice(&20u64.to_be_bytes());
        large.extend_from_slice(&[0; 4]);
        let moov = mp4_box(
            b"moov",
            &[
                header_box(b"mvhd", 1_000, 1_000),
                trak(b"soun", None, mp4a(1, 1, 0x40)),
            ]
            .concat(),
        );
        let mut to_the_end = 0u32.to_be_bytes().to_vec();
        to_the_end.extend_from_slice(b"mdat");
        to_the_end.extend_from_slice(&[0; 9]);
        let bytes = [ftyp, large, moov, to_the_end].concat();
        assert_eq!(read(&bytes).unwrap().ticks, 45_124);
    }
}
