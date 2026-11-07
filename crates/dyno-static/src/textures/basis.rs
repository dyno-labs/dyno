//! Basis Universal `.basis` files, with our own header reader (design section 7.4).
//!
//! Every field is little-endian and packed. A Basis texture transcodes to ASTC 4x4 on the phone,
//! so each level costs its ASTC 4x4 size.

use std::collections::BTreeMap;

use super::mips::{MipSource, astc_4x4_bytes};
use super::{
    BASIS_FORMAT, BASIS_VERSION, DAMAGED_HEADER, DecodedTexture, Encoding, GpuFormat, NotCovered,
    TRUNCATED, TextureFormat,
};

/// One slice description: one level of one image, or its alpha in an ETC1S file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slice {
    /// Which image in the file. A 2D texture is image 0. An array or a cube map uses them all.
    image: u32,
    level: u8,
    /// Flag bit 0. In ETC1S it marks a separate alpha slice. In UASTC it only says that the slice
    /// has alpha.
    has_alpha_flag: bool,
    width: u32,
    height: u32,
    /// Where the slice's data ends in the file.
    data_end: u64,
}

/// A level of one image, as (image, level).
type ImageLevel = (u32, u8);

// Sizes.
const HEADER_BYTES: usize = 77;
const SLICE_BYTES: usize = 23;

// Header fields.
const SIGNATURE: &[u8] = b"sB";
const VERSION_AT: usize = 2;
const HEADER_SIZE_AT: usize = 4;
const SLICE_COUNT_AT: usize = 14;
const TEXTURE_FORMAT_AT: usize = 20;
const TEXTURE_TYPE_AT: usize = 23;
const SLICE_TABLE_AT: usize = 65;

// Header values.
/// The only version this layout is known for.
const VERSION: u16 = 0x13;
const HEADER_SIZE_FIELD: [u8; 2] = (HEADER_BYTES as u16).to_le_bytes();
const TEXTURE_FORMAT_ETC1S: u8 = 0;
const TEXTURE_FORMAT_UASTC_LDR_4X4: u8 = 1;
const TEXTURE_TYPE_2D: u8 = 0;
const TEXTURE_TYPE_VIDEO_FRAMES: u8 = 3;

// Slice description fields.
const SLICE_IMAGE_AT: usize = 0;
const SLICE_LEVEL_AT: usize = 3;
const SLICE_FLAGS_AT: usize = 4;
const SLICE_WIDTH_AT: usize = 5;
const SLICE_HEIGHT_AT: usize = 7;
const SLICE_DATA_OFFSET_AT: usize = 13;
const SLICE_DATA_SIZE_AT: usize = 17;
const SLICE_FLAG_ALPHA: u8 = 1;

/// Whether the file starts like a Basis file: the signature, then its own header size. The
/// inventory sniffs the same two fields.
pub fn has_signature(bytes: &[u8]) -> bool {
    let header_size = bytes.get(HEADER_SIZE_AT..HEADER_SIZE_AT + HEADER_SIZE_FIELD.len());
    bytes.starts_with(SIGNATURE) && header_size == Some(HEADER_SIZE_FIELD.as_slice())
}

