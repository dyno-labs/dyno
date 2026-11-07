//! Bytes by type: file sizes per kind, and the gzip and brotli sizes of text files.
//!
//! Sizes are exact, but the status is `estimated` with exact uncertainty: only browser and device
//! readings are `measured` (design section 4, decision R2).

use std::io::Write;

use brotli::enc::BrotliEncoderParams;
use dyno_schema::ids::MeasureId;
use dyno_schema::manifest::FileKind;
use dyno_schema::measure::{
    Evidence, Measure, MeasureItem, MeasureStatus, MeasureValue, Method, Uncertainty,
};
use dyno_schema::path::RelPath;
use flate2::{Compression, GzBuilder};

use crate::model::{Inventory, saturating_i64};
use crate::sniff::is_source_map;

/// The measures this family owns.
pub const MEASURES: &[MeasureId] = &[
    MeasureId::StaticBytesByType,
    MeasureId::StaticBytesCompressed,
];

/// The files and bytes of one kind, and the compressed sizes of a text kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindBytes {
    pub kind: FileKind,
    pub file_count: u64,
    pub bytes: u64,
    /// None for a kind that servers do not compress.
    pub compressed: Option<CompressedBytes>,
}

/// The sum of each file's own compressed size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompressedBytes {
    pub gzip_bytes: u64,
    pub brotli_bytes: u64,
}

/// Every kind, in the order the design lists them.
const KINDS: [FileKind; 10] = [
    FileKind::Js,
    FileKind::Css,
    FileKind::Html,
    FileKind::Image,
    FileKind::Texture,
    FileKind::Audio,
    FileKind::Video,
    FileKind::Font,
    FileKind::Data,
    FileKind::Other,
];

/// The text kinds, which servers compress. The others are compressed formats already.
const TEXT_KINDS: [FileKind; 4] = [FileKind::Js, FileKind::Css, FileKind::Html, FileKind::Data];

const GZIP_LEVEL: u32 = 9;
const BROTLI_QUALITY: i32 = 11;
const BROTLI_WINDOW_BITS: i32 = 22;

/// The item key suffix for a kind's brotli bytes in the compressed measure.
const BROTLI_SUFFIX: &str = ".brotli";

const COMPRESSED_NOTE: &str =
    "exact for the pinned flate2 (gzip level 9) and brotli (quality 11, window 22)";

/// Sums the file sizes by kind and compresses the text files. No rule reads these measures, so the
/// family has no summary.
pub fn scan(inventory: &Inventory) -> Vec<Measure> {
    let kinds = bytes_by_kind(inventory);
    let source_maps = inventory
        .files()
        .iter()
        .filter(|file| is_source_map(&file.path))
        .count();
    vec![by_type(&kinds), compressed(&kinds, source_maps)]
}

/// Each kind that has files, in the design's order. A text kind also gets the sum of its files'
/// gzip and brotli sizes, each file compressed on its own as a server would send it. Source maps
/// count in the bytes but are not compressed: no player downloads them, and brotli at quality 11
/// on a large map is slow.
pub fn bytes_by_kind(inventory: &Inventory) -> Vec<KindBytes> {
    KINDS
        .into_iter()
        .filter_map(|kind| {
            let files: Vec<(&RelPath, &[u8])> = inventory
                .files_of_kind(kind)
                .filter_map(|file| Some((&file.path, inventory.contents(&file.path)?)))
                .collect();
            if files.is_empty() {
                return None;
            }
            let compressed = TEXT_KINDS.contains(&kind).then(|| {
                let served: Vec<&[u8]> = files
                    .iter()
                    .filter(|(path, _)| !is_source_map(path))
                    .map(|(_, contents)| *contents)
                    .collect();
                CompressedBytes {
                    gzip_bytes: served.iter().map(|contents| gzip_size(contents)).sum(),
                    brotli_bytes: served.iter().map(|contents| brotli_size(contents)).sum(),
                }
            });
            Some(KindBytes {
                kind,
                file_count: files.len() as u64,
                bytes: files
                    .iter()
                    .map(|(_, contents)| contents.len() as u64)
                    .sum(),
                compressed,
            })
        })
        .collect()
}

/// The size of a gzip member at level 9 with no file name and a zero time: the deflate stream
/// plus 18 bytes of header and trailer.
fn gzip_size(contents: &[u8]) -> u64 {
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::new(GZIP_LEVEL));
    encoder
        .write_all(contents)
        .expect("writing to a Vec never fails");
    let gzip = encoder.finish().expect("writing to a Vec never fails");
    gzip.len() as u64
}

