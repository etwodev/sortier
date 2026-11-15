use std::cell::Cell;
use std::collections::BTreeSet;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyView, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Pixels, Render, ScrollStrategy, SharedString, Styled, Subscription, UniformListScrollHandle,
    Window, actions, canvas, div, px, uniform_list,
};
use sortier_areas::EditorTypeId;
use sortier_library::library::{MediaKind, SortKey};
use sortier_ui::button::{Button, ButtonVariant, IconButton};
use sortier_ui::empty_state::EmptyState;
use sortier_ui::fields::Toolbar;
use sortier_ui::gallery::{Badge, MediaTile, PillRow, TagPill, ThumbnailCache, TilePlaceholder, ToolbarStrip, thumbnail};
use sortier_ui::icon::{Icon, IconKind};
use sortier_ui::label::{Label, TextSize, TextTone};
use sortier_ui::menu::{DropdownButton, MenuEntry};
use sortier_ui::text_field::{TextField, TextFieldEvent, TextFieldState};
use sortier_ui::tokens::{sizes, to_px};

use super::{EditorContext, EditorFactory};
use crate::layouts::LayoutModel;
use crate::menus::AddFolder;
use crate::model::{Entry, LibraryModel};

const KEY_CONTEXT: &str = "LibraryGrid";

actions!(
    library_grid,
    [
        Left,
        Right,
        Up,
        Down,
        ExtendLeft,
        ExtendRight,
        ExtendUp,
        ExtendDown,
        PageUp,
        PageDown,
        First,
        Last,
        OpenCursor,
        Deselect,
        FocusSearch,
        FocusTags,
        ToggleTiles,
        Bigger,
        Smaller,
        ShowVolumes,
        LeaveCollection,
    ]
);

