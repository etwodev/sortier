//! Design tokens: the single source of colour, spacing, type, radius and size
//! values for the whole application. Components read the semantic layer
//! ([`SemanticColors`]) and the scales below, never raw palette values.

use gpui::{App, Hsla, Pixels, Rems, SharedString, rems, rgb};

/// The body text size at 100% UI scale, in pixels. Every size token is a
/// multiple of it (in rems), so the whole interface scales with the theme's
/// font size.
pub const BASE_FONT: f32 = 13.0;

/// A size designed as `pixels` at 100% UI scale.
const fn scaled(pixels: f32) -> Rems {
    rems(pixels / BASE_FONT)
}

/// A size token in pixels at the current UI scale, for painting and
/// hit-testing.
pub fn to_px(value: Rems, cx: &App) -> Pixels {
    value.to_pixels(gpui_component::Theme::global(cx).font_size)
}

/// An opaque sRGB colour stored as `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(u32);

impl Color {
    pub const fn from_hex(rgb: u32) -> Self {
        Self(rgb & 0x00ff_ffff)
    }

    pub fn hsla(self) -> Hsla {
        rgb(self.0).into()
    }

    pub fn hsla_with_alpha(self, alpha: f32) -> Hsla {
        Hsla {
            a: alpha,
            ..self.hsla()
        }
    }

    /// `#rrggbb`, the form gpui-component's theme configuration parses.
    pub fn hex(self) -> SharedString {
        format!("#{:06x}", self.0).into()
    }

    /// Channels as `0.0..=1.0`, for passing to the renderer.
    pub fn rgb_f32(self) -> [f32; 3] {
        [(self.0 >> 16) & 0xff, (self.0 >> 8) & 0xff, self.0 & 0xff].map(|channel| channel as f32 / 255.0)
    }
}

/// Which variant of the semantic palette is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

/// Raw palette. Only [`SemanticColors::for_appearance`] may reference these.
mod palette {
    use super::Color;

    pub const NEUTRAL_0: Color = Color::from_hex(0xffffff);
    pub const NEUTRAL_50: Color = Color::from_hex(0xf5f5f7);
    pub const NEUTRAL_100: Color = Color::from_hex(0xececef);
    pub const NEUTRAL_150: Color = Color::from_hex(0xe7e7ea);
    pub const NEUTRAL_200: Color = Color::from_hex(0xe0e0e5);
    pub const NEUTRAL_250: Color = Color::from_hex(0xd6d6dc);
    pub const NEUTRAL_300: Color = Color::from_hex(0xd4d4d9);
    pub const NEUTRAL_350: Color = Color::from_hex(0xc9c9ce);
    pub const NEUTRAL_380: Color = Color::from_hex(0xa8a8b0);
    pub const NEUTRAL_400: Color = Color::from_hex(0xa0a0a8);
    pub const NEUTRAL_500: Color = Color::from_hex(0x6c6c73);
    pub const NEUTRAL_550: Color = Color::from_hex(0x5e5e66);
    pub const NEUTRAL_650: Color = Color::from_hex(0x46464c);
    pub const NEUTRAL_800: Color = Color::from_hex(0x333337);
    pub const NEUTRAL_900: Color = Color::from_hex(0x1f1f21);
    pub const NEUTRAL_950: Color = Color::from_hex(0x161618);
    pub const NEUTRAL_975: Color = Color::from_hex(0x1d1d1f);
    pub const NEUTRAL_LIGHT_TEXT: Color = Color::from_hex(0xe8e8ea);
    pub const NEUTRAL_LIGHT_TEXT_MUTED: Color = Color::from_hex(0xa6a6ad);

    pub const BLUE_300: Color = Color::from_hex(0x6ea0ff);
    pub const BLUE_400: Color = Color::from_hex(0x5a8de0);
    pub const BLUE_650: Color = Color::from_hex(0x2f6ad0);

