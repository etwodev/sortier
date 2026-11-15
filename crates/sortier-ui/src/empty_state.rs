use gpui::prelude::FluentBuilder as _;
use gpui::{AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div};

use crate::icon::{Icon, IconKind, IconSize};
use crate::label::{Label, TextSize, TextTone};
use crate::theme::ActiveSortierTheme;
use crate::tokens::{sizes, spacing};

/// A centred placeholder for a view with nothing to show yet: an icon in a
/// soft round bubble, a title and a sentence saying what will appear or what
/// to do next.
#[derive(IntoElement)]
pub struct EmptyState {
    icon: IconKind,
    title: SharedString,
    description: SharedString,
    action: Option<AnyElement>,
}

impl EmptyState {
    pub fn new(icon: IconKind, title: impl Into<SharedString>, description: impl Into<SharedString>) -> Self {
        Self {
            icon,
            title: title.into(),
            description: description.into(),
            action: None,
        }
    }

    /// A control below the description that does what it invites, usually
    /// a button.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(spacing::SM)
            .p(spacing::LG)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(sizes::EMPTY_STATE_BUBBLE)
                    .mb(spacing::XS)
                    .rounded_full()
                    .bg(colors.selection.hsla())
                    .text_color(colors.accent_hover.hsla())
                    .child(Icon::new(self.icon).size(IconSize::Huge)),
            )
            .child(Label::new(self.title).size(TextSize::Heading))
            .child(
                div()
                    .max_w(sizes::EMPTY_STATE_TEXT_WIDTH)
                    .text_center()
                    .child(Label::new(self.description).size(TextSize::Body).tone(TextTone::Secondary)),
            )
            .when_some(self.action, |state, action| state.child(div().mt(spacing::XS).child(action)))
    }
}
