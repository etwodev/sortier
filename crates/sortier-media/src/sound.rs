//! Sound for video playback: decoding a video's sound from any point with FFmpeg, queueing it, and playing the
//! queue through the system output with cpal, resampled and mixed to the device as it goes. What the output has
//! played is the clock pictures follow.

use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use ffmpeg_next::software::resampling;
use ffmpeg_next::util::format::sample::{Sample, Type as SampleType};
use ffmpeg_next::util::frame;
use ffmpeg_next::{ChannelLayout, codec};

use crate::MediaError;
use crate::video::{StreamReader, VideoSource, media_error, open_source, sound_stream};

/// A stretch of decoded sound: interleaved stereo at `rate`, starting `start` seconds into the video.
pub(crate) struct Chunk {
    pub start: f64,
    pub rate: u32,
    pub samples: Vec<f32>,
}

/// Interleaved mono or stereo samples as stereo.
fn to_stereo(samples: Vec<f32>, channels: usize) -> Vec<f32> {
    if channels == 1 { samples.iter().flat_map(|sample| [*sample, *sample]).collect() } else { samples }
}

/// Converts decoded sound of one sample format, layout and rate to interleaved 32-bit float: mono stays mono (to
/// be spread to both sides at full level), anything else is mixed down to stereo.
///
/// The layout is kept as its channel count and mask, since FFmpeg's layout type can't move between threads.
struct Mixer {
    context: resampling::Context,
    format: Sample,
    channels: i32,
    mask: u64,
    rate: u32,
}

impl Mixer {
    fn new(format: Sample, layout: ChannelLayout, rate: u32) -> Result<Self, MediaError> {
        let out = if layout.channels() == 1 { ChannelLayout::MONO } else { ChannelLayout::STEREO };
        let context =
            resampling::Context::get(format, layout, rate, Sample::F32(SampleType::Packed), out, rate).map_err(media_error)?;
        Ok(Self {
            context,
            format,
            channels: layout.channels(),
            mask: layout.bits(),
            rate,
        })
    }

    fn takes(&self, frame: &frame::Audio) -> bool {
        let layout = frame.channel_layout();
        self.format == frame.format() && self.channels == layout.channels() && self.mask == layout.bits() && self.rate == frame.rate()
    }

    /// `frame` as interleaved stereo.
    fn mix(&mut self, frame: &frame::Audio) -> Result<Vec<f32>, MediaError> {
        let mut mixed = frame::Audio::empty();
        self.context.run(frame, &mut mixed).map_err(media_error)?;
        let channels = usize::from(mixed.channels());
        let bytes = &mixed.data(0)[..mixed.samples() * channels * 4];
        let samples = bytes.as_chunks::<4>().0.iter().map(|bytes| f32::from_ne_bytes(*bytes)).collect();
        Ok(to_stereo(samples, channels))
    }
}

/// A file's main sound stream, decoded by FFmpeg.
pub(crate) struct SoundDecoder {
    reader: StreamReader,
    decoder: ffmpeg_next::decoder::Audio,
    mixer: Option<Mixer>,
    /// Sound before this (where the last seek went) is dropped.
    from: f64,
    /// Where the next frame starts, when frames don't say.
    next_time: f64,
}

impl SoundDecoder {
    /// Opens the sound of the video `source` reads. `Ok(None)` when it has none Sortier can decode.
    pub(crate) fn open(source: &VideoSource) -> Result<Option<Self>, MediaError> {
        let input = open_source(source)?;
        let Some(stream) = sound_stream(&input) else {
            return Ok(None);
        };
        let parameters = input.stream(stream).expect("the index came from this input").parameters();
        let decoder = codec::Context::from_parameters(parameters).map_err(media_error)?.decoder().audio().map_err(media_error)?;
        Ok(Some(Self {
            reader: StreamReader::new(input, stream),
            decoder,
            mixer: None,
            from: 0.0,
            next_time: 0.0,
        }))
    }

    /// Goes to `seconds` in; the next chunk starts there.
    pub(crate) fn seek(&mut self, seconds: f64) -> Result<(), MediaError> {
        self.reader.seek(seconds)?;
        self.decoder.flush();
        self.mixer = None;
        self.from = seconds;
        self.next_time = seconds;
        Ok(())
    }

