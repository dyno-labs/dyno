//! Raster images: PNG, JPEG, WebP, AVIF and GIF. The size comes from the header, through
//! `imagesize` (design section 7.2). Pixels are never decoded.

use imagesize::{Compression, ImageType};

use super::{
    ASTC, BMP, DAMAGED_HEADER, DDS, HEIC, HEIF, ICO, NotCovered, Note, OTHER_IMAGE_FORMAT, PVR,
    TextureFormat,
};

/// What an image's header gives: its format and size, and what the count leaves out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RasterHeader {
    pub format: TextureFormat,
    pub width: u32,
    pub height: u32,
    /// What the count leaves out, such as the other frames of a GIF.
    pub note: Option<Note>,
}

// A PNG starts with an 8-byte signature. IHDR must be the first chunk: a 4-byte length (13), the
// 4-byte type, 13 bytes of data and a 4-byte CRC.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const PNG_IHDR_START: &[u8] = b"\0\0\0\x0dIHDR";
const PNG_IHDR_END: usize = 8 + 4 + 4 + 13 + 4;

// A lossy WebP (`VP8 ` chunk) stores each side in 14 bits, with 2 bits of upscaling above them.
const WEBP_CHUNK_TYPE_AT: usize = 12;
const WEBP_LOSSY_CHUNK: &[u8] = b"VP8 ";
const WEBP_LOSSY_SIDE_MASK: usize = 0x3FFF;

// An AVIF may list its brand anywhere in `ftyp`'s compatible brands, which start after the box
// size, the box type, the major brand and the minor version.
const FTYP_COMPATIBLE_BRANDS_AT: usize = 16;
const AVIF_BRANDS: &[&[u8]] = &[b"avif", b"avis"];
const BRAND_BYTES: usize = 4;

/// Whether the file starts like an image that `imagesize` knows.
pub fn has_image_magic(bytes: &[u8]) -> bool {
    imagesize::image_type(bytes).is_ok()
}

/// Reads the width and height. A file whose header does not give a size is invalid, and an image
/// format other than PNG, JPEG, WebP, AVIF and GIF is unsupported.
pub fn read(bytes: &[u8]) -> Result<RasterHeader, NotCovered> {
    let damaged = NotCovered::Invalid(DAMAGED_HEADER);
    let image_type = imagesize::image_type(bytes).map_err(|_| damaged)?;
    let format = match image_type {
        ImageType::Png => TextureFormat::Png,
        ImageType::Jpeg => TextureFormat::Jpeg,
        ImageType::Webp => TextureFormat::Webp,
        ImageType::Gif => TextureFormat::Gif,
        ImageType::Heif(Compression::Av1) => TextureFormat::Avif,
        ImageType::Heif(_) if lists_an_avif_brand(bytes) => TextureFormat::Avif,
        ImageType::Heif(Compression::Hevc) => return Err(NotCovered::Unsupported(HEIC)),
        ImageType::Heif(_) => return Err(NotCovered::Unsupported(HEIF)),
        ImageType::Bmp => return Err(NotCovered::Unsupported(BMP)),
        ImageType::Ico => return Err(NotCovered::Unsupported(ICO)),
        // GPU texture containers that `imagesize` knows and dyno does not model. PVR 2 has no
        // magic: its header starts with its own length.
        ImageType::Pvrtc(_) => return Err(NotCovered::Unsupported(PVR)),
        ImageType::Dds(_) => return Err(NotCovered::Unsupported(DDS)),
        ImageType::Astc => return Err(NotCovered::Unsupported(ASTC)),
        _ => return Err(NotCovered::Unsupported(OTHER_IMAGE_FORMAT)),
    };
    if format == TextureFormat::Png && !png_starts_with_whole_ihdr(bytes) {
        return Err(damaged);
    }
    let mut size = imagesize::blob_size(bytes).map_err(|_| damaged)?;
    if format == TextureFormat::Webp && is_lossy_webp(bytes) {
        // `imagesize` keeps the upscaling bits, which are not part of the size.
        size.width &= WEBP_LOSSY_SIDE_MASK;
        size.height &= WEBP_LOSSY_SIDE_MASK;
    }
    let width = u32::try_from(size.width).map_err(|_| damaged)?;
    let height = u32::try_from(size.height).map_err(|_| damaged)?;
    if width == 0 || height == 0 {
        return Err(damaged);
    }
    Ok(RasterHeader {
        format,
        width,
        height,
        note: (format == TextureFormat::Gif).then_some(Note::FirstFrameOnly),
    })
}

/// `imagesize` checks the first 4 bytes of the signature and reads the size at a fixed offset.
/// This checks the whole signature, and that the first chunk is a whole IHDR.
fn png_starts_with_whole_ihdr(bytes: &[u8]) -> bool {
    bytes.len() >= PNG_IHDR_END
        && bytes.starts_with(PNG_SIGNATURE)
        && bytes[PNG_SIGNATURE.len()..].starts_with(PNG_IHDR_START)
}

fn is_lossy_webp(bytes: &[u8]) -> bool {
    bytes.get(WEBP_CHUNK_TYPE_AT..WEBP_CHUNK_TYPE_AT + WEBP_LOSSY_CHUNK.len())
        == Some(WEBP_LOSSY_CHUNK)
}

/// `imagesize` looks at the major brand and the first two compatible brands only.
fn lists_an_avif_brand(bytes: &[u8]) -> bool {
    let ftyp_bytes = bytes
        .first_chunk::<4>()
        .map_or(0, |box_size| u32::from_be_bytes(*box_size) as usize);
    let brand_bytes = bytes
        .get(FTYP_COMPATIBLE_BRANDS_AT..ftyp_bytes.min(bytes.len()))
        .unwrap_or_default();
    let (brands, _) = brand_bytes.as_chunks::<BRAND_BYTES>();
    brands
        .iter()
        .any(|brand| AVIF_BRANDS.contains(&brand.as_slice()))
}
