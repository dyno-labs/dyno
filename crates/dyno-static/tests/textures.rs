//! Textures: every fixture's `[textures]` rows and totals, then the cases no fixture has.
//!
//! The zip fixture is left out: `Fixture::inventory` reads folders only, and `zip-input` lists
//! the same textures as `duplicates`.

mod common;

use std::collections::BTreeMap;
use std::fs;

use common::{Fixture, expected_value, fixture, fixtures, integer, non_negative, text};
use dyno_schema::ids::MeasureId;
use dyno_schema::manifest::{BuildManifest, FileKind, ManifestFile};
use dyno_schema::measure::{MeasureItem, MeasureStatus, MeasureValue, UncertaintyKind};
use dyno_schema::path::RelPath;
use dyno_static::model::{Engine, Inventory, TextureCost, TextureSummary};
use dyno_static::textures::{
    self, ASTC, BASIS_FORMAT, BASIS_VERSION, BC_ON_IPHONE, BMP, DAMAGED_HEADER, DDS,
    DecodedTexture, Encoding, GpuFormat, HEIC, HEIF, ICO, KTX1, MipSource, NotCovered, Note,
    OTHER_IMAGE_FORMAT, PVR, SVG_NO_SIZE, TRUNCATED, TextureFile, TextureFormat,
};
use toml::{Table, Value};

/// How many files the textures rule names.
const TOP_FILES: usize = 5;

// `assets/spine/hero_2.png` in the images fixture: 512x256 with a full Spine chain of 10 levels.
const HERO_2_RGBA8_BYTES: u64 = 699_052;
const HERO_2_ASTC_BYTES: u64 = 174_800;

// PNG, from the PNG spec: an 8-byte signature, then IHDR's length, type, 13 bytes of data and CRC.
/// `imagesize` checks the first 4 bytes of the signature only.
const PNG_SIGNATURE_BYTES_IMAGESIZE_CHECKS: usize = 4;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const PNG_IHDR_DATA_BYTES: u32 = 13;
const PNG_IHDR_TYPE_AT: usize = 12;
const PNG_IHDR_END: usize = 33;

// WebP, from the WebP container spec: a 12-byte RIFF header, then an 8-byte chunk header.
/// A lossy frame starts with a 3-byte frame tag and a 3-byte start code, then the width and height.
const VP8_WIDTH_AT: usize = 12 + 8 + 3 + 3;
const VP8_HEIGHT_AT: usize = VP8_WIDTH_AT + 2;
/// The first byte of a lossless frame.
const VP8L_SIGNATURE: u8 = 0x2F;

// KTX2 header fields, from the Khronos KTX2 spec.
const KTX2_VK_FORMAT_AT: usize = 12;
const KTX2_PIXEL_HEIGHT_AT: usize = 24;
const KTX2_PIXEL_DEPTH_AT: usize = 28;
const KTX2_LAYER_COUNT_AT: usize = 32;
const KTX2_FACE_COUNT_AT: usize = 36;
const KTX2_LEVEL_COUNT_AT: usize = 40;
const KTX2_SUPERCOMPRESSION_AT: usize = 44;
const KTX2_DFD_OFFSET_AT: usize = 48;
/// Level 0's uncompressed length. The level index follows the 80-byte header, and each entry is
/// a byte offset, a byte length and an uncompressed length, 8 bytes each.
const KTX2_LEVEL_0_UNCOMPRESSED_LENGTH_AT: usize = 96;
// Offsets from the start of the DFD, which opens with its total size (4 bytes). The first block's
// header (8 bytes) holds the vendor and type (4), the version (2) and the block size (2). Then
// come colour model, primaries, transfer function and flags (4), then the texel block dimensions.
const KTX2_DFD_FIRST_BLOCK_VENDOR_AT: usize = 4;
const KTX2_DFD_FIRST_BLOCK_SIZE_AT: usize = 4 + 6;
const KTX2_DFD_FIRST_TEXEL_BLOCK_DIMENSION_AT: usize = 4 + 8 + 4;
const KTX2_SUPERCOMPRESSION_NONE: u32 = 0;
const KTX2_SUPERCOMPRESSION_BASIS_LZ: u32 = 1;
const KTX2_SUPERCOMPRESSION_ZSTANDARD: u32 = 2;
const KTX2_SUPERCOMPRESSION_ZLIB: u32 = 3;
const VK_FORMAT_BC1_RGB_UNORM_BLOCK: u32 = 131;
const VK_FORMAT_BC7_SRGB_BLOCK: u32 = 146;
const VK_FORMAT_R16G16B16A16_SFLOAT: u32 = 97;
/// `uastc-512x256.ktx2` holds one level: 128 x 64 blocks of 16 bytes.
const UASTC_512X256_BYTES: u64 = 128 * 64 * 16;

// Basis fields, from the Basis Universal file format.
const BASIS_HEADER_BYTES: usize = 77;
const BASIS_SLICE_BYTES: usize = 23;
const BASIS_VERSION_AT: usize = 2;
const BASIS_HEADER_SIZE_AT: usize = 4;
const BASIS_SLICE_COUNT_AT: usize = 14;
const BASIS_TEXTURE_FORMAT_AT: usize = 20;
const BASIS_TEXTURE_TYPE_AT: usize = 23;
const BASIS_SLICE_TABLE_AT: usize = 65;
const BASIS_SLICE_IMAGE_AT: usize = 0;
const BASIS_SLICE_LEVEL_AT: usize = 3;
const BASIS_SLICE_FLAGS_AT: usize = 4;
const BASIS_SLICE_WIDTH_AT: usize = 5;
const BASIS_SLICE_HEIGHT_AT: usize = 7;
const BASIS_SLICE_FLAG_HAS_ALPHA: u8 = 1;
/// A video frame that does not depend on the frame before it.
const BASIS_SLICE_FLAG_I_FRAME: u8 = 2;
/// The only version dyno reads.
const BASIS_VERSION_0X13: u16 = 0x13;
const BASIS_TEXTURE_FORMAT_UASTC_LDR_4X4: u8 = 1;
const BASIS_TEXTURE_FORMAT_UASTC_HDR_4X4: u8 = 2;
const BASIS_TEXTURE_TYPE_CUBEMAP_ARRAY: u8 = 2;
const BASIS_TEXTURE_TYPE_VIDEO_FRAMES: u8 = 3;
/// A 256 x 128 level in ASTC 4x4: 64 x 32 blocks of 16 bytes.
const LEVEL_256X128_ASTC_BYTES: u64 = 64 * 32 * 16;

// ---- Fixtures ----

#[test]
fn every_fixture_gives_the_texture_rows_it_lists() {
    for fixture in folder_fixtures_with_textures() {
        let table = fixture.required_table("textures");
        let expected: Vec<&Table> = table["file"]
            .as_array()
            .expect("[[textures.file]] rows")
            .iter()
            .map(|row| row.as_table().expect("a texture row is a table"))
            .collect();
        let report = textures::read(&fixture.inventory(), engine_for_mips(&fixture));
        let actual: Vec<Table> = report.files.iter().map(fixture_row).collect();
        assert_eq!(actual.len(), expected.len(), "{}", fixture.name);
        for (actual, expected) in actual.iter().zip(expected) {
            assert_eq!(actual, expected, "{}", fixture.name);
        }
    }
}

