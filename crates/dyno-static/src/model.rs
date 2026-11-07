//! The types that pass between measure families: the inventory every family reads, and the
//! summaries that later families and the rules read (design section 2).
//!
//! They are this crate's own and never enter a result. The result shape is `dyno-schema`'s, so
//! statuses here are `MeasureStatus`, paths are `RelPath` and the device facts come from
//! `HostProfile`.

use std::collections::BTreeMap;

use dyno_schema::ids::MeasureId;
use dyno_schema::manifest::{BuildManifest, FileKind, ManifestFile};
use dyno_schema::measure::{Measure, MeasureStatus};
use dyno_schema::path::RelPath;
use dyno_schema::profile::Viewport;

use crate::audio::StereoClass;

/// What a family's entry function gives back: its measures, and a summary for later steps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FamilyScan<S> {
    pub measures: Vec<Measure>,
    pub summary: S,
}

/// Every supplied file, with its contents.
///
/// The inventory reads every file in full to hash it, so it keeps the contents. Other families
/// read files only through it, so a folder and a .zip look the same to them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    /// None until the inventory is implemented.
    manifest: Option<BuildManifest>,
    contents: BTreeMap<RelPath, Vec<u8>>,
}

/// The rendering engine, as far as texture mipmaps go (design section 7.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Engine {
    #[default]
    Unknown,
    Pixi7,
    Pixi8,
}

/// What bundle found in the HTML, JavaScript and CSS.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BundleSummary {
    /// Textures use it to decide mipmaps.
    pub engine: Engine,
    /// Sorted by file, then offset.
    pub render_sites: Vec<RenderSite>,
    /// Every Howler call with a literal `src`, sorted by file, then offset. Audio reads them.
    pub howl_sites: Vec<HowlSite>,
    /// Scripts that did not parse, sorted. An inline script is its HTML file.
    pub invalid_scripts: Vec<RelPath>,
    /// Whether the build shows that the game draws with HTML and CSS, with no canvas.
    pub draws_with_html: bool,
}

/// An options object that configures the renderer (design section 8.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderSite {
    /// The script, or the HTML file of an inline script.
    pub file: RelPath,
    /// The byte offset of the options object from the start of `file`.
    pub offset: u32,
    /// None when the value is not static.
    pub antialias: Option<bool>,
    /// Whether the site draws to a 2D canvas, where antialias does nothing.
    pub draws_in_2d: bool,
    pub resolution: Resolution,
    /// The CSS size the canvas floor used.
    pub canvas_css: Viewport,
    /// None when the profile has none of the backends the site allows, so Pixi fails to start.
    pub canvas: Option<CanvasFloor>,
}

/// What the build proves about a resolution, in milli-units: a value between `low_milli` and
/// `high_milli`. A side is None when the build does not bound it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resolution {
    pub low_milli: Option<u32>,
    pub high_milli: Option<u32>,
}

/// A Howler call found in a script. Bundle finds it, and audio reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HowlSite {
    /// The script, or the HTML file of an inline script.
    pub file: RelPath,
    /// The byte offset of the call from the start of `file`.
    pub offset: u32,
    /// The `src` list as written, in its order. Audio resolves the paths.
    pub src: Vec<String>,
    /// Empty when the call has no `sprite` key.
    pub sprites: Vec<SpriteRange>,
}

/// One named range of a Howler sprite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpriteRange {
    pub name: String,
    pub start_us: u64,
    pub end_us: u64,
    pub looped: bool,
}

/// Decoded textures (design section 7).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextureSummary {
    /// The sum of `gpu_bytes` over `files`: one copy.
    pub low_bytes: u64,
    /// Two copies, until the phone calibration says how many the browser keeps.
    pub high_bytes: u64,
    /// Every estimated file, sorted by path.
    pub files: Vec<TextureCost>,
    /// Image and texture files that are not estimated, sorted.
    pub not_covered: Vec<RelPath>,
}

/// What one image or texture file costs on the GPU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureCost {
    pub path: RelPath,
    pub gpu_bytes: u64,
    /// The same levels as ASTC 4x4. The textures rule's saving is the difference.
    pub astc_bytes: u64,
}

