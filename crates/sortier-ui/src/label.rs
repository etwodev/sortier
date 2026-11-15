use gpui::{App, FontWeight, IntoElement, RenderOnce, SharedString, Styled, Window};

use crate::theme::ActiveSortierTheme;
use crate::tokens::typography;

/// Text colour role of a [`Label`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextTone {
    #[default]
    Primary,
    Secondary,
    Disabled,
}

/// Size step of a [`Label`], from the type scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextSize {
    Small,
    #[default]
    Body,
    Heading,
}

/// A single line of UI text in one of the type scale's sizes and tones.
#[derive(IntoElement)]
pub struct Label {
    text: SharedString,
    tone: TextTone,
    size: TextSize,
    truncate: bool,
    /// A font family to show the text in, instead of the interface's.
    font: Option<SharedString>,
}

impl Label {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: TextTone::default(),
            size: TextSize::default(),
            truncate: false,
            font: None,
        }
    }

    /// Keeps the text on one line, ending in "…" where it doesn't fit,
    /// instead of wrapping. Give it a parent that limits its width.
    pub fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }

    pub fn tone(mut self, tone: TextTone) -> Self {
        self.tone = tone;
        self
    }

    pub fn size(mut self, size: TextSize) -> Self {
        self.size = size;
        self
    }

    /// Shows the text in font family `family`, such as a font's name in its
    /// own font.
    pub fn font(mut self, family: impl Into<SharedString>) -> Self {
        self.font = Some(family.into());
        self
    }
}

impl RenderOnce for Label {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.sortier_theme().colors;
        let color = match self.tone {
            TextTone::Primary => colors.text_primary,
            TextTone::Secondary => colors.text_secondary,
            TextTone::Disabled => colors.text_disabled,
        };
        let (size, weight) = match self.size {
            TextSize::Small => (typography::SMALL, FontWeight::NORMAL),
            TextSize::Body => (typography::BODY, FontWeight::NORMAL),
            TextSize::Heading => (typography::HEADING, FontWeight::SEMIBOLD),
        };
        let mut label = gpui_component::label::Label::new(self.text)
            .text_size(size)
            .font_weight(weight)
            .text_color(color.hsla());
        if let Some(family) = self.font {
            label = label.font_family(family);
        }
        if self.truncate {
            label.whitespace_nowrap().overflow_hidden().text_ellipsis()
        } else {
            label
        }
    }
}
