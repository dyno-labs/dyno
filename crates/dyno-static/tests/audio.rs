//! Audio: every fixture's `[audio]` table, the audio measures, and a cross-check of our header
//! readers against symphonia (design sections 5 and 11).
//!
//! Bundle's Howl sites only join files that already share a folder and stem in these fixtures, so
//! these tests pass none. `tests/sprites.rs` passes the one in `howler-sprites`.

mod common;

use std::io::Cursor;

use common::{Fixture, fixtures, integer, non_negative, text};
use dyno_schema::ids::MeasureId;
use dyno_schema::measure::{Measure, MeasureStatus, MeasureValue, UncertaintyKind};
use dyno_schema::path::RelPath;
use dyno_static::audio::{self, AudioFile, AudioReport, Codec, Container, Heard, SoundGroup};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use toml::{Table, Value};

/// The fixtures with an `[audio]` table, each with its report. `zip-input` is left out: it is
/// `duplicates` zipped, and the fixture loader reads folders only.
fn reports() -> Vec<(Fixture, AudioReport)> {
    fixtures()
        .into_iter()
        .filter(|fixture| fixture.table("audio").is_some() && !fixture.is_zip())
        .map(|fixture| {
            let report = audio::analyse(&fixture.inventory(), &[], &fixture.profile());
            (fixture, report)
        })
        .collect()
}

/// The `[[audio.<name>]]` rows of a fixture.
fn rows<'a>(fixture: &'a Fixture, name: &str) -> Vec<&'a Table> {
    let audio = fixture.required_table("audio");
    audio
        .get(name)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{}: [[audio.{name}]] rows", fixture.name))
        .iter()
        .map(|row| row.as_table().expect("a row is a table"))
        .collect()
}

/// `actual` with only the keys `expected` has. Rows leave out what a fixture does not test.
fn keys_of(expected: &Table, actual: &Table) -> Table {
    expected
        .keys()
        .filter_map(|key| Some((key.clone(), actual.get(key)?.clone())))
        .collect()
}

fn int(value: impl Into<u64>) -> Value {
    Value::Integer(i64::try_from(value.into()).expect("fits a TOML integer"))
}

fn status(status: MeasureStatus) -> Value {
    Value::try_from(status).expect("a status is a string")
}

fn paths(paths: &[RelPath]) -> Value {
    Value::Array(paths.iter().map(|path| path.as_str().into()).collect())
}

/// One file as `expected.toml` writes it.
fn file_row(file: &AudioFile) -> Table {
    let mut row = Table::new();
    row.insert("path".into(), file.path.as_str().into());
    match &file.reading {
        Ok(decoded) => {
            let header = &decoded.header;
            row.insert("status".into(), status(MeasureStatus::Estimated));
            row.insert("container".into(), header.container.name().into());
            row.insert("codec".into(), header.codec.name().into());
            row.insert("channels".into(), int(header.channels));
            row.insert("ticks".into(), int(header.ticks));
            row.insert("tick_rate".into(), int(header.tick_rate));
            row.insert("decoded_bytes_48k".into(), int(decoded.decoded_bytes_48k));
            row.insert("decoded_bytes_24k".into(), int(decoded.decoded_bytes_24k));
        }
        Err(why) => {
            row.insert("status".into(), status(why.status));
            row.insert("reason".into(), why.reason.into());
        }
    }
    row
}

/// One sound group as `expected.toml` writes it.
fn group_row(group: &SoundGroup) -> Table {
    let mut row = Table::new();
    row.insert("paths".into(), paths(&group.paths));
    row.insert(
        "representative".into(),
        group.representative.as_str().into(),
    );
    row.insert("channels".into(), int(group.channels));
    row.insert("decoded_bytes_48k".into(), int(group.decoded_bytes_48k));
    row.insert("decoded_bytes_24k".into(), int(group.decoded_bytes_24k));
    row
}

#[test]
fn every_audio_file_reads_as_its_fixture_row() {
    for (fixture, report) in reports() {
        let expected = rows(&fixture, "file");
        let actual: Vec<Table> = report.files.iter().map(file_row).collect();
        let listed = |rows: &[&Table]| -> Vec<String> {
            rows.iter()
                .map(|row| text(row, "path").to_owned())
                .collect()
        };
        assert_eq!(
            listed(&actual.iter().collect::<Vec<_>>()),
            listed(&expected),
            "{}: the audio files",
            fixture.name
        );
        for (expected, actual) in expected.into_iter().zip(&actual) {
            assert_eq!(&keys_of(expected, actual), expected, "{}", fixture.name);
        }
    }
}