/// The size of a brotli stream at quality 11 with a 4 MiB window, in the default mode.
fn brotli_size(contents: &[u8]) -> u64 {
    let params = BrotliEncoderParams {
        quality: BROTLI_QUALITY,
        lgwin: BROTLI_WINDOW_BITS,
        ..BrotliEncoderParams::default()
    };
    let mut input = contents;
    let mut brotli = Vec::new();
    brotli::BrotliCompress(&mut input, &mut brotli, &params)
        .expect("reading a slice and writing to a Vec never fail");
    brotli.len() as u64
}

fn by_type(kinds: &[KindBytes]) -> Measure {
    let items = kinds
        .iter()
        .map(|kind_bytes| item(kind_name(kind_bytes.kind), kind_bytes.bytes))
        .collect();
    exact_measure(
        MeasureId::StaticBytesByType,
        kinds.iter().map(|kind_bytes| kind_bytes.bytes).sum(),
        items,
        Method::FileSize,
        "the size of every file",
    )
}

/// The gzip total, with two items per text kind: its gzip bytes, then its brotli bytes. Brotli has
/// no measure of its own, so only the gzip items sum to the value.
fn compressed(kinds: &[KindBytes], source_maps: usize) -> Measure {
    let text_kinds: Vec<(FileKind, CompressedBytes)> = kinds
        .iter()
        .filter_map(|kind_bytes| Some((kind_bytes.kind, kind_bytes.compressed?)))
        .collect();
    let mut items = Vec::new();
    for (kind, compressed) in &text_kinds {
        let name = kind_name(*kind);
        items.push(item(name.clone(), compressed.gzip_bytes));
        items.push(item(
            format!("{name}{BROTLI_SUFFIX}"),
            compressed.brotli_bytes,
        ));
    }
    let mut measure = exact_measure(
        MeasureId::StaticBytesCompressed,
        text_kinds
            .iter()
            .map(|(_, compressed)| compressed.gzip_bytes)
            .sum(),
        items,
        Method::FileSize,
        "each js, css, html and data file compressed on its own with gzip and brotli",
    );
    measure.uncertainty.note = Some(COMPRESSED_NOTE.to_owned());
    measure
        .scope
        .not_covered
        .push("image, texture, audio, video, font and other files, which are not text".to_owned());
    if source_maps > 0 {
        measure.scope.not_covered.push(format!(
            "source maps, which no player downloads: {source_maps}"
        ));
    }
    measure
}

fn exact_measure(
    id: MeasureId,
    value: u64,
    items: Vec<MeasureItem>,
    method: Method,
    detail: &str,
) -> Measure {
    Measure {
        value: Some(MeasureValue::Integer(saturating_i64(value))),
        items,
        evidence: vec![Evidence {
            method,
            detail: detail.to_owned(),
        }],
        uncertainty: Uncertainty::exact(),
        ..Measure::blank(id, MeasureStatus::Estimated)
    }
}

fn item(key: String, bytes: u64) -> MeasureItem {
    MeasureItem {
        key,
        value: MeasureValue::Integer(saturating_i64(bytes)),
    }
}

/// The kind's JSON name, such as "js", so items match the manifest.
fn kind_name(kind: FileKind) -> String {
    match serde_json::to_value(kind) {
        Ok(serde_json::Value::String(name)) => name,
        _ => unreachable!("a file kind serializes as a string"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gzip_of_nothing_is_a_header_an_empty_block_and_a_trailer() {
        assert_eq!(gzip_size(b""), 20);
    }

    #[test]
    fn brotli_of_nothing_is_one_byte() {
        assert_eq!(brotli_size(b""), 1);
    }

    #[test]
    fn kind_names_are_the_manifest_names() {
        assert_eq!(kind_name(FileKind::Js), "js");
        assert_eq!(kind_name(FileKind::Texture), "texture");
    }

    #[test]
    fn a_source_map_counts_in_the_bytes_but_is_not_compressed() {
        let files = [
            ("a.json", b"{\"a\": 1}".to_vec()),
            ("a.js.map", vec![b'x'; 4_096]),
        ];
        let inventory = crate::inventory::inventory_of(
            files
                .into_iter()
                .map(|(raw, contents)| (RelPath::new(raw).unwrap(), contents))
                .collect(),
        );
        let with_map = bytes_by_kind(&inventory);
        let data = with_map
            .iter()
            .find(|kind_bytes| kind_bytes.kind == FileKind::Data)
            .unwrap();
        assert_eq!(data.bytes, 4_096 + 8);
        assert_eq!(
            data.compressed.unwrap().gzip_bytes,
            gzip_size(b"{\"a\": 1}")
        );
        let measures = scan(&inventory);
        assert!(
            measures[1]
                .scope
                .not_covered
                .contains(&"source maps, which no player downloads: 1".to_owned())
        );
    }
}