/// Decoded audio, sound groups and sprite maps (design sections 5 and 6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioSummary {
    /// The total over sound groups. Each group counts once, at its representative.
    pub decoded_bytes_48k: u64,
    /// The same total at 24 kHz.
    pub decoded_bytes_24k: u64,
    /// Every estimated audio file with its own decoded bytes, sorted by path.
    pub files: Vec<DecodedAudio>,
    /// The representative of every stereo sound group, sorted by path.
    pub stereo_groups: Vec<StereoSound>,
    /// Every sprite map, from JSON files and Howl sites, sorted by source.
    pub sprite_maps: Vec<SpriteMapSummary>,
    /// Audio files that are not estimated, sorted.
    pub not_covered: Vec<RelPath>,
}

/// The representative of a stereo sound group, and what a mono export does to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StereoSound {
    pub path: RelPath,
    pub decoded_bytes_48k: u64,
    /// None when the sound did not decode.
    pub class: Option<StereoClass>,
}

/// One audio file and its decoded bytes at 48 kHz.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedAudio {
    pub path: RelPath,
    pub decoded_bytes_48k: u64,
}

/// The time no sprite covers in one sprite map's audio (design section 6.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpriteMapSummary {
    /// The JSON file, or the script with the Howl site.
    pub source: RelPath,
    /// Missing when its audio is not in the build, and invalid when the map does not parse.
    pub status: MeasureStatus,
    /// The sound group's representative. None unless the status is estimated.
    pub audio: Option<RelPath>,
    /// 0 unless the status is estimated.
    pub silence_bp: u32,
    /// 0 unless the status is estimated.
    pub silence_bytes_48k: u64,
}

/// Files with the same SHA-256 (design section 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateGroup {
    /// Lowercase hex, as in the manifest.
    pub sha256: String,
    /// The size of one copy.
    pub bytes: u64,
    /// Sorted.
    pub paths: Vec<RelPath>,
    /// The copy to keep: the only one the build's HTML, JavaScript, CSS or JSON names. None when
    /// the build names none of the copies, or more than one.
    pub keep: Option<RelPath>,
    /// (`paths.len()` - 1) x `bytes`.
    pub wasted_bytes: u64,
    /// (`paths.len()` - 1) x one copy's decoded bytes, or 0 when it does not decode.
    pub decoded_bytes_saved: u64,
}

/// Everything the rules read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summaries {
    pub bundle: BundleSummary,
    pub textures: TextureSummary,
    pub audio: AudioSummary,
    /// Sorted by first path.
    pub duplicates: Vec<DuplicateGroup>,
    /// The static measures with no value, in the order of the families. A family that gave no
    /// value leaves its summary empty, the same as a build with nothing of its kind, so a rule
    /// that reads one of these measures is incomplete, never a pass.
    pub measures_without_value: Vec<MeasureWithoutValue>,
}

/// A measure with no value, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasureWithoutValue {
    pub id: MeasureId,
    pub reason: String,
}

/// A canvas's backing store and its colour buffer: the least the canvas holds. Antialias
/// buffers and the GPU context come on top, and the device run measures them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CanvasFloor {
    /// In device pixels.
    pub backing_width: u32,
    /// In device pixels.
    pub backing_height: u32,
    pub pixels: u64,
    /// One colour buffer of the backing store.
    pub bytes: u64,
}

/// The resolution a fix caps the renderer at: 2. Above it, `static.render.resolution_uncapped`
/// is true. A proposal (design section 9).
pub const RESOLUTION_CAP_MILLI: u32 = 2_000;

/// The resolution the canvas floor takes when the build does not bound it from below: 1.
pub const FLOOR_RESOLUTION_MILLI: u32 = 1_000;

/// Bytes per pixel of a colour buffer: RGBA, 8 bits each.
pub const COLOUR_BUFFER_BYTES_PER_PIXEL: u64 = 4;

/// Milli-units in one: a resolution of 1.5 is 1,500.
const MILLI: u64 = 1_000;
const MILLI_U32: u32 = 1_000;