#[test]
fn every_sound_group_counts_once_at_its_representative() {
    for (fixture, report) in reports() {
        let mut expected = rows(&fixture, "group");
        // The fixtures list groups by stem; the report sorts them by representative.
        expected.sort_by_key(|row| text(row, "representative").to_owned());
        assert_eq!(
            report.groups.len(),
            expected.len(),
            "{}: groups",
            fixture.name
        );
        for (expected, group) in expected.into_iter().zip(&report.groups) {
            // `every_sound_group_decodes_as_its_fixture_row` checks what decoding gives.
            let mut expected = expected.clone();
            expected.retain(|key, _| !HEARD_KEYS.contains(&key));
            assert_eq!(
                keys_of(&expected, &group_row(group)),
                expected,
                "{}",
                fixture.name
            );
        }
    }
}

/// The `[audio.total]` table of a fixture.
fn total(fixture: &Fixture) -> &Table {
    fixture.required_table("audio")["total"]
        .as_table()
        .expect("[audio.total]")
}

/// The `[[audio.group]]` rows with two or more channels, sorted by representative.
fn stereo_rows(fixture: &Fixture) -> Vec<&Table> {
    let mut stereo: Vec<&Table> = rows(fixture, "group")
        .into_iter()
        .filter(|row| integer(row, "channels") >= 2)
        .collect();
    stereo.sort_by_key(|row| text(row, "representative").to_owned());
    stereo
}

fn measure(measures: &[Measure], id: MeasureId) -> &Measure {
    measures
        .iter()
        .find(|measure| measure.id == id)
        .unwrap_or_else(|| panic!("{} is measured", id.as_str()))
}

fn integer_value(measure: &Measure) -> u64 {
    match measure.value {
        Some(MeasureValue::Integer(value)) => non_negative(value),
        ref other => panic!(
            "{} has an integer value, not {other:?}",
            measure.id.as_str()
        ),
    }
}

/// A measure's items as (key, integer value).
fn integer_items(measure: &Measure) -> Vec<(String, u64)> {
    measure
        .items
        .iter()
        .map(|item| match item.value {
            MeasureValue::Integer(value) => (item.key.clone(), non_negative(value)),
            ref other => panic!("{}: {other:?} is not an integer", item.key),
        })
        .collect()
}

/// The two decoded measures, each with its `[audio.total]` and `[[audio.group]]` key.
const DECODED: [(MeasureId, &str); 2] = [
    (MeasureId::StaticAudioDecoded48k, "decoded_bytes_48k"),
    (MeasureId::StaticAudioDecoded24k, "decoded_bytes_24k"),
];

#[test]
fn the_report_totals_match_the_fixture_totals() {
    for (fixture, report) in reports() {
        let total = total(&fixture);
        let name = &fixture.name;
        assert_eq!(
            report.decoded_bytes_48k(),
            integer(total, "decoded_bytes_48k"),
            "{name}"
        );
        assert_eq!(
            report.decoded_bytes_24k(),
            integer(total, "decoded_bytes_24k"),
            "{name}"
        );
    }
}

#[test]
fn the_decoded_measures_hold_the_fixture_totals() {
    for (fixture, report) in reports() {
        let measures = report.measures();
        for (id, key) in DECODED {
            let decoded = measure(&measures, id);
            let at = format!("{}: {}", fixture.name, id.as_str());
            assert_eq!(status(decoded.status), total(&fixture)["status"], "{at}");
            assert_eq!(
                integer_value(decoded),
                integer(total(&fixture), key),
                "{at}"
            );
            assert_eq!(decoded.uncertainty.kind, UncertaintyKind::Floor, "{at}");
        }
    }
}

#[test]
fn the_decoded_measures_have_one_item_per_group_at_its_representative() {
    for (fixture, report) in reports() {
        let measures = report.measures();
        for (id, key) in DECODED {
            let mut expected: Vec<(String, u64)> = rows(&fixture, "group")
                .into_iter()
                .map(|row| (text(row, "representative").to_owned(), integer(row, key)))
                .collect();
            expected.sort();
            let items = integer_items(measure(&measures, id));
            assert_eq!(items, expected, "{}: {}", fixture.name, id.as_str());
        }
    }
}

