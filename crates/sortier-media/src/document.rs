//! Paged documents: PDFs (drawn by hayro, in pure Rust), comic archives (CBZ and CB7: pictures in a ZIP or 7z
//! archive, in name order) and EPUBs made of page pictures (most manga and comic EPUBs).

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufReader, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use image::ImageReader;
use zip::ZipArchive;

use crate::still::{decode_image, size_of};
use crate::{MediaError, Picture, natural_cmp};

/// The file name extensions of the documents Sortier reads, in lower case.
pub const DOCUMENT_EXTENSIONS: &[&str] = &["pdf", "epub", "cbz", "cb7"];

/// PDF pages are drawn at twice their size in points (144 dots per inch), so text stays sharp zoomed in a little...
const PDF_SCALE: f32 = 2.0;
/// ...but never longer than this on either side.
const PDF_MAX_SIDE: f32 = 4096.0;
/// How many documents are kept open, so turning pages doesn't read the whole file again.
const KEPT_OPEN: usize = 4;
/// The pictures comic archives and EPUBs hold as pages.
const PICTURE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "jfif", "webp", "gif", "bmp", "tif", "tiff"];

/// What's been read of a document: enough to find any page quickly.
enum Contents {
    /// The whole file, which hayro reads pages from.
    Pdf { data: Arc<Vec<u8>>, pages: u32 },
    /// The names of the page pictures in a ZIP archive (a CBZ, or an EPUB), in order.
    Zip { names: Vec<String> },
    /// Every page picture, unpacked. 7z archives are usually compressed as one block, so a page can't be read
    /// without unpacking the ones before it.
    Unpacked { pages: Vec<Arc<Vec<u8>>> },
}

impl Contents {
    fn page_count(&self) -> u32 {
        match self {
            Contents::Pdf { pages, .. } => *pages,
            Contents::Zip { names } => names.len() as u32,
            Contents::Unpacked { pages } => pages.len() as u32,
        }
    }
}

/// An open document, and the file's modification time and length when it was read.
struct Opened {
    path: PathBuf,
    stamp: (SystemTime, u64),
    contents: Arc<Contents>,
}

static OPENED: Mutex<VecDeque<Opened>> = Mutex::new(VecDeque::new());

fn opened() -> MutexGuard<'static, VecDeque<Opened>> {
    OPENED.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How many pages the document at `path` has.
pub fn document_page_count(path: &Path) -> Result<u32, MediaError> {
    Ok(contents(path)?.page_count())
}

/// Page `index` (from 0) of the document at `path`, shrunk so its longer side is at most `max_side` when one is
/// given. PDF pages are drawn at 144 dots per inch (at most 4096 pixels long) on white. Blocks while decoding.
pub fn document_page(path: &Path, index: u32, max_side: Option<u32>) -> Result<Picture, MediaError> {
    let contents = contents(path)?;
    let missing = || MediaError::Decode(format!("page {} doesn't exist", index + 1));
    match &*contents {
        Contents::Pdf { data, .. } => pdf_page(data, index, max_side),
        Contents::Zip { names } => {
            let name = names.get(index as usize).ok_or_else(missing)?;
            let mut archive = open_zip(path)?;
            picture_from(&read_entry(&mut archive, name)?, max_side)
        }
        Contents::Unpacked { pages } => picture_from(pages.get(index as usize).ok_or_else(missing)?, max_side),
    }
}

/// The size page `index` of the document at `path` is shown at by [`document_page`] with no `max_side`, read
/// without drawing or decoding it.
pub fn document_page_size(path: &Path, index: u32) -> Result<(u32, u32), MediaError> {
    let contents = contents(path)?;
    let missing = || MediaError::Decode(format!("page {} doesn't exist", index + 1));
    match &*contents {
        Contents::Pdf { data, .. } => {
            let pdf = Pdf::new(data.clone()).map_err(pdf_error)?;
            let (width, height) = pdf.pages().get(index as usize).ok_or_else(missing)?.render_dimensions();
            let scale = pdf_scale(width, height, None);
            Ok(((width * scale) as u32, (height * scale) as u32))
        }
        Contents::Zip { names } => {
            let name = names.get(index as usize).ok_or_else(missing)?;
            let bytes = read_entry(&mut open_zip(path)?, name)?;
            size_of(ImageReader::new(Cursor::new(bytes)))
        }
        Contents::Unpacked { pages } => {
            size_of(ImageReader::new(Cursor::new(pages.get(index as usize).ok_or_else(missing)?.as_slice())))
        }
    }
}

