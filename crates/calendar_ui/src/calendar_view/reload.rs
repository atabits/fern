use std::{collections::HashSet, time::Duration};

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{ColorBy, Id, Task};
use ui::prelude::*;

use crate::calendar;
use crate::i18n;
use crate::task_detail::{TaskTextEdited, TaskTextField};
use crate::task_rows;

use super::{AGENDA_DAYS, CalendarMode, CalendarView, EventBar, calendar_year, sidebar::CalendarFilter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReloadWindow {
    Agenda(NaiveDate),
    Year,
    Grid(NaiveDate, NaiveDate),
}

impl CalendarView {
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.refresh.set_active(active, cx) {
            self.card_updated(cx);
        }
    }

    pub fn sync_external(&mut self, cx: &mut Context<Self>) {
        if self.refresh.mark_dirty_or_reload() {
            self.card_updated(cx);
        } else {
            self.loaded_window = None;
        }
    }

    /// The days on the hour axis: the anchor's week, the anchor, or several days from it.
    pub(super) fn shown_days(&self, cx: &App) -> Vec<NaiveDate> {
        match self.mode {
            CalendarMode::Day => vec![self.anchor],
            CalendarMode::MultiDay(n) => (0..n as i64).map(|i| self.anchor + chrono::Duration::days(i)).collect(),
            _ => calendar::week_of(self.anchor, cx).into_iter().filter(|d| self.options.shows_day(*d)).collect(),
        }
    }

    pub(super) fn month_grid_weeks(&self, cx: &App) -> Vec<[NaiveDate; 7]> {
        match self.mode {
            CalendarMode::MultiWeek(n) => calendar::weeks_from(self.anchor, n as usize, cx),
            _ => calendar::month_weeks(self.month, cx),
        }
    }

    /// The tasks a day shows, as View Options say.
    pub(super) fn tasks_on(&self, date: NaiveDate) -> Vec<&Task> {
        self.tasks.iter().filter(|t| self.options.shows_task(t, date)).collect()
    }

    /// A task's color, by its list, first tag or priority as View Options → Color By says.
    pub(super) fn task_tint(&self, task: &Task, theme: &theme::Theme) -> gpui::Hsla {
        let accent = theme.colors().accent;
        match self.options.color_by {
            ColorBy::List => task.list_id.and_then(|id| self.list_colors.get(&id).copied()).unwrap_or(accent),
            ColorBy::Tag => task.tags.first().and_then(|t| self.tag_colors.get(&t.to_lowercase()).copied()).unwrap_or(accent),
            ColorBy::Priority => task_rows::priority_color(task.priority, theme).unwrap_or(accent),
        }
    }

    /// The subscribed calendars' events on `date`, all-day ones first, then by time.
    pub(super) fn events_on(&self, date: NaiveDate, cx: &App) -> Vec<EventBar> {
        self.events_between(date, date, cx)
    }

    /// The subscribed calendars' events covering any day from `from` to `to`.
    pub(super) fn events_between(&self, from: NaiveDate, to: NaiveDate, cx: &App) -> Vec<EventBar> {
        let Some(feeds) = crate::subscriptions::feeds(cx) else { return Vec::new() };
        let accent = cx.theme().colors().accent;
        let key = (
            from,
            to,
            self.feeds_revision,
            [accent.h.to_bits(), accent.s.to_bits(), accent.l.to_bits(), accent.a.to_bits()],
        );
        if let Some(cached) = self.events_cache.borrow().get(&key) {
            return cached.clone();
        }
        let mut events: Vec<EventBar> = feeds
            .read(cx)
            .occurrences(from, to)
            .into_iter()
            .map(|occurrence| EventBar {
                key: occurrence.event.uid.clone(),
                subscription_id: occurrence.calendar.id,
                writable: occurrence.calendar.is_caldav() && occurrence.calendar.writable,
                recurring: occurrence.event.repeat.is_some(),
                title: occurrence.event.title.clone(),
                start: occurrence.times.start,
                end: occurrence.times.end,
                start_time: occurrence.times.start_time,
                end_time: occurrence.times.end_time,
                location: occurrence.event.location.clone(),
                notes: occurrence.event.notes.clone(),
                calendar: occurrence.calendar.title().to_owned(),
                color: crate::lists_panel::parse_color(occurrence.calendar.color.as_deref()).unwrap_or(accent),
            })
            .collect();
        events.sort_by_key(|event| (event.start_time.is_some(), event.start_time));
        let mut cache = self.events_cache.borrow_mut();
        if cache.len() > 32 {
            cache.clear();
        }
        cache.insert(key, events.clone());
        events
    }

    /// The card's task changed: the calendar shows the change, and the card closes when the task
    /// is gone.
    pub(super) fn card_updated(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
        let Some((id, _)) = self.card else { return };
        match self.store.task(id) {
            Ok(Some(task)) if !task.is_deleted() => self.detail.update(cx, |detail, cx| detail.refresh(Some(task), cx)),
            _ => self.close_card(cx),
        }
    }

    pub(super) fn apply_task_text_edit(&mut self, edit: &TaskTextEdited, cx: &mut Context<Self>) {
        if self.refresh.is_dirty() {
            self.reload(cx);
        } else if self.mode == CalendarMode::Agenda {
            if let TaskTextField::Title(title) = &edit.field {
                for (_, items) in &mut self.agenda {
                    for item in items.iter_mut().filter(|i| !i.countdown && i.task == edit.id) {
                        item.title = title.clone();
                    }
                }
                cx.notify();
            }
        } else {
            for list in [&mut self.tasks, &mut self.undated, &mut self.horizon_tasks] {
                for task in list.iter_mut().filter(|t| t.id == edit.id) {
                    match &edit.field {
                        TaskTextField::Title(title) => task.title = title.clone(),
                        TaskTextField::Notes(notes) => task.notes = notes.clone(),
                        TaskTextField::Location(location) => task.location = location.clone(),
                    }
                }
            }
            cx.notify();
        }
    }

    fn reload_window(&self, cx: &App) -> ReloadWindow {
        match self.mode {
            CalendarMode::Agenda => ReloadWindow::Agenda(self.agenda_start()),
            CalendarMode::Year => ReloadWindow::Year,
            CalendarMode::Month | CalendarMode::MultiWeek(_) => {
                let weeks = self.month_grid_weeks(cx);
                match weeks.first().zip(weeks.last()) {
                    Some((first, last)) => ReloadWindow::Grid(first[0], last[6]),
                    None => ReloadWindow::Grid(self.month, self.month),
                }
            }
            _ => {
                let days = self.shown_days(cx);
                match days.first().copied().zip(days.last().copied()) {
                    Some((start, end)) => ReloadWindow::Grid(start, end),
                    None => ReloadWindow::Grid(self.anchor, self.anchor),
                }
            }
        }
    }

    fn reload_if_needed(&mut self, cx: &mut Context<Self>) {
        if self.refresh.is_dirty()
            || self.context_menu.is_some()
            || self.loaded_window != Some(self.reload_window(cx))
        {
            self.reload(cx);
        } else {
            cx.notify();
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.refresh.mark_clean(cx);
        self.year_heatmap.clear();
        self.events_cache.borrow_mut().clear();
        self.options = self.store.calendar_options().unwrap_or_default();
        self.preferences = self.store.preferences().unwrap_or_default();
        let lists = self.store.lists().unwrap_or_default();
        self.list_colors.clear();
        self.list_names.clear();
        for list in &lists {
            if let Some(color) = crate::lists_panel::parse_color(list.color.as_deref()) {
                self.list_colors.insert(list.id, color);
            }
            if !list.archived {
                self.list_names.push((list.id, list.name.clone()));
            }
        }
        self.lists = lists;
        if let Some(menu) = &mut self.context_menu {
            match self.store.task(menu.id) {
                Ok(Some(task)) if !task.is_deleted() => {
                    menu.nesting = crate::task_menu::TaskNesting::of(&self.store, task.id);
                    menu.task = task;
                }
                _ => {
                    self.context_menu = None;
                    self.context_side = None;
                }
            }
        }
        let all_tags = self.store.tags().unwrap_or_default();
        self.tag_names.clear();
        self.tag_colors.clear();
        self.tag_names.reserve(all_tags.len());
        for tag in all_tags {
            if let Some(color) = crate::lists_panel::parse_color(tag.color.as_deref()) {
                self.tag_colors.insert(tag.name.to_lowercase(), color);
            }
            self.tag_names.push(tag.name);
        }
        self.filter = self
            .store
            .setting("calendar_list_filter")
            .ok()
            .flatten()
            .map(|raw| CalendarFilter::parse(&raw, &self.list_names, &self.tag_names))
            .unwrap_or_default();
        if self.mode == CalendarMode::Agenda {
            let from = self.agenda_start();
            if self.options.show_countdowns {
                self.countdowns = self.store.countdowns().unwrap_or_default();
            } else {
                self.countdowns.clear();
            }
            match self.store.tasks() {
                Ok(mut tasks) => {
                    if !self.filter.all {
                        tasks.retain(|task| self.filter.matches(task));
                    }
                    self.sidebar_busy_days.clear();
                    self.sidebar_busy_days.extend(
                        tasks
                            .iter()
                            .filter(|t| !t.is_closed() && !t.is_deleted())
                            .filter_map(|t| t.due_date),
                    );
                    let until = from + chrono::Duration::days(AGENDA_DAYS);
                    let has_skip_holidays = tasks.iter().any(|t| {
                        !t.is_closed()
                            && !t.is_deleted()
                            && t.due_date.is_some()
                            && t.repeat.as_ref().is_some_and(|r| r.skip_holidays)
                    });
                    let holidays = if has_skip_holidays {
                        self.store.holiday_dates(from, until).unwrap_or_default()
                    } else {
                        HashSet::new()
                    };
                    self.agenda = fern_core::agenda::agenda_with_holidays(
                        &tasks,
                        &self.countdowns,
                        &holidays,
                        from,
                        until,
                    );
                    self.loaded_window = Some(ReloadWindow::Agenda(from));
                }
                Err(err) => {
                    self.loaded_window = None;
                    desktop_common::logging::log_error(
                        "calendar_ui",
                        format!("failed to load the agenda: {err:#}"),
                    );
                }
            }
            cx.notify();
            return;
        }
        let window = self.reload_window(cx);
        let range = match window {
            ReloadWindow::Grid(start, end) => Some((start, end)),
            ReloadWindow::Year | ReloadWindow::Agenda(_) => None,
        };
        let Ok(mut all) = self.store.tasks().inspect_err(|err| {
            desktop_common::logging::log_error(
                "calendar_ui",
                format!("failed to load calendar tasks: {err:#}"),
            );
        }) else {
            self.loaded_window = None;
            cx.notify();
            return;
        };
        if !self.filter.all {
            all.retain(|task| self.filter.matches(task));
        }
        self.sidebar_busy_days.clear();
        self.tasks.clear();
        self.undated.clear();
        self.horizon_tasks.clear();
        let mut repeats_before_range = Vec::new();
        let horizons_enabled = calendar::horizons_enabled(cx);
        for task in all.into_iter().filter(|task| !task.is_deleted()) {
            let closed = task.is_closed();
            if !closed && let Some(due) = task.due_date {
                self.sidebar_busy_days.insert(due);
            }
            let push_horizon = horizons_enabled && !closed && task.horizon.is_some();
            let Some(due) = task.due_date else {
                if !closed && task.parent_id.is_none() {
                    if push_horizon {
                        self.horizon_tasks.push(task.clone());
                    }
                    self.undated.push(task);
                } else if push_horizon {
                    self.horizon_tasks.push(task);
                }
                continue;
            };
            let in_range = range
                .is_none_or(|(start, end)| due <= end && task.end_date.unwrap_or(due).max(due) >= start);
            let is_repeat_before = !in_range
                && range.is_some_and(|(start, _)| due < start)
                && self.options.show_repeats
                && task.repeat.is_some()
                && !closed;
            if in_range {
                if push_horizon {
                    self.horizon_tasks.push(task.clone());
                }
                self.tasks.push(task);
            } else if is_repeat_before {
                if push_horizon {
                    self.horizon_tasks.push(task.clone());
                }
                repeats_before_range.push(task);
            } else if push_horizon {
                self.horizon_tasks.push(task);
            }
        }
        self.tasks.sort_by_key(|task| (task.due_date, task.due_time.is_some(), task.due_time));
        self.tasks.extend(repeats_before_range);
        if self.options.show_countdowns {
            self.countdowns = self.store.countdowns().unwrap_or_default();
        } else {
            self.countdowns.clear();
        }
        if self.options.show_habits {
            self.habits = self.store.habits().unwrap_or_default();
            self.habit_records = self.store.all_habit_checkins().unwrap_or_default();
        } else {
            self.habits.clear();
            self.habit_records.clear();
        }
        if self.options.show_focus {
            self.focus_sessions = self.store.focus_sessions().unwrap_or_default();
        } else {
            self.focus_sessions.clear();
        }
        self.loaded_window = Some(window);
        cx.notify();
    }

    /// The Agenda starts today in the current month, and on the first day of a later one.
    pub(super) fn agenda_start(&self) -> NaiveDate {
        self.selected_day.unwrap_or_else(|| Local::now().date_naive().max(self.month))
    }

    pub(super) fn can_shift(&self, delta: i32) -> bool {
        if self.mode != CalendarMode::Year {
            return true;
        }
        calendar_year::YEAR_RANGE.contains(&(self.month.year() + delta))
    }

    pub(super) fn shift(&mut self, delta: i32, cx: &mut Context<Self>) {
        if !self.can_shift(delta) || !self.finish_task_before_navigation(cx) {
            return;
        }
        if self.mode == CalendarMode::Year {
            let january = NaiveDate::from_ymd_opt(self.month.year() + delta, 1, 1).unwrap_or(self.month);
            self.navigate_year(january, cx);
            return;
        }
        self.selected_day = None;
        self.sidebar_month = None;
        self.year_day = None;
        self.open_event = None;
        match self.mode {
            CalendarMode::Month => self.month = calendar::add_months(self.month, delta),
            CalendarMode::Agenda => {
                self.month = calendar::add_months(calendar::month_start(self.agenda_start()), delta);
            }
            mode => {
                let step = match mode {
                    CalendarMode::Day => 1,
                    CalendarMode::MultiDay(n) => n as i64,
                    CalendarMode::MultiWeek(n) => 7 * n as i64,
                    _ => 7,
                };
                self.anchor += chrono::Duration::days(step * delta as i64);
                self.month = calendar::month_start(self.anchor);
            }
        }
        self.card = None;
        self.reload_if_needed(cx);
    }

    pub fn set_mode(&mut self, mode: CalendarMode, cx: &mut Context<Self>) {
        self.mode_menu_open = false;
        self.apply_mode(mode, cx);
    }

    pub(super) fn apply_mode(&mut self, mode: CalendarMode, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let prev_mode = self.mode;
        self.mode = mode;
        if prev_mode == CalendarMode::Year
            && mode != CalendarMode::Year
            && self.anchor.year() == self.month.year()
        {
            self.month = calendar::month_start(self.anchor);
        }
        self.year_day = None;
        self.year_scroll = calendar_year::YearScroll::default();
        self.year_jump = None;
        match mode {
            CalendarMode::Year => {
                let day = calendar_year::clamp_year_day(self.month);
                if day != self.month {
                    self.anchor = day;
                    self.selected_day = None;
                    self.sidebar_month = None;
                }
                self.month = NaiveDate::from_ymd_opt(day.year(), 1, 1).unwrap_or(self.month);
            }
            CalendarMode::MultiDay(days) => self.multi_days = days,
            CalendarMode::MultiWeek(weeks) => self.multi_weeks = weeks,
            _ => {}
        }
        if (mode.is_timed() || matches!(mode, CalendarMode::MultiWeek(_))) && calendar::month_start(self.anchor) != self.month {
            self.anchor = self.month;
        }
        self.card = None;
        self.reload_if_needed(cx);
    }

    pub(super) fn go_to_today(&mut self, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let today = Local::now().date_naive();
        if self.mode == CalendarMode::Year {
            self.anchor = calendar_year::clamp_year_day(today);
            self.month = NaiveDate::from_ymd_opt(self.anchor.year(), 1, 1).unwrap_or(self.month);
        } else {
            self.anchor = today;
            self.month = calendar::month_start(today);
        }
        self.selected_day = None;
        self.sidebar_month = None;
        self.year_day = None;
        self.open_event = None;
        self.year_scroll = calendar_year::YearScroll::default();
        self.reload_if_needed(cx);
    }

    pub(super) fn select_month(&mut self, year: i32, month: u32, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let today = Local::now().date_naive();
        let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(self.month);
        let target = if today.year() == year && today.month() == month { today } else { first };
        self.month = first;
        self.anchor = target;
        self.selected_day = None;
        self.sidebar_month = None;
        self.month_picker_open = None;
        self.year_day = None;
        self.open_event = None;
        self.close_card(cx);
        self.reload_if_needed(cx);
    }

    pub(super) fn select_day(&mut self, day: NaiveDate, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let day = if self.mode == CalendarMode::Year {
            let day = calendar_year::clamp_year_day(day);
            self.month = NaiveDate::from_ymd_opt(day.year(), 1, 1).unwrap_or(self.month);
            self.year_scroll = calendar_year::YearScroll::default();
            day
        } else {
            self.month = calendar::month_start(day);
            day
        };
        self.selected_day = Some(day);
        self.anchor = day;
        self.sidebar_month = None;
        self.year_day = None;
        self.open_event = None;
        self.close_card(cx);
        self.reload_if_needed(cx);
    }

    pub(super) fn new_task_day(&self, today: NaiveDate, cx: &App) -> NaiveDate {
        use chrono::Duration;
        let (start, end) = match self.mode {
            CalendarMode::Month => (self.month, calendar::add_months(self.month, 1).pred_opt().unwrap_or(self.month)),
            CalendarMode::Year => {
                (self.month, calendar::add_months(self.month, 12).pred_opt().unwrap_or(self.month))
            }
            CalendarMode::MultiWeek(count) => {
                let start = calendar::week_of(self.anchor, cx)[0];
                (start, start + Duration::days(i64::from(count) * 7 - 1))
            }
            CalendarMode::Agenda => (self.agenda_start(), self.agenda_start() + Duration::days(AGENDA_DAYS)),
            _ => {
                let days = self.shown_days(cx);
                (days[0], days[days.len() - 1])
            }
        };
        let visible = |day| (start..=end).contains(&day)
            && (!matches!(self.mode, CalendarMode::Month | CalendarMode::Week | CalendarMode::MultiWeek(_)) || self.options.shows_day(day));
        self.selected_day.filter(|day| visible(*day))
            .or_else(|| visible(today).then_some(today))
            .or_else(|| start.iter_days().take_while(|day| *day <= end).find(|day| visible(*day)))
            .unwrap_or(start)
    }

    /// Changing the view must never detach a cell's editor while its draft is still active.
    pub fn finish_task_before_navigation(&mut self, cx: &mut Context<Self>) -> bool {
        self.task_editor.as_ref().is_none_or(|editor| editor.view.update(cx, |editor, cx| editor.close(cx)))
    }

    pub(super) fn hint_if_outside_filter(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.filter.all {
            return;
        }
        let Ok(Some(task)) = self.store.task(id) else {
            return;
        };
        if self.filter.matches(&task) {
            return;
        }
        let strings = i18n::t(cx);
        let list = self.list_names.iter().find(|(id, _)| Some(*id) == task.list_id)
            .map_or(strings.inbox, |(_, name)| name.as_str());
        let message = strings.calendar_saved_outside_filter.replace("{list}", list);
        let timeout = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            let _ = this.update(cx, |this, cx| {
                this.saved_hint = None;
                cx.notify();
            });
        });
        self.saved_hint = Some((message, timeout));
    }

    /// ISO week horizon for the currently displayed week in Week view.
    pub(super) fn displayed_week_horizon(&self, cx: &App) -> fern_core::Horizon {
        let week = calendar::week_of(self.anchor, cx);
        let ref_day = week
            .iter()
            .find(|d| d.weekday() == chrono::Weekday::Thu)
            .copied()
            .unwrap_or(week[3]);
        fern_core::Horizon::current_week(ref_day)
    }

    /// Collects open non-deleted tasks for `target`, plus unfinished past tasks of the same
    /// granularity when `include_past_when_current` is true and `target` is the current period.
    /// Returns `(task, is_rollover)` pairs with exact matches first, followed by past rollovers.
    pub(super) fn horizon_goals_for(
        &self,
        target: fern_core::Horizon,
        today: NaiveDate,
        include_past_when_current: bool,
        cx: &App,
    ) -> Vec<(&Task, bool)> {
        if !calendar::horizons_enabled(cx) {
            return Vec::new();
        }
        let lists: &[fern_core::List] = if self.filter.all { &self.lists } else { &[] };
        let (in_target, past_rollover) = fern_core::horizon::horizons_bucket_partition(
            &self.horizon_tasks,
            lists,
            target,
            today,
            false,
        );
        let mut out: Vec<(&Task, bool)> = in_target.into_iter().map(|t| (t, false)).collect();
        if include_past_when_current {
            out.extend(past_rollover.into_iter().map(|t| (t, true)));
        }
        out
    }

    pub(super) fn toolbar_month(&self, cx: &App) -> NaiveDate {
        match self.mode {
            CalendarMode::Month | CalendarMode::Year => self.month,
            CalendarMode::Agenda => calendar::month_start(self.agenda_start()),
            CalendarMode::Week | CalendarMode::MultiWeek(_) => calendar::week_of(self.anchor, cx)[0],
            _ => self.anchor,
        }
    }
}
