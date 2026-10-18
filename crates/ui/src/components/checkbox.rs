use gpui::{
    App, ClickEvent, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use theme::ActiveTheme;

use crate::traits::ClickHandler;
use crate::{Clickable, Toggleable};

/// A square checkbox. A custom `tint` colors the border and fills the box lightly, which is how
/// task priorities are shown.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: bool,
    tint: Option<Hsla>,
    size: Pixels,
    on_click: Option<ClickHandler>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self { id: id.into(), checked: false, tint: None, size: px(16.), on_click: None }
    }

    pub fn tint(mut self, tint: Option<Hsla>) -> Self {
        self.tint = tint;
        self
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }
}

impl Toggleable for Checkbox {
    fn toggle_state(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

impl Clickable for Checkbox {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        let border = if self.checked { colors.accent } else { self.tint.unwrap_or(colors.checkbox_border) };
        div()
            .id(self.id)
            .size(self.size)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(3.))
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .when(self.checked, |el| el.bg(colors.accent).child("✓"))
            .when_some(self.tint.filter(|_| !self.checked), |el, tint| el.bg(tint.opacity(0.15)))
            .text_size(self.size * 0.7)
            .text_color(colors.background)
            .when_some(self.on_click, |el, handler| el.on_click(handler))
    }
}