/// How much a PDF page `width` x `height` points large is scaled to draw it (see [`PDF_SCALE`]).
fn pdf_scale(width: f32, height: f32, max_side: Option<u32>) -> f32 {
    let longest = width.max(height).max(1.0);
    let scale = PDF_SCALE.min(PDF_MAX_SIDE / longest);
    match max_side {
        Some(side) => scale.min(side as f32 / longest),
        None => scale,
    }
}

/// The document at `path`, from those kept open when it hasn't changed since, otherwise read again.
fn contents(path: &Path) -> Result<Arc<Contents>, MediaError> {
    let metadata = std::fs::metadata(path)?;
    let stamp = (metadata.modified()?, metadata.len());
    if let Some(open) = opened().iter().find(|open| open.path == path && open.stamp == stamp) {
        return Ok(open.contents.clone());
    }
    let contents = Arc::new(read_contents(path)?);
    let mut opened = opened();
    opened.retain(|open| open.path != path);
    opened.push_front(Opened {
        path: path.to_path_buf(),
        stamp,
        contents: contents.clone(),
    });
    opened.truncate(KEPT_OPEN);
    Ok(contents)
}

fn read_contents(path: &Path) -> Result<Contents, MediaError> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let contents = match extension.as_str() {
        "pdf" => {
            let data = Arc::new(std::fs::read(path)?);
            let pages = Pdf::new(data.clone()).map_err(pdf_error)?.pages().len() as u32;
            Contents::Pdf { data, pages }
        }
        "cbz" => Contents::Zip {
            names: zip_pictures(&open_zip(path)?),
        },
        "cb7" => Contents::Unpacked {
            pages: unpack_7z(path)?,
        },
        "epub" => Contents::Zip {
            names: epub_pages(&mut open_zip(path)?)?,
        },
        _ => return Err(MediaError::Unsupported("Sortier doesn't read this kind of document".into())),
    };
    if contents.page_count() == 0 {
        return Err(MediaError::Unsupported("the document has no pages Sortier can show".into()));
    }
    Ok(contents)
}

fn pdf_error(error: hayro::hayro_syntax::LoadPdfError) -> MediaError {
    match error {
        hayro::hayro_syntax::LoadPdfError::Decryption(_) => {
            MediaError::Unsupported("password-protected PDFs can't be shown".into())
        }
        other => MediaError::Decode(format!("the PDF can't be read: {other:?}")),
    }
}

fn pdf_page(data: &Arc<Vec<u8>>, index: u32, max_side: Option<u32>) -> Result<Picture, MediaError> {
    let pdf = Pdf::new(data.clone()).map_err(pdf_error)?;
    let page = pdf
        .pages()
        .get(index as usize)
        .ok_or_else(|| MediaError::Decode(format!("page {} doesn't exist", index + 1)))?;
    let (width, height) = page.render_dimensions();
    let scale = pdf_scale(width, height, max_side);
    let settings = PixmapSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: WHITE,
    };
    let pixmap = hayro::render(page, &RenderCache::new(), &InterpreterSettings::default(), &RenderSettings::default(), &settings);
    let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    if width == 0 || height == 0 {
        return Err(MediaError::Decode("the PDF page has no size".into()));
    }
    // Drawn over opaque white, so the premultiplied pixels are already straight.
    Ok(Picture {
        width,
        height,
        rgba: pixmap.data_as_u8_slice().to_vec(),
    })
}

