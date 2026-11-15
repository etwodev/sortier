use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, DragMoveEvent, ElementId, InteractiveElement, IntoElement,
    Modifiers, MouseButton, ParentElement, Render, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled,
    Window, div, prelude::FluentBuilder as _, px,
};

use crate::icon::{Icon, IconKind, IconSize};
use crate::label::{Label, TextSize, TextTone};
use crate::theme::ActiveSortierTheme;
use crate::tokens::{sizes, spacing};

type ClickHandler = Rc<dyn Fn(Modifiers, &mut Window, &mut App)>;
type Handler = Rc<dyn Fn(&mut Window, &mut App)>;
type DragOverHandler = Rc<dyn Fn(u64, DropZone, &mut Window, &mut App)>;
type DropHandler = Rc<dyn Fn(u64, &mut Window, &mut App)>;

/// Thickness of the line showing where a dragged row would land.
const DROP_LINE: f32 = 2.0;

/// Where a dragged row would land relative to the row under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropZone {
    Above,
    /// Inside a parent row, at the top of its children.
    Inside,
    Below,
}

/// A tree row being dragged: its key and its label, which follows the
/// pointer as a small chip.
#[derive(Clone)]
pub struct TreeDrag {
    pub key: u64,
    pub label: SharedString,
}

impl Render for TreeDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        div()
            .px(spacing::SM)
            .py(spacing::XXS)
            .rounded_full()
            .bg(colors.surface_raised.hsla())
            .border_1()
            .border_color(colors.border_strong.hsla())
            .shadow_md()
            .child(Label::new(self.label.clone()).size(TextSize::Small))
    }
}

/// The zone for a pointer `fraction` of the way down a row. Parent rows
/// take drops inside except near their top edge, and near their bottom edge
/// too when collapsed (an expanded parent's next row is its first child).
fn drop_zone(fraction: f32, expansion: Expansion) -> DropZone {
    match expansion {
        Expansion::Leaf if fraction < 0.5 => DropZone::Above,
        Expansion::Leaf => DropZone::Below,
        _ if fraction < 0.3 => DropZone::Above,
        Expansion::Collapsed if fraction > 0.7 => DropZone::Below,
        _ => DropZone::Inside,
    }
}

/// Whether a row can be expanded, and if so whether it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expansion {
    Leaf,
    Collapsed,
    Expanded,
}

/// How tall a tree row is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowHeight {
    #[default]
    Regular,
    /// For rows beside something larger, such as a timeline clip.
    Tall,
}

impl RowHeight {
    pub fn rems(self) -> gpui::Rems {
        match self {
            RowHeight::Regular => sizes::TREE_ROW_HEIGHT,
            RowHeight::Tall => sizes::CLIP_ROW_HEIGHT,
        }
    }
}

/// One row of a tree list: indentation, an expand chevron for parents, an
/// icon, the label (or an inline editor while renaming) and trailing controls.
#[derive(IntoElement)]
pub struct TreeRow {
    id: ElementId,
    depth: usize,
    label: SharedString,
    icon: IconKind,
    expansion: Expansion,
    height: RowHeight,
    selected: bool,
    active: bool,
    muted: bool,
    editor: Option<AnyElement>,
    trailing: Vec<AnyElement>,
    on_click: Option<ClickHandler>,
    on_double_click: Option<Handler>,
    on_toggle_expansion: Option<Handler>,
    drag_key: Option<u64>,
    drop_indicator: Option<DropZone>,
    on_drag_over: Option<DragOverHandler>,
    on_drop: Option<DropHandler>,
}

impl TreeRow {
    pub fn new(id: impl Into<ElementId>, depth: usize, label: impl Into<SharedString>, icon: IconKind) -> Self {
        Self {
            id: id.into(),
            depth,
            label: label.into(),
            icon,
            expansion: Expansion::Leaf,
            height: RowHeight::Regular,
            selected: false,
            active: false,
            muted: false,
            editor: None,
            trailing: Vec::new(),
            on_click: None,
            on_double_click: None,
            on_toggle_expansion: None,
            drag_key: None,
            drop_indicator: None,
            on_drag_over: None,
            on_drop: None,
        }
    }

    /// Lets the row be dragged; drops elsewhere receive `key`.
    pub fn draggable(mut self, key: u64) -> Self {
        self.drag_key = Some(key);
        self
    }

    /// Shows where a dragged row would land on this one.
    pub fn drop_indicator(mut self, zone: Option<DropZone>) -> Self {
        self.drop_indicator = zone;
        self
    }

    /// Called as a dragged row moves over this one, with the dragged row's
    /// key and the zone under the pointer.
    pub fn on_drag_over(mut self, handler: impl Fn(u64, DropZone, &mut Window, &mut App) + 'static) -> Self {
        self.on_drag_over = Some(Rc::new(handler));
        self
    }

    /// Called with the dragged row's key when it's dropped on this one.
    pub fn on_drop(mut self, handler: impl Fn(u64, &mut Window, &mut App) + 'static) -> Self {
        self.on_drop = Some(Rc::new(handler));
        self
    }

