use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Bounds, BoxShadow, ClickEvent, Context, Corners, ElementId, Entity, Hsla, ImageCache, ImageCacheError,
    ImageCacheItem, InteractiveElement as _, IntoElement, ObjectFit, ParentElement, Pixels, Render, RenderImage,
    RenderOnce, Resource, SharedString, StatefulInteractiveElement as _, Styled, StyledImage as _, Window, div, hash,
    img, point, px,
};

use crate::icon::{Icon, IconKind, IconSize};
use crate::label::{Label, TextSize, TextTone};
use crate::theme::ActiveSortierTheme;
use crate::tokens::{radius, sizes, spacing, to_px};

/// Keeps the most recently drawn thumbnails decoded and drops the rest from
/// every window's atlas, so scrolling a large library doesn't hold every
/// thumbnail in GPU memory. Create one per editor with [`ThumbnailCache::new`]
/// and pass it to [`thumbnail`].
pub struct ThumbnailCache {
    capacity: usize,
    tick: u64,
    items: HashMap<u64, (u64, ImageCacheItem)>,
}

impl ThumbnailCache {
    pub fn new(capacity: usize, cx: &mut App) -> Entity<Self> {
        let cache = cx.new(|_| Self {
            capacity: capacity.max(1),
            tick: 0,
            items: HashMap::new(),
        });
        cx.observe_release(&cache, |cache, cx| {
            for (_, (_, item)) in std::mem::take(&mut cache.items) {
                if let Some(Ok(image)) = item.get() {
                    cx.drop_image(image, None);
                }
            }
        })
        .detach();
        cache
    }
}

impl ImageCache for ThumbnailCache {
    fn load(&mut self, resource: &Resource, window: &mut Window, cx: &mut App) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        self.tick += 1;
        let key = hash(resource);
        if let Some((used, item)) = self.items.get_mut(&key) {
            *used = self.tick;
            return item.use_image(window);
        }
        while self.items.len() >= self.capacity {
            let Some(oldest) = self.items.iter().min_by_key(|(_, (used, _))| *used).map(|(key, _)| *key) else {
                break;
            };
            if let Some((_, item)) = self.items.remove(&oldest)
                && let Some(Ok(image)) = item.get()
            {
                cx.drop_image(image, Some(window));
            }
        }
        let item = ImageCacheItem::new(resource, cx);
        let result = item.use_image(window);
        self.items.insert(key, (self.tick, item));
        result
    }
}

/// A cached thumbnail file filling its parent: cropped to fill when `crop`,
/// otherwise letterboxed. Its corners are rounded like the tiles and pages
/// it sits in, since a parent's `overflow_hidden` clips only to a rectangle.
pub fn thumbnail(path: PathBuf, crop: bool, cache: &Entity<ThumbnailCache>) -> AnyElement {
    img(path)
        .image_cache(cache)
        .size_full()
        .rounded(radius::LG)
        .object_fit(if crop { ObjectFit::Cover } else { ObjectFit::Contain })
        .into_any_element()
}

/// A small label over a picture: a count, a kind or a position.
#[derive(IntoElement)]
pub struct Badge {
    icon: Option<IconKind>,
    text: Option<SharedString>,
}

impl Badge {
    pub fn icon(icon: IconKind) -> Self {
        Self { icon: Some(icon), text: None }
    }

    pub fn text(text: impl Into<SharedString>) -> Self {
        Self {
            icon: None,
            text: Some(text.into()),
        }
    }

    pub fn icon_text(icon: IconKind, text: impl Into<SharedString>) -> Self {
        Self {
            icon: Some(icon),
            text: Some(text.into()),
        }
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        div()
            .flex()
            .items_center()
            .gap(spacing::XXS)
            .h(sizes::BADGE_HEIGHT)
            .px(spacing::XS + spacing::XXS)
            .rounded_full()
            .bg(colors.surface_raised.hsla_with_alpha(0.88))
            .text_color(colors.text_primary.hsla())
            .when_some(self.icon, |badge, icon| badge.child(Icon::new(icon).size(IconSize::Small)))
            .when_some(self.text, |badge, text| badge.child(Label::new(text).size(TextSize::Small)))
    }
}

/// What a tile shows while it has no picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TilePlaceholder {
    Loading,
    /// The thumbnail couldn't be made; the icon says what kind of file it is.
    Unavailable(IconKind),
}

type TileClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// One result in a library grid: a square picture, badges over its top-left
/// corner, and the name underneath. `side` is the picture's side at 100% UI scale.
#[derive(IntoElement)]
pub struct MediaTile {
    id: ElementId,
    side: f32,
    picture: Option<AnyElement>,
    placeholder: TilePlaceholder,
    title: SharedString,
    badges: Vec<Badge>,
    selected: bool,
    cursor: bool,
    on_click: Option<TileClick>,
}

