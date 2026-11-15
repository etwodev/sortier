use std::collections::BTreeSet;

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    PathPromptOptions, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, Window, div,
};
use sortier_areas::EditorTypeId;
use sortier_library::library::{CollectionId, ItemId, MediaKind};
use sortier_ui::button::Button;
use sortier_ui::fields::{PropertyPanel, PropertyRow, ValueText};
use sortier_ui::gallery::{PillRow, TagPill};
use sortier_ui::icon::IconKind;
use sortier_ui::text_field::{TextField, TextFieldEvent, TextFieldState};
use sortier_ui::tokens::spacing;

use super::{EditorContext, EditorFactory};
use crate::model::{Entry, LibraryModel};

/// Which field of the inspector a tag field writes to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TagTarget {
    Selection,
    Page,
}

/// Properties of the selection: what it is, its tags (add and remove), its
/// sequence's name, and the tags of the page open in the viewers.
pub struct InspectorEditor {
    library: Entity<LibraryModel>,
    tags: Entity<TextFieldState>,
    page_tags: Entity<TextFieldState>,
    name: Entity<TextFieldState>,
    /// The sequence or collection the name field names.
    naming: Option<Entry>,
    typing: Option<TagTarget>,
    collapsed: BTreeSet<&'static str>,
    _subscriptions: Vec<Subscription>,
}

impl InspectorEditor {
    fn new(context: &EditorContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tags = cx.new(|cx| TextFieldState::new(window, cx).placeholder("Add tags, separated by spaces"));
        let page_tags = cx.new(|cx| TextFieldState::new(window, cx).placeholder("Add tags to this page only"));
        let name = cx.new(|cx| TextFieldState::new(window, cx));
        let subscriptions = vec![
            cx.observe_in(&context.library, window, |this, _, window, cx| {
                this.sync_name(window, cx);
                cx.notify();
            }),
            cx.subscribe_in(&tags, window, |this, field, event: &TextFieldEvent, window, cx| {
                this.tag_event(TagTarget::Selection, field, event, window, cx)
            }),
            cx.subscribe_in(&page_tags, window, |this, field, event: &TextFieldEvent, window, cx| {
                this.tag_event(TagTarget::Page, field, event, window, cx)
            }),
            cx.subscribe_in(&name, window, |this, field, event: &TextFieldEvent, _, cx| {
                if matches!(event, TextFieldEvent::PressEnter { .. } | TextFieldEvent::Blur)
                    && let Some(entry) = this.naming
                {
                    let name = field.read(cx).value().to_string();
                    this.library.update(cx, |library, cx| library.rename_entry(entry, &name, cx));
                }
            }),
        ];
        let mut this = Self {
            library: context.library.clone(),
            tags,
            page_tags,
            name,
            naming: None,
            typing: None,
            collapsed: BTreeSet::new(),
            _subscriptions: subscriptions,
        };
        this.sync_name(window, cx);
        this
    }

