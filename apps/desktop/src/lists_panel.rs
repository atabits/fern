//! The lists column between the narrow sidebar and the task list, modeled on TickTick's.

use fern_core::{List, Tag, View, ViewCounts};
use gpui::{
    AnyElement, App, ClickEvent, IntoElement, ParentElement, Rgba, SharedString, Styled, Window,
    div, prelude::*, px, rgb, svg,
};

use crate::i18n::Strings;
use crate::theme;

pub const WIDTH: f32 = 212.;

/// The title of a view for the list header.
pub fn title(view: &View, lists: &[List], strings: &Strings) -> SharedString {
    match view {
        View::Inbox => strings.inbox.into(),
        View::All => strings.all.into(),
        View::Today => strings.today.into(),
        View::Tomorrow => strings.tomorrow.into(),
        View::Next7Days => strings.next_7_days.into(),
        View::Completed => strings.completed.into(),
        View::List(id) => {
            lists.iter().find(|l| l.id == *id).map(|l| l.name.clone()).unwrap_or_default().into()
        }
        View::Tag(name) => name.clone().into(),
    }
}

/// A tag or list color from `#RRGGBB`; neutral gray when unset or invalid.
pub fn color(value: Option<&str>) -> Rgba {
    value
        .and_then(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok())
        .map(rgb)
        .unwrap_or_else(theme::text_muted)
}

pub struct Props<'a> {
    pub active: &'a View,
    pub counts: &'a ViewCounts,
    pub lists: &'a [List],
    pub tags: &'a [Tag],
    pub strings: &'static Strings,
    /// The inline "new list" input while the user is creating a list.
    pub new_list_input: Option<AnyElement>,
}

pub fn render(
    props: Props,
    on_select: impl Fn(View, &mut Window, &mut App) + Clone + 'static,
    on_add_list: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let Props { active, counts, lists, tags, strings, new_list_input } = props;

    let item = |id: String, view: View, label: SharedString, icon: &'static str, count: Option<usize>, dot: Option<Rgba>| {
        let on_select = on_select.clone();
        let selected = &view == active;
        div()
            .id(SharedString::from(id))
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
    let smart = |view: View, label: &'static str, icon: &'static str, count: Option<usize>| {
        item(format!("view-{label}"), view, label.into(), icon, count, None)
    };

    let separator = || div().my_2().mx_3().h(px(1.)).flex_none().bg(theme::border());
    let heading = |text: &'static str| {
        div()
            .h(px(28.))
            .px_3()
            .flex()
            .items_center()
            .text_xs()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme::text_muted())
            .child(div().flex_1().child(text))
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
        .child(smart(View::Inbox, strings.inbox, "icons/inbox.svg", Some(counts.inbox)))
        .child(smart(View::All, strings.all, "icons/layers.svg", Some(counts.all)))
        .child(smart(View::Today, strings.today, "icons/calendar-check.svg", Some(counts.today)))
        .child(smart(View::Tomorrow, strings.tomorrow, "icons/sunrise.svg", Some(counts.tomorrow)))
        .child(smart(View::Next7Days, strings.next_7_days_short, "icons/calendar-range.svg", Some(counts.next_7_days)))
        .child(separator())
        .child(
            heading(strings.lists).child(
                div()
                    .id("add-list")
                    .p_0p5()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::surface_hover()))
                    .on_click(on_add_list)
                    .child(svg().path("icons/plus.svg").size(px(14.)).text_color(theme::text_muted())),
            ),
        )
        .children(lists.iter().map(|list| {
            item(
                format!("list-{}", list.id),
                View::List(list.id),
                list.name.clone().into(),
                "icons/list.svg",
                Some(list.open_count),
                list.color.as_deref().map(|c| color(Some(c))),
            )
        }))
        .children(new_list_input.map(|input| {
            div()
                .h(px(36.))
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .rounded(px(6.))
                .bg(theme::surface())
                .child(svg().path("icons/list.svg").size(px(16.)).flex_none().text_color(theme::text_muted()))
                .child(input)
        }))
        .child(separator())
        .when(!tags.is_empty(), |el| {
            el.child(heading(strings.tags))
                .children(tags.iter().map(|tag| {
                    item(
                        format!("tag-{}", tag.name),
                        View::Tag(tag.name.clone()),
                        tag.name.clone().into(),
                        "icons/tag.svg",
                        Some(tag.open_count),
                        Some(color(tag.color.as_deref())),
                    )
                }))
                .child(separator())
        })
        .child(smart(View::Completed, strings.completed, "icons/square-check-big.svg", None))
}
