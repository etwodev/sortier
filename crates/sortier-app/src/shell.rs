use std::collections::HashMap;

use gpui::{
    App,
    AnyElement, AppContext as _, Context, Entity, ExternalPaths, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    PathPromptOptions, Render, StatefulInteractiveElement as _, Styled, Subscription, Window, div, prelude::FluentBuilder as _,
};
use sortier_areas::WorkspaceId;
use sortier_library::library::FitMode;
use sortier_client::RemoteClient;
use sortier_library::vault::VaultKind;
use sortier_ui::dialog::{FormField, open_form_dialog, open_message_dialog};
use sortier_ui::icon::IconKind;
use sortier_ui::label::{Label, TextSize, TextTone};
use sortier_ui::menu::{DropdownButton, MenuEntry};
use sortier_ui::progress::ProgressStrip;
use sortier_ui::menu_bar::MenuBar;
use sortier_ui::theme::ActiveSortierTheme;
use sortier_ui::title_bar::TitleBar;
use sortier_ui::tokens::{radius, spacing};
use sortier_ui::top_bar::TopBar;
use sortier_ui::window_frame::WindowFrame;

use crate::area_host::AreaHost;
use crate::editors::EditorContext;
use crate::layouts::LayoutModel;
use crate::delete::confirm_delete;
use crate::editors::human_size;
use crate::menus::*;
use crate::model::LibraryModel;
use crate::render_service::RenderService;
use crate::workspace_tabs::WorkspaceTabs;

/// The main window: the title bar, the menu bar and workspace tabs, and the
/// active workspace's areas. Handles window-wide commands and dropped folders.
pub struct SortierWindow {
    library: Entity<LibraryModel>,
    layouts: Entity<LayoutModel>,
    renderer: RenderService,
    menu_bar: MenuBar,
    menu_state: MenuState,
    tabs: Entity<WorkspaceTabs>,
    hosts: HashMap<WorkspaceId, Entity<AreaHost>>,
    focus_handle: FocusHandle,
    dropping: bool,
    shortcuts: bool,
    _observers: Vec<Subscription>,
}

impl SortierWindow {
    pub fn new(library: Entity<LibraryModel>, renderer: RenderService, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let layouts = cx.new(|_| LayoutModel::load());
        let menu_state = menu_state_of(library.read(cx));
        let observers = vec![
            cx.observe_in(&library, window, |this, library, window, cx| {
                let state = menu_state_of(library.read(cx));
                if state != this.menu_state {
                    this.menu_bar.set_menus(window_menus(&state), cx);
                    if cfg!(target_os = "macos") {
                        cx.set_menus(native_menus(&state));
                    }
                    this.menu_state = state;
                }
                window.set_window_title(&title(library.read(cx)));
                cx.notify();
            }),
            cx.observe(&layouts, |_, _, cx| cx.notify()),
        ];
        window.set_window_title(&title(library.read(cx)));
        if cfg!(target_os = "macos") {
            cx.set_menus(native_menus(&menu_state));
        }
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        Self {
            menu_bar: MenuBar::new(window_menus(&menu_state), cx),
            menu_state,
            tabs: cx.new(|cx| WorkspaceTabs::new(layouts.clone(), cx)),
            library,
            layouts,
            renderer,
            hosts: HashMap::new(),
            focus_handle,
            dropping: false,
            shortcuts: false,
            _observers: observers,
        }
    }

    fn host_for(&mut self, workspace: WorkspaceId, cx: &mut Context<Self>) -> Entity<AreaHost> {
        let context = EditorContext {
            library: self.library.clone(),
            layouts: self.layouts.clone(),
            renderer: self.renderer.clone(),
        };
        let layouts = self.layouts.clone();
        self.hosts
            .entry(workspace)
            .or_insert_with(|| cx.new(|cx| AreaHost::new(layouts, workspace, context, cx)))
            .clone()
    }

    fn add_folder(&mut self, _: &AddFolder, _: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add to Library".into()),
        });
        let library = self.library.clone();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await {
                library.update(cx, |library, cx| library.add_folders(paths, cx));
            }
        })
        .detach();
    }

    fn with_library(&self, cx: &mut Context<Self>, change: impl FnOnce(&mut LibraryModel, &mut Context<LibraryModel>)) {
        self.library.update(cx, change);
    }

    fn files_dropped(&mut self, paths: &ExternalPaths, _: &mut Window, cx: &mut Context<Self>) {
        self.dropping = false;
        let paths = paths.paths().to_vec();
        self.with_library(cx, |library, cx| library.add_folders(paths, cx));
    }
}

