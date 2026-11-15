use gpui::{AnyElement, App, IntoElement, ParentElement, RenderOnce, Styled, Window, div};

use crate::theme::ActiveSortierTheme;
use crate::title_bar::TitleBar;
use crate::top_bar::TopBar;

/// Top-level window layout, top to bottom: the [`TitleBar`], the optional
/// [`TopBar`], then a body that fills the rest of the window.
#[derive(IntoElement)]
pub struct WindowFrame {
    title_bar: TitleBar,
    top_bar: Option<TopBar>,
    body: Option<AnyElement>,
}

impl WindowFrame {
    pub fn new(title_bar: TitleBar) -> Self {
        Self {
            title_bar,
            top_bar: None,
            body: None,
        }
    }

    pub fn top_bar(mut self, top_bar: TopBar) -> Self {
        self.top_bar = Some(top_bar);
        self
    }

    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }
}

impl RenderOnce for WindowFrame {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background.hsla())
            .text_color(colors.text_primary.hsla())
            .child(self.title_bar)
            .children(self.top_bar)
            .child(div().flex_1().min_h_0().children(self.body))
    }
}
