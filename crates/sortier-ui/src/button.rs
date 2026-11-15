use std::rc::Rc;

use gpui::{App, ClickEvent, ElementId, IntoElement, RenderOnce, SharedString, Window};
use gpui_component::button::{Button as ComponentButton, ButtonRounded, ButtonVariants as _};
use gpui_component::{Disableable as _, Selectable as _, Sizable as _};

use crate::icon::IconKind;

type ClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// How prominent a [`Button`] is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    /// The main action in a group.
    Primary,
    /// A standard action.
    Secondary,
    /// Low emphasis: no background until hovered. Toolbars and inline actions.
    #[default]
    Ghost,
}

/// A text button, optionally with a leading icon.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    icon: Option<IconKind>,
    variant: ButtonVariant,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            variant: ButtonVariant::default(),
            disabled: false,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: IconKind) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut button = with_variant(ComponentButton::new(self.id), self.variant)
            .xsmall()
            .label(self.label)
            .disabled(self.disabled);
        if let Some(icon) = self.icon {
            button = button.icon(icon);
        }
        with_click(button, self.on_click)
    }
}

/// An icon-only button. `accessible_label` names it for screen readers and
/// is shown as its tooltip.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconKind,
    accessible_label: SharedString,
    variant: ButtonVariant,
    selected: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: IconKind, accessible_label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon,
            accessible_label: accessible_label.into(),
            variant: ButtonVariant::default(),
            selected: false,
            disabled: false,
            on_click: None,
        }
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Shows the button in its pressed state, for toggles.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let button = with_variant(ComponentButton::new(self.id), self.variant)
            .xsmall()
            .icon(self.icon)
            .tooltip(self.accessible_label.clone())
            .accessibility_label(self.accessible_label)
            .selected(self.selected)
            .disabled(self.disabled);
        with_click(button, self.on_click)
    }
}

/// Every button is a pill: twice the theme radius is past half a compact
/// button's height, so the ends come out round.
fn with_variant(button: ComponentButton, variant: ButtonVariant) -> ComponentButton {
    let button = button.rounded(ButtonRounded::Large);
    match variant {
        ButtonVariant::Primary => button.primary(),
        ButtonVariant::Secondary => button,
        ButtonVariant::Ghost => button.ghost(),
    }
}

fn with_click(button: ComponentButton, handler: Option<ClickHandler>) -> ComponentButton {
    match handler {
        Some(handler) => button.on_click(move |_: &ClickEvent, window, cx| {
            // Keep clicks on embedded buttons (such as a row's visibility
            // toggle) from also reaching the row.
            cx.stop_propagation();
            handler(window, cx);
        }),
        None => button,
    }
}