    /// The next stretch of sound, or `None` at the end.
    pub(crate) fn next_chunk(&mut self) -> Result<Option<Chunk>, MediaError> {
        loop {
            let mut frame = frame::Audio::empty();
            if !self.reader.decode(&mut self.decoder, &mut frame)? {
                return Ok(None);
            }
            let mut start = frame
                .timestamp()
                .or_else(|| frame.pts())
                .map_or(self.next_time, |ticks| self.reader.seconds(ticks));
            let rate = frame.rate().max(1);
            if frame.channel_layout().is_empty() {
                frame.set_channel_layout(ChannelLayout::default(i32::from(frame.channels())));
            }
            if self.mixer.as_ref().is_none_or(|mixer| !mixer.takes(&frame)) {
                self.mixer = Some(Mixer::new(frame.format(), frame.channel_layout(), rate)?);
            }
            let mut samples = self.mixer.as_mut().expect("made above").mix(&frame)?;
            self.next_time = start + (samples.len() / 2) as f64 / f64::from(rate);
            if start < self.from {
                let skip = ((self.from - start) * f64::from(rate)).round() as usize * 2;
                if skip >= samples.len() {
                    continue;
                }
                samples.drain(..skip);
                start = self.from;
            }
            if samples.is_empty() {
                continue;
            }
            return Ok(Some(Chunk { start, rate, samples }));
        }
    }
}

/// Decoded sound waiting to be played, and how far playing has got. The decoding thread pushes, the output's
/// callback plays.
#[derive(Default)]
pub(crate) struct SoundQueue {
    /// Interleaved stereo.
    samples: VecDeque<f32>,
    rate: u32,
    /// The time of the first sample in the queue, in seconds into the video.
    time: f64,
    /// How far playing has got between the first and second samples, for resampling.
    fraction: f64,
    /// No more sound is coming (until the next seek).
    pub finished: bool,
}

/// What one output buffer played.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Filled {
    /// Where the buffer starts and ends, in seconds into the video.
    pub start: f64,
    pub end: f64,
    /// Whether any decoded sound was played.
    pub played: bool,
    /// Whether the queue ran dry with more sound still to come.
    pub starved: bool,
}

impl SoundQueue {
    /// Empties the queue to play on from `seconds`.
    pub(crate) fn reset(&mut self, seconds: f64) {
        self.samples.clear();
        self.time = seconds;
        self.fraction = 0.0;
        self.finished = false;
    }

    /// Where playing has got, in seconds into the video.
    pub(crate) fn now(&self) -> f64 {
        if self.rate == 0 { self.time } else { self.time + self.fraction / f64::from(self.rate) }
    }

    /// Seconds of sound queued.
    pub(crate) fn ahead(&self) -> f64 {
        if self.rate == 0 { 0.0 } else { (self.samples.len() / 2) as f64 / f64::from(self.rate) }
    }

    /// Queues `chunk` after what's there, padding a gap before it with silence and dropping any overlap, so the
    /// queue stays in step with the video.
    pub(crate) fn push(&mut self, chunk: Chunk) {
        if self.rate == 0 || self.samples.is_empty() {
            self.rate = chunk.rate.max(1);
        }
        let rate = f64::from(self.rate);
        let expected = self.time + (self.samples.len() / 2) as f64 / rate;
        let gap = chunk.start - expected;
        let mut samples = chunk.samples.as_slice();
        if gap > 0.02 {
            let silence = (gap.min(10.0) * rate).round() as usize * 2;
            self.samples.extend(std::iter::repeat_n(0.0, silence));
        } else if gap < -0.02 {
            let overlap = ((-gap * rate).round() as usize * 2).min(samples.len());
            samples = &samples[overlap..];
        }
        self.samples.extend(samples);
    }

