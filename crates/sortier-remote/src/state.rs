//! The library a server holds: its version, the patches that brought it there, scanning its folders and keeping its
//! thumbnails made.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use anyhow::{Result, bail};
use sortier_client::api::Deleted;
use sortier_library::library::{ItemId, Library, MediaKind, Snapshot, data_dir};
use sortier_library::patch::Patch;
use sortier_library::thumbs;
use tokio::sync::{Notify, Semaphore, watch};

use crate::setup::{Config, Devices};

/// How many recent patches are kept for clients catching up; one that's further behind fetches everything.
const KEPT_PATCHES: usize = 2000;
/// Thumbnails are made this many at a time between saves.
const THUMB_BATCH: usize = 64;

/// The library, the version it's at, and the recent patches: each with the version it brought the library to.
pub struct Held {
    pub library: Library,
    pub version: u64,
    log: VecDeque<(u64, Patch)>,
}

impl Held {
    /// The patches after `since`, or `None` when they're no longer all kept.
    pub fn patches_since(&self, since: u64) -> Option<Vec<Patch>> {
        if since >= self.version {
            return Some(Vec::new());
        }
        let oldest = self.log.front().map(|(version, _)| version - 1)?;
        if since < oldest {
            return None;
        }
        Some(self.log.iter().filter(|(version, _)| *version > since).map(|(_, patch)| patch.clone()).collect())
    }
}

/// Everything the server's handlers share.
pub struct Server {
    pub data: PathBuf,
    config: Mutex<Config>,
    /// When `config.toml` had last changed as `config` was read.
    config_read: Mutex<Option<SystemTime>>,
    pub fingerprint: String,
    held: Mutex<Held>,
    pub devices: Mutex<Devices>,
    /// Holds the latest version, for clients waiting for changes.
    pub versions: watch::Sender<u64>,
    /// Wakes the scanner to look through the folders now.
    pub rescan: Notify,
    /// Files uploaded and not yet added to the library.
    uploaded: Mutex<Vec<PathBuf>>,
    /// Wakes the scanner to add what was uploaded.
    pub uploads_waiting: Notify,
    /// Wakes the saver to write the library, which changed.
    pub unsaved: Notify,
    /// Limits how many pictures are decoded at once for devices' requests (thumbnails, pages, covers), so a burst of
    /// them can't use up the memory: big pictures and videos take a lot to decode.
    pub decoding: Semaphore,
    /// Limits password checks to one at a time: each takes a lot of memory, by design.
    pub logins: Semaphore,
}

impl Server {
    pub fn new(data: PathBuf, config: Config, fingerprint: String) -> Arc<Self> {
        let library = Library::load(data.join("library.json"));
        let devices = Devices::load(&data);
        let (versions, _) = watch::channel(0);
        let config_read = Config::changed_at(&data);
        Arc::new(Self {
            data,
            config: Mutex::new(config),
            config_read: Mutex::new(config_read),
            fingerprint,
            held: Mutex::new(Held {
                library,
                version: 0,
                log: VecDeque::new(),
            }),
            devices: Mutex::new(devices),
            versions,
            rescan: Notify::new(),
            uploaded: Mutex::new(Vec::new()),
            uploads_waiting: Notify::new(),
            unsaved: Notify::new(),
            decoding: Semaphore::new(decoders()),
            logins: Semaphore::new(1),
        })
    }

