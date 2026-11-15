use crate::*;

fn layout() -> AreaTree {
    AreaTree::new(EditorTypeId::new("test.viewport"))
}

fn names(workspaces: &Workspaces) -> Vec<&str> {
    workspaces.iter().map(|workspace| workspace.name.as_str()).collect()
}

fn three() -> (Workspaces, WorkspaceId, WorkspaceId, WorkspaceId) {
    let mut workspaces = Workspaces::new("Layout", layout());
    let first = workspaces.active();
    let second = workspaces.add("Canvas", layout());
    let third = workspaces.add("Review", layout());
    (workspaces, first, second, third)
}

#[test]
fn add_inserts_after_the_active_workspace_and_activates_it() {
    let (mut workspaces, first, _, _) = three();
    workspaces.activate(first).unwrap();
    let added = workspaces.add("Extra", layout());
    assert_eq!(names(&workspaces), vec!["Layout", "Extra", "Canvas", "Review"]);
    assert_eq!(workspaces.active(), added);
}

#[test]
fn names_are_made_unique_blender_style() {
    let (mut workspaces, _, _, _) = three();
    workspaces.add("Layout", layout());
    workspaces.add("Layout", layout());
    workspaces.add("Layout.001", layout());
    let mut sorted = names(&workspaces);
    sorted.sort();
    assert_eq!(sorted, vec!["Canvas", "Layout", "Layout.001", "Layout.002", "Layout.003", "Review"]);
}

#[test]
fn rename_trims_rejects_empty_and_keeps_own_name() {
    let (mut workspaces, first, second, _) = three();
    assert_eq!(workspaces.rename(second, "  Paint  ").unwrap(), "Paint");
    assert_eq!(workspaces.rename(second, "Layout").unwrap(), "Layout.001");
    assert_eq!(workspaces.rename(first, "Layout").unwrap(), "Layout");
    assert_eq!(workspaces.rename(first, "   "), Err(WorkspaceError::EmptyName));
}

#[test]
fn move_to_reorders_and_clamps() {
    let (mut workspaces, first, _, third) = three();
    workspaces.move_to(third, 0).unwrap();
    assert_eq!(names(&workspaces), vec!["Review", "Layout", "Canvas"]);
    workspaces.move_to(first, 99).unwrap();
    assert_eq!(names(&workspaces), vec!["Review", "Canvas", "Layout"]);
}

#[test]
fn removing_the_active_workspace_activates_its_neighbour() {
    let (mut workspaces, _, second, third) = three();
    workspaces.activate(second).unwrap();
    workspaces.remove(second).unwrap();
    assert_eq!(workspaces.active(), third);
    workspaces.remove(third).unwrap();
    assert_eq!(names(&workspaces), vec!["Layout"]);
    assert_eq!(workspaces.active(), workspaces.iter().next().unwrap().id);
}

#[test]
fn removing_an_inactive_workspace_keeps_the_active_one() {
    let (mut workspaces, first, _, third) = three();
    workspaces.remove(first).unwrap();
    assert_eq!(workspaces.active(), third);
}

#[test]
fn the_last_workspace_cannot_be_removed() {
    let mut workspaces = Workspaces::new("Layout", layout());
    let only = workspaces.active();
    assert_eq!(workspaces.remove(only), Err(WorkspaceError::LastWorkspace));
}

#[test]
fn unknown_ids_are_errors() {
    let (mut workspaces, _, _, _) = three();
    let missing = WorkspaceId(99);
    assert_eq!(workspaces.activate(missing), Err(WorkspaceError::UnknownWorkspace(missing)));
    assert!(workspaces.rename(missing, "x").is_err());
    assert!(workspaces.move_to(missing, 0).is_err());
    assert!(workspaces.remove(missing).is_err());
}

#[test]
fn layouts_are_independent_per_workspace() {
    let (mut workspaces, first, second, _) = three();
    let bounds = Rect::new(0.0, 0.0, 800.0, 600.0);
    let metrics = LayoutMetrics {
        gap: 2.0,
        min_area: Size::new(100.0, 60.0),
        seam_hit_slop: 3.0,
    };
    workspaces
        .layout_mut(first)
        .unwrap()
        .split_area(AreaId(1), Orientation::Vertical, Point::new(400.0, 10.0), bounds, &metrics)
        .unwrap();
    assert_eq!(workspaces.get(first).unwrap().layout.areas().len(), 2);
    assert_eq!(workspaces.get(second).unwrap().layout.areas().len(), 1);
}

#[test]
fn serde_round_trip_keeps_order_active_and_id_allocation() {
    let (mut workspaces, _, second, _) = three();
    workspaces.activate(second).unwrap();
    let json = serde_json::to_string(&workspaces).unwrap();
    let mut restored: Workspaces = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, workspaces);
    assert_eq!(restored.add("New", layout()), WorkspaceId(4));
}

#[test]
fn deserialising_rejects_invalid_collections() {
    let empty = r#"{"workspaces":[],"active":1}"#;
    assert!(serde_json::from_str::<Workspaces>(empty).is_err());
    let layout_json = serde_json::to_string(&layout()).unwrap();
    let missing_active = format!(r#"{{"workspaces":[{{"id":1,"name":"A","layout":{layout_json}}}],"active":2}}"#);
    assert!(serde_json::from_str::<Workspaces>(&missing_active).is_err());
    let duplicate = format!(
        r#"{{"workspaces":[{{"id":1,"name":"A","layout":{layout_json}}},{{"id":1,"name":"B","layout":{layout_json}}}],"active":1}}"#
    );
    assert!(serde_json::from_str::<Workspaces>(&duplicate).is_err());
}
