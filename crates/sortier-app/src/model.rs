use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui::{AppContext as _, Context, SharedString};
use sortier_client::api::{Changes, LibraryState};
use sortier_client::{Cache, RemoteClient, RemoteError};
use sortier_library::library::{
    CollectionId, FitMode, Group, GroupId, Item, ItemId, Library, MediaKind, Member, Settings, Snapshot as LibrarySnapshot,
    SortKey, natural_cmp, parse_tags,
};
use sortier_library::patch::Patch;
use sortier_library::query::{self, Query};
use sortier_library::thumbs;
use sortier_library::vault::{VaultKind, Vaults};
use sortier_media::HardwareDecoder;

use crate::remote::RemoteAccess;

/// One tile of the results: a single file, or a whole sequence or collection when sequences are shown as one tile.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Entry {
    Item(ItemId),
    Group(GroupId),
    Collection(CollectionId),
}

impl Entry {
    /// The collection volume this entry is, if it's one kind.
    pub fn member(self) -> Option<Member> {
        match self {
            Entry::Item(id) => Some(Member::Item(id)),
            Entry::Group(group) => Some(Member::Group(group)),
            Entry::Collection(_) => None,
        }
    }
}

/// The state of an item's cached thumbnail.
pub enum Thumb {
    Pending,
    Ready(PathBuf),
    Failed,
}

/// How long to wait before trying a remote vault's server again.
const RETRY: Duration = Duration::from_secs(10);

/// A remote vault's connection, and where keeping in step with its server stands.
struct RemoteVault {
    access: RemoteAccess,
    /// The server's version of the library this computer's copy matches; `None` until fetched.
    version: Option<u64>,
    /// Whether the server answered when last asked for changes. While it doesn't, the version is kept, so once it
    /// answers again only what changed meanwhile is fetched rather than the whole library.
    reachable: bool,
    /// Changes made here, in order, still to be sent.
    outgoing: VecDeque<Patch>,
    sending: bool,
    /// Covers chosen on the server, by their path there, and where they're cached here.
    covers: HashMap<PathBuf, PathBuf>,
    fetching_covers: HashSet<PathBuf>,
    /// How uploading is going, while files are being uploaded.
    uploads: Option<UploadProgress>,
    upload_queue: VecDeque<Upload>,
    /// Bytes of the file being uploaded sent so far, counted on the thread sending it.
    upload_sent: Arc<AtomicU64>,
}

/// A file waiting to be uploaded: where it goes on the server, where it is here, and how large it is.
struct Upload {
    relative: String,
    local: PathBuf,
    size: u64,
}

/// How uploading to a remote vault is going.
#[derive(Clone, Debug, Default)]
pub struct UploadProgress {
    /// Files finished (sent or failed), of how many.
    pub done: usize,
    pub files: usize,
    /// Where the file being sent goes on the server.
    pub current: String,
    /// Bytes sent, of how many. (As [`LibraryModel::upload_progress`] gives it, this counts the file being sent so
    /// far as well as the finished ones.)
    pub finished_bytes: u64,
    pub bytes: u64,
    pub failed: Vec<String>,
}

impl RemoteVault {
    fn new(access: RemoteAccess) -> Self {
        Self {
            access,
            version: None,
            reachable: true,
            outgoing: VecDeque::new(),
            sending: false,
            covers: HashMap::new(),
            fetching_covers: HashSet::new(),
            uploads: None,
            upload_queue: VecDeque::new(),
            upload_sent: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// What viewers show: a result and the page of it (an index into [`LibraryModel::entry_pages`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Open {
    pub entry: Entry,
    pub page: usize,
}

/// One page the viewers show: a whole file, or page `index` (from 0) of a document.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Page {
    pub item: ItemId,
    pub index: u32,
}

struct Snapshot {
    label: String,
    state: LibrarySnapshot,
}

/// The library and everything the editors share about it: the search, its
/// results, the selection, what's open in the viewers, thumbnails, and undo.
/// Editors observe it and change it only through its methods.
pub struct LibraryModel {
    lib: Library,
    query: String,
    results: Vec<Entry>,
    index: HashMap<Entry, usize>,
    selection: Vec<Entry>,
    cursor: Option<Entry>,
    anchor: Option<Entry>,
    open: Option<Open>,
    tag_counts: Vec<(String, usize)>,
    recent_tags: Vec<String>,
    thumbs: HashMap<ItemId, Thumb>,
    thumb_queue: VecDeque<ItemId>,
    thumbs_running: bool,
    dims_dirty: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    scans: usize,
    message: Option<SharedString>,
    /// The graphics card video decoder this computer has, once found.
    hardware: Option<HardwareDecoder>,
    vaults: Vaults,
    /// Bumped whenever another vault opens, so scans and thumbnails started for the last one are dropped.
    vault_serial: u64,
    /// The open vault's server, when it's a remote vault.
    remote: Option<RemoteVault>,
    /// The collection whose volumes the results show, instead of everything.
    inside: Option<CollectionId>,
    viewer_focus: bool,
}

const UNDO_LIMIT: usize = 100;

impl LibraryModel {
    /// Opens the current vault's library, adds `extra_roots` to it, starts a rescan and starts looking for a graphics
    /// card video decoder.
    pub fn new(extra_roots: Vec<PathBuf>, cx: &mut Context<Self>) -> Self {
        let vaults = Vaults::load();
        let mut lib = Library::load(vaults.current().folder().join("library.json"));
        for root in extra_roots {
            lib.add_root(root);
        }
        let mut this = Self {
            lib,
            vaults,
            vault_serial: 0,
            remote: None,
            query: String::new(),
            results: Vec::new(),
            index: HashMap::new(),
            selection: Vec::new(),
            cursor: None,
            anchor: None,
            open: None,
            tag_counts: Vec::new(),
            recent_tags: Vec::new(),
            thumbs: HashMap::new(),
            thumb_queue: VecDeque::new(),
            thumbs_running: false,
            dims_dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            scans: 0,
            message: None,
            hardware: None,
            inside: None,
            viewer_focus: false,
        };
        this.detect_hardware(cx);
        this.start_library(cx);
        this
    }

    /// Shows the library just opened, then makes its thumbnails and rescans its folders, or for a remote vault,
    /// connects to its server (showing the copy saved here meanwhile).
    fn start_library(&mut self, cx: &mut Context<Self>) {
        self.remote = None;
        if let VaultKind::Remote { address, fingerprint, token, cache_mb } = self.vaults.current().kind.clone() {
            let cache = Cache::new(self.vaults.current().folder().join("cache"), cache_mb << 20);
            match RemoteClient::new(&address, &fingerprint, &token) {
                Ok(client) => {
                    self.remote = Some(RemoteVault::new(RemoteAccess {
                        client: Arc::new(client),
                        cache: Arc::new(cache),
                    }));
                    self.sync_remote(cx);
                }
                Err(error) => self.set_message(format!("{} can't be opened: {error}", self.vaults.current().name)),
            }
            self.refresh(cx);
            self.queue_all_thumbs(cx);
            return;
        }
        self.refresh(cx);
        self.queue_all_thumbs(cx);
        let roots = self.lib.roots.clone();
        self.scan(roots, cx);
    }

    /// The open vault's server and cache, when it's a remote vault.
    pub fn remote_access(&self) -> Option<RemoteAccess> {
        self.remote.as_ref().map(|remote| remote.access.clone())
    }

    /// Whether the open vault is remote and its server can't be reached right now.
    pub fn remote_offline(&self) -> bool {
        self.remote.as_ref().is_some_and(|remote| remote.version.is_none() || !remote.reachable)
    }