    pub fn config(&self) -> MutexGuard<'_, Config> {
        self.config.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Reads `config.toml` again if it changed since it was last read (the `sortier-remote` command changes it while
    /// the server runs), returning whether it did. A file that can't be read leaves the settings as they were.
    pub fn reload_config(&self) -> bool {
        let changed_at = Config::changed_at(&self.data);
        let mut read = self.config_read.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if changed_at == *read {
            return false;
        }
        *read = changed_at;
        drop(read);
        self.read_config()
    }

    /// Reads `config.toml` again, returning whether it could be. A file that can't be read leaves the settings as
    /// they were.
    fn read_config(&self) -> bool {
        match Config::load(&self.data) {
            Ok(config) => {
                *self.config() = config;
                true
            }
            Err(_) => false,
        }
    }

    pub fn held(&self) -> MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Changes the library with `change`, saving it and telling waiting clients when anything changed.
    pub fn change(&self, change: impl FnOnce(&mut Library)) {
        let mut held = self.held();
        let before = held.library.snapshot();
        change(&mut held.library);
        self.commit(&mut held, &before);
    }

    fn commit(&self, held: &mut Held, before: &Snapshot) {
        let patch = held.library.diff(before);
        if !patch.is_empty() {
            self.record(held, patch);
        }
    }

    fn record(&self, held: &mut Held, patch: Patch) {
        held.version += 1;
        held.log.push_back((held.version, patch));
        if held.log.len() > KEPT_PATCHES {
            held.log.pop_front();
        }
        self.versions.send_replace(held.version);
        self.unsaved.notify_one();
    }

    /// Writes the library to `library.json`. The library is only held while it's copied out, so requests aren't kept
    /// waiting while a large library is written. Blocking.
    pub fn save(&self) -> Result<()> {
        let bytes = serde_json::to_vec(&self.held().library)?;
        let file = self.data.join("library.json");
        let partial = file.with_extension("json.tmp");
        std::fs::write(&partial, bytes)?;
        std::fs::rename(&partial, &file)?;
        Ok(())
    }

    /// Makes a client's change to the library at version `base`, returning the version after it. When the library
    /// has moved on since `base` (the server's own scans and thumbnails move it on often), the change is still made
    /// as long as nothing changed since touches what it changes; otherwise it's refused (`Ok(None)`). Also refused
    /// when it goes beyond what clients may change.
    pub fn apply_client_patch(&self, base: u64, patch: Patch) -> Result<Option<u64>> {
        let mut held = self.held();
        if base != held.version {
            let Some(newer) = held.patches_since(base).filter(|_| base < held.version) else { return Ok(None) };
            if overlaps(&newer, &patch) {
                return Ok(None);
            }
        }
        check_client_patch(&held.library, &patch, &self.covers())?;
        held.library.apply_patch(&patch);
        self.record(&mut held, patch);
        Ok(Some(held.version))
    }

    /// Erases `ids`' files from disk for good and takes them out of the library, saying which went and why any
    /// didn't. Blocking.
    pub fn delete(&self, ids: &[ItemId]) -> Deleted {
        let files: Vec<(ItemId, PathBuf)> = {
            let held = self.held();
            ids.iter().filter_map(|id| held.library.item(*id).map(|item| (*id, item.path.clone()))).collect()
        };
        let mut result = Deleted::default();
        for (id, path) in files {
            match std::fs::remove_file(&path) {
                Ok(()) => result.deleted.push(id),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => result.deleted.push(id),
                Err(error) => result.failed.push(format!("{}: {error}", path.display())),
            }
        }
        self.change(|library| library.remove_items(&result.deleted));
        result
    }

    pub fn covers(&self) -> PathBuf {
        data_dir().join("covers")
    }

    /// Where uploads go: the upload folder set, else the first folder served.
    pub fn upload_folder(&self) -> Option<PathBuf> {
        self.reload_config();
        let config = self.config();
        config.upload_folder.clone().or_else(|| config.folders.first().cloned())
    }

    /// Notes that `file` was uploaded, for the scanner to add to the library soon.
    pub fn uploaded(&self, file: PathBuf) {
        self.uploaded.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(file);
        self.uploads_waiting.notify_one();
    }

    /// Adds the files uploaded since this last ran to the library and makes their thumbnails, without looking
    /// through every folder served. Files uploaded outside the folders served are left out, as a scan would leave
    /// them out. Blocking.
    pub fn add_uploads(&self) {
        let uploaded = std::mem::take(&mut *self.uploaded.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        let folders = self.config().folders.clone();
        let served: Vec<PathBuf> = uploaded.into_iter().filter(|file| folders.iter().any(|folder| file.starts_with(folder))).collect();
        if served.is_empty() {
            return;
        }
        let found = Library::scan_folders(&served);
        self.change(|library| {
            library.merge_scan(&[], found);
        });
        self.make_thumbnails();
    }

    /// Looks through the folders served (as `config.toml` lists them now), recording what came and went (and the
    /// files of folders no longer served as gone), then makes the thumbnails (and learns the sizes and page counts) of
    /// what lacks them. Blocking.
    pub fn scan(&self) {
        self.read_config();
        let folders = self.config().folders.clone();
        let found = Library::scan_folders(&folders);
        self.change(|library| {
            for gone in library.roots.clone().iter().filter(|root| !folders.contains(root)) {
                library.remove_root(gone);
            }
            for folder in &folders {
                library.add_root(folder.clone());
            }
            library.merge_scan(&folders, found);
        });
        self.make_thumbnails();
    }

    /// Makes missing thumbnails, and records sizes and page counts not yet known. The library is only held while
    /// its items are listed: which thumbnails exist is checked on disk without it, so requests aren't kept waiting.
    /// Blocking.
    pub fn make_thumbnails(&self) {
        let listed: Vec<thumbs::Job> = self
            .held()
            .library
            .items
            .values()
            .map(|item| thumbs::Job {
                id: item.id,
                src: item.path.clone(),
                kind: item.kind,
                dest: thumbs::thumb_path(&item.path, item.modified),
                need_dims: unmeasured(item),
            })
            .collect();
        let mut wanted: Vec<thumbs::Job> = listed
            .into_iter()
            .filter(|job| (job.need_dims || !job.dest.exists()) && !job.dest.with_extension("failed").exists())
            .collect();
        while !wanted.is_empty() {
            let jobs: Vec<thumbs::Job> = wanted.drain(..wanted.len().min(THUMB_BATCH)).collect();
            let results = thumbs::run_jobs(jobs);
            let still_unmeasured = self.record_measurements(&results);
            for result in &results {
                if !result.dest.exists() || still_unmeasured.contains(&result.id) {
                    remember_failure(&result.dest);
                }
            }
        }
    }

    /// Records the sizes and page counts `results` learned, as a patch of just the items they changed (rather than
    /// comparing the whole library before and after), returning the items whose size is still unknown.
    fn record_measurements(&self, results: &[thumbs::JobResult]) -> HashSet<ItemId> {
        let mut held = self.held();
        let mut changed = Vec::new();
        let mut unknown = HashSet::new();
        for result in results {
            let Some(item) = held.library.items.get_mut(&result.id) else { continue };
            let before = (item.dims, item.pages);
            item.dims = result.dims.or(item.dims);
            item.pages = result.pages.or(item.pages);
            if (item.dims, item.pages) != before {
                changed.push(item.clone());
            }
            if unmeasured(item) {
                unknown.insert(item.id);
            }
        }
        if !changed.is_empty() {
            let patch = Patch {
                items: changed,
                next_id: held.library.next_id,
                ..Patch::default()
            };
            self.record(&mut held, patch);
        }
        unknown
    }
}

/// How many pictures may be decoded at once for devices' requests: half the processor's cores, and at least two.
fn decoders() -> usize {
    std::thread::available_parallelism().map_or(2, |cores| (cores.get() / 2).max(2))
}

/// Whether `patch` changes anything the `newer` patches changed.
fn overlaps(newer: &[Patch], patch: &Patch) -> bool {
    let (mut items, mut groups, mut collections) = (HashSet::new(), HashSet::new(), HashSet::new());
    let mut dismissed = false;
    for change in newer {
        items.extend(change.items.iter().map(|item| item.id).chain(change.removed_items.iter().copied()));
        groups.extend(change.groups.iter().map(|group| group.id).chain(change.removed_groups.iter().copied()));
        collections.extend(change.collections.iter().map(|collection| collection.id).chain(change.removed_collections.iter().copied()));
        dismissed |= change.dismissed_collections.is_some();
    }
    patch.items.iter().any(|item| items.contains(&item.id))
        || patch.groups.iter().map(|group| group.id).chain(patch.removed_groups.iter().copied()).any(|id| groups.contains(&id))
        || patch
            .collections
            .iter()
            .map(|collection| collection.id)
            .chain(patch.removed_collections.iter().copied())
            .any(|id| collections.contains(&id))
        || (dismissed && patch.dismissed_collections.is_some())
}

/// Whether an item's size (or a document's page count) isn't known yet.
fn unmeasured(item: &sortier_library::library::Item) -> bool {
    item.dims.is_none() || (item.kind == MediaKind::Document && item.pages.is_none())
}

/// Marks a file whose thumbnail (or size) couldn't be made with an empty `.failed` file beside where the thumbnail
/// goes, so it isn't tried again until the file changes, which changes the thumbnail's name.
fn remember_failure(dest: &Path) {
    if let Some(folder) = dest.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let _ = std::fs::write(dest.with_extension("failed"), b"");
}

/// Whether `patch` only changes what clients may: tags, sequences, collections, covers, and the sizes and page counts
/// a viewer learns. Clients can't add or remove files, or point one somewhere else, and covers must be ones the
/// server saved.
fn check_client_patch(library: &Library, patch: &Patch, covers: &Path) -> Result<()> {
    if !patch.removed_items.is_empty() {
        bail!("files can't be removed from the library remotely");
    }
    let cover_ok = |cover: &Option<PathBuf>| cover.as_ref().is_none_or(|cover| cover.parent() == Some(covers));
    for item in &patch.items {
        let Some(existing) = library.item(item.id) else { bail!("files can't be added to the library remotely") };
        if existing.path != item.path || existing.kind != item.kind || existing.size != item.size || existing.modified != item.modified {
            bail!("a file's place can't be changed remotely");
        }
        if !cover_ok(&item.cover) {
            bail!("covers must be ones saved on the server");
        }
    }
    if !patch.groups.iter().all(|group| cover_ok(&group.cover)) || !patch.collections.iter().all(|collection| cover_ok(&collection.cover)) {
        bail!("covers must be ones saved on the server");
    }
    Ok(())
}
