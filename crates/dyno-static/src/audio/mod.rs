//! Audio: decoded bytes at 48 and 24 kHz, stereo sound groups, silence in sprite maps, and what
//! decoding each sound shows.
//!
//! Our own readers take each file's channels and playable length from its headers (design
//! section 5). The length is in the container's own clock, so the formula needs no float. A sound
//! shipped in several formats counts once, at its sound group's representative. Sprite maps give
//! the time no sprite covers (section 6). `listen` decodes each group once with symphonia, to
//! measure its silence and say whether a stereo sound can be mono (section 5.4, R4).

mod adts;
mod groups;
pub mod listen;
mod mp3;
mod mp4;
mod ogg;
pub mod sprites;
mod wav;
mod webm;

use dyno_schema::ids::MeasureId;
use dyno_schema::manifest::FileKind;
use dyno_schema::measure::{
    Evidence, Measure, MeasureItem, MeasureStatus, MeasureValue, Method, Uncertainty,
    UncertaintyKind,
};
use dyno_schema::path::RelPath;
use dyno_schema::profile::HostProfile;

use crate::model::{
    AudioSummary, DecodedAudio, FamilyScan, HowlSite, Inventory, StereoSound, saturating_i64,
};

pub use groups::SoundGroup;
pub use listen::{Listening, StereoClass};
pub use sprites::{Silence, SpriteMap, SpriteMapKind, SpriteMapRow};

/// The file format a reader understood.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Wav,
    Ogg,
    Mp3,
    /// Raw AAC, one ADTS header per frame.
    Adts,
    /// ISO-BMFF: `.m4a` and `.mp4`.
    Mp4,
    /// WebM or Matroska.
    Webm,
}

/// The codec inside the container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Pcm,
    PcmFloat,
    Vorbis,
    Opus,
    Mp3,
    Aac,
    /// A WAV format tag that is not PCM or float, such as ADPCM. Its `fact` chunk gives the length.
    Other,
}

/// What a reader takes from one file's headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub container: Container,
    pub codec: Codec,
    pub channels: u32,
    /// The rate the codec runs at. The formula never reads it: the browser resamples.
    pub sample_rate: u32,
    /// The playable length, in `tick_rate` ticks a second.
    pub ticks: u64,
    /// The container's own clock, such as the sample rate, or microseconds for WebM.
    pub tick_rate: u64,
    /// How the length was found, when it is not the usual way, such as `streamed_wav`.
    pub note: Option<&'static str>,
}

/// Why a file has no header: it is damaged (`invalid`), or a format we do not model
/// (`unsupported`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable {
    pub status: MeasureStatus,
    pub reason: &'static str,
}

/// One audio file and what its header gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFile {
    pub path: RelPath,
    pub reading: Result<Decoded, Unreadable>,
}

/// A file whose header was read: the header, and its decoded bytes at the profile's two rates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub header: Header,
    pub decoded_bytes_48k: u64,
    pub decoded_bytes_24k: u64,
}

/// Everything the audio scan finds. The measures and the summary come from it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioReport {
    /// Every audio file, sorted by path.
    pub files: Vec<AudioFile>,
    /// Sorted by representative.
    pub groups: Vec<SoundGroup>,
    /// Sorted by source.
    pub sprite_maps: Vec<SpriteMapRow>,
    /// One per sound group, in the same order.
    pub heard: Vec<Heard>,
}

/// What decoding one sound group gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heard {
    pub representative: RelPath,
    /// The file decoded: the representative, or the first other file of the group that decodes,
    /// such as the MP3 of a sound whose representative is Opus.
    pub source: Option<RelPath>,
    /// The reason is the representative's.
    pub listening: Result<Listening, &'static str>,
}

/// The reader a file's first bytes or extension point to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Wav,
    Ogg,
    /// MPEG audio: MP3, or MP1 and MP2, which the MP3 reader turns down.
    Mpeg,
    Adts,
    Mp4,
    Webm,
    /// A format we know but do not model. The text is the reason.
    Unsupported(&'static str),
}

/// The measures this family owns.
pub const MEASURES: &[MeasureId] = &[
    MeasureId::StaticAudioDecoded48k,
    MeasureId::StaticAudioDecoded24k,
    MeasureId::StaticAudioStereoFiles,
    MeasureId::StaticAudioStereoMonoSafe,
    MeasureId::StaticAudioSpriteSilence,
    MeasureId::StaticAudioSilenceByDecode,
];

