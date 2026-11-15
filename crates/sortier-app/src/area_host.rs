use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, Bounds, Context, DispatchPhase, Entity, ElementId, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, Styled, Subscription, WeakEntity,
    Window, bounds,
    canvas, div, point, prelude::FluentBuilder as _, px, relative, size,
};
use sortier_areas::{
    Area, AreaId, AreaTree, EditorTypeId, JoinDirection, LayoutMetrics, LayoutNode, Orientation, Point, Rect, Size,
    SplitPath, WorkspaceId,
};
use crate::editors::{EditorContext, EditorRegistry};
use sortier_ui::area::{AreaFrame, AreaHeader, AreaLayoutSurface, LineOrientation, SeamHandle, SplitGuide};
use sortier_ui::empty_state::EmptyState;
use sortier_ui::icon::IconKind;
use sortier_ui::menu::{DropdownButton, MenuEntry};
use sortier_ui::tokens::{sizes, to_px};

use crate::layouts::LayoutModel;

/// Area layout sizes in pixels at the current UI scale.
fn metrics(cx: &App) -> LayoutMetrics {
    let pixels = |size| f32::from(to_px(size, cx));
    LayoutMetrics {
        gap: pixels(sizes::AREA_SEAM),
        min_area: Size::new(pixels(sizes::AREA_MIN_WIDTH), pixels(sizes::AREA_MIN_HEIGHT)),
        seam_hit_slop: pixels(sizes::SEAM_HIT_SLOP),
    }
}

fn to_rect(bounds: Bounds<Pixels>) -> Rect {
    Rect::new(
        f32::from(bounds.origin.x),
        f32::from(bounds.origin.y),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
    )
}

fn to_point(position: gpui::Point<Pixels>) -> Point {
    Point::new(f32::from(position.x), f32::from(position.y))
}

/// `rect` relative to `origin`, as GPUI bounds for absolutely positioned children.
fn to_local_bounds(rect: Rect, origin: Rect) -> Bounds<Pixels> {
    bounds(
        point(px(rect.origin.x - origin.origin.x), px(rect.origin.y - origin.origin.y)),
        size(px(rect.size.width), px(rect.size.height)),
    )
}

/// A pointer-driven operation in progress.
enum Interaction {
    /// Dragging the seam of the split at `path`.
    Resizing { path: SplitPath },
    /// Choosing where to split: a guide follows the pointer across the hovered
    /// area until a click commits or Escape / right click cancels.
    ChoosingSplit {
        orientation: Orientation,
        pointer: Option<Point>,
    },
}

/// Renders one workspace's [`AreaTree`], which lives in the [`LayoutModel`],
/// and turns pointer input into tree operations. Editor views are cached per
/// area and editor type, so switching an area to another editor and back
/// restores the earlier view.
pub struct AreaHost {
    layouts: Entity<LayoutModel>,
    workspace: WorkspaceId,
    editor_context: EditorContext,
    views: HashMap<(AreaId, EditorTypeId), AnyView>,
    /// Where the areas were laid out last frame, in window coordinates. Pointer
    /// input arrives after paint, so this is current when it is read.
    layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    interaction: Option<Interaction>,
    hovered_seam: Option<SplitPath>,
    _project_changed: Subscription,
}

impl AreaHost {
    pub fn new(
        layouts: Entity<LayoutModel>,
        workspace: WorkspaceId,
        editor_context: EditorContext,
        cx: &mut Context<Self>,
    ) -> Self {
        let layouts_changed = cx.observe(&layouts, |_, _, cx| cx.notify());
        Self {
            layouts,
            workspace,
            editor_context,
            views: HashMap::new(),
            layout_bounds: Rc::new(Cell::new(None)),
            interaction: None,
            hovered_seam: None,
            _project_changed: layouts_changed,
        }
    }

