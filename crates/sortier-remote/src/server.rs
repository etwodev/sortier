//! The HTTPS API (see `sortier_client::api`): logging in, the library and its changes, thumbnails, files (with ranges,
//! for streaming), document pages, covers, and uploads.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path as UrlPath, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use sortier_client::api::{
    Changes, CoverFromPage, CoverSaved, DeleteRequest, Deleted, Hello, IfExists, Login, PatchRequest, Problem, Token, UploadPaths,
    Version,
};
use sortier_library::library::{Item, ItemId, Library, MediaKind};
use sortier_library::thumbs;
use tokio::io::AsyncWriteExt as _;
use tower::ServiceExt as _;

use crate::state::Server;

/// The largest JSON request (a patch tagging many thousands of files, say).
const LARGEST_REQUEST: usize = 256 << 20;
/// How much of an upload is gathered before it's written: written as it arrives, in the small pieces the network
/// gives, each piece would be a trip to a file-writing thread.
const UPLOAD_BUFFER: usize = 1 << 20;
/// How long a request for changes waits for one.
const CHANGES_WAIT: Duration = Duration::from_secs(25);

type Shared = Arc<Server>;

pub fn router(server: Shared) -> Router {
    Router::new()
        .route("/api/hello", get(hello))
        .route("/api/login", post(login))
        .route("/api/library", get(library))
        .route("/api/changes", get(changes))
        .route("/api/patch", post(patch))
        .route("/api/thumb/{item}", get(thumbnail))
        .route("/api/file/{item}", get(file))
        .route("/api/page/{item}/{index}", get(page))
        .route("/api/cover/{name}", get(cover))
        .route("/api/covers", post(save_cover))
        .route("/api/covers/from-page", post(cover_from_page))
        .route("/api/rescan", post(rescan))
        .route("/api/delete", post(delete))
        .route("/api/upload", post(upload).layer(DefaultBodyLimit::disable()))
        .route("/api/upload/existing", post(existing_uploads))
        .layer(DefaultBodyLimit::max(LARGEST_REQUEST))
        .with_state(server)
}

/// A failed request: its status and what to tell the client.
struct Failure(StatusCode, String);

impl Failure {
    fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }

    fn missing(message: impl Into<String>) -> Self {
        Self(StatusCode::NOT_FOUND, message.into())
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        (self.0, Json(Problem { message: self.1 })).into_response()
    }
}

/// Proof that a request carries the token of a logged-in device.
struct Authorized;

impl FromRequestParts<Shared> for Authorized {
    type Rejection = Failure;

    async fn from_request_parts(parts: &mut Parts, server: &Shared) -> Result<Self, Failure> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or_default();
        let known = server.devices.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).knows(token);
        if known { Ok(Authorized) } else { Err(Failure(StatusCode::UNAUTHORIZED, "log in first".into())) }
    }
}

/// Runs blocking work (anything holding the library, or reading or writing files) off the async threads.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, Failure> {
    tokio::task::spawn_blocking(work).await.map_err(Failure::internal)
}

/// Runs decoding work (pictures, pages, video frames) off the async threads, waiting until fewer than the server's
/// limit of it are running.
async fn decoding<T: Send + 'static>(server: &Server, work: impl FnOnce() -> T + Send + 'static) -> Result<T, Failure> {
    let _turn = server.decoding.acquire().await.map_err(Failure::internal)?;
    blocking(work).await
}

async fn hello() -> Json<Hello> {
    Json(Hello {
        server: "Sortier Remote".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

async fn login(State(server): State<Shared>, Json(login): Json<Login>) -> Result<Json<Token>, Failure> {
    let matches = {
        let _turn = server.logins.acquire().await.map_err(Failure::internal)?;
        let config = server.config().clone();
        blocking(move || config.password_matches(&login.password)).await?
    };
    if !matches {
        // Slows down guessing.
        tokio::time::sleep(Duration::from_secs(1)).await;
        return Err(Failure(StatusCode::UNAUTHORIZED, "the password isn't right".into()));
    }
    let mut devices = server.devices.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let device = if login.device.trim().is_empty() { "a device" } else { login.device.trim() };
    let token = devices.add(device).map_err(Failure::internal)?;
    devices.save(&server.data).map_err(Failure::internal)?;
    Ok(Json(Token { token }))
}

/// The library as sent, borrowed rather than copied.
#[derive(Serialize)]
struct LibraryReply<'a> {
    version: u64,
    library: &'a Library,
}

async fn library(State(server): State<Shared>, _: Authorized) -> Result<Response, Failure> {
    let body = blocking(move || {
        let held = server.held();
        serde_json::to_vec(&LibraryReply {
            version: held.version,
            library: &held.library,
        })
    })
    .await?
    .map_err(Failure::internal)?;
    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}

#[derive(Deserialize)]
struct Since {
    since: u64,
}

async fn changes(State(server): State<Shared>, _: Authorized, Query(Since { since }): Query<Since>) -> Result<Json<Changes>, Failure> {
    let mut versions = server.versions.subscribe();
    if *versions.borrow_and_update() <= since {
        let _ = tokio::time::timeout(CHANGES_WAIT, versions.changed()).await;
    }
    let changes = blocking(move || {
        let held = server.held();
        match held.patches_since(since) {
            Some(patches) => Changes {
                version: held.version,
                patches,
                reload: false,
            },
            None => Changes {
                version: held.version,
                patches: Vec::new(),
                reload: true,
            },
        }
    })
    .await?;
    Ok(Json(changes))
}

async fn patch(State(server): State<Shared>, _: Authorized, Json(request): Json<PatchRequest>) -> Result<Json<Version>, Failure> {
    match blocking(move || server.apply_client_patch(request.base, request.patch)).await? {
        Ok(Some(version)) => Ok(Json(Version { version })),
        Ok(None) => Err(Failure(StatusCode::CONFLICT, "the library changed meanwhile".into())),
        Err(error) => Err(Failure(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())),
    }
}

async fn item(server: &Shared, id: ItemId) -> Result<Item, Failure> {
    let server = server.clone();
    blocking(move || server.held().library.item(id).cloned()).await?.ok_or_else(|| Failure::missing("there's no such file"))
}

fn jpeg(bytes: Vec<u8>) -> Response {
    ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "private, max-age=31536000")], bytes).into_response()
}