impl Resolution {
    /// A resolution the build fixes at one value.
    pub fn exactly(milli: u32) -> Resolution {
        Resolution {
            low_milli: Some(milli),
            high_milli: Some(milli),
        }
    }

    /// The one value, when the build fixes it.
    pub fn fixed(self) -> Option<u32> {
        match (self.low_milli, self.high_milli) {
            (Some(low), Some(high)) if low == high => Some(low),
            _ => None,
        }
    }

    /// Whether the resolution is above `cap_milli`. None when the build does not say.
    pub fn is_above(self, cap_milli: u32) -> Option<bool> {
        if self.low_milli.is_some_and(|low| low > cap_milli) {
            Some(true)
        } else if self.high_milli.is_some_and(|high| high <= cap_milli) {
            Some(false)
        } else {
            None
        }
    }

    /// The resolution the canvas floor takes: the lowest the build allows, else
    /// `FLOOR_RESOLUTION_MILLI`, or the highest the build allows when that is lower.
    pub fn floor_milli(self) -> u32 {
        match (self.low_milli, self.high_milli) {
            (Some(low), _) => low,
            (None, Some(high)) => high.min(FLOOR_RESOLUTION_MILLI),
            (None, None) => FLOOR_RESOLUTION_MILLI,
        }
    }

    /// Whether the floor takes `FLOOR_RESOLUTION_MILLI` or a high bound, not a low bound the
    /// build proves.
    pub fn floor_is_assumed(self) -> bool {
        self.low_milli.is_none()
    }
}

impl Inventory {
    /// An inventory of these files. `contents` holds the bytes of every file in the manifest.
    pub fn new(manifest: BuildManifest, contents: BTreeMap<RelPath, Vec<u8>>) -> Inventory {
        Inventory {
            manifest: Some(manifest),
            contents,
        }
    }

    /// Every file, sorted by path. Empty until the inventory is implemented.
    pub fn files(&self) -> &[ManifestFile] {
        match &self.manifest {
            Some(manifest) => &manifest.files,
            None => &[],
        }
    }

    /// The files of one kind, sorted by path.
    pub fn files_of_kind(&self, kind: FileKind) -> impl Iterator<Item = &ManifestFile> {
        self.files().iter().filter(move |file| file.kind == kind)
    }

    /// A file's bytes, if the build has the file.
    pub fn contents(&self, path: &RelPath) -> Option<&[u8]> {
        self.contents.get(path).map(Vec::as_slice)
    }

    /// The manifest for the result. None until the inventory is implemented.
    pub fn into_manifest(self) -> Option<BuildManifest> {
        self.manifest
    }
}

/// A count or a byte total as a measure value.
///
/// A total past `i64::MAX` becomes `i64::MAX`, so the result check marks the measure invalid
/// instead of keeping a wrapped number.
pub fn saturating_i64(value: impl TryInto<i64>) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

/// The one canvas formula (design section 8.4). Bundle and rules both call it.
///
/// The backing store is the CSS size times the resolution, rounded. Its colour buffer holds 4
/// bytes a pixel.
pub fn canvas_floor(css: Viewport, resolution_milli: u32) -> CanvasFloor {
    let backing_width = scale_by_milli(css.width, resolution_milli);
    let backing_height = scale_by_milli(css.height, resolution_milli);
    let pixels = u64::from(backing_width) * u64::from(backing_height);
    CanvasFloor {
        backing_width,
        backing_height,
        pixels,
        // Saturates so that an absurd resolution in a bundle cannot overflow.
        bytes: pixels.saturating_mul(COLOUR_BUFFER_BYTES_PER_PIXEL),
    }
}

/// A resolution in milli-units as a plain number: 3,000 is "3" and 1,500 is "1.5".
pub fn resolution_text(milli: u32) -> String {
    let whole = milli / MILLI_U32;
    match milli % MILLI_U32 {
        0 => whole.to_string(),
        fraction => {
            let digits = format!("{fraction:03}");
            format!("{whole}.{}", digits.trim_end_matches('0'))
        }
    }
}

