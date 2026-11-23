//! Things-style "When" scheduling popover and Deadline picker reusing `task_ui::due_picker`.

use chrono::{Datelike, Duration, NaiveDate};
use desktop_common::calendar;
use desktop_common::text_input::TextInput;
use fern_core::{Horizon, Id, ScheduleBucket, Task};
use fern_i18n::Strings;
use gpui::{AnyElement, Context, Entity, FontWeight, Window, div, px};
use task_ui::due_picker::{self, DueDraft};
use task_ui::task_rows;
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Divider, Icon, IconButton, IconName, IconSize, Label,
    LabelSize, ListItem, Tooltip, popover, v_scroll,
};

/// Target entity for a Deadline picker session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadlineTarget {
    Task(Id),
    Project(Id),
}

/// Active state for the "When" popover.
pub struct WhenPickerState {
    pub task_id: Id,
    pub month: NaiveDate,
    pub start_date: Option<NaiveDate>,
    pub bucket: ScheduleBucket,
    pub horizon: Option<Horizon>,
    pub date_input: Entity<TextInput>,
}

/// Active state for the "Deadline" popover backed by [`DueDraft`].
pub struct DeadlinePickerState {
    pub target: DeadlineTarget,
    pub draft: DueDraft,
}

/// Actions emitted by the "When" popover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhenAction {
    Schedule {
        start_date: Option<NaiveDate>,
        bucket: ScheduleBucket,
    },
    SetHorizon(Horizon),
    ShiftMonth(i32),
    Close,
}

/// Actions emitted by the "Deadline" popover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadlineAction {
    PickDate(NaiveDate),
    ShiftMonth(i32),
    Apply(Option<NaiveDate>),
    Close,
}

/// Formats a task's Things "When" schedule label for chips and card footers.
pub fn format_when_label(task: &Task, today: NaiveDate, strings: &Strings) -> Option<String> {
    match task.schedule_bucket {
        ScheduleBucket::Someday => Some(strings.things_someday.to_owned()),
        ScheduleBucket::Evening if task.start_date.is_none_or(|d| d <= today) => {
            Some(strings.things_today_evening.to_owned())
        }
        _ => task
            .start_date
            .map(|date| task_rows::due_label(date, None, today, strings).0),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_month_grid<V: 'static>(
    id_prefix: &'static str,
    month: NaiveDate,
    selected: Option<NaiveDate>,
    today: NaiveDate,
    strings: &'static Strings,
    cx: &mut Context<V>,
    on_shift: impl Fn(&mut V, i32, &mut Window, &mut Context<V>) + Clone + 'static,
    on_pick: impl Fn(&mut V, NaiveDate, &mut Window, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let colors = cx.theme().colors().clone();
    let month_title = strings
        .month_title
        .replace("{month}", strings.months[month.month0() as usize])
        .replace("{yyyy}", &month.year().to_string());
    let weekdays = calendar::weekday_labels(&strings.weekdays_narrow);
    let shift_prev = on_shift.clone();
    let shift_next = on_shift;

    let weeks: Vec<AnyElement> = calendar::month_grid(month)
        .chunks(7)
        .map(|week| {
            h_flex()
                .children(week.iter().map(|&date| {
                    let in_month = date.month() == month.month();
                    let is_selected = selected == Some(date);
                    let is_today = date == today;
                    let on_pick = on_pick.clone();
                    div()
                        .flex_1()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .id(SharedString::from(format!("{id_prefix}-day-{date}")))
                                .size(px(26.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .cursor_pointer()
                                .when(is_selected, |el| el.bg(colors.accent))
                                .when(!is_selected && is_today, |el| {
                                    el.bg(colors.accent.opacity(0.16))
                                })
                                .when(!is_selected, |el| el.hover(|s| s.bg(colors.element_hover)))
                                .child(
                                    Label::new(date.day().to_string())
                                        .centered()
                                        .size(LabelSize::XSmall)
                                        .color(if is_selected {
                                            Color::OnAccent
                                        } else if is_today {
                                            Color::Accent
                                        } else if in_month {
                                            Color::Default
                                        } else {
                                            Color::Muted
                                        }),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    on_pick(this, date, window, cx);
                                })),
                        )
                }))
                .into_any_element()
        })
        .collect();

    v_flex()
        .gap_1()
        .child(
            h_flex()
                .h(px(26.))
                .px_1()
                .child(
                    div().flex_1().child(
                        Label::new(month_title)
                            .size(LabelSize::Small)
                            .weight(FontWeight::MEDIUM),
                    ),
                )
                .child(
                    IconButton::new(SharedString::from(format!("{id_prefix}-prev")), IconName::ChevronLeft)
                        .size(px(22.))
                        .icon_size(IconSize::XSmall)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            shift_prev(this, -1, window, cx);
                        })),
                )
                .child(
                    IconButton::new(SharedString::from(format!("{id_prefix}-next")), IconName::ChevronRight)
                        .size(px(22.))
                        .icon_size(IconSize::XSmall)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            shift_next(this, 1, window, cx);
                        })),
                ),
        )
        .child(
            h_flex().children(weekdays.into_iter().map(|wd| {
                div().flex_1().flex().justify_center().child(
                    Label::new(wd)
                        .centered()
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
            })),
        )
        .children(weeks)
        .into_any_element()
}

