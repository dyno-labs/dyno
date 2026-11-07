//! AAC in ADTS: one header per frame (ISO/IEC 14496-3). The length is the sum over every frame,
//! so it includes the encoder's priming and padding.

use super::{Codec, Container, Header, TRUNCATED, Unreadable, array, mp3};

const HEADER: usize = 7;

/// The header grows by a 16-bit CRC when `protection_absent` is 0.
const HEADER_WITH_CRC: usize = 9;

/// Samples in one raw data block.
const BLOCK_SAMPLES: u64 = 1_024;

const SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

pub(super) fn read(bytes: &[u8]) -> Result<Header, Unreadable> {
    let start = mp3::skip_id3v2(bytes);
    let end = mp3::end_before_id3v1(bytes);
    let first = Frame::at(bytes, start).ok_or(Unreadable::invalid("no frame sync"))?;
    let channels = match first.channel_configuration {
        0 => {
            return Err(Unreadable::unsupported(
                "aac channels in a program config element",
            ));
        }
        // Configuration 7 is 7.1: eight channels.
        7 => 8,
        configuration => u32::from(configuration),
    };
    let mut ticks = 0;
    let mut at = start;
    while at < end {
        let Some(frame) = Frame::at(bytes, at) else {
            let reason = if end - at < HEADER {
                TRUNCATED
            } else {
                "lost sync"
            };
            return Err(Unreadable::invalid(reason));
        };
        if at + frame.length > end {
            return Err(Unreadable::invalid(TRUNCATED));
        }
        ticks += u64::from(frame.raw_data_blocks) * BLOCK_SAMPLES;
        at += frame.length;
    }
    Ok(Header {
        container: Container::Adts,
        codec: Codec::Aac,
        channels,
        sample_rate: first.sample_rate,
        ticks,
        tick_rate: first.sample_rate.into(),
        note: None,
    })
}

/// Whether an ADTS header starts at `at`: a 12-bit sync with layer bits 00.
pub(super) fn starts_frame(bytes: &[u8], at: usize) -> bool {
    matches!(array::<2>(bytes, at), Some([0xFF, second]) if second & 0xF6 == 0xF0)
}

/// One ADTS frame header.
struct Frame {
    sample_rate: u32,
    channel_configuration: u8,
    /// The whole frame, header included.
    length: usize,
    /// `number_of_raw_data_blocks_in_frame` plus 1.
    raw_data_blocks: u8,
}

impl Frame {
    fn at(bytes: &[u8], at: usize) -> Option<Frame> {
        if !starts_frame(bytes, at) {
            return None;
        }
        let header = array::<HEADER>(bytes, at)?;
        let protection_absent = header[1] & 1 == 1;
        let sample_rate = *SAMPLE_RATES.get(usize::from((header[2] >> 2) & 0x0F))?;
        let channel_configuration = ((header[2] & 1) << 2) | (header[3] >> 6);
        let length = (usize::from(header[3] & 0b11) << 11)
            | (usize::from(header[4]) << 3)
            | usize::from(header[5] >> 5);
        let header_length = if protection_absent {
            HEADER
        } else {
            HEADER_WITH_CRC
        };
        if length < header_length {
            return None;
        }
        Some(Frame {
            sample_rate,
            channel_configuration,
            length,
            raw_data_blocks: (header[6] & 0b11) + 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ADTS frame of `length` bytes: AAC LC at 44.1 kHz, no CRC.
    fn frame(channels: u8, length: usize, raw_data_blocks: u8) -> Vec<u8> {
        let sample_rate_index = 4;
        let mut bytes = vec![
            0xFF,
            0xF1,
            (1 << 6) | (sample_rate_index << 2) | (channels >> 2),
            ((channels & 0b11) << 6) | (length >> 11) as u8,
            (length >> 3) as u8,
            ((length & 0b111) << 5) as u8 | 0x1F,
            0xFC | (raw_data_blocks - 1),
        ];
        bytes.resize(length, 0);
        bytes
    }

    #[test]
    fn length_is_1024_samples_a_raw_data_block() {
        let bytes = [frame(2, 20, 1), frame(2, 30, 2), frame(2, 25, 1)].concat();
        let header = read(&bytes).unwrap();
        assert_eq!(
            (header.channels, header.ticks, header.tick_rate),
            (2, 4 * 1_024, 44_100)
        );
    }

    #[test]
    fn channel_configuration_7_is_eight_channels() {
        assert_eq!(read(&frame(7, 20, 1)).unwrap().channels, 8);
    }

    #[test]
    fn a_last_frame_past_the_end_is_truncated() {
        let mut bytes = [frame(1, 20, 1), frame(1, 20, 1)].concat();
        bytes.truncate(bytes.len() - 1);
        assert_eq!(read(&bytes), Err(Unreadable::invalid(TRUNCATED)));
    }

    #[test]
    fn a_last_header_cut_short_is_truncated() {
        let cut_header = frame(1, 20, 1)[..HEADER - 2].to_vec();
        let bytes = [frame(1, 20, 1), cut_header].concat();
        assert_eq!(read(&bytes), Err(Unreadable::invalid(TRUNCATED)));
    }

    #[test]
    fn junk_between_frames_loses_sync() {
        let bytes = [frame(1, 20, 1), vec![0; 10], frame(1, 20, 1)].concat();
        assert_eq!(read(&bytes), Err(Unreadable::invalid("lost sync")));
    }

    #[test]
    fn channels_in_a_program_config_element_are_unsupported() {
        assert_eq!(
            read(&frame(0, 20, 1)),
            Err(Unreadable::unsupported(
                "aac channels in a program config element"
            ))
        );
    }
}
