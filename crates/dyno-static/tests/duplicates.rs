//! Duplicates: groups of files with the same SHA-256, and the bytes their extra copies waste
//! (design section 4).

mod common;

use std::collections::BTreeMap;

use common::{Fixture, expected_value, fixtures, integer, non_negative, path, paths, rows, text};
use dyno_schema::ids::MeasureId;
use dyno_schema::measure::{MeasureStatus, MeasureValue, UncertaintyKind};
use dyno_schema::path::RelPath;
use dyno_static::duplicates;
use dyno_static::inventory::inventory_of;
use dyno_static::model::DuplicateGroup;

#[test]
fn every_fixture_gives_the_duplicate_groups_it_lists() {
    for fixture in fixtures() {
        let expected = fixture.required_table("duplicates");
        let groups =
            duplicates::duplicate_groups(&fixture.scanned_inventory(), &decoded_bytes(&fixture));
        assert_eq!(
            groups.len() as u64,
            integer(expected, "group_count"),
            "{}",
            fixture.name
        );
        assert_eq!(groups, listed_groups(&fixture), "{}", fixture.name);
    }
}

#[test]
fn every_fixture_wastes_the_bytes_of_each_extra_copy() {
    for fixture in fixtures() {
        let scan = duplicates::scan(&fixture.scanned_inventory(), &decoded_bytes(&fixture));
        let [wasted] = scan.measures.as_slice() else {
            panic!("{}: duplicates gives one measure", fixture.name);
        };
        assert_eq!(
            wasted.id,
            MeasureId::StaticBytesDuplicates,
            "{}",
            fixture.name
        );
        assert_eq!(wasted.status, MeasureStatus::Estimated, "{}", fixture.name);
        assert_eq!(
            wasted.uncertainty.kind,
            UncertaintyKind::Exact,
            "{}",
            fixture.name
        );
        let groups = listed_groups(&fixture);
        let wasted_bytes: u64 = groups.iter().map(|group| group.wasted_bytes).sum();
        assert_eq!(
            wasted.value,
            Some(expected_value(wasted_bytes)),
            "{}",
            fixture.name
        );
        let extra_copies: BTreeMap<&str, u64> = groups
            .iter()
            .flat_map(|group| {
                group.paths[1..]
                    .iter()
                    .map(|path| (path.as_str(), group.bytes))
            })
            .collect();
        let items: BTreeMap<&str, u64> = wasted
            .items
            .iter()
            .map(|item| (item.key.as_str(), integer_value(&item.value)))
            .collect();
        assert_eq!(items, extra_copies, "{}", fixture.name);
        assert_eq!(wasted.items.len(), items.len(), "{}", fixture.name);
    }
}

#[test]
fn the_copy_the_page_names_is_kept_and_the_other_is_extra() {
    let files: BTreeMap<RelPath, Vec<u8>> = [
        (
            "index.html",
            b"<audio src='audio/click.mp3'></audio>".to_vec(),
        ),
        ("audio/click.mp3", b"same bytes".to_vec()),
        ("audio/click-old.mp3", b"same bytes".to_vec()),
    ]
    .into_iter()
    .map(|(raw, contents)| (path(raw), contents))
    .collect();
    let groups = duplicates::duplicate_groups(&inventory_of(files), &BTreeMap::new());
    let [group] = groups.as_slice() else {
        panic!("one group, not {}", groups.len());
    };
    assert_eq!(group.keep, Some(path("audio/click.mp3")));
    assert_eq!(
        duplicates::extra_copies(group),
        [&path("audio/click-old.mp3")]
    );
}

#[test]
fn empty_files_are_never_copies() {
    let inventory = inventory_of(BTreeMap::from([
        (path("a.txt"), Vec::new()),
        (path("b.txt"), Vec::new()),
    ]));
    assert_eq!(
        duplicates::duplicate_groups(&inventory, &BTreeMap::new()),
        []
    );
}

#[test]
fn each_extra_copy_saves_the_decoded_bytes_of_the_first() {
    let contents = b"png";
    let inventory = inventory_of(BTreeMap::from([
        (path("a.png"), contents.to_vec()),
        (path("b.png"), contents.to_vec()),
        (path("c.png"), contents.to_vec()),
    ]));
    let decoded_bytes = BTreeMap::from([(path("a.png"), 100), (path("b.png"), 7)]);
    let groups = duplicates::duplicate_groups(&inventory, &decoded_bytes);
    let [group] = groups.as_slice() else {
        panic!("one group: {groups:?}");
    };
    let extra_copies = 2;
    assert_eq!(group.wasted_bytes, extra_copies * contents.len() as u64);
    assert_eq!(group.decoded_bytes_saved, extra_copies * 100);
}

/// Each texture's GPU bytes and each audio file's bytes at 48 kHz, from the fixture's tables, as
/// the scan gives them to duplicates.
fn decoded_bytes(fixture: &Fixture) -> BTreeMap<RelPath, u64> {
    let estimated_rows = |table: &str, bytes_key: &'static str| {
        let rows = fixture.table(table).map(|table| rows(table, "file"));
        rows.unwrap_or_default()
            .into_iter()
            .filter(|row| text(row, "status") == "estimated")
            .map(move |row| (path(text(row, "path")), integer(row, bytes_key)))
    };
    estimated_rows("textures", "gpu_bytes")
        .chain(estimated_rows("audio", "decoded_bytes_48k"))
        .collect()
}

fn listed_groups(fixture: &Fixture) -> Vec<DuplicateGroup> {
    rows(fixture.required_table("duplicates"), "group")
        .into_iter()
        .map(|row| DuplicateGroup {
            sha256: text(row, "sha256").to_owned(),
            bytes: integer(row, "size"),
            paths: paths(row, "paths"),
            keep: row.get("keep").and_then(|keep| keep.as_str()).map(path),
            wasted_bytes: integer(row, "wasted_bytes"),
            decoded_bytes_saved: integer(row, "decoded_bytes_saved"),
        })
        .collect()
}

fn integer_value(value: &MeasureValue) -> u64 {
    let MeasureValue::Integer(integer) = value else {
        panic!("{value:?} is not an integer");
    };
    non_negative(*integer)
}