#[test]
fn the_decoded_measures_list_each_file_not_covered_with_its_reason() {
    for (fixture, report) in reports() {
        let files = rows(&fixture, "file");
        let line = |path: &Value| {
            let path = path.as_str().expect("a path");
            let row = files
                .iter()
                .find(|row| text(row, "path") == path)
                .expect("a file row");
            let status = text(row, "status");
            format!("{path}: {status} ({})", text(row, "reason"))
        };
        let expected: Vec<String> = total(&fixture)["not_covered"]
            .as_array()
            .expect("a list")
            .iter()
            .map(line)
            .collect();
        let measures = report.measures();
        for (id, _) in DECODED {
            let decoded = measure(&measures, id);
            assert_eq!(
                decoded.scope.not_covered,
                expected,
                "{}: {}",
                fixture.name,
                id.as_str()
            );
        }
    }
}

#[test]
fn stereo_groups_are_the_groups_with_two_or_more_channels() {
    for (fixture, report) in reports() {
        let stereo: Vec<&str> = report
            .stereo_groups()
            .map(|group| group.representative.as_str())
            .collect();
        let expected: Vec<&str> = stereo_rows(&fixture)
            .iter()
            .map(|row| text(row, "representative"))
            .collect();
        assert_eq!(stereo, expected, "{}", fixture.name);
    }
}

#[test]
fn the_stereo_measure_counts_each_stereo_group_once() {
    for (fixture, report) in reports() {
        let measures = report.measures();
        let stereo = measure(&measures, MeasureId::StaticAudioStereoFiles);
        let expected: Vec<(String, u64)> = stereo_rows(&fixture)
            .iter()
            .map(|row| (text(row, "representative").to_owned(), 1))
            .collect();
        let at = &fixture.name;
        assert_eq!(integer_value(stereo), expected.len() as u64, "{at}");
        assert_eq!(integer_items(stereo), expected, "{at}");
        assert_eq!(stereo.uncertainty.kind, UncertaintyKind::Exact, "{at}");
    }
}

#[test]
fn the_summary_lists_every_estimated_file_with_its_decoded_bytes() {
    for (fixture, report) in reports() {
        let summary = report.summary();
        let files: Vec<(&str, u64)> = summary
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.decoded_bytes_48k))
            .collect();
        let expected: Vec<(&str, u64)> = rows(&fixture, "file")
            .into_iter()
            .filter(|row| text(row, "status") == "estimated")
            .map(|row| (text(row, "path"), integer(row, "decoded_bytes_48k")))
            .collect();
        assert_eq!(files, expected, "{}", fixture.name);
    }
}

#[test]
fn the_summary_names_each_stereo_representative_with_its_decoded_bytes() {
    for (fixture, report) in reports() {
        let summary = report.summary();
        let stereo: Vec<(&str, u64)> = summary
            .stereo_groups
            .iter()
            .map(|group| (group.path.as_str(), group.decoded_bytes_48k))
            .collect();
        let expected: Vec<(&str, u64)> = stereo_rows(&fixture)
            .into_iter()
            .map(|row| {
                let bytes = integer(row, "decoded_bytes_48k");
                (text(row, "representative"), bytes)
            })
            .collect();
        assert_eq!(stereo, expected, "{}", fixture.name);
    }
}

#[test]
fn the_summary_lists_the_files_not_covered() {
    for (fixture, report) in reports() {
        let summary = report.summary();
        assert_eq!(
            paths(&summary.not_covered),
            total(&fixture)["not_covered"],
            "{}",
            fixture.name
        );
    }
}

#[test]
fn the_scan_gives_the_report_measures_and_summary() {
    let fixture = common::fixture("audio-formats");
    let report = audio::analyse(&fixture.inventory(), &[], &fixture.profile());
    let scan = audio::scan(&fixture.inventory(), &[], &fixture.profile());
    assert_eq!(scan.measures, report.measures());
    assert_eq!(scan.summary, report.summary());
}

/// What decoding a group gives, as its `[[audio.group]]` row writes it.
const HEARD_KEYS: [&str; 4] = ["decoded_from", "silence_bp", "stereo_class", "not_decoded"];

