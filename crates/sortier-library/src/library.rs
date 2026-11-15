//! The persistent library: media items, sequences (groups), tags and settings.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

pub use crate::collection::{Collection, CollectionId, Member};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub type ItemId = u64;
pub type GroupId = u64;

pub const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "jfif", "bmp", "tif", "tiff", "webp", "gif",
];
pub const DOCUMENT_EXTS: &[&str] = sortier_media::DOCUMENT_EXTENSIONS;
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "webm", "mkv", "mov", "avi", "wmv", "flv", "mpg", "mpeg",
];

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum MediaKind {
    Image,
    Animated,
    Video,
    /// A file of many pages: a PDF, an EPUB or a comic archive.
    Document,
}

impl MediaKind {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        if ext == "gif" {
            Some(MediaKind::Animated)
        } else if IMAGE_EXTS.contains(&ext.as_str()) {
            Some(MediaKind::Image)
        } else if VIDEO_EXTS.contains(&ext.as_str()) {
            Some(MediaKind::Video)
        } else if DOCUMENT_EXTS.contains(&ext.as_str()) {
            Some(MediaKind::Document)
        } else {
            None
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Item {
    pub id: ItemId,
    pub path: PathBuf,
    pub kind: MediaKind,
    #[serde(default)]
    pub tags: BTreeSet<String>,
    #[serde(default)]
    pub group: Option<GroupId>,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub modified: u64,
    #[serde(default)]
    pub added: u64,
    #[serde(default)]
    pub dims: Option<(u32, u32)>,
    /// How many pages a document has, once read. Everything else is one page.
    #[serde(default)]
    pub pages: Option<u32>,
    /// A cover the user chose, shown instead of the file itself (see [`crate::thumbs::save_cover`]).
    #[serde(default)]
    pub cover: Option<PathBuf>,
}

impl Item {
    /// The file's name. Split at either slash, since a Sortier Remote server's paths may be Windows ones.
    pub fn file_name(&self) -> String {
        let path = self.path.to_string_lossy();
        path.rsplit(['/', '\\']).next().unwrap_or_default().to_string()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    /// Ordered pages of the sequence.
    pub items: Vec<ItemId>,
    #[serde(default)]
    pub tags: BTreeSet<String>,
    /// Set when the group was created by auto-detection; new matching files join it on rescan.
    #[serde(default)]
    pub auto_key: Option<String>,
    /// A cover the user chose, shown instead of the first page (see [`crate::thumbs::save_cover`]).
    #[serde(default)]
    pub cover: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Added,
    Size,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Modified => "Modified",
            SortKey::Added => "Added",
            SortKey::Size => "Size",
        }
    }
    pub fn next(self) -> Self {
        match self {
            SortKey::Name => SortKey::Modified,
            SortKey::Modified => SortKey::Added,
            SortKey::Added => SortKey::Size,
            SortKey::Size => SortKey::Name,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FitMode {
    #[default]
    Fit,
    Width,
    Height,
    Original,
}

impl FitMode {
    pub fn label(self) -> &'static str {
        match self {
            FitMode::Fit => "Fit",
            FitMode::Width => "Fit width",
            FitMode::Height => "Fit height",
            FitMode::Original => "1:1",
        }
    }
    pub fn next(self) -> Self {
        match self {
            FitMode::Fit => FitMode::Width,
            FitMode::Width => FitMode::Height,
            FitMode::Height => FitMode::Original,
            FitMode::Original => FitMode::Fit,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_tile() -> f32 {
    180.0
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    #[serde(default = "default_true")]
    pub auto_group: bool,
    #[serde(default = "default_true")]
    pub group_view: bool,
    #[serde(default = "default_tile")]
    pub tile_size: f32,
    #[serde(default)]
    pub tile_crop: bool,
    #[serde(default)]
    pub sort: SortKey,
    #[serde(default)]
    pub sort_desc: bool,
    #[serde(default)]
    pub fit: FitMode,
    #[serde(default)]
    pub spread: bool,
    #[serde(default)]
    pub rtl: bool,
    /// Sound level for video playback, 0..=1.
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub muted: bool,
    /// Draw magnified pixels as hard squares instead of smoothing them (pixel art).
    #[serde(default)]
    pub pixelated: bool,
    /// Decode videos on the graphics card when it can.
    #[serde(default = "default_true")]
    pub hardware_decoding: bool,
}

fn default_volume() -> f32 {
    0.8
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_group: true,
            group_view: true,
            tile_size: default_tile(),
            tile_crop: false,
            sort: SortKey::Name,
            sort_desc: false,
            fit: FitMode::Fit,
            spread: false,
            rtl: false,
            volume: default_volume(),
            muted: false,
            pixelated: false,
            hardware_decoding: true,
        }
    }
}

/// Everything an edit can change, kept for undo (see [`Library::snapshot`]).
#[derive(Clone)]
pub struct Snapshot {
    pub(crate) items: BTreeMap<ItemId, Item>,
    pub(crate) groups: BTreeMap<GroupId, Group>,
    pub(crate) collections: BTreeMap<CollectionId, Collection>,
    pub(crate) dismissed_collections: BTreeSet<String>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Library {
    #[serde(default)]
    pub roots: Vec<PathBuf>,
    #[serde(default)]
    pub items: BTreeMap<ItemId, Item>,
    #[serde(default)]
    pub groups: BTreeMap<GroupId, Group>,
    #[serde(default)]
    pub collections: BTreeMap<CollectionId, Collection>,
    /// The folders of automatic collections the user dissolved, so they aren't made again.
    #[serde(default)]
    pub dismissed_collections: BTreeSet<String>,
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub settings: Settings,
    #[serde(skip)]
    path_index: HashMap<PathBuf, ItemId>,
    #[serde(skip)]
    file: PathBuf,
}

/// A media file discovered on disk during a scan.
pub struct FoundFile {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub size: u64,
    pub modified: u64,
}

pub struct ScanOutcome {
    pub added: usize,
    pub removed: usize,
}

static DATA_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Makes [`data_dir`] `dir` from now on, for programs (such as Sortier Remote) that keep their data elsewhere. Only
/// the first call counts; call it before anything reads or writes data.
pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

/// Where the library file and thumbnail cache live: the folder given to [`set_data_dir`], else `SORTIER_DATA_DIR`,
/// else the system's (`%LOCALAPPDATA%\sortier` on Windows) — handy for keeping separate libraries.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = DATA_DIR.get() {
        return dir.clone();
    }
    if let Some(dir) = std::env::var_os("SORTIER_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("sortier")
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Normalise a user-typed tag: lowercase, trimmed, internal whitespace -> `_`.
pub fn normalize_tag(raw: &str) -> Option<String> {
    let t = raw
        .trim()
        .trim_start_matches(['#', '-'])
        .trim()
        .to_lowercase();
    let t: String = t
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_");
    let t = t.trim_matches(',').to_string();
    if t.is_empty() || t.ends_with(':') { None } else { Some(t) }
}

/// Split free text into normalised tags (whitespace / comma separated).
pub fn parse_tags(raw: &str) -> Vec<String> {
    raw.split(|c: char| c.is_whitespace() || c == ',')
        .filter_map(normalize_tag)
        .collect()
}

pub use sortier_media::natural_cmp;

pub fn natural_path_cmp(a: &Path, b: &Path) -> Ordering {
    natural_cmp(&a.to_string_lossy(), &b.to_string_lossy())
}

/// Work out whether a file name looks like part of a numbered sequence and, if so,
/// return the "series" part of its stem (e.g. `chapter1_p03` -> `chapter1`).
/// Returns `None` when the name has no trailing page number.
/// The original letter case is kept (use it for display; lowercase it for keys).
pub fn sequence_stem(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let trimmed = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    if trimmed.len() == stem.len() {
        // Also accept "name (3)" / "name [3]" styles.
        let inner = stem.trim_end_matches([')', ']']);
        if inner.len() == stem.len() {
            return None;
        }
        let t2 = inner.trim_end_matches(|c: char| c.is_ascii_digit());
        if t2.len() == inner.len() {
            return None;
        }
        return Some(clean_seq_prefix(t2.trim_end_matches(['(', '['])));
    }
    Some(clean_seq_prefix(trimmed))
}

fn clean_seq_prefix(s: &str) -> String {
    let seps: &[char] = &[' ', '_', '-', '.', '#', '(', '[', '~'];
    let mut s = s.trim_end_matches(seps).to_string();
    for marker in ["page", "pg", "part", "img", "p"] {
        let cut = s.len().saturating_sub(marker.len());
        if s.len() >= marker.len()
            && s.is_char_boundary(cut)
            && s[cut..].eq_ignore_ascii_case(marker)
        {
            let rest = &s[..cut];
            if rest.is_empty() || rest.ends_with(seps) {
                s = rest.trim_end_matches(seps).to_string();
                break;
            }
        }
    }
    s
}

fn pretty_name(prefix: &str, dir: &Path) -> String {
    let folder = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.to_string_lossy().into_owned());
    let p = prefix.trim();
    if p.is_empty() {
        folder
    } else if p.chars().all(|c| c.is_ascii_digit()) {
        // e.g. pixiv-style "12345678_p0": the number is the post id
        format!("{folder} {p}")
    } else {
        p.replace('_', " ")
    }
}

impl Library {
    /// `library` (as received from a Sortier Remote server), saving to `file` from now on, keeping `settings`.
    pub fn adopt(mut library: Library, file: PathBuf, settings: Settings) -> Self {
        library.file = file;
        library.settings = settings;
        library.reindex();
        library
    }

    /// The library saved at `file`, or an empty one saving there when there's none yet.
    pub fn load(file: PathBuf) -> Self {
        let mut lib = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice::<Library>(&bytes).unwrap_or_else(|err| {
                eprintln!("sortier: failed to parse {}: {err}", file.display());
                // Keep a copy of the unreadable file instead of overwriting it.
                let _ = std::fs::copy(&file, file.with_extension("json.broken"));
                Library::default()
            }),
            Err(_) => Library::default(),
        };
        lib.file = file;
        lib.reindex();
        lib
    }

    pub(crate) fn reindex(&mut self) {
        self.path_index = self
            .items
            .values()
            .map(|it| (it.path.clone(), it.id))
            .collect();
        let max = self
            .items
            .keys()
            .chain(self.groups.keys())
            .chain(self.collections.keys())
            .copied()
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max + 1);
        // Repair any dangling references.
        let groups = &self.groups;
        for item in self.items.values_mut() {
            if let Some(g) = item.group
                && !groups.get(&g).is_some_and(|g| g.items.contains(&item.id)) {
                    item.group = None;
                }
        }
        let items = &self.items;
        for g in self.groups.values_mut() {
            g.items.retain(|id| items.contains_key(id));
        }
        self.groups.retain(|_, g| !g.items.is_empty());
        self.prune_collections();
    }

    pub fn save(&self) {
        if let Err(err) = self.try_save() {
            eprintln!("sortier: failed to save library: {err:#}");
        }
    }

    fn try_save(&self) -> Result<()> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.file.with_extension("json.tmp");
        let bytes = serde_json::to_vec(self)?;
        std::fs::write(&tmp, bytes).context("writing library")?;
        std::fs::rename(&tmp, &self.file).context("replacing library")?;
        Ok(())
    }

    /// The items, sequences and collections as they are now, for undo.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            items: self.items.clone(),
            groups: self.groups.clone(),
            collections: self.collections.clone(),
            dismissed_collections: self.dismissed_collections.clone(),
        }
    }

    /// Puts back what [`Library::snapshot`] took.
    pub fn restore(&mut self, snapshot: Snapshot) {
        self.items = snapshot.items;
        self.groups = snapshot.groups;
        self.collections = snapshot.collections;
        self.dismissed_collections = snapshot.dismissed_collections;
        self.reindex();
    }

    pub(crate) fn alloc_id(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        id
    }

    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.get(&id)
    }

    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.get(&id)
    }

    pub fn group_of(&self, item: &Item) -> Option<&Group> {
        item.group.and_then(|g| self.groups.get(&g))
    }

    // ----- scanning ---------------------------------------------------------

    /// Walk the given folders and collect every supported media file. Runs off the UI thread.
    pub fn scan_folders(roots: &[PathBuf]) -> Vec<FoundFile> {
        let mut out = Vec::new();
        for root in roots {
            for entry in walkdir::WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| {
                    !e.file_name()
                        .to_str()
                        .is_some_and(|n| n.starts_with('.') && e.depth() > 0)
                })
                .filter_map(|e| e.ok())
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                let Some(kind) = MediaKind::from_path(path) else {
                    continue;
                };
                let meta = entry.metadata().ok();
                out.push(FoundFile {
                    path: path.to_path_buf(),
                    kind,
                    size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                    modified: meta
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                });
            }
        }
        out
    }

    /// Merge scan results into the library. Files under a scanned root that no longer
    /// exist are dropped (only if the root itself is still reachable).
    pub fn merge_scan(&mut self, scanned_roots: &[PathBuf], found: Vec<FoundFile>) -> ScanOutcome {
        let now = now_secs();
        let mut seen = std::collections::HashSet::new();
        let mut new_ids = Vec::new();
        for f in found {
            seen.insert(f.path.clone());
            if let Some(&id) = self.path_index.get(&f.path) {
                if let Some(item) = self.items.get_mut(&id) {
                    if item.modified != f.modified {
                        item.dims = None; // file changed; re-read dimensions
                        item.pages = None;
                    }
                    item.size = f.size;
                    item.modified = f.modified;
                    item.kind = f.kind;
                }
                continue;
            }
            let id = self.alloc_id();
            self.path_index.insert(f.path.clone(), id);
            self.items.insert(
                id,
                Item {
                    id,
                    path: f.path,
                    kind: f.kind,
                    tags: BTreeSet::new(),
                    group: None,
                    size: f.size,
                    modified: f.modified,
                    added: now,
                    dims: None,
                    pages: None,
                    cover: None,
                },
            );
            new_ids.push(id);
        }

        let live_roots: Vec<&PathBuf> = scanned_roots.iter().filter(|r| r.exists()).collect();
        let gone: Vec<ItemId> = self
            .items
            .values()
            .filter(|it| {
                live_roots.iter().any(|r| it.path.starts_with(r)) && !seen.contains(&it.path)
            })
            .map(|it| it.id)
            .collect();
        let removed = gone.len();
        self.remove_items(&gone);

        if self.settings.auto_group {
            self.auto_group(&new_ids);
            self.auto_collect();
        }
        ScanOutcome {
            added: new_ids.len(),
            removed,
        }
    }

    /// Group newly found files that look like numbered pages of the same work. Documents are left alone.
    pub fn auto_group(&mut self, ids: &[ItemId]) {
        let mut buckets: HashMap<String, Vec<ItemId>> = HashMap::new();
        let mut bucket_meta: HashMap<String, (String, PathBuf)> = HashMap::new();
        for &id in ids {
            let Some(item) = self.items.get(&id) else { continue };
            // A document is a whole volume already: numbered ones are gathered into collections instead.
            if item.group.is_some() || item.kind == MediaKind::Document {
                continue;
            }
            let Some(prefix) = sequence_stem(&item.path) else { continue };
            let dir = item.path.parent().map(Path::to_path_buf).unwrap_or_default();
            let key = format!(
                "{}|{}",
                dir.to_string_lossy().to_lowercase(),
                prefix.to_lowercase()
            );
            bucket_meta
                .entry(key.clone())
                .or_insert_with(|| (pretty_name(&prefix, &dir), dir));
            buckets.entry(key).or_default().push(id);
        }

        let existing: HashMap<String, GroupId> = self
            .groups
            .values()
            .filter_map(|g| g.auto_key.clone().map(|k| (k, g.id)))
            .collect();

        for (key, members) in buckets {
            if let Some(&gid) = existing.get(&key) {
                for &id in &members {
                    if let Some(it) = self.items.get_mut(&id) {
                        it.group = Some(gid);
                    }
                }
                if let Some(g) = self.groups.get_mut(&gid) {
                    g.items.extend(members);
                }
                self.sort_group_naturally(gid);
                continue;
            }
            if members.len() < 2 {
                continue;
            }
            let (name, _dir) = bucket_meta.remove(&key).unwrap_or_default();
            let gid = self.alloc_id();
            for &id in &members {
                if let Some(it) = self.items.get_mut(&id) {
                    it.group = Some(gid);
                }
            }
            self.groups.insert(
                gid,
                Group {
                    id: gid,
                    name,
                    items: members,
                    tags: BTreeSet::new(),
                    auto_key: Some(key),
                    cover: None,
                },
            );
            self.sort_group_naturally(gid);
        }
    }

    pub fn sort_group_naturally(&mut self, gid: GroupId) {
        let Some(mut g) = self.groups.remove(&gid) else { return };
        g.items.sort_by(|a, b| {
            let pa = self.items.get(a).map(|i| i.path.as_path());
            let pb = self.items.get(b).map(|i| i.path.as_path());
            match (pa, pb) {
                (Some(pa), Some(pb)) => natural_path_cmp(pa, pb),
                _ => Ordering::Equal,
            }
        });
        g.items.dedup();
        self.groups.insert(gid, g);
    }

    pub fn remove_items(&mut self, ids: &[ItemId]) {
        for id in ids {
            if let Some(item) = self.items.remove(id) {
                self.path_index.remove(&item.path);
                if let Some(gid) = item.group
                    && let Some(g) = self.groups.get_mut(&gid) {
                        g.items.retain(|x| x != id);
                    }
            }
        }
        self.drop_tiny_groups();
        self.prune_collections();
    }

    /// A sequence with no pages left is deleted; a single remaining page keeps its group
    /// (the user may be about to add more), unless it was auto-created.
    fn drop_tiny_groups(&mut self) {
        let doomed: Vec<GroupId> = self
            .groups
            .values()
            .filter(|g| g.items.is_empty() || (g.items.len() == 1 && g.auto_key.is_some()))
            .map(|g| g.id)
            .collect();
        for gid in doomed {
            self.ungroup(gid);
        }
    }

    pub fn add_root(&mut self, path: PathBuf) -> bool {
        if self.roots.iter().any(|r| r == &path) {
            return false;
        }
        self.roots.push(path);
        true
    }

    pub fn remove_root(&mut self, root: &Path) {
        self.roots.retain(|r| r != root);
        // Keep items still covered by another root.
        let roots = self.roots.clone();
        let gone: Vec<ItemId> = self
            .items
            .values()
            .filter(|it| it.path.starts_with(root) && !roots.iter().any(|r| it.path.starts_with(r)))
            .map(|it| it.id)
            .collect();
        self.remove_items(&gone);
    }

    // ----- groups -----------------------------------------------------------

    /// Make a new sequence from the given items (in the given order). Items already in other
    /// sequences are moved; emptied sequences are removed.
    pub fn create_group(&mut self, items: &[ItemId], name: Option<String>) -> Option<GroupId> {
        let items: Vec<ItemId> = {
            let mut seen = std::collections::HashSet::new();
            items
                .iter()
                .copied()
                .filter(|id| self.items.contains_key(id) && seen.insert(*id))
                .collect()
        };
        if items.is_empty() {
            return None;
        }
        // Carry over tags from any sequences being merged.
        let mut inherited = BTreeSet::new();
        for id in &items {
            if let Some(g) = self.items[id].group.and_then(|g| self.groups.get(&g)) {
                inherited.extend(g.tags.iter().cloned());
            }
        }
        for id in &items {
            self.detach_from_group(*id);
        }
        let gid = self.alloc_id();
        let name = name.unwrap_or_else(|| {
            let first = &self.items[&items[0]];
            let dir = first.path.parent().map(Path::to_path_buf).unwrap_or_default();
            match sequence_stem(&first.path) {
                Some(p) => pretty_name(&p, &dir),
                // Not a numbered file: name the sequence after its first page.
                None => first
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().replace('_', " "))
                    .unwrap_or_else(|| pretty_name("", &dir)),
            }
        });
        for id in &items {
            if let Some(it) = self.items.get_mut(id) {
                it.group = Some(gid);
            }
        }
        self.groups.insert(
            gid,
            Group {
                id: gid,
                name,
                items,
                tags: inherited,
                auto_key: None,
                cover: None,
            },
        );
        self.drop_empty_groups();
        self.prune_collections();
        Some(gid)
    }

    fn detach_from_group(&mut self, id: ItemId) {
        let Some(gid) = self.items.get(&id).and_then(|i| i.group) else { return };
        if let Some(g) = self.groups.get_mut(&gid) {
            g.items.retain(|x| *x != id);
        }
        if let Some(it) = self.items.get_mut(&id) {
            it.group = None;
        }
    }

    fn drop_empty_groups(&mut self) {
        self.groups.retain(|_, g| !g.items.is_empty());
    }

    /// Dissolve a sequence. Its tags are copied onto every page so nothing is lost.
    /// A sequence in a collection leaves its pages there in its place.
    pub fn ungroup(&mut self, gid: GroupId) {
        let Some(g) = self.groups.remove(&gid) else { return };
        for collection in self.collections.values_mut() {
            if let Some(at) = collection.members.iter().position(|member| *member == Member::Group(gid)) {
                collection.members.splice(at..=at, g.items.iter().map(|id| Member::Item(*id)));
            }
        }
        for id in g.items {
            if let Some(it) = self.items.get_mut(&id) {
                it.group = None;
                it.tags.extend(g.tags.iter().cloned());
            }
        }
    }

    /// Take one page out of its sequence (it keeps the sequence's tags).
    pub fn remove_from_group(&mut self, id: ItemId) {
        let tags = self
            .items
            .get(&id)
            .and_then(|i| i.group)
            .and_then(|g| self.groups.get(&g))
            .map(|g| g.tags.clone())
            .unwrap_or_default();
        self.detach_from_group(id);
        if let Some(it) = self.items.get_mut(&id) {
            it.tags.extend(tags);
        }
        self.drop_empty_groups();
    }

    /// Move a page within its sequence. `to` is clamped.
    pub fn move_in_group(&mut self, id: ItemId, delta: isize) -> Option<usize> {
        let gid = self.items.get(&id)?.group?;
        let g = self.groups.get_mut(&gid)?;
        let from = g.items.iter().position(|x| *x == id)?;
        let to = (from as isize + delta).clamp(0, g.items.len() as isize - 1) as usize;
        let v = g.items.remove(from);
        g.items.insert(to, v);
        g.auto_key = None; // manual order now; don't re-sort automatically
        Some(to)
    }

    pub fn rename_group(&mut self, gid: GroupId, name: String) {
        if let Some(g) = self.groups.get_mut(&gid) {
            let name = name.trim();
            if !name.is_empty() {
                g.name = name.to_string();
            }
        }
    }

    // ----- tags -------------------------------------------------------------

    pub fn add_item_tags(&mut self, ids: &[ItemId], tags: &[String]) {
        for id in ids {
            if let Some(it) = self.items.get_mut(id) {
                it.tags.extend(tags.iter().cloned());
            }
        }
    }

    pub fn remove_item_tag(&mut self, ids: &[ItemId], tag: &str) {
        for id in ids {
            if let Some(it) = self.items.get_mut(id) {
                it.tags.remove(tag);
            }
        }
    }

    pub fn add_group_tags(&mut self, gids: &[GroupId], tags: &[String]) {
        for gid in gids {
            if let Some(g) = self.groups.get_mut(gid) {
                g.tags.extend(tags.iter().cloned());
            }
        }
    }

    /// Removes the tag from the sequence and from all of its pages.
    pub fn remove_group_tag(&mut self, gids: &[GroupId], tag: &str) {
        for gid in gids {
            if let Some(g) = self.groups.get_mut(gid) {
                g.tags.remove(tag);
                for id in g.items.clone() {
                    if let Some(it) = self.items.get_mut(&id) {
                        it.tags.remove(tag);
                    }
                }
            }
        }
    }

    /// Tag usage counted per *work*: a sequence counts once, a loose file counts once.
    pub fn tag_counts(&self) -> Vec<(String, usize)> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for g in self.groups.values() {
            let mut set: BTreeSet<&str> = g.tags.iter().map(String::as_str).collect();
            for id in &g.items {
                if let Some(it) = self.items.get(id) {
                    set.extend(it.tags.iter().map(String::as_str));
                }
            }
            for t in set {
                *counts.entry(t).or_default() += 1;
            }
        }
        for it in self.items.values().filter(|i| i.group.is_none()) {
            for t in &it.tags {
                *counts.entry(t.as_str()).or_default() += 1;
            }
        }
        let mut v: Vec<(String, usize)> =
            counts.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["p10.png", "p2.png", "p1.png", "P3.png"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["p1.png", "p2.png", "P3.png", "p10.png"]);
    }

    #[test]
    fn sequence_stems() {
        let s = |p: &str| sequence_stem(Path::new(p));
        assert_eq!(s("12345_p0.png").as_deref(), Some("12345"));
        assert_eq!(s("12345_p12.png").as_deref(), Some("12345"));
        assert_eq!(s("001.jpg").as_deref(), Some(""));
        assert_eq!(s("Chapter 3 - Page 07.jpg").as_deref(), Some("Chapter 3"));
        assert_eq!(s("comic (2).png").as_deref(), Some("comic"));
        assert_eq!(s("cover.png"), None);
    }

    #[test]
    fn tags_normalise() {
        assert_eq!(parse_tags("Blue Sky, artist:Foo  #cat"), vec!["blue", "sky", "artist:foo", "cat"]);
        assert_eq!(normalize_tag("  Two  Words "), Some("two_words".into()));
        assert_eq!(normalize_tag("artist:"), None);
    }
}
