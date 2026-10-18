use gpui::{
    App, FontWeight, IntoElement, ParentElement, Pixels, RenderOnce, SharedString, Styled, Window,
    div, prelude::FluentBuilder as _, px,
};

use crate::Color;

#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub enum LabelSize {
    /// 11px, chips and badges.
    XSmall,
    /// 12px, secondary details.
    Small,
    /// 14px, body text.
    #[default]
    Default,
    /// 20px, headlines.
    Large,
}

impl LabelSize {
    pub fn pixels(self) -> Pixels {
        match self {
            LabelSize::XSmall => px(11.),
            LabelSize::Small => px(12.),
            LabelSize::Default => px(14.),
            LabelSize::Large => px(20.),
        }
    }
}

/// A single line of text. Long text is truncated with an ellipsis when `truncate` is set.
#[derive(IntoElement)]
pub struct Label {
    text: SharedString,
    size: LabelSize,
    color: Color,
    weight: Option<FontWeight>,
    strikethrough: bool,
    truncate: bool,
}

impl Label {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            size: LabelSize::default(),
            color: Color::Default,
            weight: None,
            strikethrough: false,
            truncate: false,
        }
    }

    pub fn size(mut self, size: LabelSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: impl Into<Color>) -> Self {
        self.color = color.into();
        self
    }

    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = Some(weight);
        self
    }

    pub fn strikethrough(mut self, strikethrough: bool) -> Self {
        self.strikethrough = strikethrough;
        self
    }

    pub fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }
}

impl RenderOnce for Label {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .text_size(self.size.pixels())
            .text_color(self.color.color(cx))
            .when_some(self.weight, |el, weight| el.font_weight(weight))
            .when(self.strikethrough, |el| el.line_through())
            .when(self.truncate, |el| el.min_w_0().truncate())
            .child(self.text)
    }
}
