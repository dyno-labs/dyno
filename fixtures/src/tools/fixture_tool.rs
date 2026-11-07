// fixture_tool: small helpers for fixtures/generate.sh.
//
// Std only, no crates. generate.sh builds it with plain `rustc`, so it is not
// a Cargo crate and the workspace never sees it.
//
// Commands:
//   ktx2-rgba8 <out> <width> <height> <mips: 0|1>
//       Uncompressed KTX2, VK_FORMAT_R8G8B8A8_UNORM (37), no supercompression.
//   ktx2-astc <out> <width> <height>
//       KTX2, VK_FORMAT_ASTC_4x4_UNORM_BLOCK (157), one level, zeroed blocks.
//   opus-input-rate <file> <rate>
//       Rewrite the input sample rate field in an Ogg Opus OpusHead packet and
//       fix the page CRC. The field is informational; decoders run at 48 kHz.
//   fake-font <out> <magic> <size>
//       A file that starts with a font magic (for example wOF2) and is padded
//       with zeros. It only exists to test byte buckets; it is not a font.

use std::env;
use std::fs;
use std::process::exit;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        usage();
    }
    match args[1].as_str() {
        "ktx2-rgba8" if args.len() == 6 => {
            let w = num(&args[3]);
            let h = num(&args[4]);
            let mips = num(&args[5]) == 1;
            fs::write(&args[2], ktx2_rgba8(w, h, mips)).expect("write");
        }
        "ktx2-astc" if args.len() == 5 => {
            let w = num(&args[3]);
            let h = num(&args[4]);
            fs::write(&args[2], ktx2_astc(w, h)).expect("write");
        }
        "opus-input-rate" if args.len() == 4 => {
            let mut data = fs::read(&args[2]).expect("read");
            opus_input_rate(&mut data, num(&args[3]));
            fs::write(&args[2], data).expect("write");
        }
        "fake-font" if args.len() == 5 => {
            let mut data = args[3].as_bytes().to_vec();
            data.resize(num(&args[4]) as usize, 0);
            fs::write(&args[2], data).expect("write");
        }
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!("usage: fixture_tool ktx2-rgba8 <out> <w> <h> <mips 0|1>");
    eprintln!("       fixture_tool ktx2-astc <out> <w> <h>");
    eprintln!("       fixture_tool opus-input-rate <file> <rate>");
    eprintln!("       fixture_tool fake-font <out> <magic> <size>");
    exit(2);
}

fn num(s: &str) -> u32 {
    s.parse().unwrap_or_else(|_| usage())
}

const KTX2_ID: [u8; 12] = [0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];

fn le32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn le64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn pad_to(v: &mut Vec<u8>, align: usize) {
    while v.len() % align != 0 {
        v.push(0);
    }
}

/// Basic data format descriptor block (KDF 1.3, version 2).
struct Dfd {
    color_model: u8,
    texel_dims_minus_1: [u8; 4],
    bytes_plane0: u8,
    // (bit_offset, bit_length_minus_1, channel_type, lower, upper)
    samples: Vec<(u16, u8, u8, u32, u32)>,
}

fn dfd_bytes(d: &Dfd) -> Vec<u8> {
    let block_size = 24 + 16 * d.samples.len() as u32;
    let mut v = Vec::new();
    le32(&mut v, 4 + block_size); // dfdTotalSize
    le32(&mut v, 0); // vendorId 0 (Khronos), descriptorType 0 (basic)
    le32(&mut v, 2 | (block_size << 16)); // versionNumber 2, descriptorBlockSize
    // colorModel, colorPrimaries (BT709 = 1), transferFunction (linear = 1), flags (0)
    v.extend_from_slice(&[d.color_model, 1, 1, 0]);
    v.extend_from_slice(&d.texel_dims_minus_1);
    v.extend_from_slice(&[d.bytes_plane0, 0, 0, 0, 0, 0, 0, 0]);
    for &(off, len, ch, lower, upper) in &d.samples {
        v.extend_from_slice(&off.to_le_bytes());
        v.push(len);
        v.push(ch);
        v.extend_from_slice(&[0, 0, 0, 0]); // sample positions
        le32(&mut v, lower);
        le32(&mut v, upper);
    }
    v
}

fn kvd_bytes() -> Vec<u8> {
    let mut entry = b"KTXwriter\0dyno fixture_tool\0".to_vec();
    let mut v = Vec::new();
    le32(&mut v, entry.len() as u32);
    v.append(&mut entry);
    pad_to(&mut v, 4);
    v
}