    /// Shows the selected sequence's or collection's name in the name field when the selection changes.
    fn sync_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = match self.library.read(cx).selection() {
            [entry @ (Entry::Group(_) | Entry::Collection(_))] => Some(*entry),
            _ => None,
        };
        if entry != self.naming {
            self.naming = entry;
            let name = entry.map(|entry| self.library.read(cx).entry_title(entry)).unwrap_or_default();
            self.name.update(cx, |field, cx| field.set_value(name, window, cx));
        }
    }

    fn tag_event(&mut self, target: TagTarget, field: &Entity<TextFieldState>, event: &TextFieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TextFieldEvent::PressEnter { .. } => {
                let text = field.read(cx).value().to_string();
                self.apply_tags(target, &text, cx);
                field.update(cx, |field, cx| field.set_value("", window, cx));
            }
            TextFieldEvent::Focus => self.typing = Some(target),
            TextFieldEvent::Blur => self.typing = self.typing.filter(|typing| *typing != target),
            TextFieldEvent::Change => {}
        }
        cx.notify();
    }

    fn apply_tags(&self, target: TagTarget, text: &str, cx: &mut App) {
        self.library.update(cx, |library, cx| match target {
            TagTarget::Selection => {
                let selection = library.selection().to_vec();
                library.add_tags(&selection, text, cx);
            }
            TagTarget::Page => {
                if let Some(page) = library.current_page().map(|item| item.id) {
                    library.add_page_tags(page, text, cx);
                }
            }
        });
    }

    fn panel(&self, key: &'static str, title: impl Into<SharedString>, cx: &mut Context<Self>) -> PropertyPanel {
        let entity = cx.entity().downgrade();
        PropertyPanel::new(key, title).collapsed(self.collapsed.contains(key)).on_toggle(move |_, cx| {
            entity
                .update(cx, |this, cx| {
                    if !this.collapsed.remove(key) {
                        this.collapsed.insert(key);
                    }
                    cx.notify();
                })
                .ok();
        })
    }

    /// Suggestions for the field being typed in, or recent tags when it's empty.
    fn suggestions(&self, target: TagTarget, present: &BTreeSet<String>, cx: &App) -> Option<AnyElement> {
        if self.typing != Some(target) {
            return None;
        }
        let field = match target {
            TagTarget::Selection => &self.tags,
            TagTarget::Page => &self.page_tags,
        };
        let text = field.read(cx).value().to_string();
        let library = self.library.read(cx);
        let mut tags = library.suggestions(&text, present, 8);
        let mut label = "Suggestions";
        if tags.is_empty() && text.trim().is_empty() {
            tags = library.recent_tags().iter().filter(|tag| !present.contains(*tag)).cloned().collect();
            label = "Recent";
        }
        if tags.is_empty() {
            return None;
        }
        let mut row = PillRow::new().label(label);
        for (index, tag) in tags.into_iter().enumerate() {
            let field = field.clone();
            let completed = complete(&text, &tag);
            row = row.pill(TagPill::new(("suggest", index), tag).on_click(move |window, cx| {
                field.update(cx, |field, cx| {
                    field.set_value(completed.clone(), window, cx);
                    field.focus(window, cx);
                });
            }));
        }
        Some(row.into_any_element())
    }

    fn tag_pills(&self, key: &'static str, tags: Vec<(String, usize)>, of: usize, remove: impl Fn(&str, &mut App) + Clone + 'static) -> AnyElement {
        let mut row = PillRow::new();
        for (index, (tag, count)) in tags.into_iter().enumerate() {
            let remove = remove.clone();
            let removed = tag.clone();
            let mut pill = TagPill::new((key, index), tag).on_remove(move |_, cx| remove(&removed, cx));
            if of > 1 && count < of {
                pill = pill.count(format!("{count}/{of}"));
            }
            row = row.pill(pill);
        }
        row.into_any_element()
    }

    fn file_panel(&self, id: ItemId, cx: &mut Context<Self>) -> Option<PropertyPanel> {
        let item = self.library.read(cx).item(id)?.clone();
        let kind = match item.kind {
            MediaKind::Image => "Picture",
            MediaKind::Animated => "Animation",
            MediaKind::Video => "Video",
            MediaKind::Document => "Document",
        };
        let folder = item.path.parent().map(|path| path.display().to_string()).unwrap_or_default();
        let (reveal, open) = (self.library.clone(), self.library.clone());
        let remote = self.library.read(cx).remote_access().is_some();
        let mut panel = self
            .panel("file", "File", cx)
            .row(PropertyRow::new("Name", ValueText::new(item.file_name())))
            .row(PropertyRow::new("Kind", ValueText::new(kind)));
        if let Some((width, height)) = item.dims {
            panel = panel.row(PropertyRow::new("Size", ValueText::new(format!("{width} \u{d7} {height}"))));
        }
        if let Some(pages) = item.pages.filter(|_| item.kind == MediaKind::Document) {
            panel = panel.row(PropertyRow::new("Pages", ValueText::new(pages.to_string())));
        }
        panel = panel
            .row(PropertyRow::new("On Disk", ValueText::new(human_size(item.size))))
            .row(PropertyRow::new("Folder", ValueText::new(folder)))
            .row(
                div()
                    .flex()
                    .gap(spacing::XS)
                    .child(
                        Button::new("reveal", "Show in Folder")
                            .icon(IconKind::FolderOpen)
                            .disabled(remote)
                            .on_click(move |_, cx| reveal.update(cx, |library, cx| library.show_in_folder(id, cx))),
                    )
                    .child(
                        Button::new("open", "Open in Default App")
                            .icon(IconKind::External)
                            .on_click(move |_, cx| open.update(cx, |library, cx| library.open_in_default_app(id, cx))),
                    ),
            );
        Some(panel)
    }
}

