//! WAV (RIFF/WAVE): channels and rate from the `fmt ` chunk, the length from the `data` size, or
//! from the `fact` chunk for codecs that are not PCM.

use super::{BAD_HEADER, Codec, Container, Header, TRUNCATED, Unreadable, array};

/// "RIFF", the size, then "WAVE".
const RIFF_HEADER: usize = 12;

/// A chunk's id and size.
const CHUNK_HEADER: usize = 8;

/// The fields of `WAVEFORMATEX` up to `bits_per_sample`.
const FMT_MIN_BYTES: usize = 16;

/// `WAVEFORMATEXTENSIBLE`: `fmt ` holds the real format tag in the first 2 bytes of its
/// sub-format GUID, at byte 24.
const SUB_FORMAT_AT: usize = 24;

const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// A writer that streams a WAV cannot go back to fill in the `data` size, so it leaves one of
/// these. The data then runs to the end of the file.
const STREAMED_DATA_SIZES: [u32; 2] = [0, u32::MAX];

pub(super) fn read(bytes: &[u8]) -> Result<Header, Unreadable> {
    if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..RIFF_HEADER) != Some(b"WAVE") {
        return Err(Unreadable::invalid(BAD_HEADER));
    }
    let chunks = Chunks::read(bytes)?;
    let format = chunks.format.ok_or(Unreadable::invalid("no fmt chunk"))?;
    let data = chunks.data.ok_or(Unreadable::invalid("no data chunk"))?;
    let codec = format.codec();
    let ticks = match codec {
        Codec::Pcm | Codec::PcmFloat if format.block_align > 0 => {
            data.bytes / u64::from(format.block_align)
        }
        Codec::Pcm | Codec::PcmFloat => return Err(Unreadable::invalid(BAD_HEADER)),
        _ => chunks
            .fact_samples
            .ok_or(Unreadable::invalid("no fact chunk"))?,
    };
    Ok(Header {
        container: Container::Wav,
        codec,
        channels: format.channels.into(),
        sample_rate: format.sample_rate,
        ticks,
        tick_rate: format.sample_rate.into(),
        note: data.streamed.then_some("streamed_wav"),
    })
}

/// The chunks the length needs.
#[derive(Default)]
struct Chunks {
    format: Option<Format>,
    data: Option<Data>,
    /// The `fact` chunk's sample count, per channel.
    fact_samples: Option<u64>,
}

struct Format {
    tag: u16,
    channels: u16,
    sample_rate: u32,
    block_align: u16,
}

struct Data {
    bytes: u64,
    streamed: bool,
}

impl Chunks {
    /// Walks every chunk. A chunk the length needs must end inside the file. Any other chunk that
    /// runs past the end only ends the walk, since the audio before it is whole.
    fn read(bytes: &[u8]) -> Result<Chunks, Unreadable> {
        let mut chunks = Chunks::default();
        let mut at = RIFF_HEADER;
        while let (Some(id), Some(size)) = (array::<4>(bytes, at), array::<4>(bytes, at + 4)) {
            let size = u32::from_le_bytes(size);
            let body_start = at + CHUNK_HEADER;
            let rest = bytes.len() - body_start;
            if &id == b"data" && STREAMED_DATA_SIZES.contains(&size) {
                chunks.data = Some(Data {
                    bytes: rest as u64,
                    streamed: true,
                });
                break;
            }
            let needed = matches!(&id, b"fmt " | b"data" | b"fact");
            let Some(body) = bytes.get(body_start..body_start + size as usize) else {
                if needed {
                    return Err(Unreadable::invalid(TRUNCATED));
                }
                break;
            };
            match &id {
                b"fmt " => chunks.format = Some(Format::read(body)?),
                b"data" => {
                    chunks.data = Some(Data {
                        bytes: size.into(),
                        streamed: false,
                    });
                }
                b"fact" => {
                    let samples = array::<4>(body, 0).ok_or(Unreadable::invalid(TRUNCATED))?;
                    chunks.fact_samples = Some(u32::from_le_bytes(samples).into());
                }
                _ => {}
            }
            // Chunks are padded to an even size.
            at = body_start + size as usize + (size as usize & 1);
        }
        Ok(chunks)
    }
}

impl Format {
    fn read(body: &[u8]) -> Result<Format, Unreadable> {
        if body.len() < FMT_MIN_BYTES {
            return Err(Unreadable::invalid(BAD_HEADER));
        }
        let u16_at = |at: usize| array::<2>(body, at).map(u16::from_le_bytes);
        let mut tag = u16_at(0).unwrap_or_default();
        if tag == FORMAT_EXTENSIBLE {
            // Without a sub-format, section 5.2 counts an extensible format as PCM.
            tag = u16_at(SUB_FORMAT_AT).unwrap_or(FORMAT_PCM);
        }
        Ok(Format {
            tag,
            channels: u16_at(2).unwrap_or_default(),
            sample_rate: array::<4>(body, 4)
                .map(u32::from_le_bytes)
                .unwrap_or_default(),
            block_align: u16_at(12).unwrap_or_default(),
        })
    }

