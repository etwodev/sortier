use std::path::{Path, PathBuf};

use gpui::{Action, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, actions};
use sortier_library::library::{FitMode, SortKey};

actions!(
    sortier,
    [
        AddFolder,
        Rescan,
        Quit,
        HideApp,
        HideOtherApps,
        ShowAllApps,
        Undo,
        Redo,
        SelectAll,
        SelectNone,
        MakeSequence,
        SplitSequence,
        MakeCollection,
        DeleteSelected,
        ToggleSequenceTiles,
        ToggleCrop,
        LargerTiles,
        SmallerTiles,
        ToggleSortDirection,
        ToggleSpread,
        ToggleReadingDirection,
        TogglePixelated,
        ToggleHardwareDecoding,
        ToggleFullscreen,
        ShowShortcuts,
        About,
    ]
);

/// Removes a library folder (its files stay on disk), from the File menu.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = sortier, no_json)]
pub struct RemoveFolder {
    pub path: PathBuf,
}

/// Sorts the results by `key`, from the View menu.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = sortier, no_json)]
pub struct SortBy {
    pub key: SortKey,
}

/// How viewers fit pictures, from the View menu.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = sortier, no_json)]
pub struct FitBy {
    pub fit: FitMode,
}

/// Window-wide shortcuts. `secondary` is Cmd on macOS and Ctrl elsewhere.
/// Editor shortcuts live with each editor and only apply while it has focus.
pub fn key_bindings() -> Vec<KeyBinding> {
    let mut bindings = vec![
        KeyBinding::new("secondary-o", AddFolder, None),
        KeyBinding::new("f5", Rescan, None),
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-z", Undo, None),
        KeyBinding::new("secondary-shift-z", Redo, None),
        KeyBinding::new("secondary-g", MakeSequence, None),
        KeyBinding::new("secondary-shift-g", SplitSequence, None),
        KeyBinding::new("f11", ToggleFullscreen, None),
        KeyBinding::new("f1", ShowShortcuts, None),
    ];
    if cfg!(target_os = "windows") {
        bindings.push(KeyBinding::new("ctrl-y", Redo, None));
    }
    if cfg!(target_os = "macos") {
        bindings.push(KeyBinding::new("cmd-h", HideApp, None));
        bindings.push(KeyBinding::new("alt-cmd-h", HideOtherApps, None));
    }
    bindings
}

/// What the menus show right now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuState {
    pub undo: Option<String>,
    pub redo: Option<String>,
    pub folders: Vec<PathBuf>,
    pub has_selection: bool,
    pub group_view: bool,
    pub crop: bool,
    pub sort: SortKey,
    pub sort_desc: bool,
    pub fit: FitMode,
    pub spread: bool,
    pub rtl: bool,
    pub pixelated: bool,
    pub hardware_decoding: bool,
    /// The graphics card decoder found, if any.
    pub hardware_decoder: Option<&'static str>,
}

