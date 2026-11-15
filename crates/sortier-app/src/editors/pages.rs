use gpui::{
    AnyView, App, AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, Window, div,
};
use sortier_areas::EditorTypeId;
use sortier_ui::button::IconButton;
use sortier_ui::fields::Toolbar;
use sortier_ui::gallery::{PageThumb, ThumbnailCache, thumbnail};
use sortier_ui::icon::IconKind;
use sortier_ui::label::{Label, TextSize, TextTone};
use sortier_ui::tokens::spacing;

use super::{EditorContext, EditorFactory};
use crate::model::{Entry, LibraryModel};

/// The pages of the open sequence in a strip: click one to show it, drag one
/// onto another to move it there.
pub struct PagesEditor {
    library: Entity<LibraryModel>,
    thumbnails: Entity<ThumbnailCache>,
    scroll: ScrollHandle,
    shown_page: Option<usize>,
    _observe: Subscription,
}

impl PagesEditor {
    fn new(context: &EditorContext, cx: &mut Context<Self>) -> Self {
        Self {
            library: context.library.clone(),
            thumbnails: ThumbnailCache::new(400, cx),
            scroll: ScrollHandle::new(),
            shown_page: None,
            _observe: cx.observe(&context.library, |this, library, cx| {
                let page = library.read(cx).open().map(|open| open.page);
                if page != this.shown_page {
                    this.shown_page = page;
                    if let Some(page) = page {
                        this.scroll.scroll_to_item(page);
                    }
                }
                cx.notify();
            }),
        }
    }

    fn with_current(&self, cx: &mut App, action: impl FnOnce(&mut LibraryModel, sortier_library::library::ItemId, &mut Context<LibraryModel>)) {
        let Some(id) = self.library.read(cx).current_page().map(|item| item.id) else { return };
        self.library.update(cx, |library, cx| action(library, id, cx));
    }
}

impl Render for PagesEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = self.library.read(cx);
        let open = library.open();
        let pages = library.current_pages();
        let in_sequence = library.current_page().is_some_and(|item| item.group.is_some());
        let title = match open {
            Some(open @ crate::model::Open { entry: Entry::Group(_) | Entry::Collection(_), .. }) => library.entry_title(open.entry),
            Some(_) => library
                .current_page()
                .and_then(|item| library.library().group_of(item))
                .map(|group| group.name.clone())
                .unwrap_or_else(|| library.current_page().map(|item| item.file_name()).unwrap_or_default()),
            None => String::new(),
        };
        let crop = library.settings().tile_crop;
        let entity = cx.entity().downgrade();
        let action = |id: &'static str, icon: IconKind, label: &'static str, run: fn(&mut LibraryModel, sortier_library::library::ItemId, &mut Context<LibraryModel>)| {
            let entity = entity.clone();
            IconButton::new(id, icon, label).disabled(!in_sequence).on_click(move |_, cx| {
                entity.update(cx, |this, cx| this.with_current(cx, run)).ok();
            })
        };
        let toolbar = Toolbar::new()
            .item(Label::new(title).size(TextSize::Small).truncate())
            .item(
                Label::new(if pages.len() == 1 { "1 page".to_string() } else { format!("{} pages", pages.len()) })
                    .size(TextSize::Small)
                    .tone(TextTone::Secondary),
            )
            .spacer()
            .item(action("earlier", IconKind::ChevronLeft, "Move Page Earlier ([)", |library, id, cx| {
                library.move_page_by(id, -1, cx)
            }))
            .item(action("later", IconKind::ChevronRight, "Move Page Later (])", |library, id, cx| {
                library.move_page_by(id, 1, cx)
            }))
            .item(action("cover", IconKind::Image, "Make This the Cover (C)", |library, id, cx| library.make_cover(id, cx)))
            .item(action("remove", IconKind::Ungroup, "Take Out of the Sequence", |library, id, cx| {
                library.remove_from_sequence(id, cx)
            }));

        let current = open.map(|open| open.page);
        let spread = library.spread_active();
        let thumbs: Vec<PageThumb> = pages
            .iter()
            .enumerate()
            .map(|(index, page)| {
                let library_entity = self.library.clone();
                let drop_library = self.library.clone();
                let onto = page.item;
                let mut thumb = PageThumb::new(("page", index), page.item, index + 1)
                    .current(current == Some(index) || (spread && current.map(|page| page + 1) == Some(index)))
                    .on_select(move |_, cx| library_entity.update(cx, |library, cx| library.go_to_page(index, cx)))
                    .on_drop(move |drag, _, cx| drop_library.update(cx, |library, cx| library.move_page_onto(drag.key, onto, cx)));
                // Thumbnails are made of whole files, which for a document is its first page.
                if page.index == 0
                    && let Some(path) = library.thumb(page.item)
                {
                    thumb = thumb.picture(thumbnail(path.clone(), crop, &self.thumbnails));
                }
                thumb
            })
            .collect();

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(
                div()
                    .id("page-strip")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .gap(spacing::SM)
                    .p(spacing::SM)
                    .overflow_x_scroll()
                    .track_scroll(&self.scroll)
                    .children(thumbs),
            )
    }
}

pub struct PagesFactory;

impl EditorFactory for PagesFactory {
    fn id(&self) -> EditorTypeId {
        super::pages_id()
    }

    fn name(&self) -> SharedString {
        "Pages".into()
    }

    fn icon(&self) -> IconKind {
        IconKind::Pages
    }

    fn create(&self, context: &EditorContext, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|cx| PagesEditor::new(context, cx)).into()
    }
}
