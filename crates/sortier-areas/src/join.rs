//! Join planning. Joins are computed on the areas' rectangles in a unit square
//! (no gaps), then the affected subtree is rebuilt from those rectangles by
//! repeatedly finding a cut that runs the full length of the region.

use crate::geometry::{Axis, Orientation};
use crate::tree::{Area, AreaError, AreaId, AreaTree, Branch, EditorTypeId, LayoutNode, Split, node_at_path};

/// Coordinates closer than this are treated as equal. Ratios are stored as f32,
/// so recomputed edges drift by roughly 1e-7.
const EPSILON: f64 = 1e-4;

#[derive(Clone, Copy, Debug, PartialEq)]
struct UnitRect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl UnitRect {
    const FULL: UnitRect = UnitRect {
        x0: 0.0,
        y0: 0.0,
        x1: 1.0,
        y1: 1.0,
    };

    fn start(&self, axis: Axis) -> f64 {
        match axis {
            Axis::X => self.x0,
            Axis::Y => self.y0,
        }
    }

    fn end(&self, axis: Axis) -> f64 {
        match axis {
            Axis::X => self.x1,
            Axis::Y => self.y1,
        }
    }

    fn with_span(&self, axis: Axis, start: f64, end: f64) -> UnitRect {
        match axis {
            Axis::X => UnitRect {
                x0: start,
                x1: end,
                ..*self
            },
            Axis::Y => UnitRect {
                y0: start,
                y1: end,
                ..*self
            },
        }
    }

    fn split(&self, axis: Axis, ratio: f64) -> (UnitRect, UnitRect) {
        let cut = self.start(axis) + ratio * (self.end(axis) - self.start(axis));
        (
            self.with_span(axis, self.start(axis), cut),
            self.with_span(axis, cut, self.end(axis)),
        )
    }

    fn union(&self, other: &UnitRect) -> UnitRect {
        UnitRect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    fn contains_rect(&self, other: &UnitRect) -> bool {
        other.x0 >= self.x0 - EPSILON
            && other.y0 >= self.y0 - EPSILON
            && other.x1 <= self.x1 + EPSILON
            && other.y1 <= self.y1 + EPSILON
    }
}

#[derive(Clone, Debug)]
struct Piece {
    id: AreaId,
    editor_type: EditorTypeId,
    rect: UnitRect,
}

/// A computed join, ready to apply: replace the node at `subtree_path` with
/// `subtree` and continue id allocation from `next_id`.
pub(crate) struct JoinPlan {
    pub(crate) subtree_path: Vec<Branch>,
    pub(crate) subtree: LayoutNode,
    pub(crate) next_id: u32,
}

pub(crate) fn plan(tree: &AreaTree, keep: AreaId, remove: AreaId) -> Result<JoinPlan, AreaError> {
    if keep == remove {
        return Err(AreaError::SameArea);
    }
    let mut pieces = Vec::new();
    let mut paths = Vec::new();
    collect_pieces(tree.root(), UnitRect::FULL, &mut Vec::new(), &mut pieces, &mut paths);
    let index_of = |id: AreaId| pieces.iter().position(|piece| piece.id == id).ok_or(AreaError::UnknownArea(id));
    let keep_index = index_of(keep)?;
    let remove_index = index_of(remove)?;
    let kept = pieces[keep_index].clone();
    let removed = pieces[remove_index].clone();

    let axis = shared_edge_axis(&kept.rect, &removed.rect).ok_or(AreaError::NotAdjacent)?;
    let cross = axis.cross();
    let (k0, k1) = (kept.rect.start(cross), kept.rect.end(cross));
    let (r0, r1) = (removed.rect.start(cross), removed.rect.end(cross));
    let kept_contains_removed = k0 <= r0 + EPSILON && r1 <= k1 + EPSILON;
    let removed_contains_kept = r0 <= k0 + EPSILON && k1 <= r1 + EPSILON;
    if !kept_contains_removed && !removed_contains_kept {
        return Err(AreaError::Staggered);
    }
    let (span_start, span_end) = (k0.max(r0), k1.min(r1));

    let mut next_id = tree.next_id();
    let mut allocate = || {
        let id = AreaId(next_id);
        next_id += 1;
        id
    };

    let mut result: Vec<Piece> = pieces
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != keep_index && *index != remove_index)
        .map(|(_, piece)| piece.clone())
        .collect();
    let (kept_middle, kept_leftovers) = cut_to_span(&kept.rect, cross, span_start, span_end);
    for rect in kept_leftovers {
        result.push(Piece {
            id: allocate(),
            editor_type: kept.editor_type.clone(),
            rect,
        });
    }
    let (removed_middle, removed_leftovers) = cut_to_span(&removed.rect, cross, span_start, span_end);
    // The first leftover of the removed area keeps its id, so whatever state is
    // attached to that area survives on the piece that remains visible.
    for (index, rect) in removed_leftovers.into_iter().enumerate() {
        let id = if index == 0 { removed.id } else { allocate() };
        result.push(Piece {
            id,
            editor_type: removed.editor_type.clone(),
            rect,
        });
    }
    result.push(Piece {
        id: kept.id,
        editor_type: kept.editor_type.clone(),
        rect: kept_middle.union(&removed_middle),
    });