impl MediaTile {
    pub fn new(id: impl Into<ElementId>, side: f32, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            side,
            picture: None,
            placeholder: TilePlaceholder::Loading,
            title: title.into(),
            badges: Vec::new(),
            selected: false,
            cursor: false,
            on_click: None,
        }
    }

    /// The picture, usually a [`thumbnail`].
    pub fn picture(mut self, picture: AnyElement) -> Self {
        self.picture = Some(picture);
        self
    }

    pub fn placeholder(mut self, placeholder: TilePlaceholder) -> Self {
        self.placeholder = placeholder;
        self
    }

    pub fn badge(mut self, badge: Badge) -> Self {
        self.badges.push(badge);
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Marks the tile the keyboard moves from.
    pub fn cursor(mut self, cursor: bool) -> Self {
        self.cursor = cursor;
        self
    }

    /// Called on every click, with the event's modifiers and click count.
    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// The tile's height for a picture of `side`, including its name, at the
    /// current UI scale. Rows of a grid are this tall.
    pub fn height(side: f32, cx: &App) -> Pixels {
        to_px(scaled(side), cx) + to_px(spacing::XS, cx) + to_px(sizes::TILE_LABEL_HEIGHT, cx)
    }

    /// The width a tile takes at the current UI scale.
    pub fn width(side: f32, cx: &App) -> Pixels {
        to_px(scaled(side), cx)
    }
}

fn scaled(pixels: f32) -> gpui::Rems {
    gpui::rems(pixels / crate::tokens::BASE_FONT)
}

impl RenderOnce for MediaTile {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let side = scaled(self.side);
        let border: Hsla = if self.selected {
            colors.accent.hsla()
        } else if self.cursor {
            colors.text_secondary.hsla()
        } else {
            gpui::transparent_black()
        };
        let hover_border = colors.border_strong.hsla();
        let glow = selection_glow(self.selected, cx);
        let picture = self.picture.unwrap_or_else(|| {
            let icon = match self.placeholder {
                TilePlaceholder::Loading => None,
                TilePlaceholder::Unavailable(icon) => Some(icon),
            };
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.text_disabled.hsla())
                .when_some(icon, |slot, icon| slot.child(Icon::new(icon).size(IconSize::Large)))
                .into_any_element()
        });
        let on_click = self.on_click;
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .gap(spacing::XS)
            .w(side)
            .group("media-tile")
            .when_some(on_click, |tile, on_click| tile.on_click(move |event, window, cx| on_click(event, window, cx)))
            .child(
                div()
                    .relative()
                    .size(side)
                    .rounded(radius::LG)
                    .shadow(glow)
                    .overflow_hidden()
                    .bg(colors.surface.hsla())
                    .child(picture)
                    .child(
                        div()
                            .absolute()
                            .top(spacing::SM)
                            .left(spacing::SM)
                            .flex()
                            .gap(spacing::XXS)
                            .children(self.badges),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(radius::LG)
                            .border_2()
                            .border_color(border)
                            .when(!self.selected && !self.cursor, |frame| {
                                frame.group_hover("media-tile", move |style| style.border_color(hover_border))
                            }),
                    ),
            )
            .child(
                div()
                    .h(sizes::TILE_LABEL_HEIGHT)
                    .w(side)
                    .overflow_hidden()
                    .child(
                        Label::new(self.title)
                            .size(TextSize::Small)
                            .tone(if self.selected { TextTone::Primary } else { TextTone::Secondary })
                            .truncate(),
                    ),
            )
    }
}

/// The value carried while a page is dragged in a page strip. `key` is the
/// caller's identifier for the page.
#[derive(Clone, Debug)]
pub struct PageDrag {
    pub key: u64,
    label: SharedString,
}

/// What follows the pointer while a page is dragged.
pub struct PageDragPreview {
    label: SharedString,
}

impl Render for PageDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        div()
            .px(spacing::SM)
            .py(spacing::XXS)
            .rounded_full()
            .bg(colors.surface_raised.hsla())
            .border_1()
            .border_color(colors.border_strong.hsla())
            .child(Label::new(self.label.clone()).size(TextSize::Small))
    }
}

type PageHandler = Rc<dyn Fn(&mut Window, &mut App)>;
type PageDropHandler = Rc<dyn Fn(&PageDrag, &mut Window, &mut App)>;

/// One page in a page strip: its picture and number. Click shows it; dragging
/// it onto another page reports a drop there.
#[derive(IntoElement)]
pub struct PageThumb {
    id: ElementId,
    key: u64,
    number: usize,
    picture: Option<AnyElement>,
    current: bool,
    on_select: Option<PageHandler>,
    on_drop: Option<PageDropHandler>,
}

