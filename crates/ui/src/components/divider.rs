use gpui::{App, IntoElement, RenderOnce, Styled, Window, div, px};
use theme::ActiveTheme;

/// A thin horizontal or vertical line.
#[derive(IntoElement)]
pub struct Divider {
    vertical: bool,
}

impl Divider {
    pub fn horizontal() -> Self {
        Self { vertical: false }
    }

    pub fn vertical() -> Self {
        Self { vertical: true }
    }
}

impl RenderOnce for Divider {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let line = div().flex_none().bg(cx.theme().colors().border);
        if self.vertical { line.w(px(1.)).h(px(16.)) } else { line.h(px(1.)).w_full() }
    }
}
