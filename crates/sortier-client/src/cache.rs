use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::client::RemoteError;

/// Files fetched from a server, kept on disk until they take more than a size limit, when those used longest ago
/// go first. Kept files are found again by the key they were fetched under, so the server isn't asked twice, and
/// they're there while the server can't be reached.
pub struct Cache {
    dir: PathBuf,
    limit: u64,
    /// What the kept files take, once counted.
    used: Mutex<Option<u64>>,
}

impl Cache {
    /// A cache in `dir` keeping at most about `limit` bytes.
    pub fn new(dir: PathBuf, limit: u64) -> Self {
        Self {
            dir,
            limit,
            used: Mutex::new(None),
        }
    }

    fn used(&self) -> MutexGuard<'_, Option<u64>> {
        self.used.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Where the file kept under `key` is (or would be). `extension` is kept, since some files are recognised by it.
    fn path_of(&self, key: &str, extension: &str) -> PathBuf {
        let hash: String = Sha256::digest(key.as_bytes()).iter().take(16).map(|byte| format!("{byte:02x}")).collect();
        let name = if extension.is_empty() { hash.clone() } else { format!("{hash}.{extension}") };
        self.dir.join(&hash[..2]).join(name)
    }

    /// The file kept under `key`, if there is one, marked as just used.
    pub fn get(&self, key: &str, extension: &str) -> Option<PathBuf> {
        let path = self.path_of(key, extension);
        let file = File::options().append(true).open(&path).ok()?;
        // Only orders what's dropped first; a file that can't be marked is simply kept as old.
        let _ = file.set_modified(SystemTime::now());
        Some(path)
    }

    /// The file kept under `key`, fetching it with `fetch` (and keeping it) when there isn't one.
    pub fn fetch(&self, key: &str, extension: &str, fetch: impl FnOnce() -> Result<Vec<u8>, RemoteError>) -> Result<PathBuf, RemoteError> {
        if let Some(path) = self.get(key, extension) {
            return Ok(path);
        }
        let bytes = fetch()?;
        self.put(key, extension, &bytes)
    }

    /// Keeps `bytes` under `key`, making room for them first.
    pub fn put(&self, key: &str, extension: &str, bytes: &[u8]) -> Result<PathBuf, RemoteError> {
        let path = self.path_of(key, extension);
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let partial = path.with_extension("partial");
        std::fs::write(&partial, bytes)?;
        std::fs::rename(&partial, &path)?;
        let mut used = self.used();
        let total = match *used {
            Some(total) => total + bytes.len() as u64,
            None => folder_size(&self.dir),
        };
        *used = Some(if total > self.limit { self.shrink(&path) } else { total });
        Ok(path)
    }

    /// Drops the files used longest ago (never `keep`) until what's left takes at most nine tenths of the limit,
    /// returning what it takes then.
    fn shrink(&self, keep: &Path) -> u64 {
        let mut files = Vec::new();
        for folder in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            for file in std::fs::read_dir(folder.path()).into_iter().flatten().flatten() {
                if let Ok(metadata) = file.metadata() {
                    files.push((metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), metadata.len(), file.path()));
                }
            }
        }
        files.sort();
        let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
        let target = self.limit / 10 * 9;
        for (_, size, path) in files {
            if total <= target {
                break;
            }
            if path != keep && std::fs::remove_file(&path).is_ok() {
                total -= size;
            }
        }
        total
    }
}

fn folder_size(dir: &Path) -> u64 {
    let mut total = 0;
    for folder in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        for file in std::fs::read_dir(folder.path()).into_iter().flatten().flatten() {
            total += file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_used_longest_ago_go_first() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf(), 100);
        let fetches = std::cell::Cell::new(0);
        let fetch = |key: &str| {
            cache
                .fetch(key, "jpg", || {
                    fetches.set(fetches.get() + 1);
                    Ok(vec![0; 40])
                })
                .unwrap()
        };
        let first = fetch("a");
        assert_eq!(first.extension().unwrap(), "jpg");
        fetch("b");
        fetch("a");
        assert_eq!(fetches.get(), 2, "a was kept");
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(cache.get("a", "jpg").is_some(), "a was used last");
        fetch("c");
        assert!(cache.get("b", "jpg").is_none(), "b was used longest ago");
        assert!(cache.get("a", "jpg").is_some() && cache.get("c", "jpg").is_some());
    }
}