impl PageThumb {
    /// `number` is the page's 1-based position; `key` comes back in [`PageDrag`].
    pub fn new(id: impl Into<ElementId>, key: u64, number: usize) -> Self {
        Self {
            id: id.into(),
            key,
            number,
            picture: None,
            current: false,
            on_select: None,
            on_drop: None,
        }
    }

    pub fn picture(mut self, picture: AnyElement) -> Self {
        self.picture = Some(picture);
        self
    }

    pub fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    pub fn on_drop(mut self, handler: impl Fn(&PageDrag, &mut Window, &mut App) + 'static) -> Self {
        self.on_drop = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for PageThumb {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let drag = PageDrag {
            key: self.key,
            label: format!("Page {}", self.number).into(),
        };
        let (on_select, on_drop) = (self.on_select, self.on_drop);
        let drop_highlight = colors.accent.hsla();
        div()
            .id(self.id)
            .relative()
            .flex_none()
            .w(sizes::PAGE_THUMB_WIDTH)
            .h(sizes::PAGE_THUMB_HEIGHT)
            .rounded(radius::LG)
            .shadow(selection_glow(self.current, cx))
            .overflow_hidden()
            .bg(colors.surface.hsla())
            .children(self.picture.map(|picture| div().size_full().when(!self.current, |page| page.opacity(0.55)).child(picture)))
            .child(
                div()
                    .absolute()
                    .bottom(spacing::XS)
                    .right(spacing::XS)
                    .child(Badge::text(self.number.to_string())),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(radius::LG)
                    .border_2()
                    .border_color(if self.current { colors.accent.hsla() } else { gpui::transparent_black() }),
            )
            .on_click(move |_, window, cx| {
                if let Some(on_select) = &on_select {
                    on_select(window, cx);
                }
            })
            .on_drag(drag, |drag, _, _, cx| {
                cx.new(|_| PageDragPreview {
                    label: drag.label.clone(),
                })
            })
            .drag_over::<PageDrag>(move |style, _, _, _| style.border_l_2().border_color(drop_highlight))
            .when_some(on_drop, |thumb, on_drop| thumb.on_drop(move |drag: &PageDrag, window, cx| on_drop(drag, window, cx)))
    }
}

/// A short note in the top-left corner of a canvas, such as the page being
/// shown. Place it inside a relatively positioned container.
#[derive(IntoElement)]
pub struct CanvasCaption {
    text: SharedString,
}

impl CanvasCaption {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for CanvasCaption {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        div()
            .absolute()
            .top(spacing::SM)
            .left(spacing::SM)
            .px(spacing::SM)
            .py(spacing::XXS)
            .rounded_full()
            .bg(colors.surface.hsla_with_alpha(0.8))
            .child(Label::new(self.text).size(TextSize::Small).tone(TextTone::Secondary))
    }
}

/// The soft accent glow around a selected picture, or none.
fn selection_glow(selected: bool, cx: &App) -> Vec<BoxShadow> {
    if !selected {
        return Vec::new();
    }
    vec![BoxShadow {
        color: cx.sortier_theme().colors.accent.hsla_with_alpha(0.45),
        offset: point(px(0.), px(0.)),
        blur_radius: to_px(sizes::SELECTION_GLOW, cx),
        spread_radius: px(0.),
        inset: false,
    }]
}

/// The rounded corners of a viewer's picture area. Paint the picture with
/// these so it matches [`paint_backdrop`].
pub fn canvas_corners(cx: &App) -> Corners<Pixels> {
    Corners::all(to_px(radius::MD, cx))
}

/// Fills `bounds` with the backdrop colour shown around pictures in a viewer,
/// rounded like [`canvas_corners`].
pub fn paint_backdrop(bounds: Bounds<Pixels>, window: &mut Window, cx: &App) {
    window.paint_quad(gpui::fill(bounds, cx.sortier_theme().colors.canvas_backdrop.hsla()).corner_radii(canvas_corners(cx)));
}

/// The colours viewers draw behind transparent pixels, as sRGB 0..1 channels:
/// (light, dark).
pub fn checker_colors(cx: &App) -> ([f32; 3], [f32; 3]) {
    let colors = cx.sortier_theme().colors;
    (colors.checker_light.rgb_f32(), colors.checker_dark.rgb_f32())
}

/// The viewer backdrop colour as sRGB 0..1 channels, for the renderer.
pub fn backdrop_color(cx: &App) -> [f32; 3] {
    cx.sortier_theme().colors.canvas_backdrop.rgb_f32()
}

/// Side of one checkerboard square in device pixels.
pub fn checker_size(window: &Window, cx: &App) -> f32 {
    f32::from(to_px(sizes::CHECKER, cx)) * window.scale_factor()
}

/// A tag's colour in lists and chips: one per namespace.
pub fn tag_color(tag: &str, cx: &App) -> Hsla {
    let colors = cx.sortier_theme().colors;
    match tag.split_once(':').map(|(namespace, _)| namespace) {
        Some("artist" | "by" | "creator") => colors.curves[3].hsla(),
        Some("series" | "copyright" | "parody") => colors.curves[4].hsla(),
        Some("character" | "char") => colors.curves[1].hsla(),
        Some("meta" | "source") => colors.text_secondary.hsla(),
        Some(_) => colors.curves[5].hsla(),
        None => colors.accent_hover.hsla(),
    }
}

type PillHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// A tag as a small pill: its colour dot, its name, an optional count, and
/// optionally a click action (apply or complete it) and a remove button.
#[derive(IntoElement)]
pub struct TagPill {
    id: ElementId,
    tag: SharedString,
    count: Option<SharedString>,
    on_click: Option<PillHandler>,
    on_remove: Option<PillHandler>,
}

impl TagPill {
    pub fn new(id: impl Into<ElementId>, tag: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            tag: tag.into(),
            count: None,
            on_click: None,
            on_remove: None,
        }
    }

