use crate::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 600.0);
const METRICS: LayoutMetrics = LayoutMetrics {
    gap: 2.0,
    min_area: Size::new(100.0, 60.0),
    seam_hit_slop: 3.0,
};

const VIEWPORT: &str = "test.viewport";
const PROPERTIES: &str = "test.properties";
const OUTLINER: &str = "test.outliner";

fn area(id: u32, editor: &str) -> LayoutNode {
    LayoutNode::Area(Area {
        id: AreaId(id),
        editor_type: EditorTypeId::new(editor),
    })
}

fn split(orientation: Orientation, ratio: f32, first: LayoutNode, second: LayoutNode) -> LayoutNode {
    LayoutNode::Split(Split {
        orientation,
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    })
}

fn tree(root: LayoutNode) -> AreaTree {
    AreaTree::from_root(root).expect("valid test tree")
}

fn rect_of(tree: &AreaTree, id: u32) -> Rect {
    tree.layout(BOUNDS, &METRICS).area_rect(AreaId(id)).expect("area exists")
}

fn assert_close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.01, "expected {expected}, got {actual}");
}

fn ratio_of(node: &LayoutNode) -> f32 {
    match node {
        LayoutNode::Split(split) => split.ratio,
        LayoutNode::Area(_) => panic!("expected a split"),
    }
}

/// Viewport on the left; properties above the outliner on the right.
fn viewport_beside_stack() -> AreaTree {
    tree(split(
        Orientation::Vertical,
        0.5,
        area(1, VIEWPORT),
        split(Orientation::Horizontal, 0.5, area(2, PROPERTIES), area(3, OUTLINER)),
    ))
}

/// Left column split at 30%, right column split at 60%.
fn staggered_columns() -> AreaTree {
    tree(split(
        Orientation::Vertical,
        0.5,
        split(Orientation::Horizontal, 0.3, area(1, VIEWPORT), area(2, VIEWPORT)),
        split(Orientation::Horizontal, 0.6, area(3, OUTLINER), area(4, OUTLINER)),
    ))
}

#[test]
fn single_area_fills_bounds() {
    let tree = AreaTree::new(EditorTypeId::new(VIEWPORT));
    let layout = tree.layout(BOUNDS, &METRICS);
    assert_eq!(layout.areas.len(), 1);
    assert_eq!(layout.areas[0].rect, BOUNDS);
    assert!(layout.seams.is_empty());
}

#[test]
fn layout_gives_first_child_the_ratio_and_puts_the_gap_before_the_second() {
    let tree = viewport_beside_stack();
    assert_eq!(rect_of(&tree, 1), Rect::new(0.0, 0.0, 500.0, 600.0));
    assert_eq!(rect_of(&tree, 2), Rect::new(502.0, 0.0, 498.0, 300.0));
    assert_eq!(rect_of(&tree, 3), Rect::new(502.0, 302.0, 498.0, 298.0));
    let layout = tree.layout(BOUNDS, &METRICS);
    assert_eq!(layout.seams[0].rect, Rect::new(500.0, 0.0, 2.0, 600.0));
}

#[test]
fn hit_test_prefers_seams_within_slop() {
    let tree = viewport_beside_stack();
    let root = SplitPath::root();
    assert_eq!(tree.hit_test(BOUNDS, &METRICS, Point::new(501.0, 100.0)), Some(Hit::Seam(root.clone())));
    assert_eq!(tree.hit_test(BOUNDS, &METRICS, Point::new(504.0, 100.0)), Some(Hit::Seam(root)));
    assert_eq!(
        tree.hit_test(BOUNDS, &METRICS, Point::new(700.0, 301.0)),
        Some(Hit::Seam(SplitPath::root().child(Branch::Second)))
    );
    assert_eq!(tree.hit_test(BOUNDS, &METRICS, Point::new(300.0, 100.0)), Some(Hit::Area(AreaId(1))));
    assert_eq!(tree.hit_test(BOUNDS, &METRICS, Point::new(800.0, 500.0)), Some(Hit::Area(AreaId(3))));
    assert_eq!(tree.hit_test(BOUNDS, &METRICS, Point::new(-5.0, 100.0)), None);
}

#[test]
fn split_area_at_pointer_creates_same_editor_in_second_half() {
    let mut tree = AreaTree::new(EditorTypeId::new(VIEWPORT));
    let new_id = tree.split_area(AreaId(1), Orientation::Vertical, Point::new(300.0, 50.0), BOUNDS, &METRICS).unwrap();
    assert_eq!(new_id, AreaId(2));
    assert_close(ratio_of(tree.root()), 0.3);
    assert_eq!(tree.area(new_id).unwrap().editor_type, EditorTypeId::new(VIEWPORT));
    assert_eq!(rect_of(&tree, 2), Rect::new(302.0, 0.0, 698.0, 600.0));
}

