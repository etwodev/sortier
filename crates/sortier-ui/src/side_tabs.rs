use std::rc::Rc;

use gpui::{AnyElement, App, ElementId, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div};

use crate::button::{ButtonVariant, IconButton};
use crate::icon::IconKind;
use crate::theme::ActiveSortierTheme;
use crate::tokens::spacing;

type SelectHandler = Rc<dyn Fn(&mut Window, &mut App)>;

struct SideTab {
    id: ElementId,
    icon: IconKind,
    label: SharedString,
    selected: bool,
    on_select: SelectHandler,
}

/// A vertical strip of icon tabs down the left of an editor, as in Blender's
/// Properties editor, beside the selected tab's content.
#[derive(IntoElement)]
pub struct SideTabs {
    tabs: Vec<SideTab>,
    content: Option<AnyElement>,
}

impl SideTabs {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            content: None,
        }
    }

    /// Adds a tab; `label` names it for screen readers and its tooltip.
    pub fn tab(
        mut self,
        id: impl Into<ElementId>,
        icon: IconKind,
        label: impl Into<SharedString>,
        selected: bool,
        on_select: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.tabs.push(SideTab {
            id: id.into(),
            icon,
            label: label.into(),
            selected,
            on_select: Rc::new(on_select),
        });
        self
    }

    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }
}

impl Default for SideTabs {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for SideTabs {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let strip = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(spacing::XXS)
            .p(spacing::XXS)
            .border_r_1()
            .border_color(colors.border.hsla())
            .bg(colors.chrome.hsla())
            .children(self.tabs.into_iter().map(|tab| {
                let on_select = tab.on_select;
                IconButton::new(tab.id, tab.icon, tab.label)
                    .variant(ButtonVariant::Ghost)
                    .selected(tab.selected)
                    .on_click(move |window, cx| on_select(window, cx))
            }));
        div()
            .size_full()
            .flex()
            .child(strip)
            .child(div().flex_1().min_w_0().h_full().children(self.content))
    }
}
