//! Небольшие элементы интерфейса, общие для нескольких экранов.

use gpui::{Div, Rgba, div, prelude::*, px};

/// Чип тега: фон — цвет тега с прозрачностью, текст — тот же цвет, осветлённый.
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
