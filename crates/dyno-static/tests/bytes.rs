//! Bytes by type: each kind's files and bytes, and the gzip and brotli sizes of the text kinds
//! (design section 4).

mod common;

use std::collections::BTreeMap;

use common::{bucket, expected_value, file_kind, fixtures, integer, non_negative};
use dyno_schema::ids::MeasureId;
use dyno_schema::measure::{Measure, MeasureStatus, MeasureValue, UncertaintyKind};
use dyno_static::bytes::{self, CompressedBytes, KindBytes};
use dyno_static::model::Inventory;
use toml::Table;

#[test]
fn every_fixture_gives_the_bytes_by_kind_it_lists() {
    for fixture in fixtures() {
        let kinds = bytes::bytes_by_kind(&fixture.scanned_inventory());
        let by_bucket: BTreeMap<String, KindBytes> = kinds
            .into_iter()
            .map(|kind_bytes| (bucket(kind_bytes.kind), kind_bytes))
            .collect();
        assert_eq!(
            by_bucket,
            listed_kinds(fixture.required_table("bytes")),
            "{}",
            fixture.name
        );
    }
}

#[test]
fn every_fixture_sums_its_bytes_by_type_with_one_item_per_kind() {
    for fixture in fixtures() {
        let listed = listed_kinds(fixture.required_table("bytes"));
        let [by_type, _] = scan(&fixture.scanned_inventory());
        assert_eq!(by_type.id, MeasureId::StaticBytesByType, "{}", fixture.name);
        assert_exact_estimate(&by_type, &fixture.name);
        let total_bytes = integer(fixture.required_table("inventory"), "total_bytes");
        assert_eq!(
            by_type.value,
            Some(expected_value(total_bytes)),
            "{}",
            fixture.name
        );
        let items: BTreeMap<String, u64> = listed
            .into_iter()
            .map(|(bucket, kind)| (bucket, kind.bytes))
            .collect();
        assert_eq!(item_values(&by_type), items, "{}", fixture.name);
    }
}

#[test]
fn every_fixture_sums_its_gzip_bytes_and_lists_brotli_beside_them() {
    for fixture in fixtures() {
        let listed = listed_kinds(fixture.required_table("bytes"));
        let [_, compressed] = scan(&fixture.scanned_inventory());
        assert_eq!(
            compressed.id,
            MeasureId::StaticBytesCompressed,
            "{}",
            fixture.name
        );
        assert_exact_estimate(&compressed, &fixture.name);
        let mut gzip_total = 0;
        let mut items = BTreeMap::new();
        for (bucket, kind) in listed {
            let Some(sizes) = kind.compressed else {
                continue;
            };
            gzip_total += sizes.gzip_bytes;
            items.insert(format!("{bucket}.brotli"), sizes.brotli_bytes);
            items.insert(bucket, sizes.gzip_bytes);
        }
        assert_eq!(
            compressed.value,
            Some(expected_value(gzip_total)),
            "{}",
            fixture.name
        );
        assert_eq!(item_values(&compressed), items, "{}", fixture.name);
    }
}

/// Each `[bytes.<kind>]` table, by its bucket name.
fn listed_kinds(table: &Table) -> BTreeMap<String, KindBytes> {
    table
        .iter()
        .filter_map(|(bucket, row)| Some((bucket, row.as_table()?)))
        .map(|(bucket, row)| {
            let compressed = row.contains_key("gzip_bytes").then(|| CompressedBytes {
                gzip_bytes: integer(row, "gzip_bytes"),
                brotli_bytes: integer(row, "brotli_bytes"),
            });
            let kind_bytes = KindBytes {
                kind: file_kind(bucket),
                file_count: integer(row, "files"),
                bytes: integer(row, "bytes"),
                compressed,
            };
            (bucket.clone(), kind_bytes)
        })
        .collect()
}

fn scan(inventory: &Inventory) -> [Measure; 2] {
    bytes::scan(inventory)
        .try_into()
        .expect("bytes gives two measures")
}

fn assert_exact_estimate(measure: &Measure, fixture: &str) {
    assert_eq!(
        measure.status,
        MeasureStatus::Estimated,
        "{fixture}: {}",
        measure.id
    );
    assert_eq!(
        measure.uncertainty.kind,
        UncertaintyKind::Exact,
        "{fixture}: {}",
        measure.id
    );
}

/// Each item's bytes by key. A key may appear only once.
fn item_values(measure: &Measure) -> BTreeMap<String, u64> {
    let values: BTreeMap<String, u64> = measure
        .items
        .iter()
        .map(|item| {
            let MeasureValue::Integer(bytes) = item.value else {
                panic!("{} item {} is not a byte count", measure.id, item.key);
            };
            (item.key.clone(), non_negative(bytes))
        })
        .collect();
    assert_eq!(
        values.len(),
        measure.items.len(),
        "{} repeats an item key",
        measure.id
    );
    values
}
