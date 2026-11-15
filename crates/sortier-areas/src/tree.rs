use serde::{Deserialize, Serialize};

use crate::geometry::{Axis, Orientation, Point, Rect, Size};
use crate::join;

/// Stable identifier of an area within one [`AreaTree`]. Never reused by the tree
/// that allocated it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AreaId(pub u32);

/// Identifies a registered editor type, such as `"sortier.viewer"`.
///
/// A string rather than an enum so that new editors can be registered without
/// changing this crate; the editor registry validates it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EditorTypeId(String);

impl EditorTypeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A leaf of the layout: one rectangle showing one editor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Area {
    pub id: AreaId,
    pub editor_type: EditorTypeId,
}

/// A node of the layout tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LayoutNode {
    Split(Split),
    Area(Area),
}

/// Divides a rectangle in two along `orientation`. `ratio` is the fraction of the
/// rectangle's extent given to `first`; the seam gap is taken from `second`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Split {
    pub orientation: Orientation,
    pub ratio: f32,
    pub first: Box<LayoutNode>,
    pub second: Box<LayoutNode>,
}

/// Which child of a [`Split`] to descend into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Branch {
    First,
    Second,
}

/// The location of a [`Split`] in the tree, as the branches taken from the root.
/// Paths are only valid until the tree is next modified.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SplitPath(pub(crate) Vec<Branch>);

impl SplitPath {
    pub fn root() -> Self {
        Self(Vec::new())
    }

    pub fn child(&self, branch: Branch) -> Self {
        let mut branches = self.0.clone();
        branches.push(branch);
        Self(branches)
    }

    pub fn branches(&self) -> &[Branch] {
        &self.0
    }
}

/// Pixel constants that turn the ratio tree into rectangles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutMetrics {
    /// Width of the seam between two areas.
    pub gap: f32,
    /// Smallest size an area may be resized or split down to.
    pub min_area: Size,
    /// Extra distance on each side of a seam that still counts as hitting it.
    pub seam_hit_slop: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AreaLayout {
    pub id: AreaId,
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeamLayout {
    pub path: SplitPath,
    pub orientation: Orientation,
    pub rect: Rect,
}

/// The rectangles of every area and seam for one set of bounds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub areas: Vec<AreaLayout>,
    /// Ordered parents before children.
    pub seams: Vec<SeamLayout>,
}

impl Layout {
    pub fn area_rect(&self, id: AreaId) -> Option<Rect> {
        self.areas.iter().find(|area| area.id == id).map(|area| area.rect)
    }

    pub fn seam(&self, path: &SplitPath) -> Option<&SeamLayout> {
        self.seams.iter().find(|seam| &seam.path == path)
    }

    fn area_at(&self, point: Point) -> Option<AreaId> {
        self.areas.iter().find(|area| area.rect.contains(point)).map(|area| area.id)
    }
}

/// What lies under a point.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Area(AreaId),
    Seam(SplitPath),
}

/// A join offered from a seam's context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinDirection {
    Up,
    Down,
    Left,
    Right,
}

/// One valid join across a seam: `keep` grows over `remove`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinOption {
    pub direction: JoinDirection,
    pub keep: AreaId,
    pub remove: AreaId,
}

/// Areas that a join created or destroyed, so callers can update per-area state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JoinOutcome {
    /// Leftover pieces split off to align the two areas' edges.
    pub created: Vec<AreaId>,
    /// Areas that no longer exist.
    pub removed: Vec<AreaId>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AreaError {
    #[error("area {0:?} does not exist")]
    UnknownArea(AreaId),
    #[error("no split exists at that path")]
    UnknownSplit,
    #[error("the area is too small to split or resize further")]
    TooSmall,
    #[error("an area cannot be joined with itself")]
    SameArea,
    #[error("the areas do not share an edge")]
    NotAdjacent,
    #[error("neither area's edge contains the other's")]
    Staggered,
    #[error("the joined layout cannot be expressed as nested splits")]
    NotRepresentable,
    #[error("invalid layout: {0}")]
    Invalid(String),
}

/// The edge of a subtree that follows a moving seam during a resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MovingEdge {
    Start,
    End,
}

/// A workspace's area layout: a binary tree of splits with areas at the leaves.
///
/// All operations are pure functions of the tree and the host's bounds and
/// [`LayoutMetrics`]; rendering reads [`AreaTree::layout`] and calls back into
/// these operations in response to input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "LayoutNode", into = "LayoutNode")]
pub struct AreaTree {
    root: LayoutNode,
    next_id: u32,
}

