//! A file's kind: its magic bytes first, then its extension (design section 3).
//!
//! A damaged file keeps the kind its magic or extension gives. The family that reads it marks it
//! invalid.

use std::path::Path;

use dyno_schema::manifest::FileKind;
use dyno_schema::path::RelPath;

/// One ISO-BMFF box: its four-character type and its body.
struct Mp4Box<'a> {
    kind: [u8; 4],
    body: &'a [u8],
}

/// One EBML element: its ID with the length marker kept, and its body.
struct EbmlElement<'a> {
    id: u32,
    body: &'a [u8],
}

/// Magic at the start of an image file: PNG, JPEG, GIF87a and GIF89a.
/// The extension of a source map. A browser fetches one only for its developer tools.
const SOURCE_MAP_EXTENSION: &str = "map";

const IMAGE_MAGIC: &[&[u8]] = &[b"\x89PNG", b"\xFF\xD8\xFF", b"GIF87a", b"GIF89a"];

/// Magic at the start of an audio file: an ID3 tag (MP3), FLAC and Core Audio.
const AUDIO_MAGIC: &[&[u8]] = &[b"ID3", b"fLaC", b"caff"];

/// Magic at the start of a GPU texture: KTX2, KTX1, ASTC, DDS and PVR3. Basis has its own check.
const TEXTURE_MAGIC: &[&[u8]] = &[
    b"\xABKTX 20\xBB",
    b"\xABKTX 11\xBB",
    b"\x13\xAB\xA1\x5C",
    b"DDS ",
    b"PVR\x03",
];

/// Magic at the start of a font: WOFF, WOFF2 and OpenType with CFF outlines.
const FONT_MAGIC: &[&[u8]] = &[b"wOFF", b"wOF2", b"OTTO"];

/// Magic of a TrueType font. A text file can start with `true` too, such as a JSON file that holds
/// `true`, so it counts only with a TrueType extension.
const TRUETYPE_MAGIC: &[&[u8]] = &[b"true", b"\x00\x01\x00\x00"];

/// The extensions that let `TRUETYPE_MAGIC` count.
const TRUETYPE_EXTENSIONS: &[&str] = &["ttf", "otf"];

/// An MPEG audio frame sync starts a raw MP3 or ADTS file, but also any file that starts with
/// 0xFFE0 or above, so it counts only with an audio extension.
const MPEG_AUDIO_EXTENSIONS: &[&str] = &["mp3", "aac", "mp2", "mpga"];

/// Container formats the magic cannot place without reading tracks. When the tracks are
/// unreadable, these extensions mean audio, and any other means video.
const MP4_AUDIO_EXTENSIONS: &[&str] = &["m4a", "aac"];
const MATROSKA_AUDIO_EXTENSIONS: &[&str] = &["weba", "mka"];

/// ISO-BMFF brands of still images and image sequences (AVIF and HEIF).
const IMAGE_BRANDS: &[&[u8; 4]] = &[b"avif", b"avis", b"heic", b"heix", b"mif1", b"msf1"];

/// The `.basis` magic and the header size that confirms it.
const BASIS_MAGIC: &[u8] = b"sB";
const BASIS_HEADER_SIZE: u16 = 77;

/// The first packet of an Ogg Theora stream, which makes an Ogg file a video.
const THEORA_PACKET: &[u8] = b"\x80theora";

/// Bytes in an Ogg page header before its segment table.
const OGG_PAGE_HEADER_BYTES: usize = 27;

const EBML_MAGIC: &[u8] = b"\x1A\x45\xDF\xA3";
const EBML_SEGMENT: u32 = 0x1853_8067;
const EBML_TRACKS: u32 = 0x1654_AE6B;
const EBML_TRACK_ENTRY: u32 = 0xAE;
const EBML_TRACK_TYPE: u32 = 0x83;
const MATROSKA_VIDEO_TRACK: u64 = 1;
const MATROSKA_AUDIO_TRACK: u64 = 2;