/// Why a file runs past its end, the most common reason a file is invalid.
pub(crate) const TRUNCATED: &str = "truncated";

/// Why a header holds a value no encoder writes.
pub(crate) const BAD_HEADER: &str = "header";

/// Each PCM sample is a 32-bit float once decoded.
const BYTES_PER_SAMPLE: u128 = 4;

/// Reads each audio file's header, puts the files in sound groups, measures the silence in each
/// sprite map, and decodes each group. The profile gives the two PCM rates.
pub fn scan(
    inventory: &Inventory,
    howl_sites: &[HowlSite],
    profile: &HostProfile,
) -> FamilyScan<AudioSummary> {
    let report = analyse(inventory, howl_sites, profile);
    FamilyScan {
        measures: report.measures(),
        summary: report.summary(),
    }
}

/// The audio scan's findings, before they become measures and a summary.
pub fn analyse(
    inventory: &Inventory,
    howl_sites: &[HowlSite],
    profile: &HostProfile,
) -> AudioReport {
    let files: Vec<AudioFile> = inventory
        .files_of_kind(FileKind::Audio)
        .map(|file| {
            let bytes = inventory.contents(&file.path).unwrap_or_default();
            AudioFile {
                path: file.path.clone(),
                reading: read_header(&file.path, bytes).map(|header| decode(header, profile)),
            }
        })
        .collect();
    let found_maps = sprites::find(inventory, howl_sites);
    let src_lists = sprites::src_lists(&found_maps);
    let groups = groups::sound_groups(&files, &src_lists);
    let sprite_maps = sprites::measure(found_maps, &files, &groups);
    let heard = groups
        .iter()
        .map(|group| hear(inventory, &files, group))
        .collect();
    AudioReport {
        files,
        groups,
        sprite_maps,
        heard,
    }
}

/// Decodes a group's representative, or else the first other file of the group that decodes.
fn hear(inventory: &Inventory, files: &[AudioFile], group: &SoundGroup) -> Heard {
    let members = std::iter::once(&group.representative).chain(
        group
            .paths
            .iter()
            .filter(|path| **path != group.representative),
    );
    let mut first_reason = None;
    for path in members {
        let Some(decoded) = files
            .iter()
            .find(|file| file.path == *path)
            .and_then(AudioFile::decoded)
        else {
            continue;
        };
        let bytes = inventory.contents(path).unwrap_or_default();
        match listen::listen(path, bytes, &decoded.header) {
            Ok(listening) => {
                return Heard {
                    representative: group.representative.clone(),
                    source: Some(path.clone()),
                    listening: Ok(listening),
                };
            }
            Err(reason) => {
                first_reason.get_or_insert(reason);
            }
        }
    }
    Heard {
        representative: group.representative.clone(),
        source: None,
        listening: Err(first_reason.unwrap_or(listen::CANNOT_DECODE)),
    }
}

/// Reads one audio file's header. The magic bytes pick the reader, and the extension picks it when
/// they match nothing, so a damaged `.mp3` still reaches the MP3 reader.
pub fn read_header(path: &RelPath, bytes: &[u8]) -> Result<Header, Unreadable> {
    let format = Format::from_magic(bytes)
        .or_else(|| Format::from_extension(path))
        .ok_or(Unreadable::unsupported("unknown audio format"))?;
    let header = match format {
        Format::Wav => wav::read(bytes),
        Format::Ogg => ogg::read(bytes),
        Format::Mpeg => mp3::read(bytes),
        Format::Adts => adts::read(bytes),
        Format::Mp4 => mp4::read(bytes),
        Format::Webm => webm::read(bytes),
        Format::Unsupported(reason) => Err(Unreadable::unsupported(reason)),
    }?;
    // A clock of 0 would divide by zero, and no stream has 0 channels.
    if header.tick_rate == 0 || header.channels == 0 {
        return Err(Unreadable::invalid(BAD_HEADER));
    }
    Ok(header)
}

/// `ceil(ticks * rate / tick_rate) * channels * 4`: the 32-bit float PCM that `decodeAudioData`
/// returns at the context `rate`, one buffer per channel (design section 5.1).
pub fn decoded_bytes(header: &Header, rate_hz: u32) -> u64 {
    let frames = (u128::from(header.ticks) * u128::from(rate_hz))
        .div_ceil(u128::from(header.tick_rate.max(1)));
    let bytes = frames * u128::from(header.channels) * BYTES_PER_SAMPLE;
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

impl Container {
    /// The name in `expected.toml`.
    pub fn name(self) -> &'static str {
        match self {
            Container::Wav => "wav",
            Container::Ogg => "ogg",
            Container::Mp3 => "mp3",
            Container::Adts => "adts",
            Container::Mp4 => "mp4",
            Container::Webm => "webm",
        }
    }
}