async fn thumbnail(State(server): State<Shared>, _: Authorized, UrlPath(id): UrlPath<ItemId>) -> Result<Response, Failure> {
    let item = item(&server, id).await?;
    let dest = thumbs::thumb_path(&item.path, item.modified);
    if !tokio::fs::try_exists(&dest).await.unwrap_or(false) {
        let job = thumbs::Job {
            id,
            src: item.path.clone(),
            kind: item.kind,
            dest: dest.clone(),
            need_dims: false,
        };
        // The scan may have made it while this waited its turn.
        decoding(&server, move || if !job.dest.exists() { thumbs::run_jobs(vec![job]); }).await?;
    }
    let bytes = tokio::fs::read(&dest).await.map_err(|_| Failure::missing("the thumbnail can't be made"))?;
    Ok(jpeg(bytes))
}

async fn file(State(server): State<Shared>, _: Authorized, UrlPath(id): UrlPath<ItemId>, request: Request) -> Result<Response, Failure> {
    let item = item(&server, id).await?;
    let served = tower_http::services::ServeFile::new(&item.path).oneshot(request).await.map_err(Failure::internal)?;
    Ok(served.into_response())
}

async fn page(State(server): State<Shared>, _: Authorized, UrlPath((id, index)): UrlPath<(ItemId, u32)>) -> Result<Response, Failure> {
    let item = item(&server, id).await?;
    if item.kind != MediaKind::Document {
        return Err(Failure::bad("that file isn't a document"));
    }
    let encoded = decoding(&server, move || -> Result<Vec<u8>, String> {
        let picture = sortier_media::document_page(&item.path, index, None).map_err(|error| error.to_string())?;
        let image = image::RgbaImage::from_raw(picture.width, picture.height, picture.rgba).ok_or("the page had the wrong size")?;
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
            .encode_image(&image::DynamicImage::ImageRgba8(image).to_rgb8())
            .map_err(|error| error.to_string())?;
        Ok(bytes)
    })
    .await?
    .map_err(Failure::missing)?;
    Ok(jpeg(encoded))
}

/// Whether `name` is the name of a cover the server saved: hex digits and `.jpg`, so it can't reach elsewhere.
fn is_cover_name(name: &str) -> bool {
    name.strip_suffix(".jpg").is_some_and(|stem| !stem.is_empty() && stem.chars().all(|c| c.is_ascii_hexdigit()))
}

async fn cover(State(server): State<Shared>, _: Authorized, UrlPath(name): UrlPath<String>) -> Result<Response, Failure> {
    if !is_cover_name(&name) {
        return Err(Failure::bad("that isn't a cover"));
    }
    let bytes = tokio::fs::read(server.covers().join(name)).await.map_err(|_| Failure::missing("there's no such cover"))?;
    Ok(jpeg(bytes))
}

fn saved(path: Result<PathBuf, anyhow::Error>) -> Result<Json<CoverSaved>, Failure> {
    let path = path.map_err(|error| Failure::bad(format!("the cover can't be made: {error:#}")))?;
    Ok(Json(CoverSaved {
        path: path.to_string_lossy().into_owned(),
    }))
}

async fn save_cover(State(server): State<Shared>, _: Authorized, picture: axum::body::Bytes) -> Result<Json<CoverSaved>, Failure> {
    let covers = server.covers();
    saved(
        decoding(&server, move || {
            std::fs::create_dir_all(&covers)?;
            let upload = covers.join(format!("upload-{}", crate::setup::hex(&crate::setup::random_bytes::<8>()?)));
            std::fs::write(&upload, &picture)?;
            let made = thumbs::save_cover(&upload, MediaKind::Image, 0);
            let _ = std::fs::remove_file(&upload);
            made
        })
        .await?,
    )
}

