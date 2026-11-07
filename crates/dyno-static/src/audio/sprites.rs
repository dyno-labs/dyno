//! Sprite maps and the time no sprite covers (design section 6).
//!
//! A Howler sprite is one audio file with named ranges. Howler decodes the whole file, so the gaps
//! between ranges cost decoded memory for nothing. A map comes from a Howl site that bundle found
//! in a script, or from a JSON file in one of three shapes.

use dyno_schema::measure::MeasureStatus;
use dyno_schema::path::RelPath;
use memchr::memmem;
use serde_json::Value;

use super::{AudioFile, Decoded, SoundGroup, Unreadable};
use crate::model::{HowlSite, Inventory, SpriteMapSummary, SpriteRange};

/// The shape a sprite map was written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpriteMapKind {
    /// A Howl site, or Howler JSON (`src` or `urls`, and `sprite`) in milliseconds.
    Howler,
    /// audiosprite's jukebox JSON (`resources` and `spritemap`) in seconds.
    Audiosprite,
}

/// One sprite map as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpriteMap {
    pub kind: SpriteMapKind,
    /// The `src` list as written, in its order.
    pub src: Vec<String>,
    pub sprites: Vec<SpriteRange>,
}

/// The time no sprite covers in a map's audio (design section 6.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Silence {
    /// The sound group's representative: the file the times are measured on.
    pub representative: RelPath,
    pub duration_us: u64,
    /// The union of the sprite ranges, clipped to the file.
    pub covered_us: u64,
    pub silence_us: u64,
    /// The silence as a share of the duration, in basis points.
    pub silence_bp: u32,
    /// The representative's decoded bytes at 48 kHz, times the silence share.
    pub silence_bytes_48k: u64,
}

/// One sprite map and its silence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpriteMapRow {
    /// The JSON file, or the script with the Howl site.
    pub source: RelPath,
    /// Where the Howl site starts in its script. None for a JSON file.
    pub offset: Option<u32>,
    pub status: MeasureStatus,
    /// Why the status is not estimated.
    pub reason: Option<&'static str>,
    /// None when the JSON file does not parse.
    pub map: Option<SpriteMap>,
    /// The `src` files that are in the build, in written order.
    pub audio: Vec<RelPath>,
    /// Some when the status is estimated.
    pub silence: Option<Silence>,
}

/// A sprite map, or a `src` list with no sprites, before its silence is measured.
pub(super) struct FoundMap {
    source: RelPath,
    offset: Option<u32>,
    /// The map, or why the JSON file is not readable.
    map: Result<SpriteMap, Unreadable>,
    /// The `src` files that are in the build, in written order.
    audio: Vec<RelPath>,
}

/// Finds a `src` path in the build.
struct Resolver<'a> {
    inventory: &'a Inventory,
    /// The folder of every HTML page, shallowest first, then the build root.
    page_folders: Vec<&'a str>,
}

/// Why a `.json` file that names a sprite does not parse.
const BAD_JSON: &str = "json";

/// Why a map's audio is missing: no `src` file is in the build.
const AUDIO_NOT_FOUND: &str = "audio not found";

/// Why a map's audio has no silence: its `src` files are in the build, but no header was read.
const AUDIO_NOT_READ: &str = "audio not readable";

/// Why a map's audio has no silence: it lasts under a microsecond.
const EMPTY_AUDIO: &str = "empty audio";

/// Howler JSON times are in milliseconds, jukebox JSON times in seconds.
const MICROS_PER_MILLI: u32 = 1_000;
const MICROS_PER_SECOND: u32 = 1_000_000;

const BASIS_POINTS: u128 = 10_000;

impl SpriteMapKind {
    /// The name in `expected.toml`.
    pub fn name(self) -> &'static str {
        match self {
            SpriteMapKind::Howler => "howler",
            SpriteMapKind::Audiosprite => "audiosprite",
        }
    }
}

impl SpriteMapRow {
    /// The map's key in measure items and `not_covered`: the source, and a Howl site's offset,
    /// since one script can hold several sites.
    pub fn key(&self) -> String {
        match self.offset {
            Some(offset) => format!("{}@{offset}", self.source),
            None => self.source.to_string(),
        }
    }