    /// Plays from the queue into interleaved `out` (`channels` channels at `device_rate`), resampling linearly and
    /// mixing stereo to the device's channels, at `gain`. Silence where the queue has run dry.
    pub(crate) fn fill(&mut self, out: &mut [f32], channels: usize, device_rate: u32, gain: f32) -> Filled {
        let start = self.now();
        let step = f64::from(self.rate) / f64::from(device_rate.max(1));
        let (mut played, mut starved) = (false, false);
        for frame in out.chunks_mut(channels.max(1)) {
            let available = self.samples.len() / 2;
            let (left, right) = if available >= 2 || (self.finished && available == 1) {
                let next = usize::from(available >= 2);
                let between = self.fraction as f32;
                let at = |frame: usize, channel: usize| self.samples[frame * 2 + channel];
                let left = at(0, 0) + (at(next, 0) - at(0, 0)) * between;
                let right = at(0, 1) + (at(next, 1) - at(0, 1)) * between;
                self.fraction += step;
                while self.fraction >= 1.0 && !self.samples.is_empty() {
                    self.samples.drain(..2);
                    self.time += 1.0 / f64::from(self.rate);
                    self.fraction -= 1.0;
                }
                if self.samples.is_empty() {
                    self.fraction = 0.0;
                }
                played = true;
                ((left * gain).clamp(-1.0, 1.0), (right * gain).clamp(-1.0, 1.0))
            } else {
                starved |= !self.finished;
                (0.0, 0.0)
            };
            match frame {
                [only] => *only = (left + right) / 2.0,
                [first, second, rest @ ..] => {
                    *first = left;
                    *second = right;
                    rest.fill(0.0);
                }
                [] => {}
            }
        }
        Filled {
            start,
            end: self.now(),
            played,
            starved,
        }
    }
}

