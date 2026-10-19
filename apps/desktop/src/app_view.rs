use std::rc::Rc;

use fern_core::Store;
use gpui::Focusable as _;
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::actions::{
    NewTask, OpenSettings, Search, ShowInbox, ShowNext7Days, ShowShortcuts, ShowToday, ShowTomorrow, ToggleSidebar,
};
use crate::calendar_view::{self, CalendarView};
use crate::countdown_view::CountdownView;
use crate::focus_view::FocusView;
use crate::habit_view::HabitView;
use crate::i18n;
use crate::matrix_view::{self, MatrixView};
use crate::reminders::{self, Notice, NoticeAction, Notices, Reply, Target};
use crate::search_modal::{SearchEvent, SearchModal};
use crate::settings_window::{self, SettingsEvent, SettingsWindow};
use crate::sidebar::{self, Section};
use crate::title_bar::{self, TitleBar};
use crate::todo_view::TodoView;

pub struct AppView {
    store: Rc<Store>,
    section: Section,
    todo: gpui::Entity<TodoView>,
    calendar: gpui::Entity<CalendarView>,
    matrix: gpui::Entity<MatrixView>,
    countdown: gpui::Entity<CountdownView>,
    habits: gpui::Entity<HabitView>,
    focus: gpui::Entity<FocusView>,
    title_bar: gpui::Entity<TitleBar>,
    search: Option<gpui::Entity<SearchModal>>,
    account_menu_open: bool,
    notices: Notices,
    notices_open: bool,
    sync: crate::sync::SyncStatus,
    /// Settings → Features: the sections shown in the sidebar.
    features: fern_core::Features,
    /// The "Shortcuts" panel on the right, while open.
    shortcuts_open: bool,
    settings: Option<gpui::Entity<SettingsWindow>>,
    /// The Statistics page opened from the account menu, which covers the whole window.
    statistics_open: bool,
    /// The sidebar icon under the pointer, whose name shows beside it.
    nav_hovered: Option<Section>,
}

