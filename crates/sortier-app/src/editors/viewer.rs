use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyView, App, AppContext as _, Bounds, Context, ContentMask, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    PinchEvent, Render, RenderImage, ScrollDelta, ScrollWheelEvent, SharedString, Styled, Subscription, Window,
    actions, canvas, div, prelude::FluentBuilder as _, px,
};
use image::{Frame, RgbaImage};
use sortier_areas::EditorTypeId;
use sortier_library::library::{FitMode, Item, ItemId, MediaKind};
use sortier_media::{Animation, HardwareDecoder, MediaError, Picture, VideoPlayer};
use sortier_render::{Checker, Filter, Quad, Rect, TargetId, TextureKey, View};
use sortier_ui::button::{Button, IconButton};
use sortier_ui::empty_state::EmptyState;
use sortier_ui::fields::{Slider, SliderEvent, SliderState, Toolbar, slider_state};
use sortier_ui::gallery::{Badge, CanvasCaption, backdrop_color, canvas_corners, checker_colors, checker_size, paint_backdrop};
use sortier_ui::icon::IconKind;
use sortier_ui::label::{Label, TextSize, TextTone};
use sortier_ui::menu::{DropdownButton, MenuEntry};
use sortier_ui::tokens::sizes;

use super::{EditorContext, EditorFactory};
use crate::model::{LibraryModel, Page};
use crate::remote::{RemoteAccess, media_error};
use crate::render_service::RenderService;

const KEY_CONTEXT: &str = "Viewer";
const VIDEO_SLOT: u64 = 0xF_FFFF;
/// How far the pointer must move before a press becomes a drag, in view pixels.
const DRAG_THRESHOLD: f32 = 4.0;
/// Scroll distance that turns a page when there's nothing left to pan.
const SEEK_STEPS: f32 = 1000.0;

actions!(
    viewer,
    [
        PreviousPage,
        NextPage,
        PreviousItem,
        NextItem,
        FirstPage,
        LastPage,
        ScrollDown,
        ScrollUp,
        PanUp,
        PanDown,
        CycleFit,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        ActualSize,
        ToggleSpread,
        ToggleDirection,
        PageEarlier,
        PageLater,
        MakeCover,
        PlayPause,
        Mute,
        OpenExternally,
        ShowInFolder,
        DeletePage,
    ]
);

pub fn key_bindings() -> Vec<KeyBinding> {
    let context = Some(KEY_CONTEXT);
    vec![
        KeyBinding::new("a", PreviousPage, context),
        KeyBinding::new("left", PreviousPage, context),
        KeyBinding::new("d", NextPage, context),
        KeyBinding::new("right", NextPage, context),
        KeyBinding::new("w", PreviousItem, context),
        KeyBinding::new("pageup", PreviousItem, context),
        KeyBinding::new("s", NextItem, context),
        KeyBinding::new("pagedown", NextItem, context),
        KeyBinding::new("home", FirstPage, context),
        KeyBinding::new("end", LastPage, context),
        KeyBinding::new("space", ScrollDown, context),
        KeyBinding::new("shift-space", ScrollUp, context),
        KeyBinding::new("up", PanUp, context),
        KeyBinding::new("down", PanDown, context),
        KeyBinding::new("f", CycleFit, context),
        KeyBinding::new("=", ZoomIn, context),
        KeyBinding::new("shift-=", ZoomIn, context),
        KeyBinding::new("-", ZoomOut, context),
        KeyBinding::new("0", ResetZoom, context),
        KeyBinding::new("1", ActualSize, context),
        KeyBinding::new("2", ToggleSpread, context),
        KeyBinding::new("r", ToggleDirection, context),
        KeyBinding::new("[", PageEarlier, context),
        KeyBinding::new("]", PageLater, context),
        KeyBinding::new("c", MakeCover, context),
        KeyBinding::new("k", PlayPause, context),
        KeyBinding::new("m", Mute, context),
        KeyBinding::new("o", OpenExternally, context),
        KeyBinding::new("e", ShowInFolder, context),
        KeyBinding::new(crate::delete::DELETE_KEY, DeletePage, context),
    ]
}

/// One page's media as the viewer holds it.
enum Media {
    Loading,
    Still { size: (u32, u32) },
    Animation { size: (u32, u32), delays: Vec<Duration>, length: Duration },
    Video { player: VideoPlayer, size: (u32, u32), uploaded: Option<u64> },
    Failed(SharedString),
}

/// What a background decode produced.
enum Decoded {
    Still(Picture),
    Animation(Animation),
    /// A page of a document, and how many pages the document has.
    DocumentPage { picture: Picture, pages: u32 },
    /// A remote vault's video, opened.
    Video(VideoPlayer),
}

struct Drag {
    start: Point<Pixels>,
    pan: (f32, f32),
    moved: bool,
}

/// What the last shown frame was drawn from, so an unchanged view isn't drawn again.
#[derive(Clone, PartialEq)]
struct Drawn {
    width: u32,
    height: u32,
    quads: Vec<Quad>,
    /// Which picture each texture held. A video uploads every frame into the same texture, so the quads alone
    /// don't change while it plays.
    contents: Vec<u64>,
}

fn texture_key(id: ItemId, slot: u64) -> TextureKey {
    TextureKey((id << 20) | slot)
}

/// The texture holding a still page: a picture's only texture, or a document page's own. Documents are never
/// animated or videos, so their page numbers don't meet those slots.
fn still_key(page: Page) -> TextureKey {
    texture_key(page.item, u64::from(page.index))
}