    /// Sortier's own palette: three greys and two pinks. The in-between
    /// shades are mixes of them, for surfaces and states the five don't cover.
    pub const INK_950: Color = Color::from_hex(0x0d0d0d);
    pub const INK_850: Color = Color::from_hex(0x1f1f1f);
    /// Between INK_850 and INK_700: area headers and panels.
    pub const INK_800: Color = Color::from_hex(0x272727);
    pub const INK_700: Color = Color::from_hex(0x383838);
    pub const INK_600: Color = Color::from_hex(0x4a4a4a);
    pub const INK_500: Color = Color::from_hex(0x5e5e5e);
    pub const ROSE_500: Color = Color::from_hex(0xed2553);
    pub const ROSE_400: Color = Color::from_hex(0xee4972);
    /// ROSE_500 darkened, for pressed accent controls.
    pub const ROSE_600: Color = Color::from_hex(0xc91d45);
    /// ROSE_500 mixed 30% into INK_850: selected rows and text selections.
    pub const ROSE_TINT_DARK: Color = Color::from_hex(0x5c212f);
    pub const ROSE_TINT_LIGHT: Color = Color::from_hex(0xfbd3dd);

    pub const RED_400: Color = Color::from_hex(0xf0716a);
    pub const RED_500: Color = Color::from_hex(0xe5534b);
    pub const RED_600: Color = Color::from_hex(0xc9372c);
    pub const AMBER_400: Color = Color::from_hex(0xf2c14e);
    pub const AMBER_500: Color = Color::from_hex(0xd7a13b);
    pub const AMBER_600: Color = Color::from_hex(0xb7791f);
    pub const GREEN_400: Color = Color::from_hex(0x6cc684);
    pub const GREEN_500: Color = Color::from_hex(0x4fae6a);
    pub const GREEN_600: Color = Color::from_hex(0x2f8a4a);
    pub const GREEN_TINT_DARK: Color = Color::from_hex(0x2f4a36);
    pub const GREEN_TINT_LIGHT: Color = Color::from_hex(0xd3ecd9);
    pub const AMBER_TINT_DARK: Color = Color::from_hex(0x584a24);
    pub const AMBER_TINT_LIGHT: Color = Color::from_hex(0xf6e6b4);
    pub const VIOLET_400: Color = Color::from_hex(0xa98bf0);
    pub const VIOLET_600: Color = Color::from_hex(0x7656c9);
    pub const CYAN_400: Color = Color::from_hex(0x4fc3d9);
    pub const CYAN_600: Color = Color::from_hex(0x1f8ea3);
    /// Onion-skin tints: these multiply layer colours on the canvas, so they
    /// stay the same in both appearances.
    pub const ONION_PAST: Color = Color::from_hex(0xff6a5c);
    pub const ONION_FUTURE: Color = Color::from_hex(0x5cd38a);

    /// Node editor header tints by category: input, output, colour, filter,
    /// matte, distort, converter, group, texture.
    pub const NODE_HEADERS_DARK: [Color; 9] = [
        Color::from_hex(0x7a3446),
        Color::from_hex(0x6b2c2c),
        Color::from_hex(0x6a6630),
        Color::from_hex(0x4d3d78),
        Color::from_hex(0x6a4a2c),
        Color::from_hex(0x2c5776),
        Color::from_hex(0x2c665e),
        Color::from_hex(0x2f6638),
        Color::from_hex(0x7a3a66),
    ];
    pub const NODE_HEADERS_LIGHT: [Color; 9] = [
        Color::from_hex(0xebbdc8),
        Color::from_hex(0xe5b5b5),
        Color::from_hex(0xe7e2a6),
        Color::from_hex(0xd0c4ec),
        Color::from_hex(0xedd0b2),
        Color::from_hex(0xb8d5ec),
        Color::from_hex(0xb3e1d9),
        Color::from_hex(0xbfe3c5),
        Color::from_hex(0xe8c2de),
    ];
    pub const SOCKET_YELLOW_DARK: Color = Color::from_hex(0xc7c729);
    pub const SOCKET_YELLOW_LIGHT: Color = Color::from_hex(0x9e9e14);
}