    /// The summary that rules read.
    pub fn summary(&self) -> SpriteMapSummary {
        let silence = self.silence.as_ref();
        SpriteMapSummary {
            source: self.source.clone(),
            status: self.status,
            audio: silence.map(|silence| silence.representative.clone()),
            silence_bp: silence.map_or(0, |silence| silence.silence_bp),
            silence_bytes_48k: silence.map_or(0, |silence| silence.silence_bytes_48k),
        }
    }
}

/// Every `.json` file in a sprite map shape, and every Howl site. Sorted by source, then by the
/// site's offset. A map with no sprites only joins its `src` files into one sound group.
pub(super) fn find(inventory: &Inventory, howl_sites: &[HowlSite]) -> Vec<FoundMap> {
    let json_maps = inventory
        .files()
        .iter()
        .filter(|file| has_json_extension(&file.path))
        .filter_map(|file| {
            let map = read_json(inventory.contents(&file.path)?)?;
            Some((&file.path, None, map))
        });
    let site_maps = howl_sites.iter().map(|site| {
        let map = SpriteMap {
            kind: SpriteMapKind::Howler,
            src: site.src.clone(),
            sprites: site.sprites.clone(),
        };
        (&site.file, Some(site.offset), Ok(map))
    });
    let mut found: Vec<_> = json_maps.chain(site_maps).collect();
    found.sort_by_key(|&(source, offset, _)| (source, offset));

    let resolver = Resolver::new(inventory);
    found
        .into_iter()
        .map(|(source, offset, map)| {
            let audio = match &map {
                Ok(map) => resolver.resolve_all(source, &map.src),
                Err(_) => Vec::new(),
            };
            FoundMap {
                source: source.clone(),
                offset,
                map,
                audio,
            }
        })
        .collect()
}

/// The `src` lists that join files into sound groups, one per map or Howl site.
pub(super) fn src_lists(found_maps: &[FoundMap]) -> Vec<Vec<RelPath>> {
    found_maps.iter().map(|found| found.audio.clone()).collect()
}

/// Measures each map's silence on its sound group's representative. A map with no sprites is
/// left out: it has nothing to measure.
pub(super) fn measure(
    found_maps: Vec<FoundMap>,
    files: &[AudioFile],
    groups: &[SoundGroup],
) -> Vec<SpriteMapRow> {
    found_maps
        .into_iter()
        .filter(|found| !matches!(&found.map, Ok(map) if map.sprites.is_empty()))
        .map(|found| {
            let measured = found
                .map
                .as_ref()
                .map_err(|why| *why)
                .and_then(|map| map_silence(map, &found.audio, files, groups));
            let (status, reason) = match &measured {
                Ok(_) => (MeasureStatus::Estimated, None),
                Err(why) => (why.status, Some(why.reason)),
            };
            SpriteMapRow {
                source: found.source,
                offset: found.offset,
                status,
                reason,
                map: found.map.ok(),
                audio: found.audio,
                silence: measured.ok(),
            }
        })
        .collect()
}

fn has_json_extension(path: &RelPath) -> bool {
    path.as_str()
        .rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("json"))
}

/// A `.json` file's sprite map. None when the file is not a sprite map, and invalid when it does
/// not parse but names a sprite.
fn read_json(bytes: &[u8]) -> Option<Result<SpriteMap, Unreadable>> {
    let names_a_sprite = |key: &str| memmem::find(bytes, key.as_bytes()).is_some();
    if !names_a_sprite("\"sprite\"") && !names_a_sprite("\"spritemap\"") {
        return None;
    }
    let text = bytes.strip_prefix("\u{FEFF}".as_bytes()).unwrap_or(bytes);
    match serde_json::from_slice::<Value>(text) {
        Ok(value) => sprite_map(&value).map(Ok),
        Err(_) => Some(Err(Unreadable::invalid(BAD_JSON))),
    }
}

/// The map in parsed JSON, if it has one of the three shapes. Other keys are allowed.
fn sprite_map(value: &Value) -> Option<SpriteMap> {
    let object = value.as_object()?;
    if let Some(sprite) = object.get("sprite") {
        // Howler 2 writes `src`, Howler 1 writes `urls`.
        let src = object.get("src").or_else(|| object.get("urls"))?;
        return Some(SpriteMap {
            kind: SpriteMapKind::Howler,
            src: strings(src)?,
            sprites: howler_ranges(sprite)?,
        });
    }
    Some(SpriteMap {
        kind: SpriteMapKind::Audiosprite,
        src: strings(object.get("resources")?)?,
        sprites: jukebox_ranges(object.get("spritemap")?)?,
    })
}

