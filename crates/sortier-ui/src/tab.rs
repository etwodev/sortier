use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::label::{Label, TextSize, TextTone};
use crate::menu::{MenuEntry, with_context_menu};
use crate::theme::ActiveSortierTheme;
use crate::tokens::{sizes, spacing};

/// The value carried while a [`Tab`] is dragged. `key` is the caller's
/// identifier for the tab, as given to [`Tab::new`].
#[derive(Clone, Debug)]
pub struct TabDrag {
    pub key: u64,
    label: SharedString,
}

/// What follows the pointer while a tab is dragged.
pub struct TabDragPreview {
    label: SharedString,
}

impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        div()
            .flex()
            .items_center()
            .h(sizes::TAB_HEIGHT)
            .px(spacing::MD)
            .rounded_full()
            .bg(colors.surface_raised.hsla())
            .border_1()
            .border_color(colors.border_strong.hsla())
            .child(Label::new(self.label.clone()).size(TextSize::Small))
    }
}

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;
type DropHandler = Rc<dyn Fn(&TabDrag, &mut Window, &mut App)>;
type MenuBuilder = Rc<dyn Fn(&mut Window, &mut App) -> Vec<MenuEntry>>;

/// One tab in a tab strip. Click selects it, double-click asks to rename it,
/// dragging it onto another tab reports a drop there, and right-click opens its
/// menu. While renaming, `editor` is shown in place of the label.
#[derive(IntoElement)]
pub struct Tab {
    id: ElementId,
    key: u64,
    label: SharedString,
    active: bool,
    editor: Option<AnyElement>,
    on_select: Option<Handler>,
    on_rename: Option<Handler>,
    on_drop: Option<DropHandler>,
    menu: Option<MenuBuilder>,
}

impl Tab {
    /// `key` identifies the tab to the caller and comes back in [`TabDrag`].
    pub fn new(id: impl Into<ElementId>, key: u64, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            key,
            label: label.into(),
            active: false,
            editor: None,
            on_select: None,
            on_rename: None,
            on_drop: None,
            menu: None,
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Shows `editor` (typically a [`TextField`](crate::text_field::TextField))
    /// instead of the label.
    pub fn editor(mut self, editor: impl IntoElement) -> Self {
        self.editor = Some(editor.into_any_element());
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    pub fn on_rename(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_rename = Some(Rc::new(handler));
        self
    }

    pub fn on_drop(mut self, handler: impl Fn(&TabDrag, &mut Window, &mut App) + 'static) -> Self {
        self.on_drop = Some(Rc::new(handler));
        self
    }

    pub fn context_menu(mut self, build: impl Fn(&mut Window, &mut App) -> Vec<MenuEntry> + 'static) -> Self {
        self.menu = Some(Rc::new(build));
        self
    }
}

impl RenderOnce for Tab {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let editing = self.editor.is_some();
        let drag = TabDrag {
            key: self.key,
            label: self.label.clone(),
        };
        let content = match self.editor {
            Some(editor) => editor,
            None => {
                let tone = if self.active {
                    TextTone::Primary
                } else {
                    TextTone::Secondary
                };
                Label::new(self.label).size(TextSize::Small).tone(tone).into_any_element()
            }
        };
        let (on_select, on_rename, on_drop) = (self.on_select, self.on_rename, self.on_drop);
        let drop_highlight = colors.accent.hsla_with_alpha(0.35);

        let tab = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .h(sizes::TAB_HEIGHT)
            .px(spacing::MD)
            .rounded_full()
            .when(self.active, |this| this.bg(colors.selection.hsla()))
            .when(!self.active, |this| this.hover(|style| style.bg(colors.hover.hsla())))
            .child(content)
            .when(!editing, |this| {
                this.on_click(move |event, window, cx| {
                    if event.click_count() >= 2 {
                        if let Some(on_rename) = &on_rename {
                            on_rename(window, cx);
                        }
                    } else if let Some(on_select) = &on_select {
                        on_select(window, cx);
                    }
                })
                .on_drag(drag, |drag, _, _, cx| {
                    cx.new(|_| TabDragPreview {
                        label: drag.label.clone(),
                    })
                })
                .drag_over::<TabDrag>(move |style, _, _, _| style.bg(drop_highlight))
                .when_some(on_drop, |this, on_drop| {
                    this.on_drop(move |drag: &TabDrag, window, cx| on_drop(drag, window, cx))
                })
            });
        match self.menu {
            Some(menu) if !editing => with_context_menu(tab, move |window, cx| menu(window, cx)),
            _ => tab.into_any_element(),
        }
    }
}