/// The bar along the bottom of the window while files are uploading to a remote vault.
fn upload_strip(library: &LibraryModel) -> Option<ProgressStrip> {
    let progress = library.upload_progress()?;
    let fraction = if progress.bytes > 0 {
        progress.finished_bytes as f32 / progress.bytes as f32
    } else {
        progress.done as f32 / progress.files.max(1) as f32
    };
    let name = progress.current.rsplit('/').next().unwrap_or_default();
    let label = format!("Uploading {} of {}: {name}", (progress.done + 1).min(progress.files), progress.files);
    let detail = format!("{} of {}", human_size(progress.finished_bytes), human_size(progress.bytes));
    Some(ProgressStrip::new(label, detail, fraction))
}

/// The window's title, as the system shows it (in the Window menu and when switching apps).
fn title(library: &LibraryModel) -> String {
    format!("{} \u{2014} Sortier", library.vaults().current().name)
}

/// The vault switcher in the middle of the title bar: the open vault's name, opening a menu of every vault and of
/// making, renaming and removing them.
fn vault_switcher(library: &Entity<LibraryModel>, cx: &App) -> DropdownButton {
    let vaults = library.read(cx).vaults().clone();
    let current = vaults.current().clone();
    let mut entries = vec![MenuEntry::heading("Vaults")];
    for vault in &vaults.vaults {
        let (library, id) = (library.clone(), vault.id.clone());
        entries.push(
            MenuEntry::command(vault.name.clone(), move |_, cx| library.update(cx, |library, cx| library.switch_vault(&id, cx)))
                .checked(vault.id == current.id),
        );
    }
    entries.push(MenuEntry::separator());
    let connecting = library.clone();
    entries.push(
        MenuEntry::command("Connect to Sortier Remote\u{2026}", move |window, cx| connect_to_remote(connecting.clone(), window, cx))
            .icon(IconKind::Server),
    );
    if let VaultKind::Remote { cache_mb, .. } = current.kind {
        let (sizing, id) = (library.clone(), current.id.clone());
        entries.push(MenuEntry::command("Cache Size\u{2026}", move |window, cx| {
            let (library, id) = (sizing.clone(), id.clone());
            open_form_dialog(
                "Cache Size",
                Some("How much of this vault's thumbnails, pages and files are kept on this computer, in megabytes. What was used longest ago goes first.".into()),
                vec![FormField::new("Megabytes", cache_mb.to_string())],
                "Set",
                move |values, _, cx| {
                    let Ok(megabytes) = values[0].trim().parse::<u64>() else { return false };
                    library.update(cx, |library, cx| library.set_cache_size(&id, megabytes, cx));
                    true
                },
                window,
                cx,
            );
        }));
    }
    let creating = library.clone();
    entries.push(
        MenuEntry::command("New Vault\u{2026}", move |window, cx| {
            let library = creating.clone();
            open_form_dialog(
                "New Vault",
                Some("A vault is a library of its own: its own folders, tags, sequences and collections.".into()),
                vec![FormField::new("Name", "")],
                "Create",
                move |values, _, cx| {
                    let name = values[0].trim().to_string();
                    if name.is_empty() {
                        return false;
                    }
                    library.update(cx, |library, cx| library.create_vault(&name, cx));
                    true
                },
                window,
                cx,
            );
        })
        .icon(IconKind::FolderPlus),
    );
    let (renaming, id, name) = (library.clone(), current.id.clone(), current.name.clone());
    entries.push(MenuEntry::command(format!("Rename \u{201c}{name}\u{201d}\u{2026}"), move |window, cx| {
        let (library, id) = (renaming.clone(), id.clone());
        open_form_dialog(
            "Rename Vault",
            None,
            vec![FormField::new("Name", name.clone())],
            "Rename",
            move |values, _, cx| {
                library.update(cx, |library, cx| library.rename_vault(&id, &values[0], cx));
                true
            },
            window,
            cx,
        );
    }));
    let (removing, id, name) = (library.clone(), current.id.clone(), current.name.clone());
    entries.push(
        MenuEntry::command(format!("Remove \u{201c}{name}\u{201d}\u{2026}"), move |window, cx| {
            let (library, id) = (removing.clone(), id.clone());
            open_form_dialog(
                format!("Remove \u{201c}{name}\u{201d}?"),
                Some("Sortier forgets this vault: its tags, sequences, collections and covers. The files in its folders aren't touched.".into()),
                Vec::new(),
                "Remove",
                move |_, _, cx| {
                    library.update(cx, |library, cx| library.remove_vault(&id, cx));
                    true
                },
                window,
                cx,
            );
        })
        .icon(IconKind::Trash)
        .disabled(vaults.vaults.len() < 2),
    );
    let label = if library.read(cx).remote_offline() { format!("{} \u{b7} offline", current.name) } else { current.name };
    DropdownButton::new("vault-switcher", entries).label(label)
}

