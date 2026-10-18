//! The lists column between the narrow sidebar and the task list, modeled on TickTick's.

use fern_core::{List, Tag, View, ViewCounts};
use gpui::{ClickEvent, rgb};
use ui::prelude::*;

use crate::i18n::Strings;

pub const WIDTH: f32 = 212.;

/// Colors offered for lists, as stored in the database.
pub const LIST_COLORS: [&str; 8] =
    ["#EF4444", "#F97316", "#EAB308", "#22C55E", "#06B6D4", "#3B82F6", "#A855F7", "#EC4899"];

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

/// A tag or list color parsed from `#RRGGBB`.
pub fn parse_color(value: Option<&str>) -> Option<Hsla> {
    value
        .and_then(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok())
        .map(|c| rgb(c).into())
}

/// A tag or list color, or the muted text color when unset.
pub fn color(value: Option<&str>, cx: &App) -> Hsla {
    parse_color(value).unwrap_or(cx.theme().colors().text_muted)
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
    cx: &App,
) -> impl IntoElement {
    let Props { active, counts, lists, tags, strings, new_list_input } = props;
    let colors = cx.theme().colors();

    let item = |id: String,
                view: View,
                label: SharedString,
                icon: IconName,
                count: Option<usize>,
                dot: Option<Hsla>| {
        let on_select = on_select.clone();
        ListItem::new(SharedString::from(id))
            .height(px(36.))
            .toggle_state(&view == active)
            .start_slot(Icon::new(icon))
            .child(Label::new(label).truncate())
            .end_slot(
                h_flex()
                    .gap_2()
                    .when_some(dot, |el, color| el.child(div().size(px(6.)).rounded_full().bg(color)))
                    .when_some(count.filter(|c| *c > 0), |el, count| {
                        el.child(
                            div().min_w(px(18.)).flex().justify_end().child(
                                Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted),
                            ),
                        )
                    }),
            )
            .on_click(move |_, window, cx| on_select(view.clone(), window, cx))
    };
    let smart = |view: View, label: &'static str, icon: IconName, count: Option<usize>| {
        item(format!("view-{label}"), view, label.into(), icon, count, None)
    };
    let separator = || div().my_2().mx_2().child(Divider::horizontal());
    let heading = |text: &'static str| {
        h_flex()
            .h(px(28.))
            .px_2()
            .child(div().flex_1().child(Label::new(text).size(LabelSize::Small).color(Color::Muted)))
    };

    v_flex()
        .id("lists-panel")
        .flex_none()
        .w(px(WIDTH))
        .h_full()
        .overflow_y_scroll()
        .px_2()
        .py_2()
        .bg(colors.panel_background)
        .border_r_1()
        .border_color(colors.border)
        .child(smart(View::Inbox, strings.inbox, IconName::Inbox, Some(counts.inbox)))
        .child(smart(View::All, strings.all, IconName::Layers, Some(counts.all)))
        .child(smart(View::Today, strings.today, IconName::CalendarCheck, Some(counts.today)))
        .child(smart(View::Tomorrow, strings.tomorrow, IconName::Sunrise, Some(counts.tomorrow)))
        .child(smart(
            View::Next7Days,
            strings.next_7_days_short,
            IconName::CalendarRange,
            Some(counts.next_7_days),
        ))
        .child(separator())
        .child(
            heading(strings.lists).child(
                IconButton::new("add-list", IconName::Plus)
                    .size(px(20.))
                    .icon_size(IconSize::Small)
                    .on_click(on_add_list),
            ),
        )
        .children(lists.iter().map(|list| {
            item(
                format!("list-{}", list.id),
                View::List(list.id),
                list.name.clone().into(),
                IconName::List,
                Some(list.open_count),
                parse_color(list.color.as_deref()),
            )
        }))
        .children(new_list_input.map(|input| {
            h_flex()
                .h(px(36.))
                .flex_none()
                .gap_2()
                .px_2()
                .rounded(px(6.))
                .bg(colors.surface_background)
                .child(Icon::new(IconName::List))
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
                        IconName::Tag,
                        Some(tag.open_count),
                        Some(color(tag.color.as_deref(), cx)),
                    )
                }))
                .child(separator())
        })
        .child(smart(View::Completed, strings.completed, IconName::SquareCheckBig, None))
}
