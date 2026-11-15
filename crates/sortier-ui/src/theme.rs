use std::rc::Rc;

use gpui::{App, Global, Subscription, Window, WindowAppearance};
use gpui_component::{Theme, ThemeConfig, ThemeConfigColors, ThemeMode};

use crate::tokens::{Appearance, BASE_FONT, SemanticColors};

/// The active Sortier palette. Kept in step with gpui-component's [`Theme`] so
/// library widgets and Sortier's own primitives always agree.
pub struct SortierTheme {
    pub appearance: Appearance,
    pub colors: SemanticColors,
    /// How large the interface is drawn, 1.0 being as designed.
    pub ui_scale: f32,
}

/// Smallest and largest UI scale.
pub const UI_SCALE_RANGE: (f32, f32) = (0.75, 2.0);

impl Global for SortierTheme {}

/// Read access to the active [`SortierTheme`] from any GPUI context.
pub trait ActiveSortierTheme {
    fn sortier_theme(&self) -> &SortierTheme;
}

impl ActiveSortierTheme for App {
    fn sortier_theme(&self) -> &SortierTheme {
        self.global::<SortierTheme>()
    }
}

/// Installs Sortier's light and dark configurations into gpui-component and
/// applies the system appearance. Must run after `gpui_component::init`.
pub(crate) fn init(cx: &mut App) {
    install_configs(1.0, cx);
    cx.set_global(SortierTheme {
        appearance: Appearance::Light,
        colors: SemanticColors::for_appearance(Appearance::Light),
        ui_scale: 1.0,
    });
    set_appearance(appearance_from_system(cx.window_appearance()), cx);
}

fn install_configs(scale: f32, cx: &mut App) {
    let theme = Theme::global_mut(cx);
    theme.light_theme = Rc::new(theme_config(Appearance::Light, scale));
    theme.dark_theme = Rc::new(theme_config(Appearance::Dark, scale));
}

/// Draws every window's interface at `scale` (clamped to
/// [`UI_SCALE_RANGE`]): text, spacing and every sized control grow or
/// shrink together.
pub fn set_ui_scale(scale: f32, cx: &mut App) {
    let scale = scale.clamp(UI_SCALE_RANGE.0, UI_SCALE_RANGE.1);
    install_configs(scale, cx);
    let appearance = cx.global::<SortierTheme>().appearance;
    cx.global_mut::<SortierTheme>().ui_scale = scale;
    set_appearance(appearance, cx);
    cx.refresh_windows();
}

/// The system's light or dark appearance.
pub fn system_appearance(cx: &App) -> Appearance {
    appearance_from_system(cx.window_appearance())
}

/// Switches every window to `appearance`.
pub fn set_appearance(appearance: Appearance, cx: &mut App) {
    let ui_scale = cx.global::<SortierTheme>().ui_scale;
    cx.set_global(SortierTheme {
        appearance,
        colors: SemanticColors::for_appearance(appearance),
        ui_scale,
    });
    let mode = match appearance {
        Appearance::Light => ThemeMode::Light,
        Appearance::Dark => ThemeMode::Dark,
    };
    Theme::change(mode, None, cx);
}

/// Keeps the theme in step with the operating system's light/dark setting for as
/// long as the returned subscription is held and `follow` says to.
pub fn follow_system_appearance(window: &mut Window, follow: impl Fn(&App) -> bool + 'static) -> Subscription {
    window.observe_window_appearance(move |window, cx| {
        if follow(cx) {
            set_appearance(appearance_from_system(window.appearance()), cx);
        }
    })
}

fn appearance_from_system(appearance: WindowAppearance) -> Appearance {
    match appearance {
        WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
    }
}

/// Maps Sortier's semantic tokens onto gpui-component's theme keys. Keys left
/// unset fall back to values gpui-component derives from the ones set here.
fn theme_config(appearance: Appearance, scale: f32) -> ThemeConfig {
    let c = SemanticColors::for_appearance(appearance);
    let (name, mode) = match appearance {
        Appearance::Light => ("Sortier Light", ThemeMode::Light),
        Appearance::Dark => ("Sortier Dark", ThemeMode::Dark),
    };
    // Some fields of ThemeConfigColors are private, so it can't use struct-update syntax.
    let mut colors = ThemeConfigColors::default();
    colors.background = Some(c.background.hex());
    colors.foreground = Some(c.text_primary.hex());
    colors.border = Some(c.border.hex());
    colors.caret = Some(c.text_primary.hex());
    colors.ring = Some(c.focus_ring.hex());
    colors.selection = Some(c.selection.hex());
    colors.input = Some(c.border_strong.hex());
    colors.drag_border = Some(c.accent.hex());
    colors.link = Some(c.accent.hex());
    colors.window_border = Some(c.seam.hex());
    colors.accent = Some(c.accent.hex());
    colors.accent_foreground = Some(c.accent_foreground.hex());
    colors.primary = Some(c.accent.hex());
    colors.primary_hover = Some(c.accent_hover.hex());
    colors.primary_active = Some(c.accent_pressed.hex());
    colors.primary_foreground = Some(c.accent_foreground.hex());
    colors.secondary = Some(c.surface.hex());
    colors.secondary_hover = Some(c.hover.hex());
    colors.secondary_active = Some(c.pressed.hex());
    colors.secondary_foreground = Some(c.text_primary.hex());
    colors.muted = Some(c.surface.hex());
    colors.muted_foreground = Some(c.text_secondary.hex());
    colors.popover = Some(c.surface_raised.hex());
    colors.popover_foreground = Some(c.text_primary.hex());
    colors.danger = Some(c.danger.hex());
    colors.danger_foreground = Some(c.danger_foreground.hex());
    colors.warning = Some(c.warning.hex());
    colors.success = Some(c.success.hex());
    colors.title_bar = Some(c.chrome.hex());
    colors.title_bar_border = Some(c.seam.hex());
    colors.tab_bar = Some(c.chrome.hex());
    colors.tab = Some(c.chrome.hex());
    colors.tab_active = Some(c.background.hex());
    colors.tab_foreground = Some(c.text_secondary.hex());
    colors.tab_active_foreground = Some(c.text_primary.hex());
    colors.list = Some(c.background.hex());
    colors.list_hover = Some(c.hover.hex());
    colors.list_active = Some(c.selection.hex());
    colors.list_active_border = Some(c.accent.hex());

    ThemeConfig {
        name: name.into(),
        mode,
        font_size: Some(BASE_FONT * scale),
        radius: Some((8.0 * scale).round() as usize),
        radius_lg: Some((12.0 * scale).round() as usize),
        shadow: Some(true),
        colors,
        ..Default::default()
    }
}
