use std::io::{BufRead, Seek};
use std::path::Path;

use image::{DynamicImage, ImageDecoder, ImageReader};

use crate::{MediaError, Picture};

/// Decodes a still image (PNG, JPEG, BMP, TIFF, WebP, or the first frame of a GIF or other animation), recognised
/// by its contents rather than its name, turned upright as its EXIF orientation says.
pub fn decode_still(path: &Path) -> Result<Picture, MediaError> {
    let rgba = decode_image(ImageReader::open(path)?)?.into_rgba8();
    Ok(Picture {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// The image `reader` reads, recognised by its contents, turned upright as its EXIF orientation says.
pub(crate) fn decode_image<R: BufRead + Seek>(reader: ImageReader<R>) -> Result<DynamicImage, MediaError> {
    let mut decoder = reader.with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image)
}

/// An image's size as [`decode_still`] would give it (after EXIF rotation), read from its header only.
pub fn image_size(path: &Path) -> Result<(u32, u32), MediaError> {
    size_of(ImageReader::open(path)?)
}

/// The size of the image `reader` reads, after EXIF rotation, from its header only.
pub(crate) fn size_of<R: BufRead + Seek>(reader: ImageReader<R>) -> Result<(u32, u32), MediaError> {
    let mut decoder = reader.with_guessed_format()?.into_decoder()?;
    let (width, height) = decoder.dimensions();
    let turned = matches!(
        decoder.orientation()?,
        image::metadata::Orientation::Rotate90
            | image::metadata::Orientation::Rotate270
            | image::metadata::Orientation::Rotate90FlipH
            | image::metadata::Orientation::Rotate270FlipH
    );
    Ok(if turned { (height, width) } else { (width, height) })
}
