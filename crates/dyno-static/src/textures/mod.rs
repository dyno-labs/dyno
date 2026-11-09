//! Textures: what every image and texture file holds on the GPU, from its header.
//!
//! A raster image costs width x height x 4 per level, and an SVG the same at its own size. A Basis
//! texture costs its ASTC 4x4 size, and a raw KTX2 its stored size. Mip levels come from the
//! engine, a Spine atlas or the file (design section 7). The value is one copy, and the range goes
//! up to two until a device run shows how many copies the browser keeps.

use std::collections::BTreeSet;
use std::fmt;

use dyno_schema::ids::MeasureId;
use dyno_schema::manifest::FileKind;
use dyno_schema::measure::{
    Evidence, Measure, MeasureItem, MeasureStatus, MeasureValue, Method, Uncertainty,
    UncertaintyKind,
};
use dyno_schema::path::RelPath;

use crate::model::{Engine, FamilyScan, Inventory, TextureCost, TextureSummary, saturating_i64};

use self::mips::{astc_4x4_bytes, chain_bytes, raster_levels, raster_mips, rgba8_bytes};
use self::raster::RasterHeader;

mod basis;
mod ktx2;
mod mips;
mod raster;
mod spine;
mod svg;

pub use self::mips::MipSource;
pub use self::spine::MissingPage;

/// What the scan read from every image and texture file, before it becomes a measure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureReport {
    /// Every image and texture file, sorted by path.
    pub files: Vec<TextureFile>,
    /// Spine atlas pages whose image is not in the build, sorted by atlas.
    pub missing_pages: BTreeSet<MissingPage>,
}

/// One image or texture file, and what the scan made of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureFile {
    pub path: RelPath,
    pub outcome: Result<DecodedTexture, NotCovered>,
}

/// What one file holds on the GPU once the browser has decoded or transcoded it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedTexture {
    pub format: TextureFormat,
    /// None for raster images. Raw, ETC1S or UASTC for KTX2, and ETC1S or UASTC for Basis.
    pub encoding: Option<Encoding>,
    pub width: u32,
    pub height: u32,
    /// The levels counted, level 0 included.
    pub levels: u32,
    pub mips: MipSource,
    pub gpu_format: GpuFormat,
    /// The bytes of every counted level in `gpu_format`, over every layer and face.
    pub gpu_bytes: u64,
    /// The same levels in ASTC 4x4. For an image smaller than a block, this is more than
    /// `gpu_bytes`.
    pub astc_bytes: u64,
    /// What the count leaves out.
    pub note: Option<Note>,
}

/// Why a file is not in the total. Each holds a reason word that the fixtures use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotCovered {
    /// A format we know but do not model, such as KTX1, or an SVG with no size of its own.
    Unsupported(&'static str),
    /// The header is damaged, or it promises bytes past the end of the file.
    Invalid(&'static str),
}

/// The file's container format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureFormat {
    Png,
    Jpeg,
    Webp,
    Avif,
    Gif,
    Svg,
    Ktx2,
    Basis,
}

/// How a KTX2 or Basis file stores its texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// A GPU format, uploaded as is.
    Raw,
    /// Basis ETC1S, transcoded on the phone.
    Etc1s,
    /// Basis UASTC LDR 4x4, transcoded on the phone.
    Uastc,
}

/// The format the GPU holds the texture in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuFormat {
    Rgba8,
    Astc4x4,
    /// Another format that a raw KTX2 file stores, by its `VkFormat` number.
    OtherVkFormat(u32),
}

/// What a file's count leaves out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    /// A GIF counts its first frame only.
    FirstFrameOnly,
    /// A KTX2 file with no levels asks the runtime to make them, and only level 0 counts.
    RuntimeMips,
    /// An SVG counts at its own size, which need not be the size the game draws it at.
    SvgOwnSize,
}

/// Which reader a file goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReaderKind {
    Raster,
    Svg,
    Ktx2,
    Basis,
    /// A format we know but do not model, with the reason word.
    Unsupported(&'static str),
}

// Reason words for a file that is not covered, as the fixtures spell them.