#[test]
fn every_fixture_gives_its_texture_totals() {
    for fixture in folder_fixtures_with_textures() {
        let total = fixture.required_table("textures")["total"]
            .as_table()
            .expect("[textures.total]");
        let scan = textures::scan(&fixture.inventory(), engine_for_mips(&fixture));
        let name = &fixture.name;
        assert_eq!(text(total, "status"), "estimated", "{name}");
        assert_eq!(scan.measures[0].status, MeasureStatus::Estimated, "{name}");
        assert_eq!(scan.summary.low_bytes, integer(total, "lo_bytes"), "{name}");
        assert_eq!(
            scan.summary.high_bytes,
            integer(total, "hi_bytes"),
            "{name}"
        );
        assert_eq!(
            path_strings(&scan.summary.not_covered),
            text_array(total, "not_covered"),
            "{name}"
        );
        // The rules family owns the top 5. This checks the fixture's list against these bytes.
        assert_eq!(
            largest_files(&scan.summary),
            text_array(total, "top5"),
            "{name}"
        );
    }
}

#[test]
fn a_build_without_images_costs_nothing_and_leaves_nothing_out() {
    let without_images: Vec<Fixture> = fixtures()
        .into_iter()
        .filter(|fixture| !fixture.is_zip() && fixture.table("textures").is_none())
        .collect();
    assert!(!without_images.is_empty(), "every fixture has [textures]");
    for fixture in without_images {
        let scan = textures::scan(&fixture.inventory(), Engine::Unknown);
        let name = &fixture.name;
        assert_eq!(scan.summary, TextureSummary::default(), "{name}");
        let measure = &scan.measures[0];
        assert_eq!(measure.value, Some(MeasureValue::Integer(0)), "{name}");
        assert!(measure.scope.not_covered.is_empty(), "{name}");
    }
}

#[test]
fn the_family_gives_one_estimated_measure() {
    let scan = textures::scan(&fixture("images").inventory(), Engine::Unknown);
    let [measure] = scan.measures.as_slice() else {
        panic!("one measure, got {:?}", scan.measures);
    };
    assert_eq!(measure.id, MeasureId::StaticTexturesDecoded);
    assert_eq!(measure.status, MeasureStatus::Estimated);
}

#[test]
fn the_measure_is_one_copy_with_a_range_up_to_two() {
    let measure = &textures::scan(&fixture("images").inventory(), Engine::Unknown).measures[0];
    // The images fixture's lo_bytes.
    let one_copy = 43_085_776;
    assert_eq!(measure.value, Some(MeasureValue::Integer(one_copy)));
    assert_eq!(measure.uncertainty.kind, UncertaintyKind::Range);
    assert_eq!(measure.uncertainty.low, Some(one_copy));
    assert_eq!(measure.uncertainty.high, Some(2 * one_copy));
}

#[test]
fn the_measure_has_one_item_per_estimated_file_with_its_gpu_bytes() {
    let scan = textures::scan(&fixture("images").inventory(), Engine::Unknown);
    let measure = &scan.measures[0];
    assert_eq!(measure.items.len(), scan.summary.files.len());
    assert!(measure.items.contains(&MeasureItem {
        key: "assets/spine/hero_2.png".to_owned(),
        value: expected_value(HERO_2_RGBA8_BYTES),
    }));
    let item_sum: u64 = measure
        .items
        .iter()
        .map(|item| match item.value {
            MeasureValue::Integer(bytes) => non_negative(bytes),
            ref other => panic!("an item is a byte count, got {other:?}"),
        })
        .sum();
    assert_eq!(item_sum, scan.summary.low_bytes);
}

#[test]
fn the_summary_gives_each_estimated_file_its_gpu_and_astc_bytes() {
    let summary = textures::scan(&fixture("images").inventory(), Engine::Unknown).summary;
    let hero_2 = TextureCost {
        path: path("assets/spine/hero_2.png"),
        gpu_bytes: HERO_2_RGBA8_BYTES,
        astc_bytes: HERO_2_ASTC_BYTES,
    };
    assert!(summary.files.contains(&hero_2), "{:?}", summary.files);
}

#[test]
fn the_measure_names_each_file_it_leaves_out_with_its_reason() {
    let measure = &textures::scan(&fixture("broken").inventory(), Engine::Unknown).measures[0];
    assert_eq!(
        measure.scope.not_covered,
        [
            "bad/avif-ftyp-only.avif: invalid (header)",
            "bad/basis-truncated.basis: invalid (truncated)",
            "bad/jpeg-no-sof.jpg: invalid (header)",
            "bad/ktx2-truncated.ktx2: invalid (truncated)",
            "bad/png-truncated.png: invalid (header)",
            "bad/webp-truncated.webp: invalid (header)",
        ]
    );
}

#[test]
fn power_of_two_images_get_no_mips_under_pixi_8() {
    // A Pixi 7 fixture read as Pixi 8, so the engine alone drops the mips.
    let report = textures::read(&fixture("pixi7-aa").inventory(), Engine::Pixi8);
    let square_image = decoded_file(&report.files, "assets/atlas-1024x1024.png");
    assert_eq!(square_image.mips, MipSource::NoMips);
    assert_eq!(square_image.levels, 1);
    assert_eq!(square_image.gpu_bytes, 1024 * 1024 * 4);
}

// ---- Spine atlases ----

#[test]
fn a_spine_mipmap_page_gets_a_full_chain_whatever_its_size() {
    let atlas = "panel.png\nsize: 1000, 600\nfilter: MipMapLinearLinear, Linear\npanel\n";
    let inventory = inventory_of(vec![
        ("ui/panel.png", FileKind::Image, png_file(1000, 600)),
        ("ui/ui.atlas", FileKind::Data, atlas.into()),
    ]);
    let report = textures::read(&inventory, Engine::Pixi8);
    let panel = decoded_file(&report.files, "ui/panel.png");
    assert_eq!(panel.mips, MipSource::SpineAtlasFilter);
    // 1000x600 down to 1x1 is 10 levels.
    assert_eq!(panel.levels, 10);
}

#[test]
fn a_spine_filter_does_not_change_a_ktx2_page() {
    let atlas = "symbols.ktx2\nfilter: MipMap, Linear\n";
    let inventory = inventory_of(vec![
        (
            "symbols.ktx2",
            FileKind::Texture,
            images_fixture_file("ktx2/uastc-512x256.ktx2"),
        ),
        ("symbols.atlas", FileKind::Data, atlas.into()),
    ]);
    let report = textures::read(&inventory, Engine::Unknown);
    let symbols = decoded_file(&report.files, "symbols.ktx2");
    assert_eq!(symbols.mips, MipSource::FileLevels);
    // The fixture file holds one level.
    assert_eq!(symbols.levels, 1);
}