#[test]
fn preview_split_matches_the_seam_split_area_creates() {
    let mut tree = viewport_beside_stack();
    let at = Point::new(800.0, 120.0);
    let preview = tree.preview_split(AreaId(2), Orientation::Vertical, at, BOUNDS, &METRICS).unwrap();
    tree.split_area(AreaId(2), Orientation::Vertical, at, BOUNDS, &METRICS).unwrap();
    let path = SplitPath::root().child(Branch::Second).child(Branch::First);
    let created = tree.layout(BOUNDS, &METRICS).seam(&path).unwrap().rect;
    assert_eq!(preview, created);
}

#[test]
fn split_area_clamps_to_minimum_size() {
    let mut tree = AreaTree::new(EditorTypeId::new(VIEWPORT));
    tree.split_area(AreaId(1), Orientation::Horizontal, Point::new(10.0, 5.0), BOUNDS, &METRICS).unwrap();
    assert_close(rect_of(&tree, 1).size.height, 60.0);
}

#[test]
fn split_area_refuses_areas_too_small_for_two_halves() {
    let mut tree = tree(split(Orientation::Vertical, 0.85, area(1, VIEWPORT), area(2, VIEWPORT)));
    let result = tree.split_area(AreaId(2), Orientation::Vertical, Point::new(950.0, 10.0), BOUNDS, &METRICS);
    assert_eq!(result, Err(AreaError::TooSmall));
}

#[test]
fn new_ids_are_never_reused() {
    let mut tree = viewport_beside_stack();
    let id = tree.split_area(AreaId(3), Orientation::Vertical, Point::new(750.0, 450.0), BOUNDS, &METRICS).unwrap();
    assert_eq!(id, AreaId(4));
    tree.join(AreaId(3), id).unwrap();
    let again = tree.split_area(AreaId(3), Orientation::Vertical, Point::new(750.0, 450.0), BOUNDS, &METRICS).unwrap();
    assert_eq!(again, AreaId(5));
}

#[test]
fn resize_moves_only_areas_touching_the_seam() {
    let mut tree = tree(split(
        Orientation::Vertical,
        0.5,
        area(1, VIEWPORT),
        split(Orientation::Vertical, 0.5, area(2, VIEWPORT), area(3, VIEWPORT)),
    ));
    let far_width = rect_of(&tree, 3).size.width;
    tree.resize_split(&SplitPath::root(), Point::new(400.0, 0.0), BOUNDS, &METRICS).unwrap();
    assert_close(rect_of(&tree, 1).size.width, 400.0);
    assert_close(rect_of(&tree, 3).size.width, far_width);
    assert_close(rect_of(&tree, 2).size.width, 598.0 - 2.0 - far_width);
}

#[test]
fn resize_stops_at_minimum_size_of_touching_areas() {
    let mut tree = tree(split(
        Orientation::Vertical,
        0.5,
        area(1, VIEWPORT),
        split(Orientation::Vertical, 0.5, area(2, VIEWPORT), area(3, VIEWPORT)),
    ));
    tree.resize_split(&SplitPath::root(), Point::new(990.0, 0.0), BOUNDS, &METRICS).unwrap();
    assert_close(rect_of(&tree, 2).size.width, 100.0);
    tree.resize_split(&SplitPath::root(), Point::new(-50.0, 0.0), BOUNDS, &METRICS).unwrap();
    assert_close(rect_of(&tree, 1).size.width, 100.0);
}

#[test]
fn resize_across_nested_cross_splits_respects_every_touching_area() {
    let mut tree = viewport_beside_stack();
    tree.resize_split(&SplitPath::root(), Point::new(0.0, 0.0), BOUNDS, &METRICS).unwrap();
    assert_close(rect_of(&tree, 1).size.width, 100.0);
    assert_close(rect_of(&tree, 2).size.width, 898.0);
    assert_close(rect_of(&tree, 3).size.width, 898.0);
}

#[test]
fn resize_unknown_split_is_an_error() {
    let mut tree = AreaTree::new(EditorTypeId::new(VIEWPORT));
    let result = tree.resize_split(&SplitPath::root(), Point::new(10.0, 10.0), BOUNDS, &METRICS);
    assert_eq!(result, Err(AreaError::UnknownSplit));
}

#[test]
fn areas_touching_seam_include_every_area_on_either_side() {
    let tree = viewport_beside_stack();
    let mut touching = tree.areas_touching_seam(&SplitPath::root(), BOUNDS, &METRICS);
    touching.sort();
    assert_eq!(touching, vec![AreaId(1), AreaId(2), AreaId(3)]);
    let inner = tree.areas_touching_seam(&SplitPath::root().child(Branch::Second), BOUNDS, &METRICS);
    assert_eq!(inner, vec![AreaId(2), AreaId(3)]);
}

