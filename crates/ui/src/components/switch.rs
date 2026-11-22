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
            .w(px(32.))
            .h(px(18.))
            .px(px(2.))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .when(self.on, |el| el.justify_end().bg(colors.accent))
            .when(!self.on, |el| {
                el.bg(colors.background)
                    .border_1()
                    .border_color(colors.border)
            })
            .child(div().size(px(12.)).rounded_full().bg(if self.on { colors.icon_on_accent } else { gpui::white() }))
    }
}