/// Writes a KTX2 file. `levels[i]` is the data of mip level i (level 0 is the
/// largest). Level data is stored smallest first, as the KTX2 spec requires.
fn ktx2(vk_format: u32, type_size: u32, w: u32, h: u32, dfd: &Dfd, levels: &[Vec<u8>], align: usize) -> Vec<u8> {
    let level_count = levels.len() as u32;
    let dfd = dfd_bytes(dfd);
    let kvd = kvd_bytes();
    let header_len = 80 + 24 * levels.len();
    let dfd_off = header_len;
    let kvd_off = dfd_off + dfd.len();
    let mut body_start = kvd_off + kvd.len();
    while body_start % align != 0 {
        body_start += 1;
    }
    // Lay out level data, smallest level first.
    let mut offsets = vec![0usize; levels.len()];
    let mut cursor = body_start;
    for i in (0..levels.len()).rev() {
        while cursor % align != 0 {
            cursor += 1;
        }
        offsets[i] = cursor;
        cursor += levels[i].len();
    }

    let mut v = Vec::new();
    v.extend_from_slice(&KTX2_ID);
    le32(&mut v, vk_format);
    le32(&mut v, type_size);
    le32(&mut v, w);
    le32(&mut v, h);
    le32(&mut v, 0); // pixelDepth
    le32(&mut v, 0); // layerCount
    le32(&mut v, 1); // faceCount
    le32(&mut v, level_count);
    le32(&mut v, 0); // supercompressionScheme: none
    le32(&mut v, dfd_off as u32);
    le32(&mut v, dfd.len() as u32);
    le32(&mut v, kvd_off as u32);
    le32(&mut v, kvd.len() as u32);
    le64(&mut v, 0); // sgdByteOffset
    le64(&mut v, 0); // sgdByteLength
    for (i, level) in levels.iter().enumerate() {
        le64(&mut v, offsets[i] as u64);
        le64(&mut v, level.len() as u64);
        le64(&mut v, level.len() as u64);
    }
    assert_eq!(v.len(), header_len);
    v.extend_from_slice(&dfd);
    v.extend_from_slice(&kvd);
    for i in (0..levels.len()).rev() {
        v.resize(offsets[i], 0);
        v.extend_from_slice(&levels[i]);
    }
    v
}

fn ktx2_rgba8(w: u32, h: u32, mips: bool) -> Vec<u8> {
    let mut levels = Vec::new();
    let (mut lw, mut lh) = (w, h);
    loop {
        let mut data = Vec::with_capacity((lw * lh * 4) as usize);
        for y in 0..lh {
            for x in 0..lw {
                data.extend_from_slice(&[(x * 255 / lw.max(1)) as u8, (y * 255 / lh.max(1)) as u8, 128, 255]);
            }
        }
        levels.push(data);
        if !mips || (lw == 1 && lh == 1) {
            break;
        }
        lw = (lw / 2).max(1);
        lh = (lh / 2).max(1);
    }
    let dfd = Dfd {
        color_model: 1, // KHR_DF_MODEL_RGBSDA
        texel_dims_minus_1: [0, 0, 0, 0],
        bytes_plane0: 4,
        samples: vec![(0, 7, 0, 0, 255), (8, 7, 1, 0, 255), (16, 7, 2, 0, 255), (24, 7, 15, 0, 255)],
    };
    ktx2(37, 1, w, h, &dfd, &levels, 4)
}

fn ktx2_astc(w: u32, h: u32) -> Vec<u8> {
    let blocks = (w.div_ceil(4) * h.div_ceil(4)) as usize;
    let levels = vec![vec![0u8; blocks * 16]];
    let dfd = Dfd {
        color_model: 162, // KHR_DF_MODEL_ASTC
        texel_dims_minus_1: [3, 3, 0, 0],
        bytes_plane0: 16,
        samples: vec![(0, 127, 0, 0, u32::MAX)],
    };
    ktx2(157, 1, w, h, &dfd, &levels, 16)
}

/// Ogg CRC32: polynomial 0x04C11DB7, initial value 0, no reflection, no final xor.
fn ogg_crc(data: &[u8]) -> u32 {
    let mut crc: u32 = 0;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    crc
}

fn opus_input_rate(data: &mut [u8], rate: u32) {
    // The first page holds only the OpusHead packet.
    assert_eq!(&data[0..4], b"OggS", "not an Ogg file");
    let segments = data[26] as usize;
    let body = 27 + segments;
    let body_len: usize = data[27..body].iter().map(|&s| s as usize).sum();
    assert_eq!(&data[body..body + 8], b"OpusHead", "first packet is not OpusHead");
    // OpusHead: magic(8) version(1) channels(1) pre_skip(2) input_sample_rate(4) ...
    data[body + 12..body + 16].copy_from_slice(&rate.to_le_bytes());
    data[22..26].copy_from_slice(&[0, 0, 0, 0]);
    let crc = ogg_crc(&data[..body + body_len]);
    data[22..26].copy_from_slice(&crc.to_le_bytes());
}
