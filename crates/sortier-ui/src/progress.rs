use gpui::{
    App, InteractiveElement as _, IntoElement, ParentElement, Pixels, Point, RenderOnce, SharedString, Styled, Window,
    div, px,
};
use gpui_component::Sizable as _;

use crate::label::{Label, TextSize, TextTone};
use crate::theme::ActiveSortierTheme;
use crate::tokens::{radius, sizes, spacing};

/// A modal progress window over the whole window: dims and blocks the UI
/// while long work (such as importing files) runs.
#[derive(IntoElement)]
pub struct ProgressOverlay {
    title: SharedString,
    detail: SharedString,
    /// `0.0..=1.0`.
    fraction: f32,
}

impl ProgressOverlay {
    pub fn new(title: impl Into<SharedString>, detail: impl Into<SharedString>, fraction: f32) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            fraction: fraction.clamp(0.0, 1.0),
        }
    }
}

impl RenderOnce for ProgressOverlay {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let card = div()
            .flex()
            .flex_col()
            .gap(spacing::SM)
            .w(sizes::PROGRESS_WIDTH)
            .p(spacing::LG)
            .rounded(radius::LG)
            .border_1()
            .border_color(colors.border_strong.hsla())
            .bg(colors.surface_raised.hsla())
            .shadow_lg()
            .child(Label::new(self.title).size(TextSize::Body))
            .child(
                gpui_component::progress::Progress::new("progress")
                    .value(self.fraction * 100.0)
                    .accessibility_label("Progress"),
            )
            .child(Label::new(self.detail).size(TextSize::Small).tone(TextTone::Secondary));
        div()
            .id("progress-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(colors.scrim.hsla_with_alpha(0.45))
            // Swallow pointer input so nothing underneath can be used meanwhile.
            .occlude()
            .child(card)
    }
}

/// A small spinner that follows the pointer while work is running, standing
/// in for an operating-system busy cursor (GPUI has none).
#[derive(IntoElement)]
pub struct BusyPointer {
    position: Point<Pixels>,
}

impl BusyPointer {
    /// `position` is the pointer position in window coordinates.
    pub fn new(position: Point<Pixels>) -> Self {
        Self { position }
    }
}

impl RenderOnce for BusyPointer {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .absolute()
            .left(self.position.x + px(14.))
            .top(self.position.y + px(14.))
            .child(gpui_component::spinner::Spinner::new().small())
    }
}

/// An inline progress bar that fills the width it's given.
#[derive(IntoElement)]
pub struct ProgressBar {
    /// `0.0..=1.0`.
    fraction: f32,
}

impl ProgressBar {
    pub fn new(fraction: f32) -> Self {
        Self {
            fraction: fraction.clamp(0.0, 1.0),
        }
    }
}

impl RenderOnce for ProgressBar {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().w_full().child(
            gpui_component::progress::Progress::new("progress-bar")
                .value(self.fraction * 100.0)
                .accessibility_label("Progress"),
        )
    }
}