/// A picture file's bytes decoded (turned upright as its EXIF says), shrunk to fit `max_side` when given.
fn picture_from(bytes: &[u8], max_side: Option<u32>) -> Result<Picture, MediaError> {
    let mut image = decode_image(ImageReader::new(Cursor::new(bytes)))?;
    if let Some(side) = max_side
        && image.width().max(image.height()) > side
    {
        image = image.thumbnail(side, side);
    }
    let rgba = image.into_rgba8();
    Ok(Picture {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

fn unpacking(error: impl std::fmt::Display) -> MediaError {
    MediaError::Decode(format!("the archive can't be read: {error}"))
}

fn open_zip(path: &Path) -> Result<ZipArchive<BufReader<File>>, MediaError> {
    ZipArchive::new(BufReader::new(File::open(path)?)).map_err(unpacking)
}

fn read_entry(archive: &mut ZipArchive<BufReader<File>>, name: &str) -> Result<Vec<u8>, MediaError> {
    let mut entry = archive.by_name(name).map_err(unpacking)?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Whether an archive entry named `name` is a page picture: a picture file, and not one of the hidden extras
/// macOS adds to archives it makes.
fn is_page_picture(name: &str) -> bool {
    let file = name.rsplit('/').next().unwrap_or(name);
    let extension = file.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).unwrap_or_default();
    !name.starts_with("__MACOSX/") && !file.starts_with('.') && PICTURE_EXTENSIONS.contains(&extension.as_str())
}

/// The page pictures in a ZIP archive, in name order.
fn zip_pictures(archive: &ZipArchive<BufReader<File>>) -> Vec<String> {
    let mut names: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .filter(|name| is_page_picture(name))
        .map(|name| name.into_owned())
        .collect();
    names.sort_by(|a, b| natural_cmp(a, b));
    names
}

/// Every page picture in a 7z archive, unpacked, in name order.
fn unpack_7z(path: &Path) -> Result<Vec<Arc<Vec<u8>>>, MediaError> {
    let mut reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty()).map_err(unpacking)?;
    let mut pages: Vec<(String, Arc<Vec<u8>>)> = Vec::new();
    reader
        .for_each_entries(|entry, contents| {
            if !entry.is_directory() && is_page_picture(entry.name()) {
                let mut bytes = Vec::with_capacity(entry.size() as usize);
                contents.read_to_end(&mut bytes)?;
                pages.push((entry.name().to_string(), Arc::new(bytes)));
            }
            Ok(true)
        })
        .map_err(unpacking)?;
    pages.sort_by(|a, b| natural_cmp(&a.0, &b.0));
    Ok(pages.into_iter().map(|(_, bytes)| bytes).collect())
}

fn read_text(archive: &mut ZipArchive<BufReader<File>>, name: &str) -> Result<String, MediaError> {
    String::from_utf8(read_entry(archive, name)?).map_err(|_| MediaError::Decode(format!("{name} isn't text")))
}

/// The page pictures of an EPUB made of them, in reading order: each page of its spine that's a picture, or shows
/// one. EPUBs that are mostly text (novels) are refused.
fn epub_pages(archive: &mut ZipArchive<BufReader<File>>) -> Result<Vec<String>, MediaError> {
    let broken = |what: &str| MediaError::Decode(format!("the EPUB is damaged: {what}"));
    let container = read_text(archive, "META-INF/container.xml")?;
    let container = roxmltree::Document::parse(&container).map_err(|_| broken("its container can't be read"))?;
    let package_path = container
        .descendants()
        .find(|node| node.tag_name().name() == "rootfile")
        .and_then(|node| node.attribute("full-path"))
        .ok_or_else(|| broken("it doesn't say where its contents are"))?
        .to_string();
    let package = read_text(archive, &package_path)?;
    let package = roxmltree::Document::parse(&package).map_err(|_| broken("its contents can't be read"))?;
    let package_folder = folder_of(&package_path);
    let manifest: HashMap<&str, (String, &str)> = package
        .descendants()
        .filter(|node| node.tag_name().name() == "item")
        .filter_map(|node| {
            let href = resolve(package_folder, node.attribute("href")?);
            Some((node.attribute("id")?, (href, node.attribute("media-type").unwrap_or_default())))
        })
        .collect();
    let spine = package
        .descendants()
        .filter(|node| node.tag_name().name() == "itemref" && node.attribute("linear") != Some("no"))
        .filter_map(|node| manifest.get(node.attribute("idref")?));
    let mut pages = Vec::new();
    let mut text_pages = 0;
    for (href, media_type) in spine {
        if media_type.starts_with("image/") {
            pages.push(href.clone());
            continue;
        }
        let picture = read_text(archive, href).ok().and_then(|page| first_picture(&page));
        match picture {
            Some(source) => pages.push(resolve(folder_of(href), &source)),
            None => text_pages += 1,
        }
    }
    if pages.is_empty() || text_pages > pages.len() {
        return Err(MediaError::Unsupported(
            "this EPUB is text, such as a novel; Sortier reads EPUBs made of page pictures, such as manga and comics".into(),
        ));
    }
    Ok(pages)
}

/// The folder part of an archive path, with its trailing slash (empty at the top).
fn folder_of(path: &str) -> &str {
    path.rfind('/').map_or("", |slash| &path[..=slash])
}

/// The first picture an XHTML page shows: an `<img src>`, or an SVG `<image href>` (or `xlink:href`).
fn first_picture(page: &str) -> Option<String> {
    let lower = page.to_ascii_lowercase();
    let start = [lower.find("<img"), lower.find("<image")].into_iter().flatten().min()?;
    let end = start + lower[start..].find('>')?;
    let tag = &page[start..end];
    ["src", "xlink:href", "href"].into_iter().find_map(|name| attribute(tag, name))
}

/// The value of attribute `name` in the text of one tag, with `&amp;` and friends decoded.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        let preceded = lower[..at].chars().last().is_some_and(char::is_whitespace);
        let rest = lower[from..].trim_start();
        if !preceded || !rest.starts_with('=') {
            continue;
        }
        let value_start = tag.len() - rest[1..].trim_start().len();
        let quote = tag[value_start..].chars().next().filter(|quote| *quote == '"' || *quote == '\'')?;
        let value = &tag[value_start + 1..];
        let value = &value[..value.find(quote)?];
        return Some(
            value
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&"),
        );
    }
    None
}

