use gpui::{App, AppContext as _, Entity, IntoElement, RenderOnce, SharedString, Styled, Window};
use gpui_component::Sizable as _;

/// The editable state behind a [`TextField`]: its text, cursor and selection.
/// Subscribe to it for [`TextFieldEvent`]s.
pub type TextFieldState = gpui_component::input::InputState;

/// Events a [`TextFieldState`] emits, such as `PressEnter` and `Blur`.
pub use gpui_component::input::InputEvent as TextFieldEvent;

/// Creates field state holding `text`. With `focus_and_select`, the field takes
/// keyboard focus with all of its text selected, ready to be typed over.
pub fn text_field_state(
    text: impl Into<SharedString>,
    focus_and_select: bool,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TextFieldState> {
    let text = text.into();
    let state = cx.new(|cx| TextFieldState::new(window, cx).default_value(text));
    if focus_and_select {
        state.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
    }
    state
}

/// A compact single-line text input that fills the width it's given.
#[derive(IntoElement)]
pub struct TextField {
    state: Entity<TextFieldState>,
    disabled: bool,
}

impl TextField {
    pub fn new(state: &Entity<TextFieldState>) -> Self {
        Self {
            state: state.clone(),
            disabled: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for TextField {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        gpui_component::input::Input::new(&self.state)
            .xsmall()
            .w_full()
            .rounded_full()
            .disabled(self.disabled)
    }
}

/// The editable state behind a [`TextArea`]. Subscribe to it for
/// [`TextFieldEvent`]s; Enter adds a line, so commit on `Blur`.
pub type TextAreaState = gpui_component::input::TextareaState;

/// Creates text area state holding `text`, growing from `min_rows` to
/// `max_rows` lines as it fills.
pub fn text_area_state(
    text: impl Into<SharedString>,
    min_rows: usize,
    max_rows: usize,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TextAreaState> {
    let text = text.into();
    cx.new(|cx| TextAreaState::new(window, cx).auto_grow(min_rows, max_rows).default_value(text))
}

/// A compact multi-line text input that fills the width it's given.
#[derive(IntoElement)]
pub struct TextArea {
    state: Entity<TextAreaState>,
}

impl TextArea {
    pub fn new(state: &Entity<TextAreaState>) -> Self {
        Self { state: state.clone() }
    }
}

impl RenderOnce for TextArea {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        gpui_component::input::Textarea::new(&self.state).xsmall().w_full()
    }
}
