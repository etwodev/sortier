use serde::{Deserialize, Serialize};

use crate::tree::AreaTree;

/// Stable identifier of a workspace within one [`Workspaces`] collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(pub u32);

/// A named tab with its own independent area layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub layout: AreaTree,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceError {
    #[error("workspace {0:?} does not exist")]
    UnknownWorkspace(WorkspaceId),
    #[error("a workspace name cannot be empty")]
    EmptyName,
    #[error("the last workspace cannot be removed")]
    LastWorkspace,
    #[error("invalid workspaces: {0}")]
    Invalid(String),
}

/// The serialised form of [`Workspaces`]; the id counter is derived on load.
#[derive(Serialize, Deserialize)]
struct WorkspacesData {
    workspaces: Vec<Workspace>,
    active: WorkspaceId,
}

/// The ordered workspace tabs of a project, exactly one of which is active.
/// Always holds at least one workspace, and names are kept unique.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "WorkspacesData", into = "WorkspacesData")]
pub struct Workspaces {
    list: Vec<Workspace>,
    active: WorkspaceId,
    next_id: u32,
}

impl TryFrom<WorkspacesData> for Workspaces {
    type Error = WorkspaceError;

    fn try_from(data: WorkspacesData) -> Result<Self, WorkspaceError> {
        if data.workspaces.is_empty() {
            return Err(WorkspaceError::Invalid("a project needs at least one workspace".into()));
        }
        let mut ids: Vec<WorkspaceId> = data.workspaces.iter().map(|workspace| workspace.id).collect();
        ids.sort();
        if ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(WorkspaceError::Invalid("two workspaces share an id".into()));
        }
        if data.workspaces.iter().any(|workspace| workspace.name.trim().is_empty()) {
            return Err(WorkspaceError::EmptyName);
        }
        if !ids.contains(&data.active) {
            return Err(WorkspaceError::UnknownWorkspace(data.active));
        }
        let next_id = ids.last().map_or(1, |id| id.0 + 1);
        Ok(Self {
            list: data.workspaces,
            active: data.active,
            next_id,
        })
    }
}

impl From<Workspaces> for WorkspacesData {
    fn from(workspaces: Workspaces) -> Self {
        Self {
            workspaces: workspaces.list,
            active: workspaces.active,
        }
    }
}

impl Workspaces {
    /// A collection holding one workspace, which is active.
    pub fn new(name: impl Into<String>, layout: AreaTree) -> Self {
        let name = name.into();
        let name = if name.trim().is_empty() {
            "Workspace".to_string()
        } else {
            name.trim().to_string()
        };
        Self {
            list: vec![Workspace {
                id: WorkspaceId(1),
                name,
                layout,
            }],
            active: WorkspaceId(1),
            next_id: 2,
        }
    }

    /// Workspaces in tab order.
    pub fn iter(&self) -> impl Iterator<Item = &Workspace> {
        self.list.iter()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Always false: a collection holds at least one workspace.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn get(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.list.iter().find(|workspace| workspace.id == id)
    }

    pub fn layout_mut(&mut self, id: WorkspaceId) -> Option<&mut AreaTree> {
        self.list
            .iter_mut()
            .find(|workspace| workspace.id == id)
            .map(|workspace| &mut workspace.layout)
    }

    pub fn active(&self) -> WorkspaceId {
        self.active
    }

    pub fn index_of(&self, id: WorkspaceId) -> Option<usize> {
        self.list.iter().position(|workspace| workspace.id == id)
    }

    pub fn activate(&mut self, id: WorkspaceId) -> Result<(), WorkspaceError> {
        self.index_of(id).ok_or(WorkspaceError::UnknownWorkspace(id))?;
        self.active = id;
        Ok(())
    }

    /// Adds a workspace directly after the active one and activates it. The
    /// name is made unique by adding a numeric suffix if needed.
    pub fn add(&mut self, name: &str, layout: AreaTree) -> WorkspaceId {
        let id = WorkspaceId(self.next_id);
        self.next_id += 1;
        let name = self.unique_name(name, None);
        let index = self.index_of(self.active).map_or(self.list.len(), |index| index + 1);
        self.list.insert(index, Workspace { id, name, layout });
        self.active = id;
        id
    }

    /// Renames `id`, trimming whitespace and adding a numeric suffix if another
    /// workspace already has the name. Returns the name actually used.
    pub fn rename(&mut self, id: WorkspaceId, name: &str) -> Result<String, WorkspaceError> {
        let index = self.index_of(id).ok_or(WorkspaceError::UnknownWorkspace(id))?;
        if name.trim().is_empty() {
            return Err(WorkspaceError::EmptyName);
        }
        let name = self.unique_name(name, Some(id));
        self.list[index].name = name.clone();
        Ok(name)
    }

    /// Moves `id` to position `index` in the tab order, clamped to the end.
    pub fn move_to(&mut self, id: WorkspaceId, index: usize) -> Result<(), WorkspaceError> {
        let from = self.index_of(id).ok_or(WorkspaceError::UnknownWorkspace(id))?;
        let workspace = self.list.remove(from);
        let to = index.min(self.list.len());
        self.list.insert(to, workspace);
        Ok(())
    }

    /// Removes `id`. If it was active, the workspace that takes its place in
    /// the tab order (or the one before it, at the end) becomes active.
    pub fn remove(&mut self, id: WorkspaceId) -> Result<Workspace, WorkspaceError> {
        let index = self.index_of(id).ok_or(WorkspaceError::UnknownWorkspace(id))?;
        if self.list.len() == 1 {
            return Err(WorkspaceError::LastWorkspace);
        }
        let removed = self.list.remove(index);
        if self.active == id {
            self.active = self.list[index.min(self.list.len() - 1)].id;
        }
        Ok(removed)
    }

    /// `base` if no other workspace uses it, otherwise `base.001`, `base.002`,
    /// ... in Blender's style. Any existing numeric suffix on `base` is
    /// replaced rather than extended.
    pub fn unique_name(&self, base: &str, except: Option<WorkspaceId>) -> String {
        let base = base.trim();
        let taken = |candidate: &str| {
            self.list
                .iter()
                .any(|workspace| Some(workspace.id) != except && workspace.name == candidate)
        };
        if !taken(base) {
            return base.to_string();
        }
        let stem = match base.rsplit_once('.') {
            Some((stem, suffix)) if suffix.len() == 3 && suffix.chars().all(|c| c.is_ascii_digit()) => stem,
            _ => base,
        };
        (1..)
            .map(|n| format!("{stem}.{n:03}"))
            .find(|candidate| !taken(candidate))
            .expect("an unused suffix always exists")
    }
}