/// The header is damaged, or an image file is too short to give its size.
pub const DAMAGED_HEADER: &str = "header";
/// The file ends before the header or data it describes.
pub const TRUNCATED: &str = "truncated";
/// An SVG whose root element gives no absolute width and height, so the game sets its size.
pub const SVG_NO_SIZE: &str = "svg_no_size";
/// A KTX2 texture in a BC format, which an iPhone cannot sample.
pub const BC_ON_IPHONE: &str = "bc_on_iphone";
/// A Basis texture format other than ETC1S and UASTC LDR 4x4, such as HDR or XUASTC.
pub const BASIS_FORMAT: &str = "basis_format";
/// A `.basis` file of a version other than 0x13, whose layout may differ.
pub const BASIS_VERSION: &str = "basis_version";
/// A KTX version 1 texture.
pub const KTX1: &str = "ktx1";
/// A raw `.astc` texture.
pub const ASTC: &str = "astc";
/// A DirectDraw Surface texture.
pub const DDS: &str = "dds";
/// A PowerVR texture, version 2 or 3.
pub const PVR: &str = "pvr";
/// A HEIF image coded in HEVC.
pub const HEIC: &str = "heic";
/// Any other HEIF image that is not an AVIF.
pub const HEIF: &str = "heif";
/// A Windows bitmap.
pub const BMP: &str = "bmp";
/// A Windows icon.
pub const ICO: &str = "ico";
/// Any other image format `imagesize` knows, such as TIFF or QOI.
pub const OTHER_IMAGE_FORMAT: &str = "image_format";

/// A gap word: a Spine atlas names a page image that is not in the build.
pub const SPINE_PAGE_MISSING: &str = "spine_page_missing";

/// The measures this family owns.
pub const MEASURES: &[MeasureId] = &[MeasureId::StaticTexturesDecoded];

/// How many copies of each texture the high bound counts: the GPU's, and a CPU-side one the
/// browser may keep.
const HIGH_BOUND_COPIES: u64 = 2;

const EVIDENCE_DETAIL: &str = "width, height and levels from each image and texture header, \
                               and page filters from Spine atlas files";

const RANGE_NOTE: &str = "one copy of each texture to two, until a device run shows how many \
                          the browser keeps";

/// Container magic bytes, checked after Basis and before the raster check and the extension.
/// Basis is not here because its check also needs the header size at offset 4. DDS, PVR and
/// ASTC are not here because `imagesize` knows them, and the raster reader names them.
const MAGIC_READERS: &[(&[u8], ReaderKind)] = &[
    (ktx2::MAGIC, ReaderKind::Ktx2),
    (b"\xABKTX 11\xBB", ReaderKind::Unsupported(KTX1)),
];

/// Lowercase extensions that pick a reader when no magic matched. Any other file goes to the
/// raster reader. KTX2 and Basis are not here: their readers rely on the caller having checked
/// the magic, and the raster reader calls a file without it damaged. SVG has no magic: it is
/// text.
const EXTENSION_READERS: &[(&str, ReaderKind)] = &[
    ("ktx", ReaderKind::Unsupported(KTX1)),
    ("astc", ReaderKind::Unsupported(ASTC)),
    ("dds", ReaderKind::Unsupported(DDS)),
    ("pvr", ReaderKind::Unsupported(PVR)),
    ("svg", ReaderKind::Svg),
];

impl NotCovered {
    /// The fixtures' status word: "unsupported" or "invalid".
    pub fn status_word(self) -> &'static str {
        match self {
            NotCovered::Unsupported(_) => "unsupported",
            NotCovered::Invalid(_) => "invalid",
        }
    }

    /// The reason word, such as "svg_no_size" or "truncated".
    pub fn reason(self) -> &'static str {
        match self {
            NotCovered::Unsupported(reason) | NotCovered::Invalid(reason) => reason,
        }
    }
}

impl TextureFormat {
    /// The word the fixtures use for it.
    pub fn as_str(self) -> &'static str {
        match self {
            TextureFormat::Png => "png",
            TextureFormat::Jpeg => "jpeg",
            TextureFormat::Webp => "webp",
            TextureFormat::Avif => "avif",
            TextureFormat::Gif => "gif",
            TextureFormat::Svg => "svg",
            TextureFormat::Ktx2 => "ktx2",
            TextureFormat::Basis => "basis",
        }
    }
}