impl TryFrom<LayoutNode> for AreaTree {
    type Error = AreaError;

    fn try_from(root: LayoutNode) -> Result<Self, AreaError> {
        Self::from_root(root)
    }
}

impl From<AreaTree> for LayoutNode {
    fn from(tree: AreaTree) -> Self {
        tree.root
    }
}

impl AreaTree {
    /// A tree with one area showing `editor_type`.
    pub fn new(editor_type: EditorTypeId) -> Self {
        Self {
            root: LayoutNode::Area(Area {
                id: AreaId(1),
                editor_type,
            }),
            next_id: 2,
        }
    }

    /// Builds a tree from an existing layout, rejecting duplicate area ids and
    /// ratios outside `(0, 1)`.
    pub fn from_root(root: LayoutNode) -> Result<Self, AreaError> {
        let mut ids = Vec::new();
        validate(&root, &mut ids)?;
        ids.sort();
        if let Some(pair) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(AreaError::Invalid(format!("area id {} is used twice", pair[0].0)));
        }
        let next_id = ids.last().map_or(1, |id| id.0 + 1);
        Ok(Self { root, next_id })
    }

    pub fn root(&self) -> &LayoutNode {
        &self.root
    }

    /// Every area, in left-to-right, top-to-bottom tree order.
    pub fn areas(&self) -> Vec<&Area> {
        let mut areas = Vec::new();
        collect_areas(&self.root, &mut areas);
        areas
    }

    pub fn area(&self, id: AreaId) -> Option<&Area> {
        self.areas().into_iter().find(|area| area.id == id)
    }

    /// Changes which editor `id` shows.
    pub fn set_editor_type(&mut self, id: AreaId, editor_type: EditorTypeId) -> Result<(), AreaError> {
        let area = find_area_mut(&mut self.root, id).ok_or(AreaError::UnknownArea(id))?;
        area.editor_type = editor_type;
        Ok(())
    }

    pub fn layout(&self, bounds: Rect, metrics: &LayoutMetrics) -> Layout {
        let mut layout = Layout::default();
        layout_node(&self.root, bounds, &mut Vec::new(), metrics, &mut layout);
        layout
    }

    /// Seams win over areas, and parent seams over the seams that end on them.
    pub fn hit_test(&self, bounds: Rect, metrics: &LayoutMetrics, point: Point) -> Option<Hit> {
        let layout = self.layout(bounds, metrics);
        if let Some(seam) = layout
            .seams
            .iter()
            .find(|seam| seam.rect.inflate(metrics.seam_hit_slop).contains(point))
        {
            return Some(Hit::Seam(seam.path.clone()));
        }
        layout.area_at(point).map(Hit::Area)
    }

    /// Moves the seam of the split at `path` towards `pointer`.
    ///
    /// Like Blender, only areas touching the seam change size: areas further away
    /// keep their pixel size. The seam stops where an area touching it would drop
    /// below [`LayoutMetrics::min_area`].
    pub fn resize_split(
        &mut self,
        path: &SplitPath,
        pointer: Point,
        bounds: Rect,
        metrics: &LayoutMetrics,
    ) -> Result<(), AreaError> {
        let (split, rect) = split_at_mut(&mut self.root, path, bounds, metrics.gap).ok_or(AreaError::UnknownSplit)?;
        let axis = split.orientation.axis();
        let extent = rect.extent(axis);
        if extent <= 0.0 {
            return Err(AreaError::TooSmall);
        }
        let old_first = split.ratio * extent;
        let old_second = (extent - old_first - metrics.gap).max(0.0);
        let lowest = min_extent(&split.first, axis, MovingEdge::End, old_first, metrics);
        let highest = extent - metrics.gap - min_extent(&split.second, axis, MovingEdge::Start, old_second, metrics);
        if lowest > highest {
            return Err(AreaError::TooSmall);
        }

        let new_first = (pointer.along(axis) - rect.start(axis)).clamp(lowest, highest);
        let new_second = extent - new_first - metrics.gap;
        rebalance(&mut split.first, axis, old_first, new_first, MovingEdge::End, metrics.gap);
        rebalance(&mut split.second, axis, old_second, new_second, MovingEdge::Start, metrics.gap);
        split.ratio = new_first / extent;
        Ok(())
    }

    /// The seam rectangle [`AreaTree::split_area`] would create for the same
    /// arguments, for previewing a split before committing it.
    pub fn preview_split(
        &self,
        id: AreaId,
        orientation: Orientation,
        at: Point,
        bounds: Rect,
        metrics: &LayoutMetrics,
    ) -> Result<Rect, AreaError> {
        let (rect, first) = self.split_position(id, orientation, at, bounds, metrics)?;
        Ok(rect.seam(orientation.axis(), first, metrics.gap))
    }

    /// Splits area `id` with a seam of `orientation` through `at`, clamped so both
    /// halves keep the minimum size. The new area takes the second half (right or
    /// bottom) and shows the same editor type. Returns the new area's id.
    pub fn split_area(
        &mut self,
        id: AreaId,
        orientation: Orientation,
        at: Point,
        bounds: Rect,
        metrics: &LayoutMetrics,
    ) -> Result<AreaId, AreaError> {
        let (rect, first) = self.split_position(id, orientation, at, bounds, metrics)?;
        let extent = rect.extent(orientation.axis());
        let new_id = self.allocate_id();
        let node = find_node_mut(&mut self.root, id).ok_or(AreaError::UnknownArea(id))?;
        let LayoutNode::Area(area) = node else {
            return Err(AreaError::UnknownArea(id));
        };
        let new_area = Area {
            id: new_id,
            editor_type: area.editor_type.clone(),
        };
        let old = std::mem::replace(node, LayoutNode::Area(new_area.clone()));
        *node = LayoutNode::Split(Split {
            orientation,
            ratio: first / extent,
            first: Box::new(old),
            second: Box::new(LayoutNode::Area(new_area)),
        });
        Ok(new_id)
    }

    /// The areas whose size changes when the seam at `path` moves: those with an
    /// edge on the seam, on either side of it.
    pub fn areas_touching_seam(&self, path: &SplitPath, bounds: Rect, metrics: &LayoutMetrics) -> Vec<AreaId> {
        const TOLERANCE: f32 = 0.5;
        let layout = self.layout(bounds, metrics);
        let Some(seam) = layout.seam(path) else {
            return Vec::new();
        };
        let axis = seam.orientation.axis();
        let cross = axis.cross();
        let seam_start = seam.rect.start(axis);
        let seam_end = seam_start + seam.rect.extent(axis);
        let span_start = seam.rect.start(cross);
        let span_end = span_start + seam.rect.extent(cross);
        layout
            .areas
            .iter()
            .filter(|area| {
                let start = area.rect.start(axis);
                let end = start + area.rect.extent(axis);
                let on_seam = (end - seam_start).abs() < TOLERANCE || (start - seam_end).abs() < TOLERANCE;
                let area_start = area.rect.start(cross);
                let area_end = area_start + area.rect.extent(cross);
                on_seam && area_start < span_end - TOLERANCE && area_end > span_start + TOLERANCE
            })
            .map(|area| area.id)
            .collect()
    }

    /// The areas on either side of the seam at `path`, measured at the point on
    /// the seam nearest `at`: (left, right) for a vertical seam, (top, bottom) for
    /// a horizontal one.
    pub fn areas_across_seam(
        &self,
        path: &SplitPath,
        at: Point,
        bounds: Rect,
        metrics: &LayoutMetrics,
    ) -> Option<(AreaId, AreaId)> {
        let layout = self.layout(bounds, metrics);
        let seam = layout.seam(path)?;
        let axis = seam.orientation.axis();
        let cross = axis.cross();
        let along_seam = at
            .along(cross)
            .clamp(seam.rect.start(cross), seam.rect.start(cross) + seam.rect.extent(cross) - f32::EPSILON);
        let before = seam.rect.start(axis) - 0.5;
        let after = seam.rect.start(axis) + seam.rect.extent(axis) + 0.5;
        let probe = |coordinate: f32| match axis {
            Axis::X => Point::new(coordinate, along_seam),
            Axis::Y => Point::new(along_seam, coordinate),
        };
        Some((layout.area_at(probe(before))?, layout.area_at(probe(after))?))
    }

    /// The joins valid across the seam at `path` at position `at`, in the order
    /// Up, Down, Left, Right. "Join Right" keeps the left area and removes the
    /// right one; the other directions follow the same rule.
    pub fn seam_join_options(
        &self,
        path: &SplitPath,
        at: Point,
        bounds: Rect,
        metrics: &LayoutMetrics,
    ) -> Vec<JoinOption> {
        let Some(layout_seam) = self.layout(bounds, metrics).seam(path).cloned() else {
            return Vec::new();
        };
        let Some((before, after)) = self.areas_across_seam(path, at, bounds, metrics) else {
            return Vec::new();
        };
        let candidates = match layout_seam.orientation {
            Orientation::Horizontal => [
                (JoinDirection::Up, after, before),
                (JoinDirection::Down, before, after),
            ],
            Orientation::Vertical => [
                (JoinDirection::Left, after, before),
                (JoinDirection::Right, before, after),
            ],
        };
        candidates
            .into_iter()
            .filter(|(_, keep, remove)| self.can_join(*keep, *remove))
            .map(|(direction, keep, remove)| JoinOption { direction, keep, remove })
            .collect()
    }

    /// Whether `keep` can be joined over `remove`: they must share an edge, and
    /// one area's side of that edge must contain the other's.
    pub fn can_join(&self, keep: AreaId, remove: AreaId) -> bool {
        join::plan(self, keep, remove).is_ok()
    }

    /// Joins `remove` into `keep`, Blender-style. When the shared edge is only
    /// partial, the area with the longer edge is first split so the edges align;
    /// its leftover pieces stay as separate areas showing the same editor.
    /// `keep` survives, grown over the aligned part of `remove`.
    pub fn join(&mut self, keep: AreaId, remove: AreaId) -> Result<JoinOutcome, AreaError> {
        let plan = join::plan(self, keep, remove)?;
        let before: Vec<AreaId> = self.areas().iter().map(|area| area.id).collect();
        let target = node_at_path_mut(&mut self.root, &plan.subtree_path).ok_or(AreaError::UnknownSplit)?;
        *target = plan.subtree;
        self.next_id = plan.next_id;

        let after: Vec<AreaId> = self.areas().iter().map(|area| area.id).collect();
        Ok(JoinOutcome {
            created: after.iter().filter(|id| !before.contains(id)).copied().collect(),
            removed: before.iter().filter(|id| !after.contains(id)).copied().collect(),
        })
    }

    pub(crate) fn next_id(&self) -> u32 {
        self.next_id
    }

    /// Area `id`'s rectangle and the extent its first half would have when split
    /// through `at`, clamped to the minimum area size.
    fn split_position(
        &self,
        id: AreaId,
        orientation: Orientation,
        at: Point,
        bounds: Rect,
        metrics: &LayoutMetrics,
    ) -> Result<(Rect, f32), AreaError> {
        let rect = self
            .layout(bounds, metrics)
            .area_rect(id)
            .ok_or(AreaError::UnknownArea(id))?;
        let axis = orientation.axis();
        let extent = rect.extent(axis);
        let min = metrics.min_area.along(axis);
        let highest = extent - metrics.gap - min;
        if extent <= 0.0 || min > highest {
            return Err(AreaError::TooSmall);
        }
        Ok((rect, (at.along(axis) - rect.start(axis)).clamp(min, highest)))
    }

    fn allocate_id(&mut self) -> AreaId {
        let id = AreaId(self.next_id);
        self.next_id += 1;
        id
    }
}