fn heard_row(heard: &Heard) -> Table {
    let mut row = Table::new();
    match (&heard.listening, &heard.source) {
        (Ok(listening), Some(source)) => {
            if *source != heard.representative {
                row.insert("decoded_from".into(), source.as_str().into());
            }
            row.insert("silence_bp".into(), int(listening.silence_bp()));
            if let Some(class) = listening.stereo {
                row.insert("stereo_class".into(), class.name().into());
            }
        }
        (Ok(_), None) => panic!("{}: decoded with no source", heard.representative),
        (Err(reason), _) => {
            row.insert("not_decoded".into(), (*reason).into());
        }
    }
    row
}

#[test]
fn every_sound_group_decodes_as_its_fixture_row() {
    for (fixture, report) in reports() {
        let mut expected = rows(&fixture, "group");
        expected.sort_by_key(|row| text(row, "representative").to_owned());
        assert_eq!(report.heard.len(), expected.len(), "{}", fixture.name);
        for (expected, heard) in expected.into_iter().zip(&report.heard) {
            let expected: Table = HEARD_KEYS
                .iter()
                .filter_map(|key| Some(((*key).to_owned(), expected.get(*key)?.clone())))
                .collect();
            assert_eq!(heard_row(heard), expected, "{}", fixture.name);
        }
    }
}

/// "<representative>: not decoded (<reason>)" for each group the fixture says did not decode,
/// then the files with no header.
fn not_heard(fixture: &Fixture, groups: &[&Table]) -> Vec<String> {
    let unheard = groups.iter().filter_map(|row| {
        let reason = row.get("not_decoded")?.as_str()?;
        Some(format!(
            "{}: not decoded ({reason})",
            text(row, "representative")
        ))
    });
    let files = rows(fixture, "file");
    let unread = total(fixture)["not_covered"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|path| {
            let path = path.as_str().expect("a path");
            let row = files
                .iter()
                .find(|row| text(row, "path") == path)
                .expect("a file row");
            format!("{path}: {} ({})", text(row, "status"), text(row, "reason"))
        });
    let mut lines: Vec<String> = unheard.chain(unread).collect();
    lines.sort();
    lines
}

#[test]
fn the_mono_safe_measure_counts_the_stereo_groups_safe_to_make_mono() {
    for (fixture, report) in reports() {
        let measures = report.measures();
        let mono_safe = measure(&measures, MeasureId::StaticAudioStereoMonoSafe);
        let stereo = stereo_rows(&fixture);
        let classes: Vec<(String, MeasureValue)> = stereo
            .iter()
            .filter_map(|row| {
                let class = row.get("stereo_class")?.as_str()?;
                let path = text(row, "representative").to_owned();
                Some((path, MeasureValue::Text(class.to_owned())))
            })
            .collect();
        let safe = classes
            .iter()
            .filter(|(_, class)| *class == MeasureValue::Text("safe_to_make_mono".into()))
            .count();
        let items: Vec<(String, MeasureValue)> = mono_safe
            .items
            .iter()
            .map(|item| (item.key.clone(), item.value.clone()))
            .collect();
        let at = &fixture.name;
        assert_eq!(mono_safe.status, MeasureStatus::Estimated, "{at}");
        assert_eq!(integer_value(mono_safe), safe as u64, "{at}");
        assert_eq!(items, classes, "{at}");
        assert_eq!(
            mono_safe.scope.not_covered,
            not_heard(&fixture, &stereo),
            "{at}"
        );
    }
}

#[test]
fn the_silence_by_decode_measure_weighs_each_decoded_group_by_its_time() {
    for (fixture, report) in reports() {
        let measures = report.measures();
        let by_decode = measure(&measures, MeasureId::StaticAudioSilenceByDecode);
        let groups = rows(&fixture, "group");
        let mut expected: Vec<(String, u64)> = groups
            .iter()
            .filter(|row| row.contains_key("silence_bp"))
            .map(|row| {
                let path = text(row, "representative").to_owned();
                (path, integer(row, "silence_bp"))
            })
            .collect();
        expected.sort();
        let at = &fixture.name;
        assert_eq!(by_decode.status, MeasureStatus::Estimated, "{at}");
        assert_eq!(
            integer_value(by_decode),
            integer(total(&fixture), "silence_by_decode_bp"),
            "{at}"
        );
        assert_eq!(integer_items(by_decode), expected, "{at}");
        assert_eq!(
            by_decode.scope.not_covered,
            not_heard(&fixture, &groups),
            "{at}"
        );
    }
}