impl Encoding {
    /// The word the fixtures use for it.
    pub fn as_str(self) -> &'static str {
        match self {
            Encoding::Raw => "raw",
            Encoding::Etc1s => "etc1s",
            Encoding::Uastc => "uastc",
        }
    }
}

/// The word the fixtures use for it, or `vk_format_` and the number for another format.
impl fmt::Display for GpuFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuFormat::Rgba8 => formatter.write_str("rgba8"),
            GpuFormat::Astc4x4 => formatter.write_str("astc_4x4"),
            GpuFormat::OtherVkFormat(vk_format) => write!(formatter, "vk_format_{vk_format}"),
        }
    }
}

impl Note {
    /// The words the fixtures use for it.
    pub fn as_str(self) -> &'static str {
        match self {
            Note::FirstFrameOnly => "first frame only",
            Note::RuntimeMips => "runtime_mips",
            Note::SvgOwnSize => "svg_own_size",
        }
    }
}

impl TextureReport {
    /// The totals and per-file costs that later steps read.
    fn summary(&self) -> TextureSummary {
        let mut files = Vec::new();
        let mut not_covered = Vec::new();
        for file in &self.files {
            match &file.outcome {
                Ok(decoded) => files.push(TextureCost {
                    path: file.path.clone(),
                    gpu_bytes: decoded.gpu_bytes,
                    astc_bytes: decoded.astc_bytes,
                }),
                Err(_) => not_covered.push(file.path.clone()),
            }
        }
        let low_bytes = files
            .iter()
            .map(|file| file.gpu_bytes)
            .fold(0, u64::saturating_add);
        TextureSummary {
            low_bytes,
            high_bytes: low_bytes.saturating_mul(HIGH_BOUND_COPIES),
            files,
            not_covered,
        }
    }

    /// One plain-English line per gap in the total: each file that is not covered, each file
    /// whose runtime mips are not counted, each SVG counted at its own size, and each Spine page
    /// that is not in the build.
    fn gaps(&self) -> Vec<String> {
        let file_gaps = self.files.iter().filter_map(|file| {
            let path = &file.path;
            match &file.outcome {
                Err(not_covered) => {
                    let status = not_covered.status_word();
                    let reason = not_covered.reason();
                    Some(format!("{path}: {status} ({reason})"))
                }
                Ok(decoded) => match decoded.note {
                    Some(note @ Note::RuntimeMips) => Some(format!(
                        "{path}: mips made at run time are not counted ({})",
                        note.as_str()
                    )),
                    Some(note @ Note::SvgOwnSize) => Some(format!(
                        "{path}: counted at its own size, {} x {}, not the size the game draws \
                         it at ({})",
                        decoded.width,
                        decoded.height,
                        note.as_str()
                    )),
                    // A GIF's note is not a gap: the GPU holds one frame at a time.
                    Some(Note::FirstFrameOnly) | None => None,
                },
            }
        });
        let page_gaps = self
            .missing_pages
            .iter()
            .map(|MissingPage { atlas, image }| {
                format!("{atlas}: page {image} is not in the build ({SPINE_PAGE_MISSING})")
            });
        file_gaps.chain(page_gaps).collect()
    }
}

/// Reads each image and texture header and sums the GPU bytes. The engine, or a Spine atlas
/// filter, decides which raster images get mips.
pub fn scan(inventory: &Inventory, engine: Engine) -> FamilyScan<TextureSummary> {
    let report = read(inventory, engine);
    let summary = report.summary();
    FamilyScan {
        measures: vec![decoded_measure(&report, &summary)],
        summary,
    }
}