async fn cover_from_page(State(server): State<Shared>, _: Authorized, Json(from): Json<CoverFromPage>) -> Result<Json<CoverSaved>, Failure> {
    let item = item(&server, from.item).await?;
    saved(decoding(&server, move || thumbs::save_cover(&item.path, item.kind, from.index)).await?)
}

async fn delete(State(server): State<Shared>, _: Authorized, Json(request): Json<DeleteRequest>) -> Result<Json<Deleted>, Failure> {
    let deleting = server.clone();
    let deleted = blocking(move || deleting.delete(&request.items)).await?;
    Ok(Json(deleted))
}

async fn rescan(State(server): State<Shared>, _: Authorized) -> StatusCode {
    server.rescan.notify_one();
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
struct Upload {
    path: String,
    #[serde(default)]
    if_exists: IfExists,
}

/// `relative` (with `/` between folders) inside `folder`, unless it's empty or tries to leave it.
fn inside(folder: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    let plain = relative.components().all(|component| matches!(component, Component::Normal(_)));
    (plain && relative.components().next().is_some()).then(|| folder.join(relative))
}

async fn existing_uploads(State(server): State<Shared>, _: Authorized, Json(asked): Json<UploadPaths>) -> Result<Json<UploadPaths>, Failure> {
    let folder = server.upload_folder().ok_or_else(|| Failure::bad("the server has no folder to upload into"))?;
    let existing = blocking(move || {
        let mut existing = Vec::new();
        for path in asked.paths {
            if inside(&folder, &path).is_some_and(|target| target.exists()) {
                existing.push(path);
            }
        }
        existing
    })
    .await?;
    Ok(Json(UploadPaths { paths: existing }))
}

/// `wanted`, or when there's a file there already, the first of `name (2).ext`, `name (3).ext` and so on that's free.
fn free_name(wanted: &Path) -> PathBuf {
    if !wanted.exists() {
        return wanted.to_path_buf();
    }
    let stem = wanted.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = wanted.extension().map(|extension| format!(".{}", extension.to_string_lossy())).unwrap_or_default();
    let mut number = 2;
    loop {
        let candidate = wanted.with_file_name(format!("{stem} ({number}){extension}"));
        if !candidate.exists() {
            return candidate;
        }
        number += 1;
    }
}

async fn upload(
    State(server): State<Shared>,
    _: Authorized,
    Query(Upload { path, if_exists }): Query<Upload>,
    body: Body,
) -> Result<StatusCode, Failure> {
    let folder = server.upload_folder().ok_or_else(|| Failure::bad("the server has no folder to upload into"))?;
    let wanted = inside(&folder, &path).ok_or_else(|| Failure::bad("that isn't a path inside the upload folder"))?;
    let partial = wanted.with_extension("sortier-upload");
    if let Some(parent) = wanted.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(Failure::internal)?;
    }
    let mut file = tokio::io::BufWriter::with_capacity(UPLOAD_BUFFER, tokio::fs::File::create(&partial).await.map_err(Failure::internal)?);
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let written = match chunk {
            Ok(chunk) => file.write_all(&chunk).await.map_err(Failure::internal),
            Err(error) => Err(Failure::bad(format!("the upload stopped: {error}"))),
        };
        if let Err(failure) = written {
            drop(file);
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(failure);
        }
    }
    file.flush().await.map_err(Failure::internal)?;
    drop(file);
    let target = match if_exists {
        IfExists::Replace => wanted,
        IfExists::KeepBoth => blocking(move || free_name(&wanted)).await?,
        IfExists::Refuse if tokio::fs::try_exists(&wanted).await.unwrap_or(true) => {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(Failure(StatusCode::CONFLICT, "there's a file there already".into()));
        }
        IfExists::Refuse => wanted,
    };
    tokio::fs::rename(&partial, &target).await.map_err(Failure::internal)?;
    server.uploaded(target);
    Ok(StatusCode::CREATED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploads_and_covers_stay_where_they_belong() {
        let folder = Path::new("/srv/uploads");
        assert_eq!(inside(folder, "Series/Vol 1.cbz"), Some(folder.join("Series/Vol 1.cbz")));
        assert_eq!(inside(folder, "../etc/passwd"), None);
        assert_eq!(inside(folder, "/etc/passwd"), None);
        assert_eq!(inside(folder, ""), None);
        assert!(is_cover_name("0a1b2c3d4e5f6a7b.jpg"));
        assert!(!is_cover_name("../library.json") && !is_cover_name(".jpg"));
    }

    #[test]
    fn keeping_both_finds_a_free_name() {
        let folder = tempfile::tempdir().unwrap();
        let wanted = folder.path().join("page.jpg");
        assert_eq!(free_name(&wanted), wanted);
        std::fs::write(&wanted, b"").unwrap();
        assert_eq!(free_name(&wanted), folder.path().join("page (2).jpg"));
        std::fs::write(folder.path().join("page (2).jpg"), b"").unwrap();
        assert_eq!(free_name(&wanted), folder.path().join("page (3).jpg"));
        assert_eq!(free_name(&folder.path().join("notes")), folder.path().join("notes"));
    }
}