fn validate(node: &LayoutNode, ids: &mut Vec<AreaId>) -> Result<(), AreaError> {
    match node {
        LayoutNode::Area(area) => {
            ids.push(area.id);
            Ok(())
        }
        LayoutNode::Split(split) => {
            if !(split.ratio > 0.0 && split.ratio < 1.0) {
                return Err(AreaError::Invalid(format!("split ratio {} is outside (0, 1)", split.ratio)));
            }
            validate(&split.first, ids)?;
            validate(&split.second, ids)
        }
    }
}

fn collect_areas<'a>(node: &'a LayoutNode, out: &mut Vec<&'a Area>) {
    match node {
        LayoutNode::Area(area) => out.push(area),
        LayoutNode::Split(split) => {
            collect_areas(&split.first, out);
            collect_areas(&split.second, out);
        }
    }
}

fn find_node_mut(node: &mut LayoutNode, id: AreaId) -> Option<&mut LayoutNode> {
    match node {
        LayoutNode::Area(area) if area.id == id => Some(node),
        LayoutNode::Area(_) => None,
        LayoutNode::Split(split) => {
            find_node_mut(&mut split.first, id).or_else(|| find_node_mut(&mut split.second, id))
        }
    }
}

fn find_area_mut(node: &mut LayoutNode, id: AreaId) -> Option<&mut Area> {
    match find_node_mut(node, id)? {
        LayoutNode::Area(area) => Some(area),
        LayoutNode::Split(_) => None,
    }
}

