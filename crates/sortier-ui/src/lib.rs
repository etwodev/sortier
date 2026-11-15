//! Sortier's shared UI layer: design tokens, theming and the primitives every
//! editor composes. This is the only crate that touches gpui-component; editors
//! and the app use these wrappers so the visual language stays in one place.

pub mod area;
pub mod button;
pub mod chip;
pub mod dialog;
pub mod empty_state;
pub mod fields;
pub mod gallery;
pub mod icon;
pub mod label;
pub mod menu;
pub mod menu_bar;
pub mod nav_list;
pub mod progress;
pub mod side_tabs;
pub mod tab;
pub mod text_field;
pub mod theme;
pub mod title_bar;
pub mod tokens;
pub mod top_bar;
pub mod tree_row;
pub mod window_frame;

use gpui::{AnyView, App, AppContext as _, Entity, Window};

pub use icon::Assets;

/// Initialises gpui-component and installs the Sortier theme. Call once at
/// startup, before opening any window.
pub fn init(cx: &mut App) {
    gpui_component::init(cx);
    theme::init(cx);
}

/// Wraps a window's top-level view in the host that renders dialogs,
/// notifications and tooltips above it. Use as the window's root view.
pub fn window_root(view: impl Into<AnyView>, window: &mut Window, cx: &mut App) -> Entity<gpui_component::Root> {
    cx.new(|cx| gpui_component::Root::new(view, window, cx))
}