    fn codec(&self) -> Codec {
        match self.tag {
            FORMAT_PCM => Codec::Pcm,
            FORMAT_FLOAT => Codec::PcmFloat,
            _ => Codec::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV with these chunks after the RIFF header. Each chunk is (id, body).
    fn wav(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
        for (id, body) in chunks {
            bytes.extend_from_slice(*id);
            bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
            bytes.extend_from_slice(body);
            if body.len() % 2 == 1 {
                bytes.push(0);
            }
        }
        bytes
    }

    /// A 16-byte `fmt ` body: tag, channels, rate, then 2-byte samples.
    fn fmt(tag: u16, channels: u16, sample_rate: u32) -> Vec<u8> {
        let block_align = channels * 2;
        let mut body = Vec::new();
        body.extend_from_slice(&tag.to_le_bytes());
        body.extend_from_slice(&channels.to_le_bytes());
        body.extend_from_slice(&sample_rate.to_le_bytes());
        body.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
        body.extend_from_slice(&block_align.to_le_bytes());
        body.extend_from_slice(&16u16.to_le_bytes());
        body
    }

    #[test]
    fn pcm_length_is_the_data_size_over_the_block_size() {
        let bytes = wav(&[(b"fmt ", fmt(1, 2, 8_000)), (b"data", vec![0; 400])]);
        let header = read(&bytes).unwrap();
        assert_eq!(header.codec, Codec::Pcm);
        assert_eq!(
            (header.channels, header.ticks, header.tick_rate),
            (2, 100, 8_000)
        );
        assert_eq!(header.note, None);
    }

    #[test]
    fn an_odd_sized_chunk_is_padded_to_an_even_size() {
        let bytes = wav(&[
            (b"LIST", vec![0; 3]),
            (b"fmt ", fmt(1, 1, 8_000)),
            (b"data", vec![0; 10]),
        ]);
        assert_eq!(read(&bytes).unwrap().ticks, 5);
    }

    #[test]
    fn extensible_format_takes_its_codec_from_the_sub_format() {
        let mut body = fmt(FORMAT_EXTENSIBLE, 1, 8_000);
        body.extend_from_slice(&22u16.to_le_bytes());
        body.extend_from_slice(&[0; 6]);
        body.extend_from_slice(&FORMAT_FLOAT.to_le_bytes());
        body.extend_from_slice(&[0; 14]);
        let bytes = wav(&[(b"fmt ", body), (b"data", vec![0; 8])]);
        assert_eq!(read(&bytes).unwrap().codec, Codec::PcmFloat);
    }

    #[test]
    fn an_extensible_format_too_short_for_a_sub_format_is_pcm() {
        let bytes = wav(&[
            (b"fmt ", fmt(FORMAT_EXTENSIBLE, 1, 8_000)),
            (b"data", vec![0; 8]),
        ]);
        let header = read(&bytes).unwrap();
        assert_eq!((header.codec, header.ticks), (Codec::Pcm, 4));
    }

    #[test]
    fn another_codec_takes_its_length_from_fact() {
        let adpcm = 0x11;
        let bytes = wav(&[
            (b"fmt ", fmt(adpcm, 1, 8_000)),
            (b"fact", 4_000u32.to_le_bytes().to_vec()),
            (b"data", vec![0; 100]),
        ]);
        let header = read(&bytes).unwrap();
        assert_eq!((header.codec, header.ticks), (Codec::Other, 4_000));
    }

    #[test]
    fn another_codec_without_fact_is_invalid() {
        let bytes = wav(&[(b"fmt ", fmt(0x11, 1, 8_000)), (b"data", vec![0; 100])]);
        assert_eq!(read(&bytes), Err(Unreadable::invalid("no fact chunk")));
    }

    #[test]
    fn a_streamed_data_size_runs_to_the_end_of_the_file() {
        for streamed_size in STREAMED_DATA_SIZES {
            let mut bytes = wav(&[(b"fmt ", fmt(1, 1, 8_000))]);
            bytes.extend_from_slice(b"data");
            bytes.extend_from_slice(&streamed_size.to_le_bytes());
            bytes.extend_from_slice(&[0; 20]);
            let header = read(&bytes).unwrap();
            assert_eq!((header.ticks, header.note), (10, Some("streamed_wav")));
        }
    }

    #[test]
    fn data_past_the_end_is_truncated() {
        let mut bytes = wav(&[(b"fmt ", fmt(1, 1, 8_000)), (b"data", vec![0; 100])]);
        bytes.truncate(bytes.len() - 1);
        assert_eq!(read(&bytes), Err(Unreadable::invalid(TRUNCATED)));
    }

    #[test]
    fn a_damaged_chunk_after_the_data_ends_the_walk() {
        let mut bytes = wav(&[
            (b"fmt ", fmt(1, 1, 8_000)),
            (b"data", vec![0; 10]),
            (b"LIST", vec![0; 10]),
        ]);
        bytes.truncate(bytes.len() - 1);
        assert_eq!(read(&bytes).unwrap().ticks, 5);
    }

    #[test]
    fn a_wav_without_fmt_is_invalid() {
        let no_fmt = wav(&[(b"data", vec![0; 10])]);
        assert_eq!(read(&no_fmt), Err(Unreadable::invalid("no fmt chunk")));
    }

    #[test]
    fn a_wav_without_data_is_invalid() {
        let no_data = wav(&[(b"fmt ", fmt(1, 1, 8_000))]);
        assert_eq!(read(&no_data), Err(Unreadable::invalid("no data chunk")));
    }

    #[test]
    fn a_riff_header_cut_short_is_invalid() {
        assert_eq!(read(b"RIFF"), Err(Unreadable::invalid(BAD_HEADER)));
    }
}
