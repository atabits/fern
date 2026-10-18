//! The lists column between the narrow sidebar and the task list, modeled on TickTick's.

use std::collections::HashSet;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{List, Preferences, SmartList, Tag, View, ViewCounts};
use gpui::{ClickEvent, rgb};
use ui::prelude::*;

use crate::calendar;
use crate::drag::DraggedTask;
use crate::i18n::Strings;

pub const WIDTH: f32 = 212.;

/// Colors offered for lists, as stored in the database.
pub const LIST_COLORS: [&str; 8] =
    ["#EF4444", "#F97316", "#EAB308", "#22C55E", "#06B6D4", "#3B82F6", "#A855F7", "#EC4899"];

/// The title of a view for the list header.
pub fn title(view: &View, lists: &[List], filters: &[fern_core::SmartFilter], strings: &Strings) -> SharedString {
    match view {
        View::Inbox => strings.inbox.into(),
        View::All => strings.all.into(),
        View::Today => strings.today.into(),
        View::Tomorrow => strings.tomorrow.into(),
        View::Next7Days => strings.next_7_days.into(),
        View::Completed => strings.completed.into(),
        View::Trash => strings.trash.into(),
        View::WontDo => strings.wont_do.into(),
        View::List(id) => {
            lists.iter().find(|l| l.id == *id).map(|l| l.name.clone()).unwrap_or_default().into()
        }
        View::Tag(name) => name.clone().into(),
        View::Search(_) => strings.section_search.into(),
        View::Filter(id) => filters.iter().find(|f| f.id == *id).map(|f| f.name.clone()).unwrap_or_default().into(),
        View::Summary => strings.summary.into(),
        View::Day(day) => calendar::format_date(*day, strings).into(),
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
    /// Saved filters with their task counts.
    pub filters: &'a [(fern_core::SmartFilter, usize)],
    /// Which smart lists Settings → Smart List shows.
    pub preferences: &'a Preferences,
    pub mini: MiniCalendar<'a>,
}

pub fn render(
    props: Props,
    on_select: impl Fn(View, &mut Window, &mut App) + Clone + 'static,
    on_add_list: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_drop_task: impl Fn(View, fern_core::Id, &mut Window, &mut App) + Clone + 'static,
    on_add_filter: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_mini: impl Fn(MiniAction, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> impl IntoElement {
    let Props { active, counts, lists, tags, strings, new_list_input, filters, preferences, mini } = props;
    let shows = |list: SmartList, count: usize| preferences.visibility(list).shows(count);
    let colors = cx.theme().colors();

    let item = |id: String,
                view: View,
                label: SharedString,
                icon: IconName,
                count: Option<usize>,
                dot: Option<Hsla>| {
        let on_select = on_select.clone();
        let on_drop_task = on_drop_task.clone();
        let drop_view = view.clone();
        let accent = colors.accent;
        // Every entry is also a drop target for dragged tasks.
        let item = ListItem::new(SharedString::from(id.clone()))
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
            .on_click(move |_, window, cx| on_select(view.clone(), window, cx));
        div()
            .id(SharedString::from(format!("drop-{id}")))
            .rounded(px(6.))
            .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.18)))
            .on_drop(move |dragged: &DraggedTask, window, cx| on_drop_task(drop_view.clone(), dragged.id, window, cx))
            .child(item)
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

    let lists_column = v_flex()
        .id("lists-panel")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px_2()
        .py_2()
        .when(shows(SmartList::Inbox, counts.inbox), |el| {
            el.child(smart(View::Inbox, strings.inbox, IconName::Inbox, Some(counts.inbox)))
        })
        .when(shows(SmartList::All, counts.all), |el| {
            el.child(smart(View::All, strings.all, IconName::Layers, Some(counts.all)))
        })
        .when(shows(SmartList::Today, counts.today), |el| {
            el.child(smart(View::Today, strings.today, IconName::CalendarCheck, Some(counts.today)))
        })
        .when(shows(SmartList::Tomorrow, counts.tomorrow), |el| {
            el.child(smart(View::Tomorrow, strings.tomorrow, IconName::Sunrise, Some(counts.tomorrow)))
        })
        .when(shows(SmartList::Next7Days, counts.next_7_days), |el| {
            el.child(smart(
                View::Next7Days,
                strings.next_7_days_short,
                IconName::CalendarRange,
                Some(counts.next_7_days),
            ))
        })
        .when(shows(SmartList::Summary, 1), |el| el.child(smart(View::Summary, strings.summary, IconName::FileText, None)))
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
        .when(tags.iter().any(|tag| shows(SmartList::Tags, tag.open_count)), |el| {
            el.child(heading(strings.tags))
                .children(tags.iter().filter(|tag| shows(SmartList::Tags, tag.open_count)).map(|tag| {
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
        // Filters, as in the original: a hint until there is one.
        .when(shows(SmartList::Filters, 1), |el| el.child(
            heading(strings.filters).child(
                IconButton::new("add-filter", IconName::Plus)
                    .size(px(20.))
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text(strings.add_filter))
                    .on_click(on_add_filter),
            ),
        ))
        .when(filters.is_empty() && shows(SmartList::Filters, 1), |el| {
            el.child(
                div()
                    .mx_1()
                    .mb_1()
                    .p_2()
                    .rounded(px(6.))
                    .bg(colors.surface_background)
                    .child(Label::new(strings.filters_hint).size(LabelSize::XSmall).color(Color::Muted)),
            )
        })
        .children(filters.iter().filter(|_| shows(SmartList::Filters, 1)).map(|(filter, count)| {
            item(
                format!("filter-{}", filter.id),
                View::Filter(filter.id),
                filter.name.clone().into(),
                IconName::Funnel,
                Some(*count),
                None,
            )
        }))
        .when(shows(SmartList::Filters, 1), |el| el.child(separator()))
        .when(shows(SmartList::Completed, 1), |el| {
            el.child(smart(View::Completed, strings.completed, IconName::SquareCheckBig, None))
        })
        .when(shows(SmartList::WontDo, 1), |el| el.child(smart(View::WontDo, strings.wont_do, IconName::SquareX, None)))
        .when(shows(SmartList::Trash, 1), |el| el.child(smart(View::Trash, strings.trash, IconName::Trash, None)));

    v_flex()
        .flex_none()
        .w(px(WIDTH))
        .h_full()
        .bg(colors.panel_background)
        .border_r_1()
        .border_color(colors.border)
        .child(lists_column)
        .when(preferences.mini_calendar, |el| el.child(mini_calendar(active, mini, on_select.clone(), on_mini, strings, cx)))
}

/// What the mini calendar shows: the week, or — expanded with its hover tab — a month.
pub struct MiniCalendar<'a> {
    pub expanded: bool,
    /// The first day of the expanded month.
    pub month: NaiveDate,
    /// Days with open tasks, marked with a dot when expanded.
    pub busy: &'a HashSet<NaiveDate>,
}

/// The mini calendar's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiniAction {
    /// The hover tab: expand to a month or collapse to the week.
    Toggle,
    /// The month arrows.
    Shift(i32),
    /// The circle between them: back to this month.
    ThisMonth,
}

/// The calendar pinned to the bottom of the column, as in TickTick: the current week, or a month
/// with arrows once expanded. Clicking a day opens its tasks (`View::Day`); that day stays marked.
fn mini_calendar(
    active: &View,
    mini: MiniCalendar,
    on_select: impl Fn(View, &mut Window, &mut App) + Clone + 'static,
    on_mini: impl Fn(MiniAction, &mut Window, &mut App) + Clone + 'static,
    strings: &Strings,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    let today = Local::now().date_naive();
    let day_cell = |date: NaiveDate, faded: bool| {
        let is_today = date == today;
        let selected = *active == View::Day(date);
        let busy = mini.expanded && mini.busy.contains(&date);
        let on_select = on_select.clone();
        // Columns of equal width whatever they hold, so the letters sit over the numbers.
        div().flex_1().flex_basis(gpui::relative(0.)).flex().justify_center().child(
            v_flex()
                .id(SharedString::from(format!("mini-day-{date}")))
                .size(px(28.))
                .cursor_pointer()
                .hover(|s| s.bg(colors.ghost_element_hover))
                .when(selected, |el| el.bg(colors.accent))
                .on_click(move |_, window, cx| on_select(View::Day(date), window, cx))
                .items_center()
                .justify_center()
                .rounded_full()
                .when(is_today && !selected, |el| el.bg(colors.element_hover))
                .child(
                    Label::new(date.day().to_string())
                        .size(LabelSize::Small)
                        .color(if selected {
                            Color::Custom(gpui::white())
                        } else if is_today {
                            Color::Accent
                        } else if faded {
                            Color::Muted
                        } else {
                            Color::Default
                        }),
                )
                .child(div().size(px(3.)).rounded_full().when(busy, |el| el.bg(colors.accent))),
        )
    };
    let weekdays = h_flex().children(crate::calendar::weekday_labels(&strings.weekdays_narrow).iter().map(|day| {
        div().flex_1().flex_basis(gpui::relative(0.)).flex().justify_center().child(Label::new(*day).size(LabelSize::Small).color(Color::Muted))
    }));
    let toggle = on_mini.clone();
    // The hover tab on the top border: "^" expands, "v" collapses.
    let tab = div()
        .absolute()
        .top(px(-9.))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .invisible()
        .group_hover("mini-calendar", |s| s.visible())
        .child(
            div()
                .id("mini-calendar-toggle")
                .w(px(40.))
                .h(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .bg(colors.element_hover)
                .cursor_pointer()
                .child(Icon::new(if mini.expanded { IconName::ChevronDown } else { IconName::ChevronUp }).size(IconSize::XSmall))
                .on_click(move |_, window, cx| toggle(MiniAction::Toggle, window, cx)),
        );
    let body = if mini.expanded {
        let arrows = |id: &'static str, icon: IconName, action: MiniAction| {
            let on_mini = on_mini.clone();
            IconButton::new(id, icon).size(px(20.)).icon_size(IconSize::XSmall).on_click(move |_, window, cx| on_mini(action, window, cx))
        };
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(calendar::month_title(mini.month, strings)).size(LabelSize::Small)))
                    .child(arrows("mini-back", IconName::ChevronLeft, MiniAction::Shift(-1)))
                    .child(arrows("mini-this-month", IconName::Circle, MiniAction::ThisMonth))
                    .child(arrows("mini-forward", IconName::ChevronRight, MiniAction::Shift(1))),
            )
            .child(weekdays)
            .children(calendar::month_grid(mini.month).chunks(7).map(|week| {
                h_flex().children(week.iter().map(|&date| day_cell(date, date.month() != mini.month.month())))
            }))
    } else {
        v_flex().gap_2().child(weekdays).child(h_flex().children(calendar::week_of(today).map(|date| day_cell(date, false))))
    };
    v_flex()
        .id("mini-calendar")
        .group("mini-calendar")
        .relative()
        .flex_none()
        .py_2()
        .px_3()
        .border_t_1()
        .border_color(colors.border)
        .child(tab)
        .child(body)
}
