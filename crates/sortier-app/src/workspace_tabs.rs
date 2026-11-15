use gpui::{
    Context, ElementId, Entity, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, Styled,
    Subscription, WeakEntity, Window, div,
};
use sortier_areas::WorkspaceId;
use sortier_ui::icon::IconKind;
use sortier_ui::menu::{DropdownButton, MenuEntry};
use sortier_ui::tab::Tab;
use sortier_ui::text_field::{TextField, TextFieldEvent, TextFieldState, text_field_state};
use sortier_ui::tokens::spacing;

use crate::layouts::{LayoutModel, WorkspaceTemplate};

/// A workspace tab being renamed in place.
struct Renaming {
    workspace: WorkspaceId,
    field: Entity<TextFieldState>,
    _events: Subscription,
}

/// The workspace tab strip: switch, create, rename, reorder and delete
/// workspaces.
pub struct WorkspaceTabs {
    layouts: Entity<LayoutModel>,
    renaming: Option<Renaming>,
    _project_changed: Subscription,
}

impl WorkspaceTabs {
    pub fn new(layouts: Entity<LayoutModel>, cx: &mut Context<Self>) -> Self {
        let layouts_changed = cx.observe(&layouts, |this, layouts, cx| {
            let renamed_still_exists = this
                .renaming
                .as_ref()
                .is_none_or(|renaming| layouts.read(cx).workspaces().get(renaming.workspace).is_some());
            if !renamed_still_exists {
                this.renaming = None;
            }
            cx.notify();
        });
        Self {
            layouts,
            renaming: None,
            _project_changed: layouts_changed,
        }
    }

    pub fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            cx.notify();
        }
    }

    fn start_rename(&mut self, workspace: WorkspaceId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.layouts.read(cx).workspaces().get(workspace).map(|w| w.name.clone()) else {
            return;
        };
        let field = text_field_state(name, true, window, cx);
        let events = cx.subscribe_in(&field, window, |this, _, event: &TextFieldEvent, _, cx| {
            if matches!(event, TextFieldEvent::PressEnter { .. } | TextFieldEvent::Blur) {
                this.commit_rename(cx);
            }
        });
        self.renaming = Some(Renaming {
            workspace,
            field,
            _events: events,
        });
        cx.notify();
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let name = renaming.field.read(cx).value().to_string();
        self.layouts.update(cx, |layouts, cx| {
            let current = layouts.workspaces().get(renaming.workspace).map(|w| w.name.clone());
            // An empty or unchanged name leaves the tab as it was.
            if !name.trim().is_empty() && current.as_deref() != Some(name.trim()) {
                let _ = layouts.edit_workspaces(|workspaces| workspaces.rename(renaming.workspace, &name));
                cx.notify();
            }
        });
        cx.notify();
    }

    fn activate(&mut self, workspace: WorkspaceId, cx: &mut Context<Self>) {
        self.layouts.update(cx, |layouts, cx| {
            layouts.activate_workspace(workspace);
            cx.notify();
        });
    }

    fn move_to(&mut self, workspace: WorkspaceId, index: usize, cx: &mut Context<Self>) {
        self.layouts.update(cx, |layouts, cx| {
            if layouts.workspaces().index_of(workspace) != Some(index) {
                let _ = layouts.edit_workspaces(|workspaces| workspaces.move_to(workspace, index));
                cx.notify();
            }
        });
    }

    fn duplicate(&mut self, workspace: WorkspaceId, cx: &mut Context<Self>) {
        self.layouts.update(cx, |layouts, cx| {
            let Some(source) = layouts.workspaces().get(workspace).cloned() else {
                return;
            };
            layouts.edit_workspaces(|workspaces| {
                workspaces.activate(workspace).ok();
                workspaces.add(&source.name, source.layout)
            });
            cx.notify();
        });
    }

    fn add_from_template(&mut self, template: WorkspaceTemplate, cx: &mut Context<Self>) {
        self.layouts.update(cx, |layouts, cx| {
            layouts.edit_workspaces(|workspaces| workspaces.add(template.name(), template.layout()));
            cx.notify();
        });
    }

    fn delete(&mut self, workspace: WorkspaceId, cx: &mut Context<Self>) {
        self.layouts.update(cx, |layouts, cx| {
            // Deleting the last workspace is refused; its menu entry is disabled.
            if layouts.edit_workspaces(|workspaces| workspaces.remove(workspace)).is_ok() {
                cx.notify();
            }
        });
    }

    fn tab_menu(&self, workspace: WorkspaceId, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let workspaces = self.layouts.read(cx).workspaces();
        let index = workspaces.index_of(workspace).unwrap_or(0);
        let count = workspaces.len();
        let tabs = cx.entity().downgrade();
        vec![
            tab_command(&tabs, "Rename", move |this, window, cx| this.start_rename(workspace, window, cx)),
            tab_command(&tabs, "Duplicate", move |this, _, cx| this.duplicate(workspace, cx)),
            MenuEntry::separator(),
            tab_command(&tabs, "Move Left", move |this, _, cx| {
                this.move_to(workspace, index.saturating_sub(1), cx)
            })
            .disabled(index == 0),
            tab_command(&tabs, "Move Right", move |this, _, cx| this.move_to(workspace, index + 1, cx))
                .disabled(index + 1 >= count),
            MenuEntry::separator(),
            tab_command(&tabs, "Delete", move |this, _, cx| this.delete(workspace, cx)).disabled(count <= 1),
        ]
    }

    fn add_menu(&self, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let tabs = cx.entity().downgrade();
        let active = self.layouts.read(cx).workspaces().active();
        let mut entries = vec![
            MenuEntry::command("Duplicate Current", {
                let tabs = tabs.clone();
                move |_, cx| {
                    tabs.update(cx, |this, cx| this.duplicate(active, cx)).ok();
                }
            }),
            MenuEntry::separator(),
            MenuEntry::heading("New From Template"),
        ];
        for template in WorkspaceTemplate::ALL {
            let tabs = tabs.clone();
            entries.push(MenuEntry::command(template.name(), move |_, cx| {
                tabs.update(cx, |this, cx| this.add_from_template(template, cx)).ok();
            }));
        }
        entries
    }
}