impl Codec {
    /// The name in `expected.toml`.
    pub fn name(self) -> &'static str {
        match self {
            Codec::Pcm => "pcm",
            Codec::PcmFloat => "pcm_float",
            Codec::Vorbis => "vorbis",
            Codec::Opus => "opus",
            Codec::Mp3 => "mp3",
            Codec::Aac => "aac",
            Codec::Other => "other",
        }
    }
}

impl Unreadable {
    pub(crate) const fn invalid(reason: &'static str) -> Unreadable {
        Unreadable {
            status: MeasureStatus::Invalid,
            reason,
        }
    }

    pub(crate) const fn unsupported(reason: &'static str) -> Unreadable {
        Unreadable {
            status: MeasureStatus::Unsupported,
            reason,
        }
    }

    /// A file the scan needs is not in the build, or has no header.
    pub(crate) const fn missing(reason: &'static str) -> Unreadable {
        Unreadable {
            status: MeasureStatus::Missing,
            reason,
        }
    }
}

impl AudioFile {
    /// The file's own decoded bytes, when its header was read.
    pub fn decoded(&self) -> Option<&Decoded> {
        self.reading.as_ref().ok()
    }
}

impl AudioReport {
    /// The total over sound groups at the profile's PCM rate.
    pub fn decoded_bytes_48k(&self) -> u64 {
        total(self.groups.iter().map(|group| group.decoded_bytes_48k))
    }

    /// The total over sound groups at the profile's low PCM rate.
    pub fn decoded_bytes_24k(&self) -> u64 {
        total(self.groups.iter().map(|group| group.decoded_bytes_24k))
    }

    /// Audio files whose header was not read, sorted.
    pub fn not_covered(&self) -> impl Iterator<Item = (&RelPath, &Unreadable)> {
        self.files
            .iter()
            .filter_map(|file| file.reading.as_ref().err().map(|why| (&file.path, why)))
    }

    /// The groups with two or more channels.
    pub fn stereo_groups(&self) -> impl Iterator<Item = &SoundGroup> {
        self.groups.iter().filter(|group| group.is_stereo())
    }

    /// What exporting every stereo group as mono saves at the profile's PCM rate.
    pub fn mono_saving_48k(&self) -> u64 {
        total(self.stereo_groups().map(SoundGroup::mono_saving_48k))
    }

    /// The summary that later steps read.
    pub fn summary(&self) -> AudioSummary {
        let files = self
            .files
            .iter()
            .filter_map(|file| {
                let decoded = file.decoded()?;
                Some(DecodedAudio {
                    path: file.path.clone(),
                    decoded_bytes_48k: decoded.decoded_bytes_48k,
                })
            })
            .collect();
        let stereo_groups = self
            .stereo_groups()
            .map(|group| StereoSound {
                path: group.representative.clone(),
                decoded_bytes_48k: group.decoded_bytes_48k,
                class: self.stereo_class(&group.representative),
            })
            .collect();
        AudioSummary {
            decoded_bytes_48k: self.decoded_bytes_48k(),
            decoded_bytes_24k: self.decoded_bytes_24k(),
            files,
            stereo_groups,
            sprite_maps: self.sprite_maps.iter().map(SpriteMapRow::summary).collect(),
            not_covered: self.not_covered().map(|(path, _)| path.clone()).collect(),
        }
    }

    /// One measure per id in `MEASURES`.
    pub fn measures(&self) -> Vec<Measure> {
        vec![
            self.decoded_measure(MeasureId::StaticAudioDecoded48k, |group| {
                group.decoded_bytes_48k
            }),
            self.decoded_measure(MeasureId::StaticAudioDecoded24k, |group| {
                group.decoded_bytes_24k
            }),
            self.stereo_measure(),
            self.mono_safe_measure(),
            self.silence_measure(),
            self.silence_by_decode_measure(),
        ]
    }

    /// What decoding the group with this representative gives, when it decoded.
    pub fn listening(&self, representative: &RelPath) -> Option<&Listening> {
        let heard = self
            .heard
            .iter()
            .find(|heard| heard.representative == *representative)?;
        heard.listening.as_ref().ok()
    }

