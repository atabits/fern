use std::collections::{HashMap, HashSet};

use chrono::{Datelike, Local, NaiveDate};
use fern_core::Task;
use gpui::FontWeight;
use ui::prelude::*;
use ui::{Placement, PopoverAttachment};

use crate::calendar;
use crate::drag::{DraggedEvent, DraggedTask, ResizeEdge, ResizedTask};
use crate::i18n;
use crate::task_rows;

use super::{
    CALENDAR_HEADER_HEIGHT, CalendarMode, CalendarView, EventBar, OpenTask, calendar_year,
};

/// How many tasks a day cell shows before collapsing the rest into "+N".
const TASKS_PER_DAY: usize = 5;
pub(super) const WEEKDAY_HEADER_HEIGHT: f32 = 22.;
const HORIZON_BANNER_HEIGHT: f32 = 38.;
const DAY_HEADER_HEIGHT: f32 = 26.;
const DAY_ITEM_HEIGHT: f32 = 18.;
const DAY_ITEM_GAP: f32 = 2.;
const DAY_TOP_PADDING: f32 = 4.;
/// The week-number column of the month grid.
pub(super) const WEEK_NUMBER_WIDTH: f32 = 32.;

fn day_item_slots(height: Pixels) -> usize {
    let available = f32::from(height) - DAY_HEADER_HEIGHT - DAY_TOP_PADDING - 1.;
    (available / (DAY_ITEM_HEIGHT + DAY_ITEM_GAP)).floor().max(0.) as usize
}

pub(super) struct DayBuckets<'a> {
    pub(super) tasks: HashMap<NaiveDate, Vec<&'a Task>>,
    pub(super) events: HashMap<NaiveDate, Vec<EventBar>>,
    pub(super) habits: HashMap<NaiveDate, Vec<(&'a fern_core::Habit, Option<&'a fern_core::Checkin>)>>,
    pub(super) focus: HashMap<NaiveDate, Vec<&'a fern_core::FocusSession>>,
    pub(super) countdowns: HashMap<NaiveDate, Vec<&'a fern_core::Countdown>>,
}

fn fast_forward_repeat_anchor(
    start: NaiveDate,
    end: NaiveDate,
    first: NaiveDate,
    repeat: &fern_core::Recurrence,
) -> NaiveDate {
    use fern_core::Frequency;
    let interval = i64::from(repeat.interval.max(1));
    let step = if !repeat.is_specific_dates()
        && !repeat.skip_weekends
        && !repeat.skip_holidays
        && !repeat.lunar
        && !repeat.forgetting_curve
    {
        match repeat.frequency {
            Frequency::Daily => Some(interval),
            Frequency::Weekly if repeat.weekdays == 0 => Some(7 * interval),
            _ => None,
        }
    } else {
        None
    };
    let Some(step) = step else { return start };
    let elapsed = (first.max(end + chrono::Duration::days(1)) - start)
        .num_days()
        .saturating_sub(1)
        .max(0);
    start + chrono::Duration::days(elapsed / step * step)
}