/// What symphonia's demuxer says about a file's first audio track.
#[derive(Debug)]
struct SymphoniaTrack {
    channels: Option<u32>,
    sample_rate: Option<u32>,
    /// Playable frames. Section 11 lists where this differs from our ticks on purpose.
    num_frames: Option<u64>,
    delay: Option<u32>,
}

fn symphonia_track(path: &str, bytes: Vec<u8>) -> Option<SymphoniaTrack> {
    let mut hint = Hint::new();
    if let Some((_, extension)) = path.rsplit_once('.') {
        hint.with_extension(extension);
    }
    let source = MediaSourceStream::new(
        Box::new(Cursor::new(bytes)),
        MediaSourceStreamOptions::default(),
    );
    let reader = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .ok()?;
    let track = reader.first_track(TrackType::Audio)?;
    let params = track.codec_params.as_ref()?.audio()?;
    Some(SymphoniaTrack {
        channels: params
            .channels
            .as_ref()
            .map(|channels| u32::try_from(channels.count()).expect("few channels")),
        sample_rate: params.sample_rate,
        num_frames: track.num_frames,
        delay: track.delay,
    })
}

/// The frames symphonia's decoder gives for the whole of a file's first audio track.
fn symphonia_decoded_frames(path: &str, bytes: Vec<u8>) -> u64 {
    let mut hint = Hint::new();
    if let Some((_, extension)) = path.rsplit_once('.') {
        hint.with_extension(extension);
    }
    let source = MediaSourceStream::new(
        Box::new(Cursor::new(bytes)),
        MediaSourceStreamOptions::default(),
    );
    let mut reader = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .expect("symphonia opens it");
    let params = reader
        .first_track(TrackType::Audio)
        .and_then(|track| track.codec_params.as_ref()?.audio().cloned())
        .expect("an audio track");
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .expect("a decoder");
    let mut frames = 0;
    while let Ok(Some(packet)) = reader.next_packet() {
        let buffer = decoder.decode(&packet).expect("an undamaged fixture");
        frames += buffer.frames() as u64;
    }
    frames
}

#[test]
fn symphonia_agrees_on_channels_rate_and_length() {
    for (fixture, report) in reports() {
        let mut contents = fixture.files();
        for file in &report.files {
            let Some(decoded) = file.decoded() else {
                continue;
            };
            let header = &decoded.header;
            let bytes = contents.remove(file.path.as_str()).expect("a fixture file");
            let at = format!("{}: {}", fixture.name, file.path);
            let track = symphonia_track(file.path.as_str(), bytes.clone())
                .unwrap_or_else(|| panic!("{at}: symphonia opens it"));
            assert_eq!(track.channels, Some(header.channels), "{at}: channels");
            assert_eq!(
                track.sample_rate,
                Some(header.sample_rate),
                "{at}: sample rate"
            );
            match (header.container, header.codec) {
                // Symphonia's num_frames is the last granule. It reports the pre-skip as delay.
                (Container::Ogg, Codec::Opus) => {
                    let skipped = track.num_frames.zip(track.delay.map(u64::from));
                    let playable = skipped.map(|(frames, delay)| frames - delay);
                    assert_eq!(playable, Some(header.ticks), "{at}: length");
                }
                // Symphonia ignores the edit list: its length holds the priming and padding the
                // edit list leaves out, and its decoder gives every one of those frames.
                // audio/listen.rs skips the edit list's lead-in because of this, so a symphonia
                // that applied edit lists, in the demuxer or the decoder, must fail here.
                (Container::Mp4, _) => {
                    let playable = header.ticks * u64::from(header.sample_rate) / header.tick_rate;
                    let frames = track.num_frames.expect("an M4A states its length");
                    assert!(
                        frames > playable,
                        "{at}: {frames} frames, {playable} playable"
                    );
                    let decoded = symphonia_decoded_frames(file.path.as_str(), bytes);
                    // Whole packets, so at least the stated length; a trimmed lead-in is less.
                    assert!(
                        decoded >= frames,
                        "{at}: the decoder gave {decoded} of {frames} frames"
                    );
                }
                // WebM gives no num_frames.
                (Container::Webm, _) => {}
                _ => assert_eq!(track.num_frames, Some(header.ticks), "{at}: length"),
            }
        }
    }
}
