use std::borrow::Cow;

use gpui::{App, AssetSource, IntoElement, RenderOnce, Result, SharedString, Styled, Window};
use gpui_kit_assets::IconNamed;

use crate::tokens::sizes;

/// Every icon the application uses. Adding an icon means adding a variant here,
/// so icon names are checked at compile time. All of them come from the Lucide
/// catalogue bundled with gpui-kit (ISC licence).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconKind {
    Animated,
    Artist,
    /// A document: a PDF, an EPUB or a comic archive.
    Book,
    Browser,
    Character,
    Check,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Close,
    /// A collection of sequences, such as a series of volumes.
    Collection,
    /// Work done on the processor.
    Cpu,
    Download,
    /// A tag the search leaves out.
    ExcludedTag,
    External,
    Eye,
    Fit,
    Folder,
    FolderOpen,
    FolderPlus,
    Fullscreen,
    /// Work done on the graphics card.
    Gpu,
    Grid,
    Group,
    Heart,
    Image,
    ImageOff,
    ImagePlus,
    Images,
    Inspector,
    Minus,
    Pages,
    Pause,
    Play,
    Plus,
    ReadingDirection,
    Refresh,
    Repeat,
    Search,
    Sequence,
    /// A Sortier Remote server.
    Server,
    Settings,
    SkipBack,
    SkipForward,
    SortAscending,
    SortDescending,
    Sparkles,
    Spread,
    Tag,
    TagPlus,
    Tags,
    Trash,
    Ungroup,
    Untagged,
    Video,
    Viewer,
    Volume,
    VolumeOff,
    ZoomIn,
    ZoomOut,
}

impl IconKind {
    pub const ALL: [IconKind; 60] = [
        IconKind::Animated,
        IconKind::Artist,
        IconKind::Book,
        IconKind::Browser,
        IconKind::Character,
        IconKind::Check,
        IconKind::ChevronDown,
        IconKind::ChevronLeft,
        IconKind::ChevronRight,
        IconKind::Close,
        IconKind::Collection,
        IconKind::Cpu,
        IconKind::Download,
        IconKind::ExcludedTag,
        IconKind::External,
        IconKind::Eye,
        IconKind::Fit,
        IconKind::Folder,
        IconKind::FolderOpen,
        IconKind::FolderPlus,
        IconKind::Fullscreen,
        IconKind::Gpu,
        IconKind::Grid,
        IconKind::Group,
        IconKind::Heart,
        IconKind::Image,
        IconKind::ImageOff,
        IconKind::ImagePlus,
        IconKind::Images,
        IconKind::Inspector,
        IconKind::Minus,
        IconKind::Pages,
        IconKind::Pause,
        IconKind::Play,
        IconKind::Plus,
        IconKind::ReadingDirection,
        IconKind::Refresh,
        IconKind::Repeat,
        IconKind::Search,
        IconKind::Sequence,
        IconKind::Server,
        IconKind::Settings,
        IconKind::SkipBack,
        IconKind::SkipForward,
        IconKind::SortAscending,
        IconKind::SortDescending,
        IconKind::Sparkles,
        IconKind::Spread,
        IconKind::Tag,
        IconKind::TagPlus,
        IconKind::Tags,
        IconKind::Trash,
        IconKind::Ungroup,
        IconKind::Untagged,
        IconKind::Video,
        IconKind::Viewer,
        IconKind::Volume,
        IconKind::VolumeOff,
        IconKind::ZoomIn,
        IconKind::ZoomOut,
    ];
}