/// `round(value * milli / 1000)` in integers, with halves rounding up.
fn scale_by_milli(value: u32, milli: u32) -> u32 {
    let scaled = (u64::from(value) * u64::from(milli) + MILLI / 2) / MILLI;
    u32::try_from(scaled).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_that_fits_keeps_its_number() {
        assert_eq!(saturating_i64(699_052_u64), 699_052);
        assert_eq!(saturating_i64(i64::MAX as u64), i64::MAX);
    }

    #[test]
    fn a_value_past_i64_becomes_i64_max_instead_of_wrapping() {
        assert_eq!(saturating_i64(i64::MAX as u64 + 1), i64::MAX);
        assert_eq!(saturating_i64(u64::MAX), i64::MAX);
    }

    // The Reels frame, and the fixed stage of fixture pixi7-opts.
    const FRAME: Viewport = Viewport {
        width: 402,
        height: 874,
    };

    const FIXED_STAGE: Viewport = Viewport {
        width: 720,
        height: 1280,
    };

    #[test]
    fn the_floor_at_dpr_3_is_4_bytes_a_backing_pixel() {
        let floor = canvas_floor(FRAME, 3_000);
        assert_eq!((floor.backing_width, floor.backing_height), (1206, 2622));
        assert_eq!(floor.pixels, 3_162_132);
        assert_eq!(floor.bytes, 12_648_528);
    }

    #[test]
    fn a_fixed_stage_scales_by_the_resolution() {
        let floor = canvas_floor(FIXED_STAGE, 3_000);
        assert_eq!((floor.backing_width, floor.backing_height), (2160, 3840));
        assert_eq!(floor.bytes, 33_177_600);
    }

    #[test]
    fn the_backing_store_rounds_half_pixels_up() {
        let odd = Viewport {
            width: 3,
            height: 1,
        };
        let floor = canvas_floor(odd, 1_500);
        assert_eq!((floor.backing_width, floor.backing_height), (5, 2));
    }

    #[test]
    fn an_absurd_resolution_saturates_instead_of_overflowing() {
        let floor = canvas_floor(FIXED_STAGE, u32::MAX);
        assert_eq!(floor.bytes, u64::MAX);
    }

    fn between(low_milli: Option<u32>, high_milli: Option<u32>) -> Resolution {
        Resolution {
            low_milli,
            high_milli,
        }
    }

    #[test]
    fn a_resolution_is_fixed_only_when_both_bounds_are_one_value() {
        assert_eq!(Resolution::exactly(2_000).fixed(), Some(2_000));
        assert_eq!(between(Some(1_500), Some(2_000)).fixed(), None);
        assert_eq!(between(None, Some(2_000)).fixed(), None);
    }

    #[test]
    fn a_resolution_is_above_the_cap_when_its_low_bound_is() {
        assert_eq!(between(Some(2_001), None).is_above(2_000), Some(true));
        assert_eq!(
            between(Some(1_500), Some(2_000)).is_above(2_000),
            Some(false)
        );
        assert_eq!(between(Some(1_500), Some(3_000)).is_above(2_000), None);
        assert_eq!(Resolution::default().is_above(2_000), None);
    }

    #[test]
    fn the_floor_takes_the_low_bound_else_resolution_1() {
        assert_eq!(between(Some(1_500), Some(2_000)).floor_milli(), 1_500);
        assert_eq!(between(None, Some(2_000)).floor_milli(), 1_000);
        assert_eq!(between(None, Some(500)).floor_milli(), 500);
        assert_eq!(Resolution::default().floor_milli(), 1_000);
        assert!(Resolution::default().floor_is_assumed());
        assert!(!between(Some(1_500), None).floor_is_assumed());
    }

    #[test]
    fn resolutions_show_as_plain_numbers() {
        assert_eq!(resolution_text(3_000), "3");
        assert_eq!(resolution_text(1_500), "1.5");
        assert_eq!(resolution_text(1_250), "1.25");
    }

    #[test]
    fn an_unbuilt_inventory_has_no_files_and_no_manifest() {
        let inventory = Inventory::default();
        assert!(inventory.files().is_empty());
        assert_eq!(inventory.into_manifest(), None);
    }
}