/// The menus of the macOS menu bar: the application menu (About, Services, hiding, Quit), then the File, Edit,
/// View and Help menus, without the Quit and About items the application menu has.
pub fn native_menus(state: &MenuState) -> Vec<Menu> {
    let application = Menu::new("Sortier").items([
        MenuItem::action("About Sortier", About),
        MenuItem::separator(),
        MenuItem::os_submenu("Services", SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action("Hide Sortier", HideApp),
        MenuItem::action("Hide Others", HideOtherApps),
        MenuItem::action("Show All", ShowAllApps),
        MenuItem::separator(),
        MenuItem::action("Quit Sortier", Quit),
    ]);
    let mut menus = vec![application];
    for mut menu in window_menus(state) {
        menu.items.retain(|item| !matches!(item, MenuItem::Action { action, .. } if action.partial_eq(&Quit) || action.partial_eq(&About)));
        while matches!(menu.items.last(), Some(MenuItem::Separator)) {
            menu.items.pop();
        }
        menus.push(menu);
    }
    menus
}

/// The File, Edit, View and Help menus shown in the window's menu bar.
pub fn window_menus(state: &MenuState) -> Vec<Menu> {
    vec![
        Menu::new("File").items([
            MenuItem::action("Add Folder...", AddFolder),
            MenuItem::action("Rescan Folders", Rescan),
            MenuItem::submenu(Menu::new("Remove Folder").items(folder_items(&state.folders))),
            MenuItem::separator(),
            MenuItem::action("Quit", Quit),
        ]),
        Menu::new("Edit").items([
            edit_item("Undo", state.undo.as_deref(), Undo, OsAction::Undo),
            edit_item("Redo", state.redo.as_deref(), Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::action("Select All", SelectAll),
            MenuItem::action("Select None", SelectNone).disabled(!state.has_selection),
            MenuItem::separator(),
            MenuItem::action("Make Sequence", MakeSequence).disabled(!state.has_selection),
            MenuItem::action("Split Sequence", SplitSequence).disabled(!state.has_selection),
            MenuItem::action("Make Collection", MakeCollection).disabled(!state.has_selection),
            MenuItem::separator(),
            MenuItem::action("Delete\u{2026}", DeleteSelected).disabled(!state.has_selection),
        ]),
        Menu::new("View").items([
            check("Sequences as One Tile", state.group_view, ToggleSequenceTiles),
            check("Crop Tiles to Fill", state.crop, ToggleCrop),
            MenuItem::action("Larger Tiles", LargerTiles),
            MenuItem::action("Smaller Tiles", SmallerTiles),
            MenuItem::submenu(Menu::new("Sort By").items(
                [SortKey::Name, SortKey::Modified, SortKey::Added, SortKey::Size]
                    .map(|key| check(key.label(), state.sort == key, SortBy { key })),
            )),
            check("Descending", state.sort_desc, ToggleSortDirection),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Fit").items(
                [FitMode::Fit, FitMode::Width, FitMode::Height, FitMode::Original]
                    .map(|fit| check(fit.label(), state.fit == fit, FitBy { fit })),
            )),
            check("Two-Page Spread", state.spread, ToggleSpread),
            check("Right to Left", state.rtl, ToggleReadingDirection),
            check("Sharp Pixels When Zoomed In", state.pixelated, TogglePixelated),
            hardware_item(state),
            MenuItem::separator(),
            MenuItem::action("Full Screen", ToggleFullscreen),
        ]),
        Menu::new("Help").items([
            MenuItem::action("Keyboard Shortcuts", ShowShortcuts),
            MenuItem::action("About Sortier", About),
        ]),
    ]
}

/// "Decode Video on the GPU (VideoToolbox)" when a graphics card decoder was found; greyed out otherwise.
fn hardware_item(state: &MenuState) -> MenuItem {
    match state.hardware_decoder {
        Some(decoder) => check(&format!("Decode Video on the GPU ({decoder})"), state.hardware_decoding, ToggleHardwareDecoding),
        None => MenuItem::action("Decode Video on the GPU (None Found)", ToggleHardwareDecoding).disabled(true),
    }
}

fn check(label: &str, on: bool, action: impl Action) -> MenuItem {
    MenuItem::action(label.to_string(), action).checked(on)
}

/// "Undo Add Tags" when there is something to undo, otherwise a disabled "Undo".
fn edit_item(verb: &str, label: Option<&str>, action: impl Action, os_action: OsAction) -> MenuItem {
    match label {
        Some(label) => MenuItem::os_action(format!("{verb} {label}"), action, os_action),
        None => MenuItem::os_action(verb.to_string(), action, os_action).disabled(true),
    }
}

fn folder_items(folders: &[PathBuf]) -> Vec<MenuItem> {
    if folders.is_empty() {
        let placeholder = RemoveFolder { path: PathBuf::new() };
        return vec![MenuItem::action("No Folders", placeholder).disabled(true)];
    }
    folders
        .iter()
        .map(|path| MenuItem::action(folder_label(path), RemoveFolder { path: path.clone() }))
        .collect()
}

/// "name — parent folder", so folders with the same name stay distinguishable.
fn folder_label(path: &Path) -> String {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() && !name.is_empty() => format!("{name} \u{2014} {}", parent.display()),
        _ => path.display().to_string(),
    }
}
