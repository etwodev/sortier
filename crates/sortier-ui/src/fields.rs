use gpui::{
    AnyElement, App, AppContext as _, ElementId, Entity, IntoElement, ParentElement, RenderOnce, SharedString,
    Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::button::{Button, ButtonGroup};
use gpui_component::menu::DropdownMenu as _;
use gpui_component::{Disableable as _, Selectable as _, Sizable as _};

use crate::icon::{Icon, IconKind, IconSize};
use crate::label::{Label, TextSize, TextTone};
use crate::menu::MenuEntry;
use crate::tokens::{sizes, spacing};

type IndexHandler = std::rc::Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A labelled row in a property list: the label in a fixed-width column and
/// the control filling the rest.
#[derive(IntoElement)]
pub struct PropertyRow {
    label: SharedString,
    control: AnyElement,
    disabled: bool,
}

impl PropertyRow {
    pub fn new(label: impl Into<SharedString>, control: impl IntoElement) -> Self {
        Self {
            label: label.into(),
            control: control.into_any_element(),
            disabled: false,
        }
    }

    /// Greys out the label; disable the control itself too.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for PropertyRow {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let tone = if self.disabled { TextTone::Disabled } else { TextTone::Secondary };
        div()
            .flex()
            .items_center()
            .gap(spacing::SM)
            .min_h(sizes::TREE_ROW_HEIGHT)
            .child(
                div()
                    .flex_none()
                    .w(sizes::PROPERTY_LABEL_WIDTH)
                    .child(Label::new(self.label).size(TextSize::Small).tone(tone)),
            )
            .child(div().flex_1().min_w_0().child(self.control))
    }
}

/// A row of mutually exclusive choices shown side by side, such as BW / RGB
/// / RGBA. `on_select` gets the chosen index.
#[derive(IntoElement)]
pub struct SegmentedControl {
    id: ElementId,
    options: Vec<SharedString>,
    selected: usize,
    disabled: bool,
    on_select: Option<IndexHandler>,
}

impl SegmentedControl {
    pub fn new(id: impl Into<ElementId>, options: impl IntoIterator<Item = impl Into<SharedString>>, selected: usize) -> Self {
        Self {
            id: id.into(),
            options: options.into_iter().map(Into::into).collect(),
            selected,
            disabled: false,
            on_select: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for SegmentedControl {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let id = self.id.clone();
        let mut group = ButtonGroup::new(self.id).outline().xsmall().w_full().disabled(self.disabled);
        for (index, option) in self.options.into_iter().enumerate() {
            group = group.child(
                Button::new(ElementId::NamedChild(std::sync::Arc::new(id.clone()), index.to_string().into()))
                    .label(option)
                    .flex_1()
                    .selected(index == self.selected),
            );
        }
        let handler = self.on_select;
        group.on_click(move |clicked, window, cx| {
            if let (Some(handler), Some(index)) = (&handler, clicked.first()) {
                handler(*index, window, cx);
            }
        })
    }
}

/// A collapsible group of rows inside a [`PropertyPanel`], like Blender's
/// subpanels (Time Stretching, Color Management).
#[derive(IntoElement)]
pub struct PropertySection {
    id: ElementId,
    title: SharedString,
    collapsed: bool,
    rows: Vec<AnyElement>,
    on_toggle: Option<PanelToggleHandler>,
}

impl PropertySection {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            collapsed: false,
            rows: Vec::new(),
            on_toggle: None,
        }
    }

    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    pub fn row(mut self, row: impl IntoElement) -> Self {
        self.rows.push(row.into_any_element());
        self
    }

    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for PropertySection {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        use gpui::{InteractiveElement as _, StatefulInteractiveElement as _};

        let colors = crate::theme::ActiveSortierTheme::sortier_theme(cx).colors;
        let chevron = if self.collapsed { IconKind::ChevronRight } else { IconKind::ChevronDown };
        let on_toggle = self.on_toggle;
        div()
            .flex()
            .flex_col()
            .gap(spacing::XS)
            .child(
                div()
                    .id(self.id)
                    .flex()
                    .items_center()
                    .gap(spacing::XS)
                    .h(sizes::TREE_ROW_HEIGHT)
                    .px(spacing::XS)
                    .rounded_full()
                    .text_color(colors.accent_hover.hsla())
                    .hover(|style| style.bg(colors.hover.hsla()))
                    .on_click(move |_, window, cx| {
                        if let Some(handler) = &on_toggle {
                            handler(window, cx);
                        }
                    })
                    .child(Icon::new(chevron).size(IconSize::Small))
                    .child(Label::new(self.title).size(TextSize::Small)),
            )
            .when(!self.collapsed, |this| this.child(div().flex().flex_col().gap(spacing::XS).pl(spacing::LG).children(self.rows)))
    }
}

