//! Probing and playing videos. A player decodes pictures a little ahead on one thread and sound on another, and
//! picks the picture to show by its clock: the sound output's while sound plays, the wall clock otherwise.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::sound::{SoundDecoder, SoundQueue, start_output};
use crate::video::{VideoDecoder, VideoSource};
use crate::{HardwareDecoder, MediaError, Picture, VideoFrame, VideoInfo};

/// Pictures are decoded at most this far ahead of what's showing...
const READ_AHEAD: f64 = 0.5;
/// ...and at most this many of them...
const MAX_QUEUED_FRAMES: usize = 12;
/// ...taking at most about this much memory (a 4K picture is 33 MB), though always at least two.
const QUEUE_BYTES: usize = 192 << 20;
/// Seconds of sound decoded ahead of what's playing.
const SOUND_AHEAD: f64 = 1.0;
/// How often an idle decoding thread looks again for something to do.
const POLL: Duration = Duration::from_millis(15);
/// A limit the sound output set on the clock is ignored once it hasn't been renewed for this long, so pictures
/// don't freeze when the device stops asking for sound.
const STALE: Duration = Duration::from_millis(500);

/// Reads a video's size, length, frame rate and whether it has sound, without decoding any of it.
pub fn probe_video(path: &Path) -> Result<VideoInfo, MediaError> {
    crate::video::probe(path)
}

/// One representative frame of a video, a little past its start, shrunk so its longer side is at most
/// `max_side`. Blocks while decoding.
pub fn video_thumbnail(path: &Path, max_side: u32) -> Result<Picture, MediaError> {
    crate::video::thumbnail(path, max_side)
}

/// The player's clock: `base` seconds into the video at `since`, moving on in real time while playing. While sound
/// plays, the output sets it with every buffer, and limits it to the end of what it has played, so pictures wait
/// when sound is late.
struct Clock {
    base: f64,
    since: Instant,
    /// The limit, and when it was last renewed.
    limit: Option<(f64, Instant)>,
}

/// What the UI thread, the decoding threads and the sound output share about playing, under one lock.
struct State {
    /// Bumped by every seek (and loop), so threads know to start again from `start`.
    generation: u64,
    start: f64,
    playing: bool,
    looping: bool,
    ended: bool,
    volume: f32,
    muted: bool,
    clock: Clock,
    sound: SoundQueue,
}

impl State {
    fn position(&self, now: Instant, duration: f64) -> f64 {
        let mut position = self.clock.base;
        if self.playing {
            position += if now >= self.clock.since {
                (now - self.clock.since).as_secs_f64()
            } else {
                -(self.clock.since - now).as_secs_f64()
            };
            if let Some((limit, renewed)) = self.clock.limit
                && now.saturating_duration_since(renewed) < STALE
            {
                position = position.min(limit);
            }
        }
        position.clamp(0.0, duration)
    }
}

/// Decoded pictures waiting to show, and the one showing.
struct Frames {
    generation: u64,
    queue: VecDeque<Arc<VideoFrame>>,
    current: Option<Arc<VideoFrame>>,
    /// The decoder has given every frame there is from where it last started.
    done: bool,
    /// `current` is a picture from where the player last started (opened or sought), not one left from before.
    presented: bool,
    capacity: usize,
}