/// Semantic colour roles. Every colour a component paints comes from one of these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SemanticColors {
    /// Window title bar and tab strip.
    pub chrome: Color,
    /// Gaps between areas.
    pub seam: Color,
    /// Area bodies and the window background.
    pub background: Color,
    /// Area headers and panels.
    pub surface: Color,
    /// Menus, popovers and dialogs.
    pub surface_raised: Color,
    pub border: Color,
    pub border_strong: Color,
    /// Faint outline around each area.
    pub area_border: Color,
    /// Area outline while a seam that resizes the area is hovered or dragged.
    pub area_border_active: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_disabled: Color,
    /// Subtle background for hovered controls.
    pub hover: Color,
    /// Subtle background for pressed controls.
    pub pressed: Color,
    pub accent: Color,
    pub accent_hover: Color,
    pub accent_pressed: Color,
    pub accent_foreground: Color,
    /// Background of selected rows and text selections.
    pub selection: Color,
    pub focus_ring: Color,
    pub danger: Color,
    pub danger_foreground: Color,
    pub warning: Color,
    pub success: Color,
    /// Dims the window behind modal overlays.
    pub scrim: Color,
    /// Viewport area outside the canvas.
    pub canvas_backdrop: Color,
    /// Checkerboard shown through transparent canvas areas.
    pub checker_light: Color,
    pub checker_dark: Color,
    /// Keyframes that aren't selected, in the timeline and graph editor.
    pub keyframe: Color,
    /// Selected keyframes and handles.
    pub keyframe_selected: Color,
    /// A property with a key on the current frame (field tint and marker).
    pub keyed: Color,
    pub keyed_tint: Color,
    /// An animated property without a key on the current frame.
    pub animated: Color,
    pub animated_tint: Color,
    /// The current-frame line and its label.
    pub playhead: Color,
    pub playhead_foreground: Color,
    /// Camera frames drawn in the viewport.
    pub camera_frame: Color,
    /// Colours of curves in the graph editor, assigned in turn.
    pub curves: [Color; 6],
    /// What onion skinning tints past and future frames with.
    pub onion_past: Color,
    pub onion_future: Color,
    /// Node editor headers by category; see `NodeCategory` in the node
    /// editor primitives.
    pub node_headers: [Color; 9],
    /// Sockets carrying colours and numbers.
    pub socket_color: Color,
    pub socket_value: Color,
    /// Links between nodes, and the node editor's background grid.
    pub link: Color,
    pub node_grid: Color,
    /// The node editor's background, behind the grid.
    pub node_canvas: Color,
    /// A node's body, below its title bar.
    pub node_body: Color,
    /// The line around nodes and sockets.
    pub node_outline: Color,
    /// Values drawn as bars, on nodes and in the colour picker.
    pub value_field: Color,
}

