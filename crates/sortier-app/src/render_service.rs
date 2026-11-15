use std::cell::RefCell;
use std::rc::Rc;

use gpui::SharedString;
use sortier_render::MediaRenderer;

/// The one wgpu renderer every viewer shares (textures are uploaded once and
/// drawn by any view), or why it couldn't start.
#[derive(Clone)]
pub struct RenderService(Result<Rc<RefCell<MediaRenderer>>, SharedString>);

impl RenderService {
    /// Starts the renderer on the fastest GPU. A failure is kept and shown by
    /// each viewer instead of its picture.
    pub fn start() -> Self {
        Self(
            MediaRenderer::new()
                .map(|renderer| Rc::new(RefCell::new(renderer)))
                .map_err(|error| format!("{error:#}").into()),
        )
    }

    pub fn renderer(&self) -> Result<&Rc<RefCell<MediaRenderer>>, &SharedString> {
        self.0.as_ref()
    }
}
