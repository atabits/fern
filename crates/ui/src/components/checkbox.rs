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
    dismissed: bool,
    lines: bool,
    tint: Option<Hsla>,
    size: Pixels,
    on_click: Option<ClickHandler>,
    accent: bool,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self { id: id.into(), checked: false, dismissed: false, lines: false, tint: None, size: px(16.), on_click: None, accent: false }
    }

    pub fn tint(mut self, tint: Option<Hsla>) -> Self {
        self.tint = tint;
        self
    }

    /// A muted box with a cross, for tasks closed as "Won't Do".
    pub fn dismissed(mut self, dismissed: bool) -> Self {
        self.dismissed = dismissed;
        self
    }

    /// Lines inside the open box, as the original marks a task kept as a checklist.
    pub fn lines(mut self, lines: bool) -> Self {
        self.lines = lines;
        self
    }

    /// Checked in the accent color, as the original's settings boxes are (tasks are grey).
    pub fn accent(mut self) -> Self {
        self.accent = true;
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
        if self.dismissed {
            return div()
                .id(self.id)
                .size(self.size)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .border_1()
                .border_color(colors.text_muted.opacity(0.6))
                .bg(colors.text_muted.opacity(0.25))
                .cursor_pointer()
                .text_size(self.size * 0.7)
                .text_color(colors.text_muted)
                .child("✕")
                .when_some(self.on_click, |el, handler| el.on_click(handler));
        }
        // A checked task's box is grey, as the original's; a setting's is in the accent color.
        let fill = if self.accent { colors.accent } else { colors.text_muted.opacity(0.6) };
        let border = if self.checked { fill } else { self.tint.unwrap_or(colors.checkbox_border) };
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
            .when(self.checked, |el| el.bg(fill).child("✓"))
            .when(self.lines && !self.checked, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children((0..3).map(|_| div().w(self.size * 0.45).h(px(1.)).bg(border))),
                )
            })
            .when_some(self.tint.filter(|_| !self.checked), |el, tint| el.bg(tint.opacity(0.15)))
            .text_size(self.size * 0.7)
            .text_color(colors.background)
            .when_some(self.on_click, |el, handler| el.on_click(handler))
    }
}