/// One string, or an array of strings.
fn strings(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::String(one) => Some(vec![one.clone()]),
        Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect(),
        _ => None,
    }
}

/// `{name: [offset, duration, loop?]}` in milliseconds.
fn howler_ranges(sprite: &Value) -> Option<Vec<SpriteRange>> {
    sprite
        .as_object()?
        .iter()
        .map(|(name, range)| {
            let range = range
                .as_array()
                .filter(|range| (2..=3).contains(&range.len()))?;
            let offset = range[0].as_f64()?;
            let duration = range[1].as_f64()?;
            Some(SpriteRange {
                name: name.clone(),
                start_us: micros(offset, MICROS_PER_MILLI)?,
                end_us: micros(offset + duration, MICROS_PER_MILLI)?,
                looped: range.get(2) == Some(&Value::Bool(true)),
            })
        })
        .collect()
}

/// `{name: {start, end, loop}}` in seconds.
fn jukebox_ranges(spritemap: &Value) -> Option<Vec<SpriteRange>> {
    spritemap
        .as_object()?
        .iter()
        .map(|(name, range)| {
            let seconds = |key: &str| range.get(key)?.as_f64();
            Some(SpriteRange {
                name: name.clone(),
                start_us: micros(seconds("start")?, MICROS_PER_SECOND)?,
                end_us: micros(seconds("end")?, MICROS_PER_SECOND)?,
                looped: range.get("loop") == Some(&Value::Bool(true)),
            })
        })
        .collect()
}

/// A time in microseconds, rounded. None when it is negative or not finite.
fn micros(amount: f64, micros_per_unit: u32) -> Option<u64> {
    let micros = (amount * f64::from(micros_per_unit)).round();
    (micros.is_finite() && micros >= 0.0).then_some(micros as u64)
}

impl<'a> Resolver<'a> {
    fn new(inventory: &'a Inventory) -> Resolver<'a> {
        let mut page_folders: Vec<&str> = inventory
            .files()
            .iter()
            .map(|file| file.path.as_str())
            .filter(|path| is_html(path))
            .map(|path| split_folder(path).0)
            .collect();
        page_folders.sort_by_key(|folder| (depth(folder), *folder));
        page_folders.dedup();
        if !page_folders.contains(&"") {
            page_folders.push("");
        }
        Resolver {
            inventory,
            page_folders,
        }
    }

    /// The paths in `src` that are in the build, in written order.
    fn resolve_all(&self, source: &RelPath, src: &[String]) -> Vec<RelPath> {
        src.iter()
            .filter_map(|path| self.resolve(source, path))
            .collect()
    }

    /// The first candidate that is in the build: `src` read from each page folder, then from the
    /// folder of the file that names it.
    fn resolve(&self, source: &RelPath, src: &str) -> Option<RelPath> {
        let src = url_path(src)?;
        let source_folder = split_folder(source.as_str()).0;
        self.page_folders
            .iter()
            .copied()
            .chain([source_folder])
            .filter_map(|folder| join(folder, &src))
            .find(|candidate| self.inventory.contents(candidate).is_some())
    }
}

fn is_html(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")
    })
}

/// How many folders deep a folder is. The build root is 0.
fn depth(folder: &str) -> usize {
    if folder.is_empty() {
        0
    } else {
        folder.matches('/').count() + 1
    }
}