    /// What a mono export does to the group with this representative, when it decoded.
    pub fn stereo_class(&self, representative: &RelPath) -> Option<StereoClass> {
        self.listening(representative)?.stereo
    }

    /// The groups that did not decode, with the reason, and the files with no header.
    fn not_heard(&self, groups: impl Iterator<Item = RelPath>) -> Vec<String> {
        let unheard = groups.filter_map(|representative| {
            let heard = self
                .heard
                .iter()
                .find(|heard| heard.representative == representative)?;
            let reason = heard.listening.as_ref().err()?;
            Some(format!("{representative}: not decoded ({reason})"))
        });
        let unread = self
            .not_covered()
            .map(|(path, why)| format!("{path}: {} ({})", status_word(why.status), why.reason));
        unheard.chain(unread).collect()
    }

    /// The decoded stereo groups whose channels match, one item per decoded stereo group with
    /// its class.
    fn mono_safe_measure(&self) -> Measure {
        let classes: Vec<(&RelPath, StereoClass)> = self
            .stereo_groups()
            .filter_map(|group| {
                let class = self.stereo_class(&group.representative)?;
                Some((&group.representative, class))
            })
            .collect();
        let safe = classes
            .iter()
            .filter(|(_, class)| *class == StereoClass::SafeToMakeMono)
            .count();
        let items = classes
            .iter()
            .map(|(path, class)| MeasureItem {
                key: path.as_str().to_owned(),
                value: MeasureValue::Text(class.name().to_owned()),
            })
            .collect();
        let mut measure = Measure {
            value: Some(MeasureValue::Integer(saturating_i64(safe as u64))),
            items,
            evidence: vec![self.decode_evidence()],
            uncertainty: exact(Some(&format!(
                "safe to make mono: the difference between the channels is {} dB below what they \
                 share; cancels: a mono mix keeps {}% of the energy or less",
                -listen::SAFE_SIDE_DB,
                listen::CANCEL_KEPT_SHARE * 100.0
            ))),
            ..Measure::blank(
                MeasureId::StaticAudioStereoMonoSafe,
                MeasureStatus::Estimated,
            )
        };
        let stereo = self
            .stereo_groups()
            .map(|group| group.representative.clone());
        measure.scope.not_covered = self.not_heard(stereo);
        measure.sort_lists();
        measure
    }

    /// The silent share of the time of every decoded group, one item per decoded group.
    fn silence_by_decode_measure(&self) -> Measure {
        let decoded = || {
            self.heard
                .iter()
                .filter_map(|heard| Some((&heard.representative, heard.listening.as_ref().ok()?)))
        };
        let total = decoded().fold(Listening::default(), |sum, (_, listening)| Listening {
            windows: sum.windows.saturating_add(listening.windows),
            silent_windows: sum.silent_windows.saturating_add(listening.silent_windows),
            stereo: None,
        });
        let items = decoded()
            .map(|(path, listening)| integer_item(path.as_str(), listening.silence_bp().into()))
            .collect();
        let mut measure = Measure {
            value: Some(MeasureValue::Integer(total.silence_bp().into())),
            items,
            evidence: vec![self.decode_evidence()],
            uncertainty: exact(Some(&format!(
                "10 ms windows at or below {} dBFS, over the time the header gives",
                listen::SILENCE_DBFS
            ))),
            ..Measure::blank(
                MeasureId::StaticAudioSilenceByDecode,
                MeasureStatus::Estimated,
            )
        };
        let groups = self.groups.iter().map(|group| group.representative.clone());
        measure.scope.not_covered = self.not_heard(groups);
        measure.sort_lists();
        measure
    }

    fn decode_evidence(&self) -> Evidence {
        let decoded = self
            .heard
            .iter()
            .filter(|heard| heard.listening.is_ok())
            .count();
        Evidence {
            method: Method::AudioDecode,
            detail: format!(
                "decoded {decoded} of {} sound groups with symphonia",
                self.heard.len()
            ),
        }
    }

    fn decoded_measure(&self, id: MeasureId, bytes: fn(&SoundGroup) -> u64) -> Measure {
        let items = self
            .groups
            .iter()
            .map(|group| integer_item(group.representative.as_str(), bytes(group)))
            .collect();
        let mut measure = self.file_measure(id, total(self.groups.iter().map(bytes)), items);
        measure.uncertainty = Uncertainty {
            kind: UncertaintyKind::Floor,
            note: Some("0 to 23% below what the browser holds".to_owned()),
            ..Uncertainty::unknown()
        };
        measure
    }