#[test]
fn a_spine_page_missing_from_the_build_is_a_gap_in_the_measure() {
    let atlas = "hero.png\nfilter: Linear, Linear\n\nhero_3.png\nfilter: MipMap, Linear\n";
    let inventory = inventory_of(vec![
        ("spine/hero.png", FileKind::Image, png_file(64, 64)),
        ("spine/hero.atlas", FileKind::Data, atlas.into()),
    ]);
    let scan = textures::scan(&inventory, Engine::Unknown);
    // A missing page is a gap in the measure, not a file left out of the total.
    assert!(scan.summary.not_covered.is_empty());
    assert_eq!(
        scan.measures[0].scope.not_covered,
        ["spine/hero.atlas: page hero_3.png is not in the build (spine_page_missing)"]
    );
}

#[test]
fn a_missing_spine_page_listed_twice_is_one_gap() {
    let atlas = "hero_3.png\nfilter: Linear, Linear\n\nhero_3.png\nfilter: Linear, Linear\n";
    let inventory = inventory_of(vec![("hero.atlas", FileKind::Data, atlas.into())]);
    let measure = &textures::scan(&inventory, Engine::Unknown).measures[0];
    assert_eq!(
        measure.scope.not_covered,
        ["hero.atlas: page hero_3.png is not in the build (spine_page_missing)"]
    );
}

#[test]
fn a_spine_page_outside_the_build_is_a_gap_in_the_measure() {
    let atlas = "../hero.png\nfilter: Linear, Linear\n";
    let inventory = inventory_of(vec![
        ("hero.png", FileKind::Image, png_file(64, 64)),
        ("hero.atlas", FileKind::Data, atlas.into()),
    ]);
    let measure = &textures::scan(&inventory, Engine::Unknown).measures[0];
    assert_eq!(
        measure.scope.not_covered,
        ["hero.atlas: page ../hero.png is not in the build (spine_page_missing)"]
    );
}

// ---- Raster images ----