    /// A note after the name, such as "3/5" for a tag on some of a selection.
    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    pub fn on_remove(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_remove = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for TagPill {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let dot = tag_dot(&self.tag, cx);
        let tint = tag_color(&self.tag, cx);
        let (on_click, on_remove) = (self.on_click, self.on_remove);
        let clickable = on_click.is_some();
        let remove_id = ElementId::from((self.id.clone(), "remove"));
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(spacing::XS)
            .h(sizes::TAB_HEIGHT)
            .pl(spacing::SM)
            .pr(if on_remove.is_some() { spacing::XS } else { spacing::SM })
            .rounded_full()
            .bg(Hsla { a: 0.16, ..tint })
            .when(clickable, |pill| pill.hover(move |style| style.bg(Hsla { a: 0.3, ..tint })))
            .when_some(on_click, |pill, on_click| pill.on_click(move |_, window, cx| on_click(window, cx)))
            .child(dot)
            .child(Label::new(self.tag).size(TextSize::Small))
            .when_some(self.count, |pill, count| pill.child(Label::new(count).size(TextSize::Small).tone(TextTone::Secondary)))
            .when_some(on_remove, |pill, on_remove| {
                pill.child(
                    div()
                        .id(remove_id)
                        .flex_none()
                        .p(spacing::XXS)
                        .rounded_full()
                        .text_color(colors.text_secondary.hsla())
                        .hover(|style| style.bg(colors.hover.hsla()).text_color(colors.text_primary.hsla()))
                        .child(Icon::new(IconKind::Close).size(IconSize::Small))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_remove(window, cx);
                        }),
                )
            })
    }
}

/// Pills in rows that wrap.
#[derive(IntoElement, Default)]
pub struct PillRow {
    label: Option<SharedString>,
    pills: Vec<AnyElement>,
}

impl PillRow {
    pub fn new() -> Self {
        Self::default()
    }

    /// A short label before the pills, such as "Suggestions".
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn pill(mut self, pill: impl IntoElement) -> Self {
        self.pills.push(pill.into_any_element());
        self
    }

    pub fn is_empty(&self) -> bool {
        self.pills.is_empty()
    }
}

impl RenderOnce for PillRow {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(spacing::XS)
            .when_some(self.label, |row, label| {
                row.child(Label::new(label).size(TextSize::Small).tone(TextTone::Secondary))
            })
            .children(self.pills)
    }
}

/// A band under a toolbar for something that comes and goes, such as tag
/// suggestions while a field is being typed in.
#[derive(IntoElement)]
pub struct ToolbarStrip {
    content: AnyElement,
}

impl ToolbarStrip {
    pub fn new(content: impl IntoElement) -> Self {
        Self {
            content: content.into_any_element(),
        }
    }
}

impl RenderOnce for ToolbarStrip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        div()
            .flex_none()
            .px(spacing::SM)
            .pb(spacing::SM)
            .bg(colors.surface.hsla())
            .child(self.content)
    }
}

/// A coloured dot for a tag, used beside its name in lists.
pub fn tag_dot(tag: &str, cx: &App) -> AnyElement {
    div()
        .flex_none()
        .size(sizes::TAG_DOT)
        .rounded_full()
        .bg(tag_color(tag, cx))
        .into_any_element()
}
