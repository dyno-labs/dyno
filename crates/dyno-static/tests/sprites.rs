//! Sprite maps: every fixture's `[sprites]` table and the sprite silence measure (design section 6).
//!
//! Bundle finds Howl sites in scripts. These tests build the one site in `howler-sprites` by hand,
//! as written in its `main.js`, so they do not wait for bundle.

mod common;

use common::{Fixture, fixtures, text};
use dyno_schema::ids::MeasureId;
use dyno_schema::measure::{MeasureStatus, MeasureValue};
use dyno_schema::path::RelPath;
use dyno_static::audio::{self, AudioReport, SpriteMapRow};
use dyno_static::model::{HowlSite, SpriteRange};
use toml::{Table, Value};

/// `new k.Howl({src:["assets/audio/sfx.webm","assets/audio/sfx.mp3"],sprite:{spin:[0,500],
/// stop:[1500,750],bonus_loop:[3e3,1e3,!0]}})` in `howler-sprites/build/main.js`.
fn howler_sprites_site(fixture: &Fixture) -> HowlSite {
    let file = RelPath::new("main.js").unwrap();
    let script = &fixture.files()[file.as_str()];
    let call = br#"new k.Howl({src:["assets/audio/sfx.webm""#;
    let offset = script
        .windows(call.len())
        .position(|window| window == call)
        .expect("main.js makes the Howl");
    let range = |name: &str, start_us, end_us, looped| SpriteRange {
        name: name.to_owned(),
        start_us,
        end_us,
        looped,
    };
    HowlSite {
        file,
        offset: u32::try_from(offset).unwrap(),
        src: vec![
            "assets/audio/sfx.webm".into(),
            "assets/audio/sfx.mp3".into(),
        ],
        sprites: vec![
            range("spin", 0, 500_000, false),
            range("stop", 1_500_000, 2_250_000, false),
            range("bonus_loop", 3_000_000, 4_000_000, true),
        ],
    }
}

/// The fixtures with a `[sprites]` table, each with its report.
fn reports() -> Vec<(Fixture, AudioReport)> {
    fixtures()
        .into_iter()
        .filter(|fixture| fixture.table("sprites").is_some())
        .map(|fixture| {
            let sites = match fixture.name.as_str() {
                "howler-sprites" => vec![howler_sprites_site(&fixture)],
                _ => Vec::new(),
            };
            let report = audio::analyse(&fixture.inventory(), &sites, &fixture.profile());
            (fixture, report)
        })
        .collect()
}

fn int(value: impl Into<u64>) -> Value {
    Value::Integer(i64::try_from(value.into()).expect("fits a TOML integer"))
}

/// One sprite map as `expected.toml` writes it.
fn map_row(map: &SpriteMapRow) -> Table {
    let mut row = Table::new();
    row.insert("source".into(), map.source.as_str().into());
    row.insert("status".into(), Value::try_from(map.status).unwrap());
    if let Some(reason) = map.reason {
        row.insert("reason".into(), reason.into());
    }
    if let Some(sprite_map) = &map.map {
        row.insert("kind".into(), sprite_map.kind.name().into());
        row.insert("sprite_count".into(), int(sprite_map.sprites.len() as u64));
    }
    let audio = map.audio.iter().map(|path| path.as_str().into()).collect();
    row.insert("audio".into(), Value::Array(audio));
    if let Some(silence) = &map.silence {
        row.insert(
            "representative".into(),
            silence.representative.as_str().into(),
        );
        row.insert("duration_us".into(), int(silence.duration_us));
        row.insert("covered_us".into(), int(silence.covered_us));
        row.insert("silence_us".into(), int(silence.silence_us));
        row.insert("silence_bp".into(), int(silence.silence_bp));
        row.insert("silence_bytes_48k".into(), int(silence.silence_bytes_48k));
    }
    row
}

/// The `[[sprites.map]]` rows of a fixture, sorted by source as the report sorts them.
fn expected_maps(fixture: &Fixture) -> Vec<&Table> {
    let mut rows: Vec<&Table> = fixture.required_table("sprites")["map"]
        .as_array()
        .expect("[[sprites.map]] rows")
        .iter()
        .map(|row| row.as_table().expect("a row is a table"))
        .collect();
    rows.sort_by_key(|row| text(row, "source").to_owned());
    rows
}

