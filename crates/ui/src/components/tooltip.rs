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

/// Which side of its element a `with_tip` tooltip shows on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipSide {
    Above,
    Below,
}

/// A tooltip centered over or under its element while it is hovered, as the original's buttons
/// show theirs (GPUI's own tooltips follow the pointer). `key` must be unique among the view's
/// elements; `el` must have room to be `relative()`.
pub fn with_tip(
    el: gpui::Stateful<gpui::Div>,
    key: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    side: TipSide,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Stateful<gpui::Div> {
    use gpui::{StatefulInteractiveElement as _, prelude::FluentBuilder as _};
    let hovered = window.use_keyed_state(key, cx, |_, _| false);
    let shown = *hovered.read(cx);
    let colors = cx.theme().colors();
    let tip = div()
        .px_2()
        .py_1()
        .rounded(px(6.))
        .bg(colors.elevated_surface_background)
        .border_1()
        .border_color(colors.border)
        .shadow_lg()
        .text_color(colors.text)
        .text_size(px(12.))
        .whitespace_nowrap()
        .child(text.into());
    el.relative()
        .on_hover(move |on, _, cx| {
            hovered.update(cx, |state, cx| {
                if *state != *on {
                    *state = *on;
                    cx.notify();
                }
            })
        })
        .when(shown, |el| {
            let place = div().absolute().left(px(-200.)).right(px(-200.)).flex().justify_center();
            let place = match side {
                TipSide::Above => place.bottom(gpui::relative(1.)).pb(px(6.)),
                TipSide::Below => place.top(gpui::relative(1.)).pt(px(6.)),
            };
            el.child(place.child(gpui::deferred(tip).priority(3)))
        })
}