pub(crate) fn node_at_path<'a>(node: &'a LayoutNode, path: &[Branch]) -> Option<&'a LayoutNode> {
    let Some((branch, rest)) = path.split_first() else {
        return Some(node);
    };
    let LayoutNode::Split(split) = node else {
        return None;
    };
    match branch {
        Branch::First => node_at_path(&split.first, rest),
        Branch::Second => node_at_path(&split.second, rest),
    }
}

fn node_at_path_mut<'a>(node: &'a mut LayoutNode, path: &[Branch]) -> Option<&'a mut LayoutNode> {
    let Some((branch, rest)) = path.split_first() else {
        return Some(node);
    };
    let LayoutNode::Split(split) = node else {
        return None;
    };
    match branch {
        Branch::First => node_at_path_mut(&mut split.first, rest),
        Branch::Second => node_at_path_mut(&mut split.second, rest),
    }
}

/// The split at `path` together with the rectangle it occupies inside `bounds`.
fn split_at_mut<'a>(
    node: &'a mut LayoutNode,
    path: &SplitPath,
    bounds: Rect,
    gap: f32,
) -> Option<(&'a mut Split, Rect)> {
    let mut node = node;
    let mut rect = bounds;
    for branch in path.branches() {
        let LayoutNode::Split(split) = node else {
            return None;
        };
        let axis = split.orientation.axis();
        let (first, second) = rect.split(axis, split.ratio * rect.extent(axis), gap);
        (node, rect) = match branch {
            Branch::First => (&mut *split.first, first),
            Branch::Second => (&mut *split.second, second),
        };
    }
    match node {
        LayoutNode::Split(split) => Some((split, rect)),
        LayoutNode::Area(_) => None,
    }
}