impl InspectorEditor {
    /// A collection's name, how many volumes it has, and going into it or dissolving it.
    fn collection_panel(&self, collection: CollectionId, cx: &mut Context<Self>) -> PropertyPanel {
        let volumes = self.library.read(cx).volume_count(collection);
        let (show, dissolve) = (self.library.clone(), self.library.clone());
        self.panel("collection", "Collection", cx)
            .row(PropertyRow::new("Name", TextField::new(&self.name)))
            .row(PropertyRow::new("Volumes", ValueText::new(volumes.to_string())))
            .row(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(spacing::XS)
                    .child(
                        Button::new("show-volumes", "Show Volumes")
                            .icon(IconKind::Collection)
                            .on_click(move |_, cx| show.update(cx, |library, cx| library.show_collection(collection, cx))),
                    )
                    .child(
                        Button::new("dissolve-collection", "Dissolve")
                            .icon(IconKind::Ungroup)
                            .on_click(move |_, cx| dissolve.update(cx, |library, cx| library.dissolve_collection(collection, cx))),
                    ),
            )
    }

    /// While the results show a collection's volumes: moving the selected volume along it, or taking volumes out.
    fn volume_panel(&self, selection: &[Entry], cx: &mut Context<Self>) -> Option<PropertyPanel> {
        self.library.read(cx).inside()?;
        let single = selection.len() == 1;
        let (earlier, later, remove) = (self.library.clone(), self.library.clone(), self.library.clone());
        let panel = self.panel("volume", if single { "Volume" } else { "Volumes" }, cx).row(
            div()
                .flex()
                .flex_wrap()
                .gap(spacing::XS)
                .child(
                    Button::new("volume-earlier", "Earlier")
                        .icon(IconKind::ChevronLeft)
                        .disabled(!single)
                        .on_click(move |_, cx| earlier.update(cx, |library, cx| library.move_volume(-1, cx))),
                )
                .child(
                    Button::new("volume-later", "Later")
                        .icon(IconKind::ChevronRight)
                        .disabled(!single)
                        .on_click(move |_, cx| later.update(cx, |library, cx| library.move_volume(1, cx))),
                )
                .child(
                    Button::new("volume-remove", "Take Out")
                        .icon(IconKind::Ungroup)
                        .on_click(move |_, cx| remove.update(cx, |library, cx| library.remove_from_collection(cx))),
                ),
        );
        Some(panel)
    }

