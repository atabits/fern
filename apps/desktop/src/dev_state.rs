//! Debug-build `FERN_DEV_STATE` environment-variable parser used by screenshot & QA harnesses
//! to boot the app directly into a specific view, modal, or seeded state.

use ui::prelude::*;

use crate::app_view::AppView;
use crate::reminders::{Notice, Target};
use crate::settings_window;
use crate::sidebar::Section;

pub(crate) fn apply(this: &mut AppView, window: &mut Window, cx: &mut Context<AppView>) {
    let Some(state) = desktop_common::dev_state_value() else {
        return;
    };
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
    if let Some(day) = state
        .split(';')
        .find_map(|part| part.strip_prefix("year-day="))
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    {
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
        if let Some(n) = state
            .split(';')
            .find_map(|part| part.strip_prefix(prefix))
            .and_then(|n| n.parse::<usize>().ok())
        {
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
    if let Some(name) = state.split(';').find_map(|part| part.strip_prefix("countdown-age="))
        && let Some(countdown) = this.store.countdowns().unwrap_or_default().into_iter().find(|c| c.name.contains(name))
    {
        this.store.set_countdown_show_age(countdown.id, true).ok();
        this.countdown.update(cx, |view, cx| view.reload(cx));
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
    if let Some(name) = state.split(';').find_map(|part| part.strip_prefix("countdown-pin="))
        && let Some(countdown) = this.store.countdowns().unwrap_or_default().into_iter().find(|c| c.name.contains(name))
    {
        this.store.set_countdown_pinned(countdown.id, true).ok();
        this.countdown.update(cx, |view, cx| view.reload(cx));
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
    if state.contains("section=life-clock") {
        this.select(Section::LifeClock, window, cx);
    }
    if let Some(raw) = state.split(';').find_map(|part| part.strip_prefix("life-clock-birth=")) {
        let date = if raw.is_empty() || raw == "none" {
            None
        } else {
            chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()
        };
        if raw.is_empty() || raw == "none" || date.is_some() {
            this.life_clock.update(cx, |view, cx| view.dev_set_birth_date(date, cx));
        }
    }
    if let Some(palette) = state.split(';').find_map(|part| part.strip_prefix("life-clock-palette=")) {
        let mode = match palette {
            "classic" => Some(crate::life_clock_view::LifeClockPaletteMode::Classic),
            "theme" => Some(crate::life_clock_view::LifeClockPaletteMode::FernTheme),
            _ => None,
        };
        if let Some(mode) = mode {
            this.life_clock.update(cx, |view, cx| view.dev_set_palette(mode, cx));
            this.sync_title_bar_appearance(cx);
        }
    }
    if state.split(';').any(|part| matches!(part, "life-clock-popover" | "life-clock-popover=open" | "life-clock-popover=true")) {
        this.life_clock.update(cx, |view, cx| view.dev_open_popover(true, cx));
    }
    if state.split(';').any(|part| matches!(part, "life-clock-hide-rail" | "life-clock-hide-rail=true")) {
        this.set_life_clock_rail_hidden(true, cx);
    }
    if state.split(';').any(|part| part == "horizons-seed") {
        seed_horizons(&this.store);
        this.todo.update(cx, |view, cx| view.reload(cx));
        this.things.update(cx, |view, cx| view.reload(cx));
        this.calendar.update(cx, |view, cx| view.reload(cx));
        this.matrix.update(cx, |view, cx| view.reload(cx));
    }
    if state.split(';').any(|part| part == "view=horizons")
        && this.select(Section::Tasks, window, cx)
    {
        this.todo.update(cx, |todo, cx| todo.select_view(fern_core::View::Horizons, window, cx));
    }
    if let Some(spec) = state.split(';').find_map(|part| part.strip_prefix("horizons-mode=")) {
        this.todo.update(cx, |todo, cx| todo.dev_horizons_mode(spec, cx));
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
            this.add_notices(
                vec![Notice {
                    target: Target::Task(task.id),
                    title: task.title,
                    at,
                    constant: task.constant_reminder,
                    has_goal: false,
                }],
                cx,
            );
        } else if let Some(habit) = this.store.habits().unwrap_or_default().into_iter().find(|h| h.name.contains(needle)) {
            this.add_notices(
                vec![Notice {
                    target: Target::Habit(habit.id),
                    title: habit.name,
                    at,
                    constant: habit.constant_reminder,
                    has_goal: habit.goal.is_some(),
                }],
                cx,
            );
        }
    }
    // `theme=light|dark|system`, saved like a pick in the Settings.
    if let Some(mode) = state
        .split(';')
        .find_map(|part| part.strip_prefix("theme="))
        .and_then(fern_core::ThemeMode::from_code)
    {
        let mut preferences = this.store.preferences().unwrap_or_default();
        preferences.theme.mode = mode;
        this.store.save_theme_selection(&preferences.theme).ok();
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
                "life-clock" | "life_clock" => preferences.features.life_clock = false,
                "horizons" => preferences.features.horizons = false,
                _ => {}
            }
        }
        this.store.save_preferences(&preferences).ok();
        this.features = preferences.features;
        desktop_common::ActivePreferences::sync_from_preferences(
            &preferences,
            desktop_common::sound::enabled(&this.store),
            cx,
        );
        this.todo.update(cx, |view, cx| view.reload(cx));
        this.things.update(cx, |view, cx| view.reload(cx));
        this.calendar.update(cx, |view, cx| view.reload(cx));
        this.matrix.update(cx, |view, cx| view.reload(cx));
    }
    // `sync-config=<server>|<token>`: Settings → Sync filled in.
    if let Some(spec) = state.split(';').find_map(|part| part.strip_prefix("sync-config=")) {
        let (server, token) = spec.split_once('|').unwrap_or((spec, ""));
        let config = fern_core::sync::SyncConfig {
            server: server.to_owned(),
            token: token.to_owned(),
            ..Default::default()
        };
        this.store.set_sync_config(&config).ok();
        this.has_sync_config = !config.server.trim().is_empty() && !config.token.trim().is_empty();
    }
    // `settings=<page>[><menu>]` opens the Settings window (see `SettingsWindow::dev_open`).
    if let Some(spec) = state.split(';').find_map(|part| part.strip_prefix("settings=")) {
        let spec = spec.to_owned();
        cx.defer_in(window, move |this, window, cx| {
            this.open_settings(None, window, cx);
            if let Some(settings) = this.settings {
                let _ = settings.update(cx, |settings, _, cx| settings.dev_open(&spec, cx));
            }
        });
    }
    if state.contains("statistics=open") {
        this.open_statistics(window, cx);
    }
    // `statistics=task` / `statistics=focus`: the page on its Task (by day) or Focus tab.
    for (key, menu) in [("statistics=task", "stats-task-day"), ("statistics=focus", "stats-focus")] {
        if state.contains(key) {
            this.open_statistics(window, cx);
            this.focus.update(cx, |view, cx| view.dev_menu(menu, window, cx));
        }
    }
    if state.contains("account-menu=open") {
        this.account_menu_open = true;
    }
    if state.contains("shortcuts=open") {
        cx.defer_in(window, |this, window, cx| {
            this.open_settings_page(settings_window::Page::Shortcuts, window, cx);
        });
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
        if let (Some(search), Some(spec)) = (
            &this.search,
            state.split(';').find_map(|part| part.strip_prefix("search-filter=")),
        ) {
            search.update(cx, |search, cx| search.dev_filter(spec, cx));
        }
    }
}

/// Seeds a rich TimeStripe-style sample dataset for visual QA across Today (including a timed
/// 11:00–11:30 meeting), current ISO Week, current Month, past Month (rollover demo), Quarter,
/// Year, Decade, Life, Inbox, and the sample `"Comagic"` board.
fn seed_horizons(store: &fern_core::Store) {
    use chrono::NaiveTime;
    use fern_core::{Horizon, ListDraft, Priority, TaskDraft};

    let today = chrono::Local::now().date_naive();
    let current_week = Horizon::current_week(today);
    let current_month = Horizon::current_month(today);
    let past_month = current_month.shift(-1);
    let current_quarter = Horizon::current_quarter(today);
    let current_year = Horizon::current_year(today);
    let current_decade = Horizon::current_decade(today);

    let board_id = store
        .add_list(&ListDraft {
            name: "Comagic".to_owned(),
            color: Some("#4F7DF3".to_owned()),
            ..Default::default()
        })
        .ok()
        .map(|l| l.id);
    let in_progress_sec = board_id
        .and_then(|id| store.add_list_section(Some(id), "In Progress").ok())
        .map(|s| s.id);
    let review_sec = board_id
        .and_then(|id| store.add_list_section(Some(id), "Review").ok())
        .map(|s| s.id);

    let meeting_start = NaiveTime::from_hms_opt(11, 0, 0);
    let meeting_end = NaiveTime::from_hms_opt(11, 30, 0);

    let drafts = [
        // Inbox
        TaskDraft {
            title: "Capture Q1 roadmap notes".to_owned(),
            tags: vec!["notes".to_owned()],
            ..Default::default()
        },
        TaskDraft {
            title: "Order standing desk mat".to_owned(),
            ..Default::default()
        },
        // Today (including timed 11:00-11:30 meeting)
        TaskDraft {
            title: "Design sync with team".to_owned(),
            due_date: Some(today),
            due_time: meeting_start,
            end_date: Some(today),
            end_time: meeting_end,
            priority: Priority::High,
            tags: vec!["work".to_owned()],
            notes: "Walk through Horizons layout, rollover UX, and dark mode contrast.".to_owned(),
            ..Default::default()
        },
        TaskDraft {
            title: "Finalize sprint release checklist".to_owned(),
            due_date: Some(today),
            priority: Priority::Medium,
            list_id: board_id,
            checklist: vec![
                ("Verify migrations".to_owned(), true),
                ("Run clippy and tests".to_owned(), false),
                ("Tag release notes".to_owned(), false),
            ],
            ..Default::default()
        },
        TaskDraft {
            title: "Evening run — 5 km".to_owned(),
            due_date: Some(today),
            tags: vec!["health".to_owned()],
            ..Default::default()
        },
        // Current ISO Week
        TaskDraft {
            title: "Prepare weekly product metrics digest".to_owned(),
            horizon: Some(current_week),
            priority: Priority::Medium,
            list_id: board_id,
            tags: vec!["work".to_owned()],
            ..Default::default()
        },
        TaskDraft {
            title: "Clean up archived project boards".to_owned(),
            horizon: Some(current_week),
            ..Default::default()
        },
        // Current Month
        TaskDraft {
            title: "Ship Horizons panoramic planner".to_owned(),
            horizon: Some(current_month),
            priority: Priority::High,
            list_id: board_id,
            tags: vec!["launch".to_owned()],
            ..Default::default()
        },
        TaskDraft {
            title: "Rebalance monthly budget & subscriptions".to_owned(),
            horizon: Some(current_month),
            tags: vec!["finance".to_owned()],
            ..Default::default()
        },
        // Past Month (for rollover demo in current Month column)
        TaskDraft {
            title: "Audit analytics retention funnel".to_owned(),
            horizon: Some(past_month),
            priority: Priority::Low,
            list_id: board_id,
            ..Default::default()
        },
        TaskDraft {
            title: "Renew passport appointment".to_owned(),
            horizon: Some(past_month),
            ..Default::default()
        },
        // Current Quarter
        TaskDraft {
            title: "Complete annual security review".to_owned(),
            horizon: Some(current_quarter),
            priority: Priority::Medium,
            ..Default::default()
        },
        // Current Year
        TaskDraft {
            title: "Run a half marathon under 1h 50m".to_owned(),
            horizon: Some(current_year),
            tags: vec!["health".to_owned()],
            ..Default::default()
        },
        TaskDraft {
            title: "Launch Fern 2.0 across Windows, macOS & Linux".to_owned(),
            horizon: Some(current_year),
            priority: Priority::High,
            ..Default::default()
        },
        // Current Decade
        TaskDraft {
            title: "Build a self-sustaining indie studio".to_owned(),
            horizon: Some(current_decade),
            ..Default::default()
        },
        // Life
        TaskDraft {
            title: "Write and publish a technical book".to_owned(),
            horizon: Some(Horizon::Life),
            ..Default::default()
        },
        TaskDraft {
            title: "Learn conversational Japanese".to_owned(),
            horizon: Some(Horizon::Life),
            tags: vec!["personal".to_owned()],
            ..Default::default()
        },
        // Board "Comagic" tasks across default + named sections
        TaskDraft {
            title: "Draft onboarding tour copy".to_owned(),
            list_id: board_id,
            ..Default::default()
        },
        TaskDraft {
            title: "Build webhook retry monitor".to_owned(),
            list_id: board_id,
            section_id: in_progress_sec,
            priority: Priority::Medium,
            ..Default::default()
        },
        TaskDraft {
            title: "QA dark mode contrast on call logs".to_owned(),
            list_id: board_id,
            section_id: review_sec,
            ..Default::default()
        },
    ];

    for draft in drafts {
        let _ = store.create_task(draft, false);
    }
}