/// Bytes in an ISO-BMFF box header with a 32-bit size, and with a 64-bit size.
const MP4_HEADER_BYTES: usize = 8;
const MP4_LARGE_HEADER_BYTES: usize = 16;
/// Where the handler type sits in an `hdlr` body: after the version, flags and `pre_defined`.
const HDLR_HANDLER_TYPE: std::ops::Range<usize> = 8..12;

/// Extensions by kind, for files whose magic says nothing (design section 3).
const EXTENSIONS: &[(FileKind, &[&str])] = &[
    (FileKind::Js, &["js", "mjs", "cjs"]),
    (FileKind::Css, &["css"]),
    (FileKind::Html, &["html", "htm"]),
    (
        FileKind::Data,
        &[
            "json", "map", "txt", "xml", "csv", "atlas", "fnt", "skel", "plist", "yaml", "yml",
        ],
    ),
    (
        FileKind::Image,
        &[
            "png", "jpg", "jpeg", "webp", "avif", "gif", "svg", "bmp", "ico",
        ],
    ),
    (
        FileKind::Texture,
        &["ktx2", "ktx", "basis", "astc", "dds", "pvr"],
    ),
    (
        FileKind::Audio,
        &[
            "wav", "ogg", "oga", "opus", "mp3", "aac", "m4a", "weba", "flac", "caf", "aif", "aiff",
        ],
    ),
    (
        FileKind::Video,
        &["mp4", "m4v", "mov", "webm", "ogv", "mkv"],
    ),
    (FileKind::Font, &["woff", "woff2", "ttf", "otf", "eot"]),
];

/// The kind of one file, from its contents and then its path's extension.
/// Whether the file is a source map, which no player downloads.
pub fn is_source_map(path: &RelPath) -> bool {
    Path::new(path.as_str())
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(SOURCE_MAP_EXTENSION))
}

pub fn file_kind(path: &RelPath, contents: &[u8]) -> FileKind {
    let extension = lowercase_extension(path);
    kind_from_magic(contents, &extension).unwrap_or_else(|| kind_from_extension(&extension))
}

fn lowercase_extension(path: &RelPath) -> String {
    Path::new(path.as_str())
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// The kind the first bytes give, or None when they match no magic in the table.
fn kind_from_magic(contents: &[u8], extension: &str) -> Option<FileKind> {
    let starts_with_any = |magics: &[&[u8]]| magics.iter().any(|magic| contents.starts_with(magic));
    if starts_with_any(IMAGE_MAGIC) || is_riff(contents, b"WEBP") {
        return Some(FileKind::Image);
    }
    if starts_with_any(AUDIO_MAGIC) || is_riff(contents, b"WAVE") || is_aiff(contents) {
        return Some(FileKind::Audio);
    }
    if contents.starts_with(b"OggS") {
        return Some(ogg_kind(contents));
    }
    if is_mpeg_audio_sync(contents) && MPEG_AUDIO_EXTENSIONS.contains(&extension) {
        return Some(FileKind::Audio);
    }
    if contents.get(4..8) == Some(b"ftyp") {
        return Some(iso_bmff_kind(contents, extension));
    }
    if contents.starts_with(EBML_MAGIC) {
        return Some(matroska_kind(contents, extension));
    }
    if starts_with_any(TEXTURE_MAGIC) || is_basis(contents) {
        return Some(FileKind::Texture);
    }
    let truetype = starts_with_any(TRUETYPE_MAGIC) && TRUETYPE_EXTENSIONS.contains(&extension);
    if starts_with_any(FONT_MAGIC) || truetype {
        return Some(FileKind::Font);
    }
    if contents.starts_with(b"\0asm") {
        return Some(FileKind::Other);
    }
    None
}

fn kind_from_extension(extension: &str) -> FileKind {
    EXTENSIONS
        .iter()
        .find(|(_, extensions)| extensions.contains(&extension))
        .map_or(FileKind::Other, |&(kind, _)| kind)
}

/// A RIFF file whose form type, at offset 8, is `form`.
fn is_riff(contents: &[u8], form: &[u8; 4]) -> bool {
    contents.starts_with(b"RIFF") && contents.get(8..12) == Some(form)
}

fn is_aiff(contents: &[u8]) -> bool {
    let form = contents.get(8..12);
    contents.starts_with(b"FORM") && (form == Some(b"AIFF") || form == Some(b"AIFC"))
}

/// An 11-bit MPEG audio frame sync: 0xFF, then a byte with its top three bits set.
fn is_mpeg_audio_sync(contents: &[u8]) -> bool {
    matches!(contents, [0xFF, second, ..] if second & 0xE0 == 0xE0)
}

/// A `.basis` file: the magic, and the header size at offset 4.
fn is_basis(contents: &[u8]) -> bool {
    let header_size = contents
        .get(4..6)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]));
    contents.starts_with(BASIS_MAGIC) && header_size == Some(BASIS_HEADER_SIZE)
}