/// A path's folder (empty at the root) and its file name.
fn split_folder(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// The path part of a `src` URL: without a query or fragment, and with `%XX` escapes decoded.
/// None when the escapes do not decode to UTF-8.
fn url_path(src: &str) -> Option<String> {
    let path = src.split(['?', '#']).next().unwrap_or_default();
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let escape = bytes
            .get(at + 1..at + 3)
            .filter(|hex| bytes[at] == b'%' && hex.iter().all(u8::is_ascii_hexdigit));
        match escape {
            Some(hex) => {
                let hex = std::str::from_utf8(hex).ok()?;
                decoded.push(u8::from_str_radix(hex, 16).ok()?);
                at += 3;
            }
            None => {
                decoded.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

/// `src` read from `folder`. `.` parts go and `..` parts climb, so `./a.mp3` is `a.mp3`. A path
/// from the site root starts at the build root. None for a path above the root.
fn join(folder: &str, src: &str) -> Option<RelPath> {
    let (start, src) = match src.strip_prefix('/') {
        Some(from_root) => ("", from_root),
        None => (folder, src),
    };
    let mut parts: Vec<&str> = start.split('/').filter(|part| !part.is_empty()).collect();
    for part in src.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    RelPath::new(&parts.join("/")).ok()
}

/// The silence of `map` on the representative of the sound group that holds its audio.
fn map_silence(
    map: &SpriteMap,
    audio: &[RelPath],
    files: &[AudioFile],
    groups: &[SoundGroup],
) -> Result<Silence, Unreadable> {
    if audio.is_empty() {
        return Err(Unreadable::missing(AUDIO_NOT_FOUND));
    }
    let (representative, decoded) =
        representative(audio, files, groups).ok_or_else(|| unread_audio(audio, files))?;
    silence(representative, decoded, &map.sprites)
}

/// The representative of the sound group that holds the map's audio, with its decoded bytes.
fn representative<'a>(
    audio: &[RelPath],
    files: &'a [AudioFile],
    groups: &'a [SoundGroup],
) -> Option<(&'a RelPath, &'a Decoded)> {
    let group = groups
        .iter()
        .find(|group| audio.iter().any(|path| group.paths.contains(path)))?;
    let file = files
        .iter()
        .find(|file| file.path == group.representative)?;
    Some((&file.path, file.decoded()?))
}

/// Why no `src` file has a header: the first audio file's status, or missing when no `src` file
/// is an audio file.
fn unread_audio(audio: &[RelPath], files: &[AudioFile]) -> Unreadable {
    let first_unreadable = audio.iter().find_map(|path| {
        let file = files.iter().find(|file| &file.path == path)?;
        file.reading.as_ref().err()
    });
    Unreadable {
        status: first_unreadable.map_or(MeasureStatus::Missing, |why| why.status),
        reason: AUDIO_NOT_READ,
    }
}

/// The design's section 6.2 formulas, in integers. Invalid when the audio lasts under a
/// microsecond, which leaves no share to take.
fn silence(
    representative: &RelPath,
    decoded: &Decoded,
    sprites: &[SpriteRange],
) -> Result<Silence, Unreadable> {
    let header = &decoded.header;
    let duration_us = u128::from(header.ticks) * u128::from(MICROS_PER_SECOND)
        / u128::from(header.tick_rate.max(1));
    let duration_us = u64::try_from(duration_us).unwrap_or(u64::MAX);
    if duration_us == 0 {
        return Err(Unreadable::invalid(EMPTY_AUDIO));
    }
    let covered_us = covered_us(sprites, duration_us);
    let silence_us = duration_us - covered_us;
    let share = |whole: u128| whole * u128::from(silence_us) / u128::from(duration_us);
    Ok(Silence {
        representative: representative.clone(),
        duration_us,
        covered_us,
        silence_us,
        silence_bp: u32::try_from(share(BASIS_POINTS)).unwrap_or(u32::MAX),
        silence_bytes_48k: u64::try_from(share(decoded.decoded_bytes_48k.into()))
            .unwrap_or(u64::MAX),
    })
}

/// The length of the union of the ranges `[start_us, end_us)`, each clipped to
/// `[0, duration_us]`.
fn covered_us(sprites: &[SpriteRange], duration_us: u64) -> u64 {
    let mut ranges: Vec<(u64, u64)> = sprites
        .iter()
        .map(|sprite| {
            (
                sprite.start_us.min(duration_us),
                sprite.end_us.min(duration_us),
            )
        })
        .filter(|(start, end)| start < end)
        .collect();
    ranges.sort_unstable();
    let mut covered = 0;
    let mut reached = 0;
    for (start, end) in ranges {
        let start = start.max(reached);
        if end > start {
            covered += end - start;
            reached = end;
        }
    }
    covered
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use dyno_schema::ids::MeasureId;
    use dyno_schema::manifest::{BuildManifest, FileKind, ManifestFile};

    use super::super::{AudioReport, Codec, Container, Header, groups};
    use super::*;

    fn path(raw: &str) -> RelPath {
        RelPath::new(raw).unwrap()
    }

    fn range(start_us: u64, end_us: u64) -> SpriteRange {
        SpriteRange {
            name: format!("{start_us}"),
            start_us,
            end_us,
            looped: false,
        }
    }

    /// An inventory of these files, each holding its text.
    fn inventory(files: &[(&str, &str)]) -> Inventory {
        let contents: BTreeMap<RelPath, Vec<u8>> = files
            .iter()
            .map(|(raw, text)| (path(raw), text.as_bytes().to_vec()))
            .collect();
        let manifest = BuildManifest {
            build_id: None,
            files: contents
                .iter()
                .map(|(path, bytes)| ManifestFile {
                    path: path.clone(),
                    bytes: bytes.len() as u64,
                    sha256: String::new(),
                    kind: FileKind::Other,
                })
                .collect(),
        };
        Inventory::new(manifest, contents)
    }

    /// A mono file of `ticks` microseconds whose header was read.
    fn audio_file(raw: &str, ticks: u64) -> AudioFile {
        let header = Header {
            container: Container::Webm,
            codec: Codec::Opus,
            channels: 1,
            sample_rate: 48_000,
            ticks,
            tick_rate: 1_000_000,
            note: None,
        };
        AudioFile {
            path: path(raw),
            reading: Ok(Decoded {
                header,
                decoded_bytes_48k: ticks * 48 / 1_000 * 4,
                decoded_bytes_24k: ticks * 24 / 1_000 * 4,
            }),
        }
    }

    /// The rows for the maps in `inventory` and `sites`, measured on `files`.
    fn rows(inventory: &Inventory, sites: &[HowlSite], files: &[AudioFile]) -> Vec<SpriteMapRow> {
        let found = find(inventory, sites);
        let groups = groups::sound_groups(files, &src_lists(&found));
        measure(found, files, &groups)
    }

    fn json_map(text: &str) -> Option<Result<SpriteMap, Unreadable>> {
        read_json(text.as_bytes())
    }

    /// Where `src`, named in `source`, resolves in a build of `files`.
    fn resolved(files: &[&str], source: &str, src: &str) -> Option<String> {
        let files: Vec<(&str, &str)> = files.iter().map(|raw| (*raw, "")).collect();
        let inventory = inventory(&files);
        Resolver::new(&inventory)
            .resolve(&path(source), src)
            .map(|found| found.to_string())
    }

    #[test]
    fn howler_json_gives_milliseconds_as_microseconds() {
        let text = r#"{"src": "a.mp3", "sprite": {"hit": [1500, 250.5, true]}, "extra": 1}"#;
        let map = json_map(text).unwrap().unwrap();
        assert_eq!(
            (map.kind, map.src),
            (SpriteMapKind::Howler, vec!["a.mp3".to_owned()])
        );
        let hit = &map.sprites[0];
        assert_eq!(
            (hit.start_us, hit.end_us, hit.looped),
            (1_500_000, 1_750_500, true)
        );
    }

    #[test]
    fn howler_1_json_writes_urls() {
        let text = r#"{"urls": ["a.ogg", "a.mp3"], "sprite": {"hit": [0, 100]}}"#;
        assert_eq!(json_map(text).unwrap().unwrap().src, ["a.ogg", "a.mp3"]);
    }

    #[test]
    fn jukebox_json_gives_seconds_as_microseconds() {
        let text = r#"{"resources": ["ui.ogg"], "spritemap": {"tap": {"start": 1.25, "end": 1.7499996, "loop": false}}}"#;
        let map = json_map(text).unwrap().unwrap();
        assert_eq!(map.kind, SpriteMapKind::Audiosprite);
        assert_eq!(
            (map.sprites[0].start_us, map.sprites[0].end_us),
            (1_250_000, 1_750_000)
        );
    }

    #[test]
    fn json_in_another_shape_is_not_a_sprite_map() {
        assert_eq!(json_map(r#"{"sprite": {"hit": [0, 100]}}"#), None);
        assert_eq!(
            json_map(r#"{"src": "a.mp3", "sprite": {"hit": {"start": 0}}}"#),
            None
        );
        assert_eq!(json_map(r#"{"frames": {"reel.png": {}}}"#), None);
    }

    #[test]
    fn json_that_names_a_sprite_but_does_not_parse_is_invalid() {
        let broken = json_map(r#"{"src": ["a.wav"], "sprite": {"a": [0, 100],"#);
        assert_eq!(broken, Some(Err(Unreadable::invalid(BAD_JSON))));
    }

    #[test]
    fn json_that_does_not_parse_and_names_no_sprite_is_not_a_sprite_map() {
        assert_eq!(json_map(r#"{"frames": "#), None);
    }

    #[test]
    fn an_empty_sprite_object_joins_its_src_files_but_is_no_map() {
        let inventory = inventory(&[
            ("map.json", r#"{"src": ["a.mp3", "b.ogg"], "sprite": {}}"#),
            ("a.mp3", ""),
            ("b.ogg", ""),
        ]);
        let found = find(&inventory, &[]);
        assert_eq!(src_lists(&found), [vec![path("a.mp3"), path("b.ogg")]]);
        assert_eq!(measure(found, &[], &[]), []);
    }

    #[test]
    fn src_resolves_from_the_page_folder_before_the_source_folder() {
        let files = ["game/index.html", "game/a.mp3", "game/sfx/a.mp3"];
        assert_eq!(
            resolved(&files, "game/sfx/map.json", "a.mp3").as_deref(),
            Some("game/a.mp3")
        );
    }

    #[test]
    fn src_falls_back_to_the_source_folder() {
        let files = ["game/index.html", "game/sfx/b.mp3"];
        assert_eq!(
            resolved(&files, "game/sfx/map.json", "b.mp3").as_deref(),
            Some("game/sfx/b.mp3")
        );
    }

    #[test]
    fn a_root_page_of_any_name_comes_before_a_deeper_index_html() {
        let files = [
            "game.html",
            "help/index.html",
            "audio/a.mp3",
            "help/audio/a.mp3",
        ];
        assert_eq!(
            resolved(&files, "js/main.js", "audio/a.mp3").as_deref(),
            Some("audio/a.mp3")
        );
    }

    #[test]
    fn a_page_of_any_name_in_a_subfolder_is_a_page_folder() {
        let files = ["game/play.htm", "game/a.mp3"];
        assert_eq!(
            resolved(&files, "game/js/main.js", "a.mp3").as_deref(),
            Some("game/a.mp3")
        );
    }

    #[test]
    fn a_deeper_page_folder_is_tried_after_a_root_launcher() {
        let files = ["index.html", "game/index.html", "game/sfx/a.mp3"];
        assert_eq!(
            resolved(&files, "game/js/main.js", "sfx/a.mp3").as_deref(),
            Some("game/sfx/a.mp3")
        );
    }

    #[test]
    fn dot_parts_go_and_dot_dot_parts_climb() {
        let files = ["a/b.mp3", "c.mp3"];
        assert_eq!(
            resolved(&files, "a/map.json", "./b.mp3").as_deref(),
            Some("a/b.mp3")
        );
        assert_eq!(
            resolved(&files, "a/map.json", "../c.mp3").as_deref(),
            Some("c.mp3")
        );
    }

    #[test]
    fn a_leading_slash_starts_at_the_build_root() {
        let files = ["c.mp3", "a/c.mp3"];
        assert_eq!(
            resolved(&files, "a/map.json", "/c.mp3").as_deref(),
            Some("c.mp3")
        );
    }

    #[test]
    fn a_path_above_the_build_root_resolves_to_nothing() {
        assert_eq!(resolved(&["c.mp3"], "a/map.json", "../../c.mp3"), None);
    }

    #[test]
    fn a_query_a_fragment_and_escapes_leave_the_src_path() {
        let files = ["sfx/a b.mp3"];
        assert_eq!(
            resolved(&files, "sfx/map.json", "a%20b.mp3?v=3#t=1").as_deref(),
            Some("sfx/a b.mp3")
        );
    }

    #[test]
    fn maps_are_sorted_by_source_then_offset() {
        let site = |file: &str, offset: u32| HowlSite {
            file: path(file),
            offset,
            src: Vec::new(),
            sprites: vec![SpriteRange {
                name: format!("{file}@{offset}"),
                ..range(0, 1)
            }],
        };
        let sites = [site("b.js", 0), site("a.js", 50), site("a.js", 10)];
        let found = find(&inventory(&[]), &sites);
        let names: Vec<&str> = found
            .iter()
            .map(|found| found.map.as_ref().unwrap().sprites[0].name.as_str())
            .collect();
        assert_eq!(names, ["a.js@10", "a.js@50", "b.js@0"]);
    }

    #[test]
    fn overlapping_ranges_count_once() {
        let overlapping = [range(0, 500), range(400, 1_000), range(2_000, 2_500)];
        assert_eq!(covered_us(&overlapping, 10_000), 1_500);
    }

    #[test]
    fn ranges_are_clipped_to_the_file() {
        let past_the_end = [range(9_000, 12_000), range(11_000, 13_000)];
        assert_eq!(covered_us(&past_the_end, 10_000), 1_000);
    }

    #[test]
    fn a_range_that_ends_before_it_starts_covers_nothing() {
        assert_eq!(covered_us(&[range(500, 100)], 10_000), 0);
    }

    #[test]
    fn silence_follows_section_6_2() {
        // The howler-sprites bundle map: 6.0015 s of stereo, sprites cover 2.25 s.
        let header = Header {
            container: Container::Webm,
            codec: Codec::Opus,
            channels: 2,
            sample_rate: 48_000,
            ticks: 6_001_500,
            tick_rate: 1_000_000,
            note: None,
        };
        let decoded = Decoded {
            header,
            decoded_bytes_48k: 2_304_576,
            decoded_bytes_24k: 1_152_288,
        };
        let sprites = [
            range(0, 500_000),
            range(1_500_000, 2_250_000),
            range(3_000_000, 4_000_000),
        ];
        let silence = silence(&path("sfx.webm"), &decoded, &sprites).unwrap();
        assert_eq!(
            (silence.duration_us, silence.covered_us, silence.silence_us),
            (6_001_500, 2_250_000, 3_751_500)
        );
        assert_eq!(
            (silence.silence_bp, silence.silence_bytes_48k),
            (6_250, 1_440_576)
        );
    }

    #[test]
    fn a_map_with_no_audio_in_the_build_is_missing() {
        let inventory = inventory(&[(
            "map.json",
            r#"{"src": "gone.mp3", "sprite": {"a": [0, 1]}}"#,
        )]);
        let rows = rows(&inventory, &[], &[]);
        assert_eq!(
            (rows[0].status, rows[0].reason),
            (MeasureStatus::Missing, Some(AUDIO_NOT_FOUND))
        );
    }

    #[test]
    fn a_missing_map_is_not_covered_by_the_silence_measure() {
        let inventory = inventory(&[(
            "map.json",
            r#"{"src": "gone.mp3", "sprite": {"a": [0, 1]}}"#,
        )]);
        let report = AudioReport {
            sprite_maps: rows(&inventory, &[], &[]),
            ..AudioReport::default()
        };
        let measures = report.measures();
        let silence = measures
            .iter()
            .find(|measure| measure.id == MeasureId::StaticAudioSpriteSilence)
            .unwrap();
        assert_eq!(
            silence.scope.not_covered,
            ["map.json: missing (audio not found)"]
        );
    }

    #[test]
    fn a_map_whose_audio_has_no_header_takes_the_file_status() {
        let inventory = inventory(&[
            ("map.json", r#"{"src": "a.mp3", "sprite": {"a": [0, 1]}}"#),
            ("a.mp3", ""),
        ]);
        let damaged = AudioFile {
            path: path("a.mp3"),
            reading: Err(Unreadable::invalid("truncated")),
        };
        let rows = rows(&inventory, &[], &[damaged]);
        assert_eq!(
            (rows[0].status, rows[0].reason),
            (MeasureStatus::Invalid, Some(AUDIO_NOT_READ))
        );
    }

    #[test]
    fn a_map_on_audio_of_no_length_is_invalid() {
        let inventory = inventory(&[
            ("map.json", r#"{"src": "a.mp3", "sprite": {"a": [0, 1]}}"#),
            ("a.mp3", ""),
        ]);
        let rows = rows(&inventory, &[], &[audio_file("a.mp3", 0)]);
        assert_eq!(
            (rows[0].status, rows[0].reason),
            (MeasureStatus::Invalid, Some(EMPTY_AUDIO))
        );
    }

    #[test]
    fn two_howl_sites_in_one_script_have_their_own_keys() {
        let inventory = inventory(&[("a.wav", "")]);
        let site = |offset: u32| HowlSite {
            file: path("main.js"),
            offset,
            src: vec!["a.wav".to_owned()],
            sprites: vec![range(0, 250_000)],
        };
        let files = [audio_file("a.wav", 1_000_000)];
        let report = AudioReport {
            sprite_maps: rows(&inventory, &[site(50), site(10)], &files),
            ..AudioReport::default()
        };
        let measures = report.measures();
        let silence = measures
            .iter()
            .find(|measure| measure.id == MeasureId::StaticAudioSpriteSilence)
            .unwrap();
        let keys: Vec<&str> = silence.items.iter().map(|item| item.key.as_str()).collect();
        assert_eq!(keys, ["main.js@10", "main.js@50"]);
    }
}
