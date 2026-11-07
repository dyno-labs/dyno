//! Level sizes, mip chains, and which raster images get mips (design section 7.3).
//!
//! Decision R5 in the design: count the exact chain only where we know mips exist, never a flat
//! extra third for every texture. All byte math saturates, so an absurd header cannot overflow a
//! total.

use crate::model::Engine;

/// Where a texture's mip levels come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MipSource {
    /// Level 0 only.
    NoMips,
    /// Pixi 7 makes a full chain for an image whose sides are both powers of two.
    Pixi7Pow2,
    /// A Spine atlas page whose min filter is a MipMap filter gets a full chain in any engine.
    SpineAtlasFilter,
    /// KTX2 and Basis files hold their own levels.
    FileLevels,
}

/// Every ASTC block is 16 bytes, whatever its footprint.
const ASTC_BLOCK_BYTES: u64 = 16;

/// Texels on each side of an ASTC 4x4 block.
const ASTC_BLOCK_SIDE: u32 = 4;

/// RGBA8 is 4 bytes a pixel.
const RGBA8_BYTES_PER_PIXEL: u64 = 4;

impl MipSource {
    /// The word the fixtures use for it.
    pub fn as_str(self) -> &'static str {
        match self {
            MipSource::NoMips => "none",
            MipSource::Pixi7Pow2 => "pixi7_pow2",
            MipSource::SpineAtlasFilter => "spine_atlas_filter",
            MipSource::FileLevels => "file_levels",
        }
    }
}

/// The mips the engine makes for a raster image (PNG, JPEG, WebP, AVIF or GIF).
///
/// A Spine page with a MipMap filter wins, because its runtime asks for mips in any engine.
pub fn raster_mips(engine: Engine, width: u32, height: u32, spine_mip_page: bool) -> MipSource {
    if spine_mip_page {
        return MipSource::SpineAtlasFilter;
    }
    let both_sides_pow2 = width.is_power_of_two() && height.is_power_of_two();
    if engine == Engine::Pixi7 && both_sides_pow2 {
        MipSource::Pixi7Pow2
    } else {
        MipSource::NoMips
    }
}

/// How many levels a mip source gives a raster image.
pub fn raster_levels(mips: MipSource, width: u32, height: u32) -> u32 {
    match mips {
        MipSource::Pixi7Pow2 | MipSource::SpineAtlasFilter => full_chain_levels(width, height),
        // A raster image never has FileLevels.
        MipSource::NoMips | MipSource::FileLevels => 1,
    }
}

/// `floor(log2(max(w, h))) + 1`: the levels from `w x h` down to 1 x 1.
fn full_chain_levels(width: u32, height: u32) -> u32 {
    // ilog2 panics on 0.
    width.max(height).max(1).ilog2() + 1
}

/// One side of level `level`: the side halves at each level, down to 1.
pub fn level_side(side: u32, level: u32) -> u32 {
    // checked_shr is None from 32 halvings on, where the side is 1 anyway.
    side.checked_shr(level).unwrap_or(0).max(1)
}

/// `w * h * 4`: one level decoded to RGBA8.
pub fn rgba8_bytes(width: u32, height: u32) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(RGBA8_BYTES_PER_PIXEL)
}

/// `ceil(w / 4) * ceil(h / 4) * 16`: one level in ASTC 4x4.
pub fn astc_4x4_bytes(width: u32, height: u32) -> u64 {
    let blocks_wide = u64::from(width.div_ceil(ASTC_BLOCK_SIDE));
    let blocks_high = u64::from(height.div_ceil(ASTC_BLOCK_SIDE));
    blocks_wide
        .saturating_mul(blocks_high)
        .saturating_mul(ASTC_BLOCK_BYTES)
}

