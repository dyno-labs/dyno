//! KTX2 textures, read with the `ktx2` crate (design section 7.2).
//!
//! A Basis texture (ETC1S or UASTC) transcodes to ASTC 4x4 on the phone. Any other format is
//! uploaded as stored, so its cost is the levels' uncompressed lengths.

use std::ops::{Range, RangeInclusive};

// `::ktx2` is the crate. This module has the same name.
use ::ktx2::{ColorModel, Format, Header, ParseError, Reader, SupercompressionScheme};

use super::mips::{MipSource, astc_4x4_bytes, level_side};
use super::{
    BASIS_FORMAT, BC_ON_IPHONE, DAMAGED_HEADER, DecodedTexture, Encoding, GpuFormat, NotCovered,
    Note, TRUNCATED, TextureFormat,
};

/// The file identifier every KTX2 file starts with.
pub const MAGIC: &[u8] = b"\xABKTX 20\xBB\r\n\x1A\n";

/// `VkFormat` BC1 to BC7: block formats an iPhone GPU cannot sample.
const BC_FORMATS: RangeInclusive<u32> = 131..=146;

// The data format descriptor (DFD): its place in the header, then the parts of each block.
const DFD_OFFSET_AT: usize = 48;
const DFD_LENGTH_AT: usize = 52;
const DFD_TOTAL_SIZE_FIELD_BYTES: usize = 4;
const DFD_BLOCK_HEADER_BYTES: usize = 8;
/// A Basic block's header starts with vendor 0 (Khronos) and descriptor type 0 in one u32, then
/// version 2 in a u16. The crate reads texel block dimensions from Basic blocks only.
const DFD_BASIC_BLOCK_START: [u8; 6] = [0, 0, 0, 0, 2, 0];
const DFD_BLOCK_SIZE_AT: usize = 6;
const DFD_TEXEL_BLOCK_DIMENSIONS: Range<usize> = 12..16;

/// Reads the header, the level index and the data format descriptor. `Reader::new` checks every
/// section and level against the file's length. The caller has checked `MAGIC`.
pub fn read(bytes: &[u8]) -> Result<DecodedTexture, NotCovered> {
    if has_a_texel_block_dimension_of_256(bytes) {
        return Err(NotCovered::Invalid(DAMAGED_HEADER));
    }
    let reader = Reader::new(bytes).map_err(not_covered_from_parse_error)?;
    let header = reader.header();
    let width = header.pixel_width;
    // A 1D texture has height 0, and a 2D texture has depth 0.
    let height = header.pixel_height.max(1);
    let depth = header.pixel_depth.max(1);
    // Level count 0 asks the runtime to make the mips, and the file holds level 0 only.
    let levels = header.level_count.max(1);
    // A texture that is not an array has layer count 0.
    let surfaces_per_level = u64::from(header.layer_count.max(1)) * u64::from(header.face_count);
    let astc_bytes = (0..levels)
        .map(|level| {
            let level_width = level_side(width, level);
            let level_height = level_side(height, level);
            let level_depth = u64::from(level_side(depth, level));
            astc_4x4_bytes(level_width, level_height).saturating_mul(level_depth)
        })
        .fold(0, u64::saturating_add)
        .saturating_mul(surfaces_per_level);
    let (encoding, gpu_format, gpu_bytes) = match header.format {
        // The crate gives None for VkFormat 0, which a Basis texture uses.
        None => (
            basis_encoding(&reader, &header)?,
            GpuFormat::Astc4x4,
            astc_bytes,
        ),
        Some(format) if BC_FORMATS.contains(&format.value()) => {
            return Err(NotCovered::Unsupported(BC_ON_IPHONE));
        }
        Some(format) => {
            let gpu_bytes = stored_bytes(&reader, &header)?;
            (Encoding::Raw, gpu_format_of(format), gpu_bytes)
        }
    };
    Ok(DecodedTexture {
        format: TextureFormat::Ktx2,
        encoding: Some(encoding),
        width,
        height,
        levels,
        mips: MipSource::FileLevels,
        gpu_format,
        gpu_bytes,
        astc_bytes,
        note: (header.level_count == 0).then_some(Note::RuntimeMips),
    })
}

/// A file the `ktx2` crate cannot parse. One that ends early is truncated, and anything else is
/// a damaged header.
fn not_covered_from_parse_error(error: ParseError) -> NotCovered {
    match error {
        ParseError::UnexpectedEnd => NotCovered::Invalid(TRUNCATED),
        _ => NotCovered::Invalid(DAMAGED_HEADER),
    }
}