    fn tree<'a>(&self, cx: &'a App) -> Option<&'a AreaTree> {
        self.layouts.read(cx).layout(self.workspace)
    }

    /// Applies `edit` to this workspace's layout and notifies everything
    /// observing the layouts.
    fn edit_tree<R>(&self, cx: &mut Context<Self>, edit: impl FnOnce(&mut AreaTree) -> R) -> Option<R> {
        let workspace = self.workspace;
        // A seam drag edits on every pointer move; it's saved once when it ends.
        let save = !matches!(self.interaction, Some(Interaction::Resizing { .. }));
        self.layouts.update(cx, |layouts, cx| {
            let result = layouts.layout_mut(workspace).map(edit);
            if save {
                layouts.save();
            }
            cx.notify();
            result
        })
    }

    fn set_hovered_seam(&mut self, path: &SplitPath, hovered: bool, cx: &mut Context<Self>) {
        let next = match (hovered, &self.hovered_seam) {
            (true, _) => Some(path.clone()),
            (false, Some(current)) if current == path => None,
            (false, current) => current.clone(),
        };
        if next != self.hovered_seam {
            self.hovered_seam = next;
            cx.notify();
        }
    }

    /// Areas the hovered or dragged seam would resize, whose outlines are brightened.
    fn highlighted_areas(&self, cx: &App) -> Vec<AreaId> {
        let path = match &self.interaction {
            Some(Interaction::Resizing { path }) => Some(path),
            Some(Interaction::ChoosingSplit { .. }) => None,
            None => self.hovered_seam.as_ref(),
        };
        match (path, self.bounds_rect(), self.tree(cx)) {
            (Some(path), Some(bounds), Some(tree)) => tree.areas_touching_seam(path, bounds, &metrics(cx)),
            _ => Vec::new(),
        }
    }

    fn bounds_rect(&self) -> Option<Rect> {
        self.layout_bounds.get().map(to_rect)
    }

    fn view_for(&mut self, area: &Area, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyView> {
        let key = (area.id, area.editor_type.clone());
        if let Some(view) = self.views.get(&key) {
            return Some(view.clone());
        }
        let factory = cx.global::<EditorRegistry>().get(&area.editor_type)?;
        let view = factory.create(&self.editor_context, window, cx);
        self.views.insert(key, view.clone());
        Some(view)
    }

    fn set_editor_type(&mut self, id: AreaId, editor_type: EditorTypeId, cx: &mut Context<Self>) {
        self.edit_tree(cx, |tree| tree.set_editor_type(id, editor_type));
    }

    fn start_resize(&mut self, path: SplitPath, cx: &mut Context<Self>) {
        self.interaction = Some(Interaction::Resizing { path });
        cx.notify();
    }

    fn start_choosing_split(&mut self, orientation: Orientation, window: &mut Window, cx: &mut Context<Self>) {
        self.interaction = Some(Interaction::ChoosingSplit {
            orientation,
            pointer: Some(to_point(window.mouse_position())),
        });
        cx.notify();
    }

    fn end_interaction(&mut self, cx: &mut Context<Self>) {
        if let Some(interaction) = self.interaction.take() {
            if matches!(interaction, Interaction::Resizing { .. }) {
                self.layouts.read(cx).save();
            }
            cx.notify();
        }
    }

    fn pointer_moved(&mut self, position: Point, left_button_held: bool, cx: &mut Context<Self>) {
        let Some(bounds) = self.bounds_rect() else {
            return;
        };
        match &mut self.interaction {
            // The release can arrive before the drag listeners are installed (a
            // quick click), so a move without the button held ends the drag.
            Some(Interaction::Resizing { .. }) if !left_button_held => {
                self.interaction = None;
            }
            Some(Interaction::Resizing { path }) => {
                let path = path.clone();
                // A refused resize leaves the layout as it was, which is the
                // right outcome when every area is already at its minimum size.
                let metrics = metrics(cx);
                self.edit_tree(cx, |tree| tree.resize_split(&path, position, bounds, &metrics));
            }
            Some(Interaction::ChoosingSplit { pointer, .. }) => *pointer = Some(position),
            None => return,
        }
        cx.notify();
    }

    fn commit_split(&mut self, position: Point, cx: &mut Context<Self>) {
        let Some(Interaction::ChoosingSplit { orientation, .. }) = self.interaction else {
            return;
        };
        if let Some(bounds) = self.bounds_rect()
            && let Some(id) = self.hovered_area(position, bounds, cx)
        {
            // An area too small to split is left alone; the guide already showed
            // nothing for it.
            let metrics = metrics(cx);
            self.edit_tree(cx, |tree| tree.split_area(id, orientation, position, bounds, &metrics));
            self.hovered_seam = None;
        }
        self.end_interaction(cx);
    }

    fn hovered_area(&self, position: Point, bounds: Rect, cx: &App) -> Option<AreaId> {
        self.tree(cx)?
            .layout(bounds, &metrics(cx))
            .areas
            .into_iter()
            .find(|area| area.rect.contains(position))
            .map(|area| area.id)
    }

    fn join(&mut self, keep: AreaId, remove: AreaId, cx: &mut Context<Self>) {
        let Some(Ok(outcome)) = self.edit_tree(cx, |tree| tree.join(keep, remove)) else {
            return;
        };
        self.views.retain(|(id, _), _| !outcome.removed.contains(id));
        self.hovered_seam = None;
        cx.notify();
    }

    fn seam_menu(&self, path: &SplitPath, at: Point, host: WeakEntity<Self>, cx: &App) -> Vec<MenuEntry> {
        let split_entry = |label: &str, orientation: Orientation| {
            let host = host.clone();
            MenuEntry::command(label.to_string(), move |window, cx| {
                host.update(cx, |this, cx| this.start_choosing_split(orientation, window, cx)).ok();
            })
        };
        let mut entries = vec![
            MenuEntry::heading("Area Options"),
            split_entry("Vertical Split", Orientation::Vertical),
            split_entry("Horizontal Split", Orientation::Horizontal),
        ];

        let joins = match (self.bounds_rect(), self.tree(cx)) {
            (Some(bounds), Some(tree)) => tree.seam_join_options(path, at, bounds, &metrics(cx)),
            _ => Vec::new(),
        };
        if !joins.is_empty() {
            entries.push(MenuEntry::separator());
        }
        for option in joins {
            let label = match option.direction {
                JoinDirection::Up => "Join Up",
                JoinDirection::Down => "Join Down",
                JoinDirection::Left => "Join Left",
                JoinDirection::Right => "Join Right",
            };
            let host = host.clone();
            entries.push(MenuEntry::command(label, move |_, cx| {
                host.update(cx, |this, cx| this.join(option.keep, option.remove, cx)).ok();
            }));
        }
        entries
    }

    fn render_node(
        &mut self,
        node: &LayoutNode,
        highlighted: &[AreaId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            LayoutNode::Area(area) => self.render_area(area, highlighted.contains(&area.id), window, cx),
            LayoutNode::Split(split) => {
                let first = self.render_node(&split.first, highlighted, window, cx);
                let second = self.render_node(&split.second, highlighted, window, cx);
                let vertical = split.orientation == Orientation::Vertical;
                // Mirrors sortier_areas::AreaTree::layout: the first child gets
                // `ratio` of the extent, then the seam, then the remainder.
                div()
                    .size_full()
                    .flex()
                    .when(vertical, |this| this.flex_row())
                    .when(!vertical, |this| this.flex_col())
                    .child(
                        div()
                            .flex_none()
                            .overflow_hidden()
                            .when(vertical, |this| this.h_full().w(relative(split.ratio)))
                            .when(!vertical, |this| this.w_full().h(relative(split.ratio)))
                            .child(first),
                    )
                    .child(
                        div()
                            .flex_none()
                            .when(vertical, |this| this.h_full().w(sizes::AREA_SEAM))
                            .when(!vertical, |this| this.w_full().h(sizes::AREA_SEAM)),
                    )
                    .child(div().flex_1().min_w_0().min_h_0().overflow_hidden().child(second))
                    .into_any_element()
            }
        }
    }

    fn render_area(&mut self, area: &Area, highlighted: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let host = cx.entity().downgrade();
        let registry = cx.global::<EditorRegistry>().clone();
        let entries = registry
            .iter()
            .map(|factory| {
                let editor_type = factory.id();
                let host = host.clone();
                let area_id = area.id;
                MenuEntry::command(factory.name(), {
                    let editor_type = editor_type.clone();
                    move |_, cx| {
                        host.update(cx, |this, cx| this.set_editor_type(area_id, editor_type.clone(), cx)).ok();
                    }
                })
                .icon(factory.icon())
                .checked(editor_type == area.editor_type)
            })
            .collect();

        let factory = registry.get(&area.editor_type);
        let (label, icon) = match &factory {
            Some(factory) => (factory.name(), factory.icon()),
            None => (area.editor_type.as_str().to_string().into(), IconKind::Viewer),
        };
        let selector = DropdownButton::new(ElementId::NamedInteger("editor-type".into(), u64::from(area.id.0)), entries)
            .label(label)
            .icon(icon);

        let body: AnyElement = match self.view_for(area, window, cx) {
            Some(view) => view.into_any_element(),
            None => EmptyState::new(
                IconKind::Viewer,
                "Editor unavailable",
                format!("No editor named \"{}\" is installed. Choose another from the menu above.", area.editor_type.as_str()),
            )
            .into_any_element(),
        };
        AreaFrame::new(AreaHeader::new().item(selector))
            .highlighted(highlighted)
            .body(body)
            .into_any_element()
    }

    fn render_seams(&self, bounds: Rect, host: &WeakEntity<Self>, cx: &App) -> Vec<SeamHandle> {
        let metrics = metrics(cx);
        let Some(tree) = self.tree(cx) else {
            return Vec::new();
        };
        let layout = tree.layout(bounds, &metrics);
        layout
            .seams
            .into_iter()
            .enumerate()
            .map(|(index, seam)| {
                let orientation = match seam.orientation {
                    Orientation::Vertical => LineOrientation::Vertical,
                    Orientation::Horizontal => LineOrientation::Horizontal,
                };
                let hit_rect = Rect::new(
                    seam.rect.origin.x - metrics.seam_hit_slop,
                    seam.rect.origin.y - metrics.seam_hit_slop,
                    seam.rect.size.width + 2.0 * metrics.seam_hit_slop,
                    seam.rect.size.height + 2.0 * metrics.seam_hit_slop,
                );
                let press_host = host.clone();
                let hover_host = host.clone();
                let menu_host = host.clone();
                let press_path = seam.path.clone();
                let hover_path = seam.path.clone();
                let menu_path = seam.path;
                SeamHandle::new(
                    ElementId::NamedInteger("seam".into(), index as u64),
                    orientation,
                    to_local_bounds(hit_rect, bounds),
                )
                .on_hover(move |hovered, _, cx| {
                    hover_host
                        .update(cx, |this, cx| this.set_hovered_seam(&hover_path, hovered, cx))
                        .ok();
                })
                .on_press(move |_, _, cx| {
                    press_host.update(cx, |this, cx| this.start_resize(press_path.clone(), cx)).ok();
                })
                .context_menu(move |window, cx| {
                    let at = to_point(window.mouse_position());
                    let Some(host) = menu_host.upgrade() else {
                        return Vec::new();
                    };
                    host.read(cx).seam_menu(&menu_path, at, menu_host.clone(), cx)
                })
            })
            .collect()
    }

    fn split_guide(&self, bounds: Rect, cx: &App) -> Option<SplitGuide> {
        let Some(Interaction::ChoosingSplit {
            orientation,
            pointer: Some(pointer),
        }) = self.interaction
        else {
            return None;
        };
        let area = self.hovered_area(pointer, bounds, cx)?;
        let seam = self.tree(cx)?.preview_split(area, orientation, pointer, bounds, &metrics(cx)).ok()?;
        let thickness = f32::from(to_px(sizes::SPLIT_GUIDE, cx));
        let line = match orientation {
            Orientation::Vertical => Rect::new(
                seam.origin.x + (seam.size.width - thickness) / 2.0,
                seam.origin.y,
                thickness,
                seam.size.height,
            ),
            Orientation::Horizontal => Rect::new(
                seam.origin.x,
                seam.origin.y + (seam.size.height - thickness) / 2.0,
                seam.size.width,
                thickness,
            ),
        };
        Some(SplitGuide::new(to_local_bounds(line, bounds)))
    }
}