pub fn key_bindings() -> Vec<KeyBinding> {
    let context = Some(KEY_CONTEXT);
    vec![
        KeyBinding::new("a", Left, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("d", Right, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("w", Up, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("s", Down, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("shift-a", ExtendLeft, context),
        KeyBinding::new("shift-left", ExtendLeft, context),
        KeyBinding::new("shift-d", ExtendRight, context),
        KeyBinding::new("shift-right", ExtendRight, context),
        KeyBinding::new("shift-w", ExtendUp, context),
        KeyBinding::new("shift-up", ExtendUp, context),
        KeyBinding::new("shift-s", ExtendDown, context),
        KeyBinding::new("shift-down", ExtendDown, context),
        KeyBinding::new("pageup", PageUp, context),
        KeyBinding::new("pagedown", PageDown, context),
        KeyBinding::new("home", First, context),
        KeyBinding::new("end", Last, context),
        KeyBinding::new("enter", OpenCursor, context),
        KeyBinding::new("space", OpenCursor, context),
        KeyBinding::new("escape", Deselect, context),
        KeyBinding::new("secondary-f", FocusSearch, None),
        KeyBinding::new("/", FocusSearch, context),
        KeyBinding::new("t", FocusTags, context),
        KeyBinding::new("g", ToggleTiles, context),
        KeyBinding::new("=", Bigger, context),
        KeyBinding::new("shift-=", Bigger, context),
        KeyBinding::new("-", Smaller, context),
        KeyBinding::new("secondary-down", ShowVolumes, context),
        KeyBinding::new("secondary-up", LeaveCollection, context),
    ]
}

/// Which toolbar field suggestions are being offered for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Typing {
    Search,
    Tags,
}

/// The library grid: every result as a tile, a search field, and a field for
/// tagging the selection. Arrow keys and WASD move, Enter opens in a viewer.
pub struct LibraryEditor {
    library: Entity<LibraryModel>,
    layouts: Entity<LayoutModel>,
    focus_handle: FocusHandle,
    search: Entity<TextFieldState>,
    tags: Entity<TextFieldState>,
    typing: Option<Typing>,
    scroll: UniformListScrollHandle,
    thumbnails: Entity<ThumbnailCache>,
    width: Rc<Cell<Pixels>>,
    columns: usize,
    _subscriptions: Vec<Subscription>,
}

impl LibraryEditor {
    fn new(context: &EditorContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextFieldState::new(window, cx).placeholder("Search tags  \u{b7}  -exclude  type:video  is:untagged"));
        let tags = cx.new(|cx| TextFieldState::new(window, cx).placeholder("Add tags to the selection"));
        let query = context.library.read(cx).query().to_string();
        search.update(cx, |field, cx| field.set_value(query, window, cx));
        let subscriptions = vec![
            cx.observe_in(&context.library, window, |this, library, window, cx| {
                let query = library.read(cx).query().to_string();
                if this.search.read(cx).value() != query.as_str() {
                    this.search.update(cx, |field, cx| field.set_value(query, window, cx));
                }
                cx.notify();
            }),
            cx.subscribe_in(&search, window, Self::search_event),
            cx.subscribe_in(&tags, window, Self::tags_event),
        ];
        Self {
            library: context.library.clone(),
            layouts: context.layouts.clone(),
            focus_handle: cx.focus_handle(),
            search,
            tags,
            typing: None,
            scroll: UniformListScrollHandle::new(),
            thumbnails: ThumbnailCache::new(800, cx),
            width: Rc::new(Cell::new(px(0.))),
            columns: 1,
            _subscriptions: subscriptions,
        }
    }

    fn search_event(&mut self, field: &Entity<TextFieldState>, event: &TextFieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::Change => {
                let text = field.read(cx).value().to_string();
                self.library.update(cx, |library, cx| library.set_query(&text, cx));
                self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            TextFieldEvent::Focus => self.typing = Some(Typing::Search),
            TextFieldEvent::Blur => self.typing = self.typing.filter(|typing| *typing != Typing::Search),
            TextFieldEvent::PressEnter { .. } => {
                window.focus(&self.focus_handle, cx);
                self.library.update(cx, |library, cx| {
                    if library.cursor().is_none() {
                        library.move_cursor(0, false, cx);
                    }
                });
            }
        }
        cx.notify();
    }

    fn tags_event(&mut self, field: &Entity<TextFieldState>, event: &TextFieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::PressEnter { .. } => {
                let text = field.read(cx).value().to_string();
                self.library.update(cx, |library, cx| {
                    let selection = library.selection().to_vec();
                    library.add_tags(&selection, &text, cx);
                });
                field.update(cx, |field, cx| field.set_value("", window, cx));
                window.focus(&self.focus_handle, cx);
            }
            TextFieldEvent::Focus => self.typing = Some(Typing::Tags),
            TextFieldEvent::Blur => self.typing = self.typing.filter(|typing| *typing != Typing::Tags),
            TextFieldEvent::Change => {}
        }
        cx.notify();
    }

    fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let landed = self.library.update(cx, |library, cx| library.move_cursor(delta, extend, cx));
        if let Some(index) = landed {
            self.scroll.scroll_to_item(index / self.columns.max(1), ScrollStrategy::Center);
        }
    }

    fn rows_per_page(&self, window: &Window, cx: &App) -> isize {
        let tile = MediaTile::height(self.library.read(cx).settings().tile_size, cx);
        let rows = f32::from(window.viewport_size().height) / f32::from(tile).max(1.0);
        (rows as isize).max(1)
    }

    fn open(&mut self, entry: Entry, cx: &mut Context<Self>) {
        self.library.update(cx, |library, cx| library.open_in_viewer(entry, cx));
        self.layouts.update(cx, |layouts, cx| {
            layouts.reveal_editor(&super::viewer_id());
            cx.notify();
        });
    }

    fn click_tile(&mut self, entry: Entry, event: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        let modifiers = event.modifiers();
        if event.click_count() >= 2 {
            self.open(entry, cx);
            return;
        }
        self.library.update(cx, |library, cx| {
            if modifiers.shift {
                library.select_range(entry, cx);
            } else if modifiers.secondary() {
                library.toggle_selected(entry, cx);
            } else {
                library.select_only(entry, cx);
            }
        });
    }

    fn view_menu(&self, cx: &App) -> Vec<MenuEntry> {
        let settings = self.library.read(cx).settings().clone();
        let library = self.library.clone();
        let setting = move |label: &str, on: bool, change: fn(&mut sortier_library::library::Settings)| {
            let library = library.clone();
            MenuEntry::command(label.to_string(), move |_, cx| {
                library.update(cx, |library, cx| library.update_settings(change, cx));
            })
            .checked(on)
        };
        let sort_entries = [SortKey::Name, SortKey::Modified, SortKey::Added, SortKey::Size]
            .into_iter()
            .map(|key| {
                let library = self.library.clone();
                MenuEntry::command(key.label(), move |_, cx| {
                    library.update(cx, |library, cx| library.update_settings(|settings| settings.sort = key, cx));
                })
                .checked(settings.sort == key)
            })
            .collect();
        vec![
            setting("Sequences as One Tile", settings.group_view, |s| s.group_view = !s.group_view),
            setting("Crop Tiles to Fill", settings.tile_crop, |s| s.tile_crop = !s.tile_crop),
            MenuEntry::separator(),
            MenuEntry::submenu("Sort By", sort_entries),
            setting("Descending", settings.sort_desc, |s| s.sort_desc = !s.sort_desc),
            MenuEntry::separator(),
            setting("Larger Tiles", false, |s| s.tile_size = (s.tile_size + 20.0).min(400.0)),
            setting("Smaller Tiles", false, |s| s.tile_size = (s.tile_size - 20.0).max(100.0)),
        ]
    }

    fn status(&self, cx: &App) -> String {
        let library = self.library.read(cx);
        let mut parts = vec![count(library.results().len(), "item", "items")];
        if library.selection().len() > 1 {
            parts.push(format!("{} selected", library.selection().len()));
        }
        if library.is_scanning() {
            parts.push("Scanning folders\u{2026}".into());
        } else if library.thumbnails_left() > 0 {
            parts.push(format!("Making thumbnails, {} left", library.thumbnails_left()));
        } else if let Some(message) = library.message() {
            parts.push(message.to_string());
        }
        parts.join("  \u{b7}  ")
    }

    fn suggestions(&self, cx: &mut Context<Self>) -> Option<ToolbarStrip> {
        let typing = self.typing?;
        let field = match typing {
            Typing::Search => &self.search,
            Typing::Tags => &self.tags,
        };
        let text = field.read(cx).value().to_string();
        let library = self.library.read(cx);
        let present: BTreeSet<String> = match typing {
            Typing::Search => BTreeSet::new(),
            Typing::Tags if library.selection().len() == 1 => library.entry_tags(library.selection()[0]),
            Typing::Tags => BTreeSet::new(),
        };
        let mut tags = library.suggestions(&text, &present, 10);
        let label = if tags.is_empty() && typing == Typing::Tags && text.trim().is_empty() {
            tags = library.recent_tags().iter().filter(|tag| !present.contains(*tag)).cloned().collect();
            "Recent"
        } else {
            "Suggestions"
        };
        if tags.is_empty() {
            return None;
        }
        let mut row = PillRow::new().label(label);
        for (index, tag) in tags.into_iter().enumerate() {
            let field = field.clone();
            let entity = cx.entity().downgrade();
            let completed = complete(&text, &tag);
            row = row.pill(TagPill::new(("suggestion", index), tag).on_click(move |window, cx| {
                field.update(cx, |field, cx| {
                    field.set_value(completed.clone(), window, cx);
                    field.focus(window, cx);
                });
                if typing == Typing::Search {
                    let text = completed.clone();
                    entity
                        .update(cx, |this, cx| this.library.update(cx, |library, cx| library.set_query(&text, cx)))
                        .ok();
                }
            }));
        }
        Some(ToolbarStrip::new(row))
    }

    fn render_row(&self, row: usize, columns: usize, cx: &mut Context<Self>) -> gpui::Div {
        let library = self.library.read(cx);
        let side = library.settings().tile_size;
        let crop = library.settings().tile_crop;
        let start = row * columns;
        let end = (start + columns).min(library.results().len());
        let mut tiles = Vec::new();
        for index in start..end {
            let entry = library.results()[index];
            let cover = library.entry_cover(entry);
            let item = cover.and_then(|id| library.item(id));
            let kind = item.map(|item| item.kind).unwrap_or(MediaKind::Image);
            let mut tile = MediaTile::new(("tile", index), side, library.entry_title(entry))
                .selected(library.is_selected(entry))
                .cursor(library.cursor() == Some(entry));
            match library.shown_cover(entry).or_else(|| cover.and_then(|id| library.thumb(id))) {
                Some(path) => tile = tile.picture(thumbnail(path.clone(), crop, &self.thumbnails)),
                None if cover.is_some_and(|id| library.thumb_failed(id)) => {
                    tile = tile.placeholder(TilePlaceholder::Unavailable(match kind {
                        MediaKind::Video => IconKind::Video,
                        MediaKind::Document => IconKind::Book,
                        _ => IconKind::Image,
                    }));
                }
                None => {}
            }
            match entry {
                Entry::Collection(collection) => {
                    tile = tile.badge(Badge::icon_text(IconKind::Collection, library.volume_count(collection).to_string()));
                }
                Entry::Group(group) => {
                    let pages = library.group(group).map(|group| group.items.len()).unwrap_or(0);
                    tile = tile.badge(Badge::icon_text(IconKind::Sequence, pages.to_string()));
                }
                Entry::Item(id) => {
                    if let Some(group) = item.and_then(|item| item.group).and_then(|group| library.group(group)) {
                        let position = group.items.iter().position(|page| *page == id).unwrap_or(0) + 1;
                        tile = tile.badge(Badge::text(format!("{position}/{}", group.items.len())));
                    }
                }
            }
            match kind {
                MediaKind::Video => tile = tile.badge(Badge::icon(IconKind::Video)),
                MediaKind::Animated => tile = tile.badge(Badge::text("GIF")),
                MediaKind::Document => {
                    let pages = item.and_then(|item| item.pages).map(|pages| pages.to_string()).unwrap_or_default();
                    tile = tile.badge(Badge::icon_text(IconKind::Book, pages));
                }
                MediaKind::Image => {}
            }
            let entity = cx.entity().downgrade();
            tile = tile.on_click(move |event, window, cx| {
                entity.update(cx, |this, cx| this.click_tile(entry, event, window, cx)).ok();
            });
            tiles.push(tile);
        }
        div()
            .flex()
            .gap(sizes::TILE_GAP)
            .px(sizes::TILE_GAP)
            .pt(sizes::TILE_GAP)
            .children(tiles)
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// `text` with its last word replaced by `tag` (keeping a leading `-`), ready for the next word.
fn complete(text: &str, tag: &str) -> String {
    let (head, last) = match text.rfind(char::is_whitespace) {
        Some(at) => (&text[..=at], &text[at + 1..]),
        None => ("", text),
    };
    let negate = if last.starts_with('-') { "-" } else { "" };
    format!("{head}{negate}{tag} ")
}

impl Focusable for LibraryEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LibraryEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_selection = !self.library.read(cx).selection().is_empty();
        let mut toolbar = Toolbar::new();
        if let Some(collection) = self.library.read(cx).inside() {
            let library = self.library.clone();
            let name = self.library.read(cx).entry_title(Entry::Collection(collection));
            toolbar = toolbar
                .item(
                    IconButton::new("leave-collection", IconKind::ChevronLeft, "Back to Everything (\u{2318}\u{2191})")
                        .on_click(move |_, cx| library.update(cx, |library, cx| library.leave_collection(cx))),
                )
                .item(div().flex().items_center().gap(sizes::TILE_GAP).child(Icon::new(IconKind::Collection)).child(Label::new(name).size(TextSize::Small).truncate()));
        }
        let toolbar = toolbar
            .item(div().w(sizes::SEARCH_FIELD_WIDTH).min_w_0().flex_shrink(1.0).child(TextField::new(&self.search)))
            .item(Label::new(self.status(cx)).size(TextSize::Small).tone(TextTone::Secondary).truncate())
            .spacer()
            .item(div().w(sizes::DATA_BLOCK_WIDTH).child(TextField::new(&self.tags).disabled(!has_selection)))
            .item(
                DropdownButton::new("view-options", self.view_menu(cx))
                    .icon(IconKind::Settings)
                    .accessible_label("View Options"),
            );

        let library = self.library.read(cx);
        let body = if library.results().is_empty() {
            let empty_library = library.library().items.is_empty();
            let state = if empty_library && library.is_scanning() {
                EmptyState::new(IconKind::Sparkles, "Scanning folders", "Pictures appear as they're found.")
            } else if empty_library {
                EmptyState::new(
                    IconKind::Folder,
                    "Add a folder to start",
                    "Pictures, comics and videos in it (and its subfolders) are added. You can also drop a folder on the window.",
                )
                .action(
                    Button::new("add-folder", "Add Folder\u{2026}")
                        .icon(IconKind::FolderPlus)
                        .variant(ButtonVariant::Primary)
                        .on_click(|window, cx| window.dispatch_action(Box::new(AddFolder), cx)),
                )
            } else {
                EmptyState::new(IconKind::Untagged, "Nothing matches", "Take a tag out of the search, or clear it.")
            };
            state.into_any_element()
        } else {
            let tile_width = MediaTile::width(library.settings().tile_size, cx);
            let gap = to_px(sizes::TILE_GAP, cx);
            let available = self.width.get() - gap;
            let columns = ((f32::from(available) / f32::from(tile_width + gap)).floor() as usize).max(1);
            self.columns = columns;
            let rows = library.results().len().div_ceil(columns);
            let width = self.width.clone();
            div()
                .relative()
                .size_full()
                .child(
                    canvas(
                        move |bounds, window, _| {
                            if width.replace(bounds.size.width) != bounds.size.width {
                                window.refresh();
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .child(
                    uniform_list(
                        "library-grid",
                        rows,
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            range.map(|row| this.render_row(row, columns, cx)).collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .size_full()
                    .pb(sizes::TILE_GAP),
                )
                .into_any_element()
        };

        let rows_per_page = self.rows_per_page(window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(toolbar)
            .children(self.suggestions(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .track_focus(&self.focus_handle)
                    .key_context(KEY_CONTEXT)
                    .on_action(cx.listener(|this, _: &ShowVolumes, _, cx| {
                        let cursor = this.library.read(cx).cursor();
                        if let Some(Entry::Collection(collection)) = cursor {
                            this.library.update(cx, |library, cx| library.show_collection(collection, cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &LeaveCollection, _, cx| {
                        this.library.update(cx, |library, cx| library.leave_collection(cx))
                    }))
                    .on_action(cx.listener(|this, _: &Left, _, cx| this.move_cursor(-1, false, cx)))
                    .on_action(cx.listener(|this, _: &Right, _, cx| this.move_cursor(1, false, cx)))
                    .on_action(cx.listener(|this, _: &Up, _, cx| this.move_cursor(-(this.columns as isize), false, cx)))
                    .on_action(cx.listener(|this, _: &Down, _, cx| this.move_cursor(this.columns as isize, false, cx)))
                    .on_action(cx.listener(|this, _: &ExtendLeft, _, cx| this.move_cursor(-1, true, cx)))
                    .on_action(cx.listener(|this, _: &ExtendRight, _, cx| this.move_cursor(1, true, cx)))
                    .on_action(cx.listener(|this, _: &ExtendUp, _, cx| this.move_cursor(-(this.columns as isize), true, cx)))
                    .on_action(cx.listener(|this, _: &ExtendDown, _, cx| this.move_cursor(this.columns as isize, true, cx)))
                    .on_action(cx.listener(move |this, _: &PageUp, _, cx| {
                        this.move_cursor(-(this.columns as isize) * rows_per_page, false, cx)
                    }))
                    .on_action(cx.listener(move |this, _: &PageDown, _, cx| {
                        this.move_cursor(this.columns as isize * rows_per_page, false, cx)
                    }))
                    .on_action(cx.listener(|this, _: &First, _, cx| this.move_cursor(isize::MIN / 2, false, cx)))
                    .on_action(cx.listener(|this, _: &Last, _, cx| this.move_cursor(isize::MAX / 2, false, cx)))
                    .on_action(cx.listener(|this, _: &OpenCursor, _, cx| {
                        if let Some(entry) = this.library.read(cx).cursor() {
                            this.open(entry, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &Deselect, _, cx| {
                        this.library.update(cx, |library, cx| library.clear_selection(cx))
                    }))
                    .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                        this.search.update(cx, |field, cx| field.focus(window, cx))
                    }))
                    .on_action(cx.listener(|this, _: &FocusTags, window, cx| {
                        if !this.library.read(cx).selection().is_empty() {
                            this.tags.update(cx, |field, cx| field.focus(window, cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &ToggleTiles, _, cx| {
                        this.library.update(cx, |library, cx| library.update_settings(|s| s.group_view = !s.group_view, cx))
                    }))
                    .on_action(cx.listener(|this, _: &Bigger, _, cx| {
                        this.library.update(cx, |library, cx| {
                            library.update_settings(|s| s.tile_size = (s.tile_size + 20.0).min(400.0), cx)
                        })
                    }))
                    .on_action(cx.listener(|this, _: &Smaller, _, cx| {
                        this.library.update(cx, |library, cx| {
                            library.update_settings(|s| s.tile_size = (s.tile_size - 20.0).max(100.0), cx)
                        })
                    }))
                    .child(body),
            )
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.search.update(cx, |field, cx| field.focus(window, cx))
            }))
    }
}

pub struct LibraryFactory;

impl EditorFactory for LibraryFactory {
    fn id(&self) -> EditorTypeId {
        super::library_id()
    }

    fn name(&self) -> SharedString {
        "Library".into()
    }

    fn icon(&self) -> IconKind {
        IconKind::Grid
    }

    fn create(&self, context: &EditorContext, window: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|cx| LibraryEditor::new(context, window, cx)).into()
    }
}

#[cfg(test)]
mod tests {
    use super::complete;

    #[test]
    fn completion_replaces_the_last_word() {
        assert_eq!(complete("cat lu", "artist:luna"), "cat artist:luna ");
        assert_eq!(complete("-lu", "artist:luna"), "-artist:luna ");
        assert_eq!(complete("", "cat"), "cat ");
    }
}
