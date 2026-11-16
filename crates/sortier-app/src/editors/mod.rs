//! The editors areas can show, and the registry the area host finds them in.

mod browser;
mod inspector;
pub(crate) use inspector::human_size;
mod library;
mod pages;
mod viewer;

use std::rc::Rc;

use gpui::{AnyView, App, Entity, Global, SharedString, Window};
use sortier_areas::EditorTypeId;
use sortier_ui::icon::IconKind;

use crate::layouts::LayoutModel;
use crate::model::LibraryModel;
use crate::render_service::RenderService;

pub use browser::BrowserFactory;
pub use inspector::InspectorFactory;
pub use library::LibraryFactory;
pub use pages::PagesFactory;
pub use viewer::ViewerFactory;

/// What every editor is created with: the shared library, the workspace layouts
/// (so opening an item can bring a viewer forward), and the GPU renderer.
#[derive(Clone)]
pub struct EditorContext {
    pub library: Entity<LibraryModel>,
    pub layouts: Entity<LayoutModel>,
    pub renderer: RenderService,
}

/// One kind of editor that an area can show. Implement this and register it
/// with the [`EditorRegistry`] to make a new editor available everywhere.
pub trait EditorFactory: 'static {
    fn id(&self) -> EditorTypeId;
    /// Name shown in the area header's editor-type selector.
    fn name(&self) -> SharedString;
    fn icon(&self) -> IconKind;
    /// Creates a new view of this editor. Each area showing the editor gets its
    /// own view, so per-area state (scroll, zoom) is independent, while the
    /// library and selection in `context` are shared.
    fn create(&self, context: &EditorContext, window: &mut Window, cx: &mut App) -> AnyView;
}

/// The editors available to areas, in the order the selector lists them.
/// Installed as a GPUI global at startup.
#[derive(Clone, Default)]
pub struct EditorRegistry {
    factories: Vec<Rc<dyn EditorFactory>>,
}

impl Global for EditorRegistry {}

impl EditorRegistry {
    pub fn get(&self, id: &EditorTypeId) -> Option<Rc<dyn EditorFactory>> {
        self.factories.iter().find(|factory| &factory.id() == id).cloned()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Rc<dyn EditorFactory>> {
        self.factories.iter()
    }
}

pub fn library_id() -> EditorTypeId {
    EditorTypeId::new("sortier.library")
}

pub fn viewer_id() -> EditorTypeId {
    EditorTypeId::new("sortier.viewer")
}

pub fn pages_id() -> EditorTypeId {
    EditorTypeId::new("sortier.pages")
}

pub fn browser_id() -> EditorTypeId {
    EditorTypeId::new("sortier.browser")
}

pub fn inspector_id() -> EditorTypeId {
    EditorTypeId::new("sortier.inspector")
}

/// Every built-in editor, in selector order.
pub fn builtin_registry() -> EditorRegistry {
    EditorRegistry {
        factories: vec![
            Rc::new(LibraryFactory),
            Rc::new(ViewerFactory),
            Rc::new(PagesFactory),
            Rc::new(BrowserFactory),
            Rc::new(InspectorFactory),
        ],
    }
}

/// Keyboard shortcuts of the built-in editors, each active only while its
/// editor has focus.
pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    let mut bindings = library::key_bindings();
    bindings.extend(viewer::key_bindings());
    bindings
}