    /// Keeps a remote vault's library in step with its server's: fetches it, then waits for changes and makes them
    /// here, until another vault opens. When the server can't be reached it tries again every little while.
    fn sync_remote(&mut self, cx: &mut Context<Self>) {
        let serial = self.vault_serial;
        cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |this, _| {
                    let remote = this.remote.as_ref().filter(|_| this.vault_serial == serial)?;
                    Some((remote.access.clone(), remote.version))
                });
                let Ok(Some((access, version))) = next else { return };
                let reached = match version {
                    None => {
                        let fetched = cx.background_spawn(async move { access.client.library() }).await;
                        this.update(cx, |this, cx| this.remote_library(serial, fetched, cx)).unwrap_or(false)
                    }
                    Some(since) => {
                        let changes = cx.background_spawn(async move { access.client.changes(since) }).await;
                        this.update(cx, |this, cx| this.remote_changes(serial, since, changes, cx)).unwrap_or(false)
                    }
                };
                if !reached {
                    cx.background_executor().timer(RETRY).await;
                }
            }
        })
        .detach();
    }

    /// Takes the library fetched from the server in place of the copy here. Returns whether it was fetched.
    fn remote_library(&mut self, serial: u64, fetched: Result<LibraryState, RemoteError>, cx: &mut Context<Self>) -> bool {
        if self.vault_serial != serial {
            return true;
        }
        let name = self.vaults.current().name.clone();
        let state = match fetched {
            Ok(state) => state,
            Err(error) => {
                self.set_message(format!("{name} can't be reached, so this is the copy saved here: {error}"));
                cx.notify();
                return false;
            }
        };
        let file = self.vaults.current().folder().join("library.json");
        self.lib = Library::adopt(state.library, file, self.lib.settings.clone());
        self.lib.save();
        self.undo.clear();
        self.redo.clear();
        self.thumbs.retain(|_, thumb| matches!(thumb, Thumb::Ready(_)));
        if let Some(remote) = &mut self.remote {
            remote.version = Some(state.version);
            remote.reachable = true;
            remote.outgoing.clear();
        }
        self.refresh(cx);
        self.queue_all_thumbs(cx);
        self.fetch_remote_covers(cx);
        true
    }

    /// Makes here the changes made on the server since version `since`. Returns whether the server answered.
    fn remote_changes(&mut self, serial: u64, since: u64, changes: Result<Changes, RemoteError>, cx: &mut Context<Self>) -> bool {
        if self.vault_serial != serial {
            return true;
        }
        let changes = match changes {
            Ok(changes) => changes,
            Err(error) => {
                if let Some(remote) = &mut self.remote {
                    remote.reachable = false;
                }
                self.set_message(format!("{} can't be reached: {error}", self.vaults.current().name));
                cx.notify();
                return false;
            }
        };
        if self.remote.as_ref().is_some_and(|remote| !remote.reachable) {
            self.set_message(format!("{} can be reached again", self.vaults.current().name));
            cx.notify();
        }
        let Some(remote) = &mut self.remote else { return true };
        remote.reachable = true;
        let Some(current) = remote.version else { return true };
        if changes.reload {
            remote.version = None;
            return true;
        }
        // Patches this computer sent and the server already counted in `current` come back too; they're skipped.
        let fresh: Vec<Patch> = changes.patches.into_iter().skip(current.saturating_sub(since) as usize).collect();
        remote.version = Some(current.max(changes.version));
        if fresh.is_empty() {
            return true;
        }
        for patch in &fresh {
            for item in &patch.items {
                if self.lib.item(item.id).is_some_and(|known| known.modified != item.modified) {
                    self.thumbs.remove(&item.id);
                }
            }
            self.lib.apply_patch(patch);
        }
        self.lib.save();
        // Undoing to a state from before these changes would take them away again.
        self.undo.clear();
        self.redo.clear();
        self.refresh(cx);
        self.queue_all_thumbs(cx);
        self.fetch_remote_covers(cx);
        true
    }

    /// Sends a change made here to the server, after any still waiting to go.
    fn send_to_remote(&mut self, patch: Patch, cx: &mut Context<Self>) {
        if let Some(remote) = &mut self.remote {
            remote.outgoing.push_back(patch);
        }
        self.send_next(cx);
    }

    fn send_next(&mut self, cx: &mut Context<Self>) {
        let serial = self.vault_serial;
        let Some(remote) = &mut self.remote else { return };
        let (Some(base), Some(patch)) = (remote.version, remote.outgoing.front().cloned()) else { return };
        if remote.sending {
            return;
        }
        remote.sending = true;
        let access = remote.access.clone();
        cx.spawn(async move |this, cx| {
            let sent = cx.background_spawn(async move { access.client.send_patch(base, &patch) }).await;
            let retry = this
                .update(cx, |this, cx| {
                    if this.vault_serial != serial {
                        return false;
                    }
                    let name = this.vaults.current().name.clone();
                    let Some(remote) = &mut this.remote else { return false };
                    remote.sending = false;
                    match sent {
                        Ok(version) => {
                            remote.outgoing.pop_front();
                            // Past `base + 1`, others' changes came in between; the change feed brings those.
                            if version == base + 1 {
                                remote.version = Some(version);
                            }
                            this.send_next(cx);
                            false
                        }
                        Err(RemoteError::Unreachable(_)) => true,
                        Err(RemoteError::Conflict) => {
                            remote.outgoing.clear();
                            remote.version = None;
                            this.set_message(format!(
                                "{name} was changed elsewhere at the same moment, so your last change wasn't kept; it's been reloaded"
                            ));
                            cx.notify();
                            false
                        }
                        Err(error) => {
                            remote.outgoing.pop_front();
                            this.set_message(format!("{name} didn't take that change: {error}"));
                            cx.notify();
                            this.send_next(cx);
                            false
                        }
                    }
                })
                .unwrap_or(false);
            if retry {
                cx.background_executor().timer(RETRY).await;
                this.update(cx, |this, cx| this.send_next(cx)).ok();
            }
        })
        .detach();
    }

    /// Fetches the covers chosen in a remote vault that aren't in its cache yet.
    fn fetch_remote_covers(&mut self, cx: &mut Context<Self>) {
        let Some(remote) = &mut self.remote else { return };
        let chosen: BTreeSet<PathBuf> = self
            .lib
            .items
            .values()
            .filter_map(|item| item.cover.clone())
            .chain(self.lib.groups.values().filter_map(|group| group.cover.clone()))
            .chain(self.lib.collections.values().filter_map(|collection| collection.cover.clone()))
            .filter(|cover| !remote.covers.contains_key(cover) && !remote.fetching_covers.contains(cover))
            .collect();
        if chosen.is_empty() {
            return;
        }
        remote.fetching_covers.extend(chosen.iter().cloned());
        let (access, serial) = (remote.access.clone(), self.vault_serial);
        cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_spawn(async move {
                    chosen.into_iter().map(|cover| (access.cover(&cover).ok(), cover)).collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(remote) = this.remote.as_mut().filter(|_| this.vault_serial == serial) else { return };
                for (local, cover) in fetched {
                    remote.fetching_covers.remove(&cover);
                    if let Some(local) = local {
                        remote.covers.insert(cover, local);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Uploads `paths` (files, or folders of them, keeping their layout) to the open remote vault's server, which
    /// adds them to the library once they're there. Files dropped while others are uploading join the same queue.
    fn upload(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(remote) = &mut self.remote else { return };
        let mut files = Vec::new();
        for path in paths {
            let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
            for entry in walkdir::WalkDir::new(&path).into_iter().filter_map(Result::ok) {
                let file = entry.path();
                if entry.file_type().is_file()
                    && MediaKind::from_path(file).is_some()
                    && let Ok(relative) = file.strip_prefix(&base)
                {
                    let relative: Vec<String> = relative.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
                    let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                    files.push(Upload {
                        relative: relative.join("/"),
                        local: file.to_path_buf(),
                        size,
                    });
                }
            }
        }
        if files.is_empty() {
            self.set_message("There were no pictures, videos or documents to upload");
            cx.notify();
            return;
        }
        let starting = remote.uploads.is_none();
        let progress = remote.uploads.get_or_insert_with(UploadProgress::default);
        progress.files += files.len();
        progress.bytes += files.iter().map(|file| file.size).sum::<u64>();
        remote.upload_queue.extend(files);
        if starting {
            self.run_uploads(cx);
        }
        cx.notify();
    }

    /// Uploads the queued files one after another until none are left, redrawing a few times a second meanwhile so
    /// the progress shown keeps up.
    fn run_uploads(&mut self, cx: &mut Context<Self>) {
        let serial = self.vault_serial;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(150)).await;
                let uploading = this.update(cx, |this, cx| {
                    cx.notify();
                    this.vault_serial == serial && this.remote.as_ref().is_some_and(|remote| remote.uploads.is_some())
                });
                if !matches!(uploading, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |this, _| {
                    let remote = this.remote.as_mut().filter(|_| this.vault_serial == serial)?;
                    let upload = remote.upload_queue.pop_front()?;
                    remote.upload_sent.store(0, Ordering::Relaxed);
                    if let Some(progress) = &mut remote.uploads {
                        progress.current = upload.relative.clone();
                    }
                    Some((remote.access.clone(), remote.upload_sent.clone(), upload))
                });
                let Ok(Some((access, sent, upload))) = next else { break };
                let (relative, local) = (upload.relative.clone(), upload.local.clone());
                let result = cx
                    .background_spawn(async move { access.client.upload(&relative, &local, move |bytes| sent.store(bytes, Ordering::Relaxed)) })
                    .await;
                let going = this.update(cx, |this, _| {
                    let Some(progress) = this.remote.as_mut().filter(|_| this.vault_serial == serial).and_then(|remote| remote.uploads.as_mut())
                    else {
                        return false;
                    };
                    progress.done += 1;
                    progress.finished_bytes += upload.size;
                    if let Err(error) = result {
                        progress.failed.push(format!("{}: {error}", upload.relative));
                    }
                    true
                });
                if !matches!(going, Ok(true)) {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                let Some(progress) = this.remote.as_mut().filter(|_| this.vault_serial == serial).and_then(|remote| remote.uploads.take())
                else {
                    return;
                };
                let files = progress.files;
                this.set_message(match progress.failed.first() {
                    None => format!("Uploaded {files} files; they appear once the server has looked at them"),
                    Some(first) => format!("{} of {files} files couldn't be uploaded, such as {first}", progress.failed.len()),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// How uploading to the open remote vault is going, while it is.
    pub fn upload_progress(&self) -> Option<UploadProgress> {
        let remote = self.remote.as_ref()?;
        let mut progress = remote.uploads.clone()?;
        progress.finished_bytes += remote.upload_sent.load(Ordering::Relaxed);
        Some(progress)
    }

    pub fn vaults(&self) -> &Vaults {
        &self.vaults
    }

    /// Closes the open vault (saving what's unsaved) and opens vault `id`.
    pub fn switch_vault(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.vaults.get(id).is_none() || (self.vaults.current == id && self.vault_serial > 0) {
            return;
        }
        self.save_if_needed();
        self.vaults.current = id.to_string();
        self.vaults.save();
        self.vault_serial += 1;
        self.lib = Library::load(self.vaults.current().folder().join("library.json"));
        self.query.clear();
        self.results.clear();
        self.index.clear();
        self.selection.clear();
        self.cursor = None;
        self.anchor = None;
        self.open = None;
        self.recent_tags.clear();
        self.thumbs.clear();
        self.thumb_queue.clear();
        self.dims_dirty = false;
        self.undo.clear();
        self.redo.clear();
        self.scans = 0;
        self.inside = None;
        self.set_message(format!("Opened {}", self.vaults.current().name));
        self.start_library(cx);
    }

    /// Makes a new, empty local vault named `name` and opens it.
    pub fn create_vault(&mut self, name: &str, cx: &mut Context<Self>) {
        if name.trim().is_empty() {
            return;
        }
        let id = self.vaults.add(name, VaultKind::Local);
        self.vaults.save();
        self.switch_vault(&id, cx);
    }

    /// Adds the Sortier Remote server at `address` as a vault named `name`, trusting its certificate `fingerprint` and
    /// logged in with `token`, and opens it.
    pub fn add_remote_vault(&mut self, name: &str, address: &str, fingerprint: &str, token: &str, cx: &mut Context<Self>) {
        let kind = VaultKind::Remote {
            address: address.trim().to_string(),
            fingerprint: fingerprint.to_string(),
            token: token.to_string(),
            cache_mb: sortier_library::vault::default_cache_mb(),
        };
        let name = if name.trim().is_empty() { address.trim() } else { name.trim() };
        let id = self.vaults.add(name, kind);
        self.vaults.save();
        self.switch_vault(&id, cx);
    }

    /// Sets how much of remote vault `id` is cached, from when it next opens.
    pub fn set_cache_size(&mut self, id: &str, megabytes: u64, cx: &mut Context<Self>) {
        self.vaults.set_cache_mb(id, megabytes);
        self.vaults.save();
        self.set_message("The new cache size is used from the next time this vault opens");
        cx.notify();
    }

    pub fn rename_vault(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        self.vaults.rename(id, name);
        self.vaults.save();
        cx.notify();
    }

    /// Forgets vault `id` and Sortier's data for it (never the files in its folders), opening another if it was open.
    pub fn remove_vault(&mut self, id: &str, cx: &mut Context<Self>) {
        let was_open = self.vaults.current == id;
        if !self.vaults.remove(id) {
            self.set_message("The last vault can't be removed");
            cx.notify();
            return;
        }
        self.vaults.save();
        if was_open {
            let next = self.vaults.current.clone();
            self.vaults.current = String::new();
            self.switch_vault(&next, cx);
        } else {
            cx.notify();
        }
    }

    pub fn library(&self) -> &Library {
        &self.lib
    }

    pub fn settings(&self) -> &Settings {
        &self.lib.settings
    }

    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.lib.item(id)
    }

    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.lib.group(id)
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn results(&self) -> &[Entry] {
        &self.results
    }

    pub fn result_index(&self, entry: Entry) -> Option<usize> {
        self.index.get(&entry).copied()
    }

    pub fn selection(&self) -> &[Entry] {
        &self.selection
    }

    pub fn is_selected(&self, entry: Entry) -> bool {
        self.selection.contains(&entry)
    }

    pub fn cursor(&self) -> Option<Entry> {
        self.cursor
    }

    pub fn open(&self) -> Option<Open> {
        self.open
    }

    pub fn tag_counts(&self) -> &[(String, usize)] {
        &self.tag_counts
    }

    pub fn recent_tags(&self) -> &[String] {
        &self.recent_tags
    }

    pub fn is_scanning(&self) -> bool {
        self.scans > 0
    }

    pub fn thumbnails_left(&self) -> usize {
        self.thumb_queue.len()
    }

    /// A short note about the last thing that happened (a scan finishing, a
    /// sequence being made), shown by the library grid until the next one.
    pub fn message(&self) -> Option<&SharedString> {
        self.message.as_ref()
    }

    /// The graphics card video decoder this computer has, if one was found.
    pub fn hardware_decoder(&self) -> Option<HardwareDecoder> {
        self.hardware
    }

    pub fn thumb(&self, id: ItemId) -> Option<&PathBuf> {
        match self.thumbs.get(&id) {
            Some(Thumb::Ready(path)) => Some(path),
            _ => None,
        }
    }

    pub fn thumb_failed(&self, id: ItemId) -> bool {
        matches!(self.thumbs.get(&id), Some(Thumb::Failed))
    }

    pub fn entry_items(&self, entry: Entry) -> Vec<ItemId> {
        match entry {
            Entry::Item(id) => vec![id],
            Entry::Group(group) => self.lib.group(group).map(|group| group.items.clone()).unwrap_or_default(),
            Entry::Collection(collection) => self.lib.collection_items(collection),
        }
    }

    /// The pages of `entry` in order: its files, with each document spread into its pages (one page until its
    /// page count is known).
    pub fn entry_pages(&self, entry: Entry) -> Vec<Page> {
        let mut pages = Vec::new();
        for item in self.entry_items(entry) {
            let count = self
                .lib
                .item(item)
                .filter(|found| found.kind == MediaKind::Document)
                .and_then(|document| document.pages)
                .unwrap_or(1)
                .max(1);
            pages.extend((0..count).map(|index| Page { item, index }));
        }
        pages
    }

    pub fn entry_cover(&self, entry: Entry) -> Option<ItemId> {
        self.entry_items(entry).first().copied()
    }

    pub fn entry_title(&self, entry: Entry) -> String {
        match entry {
            Entry::Item(id) => self.lib.item(id).map(Item::file_name).unwrap_or_default(),
            Entry::Group(group) => self.lib.group(group).map(|group| group.name.clone()).unwrap_or_default(),
            Entry::Collection(collection) => {
                self.lib.collection(collection).map(|collection| collection.name.clone()).unwrap_or_default()
            }
        }
    }

    /// How many volumes a collection has.
    pub fn volume_count(&self, collection: CollectionId) -> usize {
        self.lib.collection(collection).map_or(0, |collection| collection.members.len())
    }

    /// Every tag on `entry`: a sequence's own tags and its pages', or a file's
    /// own tags and its sequence's, or a collection's and everything in it.
    pub fn entry_tags(&self, entry: Entry) -> BTreeSet<String> {
        let mut tags = BTreeSet::new();
        match entry {
            Entry::Collection(id) => {
                if let Some(collection) = self.lib.collection(id) {
                    tags.extend(collection.tags.iter().cloned());
                    for member in &collection.members {
                        if let Member::Group(group) = member
                            && let Some(group) = self.lib.group(*group)
                        {
                            tags.extend(group.tags.iter().cloned());
                        }
                    }
                }
                for id in self.lib.collection_items(id) {
                    if let Some(item) = self.lib.item(id) {
                        tags.extend(item.tags.iter().cloned());
                    }
                }
            }
            Entry::Group(group) => {
                if let Some(group) = self.lib.group(group) {
                    tags.extend(group.tags.iter().cloned());
                    for id in &group.items {
                        if let Some(item) = self.lib.item(*id) {
                            tags.extend(item.tags.iter().cloned());
                        }
                    }
                }
            }
            Entry::Item(id) => {
                if let Some(item) = self.lib.item(id) {
                    tags.extend(item.tags.iter().cloned());
                    if let Some(group) = self.lib.group_of(item) {
                        tags.extend(group.tags.iter().cloned());
                    }
                }
            }
        }
        tags
    }

    /// Tags on the selection with how many selected entries carry each.
    pub fn selection_tags(&self) -> Vec<(String, usize)> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for entry in &self.selection {
            for tag in self.entry_tags(*entry) {
                *counts.entry(tag).or_default() += 1;
            }
        }
        let mut tags: Vec<_> = counts.into_iter().collect();
        tags.sort_by(|a, b| tag_order(&a.0, &b.0));
        tags
    }

    /// Existing tags to offer while typing `text`: ones starting with its last
    /// word first, then ones containing it, most used first, leaving out `present`.
    pub fn suggestions(&self, text: &str, present: &BTreeSet<String>, limit: usize) -> Vec<String> {
        let last = text.rsplit(char::is_whitespace).next().unwrap_or("");
        let word = last.trim_start_matches(['-', '!']).to_lowercase();
        if word.is_empty() {
            return Vec::new();
        }
        let mut starts = Vec::new();
        let mut contains = Vec::new();
        for (tag, _) in &self.tag_counts {
            if present.contains(tag) || *tag == word {
                continue;
            }
            let bare = tag.split_once(':').map(|(_, name)| name).unwrap_or(tag);
            if tag.starts_with(&word) || bare.starts_with(&word) {
                starts.push(tag.clone());
            } else if tag.contains(&word) {
                contains.push(tag.clone());
            }
        }
        starts.extend(contains);
        starts.truncate(limit);
        starts
    }

    pub fn can_undo(&self) -> Option<&str> {
        self.undo.last().map(|snapshot| snapshot.label.as_str())
    }

    pub fn can_redo(&self) -> Option<&str> {
        self.redo.last().map(|snapshot| snapshot.label.as_str())
    }

    fn set_message(&mut self, message: impl Into<SharedString>) {
        self.message = Some(message.into());
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.tag_counts = self.lib.tag_counts();
        self.refresh_results(cx);
    }

    fn refresh_results(&mut self, cx: &mut Context<Self>) {
        let query = Query::parse(&self.query);
        let group_view = self.lib.settings.group_view;
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        if let Some(inside) = self.inside.filter(|inside| self.lib.collection(*inside).is_some()) {
            self.results = self.volumes_matching(inside, &query);
        } else {
            self.inside = None;
            for item in self.lib.items.values() {
                let group = self.lib.group_of(item);
                let collection = self.lib.collection_of(item);
                if !query.matches(item, group, collection) {
                    continue;
                }
                let entry = match (collection, group) {
                    (Some(collection), _) if group_view => Entry::Collection(collection.id),
                    (_, Some(group)) if group_view => Entry::Group(group.id),
                    _ => Entry::Item(item.id),
                };
                if seen.insert(entry) {
                    entries.push(entry);
                }
            }
            self.results = self.sorted(entries);
        }
        self.index = self.results.iter().enumerate().map(|(index, entry)| (*entry, index)).collect();
        let index = &self.index;
        self.selection.retain(|entry| index.contains_key(entry));
        if self.cursor.is_some_and(|entry| !index.contains_key(&entry)) {
            self.cursor = self.selection.first().copied();
        }
        if self.anchor.is_some_and(|entry| !index.contains_key(&entry)) {
            self.anchor = self.cursor;
        }
        if let Some(open) = self.open {
            let pages = self.entry_pages(open.entry).len();
            if pages == 0 {
                self.open = None;
            } else {
                self.open = Some(Open {
                    page: open.page.min(pages - 1),
                    ..open
                });
            }
        }
        cx.notify();
    }

    /// The volumes of collection `inside` that hold anything matching `query`, in the collection's order.
    fn volumes_matching(&self, inside: CollectionId, query: &Query) -> Vec<Entry> {
        let Some(collection) = self.lib.collection(inside) else { return Vec::new() };
        collection
            .members
            .iter()
            .filter(|member| {
                self.lib.member_items(**member).iter().filter_map(|id| self.lib.item(*id)).any(|item| {
                    query.matches(item, self.lib.group_of(item), Some(collection))
                })
            })
            .map(|member| match *member {
                Member::Item(id) => Entry::Item(id),
                Member::Group(group) => Entry::Group(group),
            })
            .collect()
    }

    /// The collection whose volumes the results show, if any.
    pub fn inside(&self) -> Option<CollectionId> {
        self.inside
    }

    /// Shows the volumes of `collection` in the results instead of everything.
    pub fn show_collection(&mut self, collection: CollectionId, cx: &mut Context<Self>) {
        self.inside = Some(collection);
        self.selection.clear();
        self.refresh_results(cx);
        if let Some(first) = self.results.first().copied() {
            self.select_only(first, cx);
        }
    }

    /// Goes back from a collection's volumes to everything, with the collection selected.
    pub fn leave_collection(&mut self, cx: &mut Context<Self>) {
        let Some(collection) = self.inside.take() else { return };
        self.refresh_results(cx);
        let entry = Entry::Collection(collection);
        if self.index.contains_key(&entry) {
            self.select_only(entry, cx);
        }
    }

    /// Orders results by the sort setting. Pages of a sequence stay together
    /// and in their order even when each page is its own tile.
    fn sorted(&self, entries: Vec<Entry>) -> Vec<Entry> {
        enum Key {
            Text(String),
            Number(u64),
        }
        let lib = &self.lib;
        let sort = lib.settings.sort;
        let key_for = |ids: &[ItemId]| -> Key {
            let items = ids.iter().filter_map(|id| lib.item(*id));
            match sort {
                SortKey::Name => Key::Text(
                    ids.first()
                        .and_then(|id| lib.item(*id))
                        .map(|item| item.path.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ),
                SortKey::Modified => Key::Number(items.map(|item| item.modified).max().unwrap_or(0)),
                SortKey::Added => Key::Number(items.map(|item| item.added).min().unwrap_or(0)),
                SortKey::Size => Key::Number(items.map(|item| item.size).sum()),
            }
        };
        let mut group_keys: HashMap<GroupId, usize> = HashMap::new();
        let mut keys: Vec<Key> = Vec::new();
        let mut keyed: Vec<(usize, u64, usize, Entry)> = Vec::with_capacity(entries.len());
        for entry in entries {
            let (key, cluster, position) = match entry {
                Entry::Collection(collection) => {
                    keys.push(key_for(&lib.collection_items(collection)));
                    (keys.len() - 1, collection, 0)
                }
                Entry::Group(group) => {
                    keys.push(key_for(&lib.groups[&group].items));
                    (keys.len() - 1, group, 0)
                }
                Entry::Item(id) => match lib.group_of(&lib.items[&id]) {
                    Some(group) => {
                        let key = *group_keys.entry(group.id).or_insert_with(|| {
                            keys.push(key_for(&group.items));
                            keys.len() - 1
                        });
                        (key, group.id, group.items.iter().position(|page| *page == id).unwrap_or(0))
                    }
                    None => {
                        keys.push(key_for(&[id]));
                        (keys.len() - 1, id, 0)
                    }
                },
            };
            keyed.push((key, cluster, position, entry));
        }
        let descending = lib.settings.sort_desc;
        keyed.sort_by(|a, b| {
            let order = match (&keys[a.0], &keys[b.0]) {
                (Key::Text(x), Key::Text(y)) => natural_cmp(x, y),
                (Key::Number(x), Key::Number(y)) => x.cmp(y),
                _ => std::cmp::Ordering::Equal,
            };
            let order = if descending { order.reverse() } else { order };
            order.then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
        });
        keyed.into_iter().map(|(_, _, _, entry)| entry).collect()
    }

    pub fn set_query(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.query != text {
            self.query = text.to_string();
            self.refresh_results(cx);
        }
    }

    /// Adds `tag` to the search, or excludes it, or takes it out again when it's already there that way.
    pub fn toggle_query_tag(&mut self, tag: &str, exclude: bool, cx: &mut Context<Self>) {
        let query = query::toggle_tag_in_query(&self.query, tag, exclude);
        self.set_query(&query, cx);
    }

    /// Whether `tag` is in the search (`Some(false)`) or excluded (`Some(true)`).
    pub fn query_tag_state(&self, tag: &str) -> Option<bool> {
        query::tag_state_in_query(&self.query, tag)
    }

    pub fn update_settings(&mut self, change: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let before = (self.lib.settings.group_view, self.lib.settings.sort, self.lib.settings.sort_desc);
        change(&mut self.lib.settings);
        self.lib.save();
        let after = (self.lib.settings.group_view, self.lib.settings.sort, self.lib.settings.sort_desc);
        if before.0 != after.0 {
            self.selection = self.selection.iter().map(|entry| self.across_views(*entry)).collect();
            self.selection.dedup();
            self.cursor = self.cursor.map(|entry| self.across_views(entry));
            self.anchor = self.cursor;
        }
        if before != after {
            self.refresh_results(cx);
        } else {
            cx.notify();
        }
    }

    /// The same thing seen with sequences shown the other way: a sequence
    /// becomes its first page, a page becomes its sequence.
    fn across_views(&self, entry: Entry) -> Entry {
        let group_view = self.lib.settings.group_view;
        match entry {
            Entry::Collection(_) if !group_view => self.entry_cover(entry).map(Entry::Item).unwrap_or(entry),
            Entry::Group(group) if !group_view => self
                .lib
                .group(group)
                .and_then(|group| group.items.first().copied())
                .map(Entry::Item)
                .unwrap_or(entry),
            Entry::Item(id) if group_view => match self.lib.item(id) {
                Some(item) => match (self.lib.collection_of(item), item.group) {
                    (Some(collection), _) => Entry::Collection(collection.id),
                    (None, Some(group)) => Entry::Group(group),
                    (None, None) => entry,
                },
                None => entry,
            },
            _ => entry,
        }
    }

    pub fn set_fit(&mut self, fit: FitMode, cx: &mut Context<Self>) {
        self.update_settings(|settings| settings.fit = fit, cx);
    }

    pub fn select_only(&mut self, entry: Entry, cx: &mut Context<Self>) {
        self.selection = vec![entry];
        self.cursor = Some(entry);
        self.anchor = Some(entry);
        cx.notify();
    }

    pub fn toggle_selected(&mut self, entry: Entry, cx: &mut Context<Self>) {
        match self.selection.iter().position(|selected| *selected == entry) {
            Some(position) => {
                self.selection.remove(position);
            }
            None => self.selection.push(entry),
        }
        self.cursor = Some(entry);
        self.anchor = Some(entry);
        cx.notify();
    }

    /// Selects every result between the anchor and `entry`.
    pub fn select_range(&mut self, entry: Entry, cx: &mut Context<Self>) {
        let Some(to) = self.result_index(entry) else { return };
        let from = self.anchor.and_then(|anchor| self.result_index(anchor)).unwrap_or(to);
        let (start, end) = (from.min(to), from.max(to));
        self.selection = self.results[start..=end].to_vec();
        self.cursor = Some(entry);
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selection = self.results.clone();
        self.anchor = self.results.first().copied();
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selection.clear();
        cx.notify();
    }

    /// Moves the keyboard cursor through the results by `delta` (a row is
    /// `delta = columns`), selecting what it lands on or extending the selection.
    /// Returns the result index it landed on.
    pub fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) -> Option<usize> {
        if self.results.is_empty() {
            return None;
        }
        let last = self.results.len() as isize - 1;
        let to = match self.cursor.and_then(|entry| self.result_index(entry)) {
            None => 0,
            Some(at) => (at as isize + delta).clamp(0, last) as usize,
        };
        let entry = self.results[to];
        if extend {
            if self.anchor.is_none() {
                self.anchor = self.cursor.or(Some(entry));
            }
            self.select_range(entry, cx);
        } else {
            self.select_only(entry, cx);
        }
        Some(to)
    }

    /// Opens `entry` in the viewers at `page` (clamped) and selects it.
    pub fn open_entry(&mut self, entry: Entry, page: usize, cx: &mut Context<Self>) {
        let pages = self.entry_pages(entry).len();
        if pages == 0 {
            return;
        }
        self.open = Some(Open {
            entry,
            page: page.min(pages - 1),
        });
        if self.index.contains_key(&entry) {
            self.select_only(entry, cx);
        }
        cx.notify();
    }

    /// Opens `entry` and asks the next viewer that draws to take keyboard focus,
    /// so page keys work straight away after opening something from a list.
    pub fn open_in_viewer(&mut self, entry: Entry, cx: &mut Context<Self>) {
        self.viewer_focus = true;
        self.open_entry(entry, 0, cx);
    }

    /// Whether a viewer should take focus; clears the request.
    pub fn take_viewer_focus(&mut self) -> bool {
        std::mem::take(&mut self.viewer_focus)
    }

    pub fn current_pages(&self) -> Vec<Page> {
        self.open.map(|open| self.entry_pages(open.entry)).unwrap_or_default()
    }

    /// The page the viewers show now.
    pub fn open_page(&self) -> Option<Page> {
        let open = self.open?;
        self.entry_pages(open.entry).get(open.page).copied()
    }

    /// The file the viewers show now (for a document, the whole document).
    pub fn current_page(&self) -> Option<&Item> {
        self.lib.item(self.open_page()?.item)
    }

    /// Whether two pages show side by side right now: the setting is on and
    /// this page and the next are both pictures.
    pub fn spread_active(&self) -> bool {
        let Some(open) = self.open else { return false };
        let pages = self.entry_pages(open.entry);
        self.lib.settings.spread
            && open.page + 1 < pages.len()
            && [pages[open.page], pages[open.page + 1]]
                .iter()
                .all(|page| self.lib.item(page.item).is_some_and(|item| item.kind != MediaKind::Video))
    }

    /// The next or previous page; past either end of a sequence, the next or
    /// previous result (entering it at its last page when going back).
    pub fn step_page(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(open) = self.open else {
            if let Some(entry) = self.cursor.or(self.results.first().copied()) {
                self.open_entry(entry, 0, cx);
            }
            return;
        };
        let step = if self.spread_active() { 2 } else { 1 };
        let pages = self.entry_pages(open.entry).len();
        if forward {
            if open.page + step < pages {
                self.open = Some(Open { page: open.page + step, ..open });
            } else if open.page + 1 < pages {
                self.open = Some(Open { page: pages - 1, ..open });
            } else {
                self.step_entry(1, false, cx);
                return;
            }
        } else if open.page > 0 {
            self.open = Some(Open {
                page: open.page.saturating_sub(step),
                ..open
            });
        } else {
            self.step_entry(-1, true, cx);
            return;
        }
        cx.notify();
    }

    /// Opens the result `delta` away from the open one.
    pub fn step_entry(&mut self, delta: isize, last_page: bool, cx: &mut Context<Self>) {
        let at = self
            .open
            .and_then(|open| self.result_index(open.entry))
            .or_else(|| self.cursor.and_then(|entry| self.result_index(entry)));
        let next = match at {
            Some(at) => at as isize + delta,
            None => 0,
        };
        if next < 0 || next >= self.results.len() as isize {
            self.set_message(if delta > 0 { "That was the last item" } else { "That was the first item" });
            cx.notify();
            return;
        }
        let entry = self.results[next as usize];
        let page = if last_page { usize::MAX } else { 0 };
        self.open_entry(entry, page, cx);
    }

    pub fn go_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        if let Some(open) = self.open {
            let pages = self.entry_pages(open.entry).len();
            self.open = Some(Open {
                page: page.min(pages.saturating_sub(1)),
                ..open
            });
            cx.notify();
        }
    }

    /// Runs `change` on the library as one undoable step named `label`, then saves.
    fn edit(&mut self, label: &str, cx: &mut Context<Self>, change: impl FnOnce(&mut Library)) {
        if !self.can_edit(cx) {
            return;
        }
        let before = self.lib.snapshot();
        change(&mut self.lib);
        let patch = self.remote.as_ref().map(|_| self.lib.diff(&before));
        self.undo.push(Snapshot {
            label: label.to_string(),
            state: before,
        });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.lib.save();
        if let Some(patch) = patch.filter(|patch| !patch.is_empty()) {
            self.send_to_remote(patch, cx);
        }
        self.refresh(cx);
    }

    /// Whether the library can be changed now: not while a remote vault's server can't be reached, since the change
    /// couldn't be kept there.
    fn can_edit(&mut self, cx: &mut Context<Self>) -> bool {
        if self.remote_offline() {
            self.set_message(format!("{} can't be reached, so it can't be changed right now", self.vaults.current().name));
            cx.notify();
            return false;
        }
        true
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        self.swap_history(true, cx);
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        self.swap_history(false, cx);
    }

    fn swap_history(&mut self, undo: bool, cx: &mut Context<Self>) {
        if !self.can_edit(cx) {
            return;
        }
        let (from, to) = if undo { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        let Some(snapshot) = from.pop() else { return };
        let before = self.lib.snapshot();
        to.push(Snapshot {
            label: snapshot.label.clone(),
            state: before.clone(),
        });
        self.lib.restore(snapshot.state);
        self.lib.save();
        if self.remote.is_some() {
            let patch = self.lib.diff(&before);
            if !patch.is_empty() {
                self.send_to_remote(patch, cx);
            }
        }
        self.set_message(format!("{} {}", if undo { "Undid" } else { "Redid" }, snapshot.label));
        self.refresh(cx);
    }

    fn remember_tags(&mut self, tags: &[String]) {
        for tag in tags.iter().rev() {
            self.recent_tags.retain(|recent| recent != tag);
            self.recent_tags.insert(0, tag.clone());
        }
        self.recent_tags.truncate(12);
    }

    /// Adds the tags typed in `text` to each entry: a sequence's tags go on the
    /// sequence, a file's on the file.
    pub fn add_tags(&mut self, entries: &[Entry], text: &str, cx: &mut Context<Self>) {
        let tags = parse_tags(text);
        if tags.is_empty() || entries.is_empty() {
            return;
        }
        self.remember_tags(&tags);
        let entries = entries.to_vec();
        self.edit("Add Tags", cx, |lib| {
            for entry in &entries {
                match *entry {
                    Entry::Collection(collection) => lib.add_collection_tags(collection, &tags),
                    Entry::Group(group) => lib.add_group_tags(&[group], &tags),
                    Entry::Item(id) => lib.add_item_tags(&[id], &tags),
                }
            }
        });
    }

    /// Removes `tag` from each entry; from a sequence, it also leaves its pages.
    pub fn remove_tag(&mut self, entries: &[Entry], tag: &str, cx: &mut Context<Self>) {
        let entries = entries.to_vec();
        self.edit("Remove Tag", cx, |lib| {
            for entry in &entries {
                match *entry {
                    Entry::Collection(collection) => lib.remove_collection_tag(collection, tag),
                    Entry::Group(group) => lib.remove_group_tag(&[group], tag),
                    Entry::Item(id) => {
                        lib.remove_item_tag(&[id], tag);
                        if let Some(group) = lib.item(id).and_then(|item| item.group) {
                            lib.remove_group_tag(&[group], tag);
                        }
                    }
                }
            }
        });
    }

    pub fn add_page_tags(&mut self, page: ItemId, text: &str, cx: &mut Context<Self>) {
        let tags = parse_tags(text);
        if tags.is_empty() {
            return;
        }
        self.remember_tags(&tags);
        self.edit("Add Tags", cx, |lib| lib.add_item_tags(&[page], &tags));
    }

    pub fn remove_page_tag(&mut self, page: ItemId, tag: &str, cx: &mut Context<Self>) {
        self.edit("Remove Tag", cx, |lib| lib.remove_item_tag(&[page], tag));
    }

    /// Renames a sequence or a collection.
    pub fn rename_entry(&mut self, entry: Entry, name: &str, cx: &mut Context<Self>) {
        if name.trim().is_empty() || self.entry_title(entry) == name.trim() {
            return;
        }
        match entry {
            Entry::Group(group) => self.edit("Rename Sequence", cx, |lib| lib.rename_group(group, name.to_string())),
            Entry::Collection(collection) => self.edit("Rename Collection", cx, |lib| lib.rename_collection(collection, name)),
            Entry::Item(_) => {}
        }
    }

    /// Makes one collection of everything selected (sequences and files, as volumes), in result order. When the
    /// selection holds a collection already, the rest is added to the first one instead.
    pub fn make_collection(&mut self, cx: &mut Context<Self>) {
        let mut selected = self.selection.clone();
        selected.sort_by_key(|entry| self.result_index(*entry).unwrap_or(usize::MAX));
        let existing = selected.iter().find_map(|entry| match entry {
            Entry::Collection(collection) => Some(*collection),
            _ => None,
        });
        let mut members = Vec::new();
        for entry in selected {
            match entry {
                Entry::Collection(collection) => {
                    members.extend(self.lib.collection(collection).map(|collection| collection.members.clone()).unwrap_or_default())
                }
                Entry::Item(id) => match self.lib.item(id).and_then(|item| item.group) {
                    Some(group) => members.push(Member::Group(group)),
                    None => members.push(Member::Item(id)),
                },
                Entry::Group(group) => members.push(Member::Group(group)),
            }
        }
        if members.is_empty() {
            self.set_message("Select the sequences or files to collect");
            cx.notify();
            return;
        }
        if let Some(collection) = existing {
            self.edit("Add to Collection", cx, |lib| lib.add_to_collection(collection, &members));
            self.set_message(format!("The collection has {} volumes now", self.volume_count(collection)));
            return;
        }
        let mut made = None;
        self.edit("Make Collection", cx, |lib| made = lib.create_collection(&members, None));
        if let Some(collection) = made {
            self.set_message(format!("Made a collection of {} volumes", self.volume_count(collection)));
            let entry = Entry::Collection(collection);
            if self.index.contains_key(&entry) {
                self.select_only(entry, cx);
            }
        }
    }

    /// Dissolves a collection; its volumes stay.
    pub fn dissolve_collection(&mut self, collection: CollectionId, cx: &mut Context<Self>) {
        if self.inside == Some(collection) {
            self.inside = None;
        }
        self.edit("Dissolve Collection", cx, |lib| lib.dissolve_collection(collection));
    }

    /// Takes the selected volumes out of the collection the results show.
    pub fn remove_from_collection(&mut self, cx: &mut Context<Self>) {
        let members: Vec<Member> = self.selection.iter().filter_map(|entry| entry.member()).collect();
        if self.inside.is_none() || members.is_empty() {
            return;
        }
        self.edit("Take Out of Collection", cx, |lib| {
            for member in &members {
                lib.remove_from_collection(*member);
            }
        });
    }

    /// Moves the selected volume `delta` places along the collection the results show.
    pub fn move_volume(&mut self, delta: isize, cx: &mut Context<Self>) {
        let (Some(_), [entry]) = (self.inside, self.selection.as_slice()) else { return };
        let Some(member) = entry.member() else { return };
        self.edit("Move Volume", cx, |lib| lib.move_in_collection(member, delta));
    }

    /// Makes one sequence of everything selected, in result order.
    pub fn make_sequence(&mut self, cx: &mut Context<Self>) {
        let mut selected = self.selection.clone();
        selected.sort_by_key(|entry| self.result_index(*entry).unwrap_or(usize::MAX));
        let items: Vec<ItemId> = selected.iter().flat_map(|entry| self.entry_items(*entry)).collect();
        if items.len() < 2 {
            self.set_message("Select two or more files or sequences to make a sequence");
            cx.notify();
            return;
        }
        let mut made = None;
        self.edit("Make Sequence", cx, |lib| made = lib.create_group(&items, None));
        if let Some(group) = made {
            self.set_message(format!("Made a sequence of {} pages", items.len()));
            let entry = if self.lib.settings.group_view { Entry::Group(group) } else { Entry::Item(items[0]) };
            if self.index.contains_key(&entry) {
                self.select_only(entry, cx);
            }
        }
    }

    /// Splits every selected sequence (or the sequence of each selected page) into single files.
    pub fn split_sequences(&mut self, cx: &mut Context<Self>) {
        let groups: BTreeSet<GroupId> = self
            .selection
            .iter()
            .filter_map(|entry| match entry {
                Entry::Group(group) => Some(*group),
                Entry::Item(id) => self.lib.item(*id).and_then(|item| item.group),
                Entry::Collection(_) => None,
            })
            .collect();
        if groups.is_empty() {
            return;
        }
        self.edit("Split Sequence", cx, |lib| {
            for group in &groups {
                lib.ungroup(*group);
            }
        });
        self.set_message(format!("Split {} sequence(s) into single files", groups.len()));
    }

    /// Moves `page` (a file) to position `to` among its sequence's files.
    pub fn move_page_to(&mut self, page: ItemId, to: usize, cx: &mut Context<Self>) {
        let Some(group) = self.lib.item(page).and_then(|item| self.lib.group_of(item)) else { return };
        let Some(from) = group.items.iter().position(|id| *id == page) else { return };
        if from == to {
            return;
        }
        let delta = to as isize - from as isize;
        self.edit("Move Page", cx, |lib| {
            lib.move_in_group(page, delta);
        });
        self.follow_page(page, cx);
    }

    /// The cover the user chose for `entry`, if any.
    pub fn custom_cover(&self, entry: Entry) -> Option<&PathBuf> {
        match entry {
            Entry::Item(id) => self.lib.item(id)?.cover.as_ref(),
            Entry::Group(group) => self.lib.group(group)?.cover.as_ref(),
            Entry::Collection(collection) => self.lib.collection(collection)?.cover.as_ref(),
        }
    }

    /// The cover a tile shows for `entry` when the user chose one: its own, or for a collection without one, its
    /// first volume's.
    pub fn shown_cover(&self, entry: Entry) -> Option<&PathBuf> {
        let chosen = self.chosen_cover(entry)?;
        match &self.remote {
            Some(remote) => remote.covers.get(chosen),
            None => Some(chosen),
        }
    }

    /// The cover chosen for `entry`, or for a collection without one, its first volume's; as the library names it
    /// (on a remote vault, a path on its server).
    fn chosen_cover(&self, entry: Entry) -> Option<&PathBuf> {
        self.custom_cover(entry).or_else(|| match entry {
            Entry::Collection(collection) => {
                let first = *self.lib.collection(collection)?.members.first()?;
                self.custom_cover(match first {
                    Member::Item(id) => Entry::Item(id),
                    Member::Group(group) => Entry::Group(group),
                })
            }
            _ => None,
        })
    }

    /// Gives `entry` the cover saved at `cover`, or with `None` goes back to showing its first page.
    pub fn set_cover(&mut self, entry: Entry, cover: Option<PathBuf>, cx: &mut Context<Self>) {
        let label = if cover.is_some() { "Set Cover" } else { "Reset Cover" };
        self.edit(label, cx, |lib| match entry {
            Entry::Item(id) => {
                if let Some(item) = lib.items.get_mut(&id) {
                    item.cover = cover;
                }
            }
            Entry::Group(group) => {
                if let Some(group) = lib.groups.get_mut(&group) {
                    group.cover = cover;
                }
            }
            Entry::Collection(collection) => {
                if let Some(collection) = lib.collections.get_mut(&collection) {
                    collection.cover = cover;
                }
            }
        });
    }

    /// Makes `entry`'s cover from the picture, video or document at `path` (its first page), in the background.
    pub fn cover_from_file(&mut self, entry: Entry, path: PathBuf, cx: &mut Context<Self>) {
        let Some(kind) = MediaKind::from_path(&path) else {
            self.set_message("Choose a picture, video or document for the cover");
            cx.notify();
            return;
        };
        self.make_cover_from(entry, path, kind, 0, cx);
    }

    /// Makes `entry`'s cover from the page open in the viewers, in the background.
    pub fn cover_from_open_page(&mut self, entry: Entry, cx: &mut Context<Self>) {
        let Some(page) = self.open_page() else { return };
        let Some(item) = self.lib.item(page.item) else { return };
        if let Some(access) = self.remote_access() {
            let id = item.id;
            let task = cx.background_spawn(async move { access.client.cover_from_page(id, page.index).map_err(anyhow::Error::from) });
            self.finish_cover(entry, task, cx);
            return;
        }
        let (path, kind) = (item.path.clone(), item.kind);
        self.make_cover_from(entry, path, kind, page.index, cx);
    }

    fn make_cover_from(&mut self, entry: Entry, path: PathBuf, kind: MediaKind, index: u32, cx: &mut Context<Self>) {
        let access = self.remote_access();
        let task = cx.background_spawn(async move {
            let saved = thumbs::save_cover(&path, kind, index)?;
            match access {
                Some(access) => {
                    let picture = std::fs::read(&saved)?;
                    let _ = std::fs::remove_file(&saved);
                    Ok(access.client.upload_cover(picture)?)
                }
                None => Ok(saved),
            }
        });
        self.finish_cover(entry, task, cx);
    }

    /// Gives `entry` the cover `task` makes, once it has.
    fn finish_cover(&mut self, entry: Entry, task: gpui::Task<anyhow::Result<PathBuf>>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let saved = task.await;
            this.update(cx, |this, cx| match saved {
                Ok(cover) => {
                    this.set_cover(entry, Some(cover), cx);
                    this.fetch_remote_covers(cx);
                }
                Err(error) => {
                    this.set_message(format!("The cover couldn't be made: {error:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Moves `page` (a file) `delta` places along its sequence.
    pub fn move_page_by(&mut self, page: ItemId, delta: isize, cx: &mut Context<Self>) {
        let Some(group) = self.lib.item(page).and_then(|item| self.lib.group_of(item)) else { return };
        let Some(from) = group.items.iter().position(|id| *id == page) else { return };
        let to = (from as isize + delta).clamp(0, group.items.len() as isize - 1) as usize;
        self.move_page_to(page, to, cx);
    }

    /// Moves `page` (a file) to where `onto`, another file of its sequence, is.
    pub fn move_page_onto(&mut self, page: ItemId, onto: ItemId, cx: &mut Context<Self>) {
        let Some(group) = self.lib.item(page).and_then(|item| self.lib.group_of(item)) else { return };
        if let Some(to) = group.items.iter().position(|id| *id == onto) {
            self.move_page_to(page, to, cx);
        }
    }

    pub fn make_cover(&mut self, page: ItemId, cx: &mut Context<Self>) {
        self.move_page_to(page, 0, cx);
    }

    pub fn remove_from_sequence(&mut self, page: ItemId, cx: &mut Context<Self>) {
        let open_group = self.open.and_then(|open| match open.entry {
            Entry::Group(group) => Some(group),
            Entry::Item(_) | Entry::Collection(_) => None,
        });
        self.edit("Remove From Sequence", cx, |lib| lib.remove_from_group(page));
        if let Some(group) = open_group
            && self.lib.group(group).is_none()
        {
            self.open = Some(Open {
                entry: Entry::Item(page),
                page: 0,
            });
        }
        self.refresh_results(cx);
    }

    /// Keeps the viewers on `page` (its first page, for a document) after it moved within its sequence.
    fn follow_page(&mut self, page: ItemId, cx: &mut Context<Self>) {
        if let Some(open) = self.open
            && let Some(position) = self.entry_pages(open.entry).iter().position(|shown| shown.item == page)
        {
            self.open = Some(Open { page: position, ..open });
            cx.notify();
        }
    }

    /// Adds folders to the library, or for a remote vault, uploads them (and any files among `paths`) to its server.
    pub fn add_folders(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.remote.is_some() {
            self.upload(paths, cx);
            return;
        }
        let mut added = Vec::new();
        for path in paths {
            let folder = if path.is_dir() {
                path
            } else if let Some(parent) = path.parent() {
                parent.to_path_buf()
            } else {
                continue;
            };
            if self.lib.add_root(folder.clone()) {
                added.push(folder);
            }
        }
        if added.is_empty() {
            self.set_message("That folder is already in the library");
            cx.notify();
            return;
        }
        self.lib.save();
        self.scan(added, cx);
    }

    pub fn remove_folder(&mut self, folder: &Path, cx: &mut Context<Self>) {
        if self.remote.is_some() {
            self.set_message("A remote vault's folders are set on its server: sortier-remote remove-folder");
            cx.notify();
            return;
        }
        self.lib.remove_root(folder);
        self.lib.save();
        self.undo.clear();
        self.redo.clear();
        self.set_message("Folder removed from the library; its files are untouched");
        self.refresh(cx);
    }

    /// Looks through the folders again, or asks a remote vault's server to.
    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        if let Some(access) = self.remote_access() {
            cx.background_spawn(async move {
                // A server that can't be reached is said so by the syncing already.
                let _ = access.client.rescan();
            })
            .detach();
            self.set_message("The server is looking through its folders");
            cx.notify();
            return;
        }
        let roots = self.lib.roots.clone();
        self.scan(roots, cx);
    }

    fn scan(&mut self, roots: Vec<PathBuf>, cx: &mut Context<Self>) {
        if roots.is_empty() || self.remote.is_some() {
            return;
        }
        self.scans += 1;
        cx.notify();
        let serial = self.vault_serial;
        cx.spawn(async move |this, cx| {
            let to_scan = roots.clone();
            let found = cx.background_spawn(async move { Library::scan_folders(&to_scan) }).await;
            this.update(cx, |this, cx| {
                if this.vault_serial != serial {
                    return;
                }
                let outcome = this.lib.merge_scan(&roots, found);
                this.lib.save();
                // Snapshots taken before files came and went would bring them back.
                this.undo.clear();
                this.redo.clear();
                this.scans -= 1;
                if outcome.added > 0 || outcome.removed > 0 {
                    this.set_message(format!("{} new, {} gone", outcome.added, outcome.removed));
                }
                this.refresh(cx);
                this.queue_all_thumbs(cx);
            })
            .ok();
        })
        .detach();
    }

    fn queue_all_thumbs(&mut self, cx: &mut Context<Self>) {
        let mut ids: Vec<ItemId> = self.results.iter().filter_map(|entry| self.entry_cover(*entry)).collect();
        ids.extend(self.lib.items.keys().copied());
        self.queue_thumbs(ids, cx);
    }

    fn queue_thumbs(&mut self, ids: Vec<ItemId>, cx: &mut Context<Self>) {
        for id in ids {
            let Some(item) = self.lib.item(id) else { continue };
            let expected = thumbs::thumb_path(&item.path, item.modified);
            match self.thumbs.get(&id) {
                Some(Thumb::Ready(_)) if self.remote.is_some() => continue,
                Some(Thumb::Ready(path)) if *path == expected => continue,
                Some(Thumb::Pending | Thumb::Failed) => continue,
                _ => {}
            }
            self.thumbs.insert(id, Thumb::Pending);
            self.thumb_queue.push_back(id);
        }
        self.run_thumbs(cx);
    }

    fn next_thumb_jobs(&mut self, count: usize) -> Vec<thumbs::Job> {
        let mut jobs = Vec::new();
        while jobs.len() < count {
            let Some(id) = self.thumb_queue.pop_front() else { break };
            let Some(item) = self.lib.item(id) else { continue };
            jobs.push(thumbs::Job {
                id,
                src: item.path.clone(),
                kind: item.kind,
                dest: thumbs::thumb_path(&item.path, item.modified),
                need_dims: item.dims.is_none() || (item.kind == MediaKind::Document && item.pages.is_none()),
            });
        }
        jobs
    }

    fn run_thumbs(&mut self, cx: &mut Context<Self>) {
        if self.thumbs_running || self.thumb_queue.is_empty() {
            return;
        }
        if self.remote.is_some() {
            self.fetch_remote_thumbs(cx);
            return;
        }
        self.thumbs_running = true;
        cx.spawn(async move |this, cx| {
            loop {
                let Ok((jobs, serial)) = this.update(cx, |this, _| (this.next_thumb_jobs(48), this.vault_serial)) else { return };
                if jobs.is_empty() {
                    this.update(cx, |this, cx| {
                        this.thumbs_running = false;
                        if std::mem::take(&mut this.dims_dirty) {
                            this.lib.save();
                        }
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                let results = cx.background_spawn(async move { thumbs::run_jobs(jobs) }).await;
                let applied = this.update(cx, |this, cx| {
                    if this.vault_serial != serial {
                        return;
                    }
                    for result in results {
                        if let Some(dims) = result.dims
                            && let Some(item) = this.lib.items.get_mut(&result.id)
                            && item.dims != Some(dims)
                        {
                            item.dims = Some(dims);
                            this.dims_dirty = true;
                        }
                        if let Some(pages) = result.pages {
                            this.set_pages(result.id, pages);
                        }
                        let thumb = if result.ok { Thumb::Ready(result.dest) } else { Thumb::Failed };
                        this.thumbs.insert(result.id, thumb);
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Fetches the queued thumbnails of a remote vault's files from its server (or its cache), several at a time.
    fn fetch_remote_thumbs(&mut self, cx: &mut Context<Self>) {
        self.thumbs_running = true;
        cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |this, _| {
                    let access = this.remote_access()?;
                    let mut items = Vec::new();
                    while items.len() < 32 {
                        let Some(id) = this.thumb_queue.pop_front() else { break };
                        if let Some(item) = this.lib.item(id) {
                            items.push(item.clone());
                        }
                    }
                    Some((access, items, this.vault_serial))
                });
                let Ok(Some((access, items, serial))) = next.map(|next| next.filter(|(_, items, _)| !items.is_empty())) else {
                    this.update(cx, |this, cx| {
                        this.thumbs_running = false;
                        cx.notify();
                    })
                    .ok();
                    return;
                };
                let fetched = cx
                    .background_spawn(async move {
                        let access = &access;
                        std::thread::scope(|scope| {
                            let handles: Vec<_> = items
                                .chunks(4)
                                .map(|chunk| scope.spawn(move || chunk.iter().map(|item| (item.id, access.thumbnail(item).ok())).collect::<Vec<_>>()))
                                .collect();
                            handles.into_iter().flat_map(|handle| handle.join().unwrap_or_default()).collect::<Vec<_>>()
                        })
                    })
                    .await;
                let applied = this.update(cx, |this, cx| {
                    if this.vault_serial != serial {
                        return;
                    }
                    for (id, path) in fetched {
                        this.thumbs.insert(id, path.map_or(Thumb::Failed, Thumb::Ready));
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// The files of everything selected, each once, in result order.
    pub fn selected_items(&self) -> Vec<ItemId> {
        let mut selected = self.selection.clone();
        selected.sort_by_key(|entry| self.result_index(*entry).unwrap_or(usize::MAX));
        let mut seen = HashSet::new();
        selected.iter().flat_map(|entry| self.entry_items(*entry)).filter(|id| seen.insert(*id)).collect()
    }

    /// Erases the files `ids` from disk for good (on the server, for a remote vault) and takes them out of the
    /// library. Undo is cleared, since it would bring back files that are gone.
    pub fn delete_items(&mut self, ids: Vec<ItemId>, cx: &mut Context<Self>) {
        if ids.is_empty() || !self.can_edit(cx) {
            return;
        }
        if let Some(access) = self.remote_access() {
            let task = cx.background_spawn(async move { access.client.delete(&ids).map_err(|error| error.to_string()) });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    // The server's change feed brings the files' removal from the library.
                    match result {
                        Ok(deleted) => this.deleted(deleted.deleted.len(), deleted.failed, cx),
                        Err(error) => this.deleted(0, vec![error], cx),
                    }
                })
                .ok();
            })
            .detach();
            return;
        }
        let files: Vec<(ItemId, PathBuf)> = ids.iter().filter_map(|id| self.lib.item(*id).map(|item| (*id, item.path.clone()))).collect();
        let serial = self.vault_serial;
        let task = cx.background_spawn(async move {
            let (mut deleted, mut failed) = (Vec::new(), Vec::new());
            for (id, path) in files {
                match std::fs::remove_file(&path) {
                    Ok(()) => deleted.push(id),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => deleted.push(id),
                    Err(error) => failed.push(format!("{}: {error}", path.display())),
                }
            }
            (deleted, failed)
        });
        cx.spawn(async move |this, cx| {
            let (deleted, failed) = task.await;
            this.update(cx, |this, cx| {
                if this.vault_serial != serial {
                    return;
                }
                this.lib.remove_items(&deleted);
                for id in &deleted {
                    this.thumbs.remove(id);
                }
                this.lib.save();
                this.undo.clear();
                this.redo.clear();
                this.deleted(deleted.len(), failed, cx);
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Says how deleting went.
    fn deleted(&mut self, count: usize, failed: Vec<String>, cx: &mut Context<Self>) {
        let files = |n: usize| if n == 1 { "1 file".to_string() } else { format!("{n} files") };
        self.set_message(match failed.first() {
            None => format!("Deleted {}", files(count)),
            Some(first) => format!("Deleted {}; {} couldn't be, such as {first}", files(count), files(failed.len())),
        });
        cx.notify();
    }

    /// Opens file `id` in the system's default app for it; a remote vault's file is fetched into its cache first.
    pub fn open_in_default_app(&mut self, id: ItemId, cx: &mut Context<Self>) {
        let Some(item) = self.lib.item(id).cloned() else { return };
        let Some(access) = self.remote_access() else {
            cx.open_with_system(&item.path);
            return;
        };
        self.set_message(format!("Fetching {}\u{2026}", item.file_name()));
        cx.notify();
        let task = cx.background_spawn(async move { access.file(&item) });
        cx.spawn(async move |this, cx| {
            let fetched = task.await;
            this.update(cx, |this, cx| match fetched {
                Ok(path) => cx.open_with_system(&path),
                Err(error) => {
                    this.set_message(format!("The file couldn't be fetched: {error}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Shows file `id` in the system's file manager. A remote vault's files aren't on this computer, so for those it
    /// says so instead.
    pub fn show_in_folder(&mut self, id: ItemId, cx: &mut Context<Self>) {
        if self.remote.is_some() {
            self.set_message("This file is on the server, not this computer");
            cx.notify();
        } else if let Some(item) = self.lib.item(id) {
            cx.reveal_path(&item.path);
        }
    }

    /// Records an item's pixel size learned by a viewer.
    pub fn set_dims(&mut self, id: ItemId, dims: (u32, u32)) {
        if let Some(item) = self.lib.items.get_mut(&id)
            && item.dims != Some(dims)
        {
            item.dims = Some(dims);
            self.dims_dirty = true;
        }
    }

    /// Records a document's page count learned by a viewer or a thumbnail job.
    pub fn set_pages(&mut self, id: ItemId, pages: u32) {
        if let Some(item) = self.lib.items.get_mut(&id)
            && item.pages != Some(pages)
        {
            item.pages = Some(pages);
            self.dims_dirty = true;
        }
    }

    /// The graphics card decoder to play videos with: the one found, while hardware decoding is on.
    pub fn playback_decoder(&self) -> Option<HardwareDecoder> {
        self.hardware.filter(|_| self.lib.settings.hardware_decoding)
    }

    fn detect_hardware(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async { sortier_media::detect_hardware_decoder() }).await;
            this.update(cx, |this, cx| {
                this.hardware = found;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Saves pixel sizes learned since the last save.
    pub fn save_if_needed(&mut self) {
        if std::mem::take(&mut self.dims_dirty) {
            self.lib.save();
        }
    }
}

/// Tags ordered by namespace (artists, series, characters, plain tags, others), then name.
pub fn tag_order(a: &str, b: &str) -> std::cmp::Ordering {
    namespace_rank(a).cmp(&namespace_rank(b)).then_with(|| a.cmp(b))
}

/// The heading a tag is listed under in the browser.
pub fn namespace_title(tag: &str) -> &'static str {
    match tag.split_once(':').map(|(namespace, _)| namespace) {
        Some("artist" | "by" | "creator") => "Artists",
        Some("series" | "copyright" | "parody") => "Series",
        Some("character" | "char") => "Characters",
        Some("meta" | "source") => "Meta",
        Some(_) => "Other",
        None => "Tags",
    }
}

fn namespace_rank(tag: &str) -> u8 {
    match namespace_title(tag) {
        "Artists" => 0,
        "Series" => 1,
        "Characters" => 2,
        "Tags" => 3,
        "Other" => 4,
        _ => 5,
    }
}
