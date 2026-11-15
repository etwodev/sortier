//! Collections: ordered groups of volumes (sequences, or single files such as one PDF per volume), like a manga
//! series. A sequence or file belongs to at most one collection.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::library::{GroupId, Item, ItemId, Library, MediaKind, natural_cmp};

pub type CollectionId = u64;

/// One volume of a collection.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Member {
    Item(ItemId),
    Group(GroupId),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Collection {
    pub id: CollectionId,
    pub name: String,
    /// The volumes in reading order.
    pub members: Vec<Member>,
    #[serde(default)]
    pub tags: BTreeSet<String>,
    /// A cover the user chose, shown instead of the first volume's (see [`crate::thumbs::save_cover`]).
    #[serde(default)]
    pub cover: Option<PathBuf>,
    /// The folder it was made from automatically, if it was.
    #[serde(default)]
    pub auto_key: Option<String>,
}

impl Library {
    pub fn collection(&self, id: CollectionId) -> Option<&Collection> {
        self.collections.get(&id)
    }

    /// The collection holding `member`, if any.
    pub fn collection_holding(&self, member: Member) -> Option<&Collection> {
        self.collections.values().find(|collection| collection.members.contains(&member))
    }

    /// The collection `item` is in, as a volume of its own or through its sequence.
    pub fn collection_of(&self, item: &Item) -> Option<&Collection> {
        match item.group {
            Some(group) => self.collection_holding(Member::Group(group)),
            None => self.collection_holding(Member::Item(item.id)),
        }
    }

    /// The files of `member`, in order.
    pub fn member_items(&self, member: Member) -> Vec<ItemId> {
        match member {
            Member::Item(id) => self.items.contains_key(&id).then_some(id).into_iter().collect(),
            Member::Group(group) => self.group(group).map(|group| group.items.clone()).unwrap_or_default(),
        }
    }

    /// Every file of a collection, volume by volume.
    pub fn collection_items(&self, id: CollectionId) -> Vec<ItemId> {
        let Some(collection) = self.collection(id) else { return Vec::new() };
        collection.members.iter().flat_map(|member| self.member_items(*member)).collect()
    }

    /// Makes a collection of `members` in this order, taking them out of any collection they were in, named
    /// `name` or after the folder the first one is in. `None` when there's nothing to collect.
    pub fn create_collection(&mut self, members: &[Member], name: Option<String>) -> Option<CollectionId> {
        let mut seen = HashSet::new();
        let members: Vec<Member> = members
            .iter()
            .copied()
            .filter(|member| !self.member_items(*member).is_empty() && seen.insert(*member))
            .collect();
        let first = *self.member_items(*members.first()?).first()?;
        let name = name.unwrap_or_else(|| {
            let folder = match members[0] {
                Member::Group(_) => self.items[&first].path.parent().and_then(Path::parent),
                Member::Item(_) => self.items[&first].path.parent(),
            };
            folder
                .and_then(Path::file_name)
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Collection".to_string())
        });
        for collection in self.collections.values_mut() {
            collection.members.retain(|member| !members.contains(member));
        }
        let id = self.alloc_id();
        self.collections.insert(
            id,
            Collection {
                id,
                name,
                members,
                tags: BTreeSet::new(),
                cover: None,
                auto_key: None,
            },
        );
        self.prune_collections();
        Some(id)
    }

    /// Adds `members` to the end of collection `id`, taking them out of any other collection.
    pub fn add_to_collection(&mut self, id: CollectionId, members: &[Member]) {
        if !self.collections.contains_key(&id) {
            return;
        }
        for (other, collection) in self.collections.iter_mut() {
            if *other != id {
                collection.members.retain(|member| !members.contains(member));
            }
        }
        let collection = self.collections.get_mut(&id).expect("checked above");
        for member in members {
            if !collection.members.contains(member) {
                collection.members.push(*member);
            }
        }
        self.prune_collections();
    }

    /// Dissolves a collection; its volumes stay as they are. One made automatically isn't made again.
    pub fn dissolve_collection(&mut self, id: CollectionId) {
        if let Some(collection) = self.collections.remove(&id)
            && let Some(key) = collection.auto_key
        {
            self.dismissed_collections.insert(key);
        }
    }