/// Video when the first packet is a Theora header, else audio (Vorbis, Opus or FLAC).
fn ogg_kind(contents: &[u8]) -> FileKind {
    let segment_count = contents.get(OGG_PAGE_HEADER_BYTES - 1).copied();
    let first_packet = segment_count
        .map(|count| OGG_PAGE_HEADER_BYTES + usize::from(count))
        .and_then(|start| contents.get(start..));
    match first_packet {
        Some(packet) if packet.starts_with(THEORA_PACKET) => FileKind::Video,
        _ => FileKind::Audio,
    }
}

/// An MP4-family file: an image by brand, else video or audio by its tracks' handlers, else by
/// extension when `moov` is missing or unreadable.
fn iso_bmff_kind(contents: &[u8], extension: &str) -> FileKind {
    if has_image_brand(contents) {
        return FileKind::Image;
    }
    let handlers = mp4_handler_types(contents).unwrap_or_default();
    if handlers.contains(b"vide") {
        FileKind::Video
    } else if handlers.contains(b"soun") || MP4_AUDIO_EXTENSIONS.contains(&extension) {
        FileKind::Audio
    } else {
        FileKind::Video
    }
}

/// Whether the `ftyp` box's major or a compatible brand is an image brand. A damaged `ftyp`
/// still gives the brands that are in the file.
fn has_image_brand(contents: &[u8]) -> bool {
    let declared_size = contents
        .get(0..4)
        .and_then(|size| size.try_into().ok())
        .map_or(0, u32::from_be_bytes);
    let end =
        usize::try_from(declared_size).map_or(contents.len(), |size| size.min(contents.len()));
    let ftyp_body = contents.get(MP4_HEADER_BYTES..end).unwrap_or_default();
    let (words, _) = ftyp_body.as_chunks::<4>();
    // The second word is the minor version, between the major brand and the compatible brands.
    let mut brands = words.iter().take(1).chain(words.iter().skip(2));
    brands.any(|brand| IMAGE_BRANDS.contains(&brand))
}

/// The handler type of every track in `moov`, or None when `moov` is missing or a box before it
/// is damaged.
fn mp4_handler_types(contents: &[u8]) -> Option<Vec<[u8; 4]>> {
    let moov = mp4_child(contents, b"moov")?;
    let handlers = mp4_boxes(moov)
        .filter(|mp4_box| &mp4_box.kind == b"trak")
        .filter_map(|trak| mp4_child(trak.body, b"mdia"))
        .filter_map(|mdia| mp4_child(mdia, b"hdlr"))
        .filter_map(|hdlr| hdlr.get(HDLR_HANDLER_TYPE)?.try_into().ok())
        .collect();
    Some(handlers)
}

/// The body of the first box of this type in `data`.
fn mp4_child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    mp4_boxes(data)
        .find(|mp4_box| &mp4_box.kind == kind)
        .map(|mp4_box| mp4_box.body)
}

/// The boxes in `data`, in order. The walk stops at the first box whose header is damaged or
/// that runs past the end of `data`.
fn mp4_boxes(data: &[u8]) -> impl Iterator<Item = Mp4Box<'_>> {
    let mut offset = 0;
    std::iter::from_fn(move || {
        let (mp4_box, end) = mp4_box_at(data, offset)?;
        offset = end;
        Some(mp4_box)
    })
}