#[test]
fn areas_touching_seam_exclude_areas_away_from_it() {
    let tree = tree(split(
        Orientation::Vertical,
        0.5,
        area(1, VIEWPORT),
        split(Orientation::Vertical, 0.5, area(2, VIEWPORT), area(3, VIEWPORT)),
    ));
    assert_eq!(
        tree.areas_touching_seam(&SplitPath::root(), BOUNDS, &METRICS),
        vec![AreaId(1), AreaId(2)]
    );
}

#[test]
fn areas_across_seam_depend_on_click_position() {
    let tree = viewport_beside_stack();
    let root = SplitPath::root();
    assert_eq!(
        tree.areas_across_seam(&root, Point::new(501.0, 100.0), BOUNDS, &METRICS),
        Some((AreaId(1), AreaId(2)))
    );
    assert_eq!(
        tree.areas_across_seam(&root, Point::new(501.0, 500.0), BOUNDS, &METRICS),
        Some((AreaId(1), AreaId(3)))
    );
}

#[test]
fn vertical_seam_offers_left_and_right_joins() {
    let tree = viewport_beside_stack();
    let options = tree.seam_join_options(&SplitPath::root(), Point::new(501.0, 100.0), BOUNDS, &METRICS);
    assert_eq!(
        options,
        vec![
            JoinOption {
                direction: JoinDirection::Left,
                keep: AreaId(2),
                remove: AreaId(1),
            },
            JoinOption {
                direction: JoinDirection::Right,
                keep: AreaId(1),
                remove: AreaId(2),
            },
        ]
    );
}

#[test]
fn horizontal_seam_offers_up_and_down_joins() {
    let tree = viewport_beside_stack();
    let path = SplitPath::root().child(Branch::Second);
    let options = tree.seam_join_options(&path, Point::new(700.0, 301.0), BOUNDS, &METRICS);
    let directions: Vec<_> = options.iter().map(|option| option.direction).collect();
    assert_eq!(directions, vec![JoinDirection::Up, JoinDirection::Down]);
    assert_eq!((options[1].keep, options[1].remove), (AreaId(2), AreaId(3)));
}

#[test]
fn staggered_seam_offers_no_join_where_edges_overlap_partially() {
    let tree = staggered_columns();
    // At y = 300 the seam separates area 2 (y 180..600) from area 3 (y 0..360).
    let options = tree.seam_join_options(&SplitPath::root(), Point::new(501.0, 300.0), BOUNDS, &METRICS);
    assert!(options.is_empty(), "areas 2 and 3 are staggered: {options:?}");
    // At y = 400 it separates area 2 from area 4 (y 362..600), whose edge area 2 contains.
    let options = tree.seam_join_options(&SplitPath::root(), Point::new(501.0, 400.0), BOUNDS, &METRICS);
    assert_eq!(options.len(), 2);
}

#[test]
fn joining_siblings_with_equal_edges_removes_the_split() {
    let mut tree = tree(split(Orientation::Vertical, 0.4, area(1, VIEWPORT), area(2, OUTLINER)));
    let outcome = tree.join(AreaId(1), AreaId(2)).unwrap();
    assert_eq!(tree.root(), &area(1, VIEWPORT));
    assert_eq!(
        outcome,
        JoinOutcome {
            created: vec![],
            removed: vec![AreaId(2)],
        }
    );
}

#[test]
fn join_left_keeps_the_right_area() {
    let mut tree = tree(split(Orientation::Vertical, 0.4, area(1, VIEWPORT), area(2, OUTLINER)));
    tree.join(AreaId(2), AreaId(1)).unwrap();
    assert_eq!(tree.root(), &area(2, OUTLINER));
}

#[test]
fn joining_into_a_nested_same_axis_subtree_absorbs_the_neighbour() {
    let mut tree = tree(split(
        Orientation::Vertical,
        0.5,
        area(1, VIEWPORT),
        split(Orientation::Vertical, 0.5, area(2, PROPERTIES), area(3, OUTLINER)),
    ));
    tree.join(AreaId(1), AreaId(2)).unwrap();
    let LayoutNode::Split(root) = tree.root() else {
        panic!("expected a split");
    };
    assert_close(root.ratio, 0.75);
    assert_eq!(*root.first, area(1, VIEWPORT));
    assert_eq!(*root.second, area(3, OUTLINER));
}