    /// Takes `member` out of its collection.
    pub fn remove_from_collection(&mut self, member: Member) {
        for collection in self.collections.values_mut() {
            collection.members.retain(|held| *held != member);
        }
        self.prune_collections();
    }

    /// Moves `member` `delta` places along its collection.
    pub fn move_in_collection(&mut self, member: Member, delta: isize) {
        for collection in self.collections.values_mut() {
            if let Some(from) = collection.members.iter().position(|held| *held == member) {
                let to = (from as isize + delta).clamp(0, collection.members.len() as isize - 1) as usize;
                let moved = collection.members.remove(from);
                collection.members.insert(to, moved);
                collection.auto_key = None;
            }
        }
    }

    pub fn rename_collection(&mut self, id: CollectionId, name: &str) {
        if let Some(collection) = self.collections.get_mut(&id)
            && !name.trim().is_empty()
        {
            collection.name = name.trim().to_string();
        }
    }

    pub fn add_collection_tags(&mut self, id: CollectionId, tags: &[String]) {
        if let Some(collection) = self.collections.get_mut(&id) {
            collection.tags.extend(tags.iter().cloned());
        }
    }

    /// Removes the tag from the collection and from every volume and file in it.
    pub fn remove_collection_tag(&mut self, id: CollectionId, tag: &str) {
        let Some(collection) = self.collections.get_mut(&id) else { return };
        collection.tags.remove(tag);
        let members = collection.members.clone();
        for member in members {
            if let Member::Group(group) = member {
                self.remove_group_tag(&[group], tag);
            }
            let items = self.member_items(member);
            self.remove_item_tag(&items, tag);
        }
    }

    /// Keeps collections in step with the files and sequences: a file that joined a sequence is held as that
    /// sequence, volumes that no longer exist are dropped (as are collections left empty), and nothing is in two
    /// collections.
    pub(crate) fn prune_collections(&mut self) {
        let mut held = HashSet::new();
        let (items, groups) = (&self.items, &self.groups);
        for collection in self.collections.values_mut() {
            let members = std::mem::take(&mut collection.members);
            for member in members {
                let member = match member {
                    Member::Item(id) => match items.get(&id) {
                        Some(item) => item.group.map_or(member, Member::Group),
                        None => continue,
                    },
                    Member::Group(group) if groups.contains_key(&group) => member,
                    Member::Group(_) => continue,
                };
                if held.insert(member) {
                    collection.members.push(member);
                }
            }
        }
        self.collections.retain(|_, collection| !collection.members.is_empty());
    }