/// Renders the Things "When" scheduling popover (`Today`, `This Evening`, `Tomorrow`, `Next Week`,
/// date input + mini calendar, `Someday`, `Clear`).
pub fn render_when_popover<V: 'static>(
    state: &WhenPickerState,
    today: NaiveDate,
    strings: &'static Strings,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, WhenAction, &mut Window, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let theme = cx.theme().clone();
    let colors = theme.colors().clone();
    let status = theme.status().clone();
    let tomorrow = today + Duration::days(1);
    let next_week = fern_core::next_week_date(today, calendar::week_start());

    let preset_row = |id: &'static str,
                      icon: IconName,
                      icon_color: Color,
                      label: &'static str,
                      active: bool,
                      start_date: Option<NaiveDate>,
                      bucket: ScheduleBucket,
                      cx: &mut Context<V>| {
        let on_action = on_action.clone();
        ListItem::new(id)
            .height(px(30.))
            .toggle_state(active)
            .start_slot(Icon::new(icon).size(IconSize::Small).color(icon_color))
            .child(
                Label::new(label)
                    .size(LabelSize::Small)
                    .color(if active { Color::Accent } else { Color::Default }),
            )
            .when(active, |el| {
                el.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                on_action(this, WhenAction::Schedule { start_date, bucket }, window, cx);
            }))
    };

    let is_today = state.start_date == Some(today) && state.bucket == ScheduleBucket::None;
    let is_evening =
        state.bucket == ScheduleBucket::Evening && state.start_date.is_none_or(|d| d <= today);
    let is_tomorrow = state.start_date == Some(tomorrow) && state.bucket == ScheduleBucket::None;
    let is_next_week = state.start_date == Some(next_week) && state.bucket == ScheduleBucket::None;
    let is_someday = state.bucket == ScheduleBucket::Someday;

    let selected_day = (state.bucket != ScheduleBucket::Someday).then_some(state.start_date).flatten();
    let cal_shift = on_action.clone();
    let cal_pick = on_action.clone();
    let cal = render_month_grid(
        "when-cal",
        state.month,
        selected_day,
        today,
        strings,
        cx,
        move |this, delta, window, cx| cal_shift(this, WhenAction::ShiftMonth(delta), window, cx),
        move |this, date, window, cx| {
            cal_pick(
                this,
                WhenAction::Schedule {
                    start_date: Some(date),
                    bucket: ScheduleBucket::None,
                },
                window,
                cx,
            );
        },
    );

    let horizons_enabled = calendar::horizons_enabled();
    let has_clearable_schedule = state.start_date.is_some()
        || state.bucket != ScheduleBucket::None
        || (horizons_enabled && state.horizon.is_some());

    let close_out = on_action.clone();
    let clear_action = on_action.clone();

    let body = v_flex()
        .id("things-when-body")
        .gap_1p5()
        .child(preset_row(
            "when-today",
            IconName::Sun,
            Color::Custom(status.warning),
            strings.things_today,
            is_today,
            Some(today),
            ScheduleBucket::None,
            cx,
        ))
        .child(preset_row(
            "when-evening",
            IconName::Moon,
            Color::Accent,
            strings.things_today_evening,
            is_evening,
            Some(today),
            ScheduleBucket::Evening,
            cx,
        ))
        .child(preset_row(
            "when-tomorrow",
            IconName::Sunrise,
            Color::Custom(status.info),
            strings.tomorrow,
            is_tomorrow,
            Some(tomorrow),
            ScheduleBucket::None,
            cx,
        ))
        .child(preset_row(
            "when-next-week",
            IconName::CalendarRange,
            Color::Icon,
            strings.next_week,
            is_next_week,
            Some(next_week),
            ScheduleBucket::None,
            cx,
        ))
        .child(preset_row(
            "when-someday",
            IconName::Archive,
            Color::IconMuted,
            strings.things_someday,
            is_someday,
            None,
            ScheduleBucket::Someday,
            cx,
        ))
        .child(div().my_0p5().child(Divider::horizontal()))
        .child(
            h_flex()
                .h(px(28.))
                .px_2()
                .gap_1p5()
                .rounded(px(6.))
                .bg(colors.surface_background)
                .border_1()
                .border_color(colors.border_variant)
                .child(Icon::new(IconName::CalendarDays).size(IconSize::XSmall).color(Color::Muted))
                .child(div().flex_1().min_w_0().child(state.date_input.clone())),
        )
        .child(cal)
        .when(has_clearable_schedule, |el| {
            el.child(div().my_0p5().child(Divider::horizontal())).child(
                Button::new("when-clear", strings.things_clear_when)
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .full_width()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        clear_action(
                            this,
                            WhenAction::Schedule {
                                start_date: None,
                                bucket: ScheduleBucket::None,
                            },
                            window,
                            cx,
                        );
                    })),
            )
        });

    let horizons_footer = horizons_enabled.then(|| {
        let horizon_action = on_action;
        let week = Horizon::current_week(today);
        let month = Horizon::current_month(today);
        let year = Horizon::current_year(today);
        let chips = [
            ("week", week, strings.horizon_this_week),
            ("month", month, strings.horizon_this_month),
            ("year", year, strings.horizon_this_year),
            ("life", Horizon::Life, strings.horizons_life),
        ];
        v_flex()
            .id("things-when-horizons")
            .when(cfg!(test), |el| {
                el.debug_selector(|| "things-when-horizons".to_owned())
            })
            .pt_1p5()
            .gap_1()
            .child(Divider::horizontal())
            .child(
                h_flex()
                    .px_1()
                    .gap_1()
                    .child(
                        Icon::new(IconName::FernHorizons)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new(strings.horizons_horizon_label)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(h_flex().gap_1().children(chips.into_iter().map(|(key, horizon, tip)| {
                let active = state.horizon == Some(horizon);
                let label = calendar::format_horizon_badge(horizon, strings);
                let on_action = horizon_action.clone();
                div()
                    .id(SharedString::from(format!("when-horizon-{key}")))
                    .when(cfg!(test), |el| {
                        el.debug_selector(move || format!("when-horizon-{key}"))
                    })
                    .flex_1()
                    .min_w_0()
                    .h(px(26.))
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if active {
                        colors.accent.opacity(0.6)
                    } else {
                        colors.border_variant
                    })
                    .bg(if active {
                        colors.accent.opacity(0.14)
                    } else {
                        colors.surface_background
                    })
                    .when(!active, |el| el.hover(|s| s.bg(colors.element_hover)))
                    .cursor_pointer()
                    .tooltip(Tooltip::text(tip))
                    .child(
                        Label::new(label)
                            .size(LabelSize::XSmall)
                            .color(if active { Color::Accent } else { Color::Default })
                            .truncate(),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        on_action(this, WhenAction::SetHorizon(horizon), window, cx);
                    }))
            })))
    });

    let scroll_max_h = px(450.);

    popover(cx)
        .id("things-when-popover")
        .when(cfg!(test), |el| el.debug_selector(|| "things-when-popover".to_owned()))
        .w(px(244.))
        .p_2()
        .on_mouse_down_out(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            close_out(this, WhenAction::Close, window, cx);
        }))
        .child(v_scroll("things-when-scroll", body).max_h(scroll_max_h).compact())
        .children(horizons_footer)
        .into_any_element()
}

