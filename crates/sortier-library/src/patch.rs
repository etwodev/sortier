//! Patches: what an edit changed in a library, so another copy of it (on a Sortier Remote server, or another
//! computer's view of one) can make the same change.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::library::{Collection, CollectionId, Group, GroupId, Item, ItemId, Library, Snapshot};

/// Records added or changed (whole), and records removed, since a [`Snapshot`].
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Patch {
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default)]
    pub removed_items: Vec<ItemId>,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub removed_groups: Vec<GroupId>,
    #[serde(default)]
    pub collections: Vec<Collection>,
    #[serde(default)]
    pub removed_collections: Vec<CollectionId>,
    /// The whole set, when it changed.
    #[serde(default)]
    pub dismissed_collections: Option<BTreeSet<String>>,
    /// The next id to give out afterwards, so both copies keep giving out the same ids.
    pub next_id: u64,
}

impl Patch {
    /// Whether it changes nothing but perhaps the next id.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
            && self.removed_items.is_empty()
            && self.groups.is_empty()
            && self.removed_groups.is_empty()
            && self.collections.is_empty()
            && self.removed_collections.is_empty()
            && self.dismissed_collections.is_none()
    }
}

/// The records of `after` that are new or differ from `before`, and the keys of `before` that `after` lacks.
fn changes<K: Ord + Copy, V: Clone + PartialEq>(before: &BTreeMap<K, V>, after: &BTreeMap<K, V>) -> (Vec<V>, Vec<K>) {
    let changed = after
        .iter()
        .filter(|(key, value)| before.get(key) != Some(value))
        .map(|(_, value)| value.clone())
        .collect();
    let removed = before.keys().filter(|key| !after.contains_key(key)).copied().collect();
    (changed, removed)
}

impl Library {
    /// What changed since `before` was taken.
    pub fn diff(&self, before: &Snapshot) -> Patch {
        let (items, removed_items) = changes(&before.items, &self.items);
        let (groups, removed_groups) = changes(&before.groups, &self.groups);
        let (collections, removed_collections) = changes(&before.collections, &self.collections);
        Patch {
            items,
            removed_items,
            groups,
            removed_groups,
            collections,
            removed_collections,
            dismissed_collections: (before.dismissed_collections != self.dismissed_collections)
                .then(|| self.dismissed_collections.clone()),
            next_id: self.next_id,
        }
    }

    /// Makes the change `patch` describes.
    pub fn apply_patch(&mut self, patch: &Patch) {
        for id in &patch.removed_items {
            self.items.remove(id);
        }
        for id in &patch.removed_groups {
            self.groups.remove(id);
        }
        for id in &patch.removed_collections {
            self.collections.remove(id);
        }
        for item in &patch.items {
            self.items.insert(item.id, item.clone());
        }
        for group in &patch.groups {
            self.groups.insert(group.id, group.clone());
        }
        for collection in &patch.collections {
            self.collections.insert(collection.id, collection.clone());
        }
        if let Some(dismissed) = &patch.dismissed_collections {
            self.dismissed_collections = dismissed.clone();
        }
        self.next_id = self.next_id.max(patch.next_id);
        self.reindex();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::collection::Member;
    use crate::library::{FoundFile, MediaKind};

    fn library(names: &[&str]) -> Library {
        let mut library = Library::default();
        let found = names
            .iter()
            .map(|name| FoundFile {
                path: PathBuf::from("/lib").join(name),
                kind: MediaKind::Image,
                size: 1,
                modified: 1,
            })
            .collect();
        library.merge_scan(&[PathBuf::from("/lib")], found);
        library
    }

    #[test]
    fn a_copy_given_the_patch_ends_up_the_same() {
        let names = ["cat.png", "dog.png", "fox.png", "owl.png"];
        let mut original = library(&names);
        let mut copy = library(&names);
        let ids: Vec<ItemId> = original.items.keys().copied().collect();

        let before = original.snapshot();
        original.add_item_tags(&ids[..1], &["cute".to_string()]);
        let group = original.create_group(&ids[1..3], Some("Pair".into())).unwrap();
        original.create_collection(&[Member::Group(group), Member::Item(ids[3])], Some("All".into()));
        original.remove_items(&ids[..1]);
        let patch = original.diff(&before);
        assert!(!patch.is_empty());
        assert_eq!(patch.removed_items, ids[..1]);

        copy.apply_patch(&patch);
        assert_eq!(copy.items, original.items);
        assert_eq!(copy.groups, original.groups);
        assert_eq!(copy.collections, original.collections);
        let next = original.create_group(&ids[1..2], None);
        assert_eq!(copy.create_group(&ids[1..2], None), next, "both give out the same next id");

        let unchanged = copy.snapshot();
        assert!(copy.diff(&unchanged).is_empty());
    }
}
