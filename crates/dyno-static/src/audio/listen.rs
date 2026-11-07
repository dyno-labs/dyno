//! Listening to a sound: decoding it with symphonia to measure its silence and compare its
//! channels (design section 5.4).
//!
//! A sound is decoded twice, as a stream, and only running sums are kept, never the samples, so
//! memory does not grow with the sound. The first pass counts the frames symphonia gives. Our
//! header readers stay the authority on length: symphonia can return frames a browser does not
//! play, such as the priming and padding an M4A edit list leaves out. So the second pass skips a
//! lead-in and measures the header's length after it. The lead-in is the edit list's start for an
//! M4A, and otherwise every extra frame, taken from the start, where priming sits. A decode that
//! gives clearly less than the header's length is not used. symphonia has no Opus decoder, so an
//! Opus sound is not listened to.

use std::io::Cursor;

use dyno_schema::path::RelPath;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

use crate::audio::mp4::{self, LeadIn};
use crate::audio::{Codec, Container, Header};

/// What listening to a sound gives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Listening {
    /// Windows the sound plays, after the extra frames are skipped.
    pub windows: u64,
    /// Windows at or below `SILENCE_DBFS`.
    pub silent_windows: u64,
    /// None for a sound with one channel.
    pub stereo: Option<StereoClass>,
}

/// What mixing a sound with two or more channels down to mono does to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StereoClass {
    /// The channels match: what they do not share is at least `SAFE_SIDE_DB` below the mono mix,
    /// or the sound is digital silence. A mono export sounds the same.
    SafeToMakeMono,
    /// The channels differ, so a mono export changes the sound.
    ChangesTheSound,
    /// The channels are out of phase: the mono mix keeps `CANCEL_KEPT_SHARE` of the channels'
    /// energy or less, so most of the sound cancels.
    Cancels,
}

/// The length of a window, as a share of a second: 10 ms.
const WINDOWS_PER_SECOND: u32 = 100;

/// A window is silent at or below -60 dBFS: an energy of 10^-6 a sample. A proposal.
pub const SILENCE_DBFS: f64 = -60.0;

/// The channels match when what they do not share is at least 40 dB below the mono mix: an
/// energy ratio of 10^-4. For two channels that is the side (L - R) / 2 against the mid
/// (L + R) / 2. Decided: it does not get looser without a listening test.
pub const SAFE_SIDE_DB: f64 = -40.0;

/// A mono mix cancels when it keeps a quarter of the channels' energy or less: -6 dB. A proposal.
pub const CANCEL_KEPT_SHARE: f64 = 0.25;

/// A decode may give up to 1% fewer frames than the header, or one window, whichever is more,
/// before it is too short to use: encoders round the last frame.
const SHORT_DECODE_SHARE: f64 = 0.01;

/// Why a sound was not listened to.
pub const NO_OPUS_DECODER: &str = "symphonia has no Opus decoder";
pub const CANNOT_DECODE: &str = "symphonia cannot decode it";
pub const DECODED_SHORT: &str = "symphonia decoded less than the header's length";
pub const SEVERAL_MEDIA_EDITS: &str = "its edit list plays more than one part of the media";

/// The running sums of the frames measured so far.
#[derive(Clone, Copy, Debug, Default)]
struct Sums {
    frames_per_window: u32,
    /// The open window: its frames, its samples over every channel, and their energy.
    window_frames: u32,
    window_samples: u64,
    window_energy: f64,
    windows: u64,
    silent_windows: u64,
    /// Over every frame: the mean energy of its channels, and the energy of its mono mix.
    channel_energy: f64,
    mono_energy: f64,
    most_channels: usize,
}