fn layout_node(node: &LayoutNode, rect: Rect, path: &mut Vec<Branch>, metrics: &LayoutMetrics, out: &mut Layout) {
    match node {
        LayoutNode::Area(area) => out.areas.push(AreaLayout { id: area.id, rect }),
        LayoutNode::Split(split) => {
            let axis = split.orientation.axis();
            let first_extent = split.ratio * rect.extent(axis);
            let (first, second) = rect.split(axis, first_extent, metrics.gap);
            out.seams.push(SeamLayout {
                path: SplitPath(path.clone()),
                orientation: split.orientation,
                rect: rect.seam(axis, first_extent, metrics.gap),
            });
            path.push(Branch::First);
            layout_node(&split.first, first, path, metrics, out);
            path.pop();
            path.push(Branch::Second);
            layout_node(&split.second, second, path, metrics, out);
            path.pop();
        }
    }
}

/// The smallest extent along `axis` that `node` (currently `extent` wide) can be
/// resized to when its `moving` edge follows a seam, with areas away from that
/// edge keeping their size.
fn min_extent(node: &LayoutNode, axis: Axis, moving: MovingEdge, extent: f32, metrics: &LayoutMetrics) -> f32 {
    match node {
        LayoutNode::Area(_) => metrics.min_area.along(axis),
        LayoutNode::Split(split) if split.orientation.axis() == axis => {
            let first = split.ratio * extent;
            let second = (extent - first - metrics.gap).max(0.0);
            match moving {
                MovingEdge::End => first + metrics.gap + min_extent(&split.second, axis, moving, second, metrics),
                MovingEdge::Start => second + metrics.gap + min_extent(&split.first, axis, moving, first, metrics),
            }
        }
        LayoutNode::Split(split) => min_extent(&split.first, axis, moving, extent, metrics)
            .max(min_extent(&split.second, axis, moving, extent, metrics)),
    }
}

/// Re-derives ratios inside `node` after its extent along `axis` changes from
/// `old_extent` to `new_extent`, so only the areas touching the `moving` edge
/// absorb the change.
fn rebalance(node: &mut LayoutNode, axis: Axis, old_extent: f32, new_extent: f32, moving: MovingEdge, gap: f32) {
    let LayoutNode::Split(split) = node else {
        return;
    };
    if new_extent <= 0.0 {
        return;
    }
    if split.orientation.axis() != axis {
        rebalance(&mut split.first, axis, old_extent, new_extent, moving, gap);
        rebalance(&mut split.second, axis, old_extent, new_extent, moving, gap);
        return;
    }
    let old_first = split.ratio * old_extent;
    let old_second = (old_extent - old_first - gap).max(0.0);
    match moving {
        MovingEdge::End => {
            split.ratio = old_first / new_extent;
            let new_second = new_extent - old_first - gap;
            rebalance(&mut split.second, axis, old_second, new_second, moving, gap);
        }
        MovingEdge::Start => {
            let new_first = new_extent - gap - old_second;
            split.ratio = new_first / new_extent;
            rebalance(&mut split.first, axis, old_first, new_first, moving, gap);
        }
    }
}