    /// Choosing `entry`'s cover: a picture (or video, or document) from disk, the page open in the viewers, or back
    /// to its first page.
    fn cover_panel(&self, entry: Entry, cx: &mut Context<Self>) -> PropertyPanel {
        let (custom, page_open) = {
            let library = self.library.read(cx);
            (library.custom_cover(entry).is_some(), library.open_page().is_some())
        };
        let (choose, use_page, reset) = (self.library.clone(), self.library.clone(), self.library.clone());
        self.panel("cover", "Cover", cx).row(
            div()
                .flex()
                .flex_wrap()
                .gap(spacing::XS)
                .child(Button::new("cover-choose", "Choose Picture\u{2026}").icon(IconKind::ImagePlus).on_click(move |_, cx| {
                    let chosen = cx.prompt_for_paths(PathPromptOptions {
                        files: true,
                        directories: false,
                        multiple: false,
                        prompt: Some("Use as Cover".into()),
                    });
                    let library = choose.clone();
                    cx.spawn(async move |cx| {
                        if let Ok(Ok(Some(paths))) = chosen.await
                            && let Some(path) = paths.into_iter().next()
                        {
                            library.update(cx, |library, cx| library.cover_from_file(entry, path, cx));
                        }
                    })
                    .detach();
                }))
                .child(
                    Button::new("cover-page", "Use Open Page")
                        .icon(IconKind::Image)
                        .disabled(!page_open)
                        .on_click(move |_, cx| use_page.update(cx, |library, cx| library.cover_from_open_page(entry, cx))),
                )
                .child(
                    Button::new("cover-reset", "Reset")
                        .icon(IconKind::Refresh)
                        .disabled(!custom)
                        .on_click(move |_, cx| reset.update(cx, |library, cx| library.set_cover(entry, None, cx))),
                ),
        )
    }
}

/// `text` with its last word replaced by `tag`, ready for the next word.
fn complete(text: &str, tag: &str) -> String {
    let head = match text.rfind(char::is_whitespace) {
        Some(at) => &text[..=at],
        None => "",
    };
    format!("{head}{tag} ")
}

fn human_size(bytes: u64) -> String {
    let size = bytes as f64;
    if size >= 1e9 {
        format!("{:.1} GB", size / 1e9)
    } else if size >= 1e6 {
        format!("{:.1} MB", size / 1e6)
    } else if size >= 1e3 {
        format!("{:.0} KB", size / 1e3)
    } else {
        format!("{bytes} bytes")
    }
}