/// Decodes a sound and measures it. `header` is our reader's, which gives the playable length.
pub fn listen(path: &RelPath, bytes: &[u8], header: &Header) -> Result<Listening, &'static str> {
    if header.codec == Codec::Opus {
        return Err(NO_OPUS_DECODER);
    }
    let mut decoded: u64 = 0;
    let mut first_rate: Option<u32> = None;
    decode(path, bytes, |planes, rate| {
        first_rate.get_or_insert(rate);
        decoded += planes.first().map_or(0, Vec::len) as u64;
    })?;
    let rate = first_rate.ok_or(CANNOT_DECODE)?;
    let frames_per_window = (rate / WINDOWS_PER_SECOND).max(1);
    let playable = header_frames(header.ticks, header.tick_rate, rate);
    let extra = decoded.saturating_sub(playable);
    // symphonia 0.6 ignores edit lists, so it gives the lead-in an M4A's edit list states. The
    // cross-check in tests/audio.rs fails if a later symphonia applies them.
    let lead_in = match header.container {
        Container::Mp4 => match mp4::lead_in(bytes) {
            LeadIn::Ticks(ticks, tick_rate) => Some(header_frames(ticks, tick_rate, rate)),
            LeadIn::SeveralSegments => return Err(SEVERAL_MEDIA_EDITS),
            LeadIn::Unknown => None,
        },
        _ => None,
    };
    let mut skip = lead_in.unwrap_or(extra);
    let allowed_short =
        ((playable as f64 * SHORT_DECODE_SHARE) as u64).max(frames_per_window.into());
    if decoded.saturating_sub(skip).saturating_add(allowed_short) < playable {
        return Err(DECODED_SHORT);
    }
    let mut left = playable;
    let mut sums = Sums {
        frames_per_window,
        ..Sums::default()
    };
    decode(path, bytes, |planes, _| {
        let frames = planes.first().map_or(0, Vec::len);
        let skipped = usize::try_from(skip).unwrap_or(usize::MAX).min(frames);
        skip -= skipped as u64;
        let measured = usize::try_from(left)
            .unwrap_or(usize::MAX)
            .min(frames - skipped);
        left -= measured as u64;
        for frame in skipped..skipped + measured {
            sums.add(planes, frame);
        }
    })?;
    Ok(sums.finish())
}

/// Decodes the first audio track and hands each buffer to `each`, with its rate. A packet that
/// fails is tried once more with a new decoder, since symphonia refuses a packet whose channels
/// differ from the first; a packet that still fails is skipped, as a browser skips it.
fn decode(
    path: &RelPath,
    bytes: &[u8],
    mut each: impl FnMut(&[Vec<f32>], u32),
) -> Result<(), &'static str> {
    let mut hint = Hint::new();
    if let Some((_, extension)) = path.as_str().rsplit_once('.') {
        hint.with_extension(extension);
    }
    let source = MediaSourceStream::new(
        Box::new(Cursor::new(bytes.to_vec())),
        MediaSourceStreamOptions::default(),
    );
    let mut reader = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|_| CANNOT_DECODE)?;
    let track = reader.first_track(TrackType::Audio).ok_or(CANNOT_DECODE)?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or(CANNOT_DECODE)?
        .clone();
    let mut decoder = new_decoder(&params)?;
    let mut planes: Vec<Vec<f32>> = Vec::new();
    loop {
        let packet = match reader.next_packet() {
            Ok(Some(packet)) => packet,
            // The end of the stream, or a truncated one: the length check judges what decoded.
            Ok(None) | Err(Error::IoError(_)) => break,
            Err(_) => return Err(CANNOT_DECODE),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buffer = match decoder.decode(&packet) {
            Ok(buffer) => buffer,
            Err(Error::DecodeError(_)) => {
                decoder = new_decoder(&params)?;
                match decoder.decode(&packet) {
                    Ok(buffer) => buffer,
                    Err(Error::DecodeError(_)) => continue,
                    Err(_) => return Err(CANNOT_DECODE),
                }
            }
            Err(_) => return Err(CANNOT_DECODE),
        };
        let rate = buffer.spec().rate();
        buffer.copy_to_vecs_planar::<f32>(&mut planes);
        each(&planes, rate);
    }
    Ok(())
}

fn new_decoder(params: &AudioCodecParameters) -> Result<Box<dyn AudioDecoder>, &'static str> {
    symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|_| CANNOT_DECODE)
}

/// `ticks` in a clock of `tick_rate` a second, as frames at the decoder's rate.
fn header_frames(ticks: u64, tick_rate: u64, rate: u32) -> u64 {
    let frames = u128::from(ticks) * u128::from(rate) / u128::from(tick_rate.max(1));
    u64::try_from(frames).unwrap_or(u64::MAX)
}

impl Sums {
    fn add(&mut self, planes: &[Vec<f32>], frame: usize) {
        let channels = planes.len();
        if channels == 0 {
            return;
        }
        let (mut sum, mut energy) = (0.0, 0.0);
        for plane in planes {
            let sample = f64::from(plane[frame]);
            sum += sample;
            energy += sample * sample;
        }
        let count = channels as f64;
        self.channel_energy += energy / count;
        self.mono_energy += (sum / count) * (sum / count);
        self.most_channels = self.most_channels.max(channels);
        self.window_frames += 1;
        self.window_samples += channels as u64;
        self.window_energy += energy;
        if self.window_frames == self.frames_per_window {
            self.close_window();
        }
    }

    fn close_window(&mut self) {
        let silence_energy = 10_f64.powf(SILENCE_DBFS / 10.0);
        self.windows += 1;
        if self.window_energy / self.window_samples.max(1) as f64 <= silence_energy {
            self.silent_windows += 1;
        }
        self.window_frames = 0;
        self.window_samples = 0;
        self.window_energy = 0.0;
    }