/// `href` (a URL, perhaps relative, perhaps percent-encoded) as a path inside the archive, from `folder`.
fn resolve(folder: &str, href: &str) -> String {
    let href = href.split(['#', '?']).next().unwrap_or_default();
    let href = percent_decoded(href);
    let joined = if let Some(absolute) = href.strip_prefix('/') { absolute.to_string() } else { format!("{folder}{href}") };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| text.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epub_links_resolve_inside_the_archive() {
        assert_eq!(resolve("OEBPS/text/", "../images/p%2001.jpg#top"), "OEBPS/images/p 01.jpg");
        assert_eq!(resolve("", "page.xhtml"), "page.xhtml");
        assert_eq!(resolve("a/b/", "/cover.png"), "cover.png");
        assert_eq!(folder_of("OEBPS/content.opf"), "OEBPS/");
        assert_eq!(folder_of("content.opf"), "");
    }

    #[test]
    fn the_first_picture_of_a_page_is_found() {
        let page = r#"<html><body><div class="x"><img alt="a" src="../img/p1.jpg"/></div><img src="p2.jpg"/></body></html>"#;
        assert_eq!(first_picture(page).as_deref(), Some("../img/p1.jpg"));
        let svg = r#"<svg><image width="800" xlink:href="i/a&amp;b.png"></image></svg>"#;
        assert_eq!(first_picture(svg).as_deref(), Some("i/a&b.png"));
        assert_eq!(first_picture(r#"<img data-src="x.jpg" src='y.jpg'>"#).as_deref(), Some("y.jpg"));
        assert_eq!(first_picture("<p>Chapter one</p>"), None);
    }

    #[test]
    fn hidden_and_non_picture_entries_are_not_pages() {
        assert!(is_page_picture("vol1/p002.JPG"));
        assert!(!is_page_picture("__MACOSX/vol1/._p002.jpg"));
        assert!(!is_page_picture("vol1/.DS_Store"));
        assert!(!is_page_picture("vol1/ComicInfo.xml"));
    }
}