/// Reads every image and texture file in the build, and every Spine atlas for its page filters.
pub fn read(inventory: &Inventory, engine: Engine) -> TextureReport {
    let spine_links = spine::link(inventory);
    let files = inventory
        .files()
        .iter()
        .filter(|file| matches!(file.kind, FileKind::Image | FileKind::Texture))
        .map(|file| {
            // Never None, since the inventory holds every file it lists. Empty bytes would read
            // as a damaged header.
            let bytes = inventory.contents(&file.path).unwrap_or_default();
            let spine_mip_page = spine_links.mip_pages.contains(&file.path);
            TextureFile {
                path: file.path.clone(),
                outcome: read_file(&file.path, bytes, engine, spine_mip_page),
            }
        })
        .collect();
    TextureReport {
        files,
        missing_pages: spine_links.missing_pages,
    }
}

fn read_file(
    path: &RelPath,
    bytes: &[u8],
    engine: Engine,
    spine_mip_page: bool,
) -> Result<DecodedTexture, NotCovered> {
    match pick_reader(path, bytes) {
        ReaderKind::Raster => {
            raster::read(bytes).map(|header| raster_texture(header, engine, spine_mip_page))
        }
        // The browser draws an SVG to pixels, so it costs what a raster image of its size does.
        ReaderKind::Svg => {
            svg::read(bytes).map(|header| raster_texture(header, engine, spine_mip_page))
        }
        ReaderKind::Ktx2 => ktx2::read(bytes),
        ReaderKind::Basis => basis::read(bytes),
        ReaderKind::Unsupported(reason) => Err(NotCovered::Unsupported(reason)),
    }
}

/// Magic bytes first, then the extension, as the inventory buckets files (design section 3).
fn pick_reader(path: &RelPath, bytes: &[u8]) -> ReaderKind {
    if basis::has_signature(bytes) {
        return ReaderKind::Basis;
    }
    let by_magic = MAGIC_READERS
        .iter()
        .find(|(magic, _)| bytes.starts_with(magic));
    if let Some(&(_, reader)) = by_magic {
        return reader;
    }
    if raster::has_image_magic(bytes) {
        return ReaderKind::Raster;
    }
    let extension = lowercase_extension(path);
    EXTENSION_READERS
        .iter()
        .find(|(known, _)| *known == extension)
        .map_or(ReaderKind::Raster, |&(_, reader)| reader)
}

/// The file name's extension in lowercase, or "" when it has none. A dot in a folder name does
/// not count.
fn lowercase_extension(path: &RelPath) -> String {
    let file_name = path
        .as_str()
        .rsplit_once('/')
        .map_or(path.as_str(), |(_, name)| name);
    file_name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default()
}

/// A raster image, or an SVG at its own size, is RGBA8, with the levels its mip source gives.
fn raster_texture(header: RasterHeader, engine: Engine, spine_mip_page: bool) -> DecodedTexture {
    let RasterHeader {
        format,
        width,
        height,
        note,
    } = header;
    let mips = raster_mips(engine, width, height, spine_mip_page);
    let levels = raster_levels(mips, width, height);
    DecodedTexture {
        format,
        encoding: None,
        width,
        height,
        levels,
        mips,
        gpu_format: GpuFormat::Rgba8,
        gpu_bytes: chain_bytes(width, height, levels, rgba8_bytes),
        astc_bytes: chain_bytes(width, height, levels, astc_4x4_bytes),
        note,
    }
}

/// The one measure this family owns: one copy of every texture, with a range up to two.
fn decoded_measure(report: &TextureReport, summary: &TextureSummary) -> Measure {
    let mut measure = Measure::blank(MeasureId::StaticTexturesDecoded, MeasureStatus::Estimated);
    measure.value = Some(MeasureValue::Integer(saturating_i64(summary.low_bytes)));
    measure.items = summary
        .files
        .iter()
        .map(|file| MeasureItem {
            key: file.path.to_string(),
            value: MeasureValue::Integer(saturating_i64(file.gpu_bytes)),
        })
        .collect();
    measure.evidence = vec![Evidence {
        method: Method::FileHeader,
        detail: EVIDENCE_DETAIL.to_owned(),
    }];
    measure.scope.not_covered = report.gaps();
    measure.uncertainty = Uncertainty {
        kind: UncertaintyKind::Range,
        low: Some(saturating_i64(summary.low_bytes)),
        high: Some(saturating_i64(summary.high_bytes)),
        note: Some(RANGE_NOTE.to_owned()),
    };
    measure.sort_lists();
    measure
}