/// The spec stores each texel block dimension minus 1, and `ktx2` 0.5 panics when it adds the 1
/// back to 255 in a Basic block. This walks the DFD blocks the way the crate does, so such a file
/// is rejected first. A block too small for its own header, or too long for the DFD, stops the
/// walk, and the crate rejects the file itself.
fn has_a_texel_block_dimension_of_256(bytes: &[u8]) -> bool {
    let le_u32_at = |at: usize| {
        let field = bytes.get(at..)?.first_chunk::<4>()?;
        Some(u32::from_le_bytes(*field) as usize)
    };
    let (Some(dfd_at), Some(dfd_length)) = (le_u32_at(DFD_OFFSET_AT), le_u32_at(DFD_LENGTH_AT))
    else {
        return false;
    };
    let blocks_at = dfd_at.saturating_add(DFD_TOTAL_SIZE_FIELD_BYTES);
    let mut rest = bytes
        .get(blocks_at..dfd_at.saturating_add(dfd_length))
        .unwrap_or_default();
    while let Some(block_header) = rest.first_chunk::<DFD_BLOCK_HEADER_BYTES>() {
        let size_field = [
            block_header[DFD_BLOCK_SIZE_AT],
            block_header[DFD_BLOCK_SIZE_AT + 1],
        ];
        let block_size = usize::from(u16::from_le_bytes(size_field));
        // A block smaller than its own header would never move the walk on.
        if block_size < DFD_BLOCK_HEADER_BYTES {
            return false;
        }
        let Some(block) = rest.get(..block_size) else {
            return false;
        };
        let dimensions = block.get(DFD_TEXEL_BLOCK_DIMENSIONS).unwrap_or_default();
        if block.starts_with(&DFD_BASIC_BLOCK_START) && dimensions.contains(&u8::MAX) {
            return true;
        }
        rest = &rest[block_size..];
    }
    false
}

/// `VkFormat` 0 with a Basis colour model: ETC1S needs BasisLZ, and UASTC is stored plain or
/// wrapped in Zstandard or ZLIB. Anything else, such as UASTC HDR, is not modelled.
fn basis_encoding(reader: &Reader<&[u8]>, header: &Header) -> Result<Encoding, NotCovered> {
    let scheme = header.supercompression_scheme;
    let uastc_scheme = matches!(
        scheme,
        None | Some(SupercompressionScheme::Zstandard | SupercompressionScheme::ZLIB)
    );
    match reader.color_model() {
        Some(ColorModel::ETC1S) if scheme == Some(SupercompressionScheme::BasisLZ) => {
            Ok(Encoding::Etc1s)
        }
        Some(ColorModel::UASTC) if uastc_scheme => Ok(Encoding::Uastc),
        _ => Err(NotCovered::Unsupported(BASIS_FORMAT)),
    }
}

/// The sum of the levels' uncompressed lengths, which already count every layer and face. A raw
/// format with BasisLZ, or a plain level whose two lengths differ, is a damaged header.
fn stored_bytes(reader: &Reader<&[u8]>, header: &Header) -> Result<u64, NotCovered> {
    let damaged = Err(NotCovered::Invalid(DAMAGED_HEADER));
    let scheme = header.supercompression_scheme;
    // BasisLZ is only for VkFormat 0, and its levels give no uncompressed length.
    if scheme == Some(SupercompressionScheme::BasisLZ) {
        return damaged;
    }
    // Without supercompression, the stored bytes are the uncompressed bytes.
    let plain_lengths_differ = scheme.is_none()
        && reader
            .levels()
            .any(|level| level.uncompressed_byte_length != level.data.len() as u64);
    if plain_lengths_differ {
        return damaged;
    }
    Ok(reader
        .levels()
        .map(|level| level.uncompressed_byte_length)
        .fold(0, u64::saturating_add))
}

fn gpu_format_of(format: Format) -> GpuFormat {
    match format {
        Format::R8G8B8A8_UNORM | Format::R8G8B8A8_SRGB => GpuFormat::Rgba8,
        Format::ASTC_4x4_UNORM_BLOCK | Format::ASTC_4x4_SRGB_BLOCK => GpuFormat::Astc4x4,
        other => GpuFormat::OtherVkFormat(other.value()),
    }
}