impl Render for InspectorEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut panels: Vec<AnyElement> = Vec::new();
        let (selection, open, current_page) = {
            let library = self.library.read(cx);
            (library.selection().to_vec(), library.open(), library.current_page().map(|item| (item.id, item.group)))
        };

        if selection.is_empty() {
            let (files, sequences, tags, folders) = {
                let library = self.library.read(cx);
                let lib = library.library();
                (lib.items.len(), lib.groups.len(), library.tag_counts().len(), lib.roots.len())
            };
            let panel = self
                .panel("library", "Library", cx)
                .row(PropertyRow::new("Files", ValueText::new(files.to_string())))
                .row(PropertyRow::new("Sequences", ValueText::new(sequences.to_string())))
                .row(PropertyRow::new("Tags", ValueText::new(tags.to_string())))
                .row(PropertyRow::new("Folders", ValueText::new(folders.to_string())));
            panels.push(panel.into_any_element());
        } else {
            let tags = self.library.read(cx).selection_tags();
            let present: BTreeSet<String> = tags.iter().map(|(tag, _)| tag.clone()).collect();
            let total = selection.len();
            if let [entry] = selection.as_slice() {
                let entry = *entry;
                let title = self.library.read(cx).entry_title(entry);
                if let Entry::Collection(collection) = entry {
                    panels.push(self.collection_panel(collection, cx).into_any_element());
                } else if let Entry::Group(group) = entry {
                    let pages = self.library.read(cx).group(group).map(|group| group.items.len()).unwrap_or(0);
                    let panel = self
                        .panel("sequence", "Sequence", cx)
                        .row(PropertyRow::new("Name", TextField::new(&self.name)))
                        .row(PropertyRow::new("Pages", ValueText::new(pages.to_string())));
                    panels.push(panel.into_any_element());
                } else if let Some(panel) = self.file_panel(self.library.read(cx).entry_cover(entry).unwrap_or_default(), cx) {
                    panels.push(panel.into_any_element());
                }
                panels.push(self.cover_panel(entry, cx).into_any_element());
                let _ = title;
            } else {
                let files: usize = selection.iter().map(|entry| self.library.read(cx).entry_items(*entry).len()).sum();
                let any_sequence = selection.iter().any(|entry| match entry {
                    Entry::Group(_) => true,
                    Entry::Item(id) => self.library.read(cx).item(*id).is_some_and(|item| item.group.is_some()),
                    Entry::Collection(_) => false,
                });
                let (make, split, collect) = (self.library.clone(), self.library.clone(), self.library.clone());
                let panel = self
                    .panel("selection", "Selection", cx)
                    .row(PropertyRow::new("Selected", ValueText::new(format!("{total} items, {files} files"))))
                    .row(
                        div()
                            .flex()
                            .gap(spacing::XS)
                            .child(Button::new("make-sequence", "Make Sequence").icon(IconKind::Group).on_click(move |_, cx| {
                                make.update(cx, |library, cx| library.make_sequence(cx))
                            }))
                            .child(
                                Button::new("split", "Split Sequences").icon(IconKind::Ungroup).disabled(!any_sequence).on_click(
                                    move |_, cx| split.update(cx, |library, cx| library.split_sequences(cx)),
                                ),
                            )
                            .child(Button::new("make-collection", "Make Collection").icon(IconKind::Collection).on_click(
                                move |_, cx| collect.update(cx, |library, cx| library.make_collection(cx)),
                            )),
                    );
                panels.push(panel.into_any_element());
            }
            if let Some(panel) = self.volume_panel(&selection, cx) {
                panels.push(panel.into_any_element());
            }

            let library = self.library.clone();
            let remove_from = selection.clone();
            let pills = self.tag_pills("selection-tag", tags, total, move |tag, cx| {
                let entries = remove_from.clone();
                library.update(cx, |library, cx| library.remove_tag(&entries, tag, cx));
            });
            let mut panel = self.panel("tags", if total > 1 { "Tags on the Selection" } else { "Tags" }, cx).row(pills).row(TextField::new(&self.tags));
            if let Some(suggestions) = self.suggestions(TagTarget::Selection, &present, cx) {
                panel = panel.row(suggestions);
            }
            panels.push(panel.into_any_element());
        }

        // The page open in the viewers, when it's part of the selected sequence.
        if let (Some(open), Some((page, Some(_)))) = (open, current_page)
            && selection.contains(&open.entry)
            && matches!(open.entry, Entry::Group(_) | Entry::Collection(_))
        {
            let tags: Vec<(String, usize)> = self
                .library
                .read(cx)
                .item(page)
                .map(|item| item.tags.iter().map(|tag| (tag.clone(), 1)).collect())
                .unwrap_or_default();
            let present: BTreeSet<String> = tags.iter().map(|(tag, _)| tag.clone()).collect();
            let library = self.library.clone();
            let pills = self.tag_pills("page-tag", tags, 1, move |tag, cx| {
                library.update(cx, |library, cx| library.remove_page_tag(page, tag, cx));
            });
            let mut panel = self.panel("page", format!("Page {}", open.page + 1), cx).row(pills).row(TextField::new(&self.page_tags));
            if let Some(suggestions) = self.suggestions(TagTarget::Page, &present, cx) {
                panel = panel.row(suggestions);
            }
            panels.push(panel.into_any_element());
            if let Some(file) = self.file_panel(page, cx) {
                panels.push(file.into_any_element());
            }
        }

        div()
            .id("inspector")
            .size_full()
            .overflow_y_scroll()
            .p(spacing::SM)
            .flex()
            .flex_col()
            .gap(spacing::SM)
            .children(panels)
    }
}

pub struct InspectorFactory;

impl EditorFactory for InspectorFactory {
    fn id(&self) -> EditorTypeId {
        super::inspector_id()
    }

    fn name(&self) -> SharedString {
        "Inspector".into()
    }

    fn icon(&self) -> IconKind {
        IconKind::Inspector
    }

    fn create(&self, context: &EditorContext, window: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|cx| InspectorEditor::new(context, window, cx)).into()
    }
}