struct Shared {
    duration: f64,
    state: Mutex<State>,
    /// Signalled when sound is played from the queue, or the player seeks or closes.
    sound_wanted: Condvar,
    frames: Mutex<Frames>,
    /// Signalled when a picture is taken from the queue, or the player seeks or closes.
    frames_wanted: Condvar,
    serial: AtomicU64,
    stop: AtomicBool,
    error: Mutex<Option<String>>,
    /// The graphics card decoder the pictures last came from, if any.
    hardware: Mutex<Option<HardwareDecoder>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Shared {
    /// A paused player's state at the start of a video described by `info`, showing `first` if it's decoded.
    fn new(info: &VideoInfo, first: Option<VideoFrame>) -> Self {
        let frame_bytes = (info.width as usize * info.height as usize * 4).max(1);
        Self {
            duration: crate::seconds(info.duration),
            state: Mutex::new(State {
                generation: 0,
                start: 0.0,
                playing: false,
                looping: false,
                ended: false,
                volume: 1.0,
                muted: false,
                clock: Clock {
                    base: 0.0,
                    since: Instant::now(),
                    limit: None,
                },
                sound: SoundQueue::default(),
            }),
            sound_wanted: Condvar::new(),
            frames: Mutex::new(Frames {
                generation: 0,
                queue: VecDeque::new(),
                presented: first.is_some(),
                current: first.map(Arc::new),
                done: false,
                capacity: (QUEUE_BYTES / frame_bytes).clamp(2, MAX_QUEUED_FRAMES),
            }),
            frames_wanted: Condvar::new(),
            serial: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            error: Mutex::new(None),
            hardware: Mutex::new(None),
        }
    }

    /// Starts playing again from `at`: both threads start over there.
    fn restart(&self, state: &mut State, at: f64, now: Instant) {
        state.generation += 1;
        state.start = at;
        state.clock = Clock {
            base: at,
            since: now,
            limit: None,
        };
        state.sound.reset(at);
        state.ended = false;
        let mut frames = lock(&self.frames);
        frames.generation = state.generation;
        frames.queue.clear();
        frames.done = false;
        frames.presented = false;
        drop(frames);
        self.frames_wanted.notify_all();
        self.sound_wanted.notify_all();
    }

    /// Notices the end of the video (looping or stopping there), then moves on to the newest decoded picture that's
    /// due. Returns the position.
    fn advance(&self) -> f64 {
        let now = Instant::now();
        let mut state = lock(&self.state);
        let mut position = state.position(now, self.duration);
        if state.playing && position >= self.duration {
            if state.looping {
                self.restart(&mut state, 0.0, now);
                position = 0.0;
            } else {
                state.playing = false;
                state.ended = true;
                state.clock.base = self.duration;
                state.clock.limit = None;
            }
        }
        let generation = state.generation;
        drop(state);
        let mut frames = lock(&self.frames);
        if frames.generation != generation {
            return position;
        }
        let mut changed = false;
        while frames.queue.front().is_some_and(|frame| crate::seconds(frame.time) <= position) {
            frames.current = frames.queue.pop_front();
            changed = true;
        }
        if changed {
            frames.presented = true;
            self.serial.fetch_add(1, Ordering::AcqRel);
            self.frames_wanted.notify_all();
        }
        position
    }

    /// Keeps the first error a decoding thread met, for [`VideoPlayer::error`].
    fn fail(&self, error: MediaError) {
        lock(&self.error).get_or_insert_with(|| error.to_string());
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

/// Decodes pictures ahead of the clock into the queue until the player closes.
fn decode_video(shared: Arc<Shared>, mut source: VideoDecoder, mut generation: Option<u64>) {
    while !shared.stopped() {
        let (wanted, start, position) = {
            let state = lock(&shared.state);
            (state.generation, state.start, state.position(Instant::now(), shared.duration))
        };
        if generation != Some(wanted) {
            generation = Some(wanted);
            if let Err(error) = source.seek(start) {
                finish_video(&shared, wanted, Some(error));
            }
            continue;
        }
        {
            let frames = lock(&shared.frames);
            let full = frames.done
                || frames.generation != wanted
                || frames.queue.len() >= frames.capacity
                || frames.queue.back().is_some_and(|last| crate::seconds(last.time) - position > READ_AHEAD);
            if full {
                // Woken early when the UI takes a picture or the player seeks.
                let _ = shared.frames_wanted.wait_timeout(frames, POLL);
                continue;
            }
        }
        match source.next_frame(position) {
            Ok(Some((time, picture))) => {
                *lock(&shared.hardware) = source.hardware_in_use();
                let mut frames = lock(&shared.frames);
                if frames.generation == wanted {
                    frames.queue.push_back(Arc::new(VideoFrame {
                        picture,
                        time: crate::duration(time),
                    }));
                }
            }
            Ok(None) => finish_video(&shared, wanted, None),
            Err(error) => finish_video(&shared, wanted, Some(error)),
        }
    }
}

/// Marks the pictures from generation `generation` as all decoded, keeping `error` if it's why.
fn finish_video(shared: &Shared, generation: u64, error: Option<MediaError>) {
    let mut frames = lock(&shared.frames);
    if frames.generation != generation {
        return;
    }
    frames.done = true;
    if let Some(error) = error {
        shared.fail(error);
    }
}

/// Decodes sound ahead of the output into its queue until the player closes.
fn decode_sound(shared: Arc<Shared>, mut source: SoundDecoder) {
    let mut generation = None;
    while !shared.stopped() {
        let state = lock(&shared.state);
        if generation != Some(state.generation) {
            let (wanted, start) = (state.generation, state.start);
            drop(state);
            generation = Some(wanted);
            if let Err(error) = source.seek(start) {
                finish_sound(&shared, wanted, Some(error));
            }
            continue;
        }
        if state.sound.finished || state.sound.ahead() > SOUND_AHEAD {
            // Woken early when the output plays from the queue or the player seeks.
            let _ = shared.sound_wanted.wait_timeout(state, POLL);
            continue;
        }
        drop(state);
        let wanted = generation.expect("set above");
        match source.next_chunk() {
            Ok(Some(chunk)) => {
                let mut state = lock(&shared.state);
                if state.generation == wanted {
                    state.sound.push(chunk);
                }
            }
            Ok(None) => finish_sound(&shared, wanted, None),
            Err(error) => finish_sound(&shared, wanted, Some(error)),
        }
    }
}

/// Marks the sound from generation `generation` as all decoded, so the clock stops waiting for it.
fn finish_sound(shared: &Shared, generation: u64, error: Option<MediaError>) {
    let mut state = lock(&shared.state);
    if state.generation != generation {
        return;
    }
    state.sound.finished = true;
    drop(state);
    if let Some(error) = error {
        shared.fail(error);
    }
}

/// Fills one output buffer from the sound queue, and sets the clock from what it played.
fn play_sound(shared: &Shared, out: &mut [f32], channels: usize, rate: u32, heard_at: Instant) {
    let mut state = lock(&shared.state);
    if !state.playing || shared.stopped() {
        out.fill(0.0);
        return;
    }
    let gain = if state.muted { 0.0 } else { state.volume };
    let filled = state.sound.fill(out, channels, rate, gain);
    let now = Instant::now();
    if filled.played {
        state.clock = Clock {
            base: filled.start,
            since: heard_at,
            limit: Some((filled.end, now)),
        };
    } else if filled.starved {
        state.clock = Clock {
            base: filled.start,
            since: now,
            limit: Some((filled.start, now)),
        };
    } else {
        // The sound has ended before the pictures; they go on by the wall clock.
        state.clock.limit = None;
    }
    drop(state);
    shared.sound_wanted.notify_all();
}

/// Plays a video and its sound. Opening probes the file and decodes its first picture; the rest of the pictures
/// and the sound decode on background threads (two, plus one for the sound output), so every other method returns
/// at once and the player can live on the UI thread.
///
/// Everything FFmpeg reads plays: MPEG-4, QuickTime, WebM, Matroska, AVI and more, with H.264, HEVC, VP8, VP9, AV1
/// (where the graphics card decodes it), ProRes, MPEG-4 Part 2 and other codecs, and their sound mixed to stereo.
///
/// Call [`VideoPlayer::current_frame`] (or [`VideoPlayer::frame_serial`] to see whether it changed) on every UI
/// frame while the video is on screen: that's also when the player notices it has reached the end.
pub struct VideoPlayer {
    shared: Arc<Shared>,
    info: VideoInfo,
    /// Dropping it closes the sound output.
    _output: Option<Sender<()>>,
}

impl VideoPlayer {
    /// Probes `path` and decodes its first picture, so it shows at once, then carries on decoding from there,
    /// paused. Pictures are decoded with `hardware` when it can take the video's codec, otherwise on the processor.
    /// The sound plays if Sortier can decode it; if it can't, the video plays silently and [`VideoPlayer::error`]
    /// says why.
    pub fn open(path: &Path, hardware: Option<HardwareDecoder>) -> Result<Self, MediaError> {
        Self::open_source(&VideoSource::File(path.to_path_buf()), hardware)
    }

    /// [`VideoPlayer::open`] for a video read from `source`, such as a file on a Sortier Remote server.
    pub fn open_source(source: &VideoSource, hardware: Option<HardwareDecoder>) -> Result<Self, MediaError> {
        let mut video = VideoDecoder::open(source, hardware, crate::video::DecodeThreads::Auto)?;
        let first = video
            .next_frame(0.0)?
            .map(|(time, picture)| VideoFrame {
                picture,
                time: crate::duration(time),
            })
            .ok_or_else(|| MediaError::Decode("the video has no pictures".into()))?;
        let (sound, sound_error) = match SoundDecoder::open(source) {
            Ok(sound) => (sound, None),
            Err(error) => (None, Some(error)),
        };
        let mut info = video.info.clone();
        info.has_sound = sound.is_some();
        let shared = Arc::new(Shared::new(&info, Some(first)));
        if let Some(error) = sound_error {
            shared.fail(error);
        }
        *lock(&shared.hardware) = video.hardware_in_use();
        let decoding = shared.clone();
        std::thread::Builder::new()
            .name("sortier-video-decode".into())
            .spawn(move || decode_video(decoding, video, Some(0)))?;
        let mut output = None;
        if let Some(sound) = sound {
            let decoding = shared.clone();
            let spawned = std::thread::Builder::new()
                .name("sortier-sound-decode".into())
                .spawn(move || decode_sound(decoding, sound));
            if let Err(error) = spawned {
                shared.stop.store(true, Ordering::Release);
                return Err(error.into());
            }
            let (sender, receiver) = std::sync::mpsc::channel();
            let playing = shared.clone();
            let failing = shared.clone();
            start_output(
                move |out, channels, rate, heard_at| play_sound(&playing, out, channels, rate, heard_at),
                move || lock(&failing.state).clock.limit = None,
                receiver,
            );
            output = Some(sender);
        }
        Ok(Self {
            shared,
            info,
            _output: output,
        })
    }

    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// Plays from where the video is, or from the start once it has ended.
    pub fn play(&self) {
        let now = Instant::now();
        let mut state = lock(&self.shared.state);
        if state.ended {
            self.shared.restart(&mut state, 0.0, now);
        }
        if !state.playing {
            state.playing = true;
            state.clock.since = now;
            state.clock.limit = None;
        }
    }

    pub fn pause(&self) {
        let now = Instant::now();
        let mut state = lock(&self.shared.state);
        if state.playing {
            state.clock.base = state.position(now, self.shared.duration);
            state.clock.limit = None;
            state.playing = false;
        }
    }

    pub fn is_playing(&self) -> bool {
        self.shared.advance();
        lock(&self.shared.state).playing
    }

    /// Goes to `to` (clamped to the video's length), playing on from there if it was playing. Returns at once; the
    /// picture at `to` shows once it's decoded, paused or not, and until then the previous one stays.
    pub fn seek(&self, to: Duration) {
        let to = crate::seconds(to).min(self.shared.duration);
        let mut state = lock(&self.shared.state);
        self.shared.restart(&mut state, to, Instant::now());
    }

    /// How far into the video playing has got: the sound output's clock while sound plays, otherwise the wall
    /// clock's.
    pub fn position(&self) -> Duration {
        crate::duration(self.shared.advance())
    }

    /// Sets the sound's volume, where 1 is as recorded (up to 2).
    pub fn set_volume(&self, volume: f32) {
        lock(&self.shared.state).volume = if volume.is_finite() { volume.clamp(0.0, 2.0) } else { 1.0 };
    }

    pub fn set_muted(&self, muted: bool) {
        lock(&self.shared.state).muted = muted;
    }

    /// Whether reaching the end starts the video (and its sound) again from the start.
    pub fn set_looping(&self, looping: bool) {
        lock(&self.shared.state).looping = looping;
    }

    /// Whether playing reached the end (without looping) and stopped there. [`VideoPlayer::play`] starts again
    /// from the start; seeking clears it.
    pub fn ended(&self) -> bool {
        self.shared.advance();
        lock(&self.shared.state).ended
    }

    /// The picture to show now: the newest decoded picture whose time has come. `None` only until the first
    /// picture is decoded. Cheap enough to call on every UI frame.
    pub fn current_frame(&self) -> Option<Arc<VideoFrame>> {
        self.shared.advance();
        lock(&self.shared.frames).current.clone()
    }

    /// Goes up by one whenever [`VideoPlayer::current_frame`] would give a different picture, so the UI can skip
    /// uploading the same one again. Poll it on every UI frame while playing (and after seeking while paused).
    pub fn frame_serial(&self) -> u64 {
        self.shared.advance();
        self.shared.serial.load(Ordering::Acquire)
    }

    /// Whether the picture for where the player is (after opening or seeking) is still being decoded. Paused
    /// players show nothing new on their own, so keep polling [`VideoPlayer::frame_serial`] until this is false.
    pub fn is_waiting(&self) -> bool {
        self.shared.advance();
        let frames = lock(&self.shared.frames);
        !frames.presented && !frames.done
    }

    /// The graphics card decoder the pictures are coming from, or `None` when they're decoded on the processor:
    /// with no hardware decoder given, or one that can't take this video's codec.
    pub fn hardware_decoder(&self) -> Option<HardwareDecoder> {
        *lock(&self.shared.hardware)
    }

    /// Why decoding stopped early, or the sound couldn't be played, if either happened (a damaged file, say).
    /// Playing carries on with what was decoded.
    pub fn error(&self) -> Option<String> {
        lock(&self.shared.error).clone()
    }
}

impl Drop for VideoPlayer {
    /// Stops the decoding threads (they finish within a frame's decode) and closes the sound output.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        lock(&self.shared.state).playing = false;
        self.shared.frames_wanted.notify_all();
        self.shared.sound_wanted.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sound::Chunk;

    fn sounding(seconds: f64) -> Chunk {
        Chunk {
            start: 0.0,
            rate: 1000,
            samples: vec![0.25; (seconds * 1000.0) as usize * 2],
        }
    }

    #[test]
    fn sound_drives_the_clock_and_pictures_wait_for_late_sound() {
        let info = VideoInfo {
            width: 2,
            height: 2,
            duration: Duration::from_secs(10),
            frame_rate: 10.0,
            has_sound: true,
        };
        let shared = Shared::new(&info, None);
        let position = |at: Instant| lock(&shared.state).position(at, 10.0);
        let mut out = vec![1.0_f32; 2 * 100];
        play_sound(&shared, &mut out, 2, 1000, Instant::now());
        assert!(out.iter().all(|sample| *sample == 0.0), "silent while paused");

        {
            let mut state = lock(&shared.state);
            state.playing = true;
            state.sound.push(sounding(0.15));
        }
        let heard_at = Instant::now() + Duration::from_millis(50);
        play_sound(&shared, &mut out, 2, 1000, heard_at);
        assert!(out.iter().all(|sample| (*sample - 0.25).abs() < 1e-6));
        assert!(position(heard_at).abs() < 1e-9, "the buffer starts at 0 when it's heard");
        assert!((position(heard_at + Duration::from_millis(40)) - 0.04).abs() < 1e-6);
        assert!((position(heard_at + Duration::from_millis(300)) - 0.1).abs() < 1e-6, "no further than was played");
        assert!(position(heard_at + Duration::from_secs(1)) > 0.5, "unless the output has gone quiet");

        lock(&shared.state).muted = true;
        let heard_at = Instant::now();
        play_sound(&shared, &mut out, 2, 1000, heard_at);
        assert!(out[..100].iter().all(|sample| *sample == 0.0), "muted, but still moving on");
        // The last sample is held back to resample against the next until the sound is known to end.
        let near = |seconds: f64, want: f64| (seconds - want).abs() <= 0.0011;
        assert!(near(position(heard_at + Duration::from_millis(300)), 0.15), "half a buffer was left");

        play_sound(&shared, &mut out, 2, 1000, Instant::now());
        assert!(near(position(Instant::now() + Duration::from_millis(200)), 0.15), "waiting for sound");

        lock(&shared.state).sound.finished = true;
        play_sound(&shared, &mut out, 2, 1000, Instant::now());
        play_sound(&shared, &mut out, 2, 1000, Instant::now());
        assert!(position(Instant::now() + Duration::from_millis(200)) > 0.3, "the wall clock once the sound has ended");
    }
}