/// Records the layout bounds every frame and, while an interaction is active,
/// listens to pointer and Escape events window-wide, so drags keep working
/// outside the seam handle and cancelling doesn't depend on keyboard focus.
fn interaction_tracker(host: WeakEntity<AreaHost>, layout_bounds: Rc<Cell<Option<Bounds<Pixels>>>>, active: bool) -> impl IntoElement {
    canvas(
        move |bounds, window, _| {
            // Seam handles are placed from these bounds, so a change needs one
            // more frame to bring them in line.
            if layout_bounds.replace(Some(bounds)) != Some(bounds) {
                window.refresh();
            }
        },
        move |_, (), window, _| {
            if !active {
                return;
            }
            let move_host = host.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture {
                    let held = event.pressed_button == Some(MouseButton::Left);
                    move_host
                        .update(cx, |this, cx| this.pointer_moved(to_point(event.position), held, cx))
                        .ok();
                }
            });
            let key_host = host.clone();
            window.on_key_event(move |event: &KeyDownEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture && event.keystroke.key == "escape" {
                    key_host.update(cx, |this, cx| this.end_interaction(cx)).ok();
                    cx.stop_propagation();
                }
            });
            let up_host = host.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                    up_host
                        .update(cx, |this, cx| {
                            if matches!(this.interaction, Some(Interaction::Resizing { .. })) {
                                this.end_interaction(cx);
                            }
                        })
                        .ok();
                }
            });
            let down_host = host.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                let consumed = down_host
                    .update(cx, |this, cx| {
                        if !matches!(this.interaction, Some(Interaction::ChoosingSplit { .. })) {
                            return false;
                        }
                        match event.button {
                            MouseButton::Left => this.commit_split(to_point(event.position), cx),
                            _ => this.end_interaction(cx),
                        }
                        true
                    })
                    .unwrap_or(false);
                // While choosing a split, the click belongs to the split, not to
                // whatever is under the pointer.
                if consumed {
                    cx.stop_propagation();
                }
            });
        },
    )
    .absolute()
    .size_full()
}

impl Render for AreaHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let host = cx.entity().downgrade();
        let Some(root) = self.tree(cx).map(|tree| tree.root().clone()) else {
            return AreaLayoutSurface::new();
        };
        let highlighted = self.highlighted_areas(cx);
        let tree = self.render_node(&root, &highlighted, window, cx);
        let mut surface = AreaLayoutSurface::new()
            .child(interaction_tracker(host.clone(), self.layout_bounds.clone(), self.interaction.is_some()))
            .child(tree);
        if let Some(bounds) = self.bounds_rect() {
            for seam in self.render_seams(bounds, &host, cx) {
                surface = surface.child(seam);
            }
            if let Some(guide) = self.split_guide(bounds, cx) {
                surface = surface.child(guide);
            }
        }

        surface
    }
}