/// Asks for a Sortier Remote server's address and password, then shows its certificate's fingerprint to check before
/// logging in and adding it as a vault.
fn connect_to_remote(library: Entity<LibraryModel>, window: &mut Window, cx: &mut App) {
    open_form_dialog(
        "Connect to Sortier Remote",
        Some("The address of the computer running sortier-remote (add :port if it isn't 7461), and the password set with sortier-remote set-password.".into()),
        vec![FormField::new("Name", ""), FormField::new("Address", ""), FormField::new("Password", "").secret()],
        "Connect",
        move |values, window, cx| {
            let [name, address, password] = <[String; 3]>::try_from(values).unwrap_or_default();
            if address.trim().is_empty() {
                return false;
            }
            let (library, handle) = (library.clone(), window.window_handle());
            let reaching = address.clone();
            cx.spawn(async move |cx| {
                let probe = cx.background_spawn(async move { RemoteClient::probe(&reaching) }).await;
                handle
                    .update(cx, |_, window, cx| match probe {
                        Ok(probe) => trust_remote(library, name, address, password, probe.fingerprint, window, cx),
                        Err(error) => open_message_dialog("Sortier couldn't connect", error.to_string(), window, cx),
                    })
                    .ok();
            })
            .detach();
            true
        },
        window,
        cx,
    );
}

/// Shows a server's certificate `fingerprint` for the user to compare with what `sortier-remote fingerprint` prints,
/// and once they trust it, logs in and adds the vault.
fn trust_remote(library: Entity<LibraryModel>, name: String, address: String, password: String, fingerprint: String, window: &mut Window, cx: &mut App) {
    let grouped: Vec<String> = fingerprint.as_bytes().chunks(4).map(|chunk| String::from_utf8_lossy(chunk).to_uppercase()).collect();
    let description = format!(
        "Make sure this is what sortier-remote fingerprint prints on the server. If it isn't, something may be pretending to be your server.\n\n{}",
        grouped.join(" ")
    );
    open_form_dialog(
        "Trust this server?",
        Some(description.into()),
        Vec::new(),
        "Trust and Log In",
        move |_, window, cx| {
            let (library, handle) = (library.clone(), window.window_handle());
            let (name, address, password, fingerprint) = (name.clone(), address.clone(), password.clone(), fingerprint.clone());
            cx.spawn(async move |cx| {
                let (logging_in, trusted) = (address.clone(), fingerprint.clone());
                let token = cx.background_spawn(async move { RemoteClient::login(&logging_in, &trusted, &password, &device_name()) }).await;
                handle
                    .update(cx, |_, window, cx| match token {
                        Ok(token) => library.update(cx, |library, cx| library.add_remote_vault(&name, &address, &fingerprint, &token, cx)),
                        Err(error) => open_message_dialog("Sortier couldn't log in", error.to_string(), window, cx),
                    })
                    .ok();
            })
            .detach();
            true
        },
        window,
        cx,
    );
}

/// What a server calls this computer among the devices logged in to it.
fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| format!("Sortier on {}", std::env::consts::OS))
}

fn menu_state_of(library: &LibraryModel) -> MenuState {
    let settings = library.settings();
    MenuState {
        undo: library.can_undo().map(str::to_string),
        redo: library.can_redo().map(str::to_string),
        folders: library.library().roots.clone(),
        has_selection: !library.selection().is_empty(),
        group_view: settings.group_view,
        crop: settings.tile_crop,
        sort: settings.sort,
        sort_desc: settings.sort_desc,
        fit: settings.fit,
        spread: settings.spread,
        rtl: settings.rtl,
        pixelated: settings.pixelated,
        hardware_decoding: settings.hardware_decoding,
        hardware_decoder: library.hardware_decoder().map(|decoder| decoder.label()),
    }
}