    let subtree_path = common_prefix(&paths[keep_index], &paths[remove_index]);
    let region = region_of(tree.root(), &subtree_path);
    let preferred = match node_at_path(tree.root(), &subtree_path) {
        Some(LayoutNode::Split(split)) => split.orientation,
        _ => Orientation::Vertical,
    };
    let inside: Vec<Piece> = result.into_iter().filter(|piece| region.contains_rect(&piece.rect)).collect();
    let subtree = rebuild(&inside, region, preferred).ok_or(AreaError::NotRepresentable)?;
    Ok(JoinPlan {
        subtree_path,
        subtree,
        next_id,
    })
}

fn collect_pieces(
    node: &LayoutNode,
    rect: UnitRect,
    path: &mut Vec<Branch>,
    pieces: &mut Vec<Piece>,
    paths: &mut Vec<Vec<Branch>>,
) {
    match node {
        LayoutNode::Area(area) => {
            pieces.push(Piece {
                id: area.id,
                editor_type: area.editor_type.clone(),
                rect,
            });
            paths.push(path.clone());
        }
        LayoutNode::Split(split) => {
            let (first, second) = rect.split(split.orientation.axis(), f64::from(split.ratio));
            path.push(Branch::First);
            collect_pieces(&split.first, first, path, pieces, paths);
            path.pop();
            path.push(Branch::Second);
            collect_pieces(&split.second, second, path, pieces, paths);
            path.pop();
        }
    }
}

/// The axis along which `a` and `b` touch edge to edge with a non-empty overlap
/// on the other axis.
fn shared_edge_axis(a: &UnitRect, b: &UnitRect) -> Option<Axis> {
    [Axis::X, Axis::Y].into_iter().find(|&axis| {
        let touching = (a.end(axis) - b.start(axis)).abs() < EPSILON || (b.end(axis) - a.start(axis)).abs() < EPSILON;
        let cross = axis.cross();
        let overlap = a.end(cross).min(b.end(cross)) - a.start(cross).max(b.start(cross));
        touching && overlap > EPSILON
    })
}

/// Cuts `rect` along `axis` at `start` and `end`, returning the middle part and
/// the non-empty parts before and after it.
fn cut_to_span(rect: &UnitRect, axis: Axis, start: f64, end: f64) -> (UnitRect, Vec<UnitRect>) {
    let mut leftovers = Vec::new();
    if start - rect.start(axis) > EPSILON {
        leftovers.push(rect.with_span(axis, rect.start(axis), start));
    }
    if rect.end(axis) - end > EPSILON {
        leftovers.push(rect.with_span(axis, end, rect.end(axis)));
    }
    (rect.with_span(axis, start, end), leftovers)
}

fn common_prefix(a: &[Branch], b: &[Branch]) -> Vec<Branch> {
    a.iter().zip(b).take_while(|(x, y)| x == y).map(|(x, _)| *x).collect()
}

fn region_of(root: &LayoutNode, path: &[Branch]) -> UnitRect {
    let mut node = root;
    let mut rect = UnitRect::FULL;
    for branch in path {
        let LayoutNode::Split(split) = node else {
            break;
        };
        let (first, second) = rect.split(split.orientation.axis(), f64::from(split.ratio));
        (node, rect) = match branch {
            Branch::First => (&*split.first, first),
            Branch::Second => (&*split.second, second),
        };
    }
    rect
}

/// Builds a split tree whose leaves are exactly `pieces`, which must tile
/// `region`. Returns `None` when no full-length cut exists at some level.
fn rebuild(pieces: &[Piece], region: UnitRect, preferred: Orientation) -> Option<LayoutNode> {
    if let [piece] = pieces {
        return Some(LayoutNode::Area(Area {
            id: piece.id,
            editor_type: piece.editor_type.clone(),
        }));
    }
    let other = match preferred {
        Orientation::Vertical => Orientation::Horizontal,
        Orientation::Horizontal => Orientation::Vertical,
    };
    for orientation in [preferred, other] {
        let axis = orientation.axis();
        let mut cuts: Vec<f64> = pieces
            .iter()
            .map(|piece| piece.rect.start(axis))
            .filter(|&start| start > region.start(axis) + EPSILON)
            .collect();
        cuts.sort_by(f64::total_cmp);
        let full_cut = cuts.into_iter().find(|&cut| {
            pieces
                .iter()
                .all(|piece| piece.rect.end(axis) <= cut + EPSILON || piece.rect.start(axis) >= cut - EPSILON)
        });
        let Some(cut) = full_cut else {
            continue;
        };
        let (before, after): (Vec<Piece>, Vec<Piece>) =
            pieces.iter().cloned().partition(|piece| piece.rect.end(axis) <= cut + EPSILON);
        let ratio = (cut - region.start(axis)) / (region.end(axis) - region.start(axis));
        let first_region = region.with_span(axis, region.start(axis), cut);
        let second_region = region.with_span(axis, cut, region.end(axis));
        return Some(LayoutNode::Split(Split {
            orientation,
            ratio: ratio as f32,
            first: Box::new(rebuild(&before, first_region, orientation)?),
            second: Box::new(rebuild(&after, second_region, orientation)?),
        }));
    }
    None
}