impl SemanticColors {
    pub fn for_appearance(appearance: Appearance) -> Self {
        use palette::*;
        match appearance {
            Appearance::Dark => Self {
                chrome: INK_950,
                seam: INK_950,
                background: INK_850,
                surface: INK_800,
                surface_raised: INK_800,
                border: INK_700,
                border_strong: INK_600,
                area_border: INK_800,
                area_border_active: INK_500,
                text_primary: NEUTRAL_LIGHT_TEXT,
                text_secondary: NEUTRAL_LIGHT_TEXT_MUTED,
                text_disabled: NEUTRAL_500,
                hover: INK_700,
                pressed: INK_600,
                accent: ROSE_500,
                accent_hover: ROSE_400,
                accent_pressed: ROSE_600,
                accent_foreground: NEUTRAL_0,
                selection: ROSE_TINT_DARK,
                focus_ring: ROSE_400,
                danger: RED_500,
                danger_foreground: NEUTRAL_0,
                warning: AMBER_500,
                success: GREEN_500,
                scrim: INK_950,
                canvas_backdrop: INK_950,
                checker_light: INK_700,
                checker_dark: INK_800,
                keyframe: NEUTRAL_LIGHT_TEXT_MUTED,
                keyframe_selected: AMBER_400,
                keyed: AMBER_400,
                keyed_tint: AMBER_TINT_DARK,
                animated: GREEN_400,
                animated_tint: GREEN_TINT_DARK,
                playhead: BLUE_400,
                playhead_foreground: NEUTRAL_0,
                camera_frame: NEUTRAL_LIGHT_TEXT,
                curves: [RED_400, GREEN_400, BLUE_300, AMBER_400, VIOLET_400, CYAN_400],
                onion_past: ONION_PAST,
                onion_future: ONION_FUTURE,
                node_headers: NODE_HEADERS_DARK,
                socket_color: SOCKET_YELLOW_DARK,
                socket_value: NEUTRAL_380,
                link: NEUTRAL_LIGHT_TEXT_MUTED,
                node_grid: NEUTRAL_800,
                node_canvas: NEUTRAL_900,
                node_body: NEUTRAL_800,
                node_outline: NEUTRAL_950,
                value_field: NEUTRAL_650,
            },
            Appearance::Light => Self {
                chrome: NEUTRAL_150,
                seam: NEUTRAL_350,
                background: NEUTRAL_50,
                surface: NEUTRAL_100,
                surface_raised: NEUTRAL_0,
                border: NEUTRAL_300,
                border_strong: NEUTRAL_400,
                area_border: NEUTRAL_300,
                area_border_active: NEUTRAL_380,
                text_primary: NEUTRAL_975,
                text_secondary: NEUTRAL_550,
                text_disabled: NEUTRAL_400,
                hover: NEUTRAL_200,
                pressed: NEUTRAL_250,
                accent: ROSE_500,
                accent_hover: ROSE_400,
                accent_pressed: ROSE_600,
                accent_foreground: NEUTRAL_0,
                selection: ROSE_TINT_LIGHT,
                focus_ring: ROSE_500,
                danger: RED_600,
                danger_foreground: NEUTRAL_0,
                warning: AMBER_600,
                success: GREEN_600,
                scrim: NEUTRAL_975,
                canvas_backdrop: NEUTRAL_250,
                checker_light: NEUTRAL_0,
                checker_dark: NEUTRAL_200,
                keyframe: NEUTRAL_550,
                keyframe_selected: AMBER_600,
                keyed: AMBER_600,
                keyed_tint: AMBER_TINT_LIGHT,
                animated: GREEN_600,
                animated_tint: GREEN_TINT_LIGHT,
                playhead: BLUE_650,
                playhead_foreground: NEUTRAL_0,
                camera_frame: NEUTRAL_975,
                curves: [RED_600, GREEN_600, BLUE_650, AMBER_600, VIOLET_600, CYAN_600],
                onion_past: ONION_PAST,
                onion_future: ONION_FUTURE,
                node_headers: NODE_HEADERS_LIGHT,
                socket_color: SOCKET_YELLOW_LIGHT,
                socket_value: NEUTRAL_500,
                link: NEUTRAL_550,
                node_grid: NEUTRAL_300,
                node_canvas: NEUTRAL_100,
                node_body: NEUTRAL_0,
                node_outline: NEUTRAL_380,
                value_field: NEUTRAL_100,
            },
        }
    }
}

/// Spacing scale on a 4px grid. All padding, margins and gaps use these.
pub mod spacing {
    use super::{Rems, scaled};

    pub const XXS: Rems = scaled(2.);
    pub const XS: Rems = scaled(4.);
    pub const SM: Rems = scaled(8.);
    pub const MD: Rems = scaled(12.);
    pub const LG: Rems = scaled(16.);
    pub const XL: Rems = scaled(24.);
    pub const XXL: Rems = scaled(32.);
}

/// Type scale. One family (the platform UI font), three sizes.
pub mod typography {
    use super::{Rems, scaled};

    pub const SMALL: Rems = scaled(12.);
    pub const BODY: Rems = scaled(13.);
    pub const HEADING: Rems = scaled(16.);
}

/// Corner radius scale. Generous on purpose: Sortier should feel soft, so
/// panels and pictures are well rounded and small controls are pills
/// (`rounded_full`).
pub mod radius {
    use super::{Rems, scaled};

    pub const SM: Rems = scaled(6.);
    pub const MD: Rems = scaled(9.);
    pub const LG: Rems = scaled(14.);
}

/// Fixed component and icon sizes.
pub mod sizes {
    use super::{Pixels, Rems, scaled};