impl CalendarView {
    pub(super) fn bucket_range<'a>(
        &'a self,
        first: NaiveDate,
        last: NaiveDate,
        today: NaiveDate,
        cx: &App,
    ) -> DayBuckets<'a> {
        let mut tasks: HashMap<NaiveDate, Vec<&'a Task>> = HashMap::new();
        for task in &self.tasks {
            if task.is_deleted() || (!self.options.show_completed && task.is_completed()) {
                continue;
            }
            let Some(due) = task.due_date else { continue };
            let end = task.end_date.unwrap_or(due).max(due);
            if due <= last && end >= first {
                for date in calendar_year::dates_between(due.max(first), end.min(last)) {
                    tasks.entry(date).or_default().push(task);
                }
            }
            if let Some(repeat) = task
                .repeat
                .as_ref()
                .filter(|_| self.options.show_repeats && !task.is_closed() && end < last)
            {
                let mut cur = fast_forward_repeat_anchor(due, end, first, repeat);
                let mut current = repeat.clone();
                loop {
                    let next = current.next_after(cur);
                    if next <= cur || next > last {
                        break;
                    }
                    if next >= first && next > end {
                        tasks.entry(next).or_default().push(task);
                    }
                    cur = next;
                    current = current.advance();
                }
            }
        }

        let mut events: HashMap<NaiveDate, Vec<EventBar>> = HashMap::new();
        for event in self.events_between(first, last, cx) {
            let start = event.start.max(first);
            let end = event.end.min(last);
            if start <= end {
                for date in calendar_year::dates_between(start, end) {
                    events.entry(date).or_default().push(event.clone());
                }
            }
        }

        let mut habits: HashMap<NaiveDate, Vec<(&'a fern_core::Habit, Option<&'a fern_core::Checkin>)>> =
            HashMap::new();
        if self.options.show_habits && first <= today {
            let week_start = calendar::week_start(cx);
            let hist_last = last.min(today);
            for habit in &self.habits {
                let recs = self.habit_records.get(&habit.id).map(Vec::as_slice).unwrap_or(&[]);
                if (first..=last).contains(&today)
                    && !habit.archived
                    && habit.is_active_on(today, recs, week_start)
                {
                    let today_rec = recs.iter().find(|r| r.date == today);
                    habits.entry(today).or_default().push((habit, today_rec));
                }
                let mut seen_dates: HashSet<NaiveDate> = HashSet::new();
                for rec in recs {
                    if rec.date >= first
                        && rec.date < today
                        && rec.date <= hist_last
                        && seen_dates.insert(rec.date)
                        && (rec.is_handled() || rec.is_partial())
                    {
                        habits.entry(rec.date).or_default().push((habit, Some(rec)));
                    }
                }
            }
        }

        let mut focus: HashMap<NaiveDate, Vec<&'a fern_core::FocusSession>> = HashMap::new();
        if self.options.show_focus {
            for session in &self.focus_sessions {
                if let Some(start) = chrono::DateTime::from_timestamp_millis(session.started_at) {
                    let date = start.with_timezone(&Local).date_naive();
                    if (first..=last).contains(&date) {
                        focus.entry(date).or_default().push(session);
                    }
                }
            }
            for list in focus.values_mut() {
                list.sort_by_key(|s| s.started_at);
            }
        }

        let mut countdowns: HashMap<NaiveDate, Vec<&'a fern_core::Countdown>> = HashMap::new();
        if self.options.show_countdowns {
            for countdown in self.countdowns.iter().filter(|c| !c.archived) {
                if let Some(repeat) = &countdown.repeat {
                    if (first..=last).contains(&countdown.date) {
                        countdowns.entry(countdown.date).or_default().push(countdown);
                    }
                    let mut date = fast_forward_repeat_anchor(countdown.date, countdown.date, first, repeat);
                    let mut current = repeat.clone();
                    loop {
                        let next = current.next_after(date);
                        if next <= date || next > last {
                            break;
                        }
                        if next >= first && next > countdown.date {
                            countdowns.entry(next).or_default().push(countdown);
                        }
                        date = next;
                        current = current.advance();
                    }
                } else {
                    let yearly = countdown.kind != fern_core::CountdownKind::Countdown;
                    let years = if yearly {
                        first.year().max(countdown.date.year())..=last.year()
                    } else {
                        countdown.date.year()..=countdown.date.year()
                    };
                    for year in years {
                        let Some(date) = countdown
                            .date
                            .with_year(year)
                            .or_else(|| NaiveDate::from_ymd_opt(year, 2, 28))
                        else {
                            continue;
                        };
                        if (first..=last).contains(&date) {
                            countdowns.entry(date).or_default().push(countdown);
                        }
                    }
                }
            }
        }

        DayBuckets {
            tasks,
            events,
            habits,
            focus,
            countdowns,
        }
    }

    pub(super) fn day_extras_from_buckets(
        &self,
        buckets: &DayBuckets<'_>,
        date: NaiveDate,
        today: NaiveDate,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let habits = buckets.habits.get(&date).map(Vec::as_slice).unwrap_or(&[]);
        let focus = buckets.focus.get(&date).map(Vec::as_slice).unwrap_or(&[]);
        let countdowns = buckets.countdowns.get(&date).map(Vec::as_slice).unwrap_or(&[]);
        self.render_day_extras_from(date, today, habits, focus, countdowns, cx)
    }

    pub(super) fn day_extras(&self, date: NaiveDate, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let today = Local::now().date_naive();
        let week_start = calendar::week_start(cx);
        let mut habits: Vec<(&fern_core::Habit, Option<&fern_core::Checkin>)> = Vec::new();
        if self.options.show_habits && date <= today {
            for habit in &self.habits {
                let recs = self.habit_records.get(&habit.id).map(Vec::as_slice).unwrap_or(&[]);
                let day_rec = recs.iter().find(|r| r.date == date);
                let show = if date == today {
                    !habit.archived && habit.is_active_on(today, recs, week_start)
                } else {
                    day_rec.is_some_and(|r| r.is_handled() || r.is_partial())
                };
                if show {
                    habits.push((habit, day_rec));
                }
            }
        }
        let focus = self.options.focus_on(&self.focus_sessions, date);
        let countdowns: Vec<&fern_core::Countdown> = self
            .countdowns
            .iter()
            .filter(|c| !c.archived && self.options.shows_countdown(c, date))
            .collect();
        self.render_day_extras_from(date, today, &habits, &focus, &countdowns, cx)
    }

    fn render_day_extras_from(
        &self,
        date: NaiveDate,
        today: NaiveDate,
        habits: &[(&fern_core::Habit, Option<&fern_core::Checkin>)],
        focus: &[&fern_core::FocusSession],
        countdowns: &[&fern_core::Countdown],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let bar = |tint: gpui::Hsla| {
            h_flex()
                .h(px(18.))
                .flex_none()
                .gap_1()
                .px_1()
                .rounded(px(3.))
                .bg(tint.opacity(0.28))
        };
        let mut items: Vec<AnyElement> = Vec::new();
        for &(habit, day_rec) in habits {
            let done = day_rec.is_some_and(|r| r.is_done());
            let tint = match self.options.habit_color {
                fern_core::HabitColor::Theme => theme.colors().accent,
                fern_core::HabitColor::Fixed => crate::calendar_options_view::palette(self.options.habit_fixed),
                fern_core::HabitColor::Habit => {
                    crate::lists_panel::parse_color(habit.color.as_deref()).unwrap_or(theme.colors().accent)
                }
            };
            let id = habit.id;
            let key = (id, date);
            let open = self.open_habit == Some(key);
            let time_badge = (date == today)
                .then(|| habit.reminders.first().copied())
                .flatten()
                .map(|t| calendar::format_time(t, strings, cx));
            items.push(
                div()
                    .relative()
                    .child(
                        bar(tint)
                            .id(SharedString::from(format!("cal-habit-{id}-{date}")))
                            .cursor_pointer()
                            .when(done, |el| el.opacity(0.65))
                            .child(
                                Icon::new(if done { IconName::Check } else { IconName::FernHabit })
                                    .size(IconSize::XSmall)
                                    .color(if done { Color::Accent } else { Color::Muted }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Label::new(habit.name.clone()).size(LabelSize::XSmall).truncate()),
                            )
                            .when_some(time_badge, |el, t| {
                                el.child(Label::new(t).size(LabelSize::XSmall).color(Color::Muted))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.open_event = None;
                                this.open_habit = if this.open_habit == Some(key) { None } else { Some(key) };
                                cx.notify();
                            })),
                    )
                    .when(open, |el| {
                        el.child(PopoverAttachment::new(
                            Placement::BelowLeft,
                            self.render_habit_popover(habit, date, cx),
                        ))
                    })
                    .into_any_element(),
            );
        }
        // A focus record: an outlined bar with a timer, the task and the time it started.
        for session in focus {
            let start = chrono::DateTime::from_timestamp_millis(session.started_at).map(|t| t.with_timezone(&Local).time());
            items.push(
                h_flex()
                    .h(px(18.))
                    .flex_none()
                    .gap_1()
                    .px_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme.colors().accent.opacity(0.4))
                    .child(Icon::new(IconName::Timer).size(IconSize::XSmall).color(Color::Muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Label::new(session.title.clone()).size(LabelSize::XSmall).color(Color::Muted).truncate()),
                    )
                    .when_some(start, |el, start| {
                        el.child(Label::new(calendar::format_time(start, strings, cx)).size(LabelSize::XSmall).color(Color::Muted))
                    })
                    .into_any_element(),
            );
        }
        for countdown in countdowns {
            let tint = match self.options.countdown_color {
                fern_core::CountdownColor::Fixed => crate::calendar_options_view::palette(self.options.countdown_fixed),
                fern_core::CountdownColor::Countdown => {
                    crate::lists_panel::parse_color(countdown.color.as_deref()).unwrap_or(theme.status().warning)
                }
            };
            items.push(
                bar(tint)
                    .child(div().size(px(6.)).flex_none().rounded_full().bg(tint))
                    .child(Label::new(countdown.name.clone()).size(LabelSize::XSmall).truncate())
                    .into_any_element(),
            );
        }
        items
    }

    /// An event's bar: the calendar's color, its time when it has one, and a card with the rest
    /// on a click. Writable CalDAV events can also be dragged to another day.
    pub(super) fn event_bar(&self, event: EventBar, date: NaiveDate, cx: &mut Context<Self>) -> Stateful<ui::HorizontalStack> {
        let strings = i18n::t(cx);
        let open = self.open_event.as_ref() == Some(&(event.key.clone(), event.start));
        let key = (event.key.clone(), event.start);
        let column = calendar::week_of(date, cx).iter().position(|d| *d == date).unwrap_or(0);
        let card = event.clone();
        let draggable = event.writable && !event.recurring;
        let dragged = DraggedEvent {
            subscription_id: event.subscription_id,
            uid: event.key.clone(),
            title: event.title.clone().into(),
        };
        h_flex()
            .id(SharedString::from(format!("calendar-event-{}-{}-{date}", event.key, event.start)))
            .when(draggable, |el| {
                el.on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
            })
            .h(px(18.))
            .flex_none()
            .gap_1()
            .px_1()
            .rounded(px(3.))
            .cursor_pointer()
            .bg(event.color.opacity(0.18))
            .border_l_2()
            .border_color(event.color)
            .when_some(event.start_time.filter(|_| event.start == date), |el, time| {
                el.child(Label::new(calendar::format_time(time, strings, cx)).size(LabelSize::XSmall).color(Color::Muted))
            })
            .child(div().flex_1().min_w_0().child(Label::new(event.title).size(LabelSize::XSmall).truncate()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_event = if this.open_event.as_ref() == Some(&key) { None } else { Some(key.clone()) };
                window.focus(&this.focus_handle, cx);
                cx.notify();
            }))
            .when(open, |el| {
                let placement = if column < 3 { Placement::RightOf } else { Placement::LeftOf };
                el.relative().child(PopoverAttachment::new(placement, self.render_event_card(&card, cx)))
            })
    }

    fn render_day(
        &self,
        date: NaiveDate,
        today: NaiveDate,
        slots: usize,
        buckets: &DayBuckets<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let tasks = buckets.tasks.get(&date).map(Vec::as_slice).unwrap_or(&[]);
        let extras = self.day_extras_from_buckets(buckets, date, today, cx);
        let events = buckets.events.get(&date).map(Vec::as_slice).unwrap_or(&[]);
        let total = tasks.len() + events.len() + extras.len();
        let visible = if total > slots.min(TASKS_PER_DAY) {
            slots.saturating_sub(1).min(TASKS_PER_DAY)
        } else {
            total
        };
        let hidden = total - visible;
        // The subscribed calendars' events after the tasks, then the habits and the rest.
        let event_room = visible.saturating_sub(tasks.len());
        let room = event_room.saturating_sub(events.len());
        // The first day of a month carries the month name, as in the original: "Oct 1".
        let number = if date.day() == 1 {
            task_rows::short_date(date, strings)
        } else {
            date.day().to_string()
        };

        let accent = theme.colors().accent;
        let editor = self.task_editor.as_ref().filter(|editor| editor.day == Some(date));
        v_flex()
            .id(SharedString::from(format!("calendar-day-{date}")))
            .debug_selector(move || format!("calendar-day-{date}"))
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.12)))
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on_day(dragged.id, date, cx)))
            .drag_over::<ResizedTask>(move |style, _, _, _| style.bg(accent.opacity(0.14)))
            .on_drop(cx.listener(move |this, resized: &ResizedTask, _, cx| this.resize_task_to_day(resized, date, cx)))
            .drag_over::<DraggedEvent>(move |style, _, _, _| style.bg(accent.opacity(0.12)))
            .on_drop(cx.listener(move |this, event: &DraggedEvent, _, cx| this.move_event_to_day(event, date, cx)))
            .gap(px(DAY_ITEM_GAP))
            .px(px(2.))
            .pt(px(DAY_TOP_PADDING))
            .border_r_1()
            .border_b_1()
            .border_color(theme.colors().border_variant)
            .when(editor.is_some(), |el| el.bg(theme.colors().element_selected))
            .on_click(cx.listener(move |this, _, window, cx| this.open_day_task_editor(date, window, cx)))
            .child(
                h_flex().h(px(DAY_HEADER_HEIGHT)).flex_none().px_1().child(
                    div()
                        .id(SharedString::from(format!("calendar-select-day-{date}")))
                        .debug_selector(|| format!("calendar-select-day-{date}"))
                        .min_w(px(22.))
                        .h(px(22.))
                        .px_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .when(date == today, |el| el.bg(theme.colors().accent))
                        .when(date != today && self.selected_day == Some(date), |el| {
                            el.bg(theme.colors().element_selected)
                        })
                        .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                            cx.stop_propagation();
                            if event.click_count() >= 2 {
                                this.anchor = date;
                                this.month = calendar::month_start(date);
                                this.selected_day = Some(date);
                                this.set_mode(CalendarMode::Day, cx);
                            } else {
                                this.select_day(date, cx);
                            }
                        }))
                        // As in the original, the days of the months around are muted.
                        .child(Label::new(number).centered().weight(FontWeight::SEMIBOLD).color(
                            if date == today {
                                Color::OnAccent
                            } else if self.mode == CalendarMode::Month && date.month() != self.month.month() {
                                Color::Muted
                            } else {
                                Color::Default
                            },
                        )),
                ),
            )
            .child(
                v_flex()
                    .id("calendar-day-items")
                    .flex_none()
                    .gap(px(DAY_ITEM_GAP))
                    // Items and their popovers have their own actions. Only empty calendar
                    // space creates a task, including for non-task items and the more row.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .children(tasks.iter().take(visible).map(|task| self.task_bar_with_resize(task, Some(date), true, cx)))
                    .children(events.iter().take(event_room).cloned().map(|event| self.event_bar(event, date, cx)))
                    .children(extras.into_iter().take(room))
                    .when(hidden > 0 && slots > 0, |el| {
                        el.child(
                            h_flex()
                                .id(SharedString::from(format!("calendar-more-{date}")))
                                .h(px(DAY_ITEM_HEIGHT))
                                .flex_none()
                                .px_1()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.colors().element_hover))
                                .child(Label::new(format!("+{hidden}")).size(LabelSize::XSmall).color(Color::Accent))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.anchor = date;
                                    this.month = calendar::month_start(date);
                                    this.selected_day = Some(date);
                                    this.set_mode(CalendarMode::Day, cx);
                                })),
                        )
                    }),
            )
            .when_some(editor, |el, editor| {
                let days: Vec<_> = calendar::week_of(date, cx).into_iter().filter(|day| self.options.shows_day(*day)).collect();
                let column = days.iter().position(|day| *day == date).unwrap_or(0);
                let placement = if column < days.len() / 2 { Placement::RightOf } else { Placement::LeftOf };
                el.child(PopoverAttachment::new(placement, editor.view.clone()))
            })
            .into_any_element()
    }

    /// A task's bar on `day`; `None` in the Year view's day card, where a click opens the task in
    /// the list.
    pub(super) fn task_bar(&self, task: &Task, day: Option<NaiveDate>, cx: &mut Context<Self>) -> Stateful<ui::HorizontalStack> {
        self.task_bar_with_resize(task, day, false, cx)
    }

    pub(super) fn task_bar_with_resize(
        &self,
        task: &Task,
        day: Option<NaiveDate>,
        horizontal_resize: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<ui::HorizontalStack> {
        let theme = cx.theme().clone();
        let id = task.id;
        // Keep closed titles muted; the checkbox distinguishes completed and dismissed tasks, while notes show their page icon.
        let done = task.is_closed();
        let completed = task.is_completed();
        let wont_do = task.is_wont_do();
        let tint = self.task_tint(task, &theme);
        let modern = self.options.modern;
        let dragged = DraggedTask {
            id,
            title: task.title.clone().into(),
        };
        let marker = if task.kind == fern_core::Kind::Note {
            Icon::new(IconName::FileText)
                .size(IconSize::XSmall)
                .color(if done { Color::Muted } else { Color::IconMuted })
                .into_any_element()
        } else {
            div()
                .debug_selector(move || format!("calendar-task-checkbox-{id}"))
                .flex_none()
                .flex()
                .items_center()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Checkbox::new(SharedString::from(format!("calendar-task-checkbox-{id}")))
                        .size(px(12.))
                        .toggle_state(completed)
                        .in_progress(task.is_in_progress())
                        .dismissed(wont_do)
                        .lines(task.checklist.is_some())
                        .tint(task_rows::priority_color(task.priority, &theme))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_task_completed(id, !completed && !wont_do, cx);
                        })),
                )
                .into_any_element()
        };
        let title = if task.title.trim().is_empty() {
            i18n::t(cx).untitled.to_owned()
        } else {
            task.title.clone()
        };
        let handle_title: SharedString = title.clone().into();
        let span_start = task.due_date.unwrap_or_else(|| Local::now().date_naive());
        let span_end = task.end_date.unwrap_or(span_start).max(span_start);
        let resize_handle = |edge: ResizeEdge| {
            let tag = if edge == ResizeEdge::Start { "start" } else { "end" };
            let payload = ResizedTask {
                id,
                title: handle_title.clone(),
                start: span_start,
                end: span_end,
                edge,
            };
            div()
                .id(SharedString::from(format!("calendar-task-resize-{tag}-{id}")))
                .flex_none()
                .w(px(5.))
                .h_full()
                .cursor_ew_resize()
                .on_drag(payload, |d, _, _, cx| cx.new(|_| d.clone()))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, _, cx| cx.stop_propagation())
        };
        let strings = i18n::t(cx);
        let tip: SharedString = match task.due_time {
            Some(time) => format!("{} {}", calendar::format_time(time, strings, cx), title).into(),
            None => title.clone().into(),
        };
        let body = h_flex()
            .id(SharedString::from(format!("calendar-task-body-{id}")))
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .gap_1()
            .tooltip(ui::Tooltip::text(tip))
            .when(horizontal_resize, |el| {
                el.on_drag(dragged.clone(), |d, _, _, cx| cx.new(|_| d.clone()))
            })
            .child(marker)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(done, |el| el.line_through())
                    .child(
                        Label::new(title)
                            .size(LabelSize::XSmall)
                            .color(if done { Color::Muted } else { Color::Default })
                            .truncate(),
                    ),
            );
        h_flex()
            .id(SharedString::from(format!("calendar-task-{id}")))
            .debug_selector(|| format!("calendar-task-{id}"))
            .when(!horizontal_resize, |el| {
                el.on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
            })
            .h(px(18.))
            .flex_none()
            .gap_0p5()
            .px(if horizontal_resize { px(0.) } else { px(4.) })
            .rounded(px(3.))
            .cursor_pointer()
            // Style → Item Style: Classic fills the bar, Modern tints it lighter with a colored edge.
            .bg(tint.opacity(match (done, modern) {
                (true, _) => 0.12,
                (false, true) => 0.16,
                (false, false) => 0.28,
            }))
            .when(modern, |el| el.border_l_2().border_color(tint))
            .hover(|s| s.bg(tint.opacity(0.4)))
            .when(horizontal_resize, |el| el.child(resize_handle(ResizeEdge::Start)))
            .child(body)
            .when(horizontal_resize, |el| el.child(resize_handle(ResizeEdge::End)))
            .on_click(cx.listener(move |this, _, _, cx| match day {
                Some(day) => this.open_card(id, day, cx),
                None => cx.emit(OpenTask(id)),
            }))
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    this.open_context(id, event.position, cx);
                    window.focus(&this.focus_handle, cx);
                }),
            )
            // As in the original, the card opens on the bar's left, or on its right in the first
            // days of the week, where there is no room.
            .when_some(day.filter(|d| self.card == Some((id, *d))), |el, day| {
                let column = calendar::week_of(day, cx).iter().position(|d| *d == day).unwrap_or(0);
                let placement = if column < 3 { Placement::RightOf } else { Placement::LeftOf };
                el.relative().child(PopoverAttachment::new(placement, self.render_card(cx)))
            })
    }

    pub(super) fn render_month_grid(&self, today: NaiveDate, has_horizon_banner: bool, window: &Window, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let shown_weeks = self.month_grid_weeks(cx);
        let buckets = shown_weeks
            .first()
            .zip(shown_weeks.last())
            .map(|(first_w, last_w)| self.bucket_range(first_w[0], last_w[6], today, cx));
        let banner_height = if has_horizon_banner { HORIZON_BANNER_HEIGHT } else { 0. };
        let grid_height = window.viewport_size().height
            - crate::title_bar::header_inset(window, cx)
            - crate::title_bar::frame_vertical_inset(window)
            - px(CALENDAR_HEADER_HEIGHT + WEEKDAY_HEADER_HEIGHT + banner_height + 1.);
        let slots = day_item_slots(grid_height / shown_weeks.len().max(1) as f32);
        v_flex()
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(colors.border_variant)
            .children(shown_weeks.into_iter().map(|week| {
                h_flex()
                    .flex_1()
                    .items_stretch()
                    .min_h_0()
                    // Settings → Show Week Numbers: "W42" before each week, as in the original.
                    .when(calendar::week_numbers(cx), |el| {
                        el.child(
                            div()
                                .w(px(WEEK_NUMBER_WIDTH))
                                .flex_none()
                                .pt_1()
                                .flex()
                                .justify_center()
                                .child(
                                    Label::new(format!("W{}", calendar::week_number(&week)))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                ),
                        )
                    })
                    .children(
                        week.into_iter()
                            .filter(|d| self.options.shows_day(*d))
                            .filter_map(|date| buckets.as_ref().map(|b| self.render_day(date, today, slots, b, cx))),
                    )
            }))
    }
}