/// Shortcuts listed by Help › Keyboard Shortcuts.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Library",
        &[
            ("A D W S, arrows", "Move through the results"),
            ("Shift + move", "Extend the selection"),
            ("Enter, double-click", "Open in the viewer"),
            ("T", "Tag the selection"),
            ("Ctrl+F", "Search"),
            ("Ctrl+A", "Select all"),
            ("Ctrl+G / Ctrl+Shift+G", "Make / split a sequence"),
            ("G", "Sequences and collections as one tile"),
            ("Ctrl+\u{2193} / Ctrl+\u{2191}", "Into a collection / back out"),
            ("Cmd+Backspace (Delete on Windows)", "Delete the selected files"),
            ("+ / \u{2212}", "Tile size"),
        ],
    ),
    (
        "Viewer",
        &[
            ("A D, left/right", "Previous / next page"),
            ("W S", "Previous / next item"),
            ("Space", "Scroll down, then next page; play/pause video"),
            ("F", "Cycle fit"),
            ("+ \u{2212} 0, wheel, pinch", "Zoom"),
            ("Drag, two-finger scroll", "Pan"),
            ("2 / R", "Two-page spread / right to left"),
            ("Home / End", "First / last page"),
            ("[ / ]", "Move the page earlier / later"),
            ("M", "Mute"),
            ("Cmd+Backspace (Delete on Windows)", "Delete this file"),
        ],
    ),
    (
        "Window",
        &[
            ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
            ("Ctrl+O", "Add a folder"),
            ("F5", "Rescan folders"),
            ("F11", "Full screen"),
            ("Right-click a seam", "Split or join areas"),
        ],
    ),
];

fn shortcuts_overlay(cx: &mut Context<SortierWindow>) -> AnyElement {
    let colors = cx.sortier_theme().colors;
    let columns = SHORTCUTS.iter().map(|(heading, rows)| {
        div()
            .flex()
            .flex_col()
            .gap(spacing::XS)
            .child(Label::new(*heading).size(TextSize::Heading))
            .children(rows.iter().map(|(keys, what)| {
                div()
                    .flex()
                    .gap(spacing::MD)
                    .child(div().w(gpui::rems(10.)).child(Label::new(*keys).size(TextSize::Small)))
                    .child(Label::new(*what).size(TextSize::Small).tone(TextTone::Secondary))
            }))
    });
    div()
        .id("shortcuts")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(colors.scrim.hsla_with_alpha(0.6))
        .on_click(cx.listener(|this, _, _, cx| {
            this.shortcuts = false;
            cx.notify();
        }))
        .child(
            div()
                .flex()
                .gap(spacing::XL)
                .p(spacing::LG)
                .rounded(radius::LG)
                .border_1()
                .border_color(colors.border.hsla())
                .bg(colors.surface_raised.hsla())
                .children(columns),
        )
        .into_any_element()
}

fn drop_overlay(cx: &mut Context<SortierWindow>) -> AnyElement {
    let colors = cx.sortier_theme().colors;
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(colors.scrim.hsla_with_alpha(0.5))
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(spacing::XS)
                .p(spacing::LG)
                .rounded(radius::LG)
                .border_2()
                .border_color(colors.accent.hsla())
                .bg(colors.surface_raised.hsla())
                .child(Label::new("Drop to add to the library").size(TextSize::Heading))
                .child(
                    Label::new("Folders are watched as a whole; a dropped file adds the folder it's in.")
                        .size(TextSize::Small)
                        .tone(TextTone::Secondary),
                ),
        )
        .into_any_element()
}