    /// gpui-component positions the macOS traffic lights for this height, so
    /// it doesn't scale with the UI.
    pub const TITLE_BAR_HEIGHT: Pixels = gpui_component::TITLE_BAR_HEIGHT;
    pub const TOP_BAR_HEIGHT: Rems = scaled(34.);
    pub const AREA_HEADER_HEIGHT: Rems = scaled(32.);
    pub const TAB_HEIGHT: Rems = scaled(24.);
    pub const PROGRESS_WIDTH: Rems = scaled(320.);
    /// Width of the field shown while renaming a tab.
    pub const TAB_EDITOR_WIDTH: Rems = scaled(120.);
    pub const TOOLBAR_HEIGHT: Rems = scaled(36.);
    pub const TREE_ROW_HEIGHT: Rems = scaled(26.);
    /// Space around a tree row's rounded highlight, so rows read as pills.
    pub const TREE_ROW_INSET: Rems = scaled(6.);
    /// Height of a timeline row showing a clip's pictures or sound.
    pub const CLIP_ROW_HEIGHT: Rems = scaled(40.);
    pub const TREE_INDENT: Rems = scaled(14.);
    pub const PROPERTY_LABEL_WIDTH: Rems = scaled(96.);
    /// Side of the square transform handles in the viewport.
    pub const GIZMO_HANDLE: Rems = scaled(8.);
    /// Distance from a layer's top edge to its rotate handle.
    pub const GIZMO_ROTATE_OFFSET: Rems = scaled(24.);
    /// Checkerboard square size behind transparent canvas areas.
    pub const CHECKER: Rems = scaled(8.);
    /// Gap between neighbouring areas; the seam colour shows through it, so
    /// areas sit on the window like cards.
    pub const AREA_SEAM: Rems = scaled(6.);
    /// How far either side of a seam still grabs it.
    pub const SEAM_HIT_SLOP: Rems = scaled(2.);
    pub const AREA_MIN_WIDTH: Rems = scaled(120.);
    pub const AREA_MIN_HEIGHT: Rems = scaled(60.);
    /// Thickness of the preview line shown while choosing where to split an area.
    pub const SPLIT_GUIDE: Rems = scaled(2.);
    /// Height of the frame-number ruler in the timeline and graph editor.
    pub const RULER_HEIGHT: Rems = scaled(22.);
    /// Height of the timeline's bar showing which frames are ready to play.
    pub const CACHE_BAR_HEIGHT: Rems = scaled(3.);
    /// Width of the channel list beside the timeline and graph editor.
    pub const CHANNEL_LIST_WIDTH: Rems = scaled(200.);
    /// Side of a keyframe diamond.
    pub const KEYFRAME: Rems = scaled(10.);
    /// Diameter of a Bézier handle's end in the graph editor.
    pub const CURVE_HANDLE: Rems = scaled(7.);
    /// The length of each dash, and each gap, in a dashed guide line.
    pub const GUIDE_DASH: Rems = scaled(4.);
    /// The tallest a list of chips grows before it scrolls.
    pub const CHIP_LIST_MAX_HEIGHT: Rems = scaled(64.);
    /// Width of a slider in a toolbar.
    pub const TOOLBAR_SLIDER_WIDTH: Rems = scaled(120.);
    pub const PLAYHEAD_WIDTH: Rems = scaled(2.);
    /// Width of the current-frame label on the ruler.
    pub const PLAYHEAD_LABEL_WIDTH: Rems = scaled(36.);
    /// Size of the triangle marking a camera's top edge.
    pub const CAMERA_MARKER: Rems = scaled(12.);
    /// How close, in view pixels, a click must be to a camera's frame to pick it.
    pub const CAMERA_PICK: Rems = scaled(6.);
    /// Smallest gap between labelled frame numbers on a ruler.
    pub const RULER_LABEL_SPACING: Rems = scaled(56.);
    /// Gap between a timeline clip bar and the edges of its row.
    pub const CLIP_INSET: Rems = scaled(3.);
    /// Width of the section list down the side of the Preferences window.
    pub const NAV_LIST_WIDTH: Rems = scaled(168.);
    /// Height of a row in a list of choices (sections, recent projects).
    pub const LIST_ROW_HEIGHT: Rems = scaled(28.);
    /// Width of the splash shown at startup and by File › New.
    pub const SPLASH_WIDTH: Rems = scaled(680.);
    /// Width of a node in a node editor, at 100% zoom.
    pub const NODE_WIDTH: Rems = scaled(184.);
    /// Height of a node's title bar.
    pub const NODE_HEADER: Rems = scaled(26.);
    /// Height of each row of a node: a socket, a setting or a value.
    pub const NODE_ROW: Rems = scaled(24.);
    /// Room above a node's first row and below its last.
    pub const NODE_PADDING: Rems = scaled(6.);
    /// Diameter of a node's socket dot.
    pub const SOCKET: Rems = scaled(10.);
    /// Spacing of the node editor's background grid.
    pub const NODE_GRID: Rems = scaled(24.);
    /// Size of a search menu's panels: the first (with the search field
    /// and results), the submenus beside it, and how tall any may grow.
    pub const SEARCH_MENU_WIDTH: Rems = scaled(240.);
    pub const SEARCH_SUBMENU_WIDTH: Rems = scaled(200.);
    pub const SEARCH_MENU_HEIGHT: Rems = scaled(360.);
    /// Height of a row in a search menu.
    pub const SEARCH_MENU_ROW: Rems = scaled(24.);
    /// Width of the field naming an editor's data, such as a set of effects.
    pub const DATA_BLOCK_WIDTH: Rems = scaled(200.);
    /// Width of the choice of whose effects an editor shows.
    pub const MODE_SELECT_WIDTH: Rems = scaled(84.);
    /// The colour picker's hue and saturation wheel, and the value bar
    /// beside it.
    pub const COLOR_WHEEL: Rems = scaled(168.);
    pub const COLOR_VALUE_BAR: Rems = scaled(16.);
    /// Height of a value row in the colour picker.
    pub const COLOR_FIELD_ROW: Rems = scaled(22.);
    /// Height of a colour swatch that opens the picker.
    pub const COLOR_SWATCH: Rems = scaled(18.);
    /// Diameter of the ring marking the colour on the wheel.
    pub const COLOR_CURSOR: Rems = scaled(12.);
    /// Width of the frame fields in the timeline header.
    pub const FRAME_FIELD_WIDTH: Rems = scaled(56.);
    /// The sound level meter in the timeline header: its width, the height
    /// of each channel's bar, and the width of the peak-hold mark.
    pub const LEVEL_METER_WIDTH: Rems = scaled(96.);
    pub const LEVEL_METER_BAR: Rems = scaled(4.);
    pub const LEVEL_METER_HOLD: Rems = scaled(2.);
    /// Height of a tile's name under its picture in a library grid.
    pub const TILE_LABEL_HEIGHT: Rems = scaled(18.);
    /// Space between tiles, and around the edge of the grid.
    pub const TILE_GAP: Rems = scaled(16.);
    /// Height of a badge over a picture.
    pub const BADGE_HEIGHT: Rems = scaled(18.);
    /// How far the glow around a selected picture spreads.
    pub const SELECTION_GLOW: Rems = scaled(14.);
    /// A page in a page strip.
    pub const PAGE_THUMB_WIDTH: Rems = scaled(60.);
    pub const PAGE_THUMB_HEIGHT: Rems = scaled(84.);
    /// Margin around a viewer's picture area, so it sits inside its area
    /// as a rounded screen.
    pub const CANVAS_INSET: Rems = scaled(6.);
    /// Width of a search field in a toolbar.
    pub const SEARCH_FIELD_WIDTH: Rems = scaled(320.);
    /// The round backdrop behind an empty state's icon.
    pub const EMPTY_STATE_BUBBLE: Rems = scaled(64.);
    /// Widest an empty state's sentence runs before it wraps.
    pub const EMPTY_STATE_TEXT_WIDTH: Rems = scaled(300.);
    /// A tag's colour dot.
    pub const TAG_DOT: Rems = scaled(8.);
    pub const ICON_SM: Rems = scaled(12.);
    pub const ICON_MD: Rems = scaled(14.);
    pub const ICON_LG: Rems = scaled(16.);
    pub const ICON_XL: Rems = scaled(28.);
}
