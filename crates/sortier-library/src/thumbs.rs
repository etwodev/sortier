use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use image::{DynamicImage, ImageReader, Rgb, RgbImage, RgbaImage, codecs::jpeg::JpegEncoder};

use crate::library::{ItemId, MediaKind, data_dir};

/// Longest side of a cached thumbnail, in pixels.
pub const THUMB_PX: u32 = 400;
/// Longest side of a cover the user chose: twice a thumbnail's, so large tiles stay sharp.
pub const COVER_PX: u32 = 800;

/// One thumbnail to make (or find already made).
pub struct Job {
    pub id: ItemId,
    pub src: PathBuf,
    pub kind: MediaKind,
    pub dest: PathBuf,
    /// The item's pixel size (or a document's page count) isn't known yet; read it while we're here.
    pub need_dims: bool,
}

pub struct JobResult {
    pub id: ItemId,
    pub dest: PathBuf,
    pub ok: bool,
    pub dims: Option<(u32, u32)>,
    /// A document's page count.
    pub pages: Option<u32>,
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Where the thumbnail of `src` (as it was at `modified`) is cached. A changed
/// file gets a new name, so stale thumbnails are never shown.
pub fn thumb_path(src: &Path, modified: u64) -> PathBuf {
    let key = format!("{}|{}|{}", src.to_string_lossy(), modified, THUMB_PX);
    let hash = fnv1a(key.as_bytes());
    data_dir()
        .join("thumbs")
        .join(format!("{:02x}", hash & 0xff))
        .join(format!("{hash:016x}.jpg"))
}

/// Makes a batch of thumbnails on all but one core. Blocking; run it off the UI thread.
pub fn run_jobs(jobs: Vec<Job>) -> Vec<JobResult> {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).max(1))
        .unwrap_or(2);
    let chunk = jobs.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .chunks(chunk)
            .map(|chunk| scope.spawn(move || chunk.iter().map(run_job).collect::<Vec<_>>()))
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().unwrap_or_default()).collect()
    })
}

fn run_job(job: &Job) -> JobResult {
    let mut dims = None;
    let pages = (job.kind == MediaKind::Document).then(|| sortier_media::document_page_count(&job.src).ok()).flatten();
    let ok = if job.dest.exists() {
        if job.need_dims {
            dims = item_dims(&job.src, job.kind);
        }
        true
    } else {
        match generate(job) {
            Ok(found) => {
                dims = found;
                true
            }
            Err(_) => false,
        }
    };
    JobResult {
        id: job.id,
        dest: job.dest.clone(),
        ok,
        dims,
        pages,
    }
}

fn item_dims(src: &Path, kind: MediaKind) -> Option<(u32, u32)> {
    match kind {
        MediaKind::Video => sortier_media::probe_video(src).ok().map(|info| (info.width, info.height)),
        MediaKind::Document => sortier_media::document_page_size(src, 0).ok(),
        _ => sortier_media::image_size(src).ok(),
    }
}

fn generate(job: &Job) -> Result<Option<(u32, u32)>> {
    if let Some(dir) = job.dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let (thumb, dims) = if job.kind == MediaKind::Video {
        let image = picture_image(sortier_media::video_thumbnail(&job.src, THUMB_PX)?)?;
        let dims = sortier_media::probe_video(&job.src).ok().map(|info| (info.width, info.height));
        (image, dims)
    } else if job.kind == MediaKind::Document {
        let image = picture_image(sortier_media::document_page(&job.src, 0, Some(THUMB_PX))?)?;
        (image, sortier_media::document_page_size(&job.src, 0).ok())
    } else {
        let image = ImageReader::open(&job.src)?.with_guessed_format()?.decode()?;
        let dims = (image.width(), image.height());
        (image.thumbnail(THUMB_PX, THUMB_PX), Some(dims))
    };
    write_jpeg(thumb, &job.dest)?;
    Ok(dims)
}

/// Writes `image` as a JPEG at `dest`, whole or not at all. Several threads may make the same thumbnail at once
/// (the server's scan, and a device asking for it), so each writes a file of its own first.
fn write_jpeg(image: DynamicImage, dest: &Path) -> Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let tmp = dest.with_extension(format!("{}-{}.tmp", std::process::id(), WRITES.fetch_add(1, Ordering::Relaxed)));
    let written = encode_jpeg(image, &tmp).and_then(|()| Ok(std::fs::rename(&tmp, dest)?));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

fn encode_jpeg(image: DynamicImage, path: &Path) -> Result<()> {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    JpegEncoder::new_with_quality(&mut file, 85).encode_image(&flatten(image))?;
    file.flush()?;
    Ok(())
}

/// Makes a cover from page `index` of the file at `src` (a picture, a page of a document, or a frame of a video),
/// shrunk to [`COVER_PX`], and saves it in Sortier's covers folder, returning where it went. The file itself is
/// never changed. Blocking; run it off the UI thread.
pub fn save_cover(src: &Path, kind: MediaKind, index: u32) -> Result<PathBuf> {
    let image = match kind {
        MediaKind::Video => picture_image(sortier_media::video_thumbnail(src, COVER_PX)?)?,
        MediaKind::Document => picture_image(sortier_media::document_page(src, index, Some(COVER_PX))?)?,
        MediaKind::Image | MediaKind::Animated => {
            ImageReader::open(src)?.with_guessed_format()?.decode()?.thumbnail(COVER_PX, COVER_PX)
        }
    };
    let made = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let key = format!("{}|{index}|{made}", src.to_string_lossy());
    let dir = data_dir().join("covers");
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(format!("{:016x}.jpg", fnv1a(key.as_bytes())));
    write_jpeg(image, &dest)?;
    Ok(dest)
}

fn picture_image(picture: sortier_media::Picture) -> Result<DynamicImage> {
    let image = RgbaImage::from_raw(picture.width, picture.height, picture.rgba)
        .ok_or_else(|| anyhow::anyhow!("a picture had the wrong size"))?;
    Ok(DynamicImage::ImageRgba8(image))
}

/// Composites transparency onto a neutral dark grey; thumbnails are JPEGs.
fn flatten(image: DynamicImage) -> RgbImage {
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    let background = [0x2au16, 0x2a, 0x2d];
    RgbImage::from_fn(width, height, |x, y| {
        let pixel = rgba.get_pixel(x, y).0;
        let alpha = pixel[3] as u16;
        let mix = |channel: u8, under: u16| ((channel as u16 * alpha + under * (255 - alpha)) / 255) as u8;
        Rgb([mix(pixel[0], background[0]), mix(pixel[1], background[1]), mix(pixel[2], background[2])])
    })
}