/// A vertical stack of [`PropertyRow`]s with consistent spacing and padding.
#[derive(IntoElement, Default)]
pub struct PropertyList {
    rows: Vec<AnyElement>,
}

impl PropertyList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn row(mut self, row: impl IntoElement) -> Self {
        self.rows.push(row.into_any_element());
        self
    }
}

impl RenderOnce for PropertyList {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().flex().flex_col().gap(spacing::XS).p(spacing::SM).children(self.rows)
    }
}

/// A read-only value shown in place of a control.
#[derive(IntoElement)]
pub struct ValueText {
    text: SharedString,
}

impl ValueText {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for ValueText {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        Label::new(self.text).size(TextSize::Small)
    }
}

/// A field that shows the current choice and opens a menu of alternatives.
#[derive(IntoElement)]
pub struct SelectField {
    id: ElementId,
    value: SharedString,
    entries: Vec<MenuEntry>,
    disabled: bool,
}

impl SelectField {
    pub fn new(id: impl Into<ElementId>, value: impl Into<SharedString>, entries: Vec<MenuEntry>) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            entries,
            disabled: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for SelectField {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let entries = self.entries;
        Button::new(self.id)
            .outline()
            .xsmall()
            .rounded(gpui_component::button::ButtonRounded::Large)
            .w_full()
            .label(self.value)
            .dropdown_caret(true)
            .disabled(self.disabled)
            .dropdown_menu(move |menu, window, cx| crate::menu::build_popup(menu, entries.clone(), window, cx))
    }
}

/// The state behind a [`Slider`]: its range and value. Subscribe for
/// [`SliderEvent`]s.
pub type SliderState = gpui_component::slider::SliderState;
pub use gpui_component::slider::{SliderEvent, SliderValue};

/// A horizontal slider from `min` to `max` in increments of `step`.
pub fn slider_state(min: f32, max: f32, step: f32, value: f32, cx: &mut App) -> Entity<SliderState> {
    // gpui-component clamps against the current bounds as each is set, so
    // the maximum goes first: a minimum above the default maximum would panic.
    cx.new(|_| SliderState::new().max(max).min(min).step(step).default_value(value.clamp(min, max)))
}

#[derive(IntoElement)]
pub struct Slider {
    state: Entity<SliderState>,
    disabled: bool,
}

impl Slider {
    pub fn new(state: &Entity<SliderState>) -> Self {
        Self {
            state: state.clone(),
            disabled: false,
        }
    }
}

impl Slider {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for Slider {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        gpui_component::slider::Slider::new(&self.state)
            .horizontal()
            .disabled(self.disabled)
    }
}

/// A compact labelled slider with its value beside it, for toolbars.
#[derive(IntoElement)]
pub struct ToolbarSlider {
    label: SharedString,
    state: Entity<SliderState>,
    value: SharedString,
}

impl ToolbarSlider {
    pub fn new(label: impl Into<SharedString>, state: &Entity<SliderState>, value: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            state: state.clone(),
            value: value.into(),
        }
    }
}

impl RenderOnce for ToolbarSlider {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(spacing::XS)
            .child(Label::new(self.label).size(TextSize::Small).tone(TextTone::Secondary))
            .child(div().w(sizes::TOOLBAR_SLIDER_WIDTH).child(Slider::new(&self.state)))
            .child(ValueText::new(self.value))
    }
}

/// A thin line between neighbouring panes: upright between side-by-side
/// panes, flat between stacked ones.
#[derive(IntoElement)]
pub struct PaneDivider {
    horizontal: bool,
}

impl PaneDivider {
    pub fn vertical() -> Self {
        Self { horizontal: false }
    }

    pub fn horizontal() -> Self {
        Self { horizontal: true }
    }
}