    pub fn expansion(mut self, expansion: Expansion) -> Self {
        self.expansion = expansion;
        self
    }

    pub fn height(mut self, height: RowHeight) -> Self {
        self.height = height;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// The row the selection is focused on, drawn with an accent marker.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Dims the label, e.g. for hidden layers.
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    /// Shown in place of the label, e.g. a text field while renaming.
    pub fn editor(mut self, editor: impl IntoElement) -> Self {
        self.editor = Some(editor.into_any_element());
        self
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }

    /// Called with the keyboard modifiers held during the click.
    pub fn on_click(mut self, handler: impl Fn(Modifiers, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    pub fn on_double_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_double_click = Some(Rc::new(handler));
        self
    }

    pub fn on_toggle_expansion(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle_expansion = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for TreeRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let indent = sizes::TREE_INDENT * self.depth as f32;
        let chevron = match self.expansion {
            Expansion::Leaf => None,
            Expansion::Collapsed => Some(IconKind::ChevronRight),
            Expansion::Expanded => Some(IconKind::ChevronDown),
        };
        let on_toggle = self.on_toggle_expansion;
        let expander = div()
            .id("expander")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(sizes::ICON_LG)
            .when_some(chevron, |this, chevron| {
                this.child(Icon::new(chevron).size(IconSize::Small))
                    .when_some(on_toggle, |this, on_toggle| {
                        this.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            cx.stop_propagation();
                            on_toggle(window, cx);
                        })
                    })
            });
        let content = match self.editor {
            Some(editor) => editor,
            None => {
                let tone = if self.muted { TextTone::Disabled } else { TextTone::Primary };
                Label::new(self.label.clone()).size(TextSize::Small).tone(tone).truncate().into_any_element()
            }
        };
        let (on_click, on_double_click) = (self.on_click, self.on_double_click);
        let expansion = self.expansion;
        let drag = self.drag_key.map(|key| TreeDrag {
            key,
            label: self.label.clone(),
        });
        let line = |zone: DropZone| {
            div()
                .absolute()
                .left(indent)
                .right_0()
                .h(px(DROP_LINE))
                .rounded_full()
                .bg(colors.accent.hsla())
                .when(zone == DropZone::Above, |this| this.top_0())
                .when(zone == DropZone::Below, |this| this.bottom_0())
        };

        let highlighted = self.selected || self.active;
        let icon_color = if highlighted { colors.accent_hover } else { colors.text_secondary };
        let row = div()
            .id(self.id)
            .relative()
            .w_full()
            .flex()
            .items_center()
            .gap(spacing::XS)
            .h(self.height.rems())
            .pl(spacing::XS + indent)
            .pr(spacing::SM)
            .rounded_full()
            .when(highlighted || self.drop_indicator == Some(DropZone::Inside), |this| {
                this.bg(colors.selection.hsla())
            })
            .when(!highlighted, |this| this.hover(|style| style.bg(colors.hover.hsla())))
            .on_click(move |event: &ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    if let Some(handler) = &on_double_click {
                        handler(window, cx);
                    }
                } else if let Some(handler) = &on_click {
                    handler(event.modifiers(), window, cx);
                }
            })
            .child(expander)
            .child(div().text_color(icon_color.hsla()).child(Icon::new(self.icon).size(IconSize::Small)))
            .child(div().flex_1().min_w_0().overflow_hidden().child(content))
            .children(self.trailing)
            .when_some(drag, |this, drag| {
                this.on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
            })
            .when_some(self.on_drag_over, |this, handler| {
                this.on_drag_move::<TreeDrag>(move |event: &DragMoveEvent<TreeDrag>, window, cx| {
                    if !event.bounds.contains(&event.event.position) {
                        return;
                    }
                    let offset = event.event.position.y - event.bounds.top();
                    let fraction = f32::from(offset) / f32::from(event.bounds.size.height).max(1.0);
                    let key = event.drag(cx).key;
                    handler(key, drop_zone(fraction, expansion), window, cx);
                })
            })
            .when_some(self.on_drop, |this, handler| {
                this.on_drop::<TreeDrag>(move |drag, window, cx| {
                    cx.stop_propagation();
                    handler(drag.key, window, cx);
                })
            })
            .when_some(
                self.drop_indicator.filter(|zone| *zone != DropZone::Inside),
                |this, zone| this.child(line(zone)),
            );
        // The inset keeps each row's rounded highlight clear of the list's
        // edges and of its neighbours.
        div().w_full().px(sizes::TREE_ROW_INSET).py(px(1.)).child(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_land_above_below_or_inside_by_pointer_position() {
        assert_eq!(drop_zone(0.2, Expansion::Leaf), DropZone::Above);
        assert_eq!(drop_zone(0.6, Expansion::Leaf), DropZone::Below);
        assert_eq!(drop_zone(0.1, Expansion::Expanded), DropZone::Above);
        assert_eq!(drop_zone(0.5, Expansion::Expanded), DropZone::Inside);
        assert_eq!(drop_zone(0.9, Expansion::Expanded), DropZone::Inside);
        assert_eq!(drop_zone(0.9, Expansion::Collapsed), DropZone::Below);
    }
}
