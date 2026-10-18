//! Small UI building blocks shared by several screens.

use gpui::{
    AnyElement, Anchor, Div, IntoElement, Rgba, anchored, deferred, div, prelude::*, px,
};

use crate::theme;

/// A tag chip: tag color at low opacity as the background, a lighter shade of it as the text.
pub fn tag_chip(name: &str, color: Rgba) -> Div {
    let text = Rgba { r: (color.r + 1.) / 2., g: (color.g + 1.) / 2., b: (color.b + 1.) / 2., a: 1. };
    div()
        .flex_none()
        .h(px(18.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap_1()
        .rounded_full()
        .bg(color.opacity(0.3))
        .text_color(text)
        .text_size(px(11.))
        .child(name.to_owned())
}

/// The frame of a popover menu.
pub fn popover_frame() -> Div {
    div()
        .flex()
        .flex_col()
        .p_2()
        .rounded(px(8.))
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border())
        .shadow_lg()
        .text_color(theme::text())
        .text_size(px(13.))
}

/// Where a popover attaches to the element it belongs to.
pub enum Placement {
    BelowLeft,
    BelowRight,
    RightTop,
}

/// Renders `content` on top of the rest of the UI, attached to the parent element.
/// The parent must be `relative()`.
pub fn popover(placement: Placement, content: AnyElement) -> impl IntoElement {
    let (attach, anchor) = match placement {
        Placement::BelowLeft => (div().absolute().top(px(30.)).left_0(), Anchor::TopLeft),
        Placement::BelowRight => (div().absolute().top(px(30.)).right_0(), Anchor::TopRight),
        Placement::RightTop => (div().absolute().top_0().left(px(38.)), Anchor::TopLeft),
    };
    attach.child(
        deferred(anchored().anchor(anchor).snap_to_window_with_margin(px(8.)).child(content))
            .priority(1),
    )
}

/// A row in a popover menu.
pub fn menu_item(id: impl Into<gpui::ElementId>) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(32.))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|s| s.bg(theme::surface_hover()))
}