impl IconNamed for IconKind {
    fn path(self) -> SharedString {
        match self {
            IconKind::Animated => "icons/image-play.svg",
            IconKind::Artist => "icons/palette.svg",
            IconKind::Book => "icons/book-open.svg",
            IconKind::Browser => "icons/compass.svg",
            IconKind::Character => "icons/heart.svg",
            IconKind::Check => "icons/check.svg",
            IconKind::ChevronDown => "icons/chevron-down.svg",
            IconKind::ChevronLeft => "icons/chevron-left.svg",
            IconKind::ChevronRight => "icons/chevron-right.svg",
            IconKind::Close => "icons/x.svg",
            IconKind::Collection => "icons/library-big.svg",
            IconKind::Cpu => "icons/cpu.svg",
            IconKind::Gpu => "icons/gpu.svg",
            IconKind::Download => "icons/download.svg",
            IconKind::ExcludedTag => "icons/tag-x.svg",
            IconKind::External => "icons/external-link.svg",
            IconKind::Eye => "icons/eye.svg",
            IconKind::Fit => "icons/scan.svg",
            IconKind::Folder => "icons/folder-heart.svg",
            IconKind::FolderOpen => "icons/folder-open.svg",
            IconKind::FolderPlus => "icons/folder-plus.svg",
            IconKind::Fullscreen => "icons/fullscreen.svg",
            IconKind::Grid => "icons/layout-grid.svg",
            IconKind::Group => "icons/group.svg",
            IconKind::Heart => "icons/heart.svg",
            IconKind::Image => "icons/image.svg",
            IconKind::ImageOff => "icons/image-off.svg",
            IconKind::ImagePlus => "icons/image-plus.svg",
            IconKind::Images => "icons/images.svg",
            IconKind::Inspector => "icons/info.svg",
            IconKind::Minus => "icons/minus.svg",
            IconKind::Pages => "icons/gallery-horizontal-end.svg",
            IconKind::Pause => "icons/pause.svg",
            IconKind::Play => "icons/play.svg",
            IconKind::Plus => "icons/plus.svg",
            IconKind::ReadingDirection => "icons/arrow-right-left.svg",
            IconKind::Refresh => "icons/refresh-cw.svg",
            IconKind::Repeat => "icons/repeat.svg",
            IconKind::Search => "icons/search.svg",
            IconKind::Sequence => "icons/book-heart.svg",
            IconKind::Server => "icons/server.svg",
            IconKind::Settings => "icons/settings.svg",
            IconKind::SkipBack => "icons/skip-back.svg",
            IconKind::SkipForward => "icons/skip-forward.svg",
            IconKind::SortAscending => "icons/arrow-up-narrow-wide.svg",
            IconKind::SortDescending => "icons/arrow-down-wide-narrow.svg",
            IconKind::Sparkles => "icons/sparkles.svg",
            IconKind::Spread => "icons/book-open.svg",
            IconKind::Tag => "icons/tag.svg",
            IconKind::TagPlus => "icons/tag-plus.svg",
            IconKind::Tags => "icons/tags.svg",
            IconKind::Trash => "icons/trash.svg",
            IconKind::Ungroup => "icons/ungroup.svg",
            IconKind::Untagged => "icons/ghost.svg",
            IconKind::Video => "icons/clapperboard.svg",
            IconKind::Viewer => "icons/eye.svg",
            IconKind::Volume => "icons/volume-2.svg",
            IconKind::VolumeOff => "icons/volume-x.svg",
            IconKind::ZoomIn => "icons/zoom-in.svg",
            IconKind::ZoomOut => "icons/zoom-out.svg",
        }
        .into()
    }
}

/// Size step of an [`Icon`], from the icon size scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconSize {
    Small,
    #[default]
    Medium,
    Large,
    /// For the centre of an empty state.
    Huge,
}

/// An icon drawn in the current text colour.
#[derive(IntoElement)]
pub struct Icon {
    kind: IconKind,
    size: IconSize,
}

impl Icon {
    pub fn new(kind: IconKind) -> Self {
        Self {
            kind,
            size: IconSize::default(),
        }
    }

    pub fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let size = match self.size {
            IconSize::Small => sizes::ICON_SM,
            IconSize::Medium => sizes::ICON_MD,
            IconSize::Large => sizes::ICON_LG,
            IconSize::Huge => sizes::ICON_XL,
        };
        gpui_component::Icon::new(self.kind).size(size)
    }
}

/// The application's asset source: gpui-kit's complete icon catalogue, which
/// covers both [`IconKind`] and the icons gpui-component draws itself.
/// Register with `Application::with_assets` before running the app.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        gpui_kit_assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit_assets::AllAssets.list(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_kind_resolves_to_an_asset() {
        for kind in IconKind::ALL {
            let path = kind.path();
            let bytes = Assets.load(&path).unwrap();
            assert!(bytes.is_some_and(|bytes| !bytes.is_empty()), "{path} is missing");
        }
    }
}