impl AppView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let todo = cx.new(|cx| TodoView::new(store.clone(), window, cx));
        let calendar = cx.new(|cx| CalendarView::new(store.clone(), window, cx));
        // A task clicked in the calendar opens in the task list with its details.
        cx.subscribe_in(&calendar, window, |this, _, calendar_view::OpenTask(id), window, cx| {
            let id = *id;
            this.select(Section::Tasks, window, cx);
            this.todo.update(cx, |todo, cx| {
                todo.set_view(fern_core::View::All, cx);
                todo.select(id, cx);
            });
        })
        .detach();
        // A picker asked for in the calendar's right-click menu opens with the task in the list.
        cx.subscribe_in(&calendar, window, |this, _, calendar_view::OpenTaskPicker(id, what), window, cx| {
            let (id, what) = (*id, *what);
            this.select(Section::Tasks, window, cx);
            this.todo.update(cx, |todo, cx| {
                todo.set_view(fern_core::View::All, cx);
                todo.start_detail(id, what, window, cx);
            });
        })
        .detach();
        cx.subscribe_in(&calendar, window, |this, _, crate::task_detail::StartFocus(id, title, stopwatch), window, cx| {
            let (target, stopwatch) = ((*id, title.clone()), *stopwatch);
            this.select(Section::Pomodoro, window, cx);
            this.focus.update(cx, |focus, cx| focus.start_for(Some(target.0), target.1, stopwatch, cx));
        })
        .detach();
        // "Start Focus" in a task's menu starts a Pomodoro for it in the Focus section.
        cx.subscribe_in(&todo, window, |this, _, crate::task_detail::StartFocus(id, title, stopwatch), window, cx| {
            let (target, stopwatch) = ((*id, title.clone()), *stopwatch);
            this.select(Section::Pomodoro, window, cx);
            this.focus.update(cx, |focus, cx| focus.start_for(Some(target.0), target.1, stopwatch, cx));
        })
        .detach();
        let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
        cx.subscribe_in(&matrix, window, |this, _, matrix_view::OpenTask(id), window, cx| {
            let id = *id;
            this.select(Section::Tasks, window, cx);
            this.todo.update(cx, |todo, cx| {
                todo.set_view(fern_core::View::All, cx);
                todo.select(id, cx);
            });
        })
        .detach();
        let countdown = cx.new(|cx| CountdownView::new(store.clone(), cx));
        let habits = cx.new(|cx| HabitView::new(store.clone(), cx));
        // "Start Focus" in a habit's menu starts a Pomodoro or a stopwatch named after it.
        cx.subscribe_in(&habits, window, |this, _, event: &crate::habit_view::StartFocus, window, cx| {
            let (title, stopwatch) = (event.title.clone(), event.stopwatch);
            this.select(Section::Pomodoro, window, cx);
            this.focus.update(cx, |focus, cx| focus.start_named(title, stopwatch, cx));
        })
        .detach();
        let focus = cx.new(|cx| FocusView::new(store.clone(), cx));
        cx.subscribe(&focus, |this, _, crate::focus_view::StatsClosed, cx| {
            this.statistics_open = false;
            cx.notify();
        })
        .detach();
        let store_features = store.preferences().map(|p| p.features).unwrap_or_default();
        // Mutated only by the debug-build dev state below.
        #[cfg_attr(not(debug_assertions), allow(unused_mut))]
        let mut this = Self {
            title_bar: cx.new(|_| TitleBar::new()),
            store,
            section: Section::Tasks,
            todo,
            calendar,
            matrix,
            countdown,
            habits,
            focus,
            search: None,
            account_menu_open: false,
            notices: Notices::default(),
            notices_open: false,
            sync: crate::sync::SyncStatus::Off,
            features: store_features,
            shortcuts_open: false,
            settings: None,
            statistics_open: false,
            nav_hovered: None,
        };
        reminders::start(this.store.clone(), window.window_handle(), cx);
        this.start_sync(cx);
        settings_window::apply(&this.store.preferences().unwrap_or_default(), window, cx);
        // "Follow System Dark Mode" tracks the system's appearance while the app runs.
        cx.observe_window_appearance(window, |this, window, cx| {
            let preferences = this.store.preferences().unwrap_or_default();
            if preferences.theme == fern_core::ThemeMode::System {
                settings_window::apply(&preferences, window, cx);
            }
        })
        .detach();
        #[cfg(debug_assertions)]
        if let Ok(state) = std::env::var("FERN_DEV_STATE") {
            if state.contains("section=calendar") {
                this.select(Section::Calendar, window, cx);
            }
            if state.contains("calendar=week") {
                this.calendar.update(cx, |c, cx| c.set_mode(crate::calendar_view::CalendarMode::Week, cx));
            }
            if state.contains("calendar=day") {
                this.calendar.update(cx, |c, cx| c.set_mode(crate::calendar_view::CalendarMode::Day, cx));
            }
            if state.contains("calendar=multi") {
                this.calendar.update(cx, |c, cx| c.set_mode(crate::calendar_view::CalendarMode::MultiDay(3), cx));
            }
            if state.contains("calendar=year") {
                this.calendar.update(cx, |c, cx| c.set_mode(crate::calendar_view::CalendarMode::Year, cx));
            }
            // `year-day=2026-10-24` opens that day's card in the Year view.
            if let Some(day) = state.split(';').find_map(|part| part.strip_prefix("year-day=")).and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()) {
                this.calendar.update(cx, |c, cx| c.dev_year_day(day, cx));
            }
            if state.contains("calendar=multi-week") {
                this.calendar.update(cx, |c, cx| c.set_mode(crate::calendar_view::CalendarMode::MultiWeek(5), cx));
            }
            if state.contains("calendar=agenda") {
                this.calendar.update(cx, |c, cx| c.set_mode(crate::calendar_view::CalendarMode::Agenda, cx));
            }
            if state.contains("calendar-menu=open") {
                this.calendar.update(cx, |c, cx| c.dev_open_menu(cx));
            }
            // `matrix=header-menu|add` opens the Matrix's "…" or quadrant I's new task field.
            if let Some(what) = state.split(';').find_map(|part| part.strip_prefix("matrix=")) {
                this.matrix.update(cx, |view, cx| view.dev_header(what, window, cx));
            }
            if state.contains("section=matrix") {
                this.select(Section::Matrix, window, cx);
            }
            // `matrix-menu=<1-4>` / `matrix-edit=<1-4>`: a quadrant's "…" menu or "Edit Matrix".
            for (prefix, edit) in [("matrix-menu=", false), ("matrix-edit=", true)] {
                if let Some(n) = state.split(';').find_map(|part| part.strip_prefix(prefix)).and_then(|n| n.parse::<usize>().ok()) {
                    this.matrix.update(cx, |view, cx| view.dev_open(n.clamp(1, 4) - 1, edit, cx));
                }
            }
            // `countdown=<name>><YYYY-MM-DD>><countdown|anniversary|holiday>[><emoji>|<#color>]`, repeatable.
            for spec in state.split(';').filter_map(|part| part.strip_prefix("countdown=")) {
                let parts: Vec<&str> = spec.split('>').collect();
                if let [name, date, kind, ..] = parts[..]
                    && let Ok(date) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                {
                    let kind = fern_core::CountdownKind::parse(kind);
                    // A fourth part, `<emoji>|<#color>`, sets its icon.
                    let look = parts.get(3).map(|look| look.split_once('|').unwrap_or((look, "")));
                    this.countdown.update(cx, |view, cx| view.dev_add(name, date, kind, look, cx));
                    this.calendar.update(cx, |view, cx| view.reload(cx));
                }
            }
            // `countdown-age=<name part>`: that birthday with "Show Age" on.
            if let Some(name) = state.split(';').find_map(|part| part.strip_prefix("countdown-age=")) {
                if let Some(countdown) = this.store.countdowns().unwrap_or_default().into_iter().find(|c| c.name.contains(name)) {
                    this.store.set_countdown_show_age(countdown.id, true).ok();
                    this.countdown.update(cx, |view, cx| view.reload(cx));
                }
            }
            // `countdown-bg=<name part>><picture path>`: that countdown's background picture.
            if let Some(spec) = state.split(';').find_map(|part| part.strip_prefix("countdown-bg=")) {
                let (name, path) = spec.split_once('>').unwrap_or((spec, ""));
                if let Some(countdown) = this.store.countdowns().unwrap_or_default().into_iter().find(|c| c.name.contains(name))
                    && let Ok(dir) = crate::attachments_dir()
                {
                    this.store.set_countdown_background(countdown.id, Some(std::path::Path::new(path)), &dir).ok();
                    this.countdown.update(cx, |view, cx| view.reload(cx));
                }
            }
            // `countdown-pin=<name part>`: that countdown pinned.
            if let Some(name) = state.split(';').find_map(|part| part.strip_prefix("countdown-pin=")) {
                if let Some(countdown) = this.store.countdowns().unwrap_or_default().into_iter().find(|c| c.name.contains(name)) {
                    this.store.set_countdown_pinned(countdown.id, true).ok();
                    this.countdown.update(cx, |view, cx| view.reload(cx));
                }
            }
            // `countdown-menu=<name part>` / `countdown-edit=<name part>`: a card's "…" menu or its
            // edit popover.
            for (prefix, edit) in [("countdown-menu=", false), ("countdown-edit=", true)] {
                if let Some(name) = state.split(';').find_map(|part| part.strip_prefix(prefix)) {
                    this.countdown.update(cx, |view, cx| view.dev_open(name, edit, window, cx));
                }
            }
            // `agenda-countdown=<name part>` opens that countdown's card in the Agenda.
            if let Some(needle) = state.split(';').find_map(|part| part.strip_prefix("calendar-context=")) {
                this.calendar.update(cx, |view, cx| view.dev_context(needle, cx));
            }
            if let Some(what) = state.split(';').find_map(|part| part.strip_prefix("calendar-more=")) {
                this.calendar.update(cx, |view, cx| view.dev_more(what, cx));
            }
            // `calendar-card=<title part>` opens that task's card in the calendar.
            if let Some(needle) = state.split(';').find_map(|part| part.strip_prefix("calendar-card=")) {
                this.calendar.update(cx, |view, cx| view.dev_card(needle, cx));
            }
            if let Some(name) = state.split(';').find_map(|part| part.strip_prefix("agenda-countdown=")) {
                this.calendar.update(cx, |view, cx| view.dev_open_countdown(name, cx));
            }
            // `habit=<name>><days ago,…>`, repeatable.
            for spec in state.split(';').filter_map(|part| part.strip_prefix("habit=")) {
                let (name, days) = spec.split_once('>').unwrap_or((spec, ""));
                let days: Vec<i64> = days.split(',').filter_map(|d| d.parse().ok()).collect();
                this.habits.update(cx, |view, cx| view.dev_add(name, &days, cx));
            }
            // `habit-select=<name>` opens a habit's details panel; `habit-do=<command>` (see
            // `HabitView::dev_apply`) acts on the selected habit. They run in order.
            for part in state.split(';') {
                if let Some(name) = part.strip_prefix("habit-select=") {
                    this.habits.update(cx, |view, cx| view.dev_select(name, cx));
                } else if let Some(command) = part.strip_prefix("habit-do=") {
                    this.habits.update(cx, |view, cx| view.dev_apply(command, window, cx));
                }
            }
            // `focus=<title>><minutes ago>><minutes>`, repeatable.
            for spec in state.split(';').filter_map(|part| part.strip_prefix("focus=")) {
                let parts: Vec<&str> = spec.split('>').collect();
                // A fourth part `sw` records it as a stopwatch session.
                if let [title, ago, minutes, ..] = parts[..]
                    && let (Ok(ago), Ok(minutes)) = (ago.parse(), minutes.parse())
                {
                    let stopwatch = parts.get(3) == Some(&"sw");
                    this.focus.update(cx, |view, cx| view.dev_record(title, ago, minutes, stopwatch, cx));
                }
            }
            // The calendar shows the habits and focus records added above.
            this.calendar.update(cx, |view, cx| view.reload(cx));
            // `focus-timer=<name>[><emoji>[>#color]]`, repeatable: a timer in the Pomodoro list;
            // `focus-timer-add` opens "Add Timer", `focus-timer-add=icons` with its icon picker.
            for spec in state.split(';').filter_map(|part| part.strip_prefix("focus-timer=")) {
                let mut parts = spec.split('>');
                let mut timer = fern_core::focus::FocusTimer::new(parts.next().unwrap_or_default());
                timer.icon = parts.next().map(str::to_owned);
                timer.color = parts.next().map(str::to_owned);
                this.store.save_focus_timer(&timer).ok();
                this.focus.update(cx, |view, cx| view.reload(cx));
            }
            if state.contains("focus-timer-add") {
                let icons = state.contains("focus-timer-add=icons");
                this.focus.update(cx, |view, cx| view.dev_add_timer(icons, window, cx));
            }
            if let Some(menu) = state.split(';').find_map(|part| part.strip_prefix("focus-menu=")) {
                this.focus.update(cx, |view, cx| view.dev_menu(menu, window, cx));
            }
            // `focus-task=<title part>`: what "Start Focus" in a task's menu does.
            if let Some(needle) = state.split(';').find_map(|part| part.strip_prefix("focus-task="))
                && let Some(task) = this.store.tasks().unwrap_or_default().into_iter().find(|t| t.title.contains(needle))
            {
                this.select(Section::Pomodoro, window, cx);
                this.focus.update(cx, |focus, cx| focus.start(Some((task.id, task.title)), cx));
            }
            if state.contains("section=focus") {
                this.select(Section::Pomodoro, window, cx);
            }
            if state.contains("section=habits") {
                this.select(Section::Habits, window, cx);
            }
            if state.contains("section=countdown") {
                this.select(Section::Countdown, window, cx);
            }
            // `countdown-header=title|more`: the header's "Countdown ▾" or "…" menu.
            if let Some(menu) = state.split(';').find_map(|part| part.strip_prefix("countdown-header=")) {
                this.countdown.update(cx, |view, cx| view.dev_header_menu(menu == "more", cx));
            }
            if state.contains("countdown-add-menu") {
                this.countdown.update(cx, |view, cx| view.dev_add_menu(cx));
            }
            if state.contains("countdown-draft") {
                this.countdown.update(cx, |view, cx| view.start_new(window, cx));
            }
            // `countdown-add=<kind>`: a new countdown of that kind, as the "+" menu starts it.
            if let Some(kind) = state.split(';').find_map(|part| part.strip_prefix("countdown-add=")) {
                let kind = fern_core::CountdownKind::parse(kind);
                this.countdown.update(cx, |view, cx| view.start_new_kind(kind, window, cx));
            }
            // `countdown-draft=reminder|calc|counting|show|month`: the dialog with that dropdown open.
            if let Some(row) = state.split(';').find_map(|part| part.strip_prefix("countdown-draft=")) {
                this.countdown.update(cx, |view, cx| view.dev_open_row(row, cx));
                // `countdown-draft=custom`: the Reminder's Custom form.
                if row == "custom" {
                    this.countdown.update(cx, |view, cx| view.open_custom_reminder(window, cx));
                }
            }
            // `attach=<task title part>><file path>`: a file attached to that task.
            for spec in state.split(';').filter_map(|part| part.strip_prefix("attach=")) {
                if let Some((needle, file)) = spec.split_once('>')
                    && let Some(task) = this.store.tasks().unwrap_or_default().into_iter().find(|t| t.title.contains(needle))
                    && let Ok(dir) = crate::attachments_dir()
                {
                    this.store.add_attachment(task.id, std::path::Path::new(file), &dir).ok();
                    this.todo.update(cx, |todo, cx| todo.reload(cx));
                }
            }
            // `notice=<task title or habit name part>><hours ago>`: a fired reminder for the bell; `notices=open`.
            for spec in state.split(';').filter_map(|part| part.strip_prefix("notice=")) {
                let (needle, ago) = spec.split_once('>').unwrap_or((spec, "0"));
                let at = chrono::Local::now().naive_local() - chrono::Duration::hours(ago.parse().unwrap_or(0));
                if let Some(task) = this.store.tasks().unwrap_or_default().into_iter().find(|t| t.title.contains(needle)) {
                    this.add_notices(vec![Notice { target: Target::Task(task.id), title: task.title, at }], cx);
                } else if let Some(habit) = this.store.habits().unwrap_or_default().into_iter().find(|h| h.name.contains(needle)) {
                    this.add_notices(vec![Notice { target: Target::Habit(habit.id), title: habit.name, at }], cx);
                }
            }
            // `theme=light|dark|system`, saved like a pick in the Settings.
            if let Some(mode) = state.split(';').find_map(|part| part.strip_prefix("theme=")).and_then(fern_core::ThemeMode::from_code) {
                let preferences = fern_core::Preferences { theme: mode, ..this.store.preferences().unwrap_or_default() };
                this.store.save_preferences(&preferences).ok();
                settings_window::apply(&preferences, window, cx);
            }
            // `visibility=<smart list>><show|if_not_empty|hide>`, repeatable: Settings → Smart List.
            for spec in state.split(';').filter_map(|part| part.strip_prefix("visibility=")) {
                let (list, visibility) = spec.split_once('>').unwrap_or((spec, ""));
                if let (Some(list), Some(visibility)) = (
                    fern_core::SmartList::ALL.into_iter().find(|l| l.code() == list),
                    fern_core::Visibility::from_code(visibility),
                ) {
                    let mut preferences = this.store.preferences().unwrap_or_default();
                    preferences.set_visibility(list, visibility);
                    this.store.save_preferences(&preferences).ok();
                    this.todo.update(cx, |todo, cx| todo.reload(cx));
                }
            }
            // `hour12` and `week-numbers`: Settings → Time Format set to 12 hours, and week numbers on.
            let flag = |name: &str| state.split(';').any(|part| part == name);
            if flag("hour12") || flag("week-numbers") || flag("overdue-last") || flag("no-mini-calendar") {
                let defaults = this.store.preferences().unwrap_or_default();
                let mut preferences = fern_core::Preferences {
                    hour12: flag("hour12") || defaults.hour12,
                    week_numbers: flag("week-numbers") || defaults.week_numbers,
                    ..defaults
                };
                preferences.task_defaults.overdue_last |= flag("overdue-last");
                preferences.mini_calendar &= !flag("no-mini-calendar");
                this.store.save_preferences(&preferences).ok();
                settings_window::apply(&preferences, window, cx);
                this.todo.update(cx, |todo, cx| todo.reload(cx));
            }
            // `features-off=matrix,countdown`: those sections turned off in Settings → Features.
            if let Some(off) = state.split(';').find_map(|part| part.strip_prefix("features-off=")) {
                let mut preferences = this.store.preferences().unwrap_or_default();
                for name in off.split(',') {
                    match name {
                        "calendar" => preferences.features.calendar = false,
                        "matrix" => preferences.features.matrix = false,
                        "habits" => preferences.features.habits = false,
                        "focus" => preferences.features.focus = false,
                        "countdown" => preferences.features.countdown = false,
                        _ => {}
                    }
                }
                this.store.save_preferences(&preferences).ok();
                this.features = preferences.features;
            }
            // `sync-config=<server>|<token>`: Settings → Sync filled in.
            if let Some(spec) = state.split(';').find_map(|part| part.strip_prefix("sync-config=")) {
                let (server, token) = spec.split_once('|').unwrap_or((spec, ""));
                let config = fern_core::sync::SyncConfig { server: server.to_owned(), token: token.to_owned() };
                this.store.set_sync_config(&config).ok();
            }
            // `settings=<page>[><menu>]` opens the Settings window (see `SettingsWindow::dev_open`).
            if let Some(spec) = state.split(';').find_map(|part| part.strip_prefix("settings=")) {
                this.open_settings(window, cx);
                if let Some(settings) = &this.settings {
                    settings.update(cx, |settings, cx| settings.dev_open(spec, cx));
                }
            }
            if state.contains("statistics=open") {
                this.open_statistics(cx);
            }
            // `statistics=task` / `statistics=focus`: the page on its Task (by day) or Focus tab.
            for (key, menu) in [("statistics=task", "stats-task-day"), ("statistics=focus", "stats-focus")] {
                if state.contains(key) {
                    this.open_statistics(cx);
                    this.focus.update(cx, |view, cx| view.dev_menu(menu, window, cx));
                }
            }
            if state.contains("account-menu=open") {
                this.account_menu_open = true;
            }
            if state.contains("shortcuts=open") {
                this.shortcuts_open = true;
            }
            if state.contains("notices=open") {
                this.set_notices_open(true, cx);
            }
            if let Some(query) = state.split(';').find_map(|part| part.strip_prefix("search-popover=")) {
                this.open_search(window, cx);
                if let Some(search) = &this.search {
                    search.update(cx, |search, cx| search.set_query(query, cx));
                }
                // `search-filter=task[>lists|tags|date|priority]` picks the Task chip and a dropdown.
                if let (Some(search), Some(spec)) = (&this.search, state.split(';').find_map(|part| part.strip_prefix("search-filter="))) {
                    search.update(cx, |search, cx| search.dev_filter(spec, cx));
                }
            }
        }
        this
    }

    fn select(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        // Search is a popover over the current screen, as in the original.
        if section == Section::Search {
            self.open_search(window, cx);
            return;
        }
        self.todo.update(cx, |todo, cx| todo.stop_search(cx));
        // Other sections change tasks and check habits in.
        if section == Section::Tasks && self.section != Section::Tasks {
            self.todo.update(cx, |todo, cx| todo.reload(cx));
        }
        if section == Section::Calendar {
            self.calendar.update(cx, |calendar, cx| calendar.reload(cx));
            window.focus(&self.calendar.focus_handle(cx), cx);
        }
        if section == Section::Matrix {
            self.matrix.update(cx, |matrix, cx| matrix.reload(cx));
        }
        if section == Section::Pomodoro {
            self.focus.update(cx, |focus, cx| focus.reload(cx));
        }
        if section == Section::Habits {
            self.habits.update(cx, |habits, cx| habits.reload(cx));
        }
        if section == Section::Countdown {
            self.countdown.update(cx, |countdown, cx| countdown.reload(cx));
        }
        self.section = section;
        cx.notify();
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let search = cx.new(|cx| SearchModal::new(self.store.clone(), cx));
        cx.subscribe_in(&search, window, |this, _, event, window, cx| {
            this.search = None;
            match event {
                SearchEvent::OpenTask(id) => {
                    let id = *id;
                    this.select(Section::Tasks, window, cx);
                    this.todo.update(cx, |todo, cx| {
                        todo.set_view(fern_core::View::All, cx);
                        todo.select(id, cx);
                    });
                }
                SearchEvent::OpenView(fern_core::View::Search(query)) => {
                    let query = query.clone();
                    this.select(Section::Tasks, window, cx);
                    this.todo.update(cx, |todo, cx| todo.show_search_results(query, cx));
                }
                SearchEvent::OpenView(view) => {
                    let view = view.clone();
                    this.select(Section::Tasks, window, cx);
                    this.todo.update(cx, |todo, cx| todo.set_view(view, cx));
                }
                SearchEvent::Dismissed => {}
            }
            cx.notify();
        })
        .detach();
        window.focus(&search.focus_handle(cx), cx);
        self.search = Some(search);
        cx.notify();
    }

    fn new_task(&mut self, _: &NewTask, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Section::Tasks, window, cx);
        self.todo.update(cx, |todo, cx| todo.focus_new_task(window, cx));
    }

    fn search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Section::Search, window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.todo.update(cx, |todo, cx| todo.toggle_lists(cx));
    }

    fn show_view(&mut self, view: fern_core::View, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Section::Tasks, window, cx);
        self.todo.update(cx, |todo, cx| todo.set_view(view, cx));
    }

    fn set_account_menu(&mut self, open: bool, cx: &mut Context<Self>) {
        self.account_menu_open = open;
        cx.notify();
    }

    fn render_account_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        popover(cx)
            .w(px(180.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_account_menu(false, cx)))
            .child(
                ListItem::new("account-settings")
                    .start_slot(Icon::new(IconName::Settings))
                    .child(Label::new(strings.settings))
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            )
            .child(
                ListItem::new("account-statistics")
                    .start_slot(Icon::new(IconName::ChartNoAxesColumn))
                    .child(Label::new(strings.statistics))
                    .on_click(cx.listener(|this, _, _, cx| this.open_statistics(cx))),
            )
            .into_any_element()
    }

    fn open_statistics(&mut self, cx: &mut Context<Self>) {
        self.account_menu_open = false;
        self.statistics_open = true;
        self.focus.update(cx, |focus, cx| focus.open_stats_overview(cx));
        cx.notify();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.account_menu_open = false;
        let settings = cx.new(|cx| SettingsWindow::new(self.store.clone(), cx));
        cx.subscribe_in(&settings, window, |this, _, event, window, cx| {
            this.settings = None;
            if let SettingsEvent::ShowShortcuts = event {
                this.shortcuts_open = true;
            }
            // The default list and the week start show up in the views.
            this.todo.update(cx, |todo, cx| todo.reload(cx));
            // A section turned off in Features closes.
            this.features = this.store.preferences().map(|p| p.features).unwrap_or_default();
            if !this.section.enabled(this.features) {
                this.select(Section::Tasks, window, cx);
            }
            window.focus(&this.todo.focus_handle(cx), cx);
            cx.notify();
        })
        .detach();
        window.focus(&settings.focus_handle(cx), cx);
        self.settings = Some(settings);
        cx.notify();
    }
}

