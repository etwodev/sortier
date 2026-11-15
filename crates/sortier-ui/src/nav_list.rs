use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::icon::{Icon, IconKind, IconSize};
use crate::label::{Label, TextTone};
use crate::theme::ActiveSortierTheme;
use crate::tokens::{radius, sizes, spacing};

type SelectHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// One clickable row of a [`NavList`] or a splash list: an icon, a label
/// and an optional detail on the right. Reachable with Tab and chosen with
/// Enter or Space.
#[derive(IntoElement)]
pub struct ListItem {
    id: ElementId,
    icon: IconKind,
    label: SharedString,
    detail: Option<SharedString>,
    selected: bool,
    disabled: bool,
    on_select: Option<SelectHandler>,
}

impl ListItem {
    pub fn new(id: impl Into<ElementId>, icon: IconKind, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon,
            label: label.into(),
            detail: None,
            selected: false,
            disabled: false,
            on_select: None,
        }
    }

    /// Secondary text after the label, such as a recent project's folder.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ListItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let on_select = self.on_select.filter(|_| !self.disabled);
        let tone = if self.disabled { TextTone::Disabled } else { TextTone::Primary };
        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(spacing::SM)
            .h(sizes::LIST_ROW_HEIGHT)
            .px(spacing::SM)
            .rounded(radius::MD)
            .min_w_0()
            .when(self.selected, |row| row.bg(colors.selection.hsla()))
            .when(!self.disabled, |row| {
                row.tab_index(0)
                    .hover(|style| style.bg(colors.hover.hsla()))
                    .focus_visible(|style| style.border_1().border_color(colors.focus_ring.hsla()))
            })
            .when_some(on_select, |row, handler| row.on_click(move |_, window, cx| handler(window, cx)))
            .child(Icon::new(self.icon).size(IconSize::Medium))
            .child(div().flex_none().child(Label::new(self.label).tone(tone)))
            .when_some(self.detail, |row, detail| {
                row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(Label::new(detail).tone(TextTone::Secondary)),
                )
            })
    }
}

/// A column of [`ListItem`]s choosing what the content beside it shows,
/// like the section list in Blender's Preferences.
#[derive(IntoElement)]
pub struct NavList {
    items: Vec<ListItem>,
    content: Option<AnyElement>,
}

impl NavList {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            content: None,
        }
    }

    pub fn item(mut self, item: ListItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }
}

impl Default for NavList {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for NavList {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w(sizes::NAV_LIST_WIDTH)
                    .gap(spacing::XXS)
                    .p(spacing::SM)
                    .border_r_1()
                    .border_color(colors.border.hsla())
                    .bg(colors.chrome.hsla())
                    .children(self.items),
            )
            .child(div().flex_1().min_w_0().h_full().children(self.content))
    }
}
