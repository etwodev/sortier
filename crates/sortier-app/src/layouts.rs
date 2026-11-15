use std::path::PathBuf;

use sortier_areas::{Area, AreaId, AreaTree, EditorTypeId, LayoutNode, Orientation, Split, WorkspaceId, Workspaces};

use crate::editors;

/// Layouts offered when creating a workspace, and the ones a new install starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceTemplate {
    /// The browser down the left, the library grid in the middle, the inspector on the right.
    Library,
    /// A large viewer above the page strip, with the inspector beside it.
    Reader,
    /// The viewer beside a smaller grid and the inspector, for tagging while looking.
    Tagging,
}

impl WorkspaceTemplate {
    pub const ALL: [WorkspaceTemplate; 3] = [WorkspaceTemplate::Library, WorkspaceTemplate::Reader, WorkspaceTemplate::Tagging];

    pub fn name(self) -> &'static str {
        match self {
            WorkspaceTemplate::Library => "Library",
            WorkspaceTemplate::Reader => "Reader",
            WorkspaceTemplate::Tagging => "Tagging",
        }
    }

    pub fn layout(self) -> AreaTree {
        let area = |id, editor_type| LayoutNode::Area(Area { id: AreaId(id), editor_type });
        let split = |orientation, ratio, first, second| {
            LayoutNode::Split(Split {
                orientation,
                ratio,
                first: Box::new(first),
                second: Box::new(second),
            })
        };
        let root = match self {
            WorkspaceTemplate::Library => split(
                Orientation::Vertical,
                0.17,
                area(1, editors::browser_id()),
                split(
                    Orientation::Vertical,
                    0.76,
                    area(2, editors::library_id()),
                    area(3, editors::inspector_id()),
                ),
            ),
            WorkspaceTemplate::Reader => split(
                Orientation::Vertical,
                0.8,
                split(
                    Orientation::Horizontal,
                    0.8,
                    area(1, editors::viewer_id()),
                    area(2, editors::pages_id()),
                ),
                area(3, editors::inspector_id()),
            ),
            WorkspaceTemplate::Tagging => split(
                Orientation::Vertical,
                0.55,
                area(1, editors::viewer_id()),
                split(
                    Orientation::Vertical,
                    0.6,
                    area(2, editors::library_id()),
                    split(
                        Orientation::Horizontal,
                        0.45,
                        area(3, editors::browser_id()),
                        area(4, editors::inspector_id()),
                    ),
                ),
            ),
        };
        AreaTree::from_root(root).expect("workspace templates are valid layouts")
    }
}

/// The workspace tabs and their area layouts. Saved next to the library, so the
/// window opens the way it was left.
pub struct LayoutModel {
    workspaces: Workspaces,
    file: PathBuf,
}

impl LayoutModel {
    /// Loads the saved workspaces, or the three templates when there are none
    /// (or the file can't be read).
    pub fn load() -> Self {
        let file = sortier_library::library::data_dir().join("workspaces.json");
        let workspaces = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Workspaces>(&bytes).ok())
            .unwrap_or_else(default_workspaces);
        Self { workspaces, file }
    }

    pub fn workspaces(&self) -> &Workspaces {
        &self.workspaces
    }

    pub fn layout(&self, workspace: WorkspaceId) -> Option<&AreaTree> {
        self.workspaces.get(workspace).map(|workspace| &workspace.layout)
    }

    /// The layout of `workspace` for editing. Saved the next time
    /// [`LayoutModel::save`] runs; the area host saves after each edit.
    pub fn layout_mut(&mut self, workspace: WorkspaceId) -> Option<&mut AreaTree> {
        self.workspaces.layout_mut(workspace)
    }

    pub fn activate_workspace(&mut self, workspace: WorkspaceId) {
        if self.workspaces.activate(workspace).is_ok() {
            self.save();
        }
    }

    /// Applies `edit` to the workspace list and saves it.
    pub fn edit_workspaces<R>(&mut self, edit: impl FnOnce(&mut Workspaces) -> R) -> R {
        let result = edit(&mut self.workspaces);
        self.save();
        result
    }

    /// Switches to the first workspace showing an editor of `editor_type`, unless
    /// the active one already does. Returns whether any workspace shows one.
    pub fn reveal_editor(&mut self, editor_type: &EditorTypeId) -> bool {
        let shows = |id: WorkspaceId| {
            self.layout(id)
                .is_some_and(|tree| tree.areas().iter().any(|area| &area.editor_type == editor_type))
        };
        if shows(self.workspaces.active()) {
            return true;
        }
        let Some(target) = self.workspaces.iter().map(|workspace| workspace.id).find(|id| shows(*id)) else {
            return false;
        };
        self.activate_workspace(target);
        true
    }

    pub fn save(&self) {
        let result = serde_json::to_vec_pretty(&self.workspaces)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                if let Some(dir) = self.file.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&self.file, bytes)
            });
        if let Err(error) = result {
            eprintln!("sortier: couldn't save the workspaces: {error}");
        }
    }
}

fn default_workspaces() -> Workspaces {
    let mut workspaces = Workspaces::new(WorkspaceTemplate::Library.name(), WorkspaceTemplate::Library.layout());
    let first = workspaces.active();
    for template in [WorkspaceTemplate::Reader, WorkspaceTemplate::Tagging] {
        workspaces.add(template.name(), template.layout());
    }
    workspaces.activate(first).ok();
    workspaces
}