impl Render for SortierWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.layouts.read(cx).workspaces().active();
        let host = self.host_for(active, cx);
        if self.dropping && !cx.has_active_drag() {
            self.dropping = false;
        }
        // macOS has the menus in its own menu bar at the top of the screen.
        let top_bar = if cfg!(target_os = "macos") { TopBar::new() } else { TopBar::new().item(self.menu_bar.clone()) };
        let switcher = vault_switcher(&self.library, cx);
        let _ = window;
        div()
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::add_folder))
            .on_action(cx.listener(|this, _: &Rescan, _, cx| this.with_library(cx, |library, cx| library.rescan(cx))))
            .on_action(cx.listener(|this, action: &RemoveFolder, _, cx| {
                let path = action.path.clone();
                this.with_library(cx, |library, cx| library.remove_folder(&path, cx));
            }))
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.with_library(cx, |library, cx| library.undo(cx))))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.with_library(cx, |library, cx| library.redo(cx))))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.with_library(cx, |library, cx| library.select_all(cx))
            }))
            .on_action(cx.listener(|this, _: &SelectNone, _, cx| {
                this.with_library(cx, |library, cx| library.clear_selection(cx))
            }))
            .on_action(cx.listener(|this, _: &MakeSequence, _, cx| {
                this.with_library(cx, |library, cx| library.make_sequence(cx))
            }))
            .on_action(cx.listener(|this, _: &SplitSequence, _, cx| {
                this.with_library(cx, |library, cx| library.split_sequences(cx))
            }))
            .on_action(cx.listener(|this, _: &DeleteSelected, window, cx| {
                let ids = this.library.read(cx).selected_items();
                confirm_delete(&this.library, ids, window, cx);
            }))
            .on_action(cx.listener(|this, _: &MakeCollection, _, cx| {
                this.with_library(cx, |library, cx| library.make_collection(cx))
            }))
            .on_action(cx.listener(|this, _: &ToggleSequenceTiles, _, cx| {
                this.with_library(cx, |library, cx| library.update_settings(|s| s.group_view = !s.group_view, cx))
            }))
            .on_action(cx.listener(|this, _: &ToggleCrop, _, cx| {
                this.with_library(cx, |library, cx| library.update_settings(|s| s.tile_crop = !s.tile_crop, cx))
            }))
            .on_action(cx.listener(|this, _: &LargerTiles, _, cx| {
                this.with_library(cx, |library, cx| {
                    library.update_settings(|s| s.tile_size = (s.tile_size + 20.0).min(400.0), cx)
                })
            }))
            .on_action(cx.listener(|this, _: &SmallerTiles, _, cx| {
                this.with_library(cx, |library, cx| {
                    library.update_settings(|s| s.tile_size = (s.tile_size - 20.0).max(100.0), cx)
                })
            }))
            .on_action(cx.listener(|this, action: &SortBy, _, cx| {
                let key = action.key;
                this.with_library(cx, |library, cx| library.update_settings(|s| s.sort = key, cx))
            }))
            .on_action(cx.listener(|this, _: &ToggleSortDirection, _, cx| {
                this.with_library(cx, |library, cx| library.update_settings(|s| s.sort_desc = !s.sort_desc, cx))
            }))
            .on_action(cx.listener(|this, action: &FitBy, _, cx| {
                let fit: FitMode = action.fit;
                this.with_library(cx, |library, cx| library.set_fit(fit, cx))
            }))
            .on_action(cx.listener(|this, _: &ToggleSpread, _, cx| {
                this.with_library(cx, |library, cx| library.update_settings(|s| s.spread = !s.spread, cx))
            }))
            .on_action(cx.listener(|this, _: &ToggleReadingDirection, _, cx| {
                this.with_library(cx, |library, cx| library.update_settings(|s| s.rtl = !s.rtl, cx))
            }))
            .on_action(cx.listener(|this, _: &TogglePixelated, _, cx| {
                this.with_library(cx, |library, cx| library.update_settings(|s| s.pixelated = !s.pixelated, cx))
            }))
            .on_action(cx.listener(|this, _: &ToggleHardwareDecoding, _, cx| {
                this.with_library(cx, |library, cx| {
                    library.update_settings(|s| s.hardware_decoding = !s.hardware_decoding, cx)
                })
            }))
            .on_action(cx.listener(|_, _: &ToggleFullscreen, window, _| window.toggle_fullscreen()))
            .on_action(cx.listener(|this, _: &ShowShortcuts, _, cx| {
                this.shortcuts = !this.shortcuts;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &About, _, cx| {
                this.shortcuts = false;
                this.with_library(cx, |_, cx| cx.notify());
            }))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.shortcuts && event.keystroke.key == "escape" {
                    this.shortcuts = false;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_drag_move(cx.listener(|this, _: &gpui::DragMoveEvent<ExternalPaths>, _, cx| {
                if !this.dropping {
                    this.dropping = true;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(Self::files_dropped))
            .child(
                WindowFrame::new(TitleBar::new(switcher))
                    .top_bar(top_bar.item(self.tabs.clone()))
                    .body(
                        div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .child(div().flex_1().min_h_0().child(host))
                            .children(upload_strip(self.library.read(cx))),
                    ),
            )
            .when(self.dropping, |this| this.child(drop_overlay(cx)))
            .when(self.shortcuts, |this| this.child(shortcuts_overlay(cx)))
    }
}