/// The box at `offset`, and the offset after it. A size of 0 runs to the end of `data`, and a size
/// of 1 means a 64-bit size follows the type.
fn mp4_box_at(data: &[u8], offset: usize) -> Option<(Mp4Box<'_>, usize)> {
    let header = data.get(offset..offset.checked_add(MP4_HEADER_BYTES)?)?;
    let size = u32::from_be_bytes(header[0..4].try_into().ok()?);
    let kind: [u8; 4] = header[4..8].try_into().ok()?;
    let (header_bytes, total_bytes) = match size {
        0 => (MP4_HEADER_BYTES, data.len() - offset),
        1 => {
            let large = data.get(offset + MP4_HEADER_BYTES..offset + MP4_LARGE_HEADER_BYTES)?;
            let large = u64::from_be_bytes(large.try_into().ok()?);
            (MP4_LARGE_HEADER_BYTES, usize::try_from(large).ok()?)
        }
        size => (MP4_HEADER_BYTES, usize::try_from(size).ok()?),
    };
    if total_bytes < header_bytes {
        return None;
    }
    let end = offset.checked_add(total_bytes)?;
    let body = data.get(offset + header_bytes..end)?;
    Some((Mp4Box { kind, body }, end))
}

/// A Matroska or WebM file: video when any track is video, audio when any is audio, else by
/// extension when the tracks are unreadable.
fn matroska_kind(contents: &[u8], extension: &str) -> FileKind {
    let track_types = matroska_track_types(contents).unwrap_or_default();
    if track_types.contains(&MATROSKA_VIDEO_TRACK) {
        FileKind::Video
    } else if track_types.contains(&MATROSKA_AUDIO_TRACK)
        || MATROSKA_AUDIO_EXTENSIONS.contains(&extension)
    {
        FileKind::Audio
    } else {
        FileKind::Video
    }
}

/// Every `TrackType` in the first `Tracks` element. The walk reads only up to `Tracks`, so a file
/// cut short after it still gives its tracks.
fn matroska_track_types(contents: &[u8]) -> Option<Vec<u64>> {
    let segment = ebml_child(contents, EBML_SEGMENT)?;
    let tracks = ebml_child(segment, EBML_TRACKS)?;
    let track_types = ebml_elements(tracks)
        .filter(|entry| entry.id == EBML_TRACK_ENTRY)
        .filter_map(|entry| ebml_child(entry.body, EBML_TRACK_TYPE))
        .filter_map(ebml_unsigned)
        .collect();
    Some(track_types)
}

/// The body of the first element with this ID.
fn ebml_child(data: &[u8], id: u32) -> Option<&[u8]> {
    ebml_elements(data)
        .find(|element| element.id == id)
        .map(|element| element.body)
}

/// The elements in `data`, in order. An element of unknown size, or one that runs past the end,
/// is cut at the end of `data` and is the last one.
fn ebml_elements(data: &[u8]) -> impl Iterator<Item = EbmlElement<'_>> {
    let mut rest = data;
    std::iter::from_fn(move || {
        let (id, id_bytes) = ebml_id(rest)?;
        let (size, size_bytes) = ebml_size(rest.get(id_bytes..)?)?;
        let body_start = id_bytes + size_bytes;
        let available = rest.len() - body_start;
        let body_bytes = size
            .and_then(|size| usize::try_from(size).ok())
            .filter(|&size| size <= available);
        let element = match body_bytes {
            Some(size) => {
                let body = &rest[body_start..body_start + size];
                rest = &rest[body_start + size..];
                EbmlElement { id, body }
            }
            None => {
                let body = &rest[body_start..];
                rest = &[];
                EbmlElement { id, body }
            }
        };
        Some(element)
    })
}

/// An element ID and its length in bytes. The ID keeps its length marker, as the spec writes IDs.
fn ebml_id(data: &[u8]) -> Option<(u32, usize)> {
    let length = ebml_vint_length(*data.first()?).filter(|&length| length <= 4)?;
    let bytes = data.get(..length)?;
    let id = bytes
        .iter()
        .fold(0_u32, |id, &byte| (id << 8) | u32::from(byte));
    Some((id, length))
}

