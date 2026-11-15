#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod area_host;
mod editors;
mod layouts;
mod menus;
mod model;
mod remote;
mod render_service;
mod shell;
mod workspace_tabs;

use std::path::PathBuf;

use gpui::{App, AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

use model::LibraryModel;
use render_service::RenderService;
use shell::SortierWindow;

fn main() {
    // Folders given on the command line are added to the library.
    let folders: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .map(|path| std::path::absolute(&path).unwrap_or(path))
        .collect();
    gpui_platform::application().with_assets(sortier_ui::Assets).run(move |cx: &mut App| {
        sortier_ui::init(cx);
        cx.set_global(editors::builtin_registry());
        cx.bind_keys(editors::key_bindings());
        cx.bind_keys(menus::key_bindings());
        cx.on_action(|_: &menus::HideApp, cx| cx.hide());
        cx.on_action(|_: &menus::HideOtherApps, cx| cx.hide_other_apps());
        cx.on_action(|_: &menus::ShowAllApps, cx| cx.unhide_other_apps());
        cx.on_action(|_: &menus::Quit, cx| cx.quit());
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let renderer = RenderService::start();
        let library = cx.new(|cx| LibraryModel::new(folders.clone(), cx));
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(720.), px(480.))),
            app_id: Some("sortier".into()),
            ..sortier_ui::title_bar::window_options()
        };
        let opened = cx.open_window(options, |window, cx| {
            let shell = cx.new(|cx| SortierWindow::new(library.clone(), renderer.clone(), window, cx));
            sortier_ui::window_root(shell, window, cx)
        });
        if let Err(error) = opened {
            eprintln!("Sortier could not open its window: {error:#}");
            cx.quit();
            return;
        }
        cx.on_app_quit({
            let library = library.clone();
            move |cx| {
                library.update(cx, |library, _| library.save_if_needed());
                async {}
            }
        })
        .detach();
        cx.activate(true);
    });
}
