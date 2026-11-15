use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, Subscription, UniformListScrollHandle, Window, div, uniform_list,
};
use sortier_areas::EditorTypeId;
use sortier_library::library::MediaKind;
use sortier_ui::button::IconButton;
use sortier_ui::fields::Toolbar;
use sortier_ui::gallery::tag_dot;
use sortier_ui::icon::IconKind;
use sortier_ui::label::{Label, TextSize, TextTone};
use sortier_ui::menu::{MenuEntry, with_context_menu};
use sortier_ui::text_field::{TextField, TextFieldEvent, TextFieldState};
use sortier_ui::tree_row::{Expansion, TreeRow};

use super::{EditorContext, EditorFactory};
use crate::menus::{AddFolder, Rescan};
use crate::model::{LibraryModel, namespace_title, tag_order};

/// What a row of the browser stands for.
#[derive(Clone)]
enum Row {
    Section { title: &'static str, icon: IconKind },
    Filter { label: &'static str, query: &'static str, icon: IconKind, count: usize },
    Folder(PathBuf),
    Tag { tag: String, count: usize },
}

/// The library's filters, folders and tags as a tree. Clicking a tag adds it
/// to the search (again takes it out); Alt-click or the context menu excludes it.
pub struct BrowserEditor {
    library: Entity<LibraryModel>,
    filter: Entity<TextFieldState>,
    collapsed: HashSet<&'static str>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl BrowserEditor {
    fn new(context: &EditorContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| TextFieldState::new(window, cx).placeholder("Filter tags"));
        let subscriptions = vec![
            cx.observe(&context.library, |_, _, cx| cx.notify()),
            cx.subscribe(&filter, |_, _, event: &TextFieldEvent, cx| {
                if matches!(event, TextFieldEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        Self {
            library: context.library.clone(),
            filter,
            collapsed: HashSet::new(),
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        let library = self.library.read(cx);
        let lib = library.library();
        let count_kind = |kind: MediaKind| lib.items.values().filter(|item| item.kind == kind).count();
        let untagged = lib
            .items
            .values()
            .filter(|item| item.tags.is_empty() && lib.group_of(item).is_none_or(|group| group.tags.is_empty()))
            .count();
        let mut rows = vec![Row::Section {
            title: "Library",
            icon: IconKind::Sparkles,
        }];
        if !self.collapsed.contains("Library") {
            rows.extend([
                Row::Filter { label: "Everything", query: "", icon: IconKind::Images, count: lib.items.len() },
                Row::Filter { label: "Untagged", query: "is:untagged", icon: IconKind::Untagged, count: untagged },
                Row::Filter { label: "Sequences", query: "is:seq", icon: IconKind::Sequence, count: lib.groups.len() },
                Row::Filter { label: "Collections", query: "is:collection", icon: IconKind::Collection, count: lib.collections.len() },
                Row::Filter { label: "Videos", query: "type:video", icon: IconKind::Video, count: count_kind(MediaKind::Video) },
                Row::Filter { label: "Animated", query: "type:gif", icon: IconKind::Animated, count: count_kind(MediaKind::Animated) },
                Row::Filter { label: "Documents", query: "type:doc", icon: IconKind::Book, count: count_kind(MediaKind::Document) },
            ]);
        }
        rows.push(Row::Section {
            title: "Folders",
            icon: IconKind::Folder,
        });
        if !self.collapsed.contains("Folders") {
            rows.extend(lib.roots.iter().cloned().map(Row::Folder));
        }
        let filter = self.filter.read(cx).value().trim().to_lowercase();
        let mut tags: Vec<(String, usize)> =
            library.tag_counts().iter().filter(|(tag, _)| filter.is_empty() || tag.contains(&filter)).cloned().collect();
        tags.sort_by(|a, b| tag_order(&a.0, &b.0));
        let mut section = None;
        for (tag, count) in tags {
            let title = namespace_title(&tag);
            if section != Some(title) {
                section = Some(title);
                let icon = match title {
                    "Artists" => IconKind::Artist,
                    "Series" => IconKind::Sequence,
                    "Characters" => IconKind::Character,
                    _ => IconKind::Tags,
                };
                rows.push(Row::Section { title, icon });
            }
            if !self.collapsed.contains(title) {
                rows.push(Row::Tag { tag, count });
            }
        }
        rows
    }

    fn toggle_section(&mut self, title: &'static str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(title) {
            self.collapsed.insert(title);
        }
        cx.notify();
    }

    fn render_row(&self, index: usize, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity().downgrade();
        let library = self.library.read(cx);
        let count_label = |count: usize| Label::new(count.to_string()).size(TextSize::Small).tone(TextTone::Secondary);
        match row {
            Row::Section { title, icon } => {
                let title = *title;
                let expansion = if self.collapsed.contains(title) { Expansion::Collapsed } else { Expansion::Expanded };
                let toggle = entity.clone();
                TreeRow::new(("section", index), 0, title, *icon)
                    .expansion(expansion)
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |this, cx| this.toggle_section(title, cx)).ok();
                    })
                    .on_toggle_expansion(move |_, cx| {
                        toggle.update(cx, |this, cx| this.toggle_section(title, cx)).ok();
                    })
                    .into_any_element()
            }
            Row::Filter { label, query, icon, count } => {
                let query = *query;
                let model = self.library.clone();
                TreeRow::new(("filter", index), 1, *label, *icon)
                    .selected(library.query().trim() == query)
                    .trailing(count_label(*count))
                    .on_click(move |_, _, cx| model.update(cx, |library, cx| library.set_query(query, cx)))
                    .into_any_element()
            }
            Row::Folder(path) => {
                let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
                let reveal = path.clone();
                let menu_path = path.clone();
                let model = self.library.clone();
                let row = TreeRow::new(("folder", index), 1, name, IconKind::Folder)
                    .on_double_click(move |_, cx| cx.reveal_path(&reveal));
                with_context_menu(div().id(ElementId::NamedInteger("folder-row".into(), index as u64)).child(row), move |_, _| {
                    let show = menu_path.clone();
                    let remove = menu_path.clone();
                    let model = model.clone();
                    vec![
                        MenuEntry::command("Show in Folder", move |_, cx| cx.reveal_path(&show)).icon(IconKind::FolderOpen),
                        MenuEntry::separator(),
                        MenuEntry::command("Remove From Library", move |_, cx| {
                            model.update(cx, |library, cx| library.remove_folder(&remove, cx));
                        })
                        .icon(IconKind::Trash),
                    ]
                })
            }
            Row::Tag { tag, count } => {
                let state = library.query_tag_state(tag);
                let name = match tag.split_once(':') {
                    Some((_, name)) if namespace_title(tag) != "Other" => name.to_string(),
                    _ => tag.clone(),
                };
                let icon = if state == Some(true) { IconKind::ExcludedTag } else { IconKind::Tag };
                let click_tag = tag.clone();
                let model = self.library.clone();
                let row = TreeRow::new(("tag", index), 1, name, icon)
                    .selected(state == Some(false))
                    .muted(state == Some(true))
                    .trailing(div().flex().items_center().gap(sortier_ui::tokens::spacing::XS).child(count_label(*count)).child(tag_dot(tag, cx)))
                    .on_click(move |modifiers, _, cx| {
                        let exclude = modifiers.alt;
                        model.update(cx, |library, cx| library.toggle_query_tag(&click_tag, exclude, cx));
                    });
                let menu_tag = tag.clone();
                let model = self.library.clone();
                with_context_menu(div().id(ElementId::NamedInteger("tag-row".into(), index as u64)).child(row), move |_, cx| {
                    let state = model.read(cx).query_tag_state(&menu_tag);
                    let (include, exclude) = (menu_tag.clone(), menu_tag.clone());
                    let (include_model, exclude_model) = (model.clone(), model.clone());
                    vec![
                        MenuEntry::command("Show Only With This Tag", move |_, cx| {
                            include_model.update(cx, |library, cx| library.toggle_query_tag(&include, false, cx));
                        })
                        .checked(state == Some(false)),
                        MenuEntry::command("Hide Everything With This Tag", move |_, cx| {
                            exclude_model.update(cx, |library, cx| library.toggle_query_tag(&exclude, true, cx));
                        })
                        .checked(state == Some(true)),
                    ]
                })
            }
        }
    }
}

impl Render for BrowserEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let toolbar = Toolbar::new()
            .item(div().flex_1().min_w_0().child(TextField::new(&self.filter)))
            .item(IconButton::new("add-folder", IconKind::FolderPlus, "Add Folder").on_click(|window, cx| {
                window.dispatch_action(Box::new(AddFolder), cx)
            }))
            .item(IconButton::new("rescan", IconKind::Refresh, "Rescan Folders (F5)").on_click(|window, cx| {
                window.dispatch_action(Box::new(Rescan), cx)
            }));
        let count = rows.len();
        div().size_full().flex().flex_col().child(toolbar).child(
            div().flex_1().min_h_0().child(
                uniform_list(
                    "browser-rows",
                    count,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        range.map(|index| this.render_row(index, &rows[index], cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full(),
            ),
        )
    }
}

pub struct BrowserFactory;

impl EditorFactory for BrowserFactory {
    fn id(&self) -> EditorTypeId {
        super::browser_id()
    }

    fn name(&self) -> SharedString {
        "Browser".into()
    }

    fn icon(&self) -> IconKind {
        IconKind::Browser
    }

    fn create(&self, context: &EditorContext, window: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|cx| BrowserEditor::new(context, window, cx)).into()
    }
}