fn tab_command(
    tabs: &WeakEntity<WorkspaceTabs>,
    label: &'static str,
    action: impl Fn(&mut WorkspaceTabs, &mut Window, &mut Context<WorkspaceTabs>) + 'static,
) -> MenuEntry {
    let tabs = tabs.clone();
    MenuEntry::command(label, move |window, cx| {
        tabs.update(cx, |this, cx| action(this, window, cx)).ok();
    })
}

impl Render for WorkspaceTabs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs_entity = cx.entity().downgrade();
        let workspaces: Vec<(WorkspaceId, String)> = self
            .layouts
            .read(cx)
            .workspaces()
            .iter()
            .map(|workspace| (workspace.id, workspace.name.clone()))
            .collect();
        let active = self.layouts.read(cx).workspaces().active();

        let mut strip = div().flex().items_center().gap(spacing::XXS);
        for (index, (id, name)) in workspaces.into_iter().enumerate() {
            let key = u64::from(id.0);
            let mut tab = Tab::new(ElementId::NamedInteger("workspace-tab".into(), key), key, name)
                .active(id == active)
                .on_select({
                    let tabs = tabs_entity.clone();
                    move |_, cx| {
                        tabs.update(cx, |this, cx| this.activate(id, cx)).ok();
                    }
                })
                .on_rename({
                    let tabs = tabs_entity.clone();
                    move |window, cx| {
                        tabs.update(cx, |this, cx| this.start_rename(id, window, cx)).ok();
                    }
                })
                .on_drop({
                    let tabs = tabs_entity.clone();
                    move |drag, _, cx| {
                        let dragged = WorkspaceId(drag.key as u32);
                        tabs.update(cx, |this, cx| this.move_to(dragged, index, cx)).ok();
                    }
                })
                .context_menu({
                    let tabs = tabs_entity.clone();
                    move |_, cx| {
                        tabs.update(cx, |this, cx| this.tab_menu(id, cx)).unwrap_or_default()
                    }
                });
            if let Some(renaming) = self.renaming.as_ref().filter(|renaming| renaming.workspace == id) {
                tab = tab.editor(
                    div()
                        .w(sortier_ui::tokens::sizes::TAB_EDITOR_WIDTH)
                        .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                            if event.keystroke.key == "escape" {
                                this.cancel_rename(cx);
                                cx.stop_propagation();
                            }
                        }))
                        .child(TextField::new(&renaming.field)),
                );
            }
            strip = strip.child(tab);
        }
        strip.child(
            DropdownButton::new("add-workspace", self.add_menu(cx))
                .icon(IconKind::Plus)
                .accessible_label("Add workspace"),
        )
    }
}
