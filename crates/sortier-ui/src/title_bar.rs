use gpui::{AnyElement, App, IntoElement, ParentElement, RenderOnce, Styled, Window, WindowOptions, div};

use crate::icon::{Icon, IconKind, IconSize};
use crate::theme::ActiveSortierTheme;
use crate::tokens::spacing;

/// Window options for a window whose chrome is drawn by [`TitleBar`]: a
/// transparent native title bar with the macOS traffic lights positioned for it,
/// and window dragging owned by the title bar.
pub fn window_options() -> WindowOptions {
    gpui_component::TitleBar::window_options()
}

/// The window's custom title bar. It keeps the platform's own window controls
/// (traffic lights on macOS, caption buttons with native hit-testing on Windows),
/// acts as the window drag region, and shows `centre` (such as a switcher) centred
/// on the whole window after the app's heart.
#[derive(IntoElement)]
pub struct TitleBar {
    centre: AnyElement,
}

impl TitleBar {
    pub fn new(centre: impl IntoElement) -> Self {
        Self {
            centre: centre.into_any_element(),
        }
    }
}

impl RenderOnce for TitleBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let chrome = colors.chrome.hsla();
        // The overlay itself has no interactive handlers, so only what's in the
        // centre takes hits away from the drag region underneath it. An overlay is
        // used because the bar reserves space for the traffic lights on macOS, so
        // centring inside the bar would sit off the window's centre.
        let centred_title = div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .gap(spacing::XS)
            .child(div().text_color(colors.accent.hsla()).child(Icon::new(IconKind::Heart).size(IconSize::Small)))
            .child(self.centre);

        div()
            .relative()
            .flex_shrink_0()
            // No bottom border: the top bar below shares the chrome colour so the
            // two read as one surface.
            .child(gpui_component::TitleBar::new().bg(chrome).border_b_0())
            .child(centred_title)
    }
}
