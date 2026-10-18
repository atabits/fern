use gpui::{AnyView, App, AppContext as _, Context, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px};
use theme::ActiveTheme;

/// A small text tooltip. Use with `.tooltip(Tooltip::text("…"))` on a stateful element.
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    pub fn text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        move |_, cx| cx.new(|_| Tooltip { text: text.clone() }).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        div()
            .px_2()
            .py_1()
            .rounded(px(6.))
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .text_color(colors.text)
            .text_size(px(12.))
            .child(self.text.clone())
    }
}
