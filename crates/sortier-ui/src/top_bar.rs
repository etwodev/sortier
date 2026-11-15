use gpui::{AnyElement, App, IntoElement, ParentElement, RenderOnce, Styled, Window, div};

use crate::theme::ActiveSortierTheme;
use crate::tokens::{sizes, spacing};

/// The bar below the [`TitleBar`](crate::title_bar::TitleBar) that holds the
/// application menus and workspace tabs, laid out from the left.
#[derive(IntoElement)]
pub struct TopBar {
    children: Vec<AnyElement>,
}

impl TopBar {
    pub fn new() -> Self {
        Self { children: Vec::new() }
    }

    /// Appends content to the bar, such as the menu bar or the workspace tabs.
    pub fn item(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }
}

impl Default for TopBar {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for TopBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(spacing::SM)
            .h(sizes::TOP_BAR_HEIGHT)
            .px(spacing::SM)
            .bg(colors.chrome.hsla())
            .children(self.children)
    }
}
