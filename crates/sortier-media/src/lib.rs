//! Decodes media for Sortier into straight RGBA8 pixels, and plays videos with
//! their sound. Still images and animations go through `image` (and `png` for
//! animated PNGs); documents (PDF, EPUB, comic archives) through hayro and the
//! archive readers; videos and their sound through FFmpeg's libraries, built into
//! Sortier, decoding pictures on the graphics card where it can
//! ([`detect_hardware_decoder`]). Nothing here knows about GPUI or wgpu: callers
//! upload the pixels themselves.

mod animation;
mod document;
mod hardware;
mod natural;
mod player;
mod sound;
mod still;
mod video;

use std::time::Duration;

pub use animation::{Animation, AnimationFrame, decode_animation};
pub use document::{DOCUMENT_EXTENSIONS, document_page, document_page_count, document_page_size};
pub use hardware::{HardwareDecoder, detect_hardware_decoder};
pub use natural::natural_cmp;
pub use player::{VideoPlayer, probe_video, video_thumbnail};
pub use still::{decode_still, image_size};
pub use video::{OpenStream, ReadSeek, VideoSource};

/// Straight (non-premultiplied) RGBA8, sRGB, tightly packed: `width * height * 4` bytes, rows top to bottom.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Picture({}\u{d7}{})", self.width, self.height)
    }
}

/// What a video is: its size as displayed (after any rotation the file asks for), its length, its average frame
/// rate, and whether it has sound ([`VideoPlayer::info`]: sound it will play, when there's an output to play it).
#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration: Duration,
    pub frame_rate: f64,
    pub has_sound: bool,
}

/// One decoded video frame and the moment of the video it's shown from.
#[derive(Debug)]
pub struct VideoFrame {
    pub picture: Picture,
    pub time: Duration,
}

/// Why media couldn't be read or played.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("the file can't be decoded: {0}")]
    Decode(String),
}

impl From<image::ImageError> for MediaError {
    fn from(error: image::ImageError) -> Self {
        match error {
            image::ImageError::IoError(error) => MediaError::Io(error),
            image::ImageError::Unsupported(error) => MediaError::Unsupported(error.to_string()),
            other => MediaError::Decode(other.to_string()),
        }
    }
}

fn seconds(duration: Duration) -> f64 {
    duration.as_secs_f64()
}

fn duration(seconds: f64) -> Duration {
    Duration::from_secs_f64(seconds.max(0.0))
}

/// `width` x `height` shrunk (never enlarged), keeping its shape, so its longer side is at most `max_side`.
fn fit_size(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let longer = width.max(height);
    if longer <= max_side.max(1) {
        return (width, height);
    }
    let scale = f64::from(max_side.max(1)) / f64::from(longer);
    let fit = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    (fit(width), fit(height))
}