    fn stereo_measure(&self) -> Measure {
        let items: Vec<MeasureItem> = self
            .stereo_groups()
            .map(|group| integer_item(group.representative.as_str(), 1))
            .collect();
        let count = items.len() as u64;
        let mut measure = self.file_measure(MeasureId::StaticAudioStereoFiles, count, items);
        measure.uncertainty = exact(None);
        measure
    }

    /// An estimated measure over the audio files, with the files it does not cover.
    fn file_measure(&self, id: MeasureId, value: u64, items: Vec<MeasureItem>) -> Measure {
        let read_count = self.files.len();
        let mut evidence = vec![Evidence {
            method: Method::FileHeader,
            detail: format!("read the headers of {read_count} audio files"),
        }];
        evidence.extend(self.files.iter().filter_map(|file| {
            let note = file.decoded()?.header.note?;
            Some(Evidence {
                method: Method::FileHeader,
                detail: format!("{}: {note}", file.path),
            })
        }));
        let mut measure = Measure {
            value: Some(MeasureValue::Integer(saturating_i64(value))),
            items,
            evidence,
            ..Measure::blank(id, MeasureStatus::Estimated)
        };
        measure.scope.not_covered = self
            .not_covered()
            .map(|(path, why)| format!("{path}: {} ({})", status_word(why.status), why.reason))
            .collect();
        measure.sort_lists();
        measure
    }

    fn silence_measure(&self) -> Measure {
        let estimated = || {
            self.sprite_maps
                .iter()
                .filter_map(|map| Some((map, map.silence.as_ref()?)))
        };
        let largest = estimated().map(|(_, silence)| silence.silence_bp).max();
        let items = estimated()
            .map(|(map, silence)| integer_item(&map.key(), silence.silence_bp.into()))
            .collect();
        let mut measure = Measure {
            value: Some(MeasureValue::Integer(largest.unwrap_or(0).into())),
            items,
            evidence: vec![Evidence {
                method: Method::SpriteMap,
                detail: format!("read {} sprite maps", self.sprite_maps.len()),
            }],
            uncertainty: exact(Some(
                "time no sprite covers: an upper bound on what trimming saves",
            )),
            ..Measure::blank(
                MeasureId::StaticAudioSpriteSilence,
                MeasureStatus::Estimated,
            )
        };
        measure.scope.not_covered = self
            .sprite_maps
            .iter()
            .filter_map(|map| {
                let reason = map.reason?;
                Some(format!(
                    "{}: {} ({reason})",
                    map.key(),
                    status_word(map.status)
                ))
            })
            .collect();
        measure.sort_lists();
        measure
    }
}

impl Format {
    fn from_magic(bytes: &[u8]) -> Option<Format> {
        let at =
            |offset: usize, magic: &[u8]| bytes.get(offset..offset + magic.len()) == Some(magic);
        let format = if at(0, b"RIFF") && at(8, b"WAVE") {
            Format::Wav
        } else if at(0, b"RF64") {
            Format::Unsupported("rf64")
        } else if at(0, b"OggS") {
            Format::Ogg
        } else if at(4, b"ftyp") {
            Format::Mp4
        } else if at(0, &[0x1A, 0x45, 0xDF, 0xA3]) {
            Format::Webm
        } else if at(0, b"fLaC") {
            Format::Unsupported("flac")
        } else if at(0, b"caff") {
            Format::Unsupported("caf")
        } else if at(0, b"FORM") && (at(8, b"AIFF") || at(8, b"AIFC")) {
            Format::Unsupported("aiff")
        } else if at(0, b"ID3") || at(0, &[0xFF]) {
            Format::mpeg_or_adts(bytes)
        } else {
            return None;
        };
        Some(format)
    }

    fn from_extension(path: &RelPath) -> Option<Format> {
        let (_, extension) = path.as_str().rsplit_once('.')?;
        let format = match extension.to_ascii_lowercase().as_str() {
            "wav" => Format::Wav,
            "ogg" | "oga" | "opus" => Format::Ogg,
            "mp3" | "mp2" | "mpga" => Format::Mpeg,
            "aac" => Format::Adts,
            "m4a" | "mp4" => Format::Mp4,
            "webm" | "weba" | "mka" => Format::Webm,
            "flac" => Format::Unsupported("flac"),
            "caf" => Format::Unsupported("caf"),
            "aif" | "aiff" => Format::Unsupported("aiff"),
            _ => return None,
        };
        Some(format)
    }

