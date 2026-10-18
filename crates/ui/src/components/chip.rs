use theme::ActiveTheme;
use gpui::{
    AnyElement, App, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window,
    div, px,
};

/// A small rounded label, used for tags: the color at low opacity as the background and a shade
/// of it as the text, lighter on a dark theme and darker on a light one.
#[derive(IntoElement)]
pub struct Chip {
    label: SharedString,
    color: Hsla,
    end_slot: Option<AnyElement>,
}

impl Chip {
    pub fn new(label: impl Into<SharedString>, color: Hsla) -> Self {
        Self { label: label.into(), color, end_slot: None }
    }

    pub fn end_slot(mut self, element: impl IntoElement) -> Self {
        self.end_slot = Some(element.into_any_element());
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut text = self.color;
        text.l = if cx.theme().is_light() { text.l * 0.6 } else { (text.l + 1.) / 2. };
        text.a = 1.;
        div()
            .flex_none()
            .h(px(18.))
            .px(px(6.))
            .flex()
            .items_center()
            .gap_1()
            .rounded_full()
            .bg(self.color.opacity(0.3))
            .text_color(text)
            .text_size(px(11.))
            .child(self.label)
            .children(self.end_slot)
    }
}
