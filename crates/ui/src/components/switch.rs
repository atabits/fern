use gpui::{
    App, IntoElement, ParentElement, RenderOnce, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use theme::ActiveTheme;

use crate::Toggleable;

/// An on/off switch. It has no click handler of its own: the row it sits in toggles it, as in
/// the original's settings.
#[derive(IntoElement, Default)]
pub struct Switch {
    on: bool,
}

impl Switch {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Toggleable for Switch {
    fn toggle_state(mut self, on: bool) -> Self {
        self.on = on;
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        div()
            .w(px(30.))
            .h(px(18.))
            .p(px(2.))
            .flex_none()
            .rounded_full()
            .flex()
            .when(self.on, |el| el.justify_end().bg(colors.accent))
            .when(!self.on, |el| el.bg(colors.element_hover))
            .child(div().size(px(14.)).rounded_full().bg(gpui::white()))
    }
}