#[test]
fn every_sprite_map_matches_its_fixture_row() {
    for (fixture, report) in reports() {
        let expected = expected_maps(&fixture);
        assert_eq!(
            report.sprite_maps.len(),
            expected.len(),
            "{}: maps",
            fixture.name
        );
        for (expected, map) in expected.into_iter().zip(&report.sprite_maps) {
            let actual = map_row(map);
            let compared: Table = expected
                .keys()
                .filter_map(|key| Some((key.clone(), actual.get(key)?.clone())))
                .collect();
            assert_eq!(&compared, expected, "{}", fixture.name);
        }
    }
}

#[test]
fn the_summary_names_the_representative_only_when_estimated() {
    for (fixture, report) in reports() {
        for map in &report.sprite_maps {
            let summary = map.summary();
            let at = format!("{}: {}", fixture.name, map.source);
            assert_eq!(summary.source, map.source, "{at}");
            assert_eq!(summary.status, map.status, "{at}");
            match &map.silence {
                Some(silence) => {
                    assert_eq!(
                        summary.audio.as_ref(),
                        Some(&silence.representative),
                        "{at}"
                    );
                    assert_eq!(summary.silence_bp, silence.silence_bp, "{at}");
                    assert_eq!(summary.silence_bytes_48k, silence.silence_bytes_48k, "{at}");
                }
                None => assert_eq!(
                    (summary.audio, summary.silence_bp, summary.silence_bytes_48k),
                    (None, 0, 0),
                    "{at}"
                ),
            }
        }
    }
}

#[test]
fn the_silence_measure_is_the_largest_share_with_one_item_per_map() {
    let fixture = common::fixture("howler-sprites");
    let sites = [howler_sprites_site(&fixture)];
    let scan = audio::scan(&fixture.inventory(), &sites, &fixture.profile());
    let silence = scan
        .measures
        .iter()
        .find(|measure| measure.id == MeasureId::StaticAudioSpriteSilence)
        .expect("the sprite silence measure");
    assert_eq!(silence.status, MeasureStatus::Estimated);
    assert_eq!(silence.value, Some(MeasureValue::Integer(6_250)));
    let items: Vec<(&str, &MeasureValue)> = silence
        .items
        .iter()
        .map(|item| (item.key.as_str(), &item.value))
        .collect();
    // A Howl site's key holds its offset, since one script can hold several sites.
    let site_key = format!("main.js@{}", sites[0].offset);
    assert_eq!(
        items,
        [
            ("assets/audio/music.json", &MeasureValue::Integer(2_500)),
            ("assets/audio/ui.json", &MeasureValue::Integer(6_250)),
            (site_key.as_str(), &MeasureValue::Integer(6_250)),
        ]
    );
    assert!(silence.scope.not_covered.is_empty());
}

#[test]
fn a_sprite_map_that_does_not_parse_is_not_covered_by_the_silence_measure() {
    let fixture = common::fixture("broken");
    let scan = audio::scan(&fixture.inventory(), &[], &fixture.profile());
    let silence = scan
        .measures
        .iter()
        .find(|measure| measure.id == MeasureId::StaticAudioSpriteSilence)
        .expect("the sprite silence measure");
    assert_eq!(silence.value, Some(MeasureValue::Integer(0)));
    assert_eq!(
        silence.scope.not_covered,
        ["bad/sprite-broken.json: invalid (json)"]
    );
}

#[test]
fn a_howl_site_without_sprites_still_joins_its_files_into_one_group() {
    let fixture = common::fixture("howler-sprites");
    let site = HowlSite {
        file: RelPath::new("main.js").unwrap(),
        offset: 0,
        src: vec![
            "assets/audio/music.m4a".into(),
            "assets/audio/ui.ogg".into(),
        ],
        sprites: Vec::new(),
    };
    let report = audio::analyse(&fixture.inventory(), &[site], &fixture.profile());
    let joined = report
        .groups
        .iter()
        .find(|group| group.representative.as_str() == "assets/audio/music.m4a")
        .expect("the music group");
    let paths: Vec<&str> = joined.paths.iter().map(RelPath::as_str).collect();
    assert_eq!(
        paths,
        [
            "assets/audio/music.m4a",
            "assets/audio/ui.mp3",
            "assets/audio/ui.ogg"
        ]
    );
    assert!(
        report
            .sprite_maps
            .iter()
            .all(|map| map.source.as_str() != "main.js")
    );
}