    /// ADTS and MPEG audio share the frame sync. ADTS has layer bits 00, which MPEG audio reserves.
    fn mpeg_or_adts(bytes: &[u8]) -> Format {
        let start = mp3::skip_id3v2(bytes);
        if adts::starts_frame(bytes, start) {
            Format::Adts
        } else {
            Format::Mpeg
        }
    }
}

/// `N` bytes at `at`, or None when the file ends first.
pub(crate) fn array<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at.checked_add(N)?)?.try_into().ok()
}

/// A sum that stops at `u64::MAX` instead of overflowing.
fn total(values: impl Iterator<Item = u64>) -> u64 {
    values.fold(0, u64::saturating_add)
}

fn decode(header: Header, profile: &HostProfile) -> Decoded {
    Decoded {
        header,
        decoded_bytes_48k: decoded_bytes(&header, profile.pcm_rate_hz),
        decoded_bytes_24k: decoded_bytes(&header, profile.pcm_low_rate_hz),
    }
}

fn integer_item(key: &str, value: u64) -> MeasureItem {
    MeasureItem {
        key: key.to_owned(),
        value: MeasureValue::Integer(saturating_i64(value)),
    }
}

fn exact(note: Option<&str>) -> Uncertainty {
    Uncertainty {
        kind: UncertaintyKind::Exact,
        note: note.map(str::to_owned),
        ..Uncertainty::unknown()
    }
}

fn status_word(status: MeasureStatus) -> &'static str {
    match status {
        MeasureStatus::Measured => "measured",
        MeasureStatus::Estimated => "estimated",
        MeasureStatus::Unsupported => "unsupported",
        MeasureStatus::Missing => "missing",
        MeasureStatus::Invalid => "invalid",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(raw_path: &str, bytes: &[u8]) -> Result<Header, Unreadable> {
        read_header(&RelPath::new(raw_path).unwrap(), bytes)
    }

    #[test]
    fn formats_we_know_but_do_not_model_are_unsupported_with_their_name() {
        let cases: [(&str, &[u8], &str); 6] = [
            ("a.wav", b"RF64\0\0\0\0WAVE", "rf64"),
            ("a.bin", b"fLaC\0\0\0\x22", "flac"),
            ("a.bin", b"caff\0\x01\0\0", "caf"),
            ("a.bin", b"FORM\0\0\0\0AIFC", "aiff"),
            ("a.flac", b"", "flac"),
            ("a.xyz", b"\0\0\0\0", "unknown audio format"),
        ];
        for (path, bytes, reason) in cases {
            assert_eq!(
                read(path, bytes),
                Err(Unreadable::unsupported(reason)),
                "{path}"
            );
        }
    }

    #[test]
    fn the_first_bytes_pick_the_reader_before_the_extension() {
        // An Ogg page cut short, named .mp3: the Ogg reader calls it truncated.
        assert_eq!(
            read("a.mp3", b"OggS\0"),
            Err(Unreadable::invalid(TRUNCATED))
        );
        // Bytes no reader knows, named .mp3: the MP3 reader finds no frame.
        assert_eq!(
            read("a.mp3", b"\0\0\0\0"),
            Err(Unreadable::invalid("no frame sync"))
        );
    }

    #[test]
    fn the_totals_stop_at_the_largest_u64_instead_of_overflowing() {
        let huge = |raw: &str| SoundGroup {
            paths: vec![RelPath::new(raw).unwrap()],
            representative: RelPath::new(raw).unwrap(),
            channels: 2,
            decoded_bytes_48k: u64::MAX,
            decoded_bytes_24k: u64::MAX,
        };
        let report = AudioReport {
            groups: vec![huge("a.wav"), huge("b.wav")],
            ..AudioReport::default()
        };
        assert_eq!(report.decoded_bytes_48k(), u64::MAX);
        assert_eq!(report.decoded_bytes_24k(), u64::MAX);
    }

    #[test]
    fn decoded_bytes_round_frames_up_at_each_rate() {
        // 1 tick at 44.1 kHz is 1.09 frames at 48 kHz and 0.54 at 24 kHz.
        let header = Header {
            container: Container::Wav,
            codec: Codec::Pcm,
            channels: 2,
            sample_rate: 44_100,
            ticks: 1,
            tick_rate: 44_100,
            note: None,
        };
        let bytes_per_frame = 2 * 4;
        assert_eq!(decoded_bytes(&header, 48_000), 2 * bytes_per_frame);
        assert_eq!(decoded_bytes(&header, 24_000), bytes_per_frame);
    }
}