#[test]
fn a_png_whose_first_chunk_is_not_ihdr_is_invalid() {
    let mut bytes = png_file(64, 64);
    bytes[PNG_IHDR_TYPE_AT..PNG_IHDR_TYPE_AT + 4].copy_from_slice(b"tEXt");
    assert_eq!(
        read_one("a.png", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

#[test]
fn a_png_cut_inside_ihdr_is_invalid() {
    let mut bytes = png_file(64, 64);
    bytes.truncate(PNG_IHDR_END - 1);
    assert_eq!(
        read_one("a.png", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

#[test]
fn a_png_whose_signature_ends_wrong_is_invalid() {
    let mut bytes = png_file(64, 64);
    // A byte past the ones `imagesize` checks.
    bytes[PNG_SIGNATURE_BYTES_IMAGESIZE_CHECKS] = b'X';
    assert_eq!(
        read_one("a.png", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

#[test]
fn a_png_with_no_width_or_no_height_is_invalid() {
    for (width, height) in [(0, 64), (64, 0)] {
        assert_eq!(
            read_one("a.png", png_file(width, height)),
            Err(NotCovered::Invalid(DAMAGED_HEADER)),
            "{width}x{height}"
        );
    }
}

#[test]
fn a_file_named_png_that_holds_no_image_is_invalid() {
    let bytes = b"<html>not an image</html>".to_vec();
    assert_eq!(
        read_one("a.png", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

#[test]
fn a_png_named_svg_reads_as_a_png() {
    let decoded = read_one("icon.svg", png_file(10, 10)).expect("estimated");
    assert_eq!(decoded.format, TextureFormat::Png);
}

#[test]
fn a_lossy_webp_size_leaves_out_the_upscaling_bits() {
    let mut bytes = images_fixture_file("webp/lossy-512x256.webp");
    // The 2 bits above each 14-bit side ask the decoder to upscale. They are not part of the size.
    put_le_u16(&mut bytes, VP8_WIDTH_AT, 512 | (1 << 14));
    put_le_u16(&mut bytes, VP8_HEIGHT_AT, 256 | (3 << 14));
    let decoded = read_one("a.webp", bytes).expect("estimated");
    assert_eq!((decoded.width, decoded.height), (512, 256));
}

#[test]
fn a_lossless_webp_may_use_every_bit_of_its_size() {
    // A lossless WebP stores each side minus 1 in 14 bits, so 16384 is its widest. The low 14
    // bits hold the width minus 1, and the next 14 the height minus 1 (here 0).
    let packed_sides: u32 = 16_383;
    let mut bytes = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0".to_vec();
    bytes.push(VP8L_SIGNATURE);
    bytes.extend_from_slice(&packed_sides.to_le_bytes());
    // Pad so the reader sees a whole header.
    bytes.resize(64, 0);
    let decoded = read_one("a.webp", bytes).expect("estimated");
    assert_eq!((decoded.width, decoded.height), (16_384, 1));
}

#[test]
fn an_avif_brand_anywhere_in_ftyp_makes_an_avif() {
    let cases = [
        // "avif" is the major brand alone.
        (b"avif", b"mif1miafMA1BMiPr"),
        // The major brand and first two compatible brands are generic HEIF. "avif" comes third.
        (b"mif1", b"mif1miafavifMA1B"),
    ];
    for (major, compatible) in cases {
        let bytes = avif_fixture_with_brands(major, compatible);
        let decoded = read_one("a.avif", bytes).expect("estimated");
        assert_eq!(decoded.format, TextureFormat::Avif);
        assert_eq!((decoded.width, decoded.height), (640, 360));
    }
}

#[test]
fn heic_and_other_heif_images_are_unsupported() {
    let heic = avif_fixture_with_brands(b"heic", b"mif1miafheicMiPr");
    assert_eq!(read_one("a.heic", heic), Err(NotCovered::Unsupported(HEIC)));
    let heif = avif_fixture_with_brands(b"mif1", b"mif1miafMiPrmiaf");
    assert_eq!(read_one("a.heif", heif), Err(NotCovered::Unsupported(HEIF)));
}

#[test]
fn a_brand_past_the_end_of_ftyp_does_not_count() {
    let mut bytes = avif_fixture_with_brands(b"mif1", b"mif1miafMiPravif");
    // Shrink the box by one brand, so "avif" sits just past its end.
    put_be_u32(&mut bytes, 0, 28);
    assert_eq!(
        read_one("a.heif", bytes),
        Err(NotCovered::Unsupported(HEIF))
    );
}

#[test]
fn other_image_formats_are_unsupported() {
    let cases: [(&[u8], &str); 3] = [
        (b"BM", BMP),
        (b"\0\0\x01\0", ICO),
        (b"qoif", OTHER_IMAGE_FORMAT),
    ];
    for (magic, reason) in cases {
        let mut bytes = magic.to_vec();
        // Pad so the reader sees a whole header.
        bytes.resize(64, 0);
        assert_eq!(
            read_one("image.bin", bytes),
            Err(NotCovered::Unsupported(reason)),
            "{reason}"
        );
    }
}

// ---- SVG ----

#[test]
fn an_svg_counts_at_the_size_its_root_element_gives() {
    let decoded = read_one("icon.svg", svg_file(r#"width="80" height="40""#)).expect("estimated");
    assert_eq!(decoded.format, TextureFormat::Svg);
    assert_eq!((decoded.width, decoded.height), (80, 40));
    assert_eq!(decoded.levels, 1);
    assert_eq!(decoded.mips, MipSource::NoMips);
    assert_eq!(decoded.gpu_format, GpuFormat::Rgba8);
    assert_eq!(decoded.gpu_bytes, 80 * 40 * 4);
    // 20 x 10 blocks of 16 bytes.
    assert_eq!(decoded.astc_bytes, 20 * 10 * 16);
    assert_eq!(decoded.note, Some(Note::SvgOwnSize));
}

#[test]
fn an_svg_size_in_another_absolute_unit_converts_to_px() {
    let decoded =
        read_one("icon.svg", svg_file(r#"width="1in" height="36pt""#)).expect("estimated");
    assert_eq!((decoded.width, decoded.height), (96, 48));
}

#[test]
fn pixi_7_makes_mips_for_an_svg_whose_sides_are_powers_of_two() {
    let inventory = inventory_of(vec![(
        "icon.svg",
        FileKind::Image,
        svg_file(r#"width="64" height="64""#),
    )]);
    let report = textures::read(&inventory, Engine::Pixi7);
    let decoded = report.files[0].outcome.as_ref().expect("estimated");
    assert_eq!(decoded.mips, MipSource::Pixi7Pow2);
    assert_eq!(decoded.levels, 7);
}

#[test]
fn an_svg_with_only_a_view_box_has_no_size_whatever_the_case_of_its_extension() {
    let bytes = svg_file(r#"viewBox="0 0 256 256""#);
    assert_eq!(
        read_one("icon.SVG", bytes),
        Err(NotCovered::Unsupported(SVG_NO_SIZE))
    );
}

#[test]
fn an_svg_with_one_side_that_is_not_absolute_has_no_size() {
    for size in [
        r#"width="100""#,
        r#"width="100" height="100%""#,
        r#"width="10em" height="100""#,
        r#"width="auto" height="auto""#,
    ] {
        assert_eq!(
            read_one("icon.svg", svg_file(size)),
            Err(NotCovered::Unsupported(SVG_NO_SIZE)),
            "{size}"
        );
    }
}

#[test]
fn an_svg_root_may_follow_a_declaration_a_comment_and_a_doctype() {
    let bytes = concat!(
        "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!-- exported -->\n",
        "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" ",
        "\"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">\n",
        "<svg:svg xmlns:svg=\"http://www.w3.org/2000/svg\" width=\"&#x33;2\" height=\"16\">",
        "</svg:svg>",
    );
    let decoded = read_one("icon.svg", bytes.into()).expect("estimated");
    assert_eq!((decoded.width, decoded.height), (32, 16));
}

#[test]
fn an_svg_root_may_use_an_entity_its_doctype_declares_in_another_attribute() {
    // Illustrator writes its namespaces this way.
    let bytes = concat!(
        "<!DOCTYPE svg [<!ENTITY ns_extend \"http://ns.adobe.com/Extensibility/1.0/\">]>",
        "<svg xmlns:x=\"&ns_extend;\" xmlns=\"http://www.w3.org/2000/svg\" width=\"80\" ",
        "height=\"40\"></svg>",
    );
    let decoded = read_one("icon.svg", bytes.into()).expect("estimated");
    assert_eq!((decoded.width, decoded.height), (80, 40));
}

#[test]
fn a_file_named_svg_with_no_svg_root_is_invalid() {
    let cases: [&[u8]; 5] = [
        b"",
        b"not an image",
        b"<html><svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"/></html>",
        // A root `svg` outside the SVG namespace is not an SVG to the browser.
        b"<svg width=\"8\" height=\"8\"/>",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"",
    ];
    for bytes in cases {
        assert_eq!(
            read_one("icon.svg", bytes.to_vec()),
            Err(NotCovered::Invalid(DAMAGED_HEADER)),
            "{}",
            String::from_utf8_lossy(bytes)
        );
    }
}

#[test]
fn an_svg_root_with_a_damaged_attribute_is_invalid() {
    let bytes = svg_file("width=80 height=40");
    assert_eq!(
        read_one("icon.svg", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

#[test]
fn an_svg_with_a_size_is_in_the_total_and_a_gap_in_the_measure() {
    let inventory = inventory_of(vec![(
        "icon.svg",
        FileKind::Image,
        svg_file(r#"width="80" height="40""#),
    )]);
    let scan = textures::scan(&inventory, Engine::Unknown);
    assert_eq!(scan.summary.low_bytes, 80 * 40 * 4);
    assert!(scan.summary.not_covered.is_empty());
    assert_eq!(
        scan.measures[0].scope.not_covered,
        [
            "icon.svg: counted at its own size, 80 x 40, not the size the game draws it at \
             (svg_own_size)"
        ]
    );
}

#[test]
fn an_svg_with_no_size_is_left_out_of_the_total() {
    let inventory = inventory_of(vec![(
        "icon.svg",
        FileKind::Image,
        svg_file(r#"viewBox="0 0 8 8""#),
    )]);
    let scan = textures::scan(&inventory, Engine::Unknown);
    assert_eq!(scan.summary.low_bytes, 0);
    assert_eq!(path_strings(&scan.summary.not_covered), ["icon.svg"]);
    assert_eq!(
        scan.measures[0].scope.not_covered,
        ["icon.svg: unsupported (svg_no_size)"]
    );
}

#[test]
fn a_dot_in_a_folder_name_is_not_the_file_extension() {
    let bytes = b"not an image".to_vec();
    assert_eq!(
        read_one("icons.svg/logo", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

// ---- KTX2 and other GPU texture containers ----

#[test]
fn texture_containers_we_do_not_model_are_unsupported_by_their_magic() {
    let cases: [(&[u8], &str); 5] = [
        (b"\xABKTX 11\xBB\r\n\x1a\n", KTX1),
        (b"DDS \x7c\x00\x00\x00", DDS),
        (b"PVR\x03\x00\x00\x00\x00", PVR),
        // PVR 2 starts with the length of its header, 52.
        (b"\x34\x00\x00\x00", PVR),
        (b"\x13\xAB\xA1\x5C\x04\x04\x01", ASTC),
    ];
    for (magic, reason) in cases {
        let mut bytes = magic.to_vec();
        // Pad so the reader sees a whole header.
        bytes.resize(128, 0);
        assert_eq!(
            read_one("texture.bin", bytes),
            Err(NotCovered::Unsupported(reason)),
            "{reason}"
        );
    }
}

#[test]
fn texture_containers_we_do_not_model_are_unsupported_by_their_extension() {
    for (extension, reason) in [("ktx", KTX1), ("dds", DDS), ("pvr", PVR), ("astc", ASTC)] {
        assert_eq!(
            read_one(&format!("texture.{extension}"), vec![0; 128]),
            Err(NotCovered::Unsupported(reason)),
            "{extension}"
        );
    }
}

#[test]
fn a_ktx2_named_png_reads_as_a_ktx2() {
    let bytes = images_fixture_file("ktx2/uastc-512x256.ktx2");
    let decoded = read_one("symbols.png", bytes).expect("estimated");
    assert_eq!(decoded.format, TextureFormat::Ktx2);
}

#[test]
fn a_ktx2_in_a_bc_format_is_unsupported_on_an_iphone() {
    for vk_format in [VK_FORMAT_BC1_RGB_UNORM_BLOCK, VK_FORMAT_BC7_SRGB_BLOCK] {
        let mut bytes = images_fixture_file("ktx2/rgba8-mips-64x64.ktx2");
        put_le_u32(&mut bytes, KTX2_VK_FORMAT_AT, vk_format);
        assert_eq!(
            read_one("a.ktx2", bytes),
            Err(NotCovered::Unsupported(BC_ON_IPHONE)),
            "{vk_format}"
        );
    }
}

#[test]
fn a_ktx2_in_another_raw_format_is_its_stored_size_under_its_vk_format() {
    let rgba8 =
        read_one("a.ktx2", images_fixture_file("ktx2/rgba8-mips-64x64.ktx2")).expect("estimated");
    let mut bytes = images_fixture_file("ktx2/rgba8-mips-64x64.ktx2");
    put_le_u32(&mut bytes, KTX2_VK_FORMAT_AT, VK_FORMAT_R16G16B16A16_SFLOAT);
    let decoded = read_one("a.ktx2", bytes).expect("estimated");
    assert_eq!(
        decoded.gpu_format,
        GpuFormat::OtherVkFormat(VK_FORMAT_R16G16B16A16_SFLOAT)
    );
    assert_eq!(decoded.gpu_format.to_string(), "vk_format_97");
    assert_eq!(decoded.gpu_bytes, rgba8.gpu_bytes);
}

#[test]
fn a_ktx2_with_no_levels_counts_level_0_and_notes_the_runtime_mips() {
    let decoded = read_one("a.ktx2", rgba8_ktx2_with_level_count_0()).expect("estimated");
    assert_eq!(decoded.levels, 1);
    assert_eq!(decoded.gpu_bytes, 64 * 64 * 4);
    assert_eq!(decoded.note, Some(Note::RuntimeMips));
}

#[test]
fn runtime_mips_are_a_gap_in_the_measure() {
    let inventory = inventory_of(vec![(
        "a.ktx2",
        FileKind::Texture,
        rgba8_ktx2_with_level_count_0(),
    )]);
    let measure = &textures::scan(&inventory, Engine::Unknown).measures[0];
    assert_eq!(
        measure.scope.not_covered,
        ["a.ktx2: mips made at run time are not counted (runtime_mips)"]
    );
}

#[test]
fn a_gif_counts_its_first_frame_and_is_not_a_gap() {
    let gif = images_fixture_file("gif/spinner-32x32.gif");
    let decoded = read_one("a.gif", gif.clone()).expect("estimated");
    assert_eq!(decoded.note, Some(Note::FirstFrameOnly));
    assert_eq!(decoded.gpu_bytes, 32 * 32 * 4);
    let inventory = inventory_of(vec![("a.gif", FileKind::Image, gif)]);
    let measure = &textures::scan(&inventory, Engine::Unknown).measures[0];
    assert!(measure.scope.not_covered.is_empty());
}

#[test]
fn the_measure_sorts_its_gaps_by_path() {
    // The scan builds file gaps before page gaps, so b.ktx2 comes first until the sort.
    let inventory = inventory_of(vec![
        ("a.atlas", FileKind::Data, "hero.png\n".into()),
        ("b.ktx2", FileKind::Texture, rgba8_ktx2_with_level_count_0()),
    ]);
    let measure = &textures::scan(&inventory, Engine::Unknown).measures[0];
    assert_eq!(
        measure.scope.not_covered,
        [
            "a.atlas: page hero.png is not in the build (spine_page_missing)",
            "b.ktx2: mips made at run time are not counted (runtime_mips)",
        ]
    );
}

#[test]
fn a_basis_ktx2_cube_map_counts_six_faces() {
    let mut bytes = images_fixture_file("ktx2/uastc-512x256.ktx2");
    put_le_u32(&mut bytes, KTX2_FACE_COUNT_AT, 6);
    let decoded = read_one("a.ktx2", bytes).expect("estimated");
    assert_eq!(decoded.gpu_bytes, 6 * UASTC_512X256_BYTES);
    assert_eq!(decoded.astc_bytes, 6 * UASTC_512X256_BYTES);
}

#[test]
fn a_basis_ktx2_array_counts_every_layer() {
    let mut bytes = images_fixture_file("ktx2/uastc-512x256.ktx2");
    put_le_u32(&mut bytes, KTX2_LAYER_COUNT_AT, 3);
    let decoded = read_one("a.ktx2", bytes).expect("estimated");
    assert_eq!(decoded.gpu_bytes, 3 * UASTC_512X256_BYTES);
    assert_eq!(decoded.astc_bytes, 3 * UASTC_512X256_BYTES);
}

#[test]
fn a_basis_ktx2_volume_counts_every_depth_slice() {
    let mut bytes = images_fixture_file("ktx2/uastc-512x256.ktx2");
    put_le_u32(&mut bytes, KTX2_PIXEL_DEPTH_AT, 4);
    let decoded = read_one("a.ktx2", bytes).expect("estimated");
    assert_eq!(decoded.gpu_bytes, 4 * UASTC_512X256_BYTES);
    assert_eq!(decoded.astc_bytes, 4 * UASTC_512X256_BYTES);
}

#[test]
fn a_1d_ktx2_is_one_pixel_high() {
    let mut bytes = images_fixture_file("ktx2/rgba8-mips-64x64.ktx2");
    put_le_u32(&mut bytes, KTX2_PIXEL_HEIGHT_AT, 0);
    let decoded = read_one("a.ktx2", bytes).expect("estimated");
    assert_eq!(decoded.height, 1);
}

#[test]
fn uastc_may_be_stored_plain_or_in_zstandard_or_zlib() {
    let schemes = [
        KTX2_SUPERCOMPRESSION_NONE,
        KTX2_SUPERCOMPRESSION_ZSTANDARD,
        KTX2_SUPERCOMPRESSION_ZLIB,
    ];
    for scheme in schemes {
        let mut bytes = images_fixture_file("ktx2/uastc-512x256.ktx2");
        put_le_u32(&mut bytes, KTX2_SUPERCOMPRESSION_AT, scheme);
        let decoded = read_one("a.ktx2", bytes).expect("estimated");
        assert_eq!(decoded.encoding, Some(Encoding::Uastc), "{scheme}");
    }
}

#[test]
fn a_supercompressed_raw_ktx2_costs_the_lengths_it_inflates_to() {
    let mut bytes = images_fixture_file("ktx2/rgba8-mips-64x64.ktx2");
    let plain = read_one("a.ktx2", bytes.clone()).expect("estimated");
    put_le_u32(
        &mut bytes,
        KTX2_SUPERCOMPRESSION_AT,
        KTX2_SUPERCOMPRESSION_ZSTANDARD,
    );
    // The stored level stays 64x64x4 bytes, and the header now says it inflates to twice that.
    let level_0_bytes = 64 * 64 * 4;
    put_le_u64(
        &mut bytes,
        KTX2_LEVEL_0_UNCOMPRESSED_LENGTH_AT,
        2 * level_0_bytes,
    );
    let decoded = read_one("a.ktx2", bytes).expect("estimated");
    assert_eq!(decoded.gpu_bytes, plain.gpu_bytes + level_0_bytes);
}

#[test]
fn etc1s_without_basis_lz_and_uastc_with_it_are_unsupported() {
    let mut etc1s = images_fixture_file("ktx2/etc1s-mips-1024x1024.ktx2");
    put_le_u32(
        &mut etc1s,
        KTX2_SUPERCOMPRESSION_AT,
        KTX2_SUPERCOMPRESSION_NONE,
    );
    assert_eq!(
        read_one("a.ktx2", etc1s),
        Err(NotCovered::Unsupported(BASIS_FORMAT))
    );
    let mut uastc = images_fixture_file("ktx2/uastc-512x256.ktx2");
    put_le_u32(
        &mut uastc,
        KTX2_SUPERCOMPRESSION_AT,
        KTX2_SUPERCOMPRESSION_BASIS_LZ,
    );
    assert_eq!(
        read_one("a.ktx2", uastc),
        Err(NotCovered::Unsupported(BASIS_FORMAT))
    );
}

#[test]
fn a_raw_ktx2_with_basis_lz_is_invalid() {
    let mut bytes = images_fixture_file("ktx2/rgba8-mips-64x64.ktx2");
    put_le_u32(
        &mut bytes,
        KTX2_SUPERCOMPRESSION_AT,
        KTX2_SUPERCOMPRESSION_BASIS_LZ,
    );
    assert_eq!(
        read_one("a.ktx2", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

#[test]
fn a_plain_ktx2_level_whose_two_lengths_differ_is_invalid() {
    let mut bytes = images_fixture_file("ktx2/rgba8-mips-64x64.ktx2");
    put_le_u64(&mut bytes, KTX2_LEVEL_0_UNCOMPRESSED_LENGTH_AT, u64::MAX);
    assert_eq!(
        read_one("a.ktx2", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

#[test]
fn a_ktx2_with_a_texel_block_dimension_of_256_is_invalid_and_does_not_panic() {
    let mut bytes = images_fixture_file("ktx2/rgba8-mips-64x64.ktx2");
    let dfd_at = le_u32(&bytes, KTX2_DFD_OFFSET_AT) as usize;
    // The file stores each dimension minus 1, so 255 means 256.
    bytes[dfd_at + KTX2_DFD_FIRST_TEXEL_BLOCK_DIMENSION_AT] = 255;
    assert_eq!(
        read_one("a.ktx2", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

#[test]
fn a_texel_block_dimension_of_256_outside_a_basic_dfd_block_is_fine() {
    let plain = images_fixture_file("ktx2/rgba8-mips-64x64.ktx2");
    let mut bytes = plain.clone();
    let dfd_at = le_u32(&bytes, KTX2_DFD_OFFSET_AT) as usize;
    // Vendor 1 makes the block one only that vendor reads.
    bytes[dfd_at + KTX2_DFD_FIRST_BLOCK_VENDOR_AT] = 1;
    bytes[dfd_at + KTX2_DFD_FIRST_TEXEL_BLOCK_DIMENSION_AT] = 255;
    assert_eq!(read_one("a.ktx2", bytes), read_one("a.ktx2", plain));
}

#[test]
fn a_ktx2_with_a_dfd_block_of_size_0_is_rejected_and_does_not_hang() {
    let mut bytes = images_fixture_file("ktx2/rgba8-mips-64x64.ktx2");
    let dfd_at = le_u32(&bytes, KTX2_DFD_OFFSET_AT) as usize;
    put_le_u16(&mut bytes, dfd_at + KTX2_DFD_FIRST_BLOCK_SIZE_AT, 0);
    // The crate reads a block of size 0 as a DFD that ends early.
    assert_eq!(
        read_one("a.ktx2", bytes),
        Err(NotCovered::Invalid(TRUNCATED))
    );
}

#[test]
fn a_file_named_ktx2_without_the_magic_is_invalid() {
    let mut bytes = images_fixture_file("ktx2/uastc-512x256.ktx2");
    // Break the K of the magic.
    bytes[1] = b'X';
    assert_eq!(
        read_one("a.ktx2", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

// ---- Basis Universal files ----

#[test]
fn a_basis_file_counts_each_level_of_image_0_in_astc() {
    let bytes = basis_file(&[
        BasisSlice::color(0, 256, 128),
        BasisSlice::color(1, 128, 64),
        BasisSlice::color(2, 64, 32),
    ]);
    let decoded = read_one("a.basis", bytes).expect("estimated");
    assert_eq!((decoded.width, decoded.height), (256, 128));
    assert_eq!(decoded.levels, 3);
    // 256x128, 128x64 and 64x32 in 4x4 blocks of 16 bytes.
    assert_eq!(decoded.gpu_bytes, 64 * 32 * 16 + 32 * 16 * 16 + 16 * 8 * 16);
}

#[test]
fn a_basis_file_named_png_reads_as_basis() {
    let bytes = basis_file(&[BasisSlice::color(0, 256, 128)]);
    let decoded = read_one("a.png", bytes).expect("estimated");
    assert_eq!(decoded.format, TextureFormat::Basis);
}

#[test]
fn etc1s_alpha_slices_do_not_set_the_level_size() {
    // An ETC1S file lists each level's colour slice, then its alpha slice. The odd alpha size
    // shows which slice was read.
    let bytes = basis_file(&[
        BasisSlice::color(0, 256, 128),
        BasisSlice::alpha(0, 512, 512),
    ]);
    let decoded = read_one("a.basis", bytes).expect("estimated");
    assert_eq!((decoded.width, decoded.height), (256, 128));
    assert_eq!(decoded.gpu_bytes, LEVEL_256X128_ASTC_BYTES);
}

#[test]
fn uastc_slices_with_the_alpha_flag_count() {
    let mut bytes = basis_file(&[BasisSlice::alpha(0, 256, 128)]);
    bytes[BASIS_TEXTURE_FORMAT_AT] = BASIS_TEXTURE_FORMAT_UASTC_LDR_4X4;
    let decoded = read_one("a.basis", bytes).expect("estimated");
    assert_eq!(decoded.encoding, Some(Encoding::Uastc));
    assert_eq!(decoded.gpu_bytes, LEVEL_256X128_ASTC_BYTES);
}

#[test]
fn a_2d_basis_file_counts_image_0_only() {
    let bytes = basis_file(&[
        BasisSlice::color(0, 512, 512).of_image(1),
        BasisSlice::color(0, 256, 128),
    ]);
    let decoded = read_one("a.basis", bytes).expect("estimated");
    assert_eq!((decoded.width, decoded.height), (256, 128));
    assert_eq!(decoded.gpu_bytes, LEVEL_256X128_ASTC_BYTES);
}

#[test]
fn a_basis_cube_map_counts_every_face() {
    let faces: Vec<BasisSlice> = (0..6)
        .map(|face| BasisSlice::color(0, 256, 128).of_image(face))
        .collect();
    let mut bytes = basis_file(&faces);
    bytes[BASIS_TEXTURE_TYPE_AT] = BASIS_TEXTURE_TYPE_CUBEMAP_ARRAY;
    let decoded = read_one("a.basis", bytes).expect("estimated");
    assert_eq!(decoded.levels, 1);
    assert_eq!(decoded.gpu_bytes, 6 * LEVEL_256X128_ASTC_BYTES);
}

#[test]
fn a_basis_video_counts_one_frame() {
    let frames: Vec<BasisSlice> = (0..4)
        .map(|frame| BasisSlice::color(0, 256, 128).of_image(frame))
        .collect();
    let mut bytes = basis_file(&frames);
    bytes[BASIS_TEXTURE_TYPE_AT] = BASIS_TEXTURE_TYPE_VIDEO_FRAMES;
    let decoded = read_one("a.basis", bytes).expect("estimated");
    assert_eq!(decoded.gpu_bytes, LEVEL_256X128_ASTC_BYTES);
}

#[test]
fn an_i_frame_flag_does_not_make_a_slice_alpha() {
    let frame = BasisSlice::color(0, 256, 128).with_flags(BASIS_SLICE_FLAG_I_FRAME);
    let mut bytes = basis_file(&[frame]);
    bytes[BASIS_TEXTURE_TYPE_AT] = BASIS_TEXTURE_TYPE_VIDEO_FRAMES;
    let decoded = read_one("a.basis", bytes).expect("estimated");
    assert_eq!(decoded.gpu_bytes, LEVEL_256X128_ASTC_BYTES);
}

#[test]
fn a_basis_file_without_level_0_of_image_0_is_invalid() {
    let bytes = basis_file(&[BasisSlice::color(1, 128, 64)]);
    assert_eq!(
        read_one("a.basis", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

#[test]
fn a_basis_file_whose_level_0_has_no_width_or_no_height_is_invalid() {
    for (width, height) in [(0, 128), (256, 0)] {
        let bytes = basis_file(&[BasisSlice::color(0, width, height)]);
        assert_eq!(
            read_one("a.basis", bytes),
            Err(NotCovered::Invalid(DAMAGED_HEADER)),
            "{width}x{height}"
        );
    }
}

#[test]
fn a_basis_file_whose_slice_data_runs_past_the_end_is_truncated() {
    let mut bytes = images_fixture_file("basis/etc1s-256x128.basis");
    // The last byte is the last slice's data.
    bytes.pop();
    assert_eq!(
        read_one("a.basis", bytes),
        Err(NotCovered::Invalid(TRUNCATED))
    );
}

#[test]
fn a_basis_file_whose_slice_table_runs_past_the_end_is_truncated() {
    let mut bytes = basis_file(&[BasisSlice::color(0, 64, 64)]);
    bytes.truncate(BASIS_HEADER_BYTES + BASIS_SLICE_BYTES - 1);
    assert_eq!(
        read_one("a.basis", bytes),
        Err(NotCovered::Invalid(TRUNCATED))
    );
}

#[test]
fn a_basis_file_cut_inside_its_header_is_truncated() {
    let mut bytes = images_fixture_file("basis/etc1s-256x128.basis");
    bytes.truncate(BASIS_HEADER_BYTES - 1);
    assert_eq!(
        read_one("a.basis", bytes),
        Err(NotCovered::Invalid(TRUNCATED))
    );
}

#[test]
fn a_basis_hdr_file_is_unsupported() {
    let mut bytes = images_fixture_file("basis/etc1s-256x128.basis");
    bytes[BASIS_TEXTURE_FORMAT_AT] = BASIS_TEXTURE_FORMAT_UASTC_HDR_4X4;
    assert_eq!(
        read_one("a.basis", bytes),
        Err(NotCovered::Unsupported(BASIS_FORMAT))
    );
}

#[test]
fn a_basis_file_of_another_version_is_unsupported() {
    let mut bytes = images_fixture_file("basis/etc1s-256x128.basis");
    put_le_u16(&mut bytes, BASIS_VERSION_AT, BASIS_VERSION_0X13 + 1);
    assert_eq!(
        read_one("a.basis", bytes),
        Err(NotCovered::Unsupported(BASIS_VERSION))
    );
}

#[test]
fn a_file_named_basis_without_the_signature_is_invalid() {
    let mut bytes = images_fixture_file("basis/etc1s-256x128.basis");
    bytes[0] = b'x';
    assert_eq!(
        read_one("a.basis", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

#[test]
fn a_file_named_basis_with_another_header_size_is_invalid() {
    let mut bytes = images_fixture_file("basis/etc1s-256x128.basis");
    // "sB" alone is too common to mark a Basis file, so the header size must match too.
    put_le_u16(
        &mut bytes,
        BASIS_HEADER_SIZE_AT,
        BASIS_HEADER_BYTES as u16 - 1,
    );
    assert_eq!(
        read_one("a.basis", bytes),
        Err(NotCovered::Invalid(DAMAGED_HEADER))
    );
}

// ---- Helpers ----

/// One slice description of a `.basis` file.
struct BasisSlice {
    image: u32,
    level: u8,
    flags: u8,
    width: u16,
    height: u16,
}

impl BasisSlice {
    /// A level of image 0 with no flags.
    fn color(level: u8, width: u16, height: u16) -> BasisSlice {
        BasisSlice {
            image: 0,
            level,
            flags: 0,
            width,
            height,
        }
    }

    /// A level of image 0 with the alpha flag set.
    fn alpha(level: u8, width: u16, height: u16) -> BasisSlice {
        BasisSlice::color(level, width, height).with_flags(BASIS_SLICE_FLAG_HAS_ALPHA)
    }

    /// The same slice in another image.
    fn of_image(self, image: u32) -> BasisSlice {
        BasisSlice { image, ..self }
    }

    /// The same slice with other flags.
    fn with_flags(self, flags: u8) -> BasisSlice {
        BasisSlice { flags, ..self }
    }
}

fn folder_fixtures_with_textures() -> Vec<Fixture> {
    let with_textures: Vec<Fixture> = fixtures()
        .into_iter()
        .filter(|fixture| !fixture.is_zip() && fixture.table("textures").is_some())
        .collect();
    assert!(!with_textures.is_empty(), "no fixture has [textures]");
    with_textures
}

fn engine_for_mips(fixture: &Fixture) -> Engine {
    match text(fixture.required_table("textures"), "engine_for_mips") {
        "unknown" => Engine::Unknown,
        "pixi7" => Engine::Pixi7,
        "pixi8" => Engine::Pixi8,
        other => panic!("{}: engine_for_mips {other:?}", fixture.name),
    }
}

/// A file as a `[[textures.file]]` row.
fn fixture_row(file: &TextureFile) -> Table {
    match &file.outcome {
        Ok(decoded) => estimated_row(&file.path, decoded),
        Err(not_covered) => not_covered_row(&file.path, *not_covered),
    }
}

fn not_covered_row(path: &RelPath, not_covered: NotCovered) -> Table {
    let mut row = Table::new();
    row.insert("path".into(), path.as_str().into());
    row.insert("status".into(), not_covered.status_word().into());
    row.insert("reason".into(), not_covered.reason().into());
    row
}

/// KTX2 and Basis rows have no `mips` key: their levels are the file's.
fn estimated_row(path: &RelPath, decoded: &DecodedTexture) -> Table {
    let mut row = Table::new();
    row.insert("path".into(), path.as_str().into());
    row.insert("status".into(), "estimated".into());
    row.insert("format".into(), decoded.format.as_str().into());
    if let Some(encoding) = decoded.encoding {
        row.insert("encoding".into(), encoding.as_str().into());
    }
    row.insert("width".into(), toml_integer(decoded.width.into()));
    row.insert("height".into(), toml_integer(decoded.height.into()));
    row.insert("levels".into(), toml_integer(decoded.levels.into()));
    if decoded.mips != MipSource::FileLevels {
        row.insert("mips".into(), decoded.mips.as_str().into());
    }
    row.insert("gpu_format".into(), decoded.gpu_format.to_string().into());
    row.insert("gpu_bytes".into(), toml_integer(decoded.gpu_bytes));
    row.insert("astc_bytes".into(), toml_integer(decoded.astc_bytes));
    if let Some(note) = decoded.note {
        row.insert("note".into(), note.as_str().into());
    }
    row
}

fn toml_integer(value: u64) -> Value {
    Value::Integer(i64::try_from(value).expect("fits in a TOML integer"))
}

/// The files the textures rule names: the largest by GPU bytes, ties by path.
fn largest_files(summary: &TextureSummary) -> Vec<String> {
    let mut files: Vec<_> = summary.files.iter().collect();
    files.sort_by(|left, right| {
        right
            .gpu_bytes
            .cmp(&left.gpu_bytes)
            .then_with(|| left.path.cmp(&right.path))
    });
    files
        .into_iter()
        .take(TOP_FILES)
        .map(|file| file.path.to_string())
        .collect()
}

fn path_strings(paths: &[RelPath]) -> Vec<String> {
    paths.iter().map(RelPath::to_string).collect()
}

fn text_array(table: &Table, key: &str) -> Vec<String> {
    table[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} is an array"))
        .iter()
        .map(|value| value.as_str().expect("a string").to_owned())
        .collect()
}

fn path(raw: &str) -> RelPath {
    RelPath::new(raw).unwrap()
}

fn decoded_file<'a>(files: &'a [TextureFile], path: &str) -> &'a DecodedTexture {
    let file = files
        .iter()
        .find(|file| file.path.as_str() == path)
        .unwrap_or_else(|| panic!("{path} is not in the report"));
    file.outcome
        .as_ref()
        .unwrap_or_else(|not_covered| panic!("{path}: {not_covered:?}"))
}

/// What the scan makes of one file, alone in a build, under `Engine::Unknown`, so a raster image
/// has one level. Its kind does not change the reader.
fn read_one(path: &str, bytes: Vec<u8>) -> Result<DecodedTexture, NotCovered> {
    let inventory = inventory_of(vec![(path, FileKind::Image, bytes)]);
    let mut report = textures::read(&inventory, Engine::Unknown);
    assert_eq!(report.files.len(), 1);
    report.files.remove(0).outcome
}

fn inventory_of(files: Vec<(&str, FileKind, Vec<u8>)>) -> Inventory {
    let mut manifest_files = Vec::new();
    let mut contents = BTreeMap::new();
    for (raw_path, kind, bytes) in files {
        manifest_files.push(ManifestFile {
            path: path(raw_path),
            bytes: bytes.len() as u64,
            sha256: String::new(),
            kind,
        });
        contents.insert(path(raw_path), bytes);
    }
    // The inventory lists its files sorted by path.
    manifest_files.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = BuildManifest {
        build_id: None,
        files: manifest_files,
    };
    Inventory::new(manifest, contents)
}

/// A PNG signature and IHDR chunk, enough for the header reader. The CRC is not checked.
fn png_file(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = PNG_SIGNATURE.to_vec();
    bytes.extend_from_slice(&PNG_IHDR_DATA_BYTES.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    // Bit depth 8, colour type 6 (RGBA), compression, filter and interlace 0.
    bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
    // The CRC.
    bytes.extend_from_slice(&[0; 4]);
    bytes
}

/// A version 0x13 `.basis` header and slice table. No slice has data. Texture format 0 is ETC1S
/// and texture type 0 is 2D, so both bytes stay 0 unless a test sets them.
fn basis_file(slices: &[BasisSlice]) -> Vec<u8> {
    let mut bytes = vec![0; BASIS_HEADER_BYTES];
    bytes[..2].copy_from_slice(b"sB");
    put_le_u16(&mut bytes, BASIS_VERSION_AT, BASIS_VERSION_0X13);
    put_le_u16(&mut bytes, BASIS_HEADER_SIZE_AT, BASIS_HEADER_BYTES as u16);
    put_le_u24(&mut bytes, BASIS_SLICE_COUNT_AT, slices.len() as u32);
    put_le_u32(&mut bytes, BASIS_SLICE_TABLE_AT, BASIS_HEADER_BYTES as u32);
    for slice in slices {
        let mut entry = vec![0; BASIS_SLICE_BYTES];
        put_le_u24(&mut entry, BASIS_SLICE_IMAGE_AT, slice.image);
        entry[BASIS_SLICE_LEVEL_AT] = slice.level;
        entry[BASIS_SLICE_FLAGS_AT] = slice.flags;
        put_le_u16(&mut entry, BASIS_SLICE_WIDTH_AT, slice.width);
        put_le_u16(&mut entry, BASIS_SLICE_HEIGHT_AT, slice.height);
        bytes.extend_from_slice(&entry);
    }
    bytes
}

fn rgba8_ktx2_with_level_count_0() -> Vec<u8> {
    let mut bytes = images_fixture_file("ktx2/rgba8-mips-64x64.ktx2");
    put_le_u32(&mut bytes, KTX2_LEVEL_COUNT_AT, 0);
    bytes
}

/// The images fixture's AVIF with other brands in its `ftyp` box. The box holds the major brand
/// at 8 and 4 compatible brands at 16.
fn avif_fixture_with_brands(major: &[u8; 4], compatible: &[u8; 16]) -> Vec<u8> {
    let mut bytes = images_fixture_file("avif/photo-640x360.avif");
    bytes[8..12].copy_from_slice(major);
    bytes[16..32].copy_from_slice(compatible);
    bytes
}

/// A file from the `images` fixture's `build/assets` folder.
/// An SVG whose root element has the SVG namespace, the given attributes and one shape.
fn svg_file(attributes: &str) -> Vec<u8> {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" {attributes}><circle cx=\"4\" cy=\"4\" \
         r=\"4\"/></svg>"
    )
    .into_bytes()
}

fn images_fixture_file(relative: &str) -> Vec<u8> {
    let path = fixture("images").build.join("assets").join(relative);
    fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn put_be_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_be_bytes());
}

fn put_le_u16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_le_u24(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 3].copy_from_slice(&value.to_le_bytes()[..3]);
}

fn put_le_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_le_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}
