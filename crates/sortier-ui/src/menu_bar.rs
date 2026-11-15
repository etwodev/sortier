use gpui::{App, Entity, IntoElement, Menu, ParentElement, RenderOnce, Styled, Window, div};
use gpui_component::GlobalState;
use gpui_component::menu::AppMenuBar;

/// The in-window application menu bar (File, Edit, Help, ...), with keyboard
/// navigation and shortcut hints. Built from GPUI [`Menu`] definitions, the same
/// type the native macOS menu bar takes, so one definition can serve both.
#[derive(Clone, IntoElement)]
pub struct MenuBar {
    inner: Entity<AppMenuBar>,
}

impl MenuBar {
    pub fn new(menus: Vec<Menu>, cx: &mut App) -> Self {
        store_menus(menus, cx);
        Self {
            inner: AppMenuBar::new(cx),
        }
    }

    /// Replaces the menus, for example after an item's enabled state changes.
    pub fn set_menus(&self, menus: Vec<Menu>, cx: &mut App) {
        store_menus(menus, cx);
        self.inner.update(cx, |bar, cx| bar.reload(cx));
    }
}

fn store_menus(menus: Vec<Menu>, cx: &mut App) {
    let owned = menus.into_iter().map(Menu::owned).collect();
    GlobalState::global_mut(cx).set_app_menus(owned);
}

impl RenderOnce for MenuBar {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().flex_none().h_full().child(self.inner)
    }
}
