use std::rc::Rc;

use gpui::{AnyElement, App, Context, Div, ElementId, IntoElement, RenderOnce, SharedString, Stateful, Window};
use gpui_component::button::{Button, ButtonRounded, ButtonVariants as _};
use gpui_component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_component::{Selectable as _, Sizable as _};

use crate::icon::IconKind;

type MenuHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// One row of a dropdown or context menu.
#[derive(Clone)]
pub enum MenuEntry {
    Command {
        label: SharedString,
        icon: Option<IconKind>,
        checked: bool,
        disabled: bool,
        handler: MenuHandler,
    },
    /// A row that opens more rows beside it.
    Submenu {
        label: SharedString,
        icon: Option<IconKind>,
        disabled: bool,
        entries: Vec<MenuEntry>,
    },
    /// A non-interactive title for the rows below it.
    Heading(SharedString),
    Separator,
}

impl MenuEntry {
    pub fn command(label: impl Into<SharedString>, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self::Command {
            label: label.into(),
            icon: None,
            checked: false,
            disabled: false,
            handler: Rc::new(handler),
        }
    }

    /// A row opening `entries` beside the menu, to keep long menus short.
    pub fn submenu(label: impl Into<SharedString>, entries: Vec<MenuEntry>) -> Self {
        Self::Submenu {
            label: label.into(),
            icon: None,
            disabled: false,
            entries,
        }
    }

    pub fn heading(label: impl Into<SharedString>) -> Self {
        Self::Heading(label.into())
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    /// Sets the icon of a command or submenu row. No effect on other rows.
    pub fn icon(mut self, kind: IconKind) -> Self {
        if let Self::Command { icon, .. } | Self::Submenu { icon, .. } = &mut self {
            *icon = Some(kind);
        }
        self
    }

    /// Marks a command row as the current choice. No effect on other rows.
    pub fn checked(mut self, value: bool) -> Self {
        if let Self::Command { checked, .. } = &mut self {
            *checked = value;
        }
        self
    }

    /// Greys out a command or submenu row. No effect on other rows.
    pub fn disabled(mut self, value: bool) -> Self {
        if let Self::Command { disabled, .. } | Self::Submenu { disabled, .. } = &mut self {
            *disabled = value;
        }
        self
    }
}

/// Fraction of the window's height a menu may fill before it scrolls.
const MENU_MAX_HEIGHT_FRACTION: f32 = 0.6;

/// Fills `menu` with `entries`. Long menus scroll instead of running off the
/// window.
pub(crate) fn build_popup(menu: PopupMenu, entries: Vec<MenuEntry>, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let max_height = window.viewport_size().height * MENU_MAX_HEIGHT_FRACTION;
    let mut menu = menu.scrollable(true).max_h(max_height);
    for entry in entries {
        menu = match entry {
            MenuEntry::Command {
                label,
                icon,
                checked,
                disabled,
                handler,
            } => {
                let mut item = PopupMenuItem::new(label)
                    .checked(checked)
                    .disabled(disabled)
                    .on_click(move |_, window, cx| handler(window, cx));
                if let Some(icon) = icon {
                    item = item.icon(icon);
                }
                menu.item(item)
            }
            MenuEntry::Submenu {
                label,
                icon,
                disabled,
                entries,
            } => {
                // A disabled submenu can't open, so it's shown as a greyed row.
                if disabled {
                    let mut item = PopupMenuItem::new(label).disabled(true);
                    if let Some(icon) = icon {
                        item = item.icon(icon);
                    }
                    menu.item(item)
                } else {
                    menu.submenu_with_icon(icon.map(Into::into), label, window, cx, move |menu, window, cx| {
                        build_popup(menu, entries.clone(), window, cx)
                    })
                }
            }
            MenuEntry::Heading(label) => menu.label(label),
            MenuEntry::Separator => menu.separator(),
        };
    }
    menu
}

/// A compact, borderless button that opens a menu below it. With a label it
/// shows a dropdown caret (choosers such as an area's editor-type selector);
/// icon-only buttons need an accessible label.
#[derive(IntoElement)]
pub struct DropdownButton {
    id: ElementId,
    label: Option<SharedString>,
    accessible_label: Option<SharedString>,
    icon: Option<IconKind>,
    entries: Vec<MenuEntry>,
}

impl DropdownButton {
    pub fn new(id: impl Into<ElementId>, entries: Vec<MenuEntry>) -> Self {
        Self {
            id: id.into(),
            label: None,
            accessible_label: None,
            icon: None,
            entries,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The name screen readers announce, required when there is no label.
    pub fn accessible_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessible_label = Some(label.into());
        self
    }

    pub fn icon(mut self, icon: IconKind) -> Self {
        self.icon = Some(icon);
        self
    }
}

impl RenderOnce for DropdownButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut button = Button::new(self.id).ghost().xsmall().rounded(ButtonRounded::Large);
        if let Some(label) = self.label {
            button = button.label(label).dropdown_caret(true);
        }
        if let Some(label) = self.accessible_label {
            button = button.accessibility_label(label);
        }
        if let Some(icon) = self.icon {
            button = button.icon(icon);
        }
        let entries = self.entries;
        button.selected(false).dropdown_menu(move |menu, window, cx| build_popup(menu, entries.clone(), window, cx))
    }
}

/// Attaches a right-click menu to an interactive element. `build` runs each
/// time the menu opens, so it can read the pointer position and current state.
pub fn with_context_menu(
    target: Stateful<Div>,
    build: impl Fn(&mut Window, &mut App) -> Vec<MenuEntry> + 'static,
) -> AnyElement {
    target
        .context_menu(move |menu, window, cx| {
            let entries = build(window, cx);
            build_popup(menu, entries, window, cx)
        })
        .into_any_element()
}
