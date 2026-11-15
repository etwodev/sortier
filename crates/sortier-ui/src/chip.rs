use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, MouseButton, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div,
};

use crate::button::{ButtonVariant, IconButton};
use crate::icon::{Icon, IconKind, IconSize};
use crate::label::{Label, TextSize};
use crate::theme::ActiveSortierTheme;
use crate::tokens::{radius, sizes, spacing};

type RemoveHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// A small labelled token standing for one item in a list, with a button
/// to remove it, and optionally a toggle before its label for a setting of
/// the item.
#[derive(IntoElement)]
pub struct Chip {
    id: ElementId,
    label: SharedString,
    on_remove: Option<RemoveHandler>,
    toggle: Option<IconButton>,
}

impl Chip {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            on_remove: None,
            toggle: None,
        }
    }

    /// A toggle before the label: `icon`, shown pressed when `on`, named by
    /// `label` (its tooltip). `handler` runs when it's clicked.
    pub fn toggle(
        mut self,
        icon: IconKind,
        label: impl Into<SharedString>,
        on: bool,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        let id = ElementId::from((self.id.clone(), "toggle"));
        self.toggle = Some(IconButton::new(id, icon, label).variant(ButtonVariant::Ghost).selected(on).on_click(handler));
        self
    }

    pub fn on_remove(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_remove = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let on_remove = self.on_remove;
        div()
            .flex()
            .items_center()
            .gap(spacing::XXS)
            .max_w_full()
            .pl(spacing::XS)
            .pr(spacing::XXS)
            .rounded(radius::SM)
            .bg(colors.selection.hsla())
            .when_some(self.toggle, |chip, toggle| chip.child(div().flex_none().child(toggle)))
            .child(div().min_w_0().overflow_hidden().child(Label::new(self.label).size(TextSize::Small)))
            .when_some(on_remove, |chip, on_remove| {
                chip.child(
                    div()
                        .id(self.id)
                        .flex_none()
                        .rounded(radius::SM)
                        .hover(|style| style.bg(colors.hover.hsla()))
                        .child(Icon::new(IconKind::Close).size(IconSize::Small))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_remove(window, cx);
                        }),
                )
            })
    }
}

/// Chips in rows that wrap, after a short label. However many there are,
/// it stays a few rows tall and scrolls.
#[derive(IntoElement)]
pub struct ChipList {
    id: ElementId,
    label: SharedString,
    chips: Vec<Chip>,
    hint: Option<SharedString>,
}

impl ChipList {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            chips: Vec::new(),
            hint: None,
        }
    }

    pub fn chip(mut self, chip: Chip) -> Self {
        self.chips.push(chip);
        self
    }

    /// A sentence after the chips saying how to add more.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl RenderOnce for ChipList {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let id = self.id.clone();
        div()
            .id(id)
            .flex_none()
            .max_h(sizes::CHIP_LIST_MAX_HEIGHT)
            .overflow_y_scroll()
            .border_t_1()
            .border_color(colors.border.hsla())
            .bg(colors.surface.hsla())
            .child(self.contents())
    }
}

impl ChipList {
    fn contents(self) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(spacing::XS)
            .px(spacing::SM)
            .py(spacing::XS)
            .child(Label::new(self.label).size(TextSize::Small).tone(crate::label::TextTone::Secondary))
            .children(self.chips)
            .when_some(self.hint, |list, hint| {
                list.child(Label::new(hint).size(TextSize::Small).tone(crate::label::TextTone::Secondary))
            })
    }
}