    fn finish(mut self) -> Listening {
        if self.window_frames > 0 {
            self.close_window();
        }
        Listening {
            windows: self.windows,
            silent_windows: self.silent_windows,
            stereo: (self.most_channels >= 2)
                .then(|| stereo_class(self.channel_energy, self.mono_energy)),
        }
    }
}

/// The class of a sound with two or more channels, from the mean energy of its channels and the
/// energy of its mono mix, both summed over every frame. What the channels do not share is the
/// difference.
pub fn stereo_class(channel_energy: f64, mono_energy: f64) -> StereoClass {
    let unshared = (channel_energy - mono_energy).max(0.0);
    if channel_energy <= 0.0 || unshared <= mono_energy * 10_f64.powf(SAFE_SIDE_DB / 10.0) {
        StereoClass::SafeToMakeMono
    } else if mono_energy <= channel_energy * CANCEL_KEPT_SHARE {
        StereoClass::Cancels
    } else {
        StereoClass::ChangesTheSound
    }
}

impl Listening {
    /// The silent share in basis points, rounded half up. 0 for no windows.
    pub fn silence_bp(&self) -> u32 {
        if self.windows == 0 {
            return 0;
        }
        let share = (u128::from(self.silent_windows) * 20_000 + u128::from(self.windows))
            / (2 * u128::from(self.windows));
        u32::try_from(share).unwrap_or(u32::MAX)
    }
}

