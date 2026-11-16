//! Asking before files are deleted.

use gpui::{App, Entity, Window};
use sortier_library::library::ItemId;
use sortier_ui::dialog::open_form_dialog;

use crate::model::LibraryModel;

/// The key that deletes what's selected or shown: Cmd-Backspace on macOS, as in Finder, and Delete elsewhere.
pub const DELETE_KEY: &str = if cfg!(target_os = "macos") { "cmd-backspace" } else { "delete" };

/// Asks whether to erase the files `ids` for good, naming them, and erases them if so.
pub fn confirm_delete(library: &Entity<LibraryModel>, ids: Vec<ItemId>, window: &mut Window, cx: &mut App) {
    if ids.is_empty() {
        return;
    }
    let (title, remote) = {
        let model = library.read(cx);
        let title = match ids.as_slice() {
            [id] => format!("Delete \u{201c}{}\u{201d}?", model.item(*id).map(|item| item.file_name()).unwrap_or_default()),
            _ => format!("Delete {} files?", ids.len()),
        };
        (title, model.remote_access().is_some())
    };
    let place = if remote { "the server's disk" } else { "this computer's disk" };
    let them = if ids.len() == 1 { "It's" } else { "They're" };
    let description = format!("{them} erased from {place} for good, not moved to the Trash. This can't be undone.");
    let library = library.clone();
    open_form_dialog(
        title,
        Some(description.into()),
        Vec::new(),
        "Delete",
        move |_, _, cx| {
            let ids = ids.clone();
            library.update(cx, |library, cx| library.delete_items(ids, cx));
            true
        },
        window,
        cx,
    );
}