/// Shows the open page (or two, side by side) with the GPU renderer: pictures
/// fitted, zoomed and panned, animations playing, and videos with their sound.
pub struct ViewerEditor {
    library: Entity<LibraryModel>,
    renderer: RenderService,
    target: Option<TargetId>,
    focus_handle: FocusHandle,
    media: HashMap<Page, Media>,
    /// When the shown page appeared, which times animations.
    shown_since: Instant,
    shown: Option<(Option<crate::model::Entry>, usize)>,
    zoom: f32,
    /// Offset of the picture from centred, in device pixels.
    pan: (f32, f32),
    reset_pan: bool,
    /// Largest pan each way, in device pixels, from the last layout.
    max_pan: (f32, f32),
    drag: Option<Drag>,
    drawn: Option<Drawn>,
    next_revision: u64,
    displayed: Option<Arc<RenderImage>>,
    retired: Vec<Arc<RenderImage>>,
    error: Option<SharedString>,
    seek: Entity<SliderState>,
    volume: Entity<SliderState>,
    /// When the canvas was last drawn. A viewer in a workspace that isn't
    /// showing stops drawing, and its videos are paused so they aren't heard.
    painted: Instant,
    /// The graphics card decoder videos were opened with, so they're opened again when it changes (hardware
    /// decoding turned on or off, or the decoder found after startup).
    video_decoder: Option<HardwareDecoder>,
    _hidden_check: gpui::Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl ViewerEditor {
    fn new(context: &EditorContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let target = context.renderer.renderer().ok().map(|renderer| renderer.borrow_mut().create_target());
        let volume_value = context.library.read(cx).settings().volume * 100.0;
        let seek = slider_state(0.0, SEEK_STEPS, 1.0, 0.0, cx);
        let volume = slider_state(0.0, 100.0, 1.0, volume_value, cx);
        let subscriptions = vec![
            cx.observe_in(&context.library, window, |this, _, _, cx| {
                this.follow_open_page(cx);
                this.follow_video_decoder(cx);
                cx.notify();
            }),
            cx.subscribe(&seek, |this, _, event: &SliderEvent, cx| {
                let SliderEvent::Change(value) = event else { return };
                if let Some(Media::Video { player, .. }) = this.current_media(cx) {
                    let length = player.info().duration.as_secs_f32();
                    player.seek(Duration::from_secs_f32(value.start() / SEEK_STEPS * length));
                }
                cx.notify();
            }),
            cx.subscribe(&volume, |this, _, event: &SliderEvent, cx| {
                let SliderEvent::Change(value) = event else { return };
                let level = value.start() / 100.0;
                this.library.update(cx, |library, cx| library.update_settings(|s| s.volume = level, cx));
                this.apply_volume(cx);
            }),
        ];
        let video_decoder = context.library.read(cx).playback_decoder();
        let mut this = Self {
            video_decoder,
            library: context.library.clone(),
            renderer: context.renderer.clone(),
            target,
            focus_handle: cx.focus_handle(),
            media: HashMap::new(),
            shown_since: Instant::now(),
            shown: None,
            zoom: 1.0,
            pan: (0.0, 0.0),
            reset_pan: true,
            max_pan: (0.0, 0.0),
            drag: None,
            drawn: None,
            next_revision: 1,
            displayed: None,
            retired: Vec::new(),
            error: None,
            seek,
            volume,
            painted: Instant::now(),
            _hidden_check: cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    let alive = this.update(cx, |this, _| {
                        if this.painted.elapsed() > Duration::from_secs(1) {
                            for media in this.media.values() {
                                if let Media::Video { player, .. } = media
                                    && player.is_playing()
                                {
                                    player.pause();
                                }
                            }
                        }
                    });
                    if alive.is_err() {
                        return;
                    }
                }
            }),
            _subscriptions: subscriptions,
        };
        this.follow_open_page(cx);
        this
    }

    /// The pages shown now, left to right.
    fn shown_pages(&self, cx: &App) -> Vec<Page> {
        let library = self.library.read(cx);
        let Some(open) = library.open() else { return Vec::new() };
        let pages = library.entry_pages(open.entry);
        let Some(&page) = pages.get(open.page) else { return Vec::new() };
        if library.spread_active() {
            let mut pair = vec![page, pages[open.page + 1]];
            if library.settings().rtl {
                pair.reverse();
            }
            pair
        } else {
            vec![page]
        }
    }

    fn current_page(&self, cx: &App) -> Option<Page> {
        self.library.read(cx).open_page()
    }

    fn current_media<'a>(&'a self, cx: &App) -> Option<&'a Media> {
        self.media.get(&self.current_page(cx)?)
    }

    /// Reacts to the open page changing: resets the view, starts loading what's
    /// needed (and the pages either side), pauses videos that are no longer shown,
    /// and lets go of what's far away.
    fn follow_open_page(&mut self, cx: &mut Context<Self>) {
        let (open, pages) = {
            let library = self.library.read(cx);
            (library.open(), library.current_pages())
        };
        let shown = open.map(|open| (Some(open.entry), open.page));
        if shown != self.shown {
            self.shown = shown;
            self.shown_since = Instant::now();
            self.reset_pan = true;
        }
        let Some(open) = open else { return };
        let mut wanted: Vec<Page> = Vec::new();
        for offset in [0isize, 1, 2, -1, 3] {
            let index = open.page as isize + offset;
            if index >= 0
                && let Some(page) = pages.get(index as usize)
            {
                wanted.push(*page);
            }
        }
        let shown_now: HashSet<Page> = self.shown_pages(cx).into_iter().collect();
        for page in &wanted {
            if !self.media.contains_key(page) {
                let is_video = self.library.read(cx).item(page.item).is_some_and(|item| item.kind == MediaKind::Video);
                // Videos open only when shown; they hold a decoder and a sound stream.
                if !is_video || shown_now.contains(page) {
                    self.load(*page, cx);
                }
            }
        }
        let current = self.current_page(cx);
        for (page, media) in &self.media {
            if let Media::Video { player, .. } = media
                && Some(*page) != current
            {
                player.pause();
            }
        }
        let keep: HashSet<Page> = wanted.into_iter().collect();
        let gone: Vec<Page> = self.media.keys().copied().filter(|page| !keep.contains(page)).collect();
        for page in gone {
            self.unload(page);
        }
    }

    fn unload(&mut self, page: Page) {
        let Some(media) = self.media.remove(&page) else { return };
        let Ok(renderer) = self.renderer.renderer() else { return };
        let mut renderer = renderer.borrow_mut();
        match media {
            Media::Still { .. } => renderer.release(still_key(page)),
            Media::Animation { delays, .. } => {
                for frame in 0..delays.len() as u64 {
                    renderer.release(texture_key(page.item, frame + 1));
                }
            }
            Media::Video { .. } => renderer.release(texture_key(page.item, VIDEO_SLOT)),
            _ => {}
        }
    }

    fn load(&mut self, page: Page, cx: &mut Context<Self>) {
        let id = page.item;
        let (item, remote, hardware, volume, muted) = {
            let library = self.library.read(cx);
            let Some(item) = library.item(id) else { return };
            let settings = library.settings();
            (item.clone(), library.remote_access(), library.playback_decoder(), settings.volume, settings.muted)
        };
        if let Some(access) = remote {
            self.load_remote(page, item, access, hardware, cx);
            return;
        }
        let (path, kind) = (item.path, item.kind);
        if kind == MediaKind::Video {
            let media = match VideoPlayer::open(&path, hardware) {
                Ok(player) => {
                    player.set_volume(volume);
                    player.set_muted(muted);
                    player.play();
                    let info = player.info();
                    let size = (info.width, info.height);
                    self.library.update(cx, |library, _| library.set_dims(id, size));
                    Media::Video {
                        player,
                        size,
                        uploaded: None,
                    }
                }
                Err(error) => Media::Failed(error.to_string().into()),
            };
            self.media.insert(page, media);
            return;
        }
        self.media.insert(page, Media::Loading);
        let task = cx.background_spawn(async move {
            if kind == MediaKind::Document {
                decode_document_page(&path, page.index)
            } else {
                decode(&path)
            }
        });
        cx.spawn(async move |this, cx| {
            let decoded = task.await;
            this.update(cx, |this, cx| this.loaded(page, decoded, cx)).ok();
        })
        .detach();
    }

    /// Loads a page of a remote vault: pictures fetched whole into its cache, document pages drawn by the server, and
    /// videos streamed (opened off the UI thread, since that waits on the network).
    fn load_remote(&mut self, page: Page, item: Item, access: RemoteAccess, hardware: Option<HardwareDecoder>, cx: &mut Context<Self>) {
        self.media.insert(page, Media::Loading);
        let task = cx.background_spawn(async move {
            match item.kind {
                MediaKind::Video => VideoPlayer::open_source(&access.video(&item), hardware).map(Decoded::Video),
                MediaKind::Document => Ok(Decoded::DocumentPage {
                    picture: access.page(&item, page.index)?,
                    pages: item.pages.unwrap_or(1),
                }),
                _ => decode(&access.file(&item).map_err(media_error)?),
            }
        });
        cx.spawn(async move |this, cx| {
            let decoded = task.await;
            this.update(cx, |this, cx| this.loaded(page, decoded, cx)).ok();
        })
        .detach();
    }

    fn loaded(&mut self, page: Page, decoded: Result<Decoded, MediaError>, cx: &mut Context<Self>) {
        if !self.media.contains_key(&page) {
            return;
        }
        let id = page.item;
        let Ok(renderer) = self.renderer.renderer() else { return };
        let mut pages = None;
        let media = match decoded {
            Ok(Decoded::Still(picture)) => {
                let size = (picture.width, picture.height);
                match renderer.borrow_mut().upload(still_key(page), picture.width, picture.height, &picture.rgba, true) {
                    Ok(()) => Media::Still { size },
                    Err(error) => Media::Failed(format!("{error:#}").into()),
                }
            }
            Ok(Decoded::Video(player)) => {
                let settings = self.library.read(cx).settings().clone();
                player.set_volume(settings.volume);
                player.set_muted(settings.muted);
                if Some(page) == self.current_page(cx) {
                    player.play();
                }
                let size = (player.info().width, player.info().height);
                Media::Video {
                    player,
                    size,
                    uploaded: None,
                }
            }
            Ok(Decoded::DocumentPage { picture, pages: count }) => {
                pages = Some(count);
                let size = (picture.width, picture.height);
                match renderer.borrow_mut().upload(still_key(page), picture.width, picture.height, &picture.rgba, true) {
                    Ok(()) => Media::Still { size },
                    Err(error) => Media::Failed(format!("{error:#}").into()),
                }
            }
            Ok(Decoded::Animation(animation)) => {
                let size = (animation.width, animation.height);
                let mut renderer = renderer.borrow_mut();
                let mut delays = Vec::with_capacity(animation.frames.len());
                let mut failed = None;
                for (index, frame) in animation.frames.iter().enumerate() {
                    let picture = &frame.picture;
                    if let Err(error) = renderer.upload(texture_key(id, index as u64 + 1), picture.width, picture.height, &picture.rgba, true) {
                        failed = Some(format!("{error:#}"));
                        break;
                    }
                    // Browsers treat very short GIF delays as 100 ms; so do we.
                    let delay = if frame.delay < Duration::from_millis(20) { Duration::from_millis(100) } else { frame.delay };
                    delays.push(delay);
                }
                match failed {
                    Some(error) => Media::Failed(error.into()),
                    None => Media::Animation {
                        size,
                        length: delays.iter().sum(),
                        delays,
                    },
                }
            }
            Err(error) => Media::Failed(error.to_string().into()),
        };
        if let Media::Still { size } | Media::Animation { size, .. } = &media
            && page.index == 0
        {
            let size = *size;
            self.library.update(cx, |library, _| library.set_dims(id, size));
        }
        self.media.insert(page, media);
        if let Some(pages) = pages {
            // Learning the page count makes the document's later pages exist, so the library tells everyone.
            self.library.update(cx, |library, cx| {
                library.set_pages(id, pages);
                cx.notify();
            });
        }
        cx.notify();
    }

    /// Opens the loaded videos again when the decoder to play them with has changed, carrying on from where each
    /// was, playing or paused as it was. One that had played to the end waits at the start, since there's no
    /// picture to show past the last one.
    fn follow_video_decoder(&mut self, cx: &mut Context<Self>) {
        let decoder = self.library.read(cx).playback_decoder();
        if decoder == self.video_decoder {
            return;
        }
        self.video_decoder = decoder;
        let videos: Vec<(Page, Option<Duration>, bool)> = self
            .media
            .iter()
            .filter_map(|(page, media)| match media {
                Media::Video { player, .. } => {
                    let position = (!player.ended()).then(|| player.position());
                    Some((*page, position, player.is_playing()))
                }
                _ => None,
            })
            .collect();
        for (page, position, playing) in videos {
            self.unload(page);
            self.load(page, cx);
            if let Some(Media::Video { player, .. }) = self.media.get(&page) {
                if let Some(position) = position {
                    player.seek(position);
                }
                if !playing {
                    player.pause();
                }
            }
        }
    }

    fn apply_volume(&mut self, cx: &App) {
        let settings = self.library.read(cx).settings();
        for media in self.media.values() {
            if let Media::Video { player, .. } = media {
                player.set_volume(settings.volume);
                player.set_muted(settings.muted);
            }
        }
    }

    /// The texture showing `id` right now, its pixel size, and which picture it holds (for a video, the frame's
    /// serial; for anything else, 0), if it's ready.
    fn texture_for(&mut self, page: Page, cx: &App) -> Option<(TextureKey, (u32, u32), u64)> {
        let renderer = self.renderer.renderer().ok()?.clone();
        let mut renderer = renderer.borrow_mut();
        let id = page.item;
        match self.media.get_mut(&page)? {
            Media::Still { size } => Some((still_key(page), *size, 0)),
            Media::Animation { size, delays, length } => {
                let length = length.as_nanos().max(1);
                let mut at = Duration::from_nanos((self.shown_since.elapsed().as_nanos() % length) as u64);
                let mut frame = 0;
                for (index, delay) in delays.iter().enumerate() {
                    if at < *delay {
                        frame = index;
                        break;
                    }
                    at -= *delay;
                }
                Some((texture_key(id, frame as u64 + 1), *size, 0))
            }
            Media::Video { player, size, uploaded } => {
                let key = texture_key(id, VIDEO_SLOT);
                let serial = player.frame_serial();
                if *uploaded != Some(serial)
                    && let Some(frame) = player.current_frame()
                {
                    let picture = &frame.picture;
                    if renderer.upload(key, picture.width, picture.height, &picture.rgba, false).is_ok() {
                        *uploaded = Some(serial);
                        *size = (picture.width, picture.height);
                    }
                }
                let _ = cx;
                uploaded.map(|serial| (key, *size, serial))
            }
            _ => None,
        }
    }

    fn animating(&self, cx: &App) -> bool {
        self.shown_pages(cx).iter().any(|page| match self.media.get(page) {
            Some(Media::Animation { delays, .. }) => delays.len() > 1,
            Some(Media::Video { player, .. }) => player.is_playing() || player.is_waiting(),
            _ => false,
        })
    }

    /// Lays out the shown pages in `bounds`, submits a new frame when what's
    /// shown changed, and collects any finished frame.
    fn prepaint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        self.painted = Instant::now();
        let scale = window.scale_factor();
        let width = (f32::from(bounds.size.width) * scale).round().max(1.0);
        let height = (f32::from(bounds.size.height) * scale).round().max(1.0);
        let shown = self.shown_pages(cx);
        let mut textures = Vec::new();
        for page in &shown {
            if let Some(texture) = self.texture_for(*page, cx) {
                textures.push(texture);
            }
        }
        // Missing textures (another viewer let go of them) are loaded again.
        if let Ok(renderer) = self.renderer.renderer() {
            let missing: Vec<Page> = shown
                .iter()
                .copied()
                .filter(|page| match self.media.get(page) {
                    Some(Media::Still { .. }) => !renderer.borrow().contains(still_key(*page)),
                    None => true,
                    _ => false,
                })
                .collect();
            for page in missing {
                self.media.remove(&page);
                self.load(page, cx);
            }
        }

        let settings = self.library.read(cx).settings().clone();
        let quads = if textures.len() == shown.len() && !textures.is_empty() {
            let first_height = textures[0].1.1.max(1) as f32;
            let widths: Vec<f32> = textures
                .iter()
                .map(|(_, (w, h), _)| *w as f32 * first_height / (*h).max(1) as f32)
                .collect();
            let total_width: f32 = widths.iter().sum();
            let fit = match settings.fit {
                FitMode::Fit => (width / total_width).min(height / first_height),
                FitMode::Width => width / total_width,
                FitMode::Height => height / first_height,
                FitMode::Original => 1.0,
            };
            let scale_to_view = fit * self.zoom;
            let (shown_width, shown_height) = (total_width * scale_to_view, first_height * scale_to_view);
            self.max_pan = (((shown_width - width) / 2.0).max(0.0), ((shown_height - height) / 2.0).max(0.0));
            if self.reset_pan {
                // Long pages start at their top; wide ones at their start in reading order.
                let x = if settings.rtl { -self.max_pan.0 } else { self.max_pan.0 };
                self.pan = (x, self.max_pan.1);
                self.reset_pan = false;
            }
            self.pan = (self.pan.0.clamp(-self.max_pan.0, self.max_pan.0), self.pan.1.clamp(-self.max_pan.1, self.max_pan.1));
            let (light, dark) = checker_colors(cx);
            let checker = Checker {
                light,
                dark,
                size: checker_size(window, cx),
            };
            let filter = if settings.pixelated { Filter::Pixelated } else { Filter::Smooth };
            let mut x = (width - shown_width) / 2.0 + self.pan.0;
            let y = (height - shown_height) / 2.0 + self.pan.1;
            textures
                .iter()
                .zip(widths)
                .map(|((key, _, _), page_width)| {
                    let rect = Rect {
                        x,
                        y,
                        width: page_width * scale_to_view,
                        height: shown_height,
                    };
                    x += rect.width;
                    Quad {
                        texture: *key,
                        rect,
                        opacity: 1.0,
                        checker: Some(checker),
                        filter,
                    }
                })
                .collect()
        } else {
            self.max_pan = (0.0, 0.0);
            Vec::new()
        };

        if let (Ok(renderer), Some(target)) = (self.renderer.renderer(), self.target) {
            let mut renderer = renderer.borrow_mut();
            let drawn = Drawn {
                width: width as u32,
                height: height as u32,
                contents: if quads.is_empty() { Vec::new() } else { textures.iter().map(|(_, _, contents)| *contents).collect() },
                quads,
            };
            if self.drawn.as_ref() != Some(&drawn) {
                let view = View {
                    width: drawn.width,
                    height: drawn.height,
                    background: backdrop_color(cx),
                    quads: drawn.quads.clone(),
                    revision: self.next_revision,
                };
                match renderer.submit(target, &view) {
                    Ok(true) => {
                        self.next_revision += 1;
                        self.drawn = Some(drawn);
                        self.error = None;
                    }
                    Ok(false) => window.request_animation_frame(),
                    Err(error) => self.error = Some(format!("{error:#}").into()),
                }
            }
            match renderer.take_frame(target) {
                Ok(Some(frame)) => {
                    if let Some(buffer) = RgbaImage::from_raw(frame.width, frame.height, frame.bgra) {
                        let image = Arc::new(RenderImage::new(vec![Frame::new(buffer)]));
                        if let Some(previous) = self.displayed.replace(image) {
                            self.retired.push(previous);
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => self.error = Some(format!("{error:#}").into()),
            }
            if renderer.has_pending_frames(target) {
                window.request_animation_frame();
            }
        }
        if self.animating(cx) || shown.iter().any(|id| matches!(self.media.get(id), Some(Media::Loading))) {
            window.request_animation_frame();
        }
        self.displayed.clone()
    }

    fn pan_by(&mut self, dx: f32, dy: f32) -> bool {
        let before = self.pan;
        self.pan = (
            (self.pan.0 + dx).clamp(-self.max_pan.0, self.max_pan.0),
            (self.pan.1 + dy).clamp(-self.max_pan.1, self.max_pan.1),
        );
        (self.pan.0 - before.0).abs() > 0.5 || (self.pan.1 - before.1).abs() > 0.5
    }

    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.zoom_at(factor, (0.0, 0.0), cx);
    }

    /// Zooms by `factor`, keeping the point `anchor` (device pixels from the canvas centre) still on screen.
    fn zoom_at(&mut self, factor: f32, anchor: (f32, f32), cx: &mut Context<Self>) {
        let before = self.zoom;
        self.zoom = (self.zoom * factor).clamp(0.1, 32.0);
        let ratio = self.zoom / before;
        self.pan = (
            anchor.0 - (anchor.0 - self.pan.0) * ratio,
            anchor.1 - (anchor.1 - self.pan.1) * ratio,
        );
        cx.notify();
    }

    /// Where `position` is, in device pixels from the centre of the canvas at `bounds`.
    fn anchor(position: Point<Pixels>, bounds: Bounds<Pixels>, window: &Window) -> (f32, f32) {
        let centre = bounds.center();
        let scale = window.scale_factor();
        (f32::from(position.x - centre.x) * scale, f32::from(position.y - centre.y) * scale)
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.library.update(cx, |library, cx| library.step_page(forward, cx));
    }

    /// Page turns follow the reading direction: in right-to-left mode, left goes forward.
    fn turn(&mut self, toward_right: bool, cx: &mut Context<Self>) {
        let rtl = self.library.read(cx).settings().rtl;
        self.step(toward_right != rtl, cx);
    }

    fn scroll_page(&mut self, down: bool, window: &Window, cx: &mut Context<Self>) {
        if let Some(Media::Video { player, .. }) = self.current_media(cx) {
            if player.is_playing() { player.pause() } else { player.play() }
            cx.notify();
            return;
        }
        let step = f32::from(window.viewport_size().height) * window.scale_factor() * 0.8;
        if !self.pan_by(0.0, if down { -step } else { step }) {
            self.step(down, cx);
        }
        cx.notify();
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        if event.click_count >= 2 {
            self.drag = None;
            self.zoom = if self.zoom > 1.01 { 1.0 } else { 2.0 };
            self.reset_pan = self.zoom <= 1.0;
        } else {
            self.drag = Some(Drag {
                start: event.position,
                pan: self.pan,
                moved: false,
            });
        }
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let scale = window.scale_factor();
        let dx = f32::from(event.position.x - drag.start.x);
        let dy = f32::from(event.position.y - drag.start.y);
        if dx.abs() + dy.abs() > DRAG_THRESHOLD {
            drag.moved = true;
        }
        let (x, y) = (drag.pan.0 + dx * scale, drag.pan.1 + dy * scale);
        self.pan = (x.clamp(-self.max_pan.0, self.max_pan.0), y.clamp(-self.max_pan.1, self.max_pan.1));
        cx.notify();
    }

    /// A click that didn't drag turns the page when it's in the outer thirds.
    fn mouse_up(&mut self, event: &MouseUpEvent, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        if drag.moved {
            return;
        }
        let along = f32::from(event.position.x - bounds.origin.x) / f32::from(bounds.size.width).max(1.0);
        if along < 0.3 {
            self.turn(false, cx);
        } else if along > 0.7 {
            self.turn(true, cx);
        }
    }

    /// A mouse wheel (or any scrolling with Cmd/Ctrl held) zooms toward the pointer; a trackpad's two-finger scroll
    /// pans.
    fn scroll(&mut self, event: &ScrollWheelEvent, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(40.));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        let wheel = matches!(event.delta, ScrollDelta::Lines(_));
        if wheel || event.modifiers.secondary() {
            let anchor = Self::anchor(event.position, bounds, window);
            self.zoom_at((1.0 + dy / 300.0).clamp(0.5, 2.0), anchor, cx);
            return;
        }
        let scale = window.scale_factor();
        self.pan_by(dx * scale, dy * scale);
        cx.notify();
    }

    fn pinch(&mut self, event: &PinchEvent, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = Self::anchor(event.position, bounds, window);
        self.zoom_at((1.0 + event.delta).clamp(0.5, 2.0), anchor, cx);
    }

    fn with_page(&mut self, cx: &mut Context<Self>, action: impl FnOnce(&mut LibraryModel, ItemId, &mut Context<LibraryModel>)) {
        if let Some(id) = self.current_page(cx).map(|page| page.item) {
            self.library.update(cx, |library, cx| action(library, id, cx));
        }
    }

    fn move_page(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.with_page(cx, |library, id, cx| library.move_page_by(id, delta, cx));
    }

    fn open_externally(&mut self, cx: &mut Context<Self>) {
        self.with_page(cx, |library, id, cx| library.open_in_default_app(id, cx));
    }

    fn fit_menu(&self, cx: &App) -> Vec<MenuEntry> {
        let current = self.library.read(cx).settings().fit;
        [FitMode::Fit, FitMode::Width, FitMode::Height, FitMode::Original]
            .into_iter()
            .map(|fit| {
                let library = self.library.clone();
                MenuEntry::command(fit.label(), move |_, cx| {
                    library.update(cx, |library, cx| library.set_fit(fit, cx));
                })
                .checked(fit == current)
            })
            .collect()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> Toolbar {
        let entity = cx.entity().downgrade();
        let library = self.library.read(cx);
        let settings = library.settings().clone();
        let open = library.open();
        let pages = library.current_pages().len();
        let position = match open {
            Some(open) if pages > 1 && library.spread_active() => format!("Pages {}\u{2013}{} of {pages}", open.page + 1, open.page + 2),
            Some(open) if pages > 1 => format!("Page {} of {pages}", open.page + 1),
            Some(open) => {
                let index = library.result_index(open.entry).map(|index| index + 1).unwrap_or(0);
                format!("{index} of {}", library.results().len())
            }
            None => String::new(),
        };
        let name = library.current_page().map(|item| item.file_name()).unwrap_or_default();
        let button = |id: &'static str, icon: IconKind, label: &'static str, action: fn(&mut Self, &mut Context<Self>)| {
            let entity = entity.clone();
            IconButton::new(id, icon, label).on_click(move |_, cx| {
                entity.update(cx, action).ok();
            })
        };
        let toggle = |id: &'static str, icon: IconKind, label: &'static str, on: bool, change: fn(&mut sortier_library::library::Settings)| {
            let library = self.library.clone();
            IconButton::new(id, icon, label).selected(on).on_click(move |_, cx| {
                library.update(cx, |library, cx| library.update_settings(change, cx));
            })
        };
        Toolbar::new()
            .item(button("previous", IconKind::ChevronLeft, "Previous Page (A)", |this, cx| this.turn(false, cx)))
            .item(button("next", IconKind::ChevronRight, "Next Page (D)", |this, cx| this.turn(true, cx)))
            .item(Label::new(position).size(TextSize::Small))
            .item(Label::new(name).size(TextSize::Small).tone(TextTone::Secondary).truncate())
            .spacer()
            .item(DropdownButton::new("fit", self.fit_menu(cx)).label(settings.fit.label()).icon(IconKind::Fit))
            .item(button("zoom-out", IconKind::ZoomOut, "Zoom Out (\u{2212})", |this, cx| this.zoom_by(0.8, cx)))
            .item(Label::new(format!("{:.0}%", self.zoom * 100.0)).size(TextSize::Small).tone(TextTone::Secondary))
            .item(button("zoom-in", IconKind::ZoomIn, "Zoom In (+)", |this, cx| this.zoom_by(1.25, cx)))
            .divider(cx)
            .item(toggle("spread", IconKind::Spread, "Two-Page Spread (2)", settings.spread, |s| s.spread = !s.spread))
            .item(toggle("rtl", IconKind::ReadingDirection, "Right to Left (R)", settings.rtl, |s| s.rtl = !s.rtl))
            .item(button("external", IconKind::External, "Open in Default App (O)", |this, cx| this.open_externally(cx)))
    }

    fn transport(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Toolbar> {
        let Some(Media::Video { player, .. }) = self.current_media(cx) else { return None };
        let playing = player.is_playing();
        let position = player.position();
        let length = player.info().duration;
        let fraction = position.as_secs_f32() / length.as_secs_f32().max(0.001);
        self.seek.update(cx, |seek, cx| seek.set_value(fraction * SEEK_STEPS, window, cx));
        let muted = self.library.read(cx).settings().muted;
        let decoding = match player.hardware_decoder() {
            Some(decoder) => Badge::icon_text(IconKind::Gpu, decoder.label()),
            None => Badge::icon_text(IconKind::Cpu, "CPU"),
        };
        let entity = cx.entity().downgrade();
        let library = self.library.clone();
        Some(
            Toolbar::new()
                .item(
                    IconButton::new("play", if playing { IconKind::Pause } else { IconKind::Play }, "Play or Pause (K)").on_click(
                        move |_, cx| {
                            entity
                                .update(cx, |this, cx| {
                                    if let Some(Media::Video { player, .. }) = this.current_media(cx) {
                                        if player.is_playing() { player.pause() } else { player.play() }
                                    }
                                    cx.notify();
                                })
                                .ok();
                        },
                    ),
                )
                .item(div().flex_1().min_w_0().child(Slider::new(&self.seek)))
                .item(Label::new(format!("{} / {}", clock(position), clock(length))).size(TextSize::Small).tone(TextTone::Secondary))
                .item(decoding)
                .item(
                    IconButton::new("mute", if muted { IconKind::VolumeOff } else { IconKind::Volume }, "Mute (M)")
                        .selected(muted)
                        .on_click(move |_, cx| {
                            library.update(cx, |library, cx| library.update_settings(|s| s.muted = !s.muted, cx));
                        }),
                )
                .item(div().w(sizes::TOOLBAR_SLIDER_WIDTH).child(Slider::new(&self.volume))),
        )
    }

    fn notice(&self, cx: &mut Context<Self>) -> Option<EmptyState> {
        if let Err(error) = self.renderer.renderer() {
            return Some(EmptyState::new(IconKind::Viewer, "The viewer can't draw", error.clone()));
        }
        if let Some(error) = &self.error {
            return Some(EmptyState::new(IconKind::Viewer, "The viewer can't draw", error.clone()));
        }
        match self.current_media(cx)? {
            Media::Failed(error) => Some(EmptyState::new(IconKind::ImageOff, "This file can't be shown", error.clone()).action(
                Button::new("open-external", "Open in Default App").icon(IconKind::External).on_click({
                    let library = self.library.clone();
                    let id = self.current_page(cx).map(|page| page.item);
                    move |_, cx| {
                        if let Some(id) = id {
                            library.update(cx, |library, cx| library.open_in_default_app(id, cx));
                        }
                    }
                }),
            )),
            _ => None,
        }
    }
}

/// `m:ss`, or `h:mm:ss` past an hour.
fn clock(time: Duration) -> String {
    let seconds = time.as_secs();
    if seconds >= 3600 {
        format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

/// Decodes a picture, keeping every frame of animated GIF, PNG and WebP files.
fn decode(path: &Path) -> Result<Decoded, MediaError> {
    let animatable = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| ["gif", "png", "apng", "webp"].contains(&extension.to_ascii_lowercase().as_str()));
    if animatable && let Some(animation) = sortier_media::decode_animation(path)? {
        return Ok(Decoded::Animation(animation));
    }
    sortier_media::decode_still(path).map(Decoded::Still)
}

/// Page `index` of a document at its full size, and how many pages it has.
fn decode_document_page(path: &Path, index: u32) -> Result<Decoded, MediaError> {
    let pages = sortier_media::document_page_count(path)?;
    let picture = sortier_media::document_page(path, index, None)?;
    Ok(Decoded::DocumentPage { picture, pages })
}

impl Drop for ViewerEditor {
    fn drop(&mut self) {
        let pages: Vec<Page> = self.media.keys().copied().collect();
        for page in pages {
            self.unload(page);
        }
        if let (Ok(renderer), Some(target)) = (self.renderer.renderer(), self.target) {
            renderer.borrow_mut().release_target(target);
        }
    }
}

impl Focusable for ViewerEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ViewerEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.library.update(cx, |library, _| library.take_viewer_focus()) {
            window.focus(&self.focus_handle, cx);
        }
        let prepaint_entity = cx.entity();
        let paint_entity = cx.entity();
        let bounds_cell = std::rc::Rc::new(std::cell::Cell::new(Bounds::default()));
        let bounds_for_up = bounds_cell.clone();
        let bounds_for_scroll = bounds_cell.clone();
        let bounds_for_pinch = bounds_cell.clone();
        let notice = self.notice(cx);
        let caption = {
            let library = self.library.read(cx);
            match library.open() {
                Some(open) if library.current_pages().len() > 1 => {
                    Some(format!("{} \u{b7} {}", library.entry_title(open.entry), open.page + 1))
                }
                _ => None,
            }
        };
        let canvas_view = div()
            .id("viewer-canvas")
            .relative()
            .flex_1()
            .min_h_0()
            .m(sizes::CANVAS_INSET)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, _, cx| this.mouse_up(event, bounds_for_up.get(), cx)),
            )
            .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, window, cx| {
                this.scroll(event, bounds_for_scroll.get(), window, cx)
            }))
            .on_pinch(cx.listener(move |this, event: &PinchEvent, window, cx| this.pinch(event, bounds_for_pinch.get(), window, cx)))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        bounds_cell.set(bounds);
                        prepaint_entity.update(cx, |this, cx| this.prepaint(bounds, window, cx))
                    },
                    move |bounds, image: Option<Arc<RenderImage>>, window, cx| {
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            paint_backdrop(bounds, window, cx);
                            if let Some(image) = image {
                                // A frame that can't be placed is skipped; the next one replaces it.
                                let _ = window.paint_image(bounds, bounds, canvas_corners(cx), image, 0, false);
                            }
                        });
                        let retired = paint_entity.update(cx, |this, _| std::mem::take(&mut this.retired));
                        for image in retired {
                            let _ = window.drop_image(image);
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(caption.map(CanvasCaption::new))
            .when_some(notice, |view, notice| view.child(div().absolute().inset_0().child(notice)));
        let transport = self.transport(window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &PreviousPage, _, cx| this.turn(false, cx)))
            .on_action(cx.listener(|this, _: &NextPage, _, cx| this.turn(true, cx)))
            .on_action(cx.listener(|this, _: &PreviousItem, _, cx| {
                this.library.update(cx, |library, cx| library.step_entry(-1, false, cx))
            }))
            .on_action(cx.listener(|this, _: &NextItem, _, cx| {
                this.library.update(cx, |library, cx| library.step_entry(1, false, cx))
            }))
            .on_action(cx.listener(|this, _: &FirstPage, _, cx| this.library.update(cx, |library, cx| library.go_to_page(0, cx))))
            .on_action(cx.listener(|this, _: &LastPage, _, cx| {
                this.library.update(cx, |library, cx| library.go_to_page(usize::MAX, cx))
            }))
            .on_action(cx.listener(|this, _: &ScrollDown, window, cx| this.scroll_page(true, window, cx)))
            .on_action(cx.listener(|this, _: &ScrollUp, window, cx| this.scroll_page(false, window, cx)))
            .on_action(cx.listener(|this, _: &PanUp, window, cx| {
                let step = f32::from(window.viewport_size().height) * window.scale_factor() * 0.2;
                this.pan_by(0.0, step);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PanDown, window, cx| {
                let step = f32::from(window.viewport_size().height) * window.scale_factor() * 0.2;
                this.pan_by(0.0, -step);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CycleFit, _, cx| {
                this.zoom = 1.0;
                this.reset_pan = true;
                this.library.update(cx, |library, cx| {
                    let next = library.settings().fit.next();
                    library.set_fit(next, cx)
                });
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom_by(1.25, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom_by(0.8, cx)))
            .on_action(cx.listener(|this, _: &ResetZoom, _, cx| {
                this.zoom = 1.0;
                this.reset_pan = true;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ActualSize, _, cx| {
                this.zoom = 1.0;
                this.reset_pan = true;
                this.library.update(cx, |library, cx| library.set_fit(FitMode::Original, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleSpread, _, cx| {
                this.library.update(cx, |library, cx| library.update_settings(|s| s.spread = !s.spread, cx))
            }))
            .on_action(cx.listener(|this, _: &ToggleDirection, _, cx| {
                this.library.update(cx, |library, cx| library.update_settings(|s| s.rtl = !s.rtl, cx))
            }))
            .on_action(cx.listener(|this, _: &PageEarlier, _, cx| this.move_page(-1, cx)))
            .on_action(cx.listener(|this, _: &PageLater, _, cx| this.move_page(1, cx)))
            .on_action(cx.listener(|this, _: &MakeCover, _, cx| this.with_page(cx, |library, id, cx| library.make_cover(id, cx))))
            .on_action(cx.listener(|this, _: &PlayPause, window, cx| this.scroll_page(true, window, cx)))
            .on_action(cx.listener(|this, _: &Mute, _, cx| {
                this.library.update(cx, |library, cx| library.update_settings(|s| s.muted = !s.muted, cx));
                this.apply_volume(cx);
            }))
            .on_action(cx.listener(|this, _: &OpenExternally, _, cx| this.open_externally(cx)))
            .on_action(cx.listener(|this, _: &DeletePage, window, cx| {
                if let Some(page) = this.current_page(cx) {
                    crate::delete::confirm_delete(&this.library, vec![page.item], window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowInFolder, _, cx| {
                this.with_page(cx, |library, id, cx| library.show_in_folder(id, cx))
            }))
            .child(self.toolbar(cx))
            .child(canvas_view)
            .children(transport)
    }
}

pub struct ViewerFactory;

impl EditorFactory for ViewerFactory {
    fn id(&self) -> EditorTypeId {
        super::viewer_id()
    }

    fn name(&self) -> SharedString {
        "Viewer".into()
    }

    fn icon(&self) -> IconKind {
        IconKind::Viewer
    }

    fn create(&self, context: &EditorContext, window: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|cx| ViewerEditor::new(context, window, cx)).into()
    }
}

#[cfg(test)]
mod tests {
    use super::clock;
    use std::time::Duration;

    #[test]
    fn clock_shows_hours_only_when_needed() {
        assert_eq!(clock(Duration::from_secs(75)), "1:15");
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
    }
}