impl StereoClass {
    /// The name in a result and in `expected.toml`.
    pub fn name(self) -> &'static str {
        match self {
            StereoClass::SafeToMakeMono => "safe_to_make_mono",
            StereoClass::ChangesTheSound => "changes_the_sound",
            StereoClass::Cancels => "cancels",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::read_header;

    const RATE: u32 = 48_000;

    /// A 16-bit PCM WAV with one plane per channel, all the same length.
    fn wav(channels: &[&[f32]]) -> Vec<u8> {
        let count = u16::try_from(channels.len()).unwrap();
        let frames = channels[0].len();
        let data_bytes = frames * channels.len() * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&u32::try_from(36 + data_bytes).unwrap().to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&RATE.to_le_bytes());
        bytes.extend_from_slice(&(RATE * u32::from(count) * 2).to_le_bytes());
        bytes.extend_from_slice(&(count * 2).to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&u32::try_from(data_bytes).unwrap().to_le_bytes());
        for frame in 0..frames {
            for channel in channels {
                let value = (channel[frame] * 32_767.0).round() as i16;
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes
    }

    fn tone(hertz: f32, seconds: f32) -> Vec<f32> {
        let frames = (RATE as f32 * seconds) as usize;
        (0..frames)
            .map(|frame| {
                0.4 * (2.0 * std::f32::consts::PI * hertz * frame as f32 / RATE as f32).sin()
            })
            .collect()
    }

    fn listen_to(bytes: &[u8]) -> Listening {
        let path = RelPath::new("a.wav").unwrap();
        let header = read_header(&path, bytes).unwrap();
        listen(&path, bytes, &header).unwrap()
    }

    #[test]
    fn identical_channels_are_safe_to_make_mono() {
        let left = tone(440.0, 0.5);
        let listening = listen_to(&wav(&[&left, &left]));
        assert_eq!(listening.stereo, Some(StereoClass::SafeToMakeMono));
    }

    #[test]
    fn different_channels_change_the_sound() {
        let listening = listen_to(&wav(&[&tone(440.0, 0.5), &tone(660.0, 0.5)]));
        assert_eq!(listening.stereo, Some(StereoClass::ChangesTheSound));
    }

    #[test]
    fn inverted_channels_cancel() {
        let left = tone(440.0, 0.5);
        let right: Vec<f32> = left.iter().map(|sample| -sample).collect();
        let listening = listen_to(&wav(&[&left, &right]));
        assert_eq!(listening.stereo, Some(StereoClass::Cancels));
    }

    #[test]
    fn digital_silence_is_safe_to_make_mono_and_all_silent() {
        let silence = vec![0.0; RATE as usize / 2];
        let listening = listen_to(&wav(&[&silence, &silence]));
        assert_eq!(listening.stereo, Some(StereoClass::SafeToMakeMono));
        assert_eq!(listening.silence_bp(), 10_000);
    }

    #[test]
    fn the_silent_share_counts_quiet_windows() {
        // 0.25 s of tone, then 0.75 s of silence: 75% silent.
        let mut samples = tone(523.25, 0.25);
        samples.extend(vec![0.0; RATE as usize * 3 / 4]);
        let listening = listen_to(&wav(&[&samples]));
        assert_eq!(listening.windows, 100);
        assert_eq!(listening.silence_bp(), 7_500);
        assert_eq!(listening.stereo, None);
    }

    #[test]
    fn the_stereo_class_follows_the_energy_of_the_mono_mix() {
        // Channel energy against mono-mix energy.
        assert_eq!(stereo_class(1.0, 1.0), StereoClass::SafeToMakeMono);
        assert_eq!(stereo_class(0.0, 0.0), StereoClass::SafeToMakeMono);
        assert_eq!(stereo_class(1.0, 0.5), StereoClass::ChangesTheSound);
        assert_eq!(stereo_class(1.0, 0.25), StereoClass::Cancels);
        assert_eq!(stereo_class(1.0, 0.0), StereoClass::Cancels);
    }

    #[test]
    fn an_opus_sound_is_not_listened_to() {
        let path = RelPath::new("a.opus").unwrap();
        let header = Header {
            container: crate::audio::Container::Ogg,
            codec: Codec::Opus,
            channels: 2,
            sample_rate: RATE,
            ticks: 48_000,
            tick_rate: 48_000,
            note: None,
        };
        assert_eq!(listen(&path, b"OggS", &header), Err(NO_OPUS_DECODER));
    }

    #[test]
    fn frames_past_the_header_length_are_skipped_from_the_start_to_the_frame() {
        // 105 ms of tone, then 1 s of silence, but the header says only the last 1 s plays: the
        // tone is priming, so nothing that plays is loud. 105 ms is not a whole number of
        // windows, so dropping whole windows would leave 5 ms of tone in the first one.
        let mut samples = tone(440.0, 0.105);
        samples.extend(vec![0.0; RATE as usize]);
        let bytes = wav(&[&samples]);
        let path = RelPath::new("a.wav").unwrap();
        let mut header = read_header(&path, &bytes).unwrap();
        header.ticks = u64::from(RATE);
        header.tick_rate = u64::from(RATE);
        let listening = listen(&path, &bytes, &header).unwrap();
        assert_eq!(listening.windows, 100);
        assert_eq!(listening.silence_bp(), 10_000);
    }

    #[test]
    fn three_channels_that_cancel_in_a_mono_mix_cancel() {
        // Left and right match, and the centre is their sum inverted: the mono mix is silent.
        let side = tone(440.0, 0.5);
        let centre: Vec<f32> = side.iter().map(|sample| -2.0 * sample).collect();
        let quiet: Vec<f32> = side.iter().map(|sample| sample / 2.0).collect();
        let listening = listen_to(&wav(&[&quiet, &quiet, &centre]));
        assert_eq!(listening.stereo, Some(StereoClass::Cancels));
    }

    #[test]
    fn a_change_of_channels_mid_stream_decodes_to_the_end() {
        // testdata/channel-change.mp3: 0.5 s of silent stereo, then 1 s of a mono 440 Hz tone,
        // two 128 kbit/s MP3s with no Xing header joined end to end (made with ffmpeg).
        // symphonia refuses the first mono packet until it gets a new decoder.
        let bytes = include_bytes!("testdata/channel-change.mp3");
        let path = RelPath::new("channel-change.mp3").unwrap();
        let header = read_header(&path, bytes).unwrap();
        let listening = listen(&path, bytes, &header).unwrap();
        let share = listening.silence_bp();
        assert!(
            (3_000..=3_700).contains(&share),
            "a third is silent, not all: {share}"
        );
    }

    #[test]
    fn a_damaged_m4a_still_skips_its_lead_in_and_is_too_short_to_use() {
        // testdata/last-packet-zeroed.m4a: 100 ms of ffmpeg AAC at 48 kHz, an edit list that
        // starts at 1,024 frames, and its last packet zeroed. symphonia gives 5,120 frames for
        // 4,800 playable; after the lead-in only 4,096 are left.
        let bytes = include_bytes!("testdata/last-packet-zeroed.m4a");
        let path = RelPath::new("last-packet-zeroed.m4a").unwrap();
        let header = read_header(&path, bytes).unwrap();
        assert_eq!(listen(&path, bytes, &header), Err(DECODED_SHORT));
    }

    #[test]
    fn a_decode_much_shorter_than_the_header_is_not_used() {
        let bytes = wav(&[&tone(440.0, 0.5)]);
        let path = RelPath::new("a.wav").unwrap();
        let mut header = read_header(&path, &bytes).unwrap();
        header.ticks = u64::from(RATE);
        header.tick_rate = u64::from(RATE);
        assert_eq!(listen(&path, &bytes, &header), Err(DECODED_SHORT));
    }
}
