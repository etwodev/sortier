use std::fs::File;
use std::io::{BufReader, Read as _};
use std::path::Path;
use std::time::Duration;

use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, Frames, ImageFormat};

use crate::{MediaError, Picture};

/// One frame of an animation: the whole canvas as it looks while the frame shows, and for how long.
pub struct AnimationFrame {
    pub picture: Picture,
    pub delay: Duration,
}

/// Every frame of an animated GIF, PNG or WebP, decoded up front. Holds `width * height * 4` bytes per frame, so a
/// long animation can take a lot of memory.
pub struct Animation {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<AnimationFrame>,
}

/// Frames that ask to show for no time at all show this long, as browsers do.
const DEFAULT_DELAY: Duration = Duration::from_millis(100);

/// Decodes every frame of an animated GIF, PNG (APNG) or WebP, recognised by its contents. `Ok(None)` when the file
/// has a single frame, or is a kind of image that can't be animated (JPEG, BMP, TIFF): show it with
/// [`crate::decode_still`] instead.
pub fn decode_animation(path: &Path) -> Result<Option<Animation>, MediaError> {
    let mut head = [0_u8; 32];
    let read = File::open(path)?.read(&mut head)?;
    let Ok(format) = image::guess_format(&head[..read]) else {
        return Ok(None);
    };
    let reader = || -> Result<BufReader<File>, MediaError> { Ok(BufReader::new(File::open(path)?)) };
    let frames: Frames<'_> = match format {
        ImageFormat::Gif => GifDecoder::new(reader()?)?.into_frames(),
        ImageFormat::Png => {
            let decoder = PngDecoder::new(reader()?)?;
            if !decoder.is_apng()? {
                return Ok(None);
            }
            decoder.apng()?.into_frames()
        }
        ImageFormat::WebP => {
            let decoder = WebPDecoder::new(reader()?)?;
            if !decoder.has_animation() {
                return Ok(None);
            }
            decoder.into_frames()
        }
        _ => return Ok(None),
    };
    let mut decoded = Vec::new();
    for frame in frames {
        let frame = frame?;
        let delay = delay_of(&frame, format);
        let buffer = frame.into_buffer();
        decoded.push(AnimationFrame {
            picture: Picture {
                width: buffer.width(),
                height: buffer.height(),
                rgba: buffer.into_raw(),
            },
            delay,
        });
    }
    if decoded.len() < 2 {
        return Ok(None);
    }
    let (width, height) = (decoded[0].picture.width, decoded[0].picture.height);
    Ok(Some(Animation {
        width,
        height,
        frames: decoded,
    }))
}

/// How long `frame` shows. GIFs asking for 10 ms or less get [`DEFAULT_DELAY`], as browsers give them, since
/// many GIFs were made for that.
fn delay_of(frame: &image::Frame, format: ImageFormat) -> Duration {
    let (numerator, denominator) = frame.delay().numer_denom_ms();
    let milliseconds = f64::from(numerator) / f64::from(denominator.max(1));
    let too_short = if format == ImageFormat::Gif { milliseconds <= 10.0 } else { milliseconds <= 0.0 };
    if too_short { DEFAULT_DELAY } else { Duration::from_secs_f64(milliseconds / 1000.0) }
}