/// An element size and its length in bytes. None for the size means unknown: every value bit is
/// set.
fn ebml_size(data: &[u8]) -> Option<(Option<u64>, usize)> {
    let first = *data.first()?;
    let length = ebml_vint_length(first)?;
    let bytes = data.get(..length)?;
    let first_value_bits = 0x7F_u8 >> (length - 1);
    let value = bytes[1..]
        .iter()
        .fold(u64::from(first & first_value_bits), |value, &byte| {
            (value << 8) | u64::from(byte)
        });
    let value_bits = 7 * u32::try_from(length).ok()?;
    let unknown = value == (1_u64 << value_bits) - 1;
    Some((if unknown { None } else { Some(value) }, length))
}

/// The length of a variable-length integer from its first byte: one more than its leading zeros.
fn ebml_vint_length(first: u8) -> Option<usize> {
    let leading_zeros = usize::try_from(first.leading_zeros()).ok()?;
    (leading_zeros < 8).then_some(leading_zeros + 1)
}

/// A big-endian unsigned integer of up to 8 bytes.
fn ebml_unsigned(body: &[u8]) -> Option<u64> {
    (body.len() <= 8).then(|| {
        body.iter()
            .fold(0_u64, |value, &byte| (value << 8) | u64::from(byte))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEGMENT_ID: &[u8] = &[0x18, 0x53, 0x80, 0x67];

    fn kind(path: &str, contents: &[u8]) -> FileKind {
        file_kind(&RelPath::new(path).unwrap(), contents)
    }

    /// An ISO-BMFF box with this type and body.
    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let size = u32::try_from(MP4_HEADER_BYTES + body.len()).unwrap();
        [&size.to_be_bytes()[..], kind, body].concat()
    }

    /// A `moov` with one track whose handler type is `handler`.
    fn moov_with_handler(handler: &[u8; 4]) -> Vec<u8> {
        let hdlr_body = [&[0_u8; 8][..], handler, &[0_u8; 12]].concat();
        let mdia = mp4_box(b"mdia", &mp4_box(b"hdlr", &hdlr_body));
        mp4_box(b"moov", &mp4_box(b"trak", &mdia))
    }

    fn ftyp(major_brand: &[u8; 4]) -> Vec<u8> {
        mp4_box(b"ftyp", &[major_brand, &[0_u8; 4][..], b"isom"].concat())
    }

    /// An EBML element with a one-byte size.
    fn ebml(id: &[u8], body: &[u8]) -> Vec<u8> {
        let size = 0x80 | u8::try_from(body.len()).unwrap();
        [id, &[size], body].concat()
    }

    /// The `Tracks` element of one track of this type.
    fn matroska_tracks(track_type: u64) -> Vec<u8> {
        let track_type = u8::try_from(track_type).unwrap();
        let entry = ebml(&[0xAE], &ebml(&[0x83], &[track_type]));
        ebml(&[0x16, 0x54, 0xAE, 0x6B], &entry)
    }

    /// A Matroska file with one track of this type.
    fn matroska(track_type: u64) -> Vec<u8> {
        let segment = ebml(SEGMENT_ID, &matroska_tracks(track_type));
        [ebml(EBML_MAGIC, b""), segment].concat()
    }

    #[test]
    fn image_magic_wins_over_the_extension() {
        assert_eq!(kind("a.bin", b"\x89PNG\r\n\x1a\n"), FileKind::Image);
        assert_eq!(kind("a.bin", b"\xFF\xD8\xFF\xE0"), FileKind::Image);
        assert_eq!(kind("a.bin", b"GIF89a"), FileKind::Image);
        assert_eq!(kind("a.bin", b"RIFF\0\0\0\0WEBPVP8 "), FileKind::Image);
    }

    #[test]
    fn audio_magic_wins_over_the_extension() {
        assert_eq!(kind("a.bin", b"RIFF\0\0\0\0WAVEfmt "), FileKind::Audio);
        assert_eq!(kind("a.bin", b"ID3\x04"), FileKind::Audio);
        assert_eq!(kind("a.bin", b"fLaC"), FileKind::Audio);
        assert_eq!(kind("a.bin", b"caff"), FileKind::Audio);
        assert_eq!(kind("a.bin", b"FORM\0\0\0\0AIFC"), FileKind::Audio);
    }

    #[test]
    fn an_ogg_file_is_audio_unless_its_first_packet_is_theora() {
        let page = |packet: &[u8]| [&b"OggS"[..], &[0; 22], &[1, 30], packet].concat();
        assert_eq!(kind("a.ogg", &page(b"\x01vorbis")), FileKind::Audio);
        assert_eq!(kind("a.ogg", &page(b"\x80theora")), FileKind::Video);
    }

    #[test]
    fn a_frame_sync_is_audio_only_with_an_mpeg_audio_extension() {
        // mp2 and mpga have no kind of their own, so only the frame sync makes them audio.
        assert_eq!(kind("a.mp2", b"\xFF\xFB\x90\x00"), FileKind::Audio);
        assert_eq!(kind("a.mpga", b"\xFF\xF3\x90\x00"), FileKind::Audio);
        assert_eq!(kind("a.dat", b"\xFF\xFB\x90\x00"), FileKind::Other);
    }

    #[test]
    fn an_mp4_with_an_image_brand_is_an_image() {
        assert_eq!(kind("a.mp4", &ftyp(b"avif")), FileKind::Image);
        let compatible_mif1 = mp4_box(b"ftyp", b"isom\0\0\0\0mif1");
        assert_eq!(kind("a.mp4", &compatible_mif1), FileKind::Image);
    }

    #[test]
    fn an_ftyp_cut_short_still_gives_its_brands() {
        let mut ftyp_only = ftyp(b"avif");
        ftyp_only[3] = 0xFF;
        assert_eq!(kind("a.avif", &ftyp_only), FileKind::Image);
    }

    #[test]
    fn an_mp4_is_video_or_audio_by_its_track_handlers() {
        let video = [ftyp(b"isom"), moov_with_handler(b"vide")].concat();
        let audio = [ftyp(b"M4A "), moov_with_handler(b"soun")].concat();
        assert_eq!(kind("a.m4a", &video), FileKind::Video);
        assert_eq!(kind("a.mp4", &audio), FileKind::Audio);
    }

    #[test]
    fn a_moov_after_mdat_is_still_read() {
        let file = [
            ftyp(b"M4A "),
            mp4_box(b"mdat", &[0; 16]),
            moov_with_handler(b"soun"),
        ]
        .concat();
        assert_eq!(kind("a.mp4", &file), FileKind::Audio);
    }

    #[test]
    fn an_mp4_without_a_readable_moov_goes_by_extension() {
        let truncated_mdat = [&ftyp(b"M4A ")[..], &[0, 0, 0x10, 0], b"mdat"].concat();
        assert_eq!(kind("a.m4a", &truncated_mdat), FileKind::Audio);
        assert_eq!(kind("a.aac", &truncated_mdat), FileKind::Audio);
        assert_eq!(kind("a.mp4", &truncated_mdat), FileKind::Video);
    }

    #[test]
    fn a_matroska_file_is_video_or_audio_by_its_track_types() {
        assert_eq!(
            kind("a.weba", &matroska(MATROSKA_VIDEO_TRACK)),
            FileKind::Video
        );
        assert_eq!(
            kind("a.webm", &matroska(MATROSKA_AUDIO_TRACK)),
            FileKind::Audio
        );
    }

    #[test]
    fn a_matroska_file_cut_short_after_its_tracks_keeps_its_tracks() {
        let tracks = matroska_tracks(MATROSKA_AUDIO_TRACK);
        let missing_bytes = 16;
        let claimed_size = 0x80 | u8::try_from(tracks.len() + missing_bytes).unwrap();
        let file = [
            ebml(EBML_MAGIC, b""),
            SEGMENT_ID.to_vec(),
            vec![claimed_size],
            tracks,
        ]
        .concat();
        assert_eq!(kind("a.webm", &file), FileKind::Audio);
    }

    #[test]
    fn a_matroska_file_without_readable_tracks_goes_by_extension() {
        let header_only = ebml(EBML_MAGIC, b"");
        assert_eq!(kind("a.weba", &header_only), FileKind::Audio);
        assert_eq!(kind("a.mka", &header_only), FileKind::Audio);
        assert_eq!(kind("a.webm", &header_only), FileKind::Video);
    }

    #[test]
    fn a_segment_of_unknown_size_runs_to_the_end() {
        // A one-byte size with every value bit set is unknown. Read as 127 bytes, the segment
        // would end inside the Void elements, before its tracks.
        let unknown_size = 0xFF;
        let void = ebml(&[0xEC], &[0; 100]);
        let file = [
            ebml(EBML_MAGIC, b""),
            SEGMENT_ID.to_vec(),
            vec![unknown_size],
            void.clone(),
            void,
            matroska_tracks(MATROSKA_AUDIO_TRACK),
        ]
        .concat();
        assert_eq!(kind("a.webm", &file), FileKind::Audio);
    }

    #[test]
    fn an_eight_byte_size_reads_without_overflow() {
        let size = [0x01, 0, 0, 0, 0, 0, 0, 0x05];
        assert_eq!(ebml_size(&size), Some((Some(5), 8)));
    }

    #[test]
    fn texture_magic_and_the_basis_header_size_give_a_texture() {
        assert_eq!(kind("a.bin", b"\xABKTX 20\xBB\r\n"), FileKind::Texture);
        assert_eq!(kind("a.bin", b"\xABKTX 11\xBB\r\n"), FileKind::Texture);
        assert_eq!(
            kind("a.bin", b"\x13\xAB\xA1\x5C\x04\x04"),
            FileKind::Texture
        );
        assert_eq!(kind("a.bin", b"DDS \x7C"), FileKind::Texture);
        assert_eq!(kind("a.bin", b"PVR\x03\x00"), FileKind::Texture);
        assert_eq!(kind("a.bin", b"sB\x13\x00\x4D\x00"), FileKind::Texture);
        assert_eq!(kind("a.txt", b"sB\x13\x00\x4E\x00"), FileKind::Data);
    }

    #[test]
    fn font_magic_gives_a_font_with_any_extension() {
        assert_eq!(kind("a.bin", b"wOFF\0\0"), FileKind::Font);
        assert_eq!(kind("a.bin", b"wOF2\0\0"), FileKind::Font);
        assert_eq!(kind("a.bin", b"OTTO\0\0"), FileKind::Font);
    }

    #[test]
    fn truetype_magic_is_a_font_only_with_a_truetype_extension() {
        assert_eq!(kind("a.ttf", b"\x00\x01\x00\x00"), FileKind::Font);
        assert_eq!(kind("a.otf", b"true"), FileKind::Font);
        assert_eq!(kind("flag.json", b"true"), FileKind::Data);
        assert_eq!(kind("a.bin", b"\x00\x01\x00\x00"), FileKind::Other);
    }

    #[test]
    fn webassembly_magic_is_other_with_any_extension() {
        assert_eq!(kind("a.json", b"\0asm\x01\0\0\0"), FileKind::Other);
    }

    #[test]
    fn without_magic_the_extension_decides_ignoring_case() {
        assert_eq!(kind("app.MJS", b"export {}"), FileKind::Js);
        assert_eq!(kind("a/b.css", b"body{}"), FileKind::Css);
        assert_eq!(kind("index.htm", b"<!doctype html>"), FileKind::Html);
        assert_eq!(kind("hero.atlas", b"hero.png"), FileKind::Data);
        assert_eq!(kind("icon.svg", b"<svg/>"), FileKind::Image);
        assert_eq!(kind("a.ktx2", b""), FileKind::Texture);
        assert_eq!(kind("a.flac", b""), FileKind::Audio);
        assert_eq!(kind("a.mov", b""), FileKind::Video);
        assert_eq!(kind("a.eot", b""), FileKind::Font);
    }

    #[test]
    fn no_magic_and_no_known_extension_is_other() {
        assert_eq!(kind("LICENSE", b"MIT"), FileKind::Other);
        assert_eq!(kind(".htaccess", b"Deny"), FileKind::Other);
        assert_eq!(kind("a.wasm", b""), FileKind::Other);
    }
}