impl RenderOnce for PaneDivider {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = crate::theme::ActiveSortierTheme::sortier_theme(cx).colors;
        let line = div().flex_none().bg(colors.border.hsla());
        if self.horizontal { line.h(gpui::px(1.)).w_full() } else { line.w(gpui::px(1.)).h_full() }
    }
}

/// A horizontal strip of controls along the top of an editor body.
#[derive(IntoElement, Default)]
pub struct Toolbar {
    items: Vec<AnyElement>,
}

impl Toolbar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn item(mut self, item: impl IntoElement) -> Self {
        self.items.push(item.into_any_element());
        self
    }

    /// Pushes the items after it to the far end.
    pub fn spacer(mut self) -> Self {
        self.items.push(div().flex_1().into_any_element());
        self
    }

    /// A small dot between groups of items.
    pub fn divider(mut self, cx: &App) -> Self {
        let colors = crate::theme::ActiveSortierTheme::sortier_theme(cx).colors;
        self.items.push(
            div()
                .flex_none()
                .size(spacing::XS)
                .mx(spacing::XS)
                .rounded_full()
                .bg(colors.border_strong.hsla())
                .into_any_element(),
        );
        self
    }
}

impl RenderOnce for Toolbar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = crate::theme::ActiveSortierTheme::sortier_theme(cx).colors;
        // Continues the area header's band, so the editor's own controls sit
        // with its title instead of under a hard rule.
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(spacing::XS)
            .px(spacing::SM)
            .pb(spacing::XS)
            .h(sizes::TOOLBAR_HEIGHT)
            .bg(colors.surface.hsla())
            .children(self.items)
    }
}

type ToggleHandler = std::rc::Rc<dyn Fn(bool, &mut Window, &mut App)>;
type PanelToggleHandler = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// A checkbox with an optional label to its right.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: bool,
    disabled: bool,
    label: Option<SharedString>,
    on_change: Option<ToggleHandler>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        Self {
            id: id.into(),
            checked,
            disabled: false,
            label: None,
            on_change: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Called with the new checked state.
    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut checkbox = gpui_component::checkbox::Checkbox::new(self.id)
            .checked(self.checked)
            .disabled(self.disabled)
            .xsmall();
        if let Some(label) = self.label {
            checkbox = checkbox.label(label);
        }
        if let Some(handler) = self.on_change {
            checkbox = checkbox.on_change(move |checked, window, cx| handler(*checked, window, cx));
        }
        checkbox
    }
}

/// A titled, collapsible group of property rows drawn as a rounded panel,
/// like the panels in Blender's Properties editor.
#[derive(IntoElement)]
pub struct PropertyPanel {
    id: ElementId,
    title: SharedString,
    collapsed: bool,
    rows: Vec<AnyElement>,
    on_toggle: Option<PanelToggleHandler>,
}

impl PropertyPanel {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            collapsed: false,
            rows: Vec::new(),
            on_toggle: None,
        }
    }

    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    pub fn row(mut self, row: impl IntoElement) -> Self {
        self.rows.push(row.into_any_element());
        self
    }

    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for PropertyPanel {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        use gpui::{InteractiveElement as _, StatefulInteractiveElement as _};

        let colors = crate::theme::ActiveSortierTheme::sortier_theme(cx).colors;
        let chevron = if self.collapsed {
            crate::icon::IconKind::ChevronRight
        } else {
            crate::icon::IconKind::ChevronDown
        };
        let on_toggle = self.on_toggle;
        let header = div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(spacing::XS)
            .h(sizes::TREE_ROW_HEIGHT)
            .px(spacing::SM)
            .rounded(crate::tokens::radius::LG)
            .text_color(colors.accent_hover.hsla())
            .hover(|style| style.bg(colors.hover.hsla()))
            .on_click(move |_, window, cx| {
                if let Some(handler) = &on_toggle {
                    handler(window, cx);
                }
            })
            .child(crate::icon::Icon::new(chevron).size(crate::icon::IconSize::Small))
            .child(Label::new(self.title).size(TextSize::Body));
        div()
            .flex()
            .flex_col()
            .rounded(crate::tokens::radius::LG)
            .bg(colors.surface.hsla())
            .overflow_hidden()
            .child(header)
            .when(!self.collapsed, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(spacing::XS)
                        .px(spacing::MD)
                        .pb(spacing::MD)
                        .children(self.rows),
                )
            })
    }
}