/// The sum of `level_bytes` over the first `levels` levels of a `w x h` texture.
pub fn chain_bytes(width: u32, height: u32, levels: u32, level_bytes: fn(u32, u32) -> u64) -> u64 {
    (0..levels)
        .map(|level| level_bytes(level_side(width, level), level_side(height, level)))
        .fold(0, u64::saturating_add)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_chain_runs_down_to_one_pixel_on_the_long_side() {
        assert_eq!(full_chain_levels(1024, 1024), 11);
        assert_eq!(full_chain_levels(512, 256), 10);
        assert_eq!(full_chain_levels(1000, 600), 10);
        assert_eq!(full_chain_levels(1, 1), 1);
    }

    #[test]
    fn each_level_halves_a_side_but_never_below_one() {
        assert_eq!(level_side(512, 0), 512);
        assert_eq!(level_side(512, 1), 256);
        assert_eq!(level_side(256, 9), 1);
    }

    #[test]
    fn a_level_past_32_halvings_is_one_pixel() {
        assert_eq!(level_side(u32::MAX, 40), 1);
    }

    #[test]
    fn astc_rounds_each_side_up_to_whole_blocks() {
        assert_eq!(astc_4x4_bytes(100, 50), 25 * 13 * 16);
        assert_eq!(astc_4x4_bytes(1, 1), 16);
    }

    #[test]
    fn a_full_1024_rgba8_chain_sums_every_level() {
        let pixels = 1024 * 1024
            + 512 * 512
            + 256 * 256
            + 128 * 128
            + 64 * 64
            + 32 * 32
            + 16 * 16
            + 8 * 8
            + 4 * 4
            + 2 * 2
            + 1;
        assert_eq!(chain_bytes(1024, 1024, 11, rgba8_bytes), 4 * pixels);
    }

    #[test]
    fn a_512_by_256_astc_chain_sums_every_level_in_whole_blocks() {
        // The last three levels are smaller than a block, so each rounds up to 16 bytes.
        let expected = 131_072 + 32_768 + 8_192 + 2_048 + 512 + 128 + 32 + 16 + 16 + 16;
        assert_eq!(chain_bytes(512, 256, 10, astc_4x4_bytes), expected);
    }

    #[test]
    fn rgba8_bytes_saturate_instead_of_overflowing() {
        assert_eq!(rgba8_bytes(u32::MAX, u32::MAX), u64::MAX);
    }

    #[test]
    fn a_chain_saturates_instead_of_overflowing() {
        // Each level fits in a u64, but the two together do not.
        assert!(rgba8_bytes(u32::MAX, 1 << 30) < u64::MAX);
        assert_eq!(chain_bytes(u32::MAX, 1 << 30, 2, rgba8_bytes), u64::MAX);
    }

    #[test]
    fn pixi_7_makes_mips_only_when_both_sides_are_powers_of_two() {
        let spine_mip_page = false;
        assert_eq!(
            raster_mips(Engine::Pixi7, 1024, 512, spine_mip_page),
            MipSource::Pixi7Pow2
        );
        assert_eq!(
            raster_mips(Engine::Pixi7, 1024, 600, spine_mip_page),
            MipSource::NoMips
        );
        assert_eq!(
            raster_mips(Engine::Pixi7, 1000, 600, spine_mip_page),
            MipSource::NoMips
        );
    }

    #[test]
    fn pixi_8_and_an_unknown_engine_make_no_mips() {
        let spine_mip_page = false;
        for engine in [Engine::Pixi8, Engine::Unknown] {
            assert_eq!(
                raster_mips(engine, 1024, 1024, spine_mip_page),
                MipSource::NoMips
            );
        }
    }

    #[test]
    fn a_spine_mipmap_page_gets_a_chain_in_any_engine_and_any_size() {
        let spine_mip_page = true;
        for engine in [Engine::Unknown, Engine::Pixi7, Engine::Pixi8] {
            for (width, height) in [(1000, 600), (1024, 1024)] {
                assert_eq!(
                    raster_mips(engine, width, height, spine_mip_page),
                    MipSource::SpineAtlasFilter,
                    "{engine:?} {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn without_mips_an_image_has_one_level() {
        assert_eq!(raster_levels(MipSource::NoMips, 1024, 1024), 1);
    }

    #[test]
    fn a_pixi_7_power_of_two_image_has_a_full_chain_of_levels() {
        assert_eq!(raster_levels(MipSource::Pixi7Pow2, 1024, 1024), 11);
    }
}
