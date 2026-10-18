use gpui::{App, IntoElement, Pixels, RenderOnce, Styled, Window, px, svg};
use icons::IconName;

use crate::Color;

#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub enum IconSize {
    /// 12px
    XSmall,
    /// 14px
    Small,
    /// 16px
    #[default]
    Medium,
    /// 18px
    Large,
    /// 20px
    XLarge,
    Custom(Pixels),
}

impl IconSize {
    pub fn pixels(self) -> Pixels {
        match self {
            IconSize::XSmall => px(12.),
            IconSize::Small => px(14.),
            IconSize::Medium => px(16.),
            IconSize::Large => px(18.),
            IconSize::XLarge => px(20.),
            IconSize::Custom(size) => size,
        }
    }
}

#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: IconSize,
    color: Color,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Self { name, size: IconSize::default(), color: Color::Muted }
    }

    pub fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: impl Into<Color>) -> Self {
        self.color = color.into();
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        svg()
            .path(self.name.path())
            .flex_none()
            .size(self.size.pixels())
            .text_color(self.color.color(cx))
    }
}
