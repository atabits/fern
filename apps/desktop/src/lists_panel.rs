//! Колонка списков между узкой панелью разделов и списком задач, как в TickTick.

use amin_core::{Tag, View, ViewCounts};
use gpui::{
    App, IntoElement, ParentElement, Rgba, SharedString, Styled, Window, div, prelude::*, px, rgb,
    svg,
};

use crate::theme;

pub const WIDTH: f32 = 212.;

pub fn title(view: &View) -> SharedString {
    match view {
        View::Inbox => "Входящие".into(),
        View::All => "Все".into(),
        View::Today => "Сегодня".into(),
        View::Tomorrow => "Завтра".into(),
        View::Next7Days => "Следующие 7 дней".into(),
        View::Completed => "Выполненные".into(),
        View::Tag(name) => name.clone().into(),
    }
}

/// Цвет тега из `#RRGGBB`; без цвета — нейтральный серый.
pub fn tag_color(tag: Option<&str>) -> Rgba {
    tag.and_then(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok())
        .map(rgb)
        .unwrap_or_else(theme::text_muted)
}

pub fn render(
    active: &View,
    counts: &ViewCounts,
    tags: &[Tag],
    on_select: impl Fn(View, &mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let item = |view: View, icon: &'static str, count: Option<usize>, dot: Option<Rgba>| {
        let on_select = on_select.clone();
        let selected = &view == active;
        let label = match &view {
            View::Next7Days => "7 дней".into(),
            other => title(other),
        };
        div()
            .id(SharedString::from(format!("view-{label}")))
            .h(px(36.))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme::row_selected()))
            .when(!selected, |el| el.hover(|s| s.bg(theme::row_hover())))
            .on_click(move |_, window, cx| on_select(view.clone(), window, cx))
            .child(svg().path(icon).size(px(16.)).flex_none().text_color(theme::text_muted()))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when_some(dot, |el, color| el.child(div().size(px(6.)).rounded_full().bg(color)))
            .when_some(count.filter(|c| *c > 0), |el, count| {
                el.child(
                    div()
                        .min_w(px(18.))
                        .flex()
                        .justify_end()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(count.to_string()),
                )
            })
    };

    let separator = || div().my_2().mx_3().h(px(1.)).flex_none().bg(theme::border());
    let heading = |text: &'static str| {
        div()
            .px_3()
            .pt_2()
            .pb_1()
            .text_xs()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme::text_muted())
            .child(text)
    };

    div()
        .id("lists-panel")
        .flex_none()
        .w(px(WIDTH))
        .h_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .px_2()
        .py_2()
        .bg(theme::lists_panel())
        .border_r_1()
        .border_color(theme::border())
        .child(item(View::Inbox, "icons/inbox.svg", Some(counts.inbox), None))
        .child(item(View::All, "icons/layers.svg", Some(counts.all), None))
        .child(item(View::Today, "icons/calendar-check.svg", Some(counts.today), None))
        .child(item(View::Tomorrow, "icons/sunrise.svg", Some(counts.tomorrow), None))
        .child(item(View::Next7Days, "icons/calendar-range.svg", Some(counts.next_7_days), None))
        .child(separator())
        .when(!tags.is_empty(), |el| {
            el.child(heading("Теги")).children(tags.iter().map(|tag| {
                item(
                    View::Tag(tag.name.clone()),
                    "icons/tag.svg",
                    Some(tag.open_count),
                    Some(tag_color(tag.color.as_deref())),
                )
            }))
            .child(separator())
        })
        .child(item(View::Completed, "icons/square-check-big.svg", None, None))
}