/// Renders the Deadline popover reusing [`DueDraft`] and [`due_picker::quick_dates`].
pub fn render_deadline_popover<V: 'static>(
    state: &DeadlinePickerState,
    today: NaiveDate,
    strings: &'static Strings,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, DeadlineAction, &mut Window, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let colors = cx.theme().colors().clone();
    let [d_today, d_tomorrow, d_week, d_month] = due_picker::quick_dates(today);

    let quick_btn = |id: &'static str, icon: IconName, tip: &'static str, date: NaiveDate, cx: &mut Context<V>| {
        let on_action = on_action.clone();
        let active = state.draft.date == Some(date);
        div()
            .id(id)
            .flex_1()
            .h(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .when(active, |el| el.bg(colors.accent.opacity(0.14)))
            .when(!active, |el| el.hover(|s| s.bg(colors.element_hover)))
            .tooltip(Tooltip::text(tip))
            .child(Icon::new(icon).size(IconSize::Medium).color(if active {
                Color::Accent
            } else {
                Color::Icon
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                on_action(this, DeadlineAction::PickDate(date), window, cx);
            }))
    };

    let cal_shift = on_action.clone();
    let cal_pick = on_action.clone();
    let cal = render_month_grid(
        "deadline-cal",
        state.draft.month,
        state.draft.date,
        today,
        strings,
        cx,
        move |this, delta, window, cx| cal_shift(this, DeadlineAction::ShiftMonth(delta), window, cx),
        move |this, date, window, cx| cal_pick(this, DeadlineAction::PickDate(date), window, cx),
    );

    let close_out = on_action.clone();
    let clear_action = on_action.clone();
    let ok_action = on_action.clone();
    let picked_date = state.draft.date;

    popover(cx)
        .id("things-deadline-popover")
        .when(cfg!(test), |el| el.debug_selector(|| "things-deadline-popover".to_owned()))
        .w(px(244.))
        .p_2()
        .gap_1p5()
        .on_mouse_down_out(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            close_out(this, DeadlineAction::Close, window, cx);
        }))
        .child(
            Label::new(strings.things_deadline)
                .size(LabelSize::Small)
                .weight(FontWeight::SEMIBOLD),
        )
        .child(
            h_flex()
                .gap_1()
                .child(quick_btn("deadline-today", IconName::Sun, strings.today, d_today, cx))
                .child(quick_btn("deadline-tomorrow", IconName::Sunrise, strings.tomorrow, d_tomorrow, cx))
                .child(quick_btn("deadline-week", IconName::CalendarRange, strings.next_week, d_week, cx))
                .child(quick_btn("deadline-month", IconName::CalendarDays, strings.next_month, d_month, cx)),
        )
        .child(Divider::horizontal())
        .child(cal)
        .child(Divider::horizontal())
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("deadline-clear", strings.clear)
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Sm)
                        .full_width()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            clear_action(this, DeadlineAction::Apply(None), window, cx);
                        })),
                )
                .child(
                    Button::new("deadline-ok", strings.ok)
                        .variant(ButtonVariant::Primary)
                        .size(ButtonSize::Sm)
                        .full_width()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            ok_action(this, DeadlineAction::Apply(picked_date), window, cx);
                        })),
                ),
        )
        .into_any_element()
}
