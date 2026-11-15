//! The area layout model shared by every workspace. Pure data and functions: no
//! UI or rendering dependencies, so all behaviour is unit-tested here.

mod geometry;
mod join;
mod tree;
mod workspace;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod workspace_tests;

pub use geometry::{Orientation, Point, Rect, Size};
pub use tree::{
    Area, AreaError, AreaId, AreaLayout, AreaTree, Branch, EditorTypeId, Hit, JoinDirection, JoinOption,
    JoinOutcome, Layout, LayoutMetrics, LayoutNode, SeamLayout, Split, SplitPath,
};
pub use workspace::{Workspace, WorkspaceError, WorkspaceId, Workspaces};