#[test]
fn join_from_the_taller_area_leaves_its_remainder_as_a_new_area() {
    let mut tree = viewport_beside_stack();
    let outcome = tree.join(AreaId(1), AreaId(2)).unwrap();
    assert_eq!(
        outcome,
        JoinOutcome {
            created: vec![AreaId(4)],
            removed: vec![AreaId(2)],
        }
    );
    let expected = split(
        Orientation::Horizontal,
        0.5,
        area(1, VIEWPORT),
        split(Orientation::Vertical, 0.5, area(4, VIEWPORT), area(3, OUTLINER)),
    );
    assert_eq!(tree.root(), &expected);
    assert_eq!(rect_of(&tree, 1).size.width, 1000.0);
}

#[test]
fn join_into_the_taller_area_splits_it_and_keeps_its_id_on_the_remainder() {
    let mut tree = viewport_beside_stack();
    let outcome = tree.join(AreaId(2), AreaId(1)).unwrap();
    assert_eq!(outcome, JoinOutcome::default());
    let expected = split(
        Orientation::Horizontal,
        0.5,
        area(2, PROPERTIES),
        split(Orientation::Vertical, 0.5, area(1, VIEWPORT), area(3, OUTLINER)),
    );
    assert_eq!(tree.root(), &expected);
}

#[test]
fn join_into_area_containing_the_edge_splits_both_ends_when_needed() {
    let mut tree = tree(split(
        Orientation::Vertical,
        0.5,
        area(1, VIEWPORT),
        split(
            Orientation::Horizontal,
            0.25,
            area(2, OUTLINER),
            split(Orientation::Horizontal, 0.5, area(3, PROPERTIES), area(4, OUTLINER)),
        ),
    ));
    let outcome = tree.join(AreaId(1), AreaId(3)).unwrap();
    assert_eq!(outcome.created, vec![AreaId(5), AreaId(6)]);
    let merged = rect_of(&tree, 1);
    assert_close(merged.size.width, 1000.0);
    for id in [2, 4, 5, 6] {
        assert!(rect_of(&tree, id).size.width < 600.0, "area {id} should stay in one column");
    }
}

#[test]
fn staggered_areas_cannot_join() {
    let mut tree = staggered_columns();
    assert!(!tree.can_join(AreaId(2), AreaId(3)));
    assert_eq!(tree.join(AreaId(2), AreaId(3)), Err(AreaError::Staggered));
}

#[test]
fn area_contained_in_neighbours_edge_can_join_across_nested_splits() {
    let mut tree = staggered_columns();
    assert!(tree.can_join(AreaId(3), AreaId(1)));
    tree.join(AreaId(3), AreaId(1)).unwrap();
    let merged = rect_of(&tree, 3);
    assert_close(merged.size.width, 1000.0);
    assert_close(merged.size.height, 0.3 * 600.0);
}

#[test]
fn diagonal_areas_are_not_adjacent() {
    let tree = staggered_columns();
    assert!(!tree.can_join(AreaId(1), AreaId(4)));
    let mut tree = tree;
    assert_eq!(tree.join(AreaId(1), AreaId(4)), Err(AreaError::NotAdjacent));
}

#[test]
fn joining_an_area_with_itself_is_rejected() {
    let mut tree = viewport_beside_stack();
    assert_eq!(tree.join(AreaId(1), AreaId(1)), Err(AreaError::SameArea));
}

#[test]
fn set_editor_type_changes_only_that_area() {
    let mut tree = viewport_beside_stack();
    tree.set_editor_type(AreaId(2), EditorTypeId::new(OUTLINER)).unwrap();
    assert_eq!(tree.area(AreaId(2)).unwrap().editor_type, EditorTypeId::new(OUTLINER));
    assert_eq!(tree.area(AreaId(1)).unwrap().editor_type, EditorTypeId::new(VIEWPORT));
    assert_eq!(
        tree.set_editor_type(AreaId(9), EditorTypeId::new(OUTLINER)),
        Err(AreaError::UnknownArea(AreaId(9)))
    );
}

#[test]
fn serde_round_trip_preserves_layout_and_id_allocation() {
    let tree = viewport_beside_stack();
    let json = serde_json::to_string(&tree).unwrap();
    let mut restored: AreaTree = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, tree);
    let id = restored
        .split_area(AreaId(1), Orientation::Vertical, Point::new(250.0, 10.0), BOUNDS, &METRICS)
        .unwrap();
    assert_eq!(id, AreaId(4));
}

#[test]
fn deserialising_rejects_duplicate_ids_and_bad_ratios() {
    let duplicate = serde_json::to_string(&split(Orientation::Vertical, 0.5, area(1, VIEWPORT), area(1, VIEWPORT))).unwrap();
    assert!(serde_json::from_str::<AreaTree>(&duplicate).is_err());
    let bad_ratio = serde_json::to_string(&split(Orientation::Vertical, 1.5, area(1, VIEWPORT), area(2, VIEWPORT))).unwrap();
    assert!(serde_json::from_str::<AreaTree>(&bad_ratio).is_err());
}
