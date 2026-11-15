//! Getting a remote vault's media from its Sortier Remote server, through the vault's cache.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sortier_client::{Cache, RemoteClient, RemoteError, RemoteFile};
use sortier_library::library::Item;
use sortier_media::{MediaError, Picture, ReadSeek, VideoSource};

/// The server of a remote vault and the cache of what was fetched from it. Cheap to clone; every method blocks, so
/// call them off the UI thread.
#[derive(Clone)]
pub struct RemoteAccess {
    pub client: Arc<RemoteClient>,
    pub cache: Arc<Cache>,
}

/// A failure to fetch something as a media error, for the viewers to show.
pub fn media_error(error: RemoteError) -> MediaError {
    MediaError::Io(std::io::Error::other(error))
}

fn extension_of(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, extension)| extension).unwrap_or_default()
}

impl RemoteAccess {
    /// The item's thumbnail, made by the server.
    pub fn thumbnail(&self, item: &Item) -> Result<PathBuf, RemoteError> {
        let key = format!("thumb:{}:{}", item.id, item.modified);
        self.cache.fetch(&key, "jpg", || self.client.thumbnail(item.id))
    }

    /// The item's whole file, kept with its extension (some files are recognised by it).
    pub fn file(&self, item: &Item) -> Result<PathBuf, RemoteError> {
        let name = item.file_name();
        let key = format!("file:{}:{}:{}", item.id, item.modified, item.size);
        self.cache.fetch(&key, &extension_of(&name).to_ascii_lowercase(), || self.client.file(item.id))
    }

    /// Page `index` of a document, drawn by the server.
    pub fn page(&self, item: &Item, index: u32) -> Result<Picture, MediaError> {
        let key = format!("page:{}:{index}:{}", item.id, item.modified);
        let path = self.cache.fetch(&key, "jpg", || self.client.page(item.id, index)).map_err(media_error)?;
        sortier_media::decode_still(&path)
    }

    /// The cover the server saved at `cover`.
    pub fn cover(&self, cover: &Path) -> Result<PathBuf, RemoteError> {
        let key = format!("cover:{}", cover.to_string_lossy());
        self.cache.fetch(&key, "jpg", || self.client.cover(cover))
    }

    /// The item's file read from the server as it's played, a block at a time.
    pub fn video(&self, item: &Item) -> VideoSource {
        let (client, id) = (self.client.clone(), item.id);
        VideoSource::Stream {
            name: item.file_name(),
            open: Arc::new(move || {
                let file = RemoteFile::open(client.clone(), id).map_err(std::io::Error::other)?;
                Ok(Box::new(file) as Box<dyn ReadSeek + Send>)
            }),
        }
    }
}