/// Reads the header and the slice table, then counts every level of every image the GPU holds.
/// The caller has checked `has_signature`.
pub fn read(bytes: &[u8]) -> Result<DecodedTexture, NotCovered> {
    let Some(header) = bytes.first_chunk::<HEADER_BYTES>() else {
        return Err(NotCovered::Invalid(TRUNCATED));
    };
    if le_u16(header, VERSION_AT) != VERSION {
        return Err(NotCovered::Unsupported(BASIS_VERSION));
    }
    let encoding = match header[TEXTURE_FORMAT_AT] {
        TEXTURE_FORMAT_ETC1S => Encoding::Etc1s,
        TEXTURE_FORMAT_UASTC_LDR_4X4 => Encoding::Uastc,
        _ => return Err(NotCovered::Unsupported(BASIS_FORMAT)),
    };
    let slices = read_slices(bytes, header)?;
    let sizes = level_sizes(&slices, encoding, header[TEXTURE_TYPE_AT]);
    // Image 0 must have a level 0 with a real size.
    let level_0 = sizes
        .get(&(0, 0))
        .filter(|&&(width, height)| width > 0 && height > 0);
    let Some(&(width, height)) = level_0 else {
        return Err(NotCovered::Invalid(DAMAGED_HEADER));
    };
    // Level indexes are a u8, so there are at most 256.
    let levels = sizes.keys().filter(|(image, _)| *image == 0).count() as u32;
    let gpu_bytes = sizes
        .values()
        .map(|&(level_width, level_height)| astc_4x4_bytes(level_width, level_height))
        .fold(0, u64::saturating_add);
    Ok(DecodedTexture {
        format: TextureFormat::Basis,
        encoding: Some(encoding),
        width,
        height,
        levels,
        mips: MipSource::FileLevels,
        gpu_format: GpuFormat::Astc4x4,
        gpu_bytes,
        astc_bytes: gpu_bytes,
        note: None,
    })
}

/// Reads every slice description. Truncated when the table, or any slice's data, runs past the
/// end of the file.
fn read_slices(bytes: &[u8], header: &[u8; HEADER_BYTES]) -> Result<Vec<Slice>, NotCovered> {
    let truncated = NotCovered::Invalid(TRUNCATED);
    let slice_count = le_u24(header, SLICE_COUNT_AT) as usize;
    let table_at = le_u32(header, SLICE_TABLE_AT) as usize;
    let table_end = slice_count
        .checked_mul(SLICE_BYTES)
        .and_then(|table_bytes| table_at.checked_add(table_bytes))
        .ok_or(truncated)?;
    let table = bytes.get(table_at..table_end).ok_or(truncated)?;
    // The table is a whole number of entries, so nothing is left over.
    let (entries, _) = table.as_chunks::<SLICE_BYTES>();
    let slices: Vec<Slice> = entries.iter().map(read_slice).collect();
    let file_len = bytes.len() as u64;
    if slices.iter().any(|slice| slice.data_end > file_len) {
        return Err(truncated);
    }
    Ok(slices)
}

fn read_slice(entry: &[u8; SLICE_BYTES]) -> Slice {
    let data_offset = u64::from(le_u32(entry, SLICE_DATA_OFFSET_AT));
    let data_size = u64::from(le_u32(entry, SLICE_DATA_SIZE_AT));
    Slice {
        image: le_u24(entry, SLICE_IMAGE_AT),
        level: entry[SLICE_LEVEL_AT],
        has_alpha_flag: entry[SLICE_FLAGS_AT] & SLICE_FLAG_ALPHA != 0,
        width: u32::from(le_u16(entry, SLICE_WIDTH_AT)),
        height: u32::from(le_u16(entry, SLICE_HEIGHT_AT)),
        data_end: data_offset + data_size,
    }
}

/// The width and height of each level of each image the GPU holds.
///
/// A 2D texture holds image 0, and so does a video, which shows one frame at a time. An array, a
/// cube map or a volume holds every image. ETC1S stores alpha as extra slices of the same levels.
/// They transcode into the same ASTC texture, so their sizes are skipped.
fn level_sizes(
    slices: &[Slice],
    encoding: Encoding,
    texture_type: u8,
) -> BTreeMap<ImageLevel, (u32, u32)> {
    let holds_every_image = !matches!(texture_type, TEXTURE_TYPE_2D | TEXTURE_TYPE_VIDEO_FRAMES);
    let has_alpha_slices = encoding == Encoding::Etc1s;
    slices
        .iter()
        .filter(|slice| holds_every_image || slice.image == 0)
        .filter(|slice| !(has_alpha_slices && slice.has_alpha_flag))
        .map(|slice| ((slice.image, slice.level), (slice.width, slice.height)))
        .collect()
}

// These take a fixed-size array, never the whole file, so every constant offset is in range and
// indexing cannot panic.

fn le_u16<const N: usize>(bytes: &[u8; N], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn le_u24<const N: usize>(bytes: &[u8; N], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], 0])
}

fn le_u32<const N: usize>(bytes: &[u8; N], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}