/// Plays sound through the system's default output on a thread of its own, which keeps the stream open until
/// `stop`'s sender is dropped. `fill(out, channels, rate, heard_at)` fills each buffer the device asks for, with the
/// moment its first sample will be heard. `failed` is called if there's no output or it stops working.
pub(crate) fn start_output(
    mut fill: impl FnMut(&mut [f32], usize, u32, Instant) + Send + 'static,
    failed: impl Fn() + Send + Sync + 'static,
    stop: Receiver<()>,
) {
    let failed = std::sync::Arc::new(failed);
    let on_error = failed.clone();
    // Without the thread nothing plays, and the player keeps time by the wall clock as it does without a device.
    let _ = std::thread::Builder::new().name("sortier-sound-output".into()).spawn(move || {
        let open = || -> Result<cpal::Stream, String> {
            let device = cpal::default_host().default_output_device().ok_or("there's no sound output device")?;
            let config = device.default_output_config().map_err(|error| error.to_string())?.config();
            let channels = usize::from(config.channels);
            let rate = config.sample_rate;
            let stream = device
                .build_output_stream::<f32, _, _>(
                    config,
                    move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                        let stamp = info.timestamp();
                        // Sound written now is heard after the device's latency.
                        let latency = stamp.playback.duration_since(stamp.callback).min(Duration::from_millis(500));
                        fill(out, channels, rate, Instant::now() + latency);
                    },
                    move |_| on_error(),
                    None,
                )
                .map_err(|error| error.to_string())?;
            stream.play().map_err(|error| error.to_string())?;
            Ok(stream)
        };
        match open() {
            Ok(stream) => {
                // Waits until the player is dropped, which drops the sender.
                let _ = stop.recv();
                drop(stream);
            }
            Err(_) => failed(),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second at 1000 Hz whose left channel ramps from 0 to 1 and right channel is its negative.
    fn ramp(start: f64) -> Chunk {
        Chunk {
            start,
            rate: 1000,
            samples: (0..1000).flat_map(|index| [index as f32 / 1000.0, -(index as f32) / 1000.0]).collect(),
        }
    }

    #[test]
    fn queued_sound_is_resampled_to_the_device_and_mixed_to_its_channels() {
        let mut queue = SoundQueue::default();
        queue.reset(2.0);
        queue.push(ramp(2.0));
        assert!((queue.ahead() - 1.0).abs() < 1e-9);
        let mut out = vec![0.0_f32; 8];
        let filled = queue.fill(&mut out, 2, 2000, 1.0);
        assert!(filled.played && !filled.starved);
        assert_eq!(filled.start, 2.0);
        assert!((filled.end - 2.002).abs() < 1e-9, "four frames at twice the rate: {}", filled.end);
        assert!((out[2] - 0.0005).abs() < 1e-6, "halfway between the first two samples: {}", out[2]);
        assert!((out[3] + 0.0005).abs() < 1e-6);
        let mut mono = vec![0.0_f32; 4];
        queue.fill(&mut mono, 1, 1000, 1.0);
        assert!(mono.iter().all(|sample| sample.abs() < 1e-6), "left and right cancel out in mono");
        let mut surround = vec![1.0_f32; 6];
        queue.fill(&mut surround, 6, 1000, 0.5);
        assert!(surround[0] > 0.0 && surround[1] < 0.0 && surround[2..].iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn a_dry_queue_is_silent_and_says_so_until_the_sound_ends() {
        let mut queue = SoundQueue::default();
        queue.reset(0.0);
        let mut out = vec![1.0_f32; 4];
        let filled = queue.fill(&mut out, 2, 1000, 1.0);
        assert!(!filled.played && filled.starved);
        assert_eq!(out, [0.0; 4]);
        queue.finished = true;
        assert!(!queue.fill(&mut out, 2, 1000, 1.0).starved, "nothing more is coming");
    }

    #[test]
    fn gaps_are_filled_with_silence_and_overlaps_dropped() {
        let mut queue = SoundQueue::default();
        queue.reset(0.0);
        queue.push(ramp(0.5));
        assert!((queue.ahead() - 1.5).abs() < 1e-3, "half a second of silence first: {}", queue.ahead());
        queue.push(ramp(1.0));
        assert!((queue.ahead() - 2.0).abs() < 1e-3, "the overlapping half second is dropped: {}", queue.ahead());
    }

    /// Plays a moment of silence through the default output where there is one.
    #[test]
    fn the_output_asks_for_sound_when_there_is_a_device() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let (calls, failed) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        let (counting, failing) = (calls.clone(), failed.clone());
        let (sender, receiver) = std::sync::mpsc::channel();
        start_output(
            move |out, channels, rate, heard_at| {
                assert!(channels > 0 && rate > 0 && out.len() % channels == 0);
                assert!(heard_at + Duration::from_millis(1) >= Instant::now());
                out.fill(0.0);
                counting.fetch_add(1, Ordering::Relaxed);
            },
            move || failing.store(true, Ordering::Relaxed),
            receiver,
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while calls.load(Ordering::Relaxed) == 0 && !failed.load(Ordering::Relaxed) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(sender);
        if failed.load(Ordering::Relaxed) {
            eprintln!("there's no sound output here, so playing through one wasn't tried");
            return;
        }
        assert!(calls.load(Ordering::Relaxed) > 0, "the device never asked for sound");
    }

    #[test]
    fn sound_files_decode_from_any_point() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ramp.wav");
        let rate = 8000_u32;
        let samples: Vec<i16> = (0..rate * 2).map(|index| (index % 8000) as i16).collect();
        let mut wav = Vec::new();
        let data = (samples.len() * 2) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data.to_le_bytes());
        for sample in &samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();

        let mut sound = SoundDecoder::open(&VideoSource::File(path.clone())).unwrap().unwrap();
        let first = sound.next_chunk().unwrap().unwrap();
        assert_eq!((first.start, first.rate), (0.0, rate));
        assert_eq!(first.samples[0], first.samples[1], "mono spread to both channels");
        sound.seek(1.25).unwrap();
        let chunk = sound.next_chunk().unwrap().unwrap();
        assert!((chunk.start - 1.25).abs() < 1e-3, "{}", chunk.start);
        let expected = 2000.0 / 32768.0;
        assert!((chunk.samples[0] - expected).abs() < 2.0 / 32768.0, "{} vs {expected}", chunk.samples[0]);
        let mut total = chunk.samples.len() / 2;
        while let Some(chunk) = sound.next_chunk().unwrap() {
            total += chunk.samples.len() / 2;
        }
        assert_eq!(total, 6000, "the last three quarters of a second");
    }
}
