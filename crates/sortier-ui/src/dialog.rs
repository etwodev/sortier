use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, AppContext as _, Entity, ParentElement, SharedString, Styled, Window, div};
use gpui_component::WindowExt as _;
use gpui_component::dialog::{DialogAction, DialogClose, DialogFooter};

use crate::button::{Button, ButtonVariant};
use crate::fields::Checkbox;
use crate::label::{Label, TextSize, TextTone};
use crate::text_field::{TextField, TextFieldState};
use crate::tokens::spacing;

/// Opens a modal message with a title, a body and a single dismiss button.
/// Escape and the button both close it.
pub fn open_message_dialog(
    title: impl Into<SharedString>,
    message: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) {
    let title = title.into();
    let message = message.into();
    window.open_alert_dialog(cx, move |alert, _, _| alert.title(title.clone()).description(message.clone()));
}

/// One text field of a form dialog (see [`open_form_dialog`]).
pub struct FormField {
    pub label: SharedString,
    pub value: SharedString,
    /// Shown as dots, for passwords.
    pub secret: bool,
}

impl FormField {
    pub fn new(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            secret: false,
        }
    }

    pub fn secret(mut self) -> Self {
        self.secret = true;
        self
    }
}

/// Opens a modal form: `title`, an optional `description`, labelled `fields`, and Cancel and `confirm` buttons.
/// Confirming (the button, or Enter) calls `on_confirm` with the fields' text in order, then closes the form unless
/// it returned `false` (to keep it open, say, while saying what's wrong). The first field starts focused with its
/// text selected.
pub fn open_form_dialog(
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    fields: Vec<FormField>,
    confirm: impl Into<SharedString>,
    on_confirm: impl Fn(Vec<String>, &mut Window, &mut App) -> bool + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let title = title.into();
    let confirm = confirm.into();
    let states: Vec<(SharedString, Entity<TextFieldState>)> = fields
        .into_iter()
        .map(|field| {
            let value = field.value.clone();
            let state = cx.new(|cx| TextFieldState::new(window, cx).masked(field.secret).default_value(value));
            (field.label, state)
        })
        .collect();
    if let Some((_, first)) = states.first() {
        first.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
    }
    let on_confirm = Rc::new(on_confirm);
    window.open_dialog(cx, move |dialog, _, _| {
        let values = states.clone();
        let on_confirm = on_confirm.clone();
        let mut body = div().flex().flex_col().gap(spacing::SM);
        if let Some(description) = &description {
            body = body.child(Label::new(description.clone()).size(TextSize::Small).tone(TextTone::Secondary));
        }
        for (label, state) in &states {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(spacing::XS)
                    .child(Label::new(label.clone()).size(TextSize::Small))
                    .child(TextField::new(state)),
            );
        }
        dialog
            .title(title.clone())
            .child(body)
            .footer(
                DialogFooter::new()
                    .child(DialogClose::new().child(Button::new("form-cancel", "Cancel")))
                    .child(DialogAction::new().child(Button::new("form-confirm", confirm.clone()).variant(ButtonVariant::Primary))),
            )
            .on_ok(move |_, window, cx| {
                let texts = values.iter().map(|(_, state)| state.read(cx).value().to_string()).collect();
                on_confirm(texts, window, cx)
            })
    });
}

/// One answer of a choice dialog (see [`open_choice_dialog`]): its button's text and look, and the value it gives.
pub struct Choice<T> {
    pub value: T,
    pub label: SharedString,
    pub variant: ButtonVariant,
}

impl<T> Choice<T> {
    pub fn new(value: T, label: impl Into<SharedString>, variant: ButtonVariant) -> Self {
        Self {
            value,
            label: label.into(),
            variant,
        }
    }
}

/// What a choice dialog asks (see [`open_choice_dialog`]).
pub struct ChoiceQuestion<T> {
    pub title: SharedString,
    pub description: SharedString,
    /// A button each, in order.
    pub choices: Vec<Choice<T>>,
    /// The label of a checkbox shown below the description, when there is one.
    pub check: Option<SharedString>,
    /// The answer when the question is closed without choosing (with Escape).
    pub dismissed: T,
}

/// Opens a modal question. Choosing an answer closes it and calls `on_choose` with the answer's value and whether
/// the box was ticked; closing it another way answers `question.dismissed`, with the box unticked. `on_choose` runs
/// once the question has gone, so it may ask another.
pub fn open_choice_dialog<T: Copy + 'static>(
    question: ChoiceQuestion<T>,
    on_choose: impl Fn(T, bool, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let ChoiceQuestion {
        title,
        description,
        choices,
        check,
        dismissed,
    } = question;
    let choices = Rc::new(choices);
    let on_choose = Rc::new(on_choose);
    let ticked = Rc::new(Cell::new(false));
    let answered = Rc::new(Cell::new(false));
    window.open_dialog(cx, move |dialog, _, _| {
        let mut body = div()
            .flex()
            .flex_col()
            .gap(spacing::MD)
            .child(Label::new(description.clone()).size(TextSize::Small).tone(TextTone::Secondary));
        if let Some(check) = &check {
            let ticking = ticked.clone();
            body = body.child(Checkbox::new("choice-check", ticked.get()).label(check.clone()).on_change(move |checked, window, _| {
                ticking.set(checked);
                window.refresh();
            }));
        }
        let mut footer = DialogFooter::new();
        for (index, choice) in choices.iter().enumerate() {
            let (value, on_choose, ticked, answered) = (choice.value, on_choose.clone(), ticked.clone(), answered.clone());
            footer = footer.child(Button::new(("choice", index), choice.label.clone()).variant(choice.variant).on_click(move |window, cx| {
                answered.set(true);
                window.close_dialog(cx);
                let (on_choose, ticked) = (on_choose.clone(), ticked.get());
                window.defer(cx, move |window, cx| on_choose(value, ticked, window, cx));
            }));
        }
        let (on_close, answered) = (on_choose.clone(), answered.clone());
        dialog
            .title(title.clone())
            .overlay_closable(false)
            .child(body)
            .footer(footer)
            .on_close(move |_, window, cx| {
                if !answered.replace(true) {
                    let on_close = on_close.clone();
                    window.defer(cx, move |window, cx| on_close(dismissed, false, window, cx));
                }
            })
    });
}
