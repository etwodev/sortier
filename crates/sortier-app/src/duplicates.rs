//! Asking what to do with uploads the server has a file at the same place for already.

use gpui::{App, Entity, Window};
use sortier_ui::button::ButtonVariant;
use sortier_ui::dialog::{Choice, ChoiceQuestion, open_choice_dialog};

use crate::model::{DuplicateChoice, LibraryModel};

/// Asks what to do with the first duplicate upload waiting in `library` (skip it, keep both, or replace the one on
/// the server), offering to do the same with the rest, and does it. Escape skips it.
pub fn ask_about_duplicate(library: &Entity<LibraryModel>, window: &mut Window, cx: &mut App) {
    let Some(question) = library.read(cx).duplicate_question() else { return };
    let title = format!("\u{201c}{}\u{201d} is on the server already", question.name);
    let place = if question.folder.is_empty() {
        "the upload folder".to_string()
    } else {
        format!("{} in the upload folder", question.folder)
    };
    let description = format!(
        "{place} has a file with this name. Replace it with the one being uploaded, keep both (the upload gets a new name), or skip it?"
    );
    let check = match question.others {
        0 => None,
        1 => Some("Do the same with the other file that's already there".into()),
        others => Some(format!("Do the same with the other {others} files that are already there").into()),
    };
    let question = ChoiceQuestion {
        title: title.into(),
        description: description.into(),
        choices: vec![
            Choice::new(DuplicateChoice::Skip, "Skip", ButtonVariant::Secondary),
            Choice::new(DuplicateChoice::KeepBoth, "Keep Both", ButtonVariant::Secondary),
            Choice::new(DuplicateChoice::Replace, "Replace", ButtonVariant::Primary),
        ],
        check,
        dismissed: DuplicateChoice::Skip,
    };
    let library = library.clone();
    open_choice_dialog(
        question,
        move |choice, for_all, _, cx| library.update(cx, |library, cx| library.answer_duplicate(choice, for_all, cx)),
        window,
        cx,
    );
}
