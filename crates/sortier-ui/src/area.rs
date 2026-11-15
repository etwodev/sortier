use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, ElementId, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Point,
    RenderOnce, StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _, px,
};

use crate::menu::{MenuEntry, with_context_menu};
use crate::theme::ActiveSortierTheme;
use crate::tokens::{radius, sizes, spacing, to_px};

/// Direction of a seam or guide line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineOrientation {
    Vertical,
    Horizontal,
}

/// The background an area layout sits on: the seam colour, inset by one seam
/// width so the outermost areas are framed like the inner ones.
#[derive(IntoElement, Default)]
pub struct AreaLayoutSurface {
    children: Vec<AnyElement>,
}

impl AreaLayoutSurface {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }
}

impl RenderOnce for AreaLayoutSurface {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .size_full()
            .p(sizes::AREA_SEAM)
            .bg(cx.sortier_theme().colors.seam.hsla())
            .child(div().relative().size_full().children(self.children))
    }
}

/// One area of a workspace: its header above the editor body, drawn as a
/// rounded, outlined panel so the seam colour shows between neighbouring areas.
#[derive(IntoElement)]
pub struct AreaFrame {
    header: AreaHeader,
    body: Option<AnyElement>,
    highlighted: bool,
}

impl AreaFrame {
    pub fn new(header: AreaHeader) -> Self {
        Self {
            header,
            body: None,
            highlighted: false,
        }
    }

    /// Brightens the outline, marking the area as one a seam action affects.
    pub fn highlighted(mut self, highlighted: bool) -> Self {
        self.highlighted = highlighted;
        self
    }

    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }
}

impl RenderOnce for AreaFrame {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let border = if self.highlighted {
            colors.area_border_active
        } else {
            colors.area_border
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(radius::LG)
            .border_1()
            .border_color(border.hsla())
            .bg(colors.background.hsla())
            .child(self.header)
            .child(div().flex_1().min_h_0().overflow_hidden().children(self.body))
    }
}

/// The strip across the top of an area. Items are laid out from the left; the
/// first is conventionally the editor-type selector.
#[derive(IntoElement, Default)]
pub struct AreaHeader {
    items: Vec<AnyElement>,
}

impl AreaHeader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn item(mut self, item: impl IntoElement) -> Self {
        self.items.push(item.into_any_element());
        self
    }
}

impl RenderOnce for AreaHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        // No line under the header: it shares the surface colour with the
        // editor's toolbar below, so the two read as one soft band.
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(spacing::XS)
            .h(sizes::AREA_HEADER_HEIGHT)
            .px(spacing::SM)
            // Children are clipped to a rectangle, not to the frame's rounded
            // corners, so the header rounds its own top corners to match.
            .rounded_t(to_px(radius::LG, cx) - BORDER_WIDTH)
            .bg(colors.surface.hsla())
            .children(self.items)
    }
}

/// Width of the area outline drawn by `border_1`.
const BORDER_WIDTH: Pixels = px(1.);

type PressHandler = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App)>;
type HoverHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;
type MenuBuilder = Rc<dyn Fn(&mut Window, &mut App) -> Vec<MenuEntry>>;

/// An invisible handle over a seam, positioned absolutely inside its host.
/// Shows the resize cursor, reports left-button presses with the pointer
/// position, and opens a context menu on right click.
#[derive(IntoElement)]
pub struct SeamHandle {
    id: ElementId,
    orientation: LineOrientation,
    bounds: Bounds<Pixels>,
    on_press: Option<PressHandler>,
    on_hover: Option<HoverHandler>,
    menu: Option<MenuBuilder>,
}

impl SeamHandle {
    /// `bounds` is relative to the host and should already include any hit slop.
    pub fn new(id: impl Into<ElementId>, orientation: LineOrientation, bounds: Bounds<Pixels>) -> Self {
        Self {
            id: id.into(),
            orientation,
            bounds,
            on_press: None,
            on_hover: None,
            menu: None,
        }
    }

    /// Called with `true` when the pointer enters the handle and `false` when it leaves.
    pub fn on_hover(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Rc::new(handler));
        self
    }

    pub fn on_press(mut self, handler: impl Fn(Point<Pixels>, &mut Window, &mut App) + 'static) -> Self {
        self.on_press = Some(Rc::new(handler));
        self
    }

    pub fn context_menu(mut self, build: impl Fn(&mut Window, &mut App) -> Vec<MenuEntry> + 'static) -> Self {
        self.menu = Some(Rc::new(build));
        self
    }
}

impl RenderOnce for SeamHandle {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let on_press = self.on_press;
        let on_hover = self.on_hover;
        let handle = div()
            .id(self.id)
            .size_full()
            .map(|this| match self.orientation {
                LineOrientation::Vertical => this.cursor_col_resize(),
                LineOrientation::Horizontal => this.cursor_row_resize(),
            })
            .when_some(on_hover, |this, on_hover| {
                this.on_hover(move |hovered, window, cx| on_hover(*hovered, window, cx))
            })
            .when_some(on_press, |this, on_press| {
                this.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    cx.stop_propagation();
                    on_press(event.position, window, cx);
                })
            });
        let handle = match self.menu {
            Some(menu) => with_context_menu(handle, move |window, cx| menu(window, cx)),
            None => handle.into_any_element(),
        };
        // The context menu wrapper repositions its target, so placement lives on
        // an outer element.
        div()
            .absolute()
            .left(self.bounds.origin.x)
            .top(self.bounds.origin.y)
            .w(self.bounds.size.width)
            .h(self.bounds.size.height)
            .child(handle)
    }
}

/// The line previewing where an area will be split, positioned absolutely
/// inside its host.
#[derive(IntoElement)]
pub struct SplitGuide {
    bounds: Bounds<Pixels>,
}

impl SplitGuide {
    /// `bounds` is relative to the host.
    pub fn new(bounds: Bounds<Pixels>) -> Self {
        Self { bounds }
    }
}

impl RenderOnce for SplitGuide {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .absolute()
            .left(self.bounds.origin.x)
            .top(self.bounds.origin.y)
            .w(self.bounds.size.width)
            .h(self.bounds.size.height)
            .rounded_full()
            .bg(cx.sortier_theme().colors.accent.hsla())
    }
}