impl AppView {
    /// Opens a task from a `fern://task/…` link, in All with its details.
    pub fn open_task(&mut self, task: fern_core::Id, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Section::Tasks, window, cx);
        self.todo.update(cx, |todo, cx| {
            todo.set_view(fern_core::View::All, cx);
            todo.select(task, cx);
        });
    }

    fn toggle_shortcuts(&mut self, cx: &mut Context<Self>) {
        self.shortcuts_open = !self.shortcuts_open;
        cx.notify();
    }

    /// The original's Shortcuts panel: the keymap's bindings by group, and quick add syntax.
    fn render_shortcuts(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let list = std::str::from_utf8(
            &gpui::AssetSource::load(&assets::Assets, crate::keymap::DEFAULT_KEYMAP_PATH).ok().flatten().unwrap_or_default(),
        )
        .ok()
        .and_then(|source| crate::keymap::shortcut_list(source).ok())
        .unwrap_or_default();
        let chip = |text: String| {
            div()
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .border_1()
                .border_color(theme.colors().border)
                .child(Label::new(text).size(LabelSize::XSmall).color(Color::Muted))
        };
        let row = |label: &'static str, keys: Vec<String>| {
            h_flex()
                .h(px(32.))
                .child(div().flex_1().child(Label::new(label).size(LabelSize::Small)))
                .child(h_flex().gap_1().children(keys.into_iter().map(chip)))
        };
        let heading = |text: &'static str| div().pt_3().pb_1().child(Label::new(text).weight(gpui::FontWeight::SEMIBOLD));
        let mut body = v_flex().child(heading(strings.shortcut_groups[0]));
        for (i, (action, keys)) in list.into_iter().enumerate() {
            if action.starts_with("task_list::") && i > 0 && list_group_start(i) {
                body = body.child(heading(strings.shortcut_groups[1]));
            }
            if action == "task_list::PinTask" {
                body = body.child(heading(strings.shortcut_groups[3]));
            }
            if !keys.is_empty() {
                body = body.child(row(strings.shortcut_labels[i], keys));
            }
        }
        body = body.child(heading(strings.shortcut_groups[2]));
        for (what, example) in strings.quick_add_help {
            body = body.child(row(what, vec![example.to_owned()]));
        }
        v_flex()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(380.))
            .bg(theme.colors().elevated_surface_background)
            .border_l_1()
            .border_color(theme.colors().border)
            .shadow_lg()
            .child(
                h_flex()
                    .h(px(48.))
                    .px_4()
                    .child(div().flex_1().child(Label::new(strings.shortcuts).size(LabelSize::Large)))
                    .child(IconButton::new("close-shortcuts", IconName::X).on_click(cx.listener(|this, _, _, cx| this.toggle_shortcuts(cx)))),
            )
            .child(v_flex().id("shortcut-list").flex_1().min_h_0().overflow_y_scroll().px_4().pb_4().child(body))
            .into_any_element()
    }

    /// Syncs on start, whenever there are local changes to send, and every few minutes to fetch
    /// other devices' changes. Screenshots of the debug build do not sync.
    fn start_sync(&mut self, cx: &mut Context<Self>) {
        if std::env::var_os("FERN_DEV_STATE").is_some() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let mut since_pull = std::time::Duration::ZERO;
            let mut first = true;
            loop {
                let Ok(due) = this.update(cx, |this, _| {
                    first || since_pull >= crate::sync::PULL_EVERY || this.store.has_unsent_changes().unwrap_or(false)
                }) else {
                    break;
                };
                if due {
                    since_pull = std::time::Duration::ZERO;
                    if this.update(cx, |this, cx| this.sync_now(cx)).is_err() {
                        break;
                    }
                }
                first = false;
                cx.background_executor().timer(crate::sync::PUSH_CHECK_EVERY).await;
                since_pull += crate::sync::PUSH_CHECK_EVERY;
            }
        })
        .detach();
    }

    /// One sync with the server from Settings → Sync: sends the local changes, applies the others
    /// and refreshes the views.
    pub fn sync_now(&mut self, cx: &mut Context<Self>) {
        if self.sync == crate::sync::SyncStatus::Running {
            return;
        }
        let config = match self.store.sync_config() {
            Ok(Some(config)) => config,
            Ok(None) => {
                self.sync = crate::sync::SyncStatus::Off;
                cx.notify();
                return;
            }
            Err(err) => {
                self.sync = crate::sync::SyncStatus::Failed(format!("{err:#}"));
                cx.notify();
                return;
            }
        };
        let request = match self.store.sync_request() {
            Ok(request) => request,
            Err(err) => {
                self.sync = crate::sync::SyncStatus::Failed(format!("{err:#}"));
                cx.notify();
                return;
            }
        };
        self.sync = crate::sync::SyncStatus::Running;
        cx.notify();
        let body = request.body.clone();
        let sent_config = config.clone();
        let answer = cx.background_spawn(async move { crate::sync::post(&sent_config, &body) });
        cx.spawn(async move |this, cx| {
            let answer = answer.await;
            this.update(cx, |this, cx| {
                let result = answer.and_then(|answer| this.store.finish_sync(&request, &answer));
                this.sync = match result {
                    Ok(received) => {
                        if received > 0 {
                            this.reload_all(cx);
                        }
                        this.sync_files(config, cx);
                        crate::sync::SyncStatus::Done(chrono::Local::now().naive_local())
                    }
                    Err(err) => crate::sync::SyncStatus::Failed(format!("{err:#}")),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// After a sync, sends the attached files the server lacks and fetches the missing ones.
    fn sync_files(&mut self, config: fern_core::sync::SyncConfig, cx: &mut Context<Self>) {
        let Ok(dir) = crate::attachments_dir() else { return };
        let (upload, download) = match self.store.files_to_sync(&dir) {
            Ok(files) => files,
            Err(err) => {
                eprintln!("failed to list the files to sync: {err:#}");
                return;
            }
        };
        if upload.is_empty() && download.is_empty() {
            return;
        }
        let fetched = !download.is_empty();
        let done = cx.background_spawn(async move { crate::sync::transfer_files(&config, &dir, &upload, &download) });
        cx.spawn(async move |this, cx| {
            let done = done.await;
            this.update(cx, |this, cx| {
                if let Err(err) = this.store.mark_files_synced(&done) {
                    eprintln!("failed to note the synced files: {err:#}");
                }
                if fetched {
                    this.reload_all(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Re-reads every view, after changes came from another device.
    fn reload_all(&mut self, cx: &mut Context<Self>) {
        self.todo.update(cx, |view, cx| view.reload(cx));
        self.calendar.update(cx, |view, cx| view.reload(cx));
        self.matrix.update(cx, |view, cx| view.reload(cx));
        self.countdown.update(cx, |view, cx| view.reload(cx));
        self.habits.update(cx, |view, cx| view.reload(cx));
        self.focus.update(cx, |view, cx| view.reload(cx));
    }

    /// The sync button's tooltip: "Sync" and how the last sync went.
    fn sync_tooltip(&self, strings: &'static i18n::Strings) -> String {
        let status = match &self.sync {
            crate::sync::SyncStatus::Off if self.store.sync_config().ok().flatten().is_some() => strings.sync_never.to_owned(),
            crate::sync::SyncStatus::Off => strings.sync_off.to_owned(),
            crate::sync::SyncStatus::Running => strings.sync_running.to_owned(),
            crate::sync::SyncStatus::Done(at) => strings.sync_done.replace("{time}", &crate::calendar::format_time(at.time())),
            crate::sync::SyncStatus::Failed(error) => strings.sync_failed.replace("{error}", error),
        };
        format!("{}\n{status}", strings.sync)
    }

    pub fn add_notices(&mut self, notices: Vec<Notice>, cx: &mut Context<Self>) {
        if !notices.is_empty() {
            self.notices.push(notices);
            cx.notify();
        }
    }

    /// Opening the list marks the notifications read, which clears the badge.
    fn set_notices_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.notices_open = open;
        if open {
            self.notices.mark_read();
        }
        cx.notify();
    }

    fn open_notice(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.set_notices_open(false, cx);
        match target {
            Target::Task(task) => {
                self.select(Section::Tasks, window, cx);
                self.todo.update(cx, |todo, cx| {
                    todo.set_view(fern_core::View::All, cx);
                    todo.select(task, cx);
                });
            }
            Target::Habit(habit) => {
                self.select(Section::Habits, window, cx);
                self.habits.update(cx, |habits, cx| habits.select(habit, cx));
            }
            Target::Countdown(_) => self.select(Section::Countdown, window, cx),
        }
    }

    /// A click on a reminder, in the bell or on the desktop notification. Answering a habit's
    /// reminder takes it off the bell.
    pub fn answer_notice(&mut self, notice: &Notice, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        let Reply::Action(action) = reply else {
            window.activate_window();
            self.open_notice(notice.target, window, cx);
            return;
        };
        let Target::Habit(habit) = notice.target else { return };
        let now = chrono::Local::now().naive_local();
        let done = match action {
            NoticeAction::Snooze => self.store.snooze_habit(habit, now + chrono::Duration::minutes(reminders::SNOOZE_MINUTES)),
            NoticeAction::CheckIn => {
                let today = now.date();
                if self.store.checkins(habit).unwrap_or_default().contains(&today) {
                    Ok(())
                } else {
                    self.store.toggle_checkin(habit, today).map(|_| ())
                }
            }
            NoticeAction::PlusOne => self.store.add_checkin_amount(habit, now.date(), 1).map(|_| ()),
            NoticeAction::Record => {
                window.activate_window();
                self.set_notices_open(false, cx);
                self.select(Section::Habits, window, cx);
                self.habits.update(cx, |habits, cx| habits.record(habit, window, cx));
                Ok(())
            }
        };
        if let Err(err) = done {
            eprintln!("failed to answer the reminder: {err:#}");
        }
        self.notices.items.retain(|n| n != notice);
        self.habits.update(cx, |habits, cx| habits.reload(cx));
        cx.notify();
    }

    fn render_notices(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let today = chrono::Local::now().date_naive();
        let theme = cx.theme().clone();
        popover(cx)
            .w(px(300.))
            .max_h(px(420.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_notices_open(false, cx)))
            .child(div().px_2().py_1().child(Label::new(strings.notifications).weight(gpui::FontWeight::SEMIBOLD)))
            .when(self.notices.items.is_empty(), |el| {
                el.child(div().px_2().py_3().child(Label::new(strings.no_notifications).color(Color::Muted)))
            })
            .child(
                v_flex().id("notices").overflow_y_scroll().children(self.notices.items.iter().enumerate().map(|(i, notice)| {
                    let target = notice.target;
                    let (when, _) = crate::task_rows::due_label(notice.at.date(), Some(notice.at.time()), today, strings);
                    let actions = reminders::actions(&self.store, target);
                    let buttons = actions.iter().map(|&action| {
                        let notice = notice.clone();
                        div()
                            .id(SharedString::from(format!("notice-{i}-{}", action.key())))
                            .px_2()
                            .py_0p5()
                            .rounded(px(4.))
                            .border_1().border_color(theme.colors().border)
                            .hover(|s| s.bg(theme.colors().element_hover))
                            .cursor_pointer()
                            .child(Label::new(action.label(strings)).size(LabelSize::Small))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.answer_notice(&notice, Reply::Action(action), window, cx);
                            }))
                    });
                    ListItem::new(SharedString::from(format!("notice-{i}")))
                        .height(px(if actions.is_empty() { 44. } else { 72. }))
                        .start_slot(Icon::new(IconName::AlarmClock).color(Color::Accent))
                        .child(
                            v_flex()
                                .min_w_0()
                                .py_1()
                                .child(Label::new(notice.title.clone()).truncate())
                                .child(Label::new(when).size(LabelSize::XSmall).color(Color::Muted))
                                .child(h_flex().gap_1().children(buttons)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| this.open_notice(target, window, cx)))
                })),
            )
            .into_any_element()
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let this = cx.entity().downgrade();
        let on_select = move |section, window: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.select(section, window, cx)).ok();
        };
        let hover_target = cx.entity().downgrade();
        let on_nav_hover = move |section: Section, hovered: bool, cx: &mut App| {
            hover_target
                .update(cx, |this, cx| {
                    let now = if hovered { Some(section) } else { this.nav_hovered.filter(|s| *s != section) };
                    if this.nav_hovered != now {
                        this.nav_hovered = now;
                        cx.notify();
                    }
                })
                .ok();
        };
        let on_avatar = cx.listener(|this, _, _, cx| this.set_account_menu(true, cx));
        let account_menu = self.account_menu_open.then(|| {
            PopoverAttachment::new(Placement::RightTop, self.render_account_menu(cx)).into_any_element()
        });
        let features = self.features;
        let sync_button = self.store.sync_config().ok().flatten().map(|_| sidebar::SyncButton {
            tooltip: self.sync_tooltip(i18n::t(cx)),
            failed: matches!(self.sync, crate::sync::SyncStatus::Failed(_)),
            on_click: Box::new(cx.listener(|this, _, _, cx| this.sync_now(cx))),
        });
        let bell = sidebar::Bell {
            unread: self.notices.unread,
            on_click: Box::new(cx.listener(|this, _, _, cx| {
                let open = !this.notices_open;
                this.set_notices_open(open, cx)
            })),
            menu: self
                .notices_open
                .then(|| PopoverAttachment::new(Placement::RightBottom, self.render_notices(cx)).into_any_element()),
        };

        let content = h_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .key_context("Workspace")
            .on_action(cx.listener(Self::new_task))
            .on_action(cx.listener(Self::search))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(|this, _: &ShowShortcuts, _, cx| this.toggle_shortcuts(cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
            .on_action(cx.listener(|this, _: &ShowInbox, window, cx| this.show_view(fern_core::View::Inbox, window, cx)))
            .on_action(cx.listener(|this, _: &ShowToday, window, cx| this.show_view(fern_core::View::Today, window, cx)))
            .on_action(cx.listener(|this, _: &ShowTomorrow, window, cx| {
                this.show_view(fern_core::View::Tomorrow, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowNext7Days, window, cx| {
                this.show_view(fern_core::View::Next7Days, window, cx)
            }))
            .items_start()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .when(self.statistics_open, |el| el.child(div().size_full().child(self.focus.clone())))
            .when(!self.statistics_open, |el| {
                el.child(sidebar::render(self.section, self.nav_hovered, on_nav_hover, on_select, on_avatar, account_menu, bell, sync_button, features, cx)).child(match self.section {
                Section::Tasks | Section::Search => div().flex_1().h_full().child(self.todo.clone()),
                Section::Calendar => div().flex_1().h_full().child(self.calendar.clone()),
                Section::Matrix => div().flex_1().h_full().child(self.matrix.clone()),
                Section::Countdown => div().flex_1().h_full().child(self.countdown.clone()),
                Section::Habits => div().flex_1().h_full().child(self.habits.clone()),
                    Section::Pomodoro => div().flex_1().h_full().child(self.focus.clone()),
                })
            })
            .when(self.shortcuts_open, |el| el.child(self.render_shortcuts(cx)))
            .when_some(self.settings.clone(), |el, settings| {
                el.child(
                    div()
                        .id("settings-backdrop")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(gpui::black().opacity(0.35))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(settings),
                )
            })
            .when_some(self.search.clone(), |el, search| {
                el.child(div().absolute().top(px(160.)).left_0().right_0().flex().justify_center().child(search))
            });
        let root = v_flex()
            .size_full()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(self.title_bar.clone())
            .child(content);
        title_bar::window_frame(root, window, cx)
    }
}

/// Where the task list's shortcuts start in `keymap::LISTED_ACTIONS`.
fn list_group_start(index: usize) -> bool {
    crate::keymap::LISTED_ACTIONS.iter().position(|a| a.starts_with("task_list::")) == Some(index)
}