    /// Makes collections of folders holding two or more volumes: folders each holding one whole sequence (as a
    /// manga volume's pages do), or documents. Folders that are library roots aren't, since they usually hold many
    /// series. Volumes already in a collection are left alone, and collections the user dissolved aren't made
    /// again.
    pub fn auto_collect(&mut self) {
        let mut sequences_in: HashMap<PathBuf, BTreeSet<GroupId>> = HashMap::new();
        let mut loose_in: HashMap<PathBuf, usize> = HashMap::new();
        for item in self.items.values() {
            let Some(folder) = item.path.parent() else { continue };
            match item.group {
                Some(group) => {
                    sequences_in.entry(folder.to_path_buf()).or_default().insert(group);
                }
                None => *loose_in.entry(folder.to_path_buf()).or_default() += 1,
            }
        }
        let mut volumes: BTreeMap<PathBuf, Vec<(PathBuf, Member)>> = BTreeMap::new();
        for (folder, groups) in &sequences_in {
            let whole = groups.len() == 1 && !loose_in.contains_key(folder);
            if let (true, Some(parent)) = (whole, folder.parent()) {
                let group = *groups.first().expect("one group");
                volumes.entry(parent.to_path_buf()).or_default().push((folder.clone(), Member::Group(group)));
            }
        }
        for item in self.items.values().filter(|item| item.kind == MediaKind::Document && item.group.is_none()) {
            if let Some(folder) = item.path.parent() {
                volumes.entry(folder.to_path_buf()).or_default().push((item.path.clone(), Member::Item(item.id)));
            }
        }
        for (folder, mut found) in volumes {
            let key = folder.to_string_lossy().into_owned();
            let inside_a_root = self.roots.iter().any(|root| folder.starts_with(root) && folder != *root);
            let taken = found.iter().any(|(_, member)| self.collection_holding(*member).is_some());
            let known = self.dismissed_collections.contains(&key)
                || self.collections.values().any(|collection| collection.auto_key.as_deref() == Some(key.as_str()));
            if found.len() < 2 || !inside_a_root || taken || known {
                continue;
            }
            found.sort_by(|a, b| natural_cmp(&a.0.to_string_lossy(), &b.0.to_string_lossy()));
            let members: Vec<Member> = found.into_iter().map(|(_, member)| member).collect();
            let name = folder.file_name().map(|name| name.to_string_lossy().into_owned());
            if let Some(id) = self.create_collection(&members, name)
                && let Some(collection) = self.collections.get_mut(&id)
            {
                collection.auto_key = Some(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::FoundFile;

    /// A library of `paths` under `/lib`, merged as a scan would, with sequences and collections made automatically.
    fn scanned(paths: &[&str]) -> Library {
        let mut library = Library::default();
        let root = PathBuf::from("/lib");
        library.add_root(root.clone());
        let found = paths
            .iter()
            .map(|path| FoundFile {
                path: root.join(path),
                kind: MediaKind::from_path(Path::new(path)).unwrap(),
                size: 1,
                modified: 1,
            })
            .collect();
        library.merge_scan(&[root], found);
        library
    }

    fn names(library: &Library) -> BTreeSet<String> {
        library.collections.values().map(|collection| collection.name.clone()).collect()
    }

    #[test]
    fn folders_of_volumes_become_collections() {
        let library = scanned(&[
            "Series/Vol 2/001.jpg",
            "Series/Vol 2/002.jpg",
            "Series/Vol 10/001.jpg",
            "Series/Vol 10/002.jpg",
            "Series/Vol 1/001.jpg",
            "Series/Vol 1/002.jpg",
            "Books/Part 2.cbz",
            "Books/Part 1.pdf",
            "Loose/One.cbz",
            "Top 1.cbz",
            "Top 2.cbz",
        ]);
        let expected: BTreeSet<String> = ["Series".to_string(), "Books".to_string()].into();
        assert_eq!(names(&library), expected, "not a lone volume, nor the library's own folder");
        let series = library.collections.values().find(|collection| collection.name == "Series").unwrap();
        let volume_names: Vec<String> = series
            .members
            .iter()
            .map(|member| match member {
                Member::Group(group) => library.group(*group).unwrap().name.clone(),
                Member::Item(_) => panic!("volumes of pages are sequences"),
            })
            .collect();
        assert_eq!(volume_names, vec!["Vol 1", "Vol 2", "Vol 10"]);
        assert_eq!(library.collection_items(series.id).len(), 6);
        let page = library.items.values().find(|item| item.path.ends_with("Vol 10/002.jpg")).unwrap();
        assert_eq!(library.collection_of(page).map(|collection| collection.id), Some(series.id));
    }

    #[test]
    fn dissolved_automatic_collections_stay_dissolved() {
        let mut library = scanned(&["Books/Part 1.pdf", "Books/Part 2.pdf"]);
        let id = *library.collections.keys().next().unwrap();
        library.dissolve_collection(id);
        library.auto_collect();
        assert!(library.collections.is_empty());
    }

    #[test]
    fn collections_follow_their_volumes() {
        let mut library = scanned(&["a.pdf", "b.pdf", "c.pdf", "d.pdf"]);
        let ids: Vec<ItemId> = library.items.keys().copied().collect();
        let members: Vec<Member> = ids.iter().map(|id| Member::Item(*id)).collect();
        let first = library.create_collection(&members[..3], Some("First".into())).unwrap();
        let second = library.create_collection(&members[2..], Some("Second".into())).unwrap();
        assert_eq!(library.collection(first).unwrap().members, members[..2], "c moved to the second");
        assert_eq!(library.collection(second).unwrap().members, members[2..]);

        let group = library.create_group(&ids[..2], None).unwrap();
        assert_eq!(library.collection(first).unwrap().members, vec![Member::Group(group)], "held as their sequence");
        library.ungroup(group);
        assert_eq!(library.collection(first).unwrap().members, members[..2], "and as files again");

        library.remove_items(&ids[2..]);
        assert!(library.collection(second).is_none(), "nothing left in it");
        library.move_in_collection(members[1], -1);
        assert_eq!(library.collection(first).unwrap().members, vec![members[1], members[0]]);
    }
}
