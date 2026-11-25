//! The Focus (Pomodoro) section, matched to TickTick's: things to focus on on the left with a timer
//! bar at the bottom, an overview and the focus record on the right.

use std::collections::HashMap;
use std::collections::HashSet;
use std::rc::Rc;
use web_time::{SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, NaiveDate};
#[cfg(debug_assertions)]
use fern_core::stats::Bucket;
use fern_core::{FocusSession, Id, Store, Task, View, focus_stats};
use gpui::{Entity, Focusable as _, FontWeight, ScrollHandle, Task as GpuiTask};
use ui::{Modal, Placement, PopoverAttachment, popover};
use ui::prelude::*;

use crate::focus_timer::{BreakKind, DEFAULT_LONG_BREAK_EVERY, FocusCycle, FocusTimer, MIN_FOCUS_MS};
use crate::habit_view::{HABIT_EMOJI, icon_avatar};
use crate::i18n::{self, Strings};
use crate::text_input::{Mode, TextInput};
#[path = "focus_controls.rs"]
mod focus_controls;
#[path = "focus_review.rs"]
mod focus_review;
mod modals;
mod picker;
mod stats;
mod stats_charts;
#[cfg(test)]
mod tests;
mod timer;

use modals::{RecordMenu, TimerDraft};
use picker::{NextFocus, Picker};
use stats::{StatsData, StatsPage};

/// Timer lengths in minutes: (setting, default, minimum, maximum).
/// Verified against PomoSettings::.cctor, RVA 0x000d1a68.
const POMO_SETTING: (&str, i64, i64, i64) = ("focus_pomo_minutes", 25, 5, 180);
const SHORT_BREAK_SETTING: (&str, i64, i64, i64) = ("focus_short_break_minutes", 5, 1, 60);
const LONG_BREAK_SETTING: (&str, i64, i64, i64) = ("focus_long_break_minutes", 15, 1, 60);
const DAILY_GOAL_SETTING: (&str, i64, i64, i64) = ("focus_daily_goal_minutes", 0, 0, 720);
const LONG_BREAK_EVERY_SETTING: (&str, i64, i64, i64) =
    ("focus_long_break_every", DEFAULT_LONG_BREAK_EVERY as i64, 1, 60);
const AUTO_COUNT_SETTING: (&str, i64, i64, i64) = ("focus_auto_count", 4, 2, 10);
/// "pomo" or "stopwatch".
const MODE_SETTING: &str = "focus_mode";

/// Cached Focus settings so `render()`, `clock_state()`, and 1Hz ticks perform zero SQLite reads.
#[derive(Clone, Copy, Debug)]
struct FocusSettings {
    pomo_minutes: i64,
    short_break_minutes: i64,
    long_break_minutes: i64,
    daily_goal_minutes: i64,
    long_break_every: i64,
    auto_count: i64,
    auto_break: bool,
    auto_next: bool,
    stopwatch_mode: bool,
    white_noise: desktop_common::sound::WhiteNoise,
}

impl FocusSettings {
    fn read_clamped(store: &Store, (key, default, min, max): (&str, i64, i64, i64)) -> i64 {
        store
            .setting(key)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
            .clamp(min, max)
    }

    fn load(store: &Store) -> Self {
        Self {
            pomo_minutes: Self::read_clamped(store, POMO_SETTING),
            short_break_minutes: Self::read_clamped(store, SHORT_BREAK_SETTING),
            long_break_minutes: Self::read_clamped(store, LONG_BREAK_SETTING),
            daily_goal_minutes: Self::read_clamped(store, DAILY_GOAL_SETTING),
            long_break_every: Self::read_clamped(store, LONG_BREAK_EVERY_SETTING),
            auto_count: Self::read_clamped(store, AUTO_COUNT_SETTING),
            auto_break: store.setting("focus_auto_break").ok().flatten().as_deref() == Some("1"),
            auto_next: store.setting("focus_auto_next").ok().flatten().as_deref() == Some("1"),
            stopwatch_mode: store.setting(MODE_SETTING).ok().flatten().as_deref() == Some("stopwatch"),
            white_noise: desktop_common::sound::white_noise(store),
        }
    }
}

/// Resolves a local datetime mapping to Unix epoch milliseconds without silently dropping DST
/// fall-back ambiguous times (`earliest()` returns `Some` for both `Single` and `Ambiguous`).
fn resolve_local_datetime<Tz: chrono::TimeZone>(
    mapped: chrono::LocalResult<chrono::DateTime<Tz>>,
) -> Option<i64> {
    mapped.earliest().map(|dt| dt.timestamp_millis())
}

fn duration_unit_label(unit: char, strings: &Strings) -> &'static str {
    match unit {
        'h' => strings.duration_hour_short,
        _ => strings.duration_min_short,
    }
}

fn localized_duration_parts(
    parts: Vec<(i64, char)>,
    strings: &Strings,
) -> Vec<(i64, Option<&'static str>)> {
    parts
        .into_iter()
        .map(|(n, u)| (n, Some(duration_unit_label(u, strings))))
        .collect()
}

pub(super) fn format_minutes(minutes: i64, strings: &Strings) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m}{}", strings.duration_min_short),
        (h, 0) => format!("{h}{}", strings.duration_hour_short),
        (h, m) => format!("{h}{} {m}{}", strings.duration_hour_short, strings.duration_min_short),
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FocusTarget {
    pub(super) task_id: Option<Id>,
    pub(super) habit_id: Option<Id>,
    pub(super) title: String,
}

struct Running {
    timer: FocusTimer,
    /// What the session is for: a task, a habit, or a timer name.
    task: Option<FocusTarget>,
    /// A break after a finished Pomodoro; breaks are not recorded.
    on_break: Option<BreakKind>,
    _ticker: GpuiTask<()>,
}

type FocusTask = FocusTarget;

struct SessionSummaryCache {
    day: NaiveDate,
    today_minutes_by_task: HashMap<Id, i64>,
    sessions_by_task: HashMap<Id, (usize, i64)>,
    today_minutes_by_timer: HashMap<String, i64>,
    stats: fern_core::FocusStats,
}

impl SessionSummaryCache {
    fn empty(day: NaiveDate) -> Self {
        Self {
            day,
            today_minutes_by_task: HashMap::new(),
            sessions_by_task: HashMap::new(),
            today_minutes_by_timer: HashMap::new(),
            stats: fern_core::FocusStats::default(),
        }
    }

    fn build(sessions: &[FocusSession], day: NaiveDate) -> Self {
        let mut today_minutes_by_task: HashMap<Id, i64> = HashMap::new();
        let mut sessions_by_task: HashMap<Id, (usize, i64)> = HashMap::new();
        let mut today_minutes_by_timer: HashMap<String, i64> = HashMap::new();
        for session in sessions {
            let is_today = session.start_local().date_naive() == day;
            let mins = session.minutes();
            if let Some(task_id) = session.task_id {
                let entry = sessions_by_task.entry(task_id).or_default();
                entry.0 += 1;
                entry.1 += (session.ended_at - session.started_at) / 60_000;
                if is_today {
                    *today_minutes_by_task.entry(task_id).or_default() += mins;
                }
            }
            if is_today && !session.title.is_empty() {
                *today_minutes_by_timer.entry(session.title.clone()).or_default() += mins;
            }
        }
        Self {
            day,
            today_minutes_by_task,
            sessions_by_task,
            today_minutes_by_timer,
            stats: focus_stats(sessions, day),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PendingFocus {
    View,
    RecordNote,
}

pub struct FocusView {
    store: Rc<Store>,
    focus_handle: gpui::FocusHandle,
    /// Cached settings (timer lengths, automation flags, mode, white noise) refreshed on
    /// mutation and `reload()`/`DataVersion` so `render()` and 1Hz ticks never hit SQLite.
    settings: FocusSettings,
    /// Last `(active, preset)` sent to the ambient sound worker so unchanged renders are a no-op.
    last_ambient: (bool, desktop_common::sound::WhiteNoise),
    tasks: Vec<Task>,
    sessions: Vec<FocusSession>,
    /// Precomputed per-task/timer session aggregates and focus stats, refreshed on `reload()` or day rollover.
    session_summary: SessionSummaryCache,
    /// What the Statistics page reads, loaded lazily when Statistics or the picker opens.
    stats_data: StatsData,
    stats_data_dirty: bool,
    running: Option<Running>,
    finish_prompt: bool,
    review_window: Option<gpui::WindowHandle<focus_review::FocusReview>>,
    continuation: Option<(Option<FocusTask>, i64)>,
    completed_in_chain: usize,
    cycle: FocusCycle,
    record_menu: Option<RecordMenu>,
    /// The Pomodoro list's timers (the original's "Add Timer"), the tab showing (archived or
    /// active), the "Add Timer"/edit popover and the timer whose "…" menu is open.
    timers: Vec<fern_core::focus::FocusTimer>,
    archived_tab: bool,
    timer_draft: Option<TimerDraft>,
    timer_menu: Option<Id>,
    confirm_delete_timer: Option<(Id, String)>,
    confirm_delete_all: bool,
    confirm_delete_record: Option<(Id, String)>,
    /// The records ticked in Batch Management, its filters and the filter whose menu is open.
    batch: HashSet<Id>,
    confirm_batch_delete: bool,
    batch_filter: fern_core::focus::RecordFilter,
    batch_menu: Option<u8>,
    /// A filter's Custom fields, while open: the filter (0 Date, 1 Duration) and its two fields
    /// (Start and End, or Min and Max).
    batch_custom: Option<(u8, Entity<TextInput>, Entity<TextInput>)>,
    /// The original's full focus page, opened from the timer bar.
    expanded: bool,
    /// The page's picker of what to focus on, while open.
    picker: Option<Picker>,
    /// What the page's Start focuses on, picked there.
    next: Option<NextFocus>,
    /// The picker's tab, kept between openings as the original keeps it.
    picker_tab: usize,
    /// The view the picker's Task tab lists ("Today" until another is chosen).
    picker_view: View,
    add_title: Entity<TextInput>,
    add_start: Entity<TextInput>,
    add_minutes: i64,
    /// The last export's result.
    message: Option<String>,
    /// The Statistics page, while it is open.
    stats: Option<StatsPage>,
    stats_scroll: ScrollHandle,
    /// The focus record whose dialog is open, and its note field.
    open_record: Option<Id>,
    record_note: Entity<TextInput>,
    pending_focus: Option<PendingFocus>,
    refresh: desktop_common::data_version::ViewRefreshState,
}

impl gpui::Focusable for FocusView {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl FocusView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, true, cx)
    }

    pub fn new_inactive(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, false, cx)
    }

    fn new_with_active(store: Rc<Store>, initial_active: bool, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let add_title = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.focus_record_title, cx));
        let add_start = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.focus_record_start, cx));
        let record_note = cx.new(|cx| TextInput::new(Mode::MultiLine, strings.focus_note_placeholder, cx));
        cx.subscribe(&record_note, |this, _, _: &crate::text_input::Cancelled, cx| {
            if this.open_record.is_some() {
                this.close_record(cx);
                this.pending_focus = Some(PendingFocus::View);
            }
        })
        .detach();
        for input in [&add_title, &add_start] {
            cx.subscribe(input, |this, _, _: &crate::text_input::Cancelled, cx| {
                if this.record_menu == Some(RecordMenu::Add) {
                    this.set_record_menu(None, cx);
                    this.pending_focus = Some(PendingFocus::View);
                }
            })
            .detach();
        }
        let settings = FocusSettings::load(&store);
        cx.observe_global::<desktop_common::data_version::DataVersion>(|this, cx| {
            if this.refresh.on_data_version(cx.entity_id(), cx)
                || (this.refresh.is_dirty() && this.running_clock().is_some())
            {
                this.reload(cx);
            }
        })
        .detach();
        let mut this = Self {
            store,
            focus_handle: cx.focus_handle(),
            settings,
            last_ambient: (false, desktop_common::sound::WhiteNoise::Off),
            tasks: Vec::new(),
            sessions: Vec::new(),
            session_summary: SessionSummaryCache::empty(Local::now().date_naive()),
            stats_data: StatsData::default(),
            stats_data_dirty: true,
            running: None,
            finish_prompt: false,
            review_window: None,
            continuation: None,
            completed_in_chain: 0,
            cycle: FocusCycle::default(),
            record_menu: None,
            timers: Vec::new(),
            archived_tab: false,
            timer_draft: None,
            timer_menu: None,
            confirm_delete_timer: None,
            confirm_delete_all: false,
            confirm_delete_record: None,
            batch: HashSet::new(),
            confirm_batch_delete: false,
            batch_filter: Default::default(),
            batch_menu: None,
            batch_custom: None,
            expanded: false,
            picker: None,
            next: None,
            picker_tab: 0,
            picker_view: View::Today,
            add_title,
            add_start,
            add_minutes: 25,
            message: None,
            stats: None,
            stats_scroll: ScrollHandle::new(),
            open_record: None,
            record_note,
            pending_focus: None,
            refresh: desktop_common::data_version::ViewRefreshState::new(
                initial_active,
                desktop_common::data_version::DataScope::FOCUS
                    | desktop_common::data_version::DataScope::TASKS
                    | desktop_common::data_version::DataScope::HABITS
                    | desktop_common::data_version::DataScope::PREFS,
                cx,
            ),
        };
        if initial_active {
            this.reload(cx);
        }
        this
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.refresh.set_active(active, cx) {
            self.reload(cx);
        }
    }

    pub fn sync_external(&mut self, cx: &mut Context<Self>) {
        if self.running_clock().is_some() || self.refresh.mark_dirty_or_reload() {
            self.reload(cx);
        }
    }

    pub(super) fn ensure_stats_data(&mut self) {
        if !self.stats_data_dirty {
            return;
        }
        let habit_records = self.store.all_habit_checkins().unwrap_or_default();
        let all_habits = self.store.habits().unwrap_or_default();
        let habits: Vec<(fern_core::Habit, HashSet<NaiveDate>)> = all_habits
            .iter()
            .map(|h| {
                let done = habit_records
                    .get(&h.id)
                    .map(|recs| fern_core::habit::done_dates(recs))
                    .unwrap_or_default();
                (h.clone(), done)
            })
            .collect();
        let all_habit_checkins = habits
            .iter()
            .flat_map(|(_, done)| done.iter().copied())
            .collect();
        let lists = self.store.lists().unwrap_or_default();
        let list_lookup = lists
            .iter()
            .map(|l| {
                (
                    l.id,
                    (
                        SharedString::from(l.name.clone()),
                        crate::lists_panel::parse_color(l.color.as_deref()),
                    ),
                )
            })
            .collect();
        let tag_colors = self
            .store
            .tags()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| {
                crate::lists_panel::parse_color(t.color.as_deref()).map(|c| (t.name, c))
            })
            .collect();
        self.stats_data = StatsData {
            tasks: self.store.tasks().unwrap_or_default(),
            lists,
            list_lookup,
            tag_colors,
            all_habits,
            all_habit_checkins,
            habits,
            habit_records,
        };
        self.stats_data_dirty = false;
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.refresh.mark_clean(cx);
        let today = Local::now().date_naive();
        self.settings = FocusSettings::load(&self.store);
        self.sessions = self.store.focus_sessions().unwrap_or_default();
        if self
            .confirm_delete_record
            .as_ref()
            .is_some_and(|(id, _)| !self.sessions.iter().any(|s| s.id == *id))
        {
            self.confirm_delete_record = None;
        }
        self.session_summary = SessionSummaryCache::build(&self.sessions, today);
        self.timers = self.store.focus_timers(self.archived_tab).unwrap_or_default();
        self.stats_data_dirty = true;
        self.stats_data = StatsData::default();
        if self.stats.is_some() || self.picker.is_some() {
            self.ensure_stats_data();
            self.tasks = self
                .store
                .tasks_in_from(self.stats_data.tasks.clone(), &View::Today, today)
                .unwrap_or_default();
            self.refresh_picker_cache(today);
        } else {
            self.tasks = self.store.tasks_in(&View::Today, today).unwrap_or_default();
        }
        self.sync_ambient_sound();
        cx.notify();
    }

    fn minutes(&self, (key, default, min, max): (&str, i64, i64, i64)) -> i64 {
        match key {
            "focus_pomo_minutes" => self.settings.pomo_minutes,
            "focus_short_break_minutes" => self.settings.short_break_minutes,
            "focus_long_break_minutes" => self.settings.long_break_minutes,
            "focus_daily_goal_minutes" => self.settings.daily_goal_minutes,
            "focus_long_break_every" => self.settings.long_break_every,
            "focus_auto_count" => self.settings.auto_count,
            _ => FocusSettings::read_clamped(&self.store, (key, default, min, max)),
        }
    }

    fn set_minutes(&mut self, (key, default, min, max): (&str, i64, i64, i64), delta: i64, cx: &mut Context<Self>) {
        let value = (self.minutes((key, default, min, max)) + delta).clamp(min, max);
        match key {
            "focus_pomo_minutes" => self.settings.pomo_minutes = value,
            "focus_short_break_minutes" => self.settings.short_break_minutes = value,
            "focus_long_break_minutes" => self.settings.long_break_minutes = value,
            "focus_daily_goal_minutes" => self.settings.daily_goal_minutes = value,
            "focus_long_break_every" => self.settings.long_break_every = value,
            "focus_auto_count" => self.settings.auto_count = value,
            _ => {}
        }
        if let Err(err) = self.store.set_setting(key, &value.to_string()) {
            desktop_common::logging::log_error("focus_ui", format!("failed to save the focus setting: {err:#}"));
        }
        cx.notify();
    }

    /// The Statistics page on its Overview tab, as the account menu opens it.
    pub fn open_stats_overview(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
        self.open_stats(cx);
        self.update_stats(cx, |p| p.tab = 0);
    }

    pub fn close_stats(&mut self, cx: &mut Context<Self>) -> bool {
        if self.stats.take().is_some() {
            cx.emit(StatsClosed);
            cx.notify();
            true
        } else {
            false
        }
    }

    fn bind_focus_root<E: InteractiveElement>(&self, el: E, cx: &mut Context<Self>) -> E {
        el.track_focus(&self.focus_handle)
            .key_context("Focus")
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let m = &event.keystroke.modifiers;
                if event.keystroke.key == "space"
                    && !m.control
                    && !m.alt
                    && !m.platform
                    && !m.shift
                    && this.focus_handle.is_focused(window)
                    && this.stats.is_none()
                    && this.open_record.is_none()
                    && this.timer_draft.is_none()
                    && this.picker.is_none()
                    && !this.finish_prompt
                    && !this.confirm_batch_delete
                    && !this.confirm_delete_all
                    && this.confirm_delete_timer.is_none()
                    && this.confirm_delete_record.is_none()
                    && this.batch_custom.is_none()
                    && this.record_menu != Some(RecordMenu::Batch)
                    && this.review_window.is_none()
                {
                    if this.running.is_some() {
                        this.toggle_pause(cx);
                    } else {
                        this.start_next(cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &crate::actions::TogglePause, _, cx| {
                this.toggle_pause_or_start(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::StartStop, window, cx| {
                this.start_or_end(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::DismissTransient, _, cx| {
                if !this.dismiss_transient(cx) {
                    cx.propagate();
                }
            }))
    }

    pub fn dismiss_transient(&mut self, cx: &mut Context<Self>) -> bool {
        if self.confirm_delete_record.is_some() {
            self.cancel_delete_record(None, cx);
            return true;
        }
        if std::mem::take(&mut self.confirm_batch_delete) {
            cx.notify();
            return true;
        }
        if std::mem::take(&mut self.confirm_delete_all) {
            cx.notify();
            return true;
        }
        if self.confirm_delete_timer.take().is_some() {
            cx.notify();
            return true;
        }
        if self.stats.as_mut().is_some_and(|stats| {
            stats.curve_menu.take().is_some()
                || std::mem::take(&mut stats.task_menu)
                || std::mem::take(&mut stats.classify_menu)
                || stats.details_menu.take().is_some()
        }) {
            cx.notify();
            return true;
        }
        if self.batch_custom.take().is_some() {
            self.pending_focus = Some(PendingFocus::View);
            cx.notify();
            return true;
        }
        if self.batch_menu.take().is_some() {
            cx.notify();
            return true;
        }
        if self.timer_draft.as_mut().is_some_and(|draft| draft.icon_tab.take().is_some()) {
            cx.notify();
            return true;
        }
        if self.open_record.is_some() {
            self.close_record(cx);
            return true;
        }
        if self.record_menu.take().is_some()
            || self.timer_menu.take().is_some()
            || self.timer_draft.take().is_some()
            || self.picker.take().is_some()
        {
            cx.notify();
            true
        } else if self.finish_prompt {
            // Keep the unsaved session prompt open so Escape does not discard focus work.
            true
        } else if self.stats.is_some() {
            self.close_stats(cx)
        } else {
            false
        }
    }

    #[cfg(debug_assertions)]
    pub fn dev_add_timer(&mut self, icons: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open_timer(None, window, cx);
        if let Some(draft) = self.timer_draft.as_mut() {
            draft.icon_tab = icons.then_some(0);
        }
    }

    #[cfg(debug_assertions)]
    pub fn dev_menu(&mut self, menu: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.refresh.is_dirty() {
            self.reload(cx);
        }
        let menu = match menu {
            "add" => RecordMenu::Add,
            "settings" => RecordMenu::Settings,
            "more" => RecordMenu::More,
            "header-more" => RecordMenu::HeaderMore,
            "stopwatch" => {
                self.set_stopwatch_mode(true, cx);
                return;
            }
            // The Overview with the completion curve by week and its period menu open.
            "stats-curves" => {
                self.open_stats(cx);
                self.update_stats(cx, |p| {
                    p.tab = 0;
                    p.curves[0] = Bucket::Week;
                    p.curves[2] = Bucket::Month;
                    p.curve_menu = Some(1);
                });
                return;
            }
            // The full focus page.
            "page" => {
                self.set_expanded(true, cx);
                return;
            }
            // The page with its picker open: `page-pick` (Task), `page-pick-timer`, or
            // `page-pick-views` (the Task tab's list of views).
            "page-pick-views" => {
                self.set_expanded(true, cx);
                self.open_picker(false, window, cx);
                self.picker_tab = 0;
                if let Some(picker) = self.picker.as_mut() {
                    picker.views_open = true;
                }
                return;
            }
            "page-pick" | "page-pick-timer" => {
                self.set_expanded(true, cx);
                self.open_picker(false, window, cx);
                self.picker_tab = (menu == "page-pick-timer") as usize;
                return;
            }
            // Batch Management with a filter's Custom fields open: `batch-date` or `batch-minutes`.
            "batch-date" | "batch-minutes" => {
                self.record_menu = Some(RecordMenu::Batch);
                self.open_batch_custom(if menu == "batch-date" { 0 } else { 1 }, window, cx);
                return;
            }
            // Batch Management with the newest record ticked.
            "batch" => {
                self.batch = self.sessions.first().map(|s| s.id).into_iter().collect();
                self.batch_menu = Some(1);
                self.record_menu = Some(RecordMenu::Batch);
                cx.notify();
                return;
            }
            // The newest record's dialog, with a note.
            "record" => {
                if let Some(session) = self.sessions.first() {
                    let id = session.id;
                    self.store.set_focus_note(id, "Outlined the first two sections").ok();
                    self.reload(cx);
                    self.record_note.update(cx, |input, cx| input.set_text("Outlined the first two sections", cx));
                    self.open_record = Some(id);
                }
                return;
            }
            "stats-focus" => {
                self.open_stats(cx);
                return;
            }
            "stats-focus-details" => {
                self.open_stats(cx);
                self.update_stats(cx, |p| {
                    p.details_by = 2;
                    p.details_menu = Some(0);
                });
                return;
            }
            "stats-task-day" => {
                self.open_stats(cx);
                self.update_stats(cx, |p| p.tab = 1);
                return;
            }
            "stats-task-week" | "stats-task-tags" => {
                let tags = menu == "stats-task-tags";
                self.open_stats(cx);
                self.update_stats(cx, |p| {
                    p.tab = 1;
                    p.task_period = Bucket::Week;
                    p.by_tag = tags;
                    p.classify_menu = tags;
                });
                return;
            }
            "stats" | "stats-overview" | "stats-task" => {
                self.open_stats(cx);
                let tab = match menu {
                    "stats-overview" => 0,
                    "stats-task" => 1,
                    _ => 2,
                };
                self.update_stats(cx, |p| p.tab = tab);
                return;
            }
            _ => RecordMenu::More,
        };
        self.set_record_menu(Some(menu), cx);
    }

    #[cfg(debug_assertions)]
    pub fn dev_record(&mut self, title: &str, minutes_ago: i64, minutes: i64, stopwatch: bool, cx: &mut Context<Self>) {
        let start = now_ms() - minutes_ago * 60_000;
        self.store.add_focus_session_as(None, title, start, start + minutes * 60_000, stopwatch).ok();
        self.reload(cx);
    }

    fn render_overview(&self, strings: &Strings, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let today = Local::now().date_naive();
        let stats = if self.session_summary.day == today {
            self.session_summary.stats
        } else {
            focus_stats(&self.sessions, today)
        };
        // As the original's: the numbers large, their unit letters small beside them,
        // on the same baseline.
        let card = |label: &'static str, parts: Vec<(i64, Option<&'static str>)>| {
            v_flex()
                .flex_1()
                .h(px(73.))
                .px_3()
                .py_2()
                .gap_0p5()
                .rounded(px(8.))
                .bg(theme.colors().surface_background)
                .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
                .child(h_flex().items_end().gap(px(6.)).children(parts.into_iter().map(|(number, unit)| {
                    h_flex()
                        .items_end()
                        .child(div().text_size(px(26.)).line_height(px(32.)).font_weight(FontWeight::SEMIBOLD).child(number.to_string()))
                        .children(unit.map(|u| {
                            div().text_size(px(14.)).line_height(px(14.)).mb(px(6.)).ml_0p5().font_weight(FontWeight::SEMIBOLD).child(u)
                        }))
                })))
        };
        let daily_goal = self.minutes(DAILY_GOAL_SETTING);
        let duration = |minutes: i64| localized_duration_parts(fern_core::focus::duration_parts(minutes), strings);
        let today_focus_card = {
            let parts = duration(stats.today_minutes);
            let pct = (daily_goal > 0).then(|| ((stats.today_minutes.max(0) * 100) / daily_goal).min(999));
            let fill_frac = if daily_goal > 0 {
                (stats.today_minutes.max(0) as f32 / daily_goal as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let reached = daily_goal > 0 && stats.today_minutes >= daily_goal;
            v_flex()
                .flex_1()
                .h(px(73.))
                .px_3()
                .py_2()
                .gap_0p5()
                .rounded(px(8.))
                .bg(theme.colors().surface_background)
                .child(
                    h_flex()
                        .gap_1()
                        .child(div().flex_1().min_w_0().child(Label::new(strings.focus_today_focus).size(LabelSize::Small).color(Color::Muted).truncate()))
                        .when_some(pct, |el, pct| {
                            el.child(
                                Label::new(format!("{pct}%"))
                                    .size(LabelSize::XSmall)
                                    .weight(FontWeight::SEMIBOLD)
                                    .color(if reached { Color::Custom(theme.status().success) } else { Color::Accent }),
                            )
                        }),
                )
                .child(h_flex().items_end().gap(px(6.)).children(parts.into_iter().map(|(number, unit)| {
                    h_flex()
                        .items_end()
                        .child(div().text_size(px(26.)).line_height(px(32.)).font_weight(FontWeight::SEMIBOLD).child(number.to_string()))
                        .children(unit.map(|u| {
                            div().text_size(px(14.)).line_height(px(14.)).mb(px(6.)).ml_0p5().font_weight(FontWeight::SEMIBOLD).child(u)
                        }))
                })))
                .when(daily_goal > 0, |el| {
                    el.child(
                        div()
                            .w_full()
                            .h(px(3.))
                            .rounded_full()
                            .bg(theme.colors().border_variant)
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(fill_frac))
                                    .rounded_full()
                                    .bg(if reached { theme.status().success } else { theme.colors().accent }),
                            ),
                    )
                })
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(card(strings.focus_today_pomo, vec![(stats.today_count as i64, None)]))
                    .child(today_focus_card),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(card(strings.focus_total_pomo, vec![(stats.total_count as i64, None)]))
                    .child(card(strings.focus_total_duration, duration(stats.total_minutes))),
            )
    }

    /// "Focus Record" with its "+" and "…" menus; `large` for the Focus section, smaller on the
    /// Statistics page's Focus tab.
    fn render_record_header(&self, large: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        h_flex()
                    .when(large, |el| el.pt_5())
                    .child(div().flex_1().child(
                        Label::new(strings.focus_record)
                            .size(if large { LabelSize::Large } else { LabelSize::Default })
                            .weight(if large { FontWeight::MEDIUM } else { FontWeight::SEMIBOLD }),
                    ))
                    .child(
                        div()
                            .relative()
                            .child(
                                IconButton::new("record-add", IconName::Plus)
                                    .tooltip(Tooltip::text(strings.focus_add_record))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_record_menu(Some(RecordMenu::Add), cx)
                                    })),
                            )
                            .when(self.record_menu == Some(RecordMenu::Add), |el| {
                                el.child(PopoverAttachment::new(Placement::BelowRight, self.render_record_menu(RecordMenu::Add, cx)))
                            }),
                    )
                    .child(
                        div()
                            .relative()
                            .child(
                                div()
                                    .when(cfg!(test), |el| el.debug_selector(|| "focus-record-more".to_owned()))
                                    .child(
                                        IconButton::new("record-more", IconName::Ellipsis)
                                            .tooltip(Tooltip::text(strings.more))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.set_record_menu(Some(RecordMenu::More), cx)
                                            })),
                                    ),
                            )
                            .when_some(self.record_menu.filter(|m| *m == RecordMenu::More), |el, menu| {
                                el.child(PopoverAttachment::new(Placement::BelowRight, self.render_record_menu(menu, cx)))
                            }),
                    )
    }

    fn render_record(&self, strings: &Strings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let today = Local::now().date_naive();
        let mut out = Vec::new();
        let mut current: Option<NaiveDate> = None;
        for session in &self.sessions {
            let session_id = session.id;
            let group = SharedString::from(format!("focus-record-{session_id}"));
            let start = session.start_local();
            let date = start.date_naive();
            if current != Some(date) {
                current = Some(date);
                let label = crate::task_rows::format_day(
                    strings.short_date[usize::from(date.year() != today.year())], date, strings,
                );
                out.push(div().pt_3().pb_1().child(Label::new(label).size(LabelSize::Small).color(Color::Muted)).into_any_element());
            }
            out.push(
                h_flex()
                    .id(group.clone())
                    .group(group.clone())
                    .items_start()
                    .gap_2()
                    .py_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| this.open_record(session_id, window, cx)))
                    // As in the original: the session's kind (Pomodoro or stopwatch) in a circle,
                    // and a line down to a small ring beside the title.
                    .child(
                        v_flex()
                            .w(px(20.))
                            .flex_none()
                            .items_center()
                            .child(
                                div()
                                    .size(px(20.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(theme.colors().accent.opacity(0.2))
                                    .child(
                                        Icon::new(if session.stopwatch { IconName::Timer } else { IconName::CircleDot })
                                            .size(IconSize::XSmall)
                                            .color(Color::Accent),
                                    ),
                            )
                            .child(div().w(px(1.)).h(px(6.)).bg(theme.colors().accent.opacity(0.5)))
                            .child(div().size(px(7.)).rounded_full().border_1().border_color(theme.colors().accent)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                Label::new(format!(
                                    "{} - {}",
                                    crate::calendar::format_time(start, strings, cx),
                                    crate::calendar::format_time(session.end_local(), strings, cx)
                                ))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                            )
                            .child(Label::new(session.title.clone()).truncate())
                            .when(!session.note.is_empty(), |el| {
                                el.child(Label::new(session.note.clone()).size(LabelSize::Small).color(Color::Muted).truncate())
                            }),
                    )
                    .child(Label::new(format_minutes(session.minutes(), strings)).size(LabelSize::XSmall).color(Color::Muted))
                    .child(
                        div().opacity(if crate::touch::active() { 1. } else { 0. }).group_hover(group, |s| s.opacity(1.)).child(
                            IconButton::new(SharedString::from(format!("delete-record-{session_id}")), IconName::X)
                                .size(px(20.))
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .hover_icon_color(Color::Error)
                                .tooltip(Tooltip::text(strings.delete))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.request_delete_record(session_id, window, cx);
                                })),
                        ),
                    )
                    .into_any_element(),
            );
        }
        out
    }
}

/// "Done" on the Statistics page, which the app shows over everything when opened from the
/// account menu.
pub struct StatsClosed;

impl gpui::EventEmitter<StatsClosed> for FocusView {}

impl FocusView {
    /// Pomo or Stopwatch, chosen while no timer runs.
    pub(super) fn render_mode_switch(&self, size: LabelSize, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let stopwatch = self.stopwatch_mode();
        let height = if size == LabelSize::XSmall { 24. } else { 30. };
        h_flex()
            .p_0p5()
            .rounded(px(height / 2. + 2.))
            .bg(theme.colors().surface_background)
            .children(strings.focus_modes.iter().enumerate().map(|(i, label)| {
                let selected = (i == 1) == stopwatch;
                div()
                    .id(SharedString::from(format!("focus-mode-{i}")))
                    .px_3()
                    .h(px(height))
                    .flex()
                    .items_center()
                    .rounded(px(height / 2.))
                    .cursor_pointer()
                    .when(selected, |el| el.bg(theme.colors().element_selected))
                    .child(Label::new(*label).size(size).color(if selected { Color::Default } else { Color::Muted }))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_stopwatch_mode(i == 1, cx)))
            }))
            .into_any_element()
    }

    pub(super) fn render_confirm_delete_all(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        div()
            .absolute()
            .inset_0()
            .when(cfg!(test), |el| {
                el.debug_selector(|| "focus-delete-all-modal".to_owned())
            })
            .child(
                ConfirmDialog::new("focus-delete-all", strings.delete_all, strings.cancel, strings.delete)
                    .width(px(360.))
                    .description(strings.focus_batch_delete_message)
                    .destructive(true)
                    .on_cancel(cx.listener(|this, _, _, cx| {
                        this.confirm_delete_all = false;
                        cx.notify();
                    }))
                    .on_confirm(cx.listener(|this, _, _, cx| {
                        this.confirm_delete_all = false;
                        if let Err(err) = this.store.delete_all_focus_sessions() {
                            desktop_common::logging::log_error("focus_ui", format!("failed to delete the focus records: {err:#}"));
                        }
                        this.reload(cx);
                        desktop_common::data_version::data_changed_scoped(
                            cx.entity_id(),
                            desktop_common::data_version::DataScope::FOCUS,
                            cx,
                        );
                    })),
            )
            .into_any_element()
    }
}

impl Drop for FocusView {
    fn drop(&mut self) {
        desktop_common::sound::stop_white_noise();
    }
}

impl Render for FocusView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.pending_focus.take() {
            Some(PendingFocus::RecordNote)
                if self.open_record.is_some() && self.confirm_delete_record.is_none() =>
            {
                window.focus(&self.record_note.focus_handle(cx), cx);
            }
            Some(_) => window.focus(&self.focus_handle, cx),
            None => {
                if (self.open_record.is_none() && self.record_note.focus_handle(cx).is_focused(window))
                    || (self.record_menu != Some(RecordMenu::Add)
                        && (self.add_title.focus_handle(cx).is_focused(window)
                            || self.add_start.focus_handle(cx).is_focused(window)))
                {
                    window.focus(&self.focus_handle, cx);
                }
            }
        }
        self.sync_ambient_sound();
        if let Some(page) = self.stats {
            return self.render_stats(page, window, cx);
        }
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let now = now_ms();

        let (clock, label, running, paused) = self.clock_state(now, strings);

        if self.expanded {
            return self.render_page(clock, label, running, paused, window, cx);
        }

        // Refresh precomputed session summaries only if the calendar day rolled over.
        let today = Local::now().date_naive();
        if self.session_summary.day != today {
            self.session_summary = SessionSummaryCache::build(&self.sessions, today);
        }
        let rows: Vec<AnyElement> = self
            .tasks
            .iter()
            .map(|task| {
                let target = (task.id, task.title.clone());
                let (task_sessions, task_minutes) = self
                    .session_summary
                    .sessions_by_task
                    .get(&task.id)
                    .copied()
                    .unwrap_or((0, 0));
                let today_minutes = self
                    .session_summary
                    .today_minutes_by_task
                    .get(&task.id)
                    .copied()
                    .unwrap_or(0);
                ListItem::new(SharedString::from(format!("focus-task-{}", task.id)))
                    .height(px(40.))
                    .start_slot(Icon::new(IconName::CircleDot).color(Color::Accent))
                    .child(Label::new(task.title.clone()).truncate())
                    .end_slot(
                        h_flex()
                            .gap_2()
                            .child(
                                // With an Estimation, the progress towards it over all sessions.
                                Label::new(match task.estimate {
                                    Some(fern_core::Estimate::Pomos(n)) => {
                                        format!("{task_sessions}/{n} {}", strings.pomo_unit)
                                    }
                                    Some(fern_core::Estimate::Minutes(n)) => {
                                        format!("{}/{}", format_minutes(task_minutes, strings), format_minutes(n as i64, strings))
                                    }
                                    None => format_minutes(today_minutes, strings),
                                })
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(Icon::new(IconName::Play).size(IconSize::Small).color(Color::Accent)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.start(Some(target.clone()), cx)))
                    .into_any_element()
            })
            .collect();

        // A timer's minutes today: the sessions named after it.
        let timer_rows: Vec<AnyElement> = self
            .timers
            .iter()
            .map(|timer| {
                let minutes = self
                    .session_summary
                    .today_minutes_by_timer
                    .get(&timer.name)
                    .copied()
                    .unwrap_or(0);
                self.render_timer(timer, minutes, cx)
            })
            .collect();
        let left = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .border_r_1()
            .border_color(theme.colors().border)
            .child(
                h_flex()
                    .relative()
                    .mt(crate::title_bar::header_inset(window, cx))
                    .h(px(52.))
                    .flex_none()
                    .px(px(22.))
                    .gap_2()
                    .child(div().flex_1().child(Label::new(strings.pomodoro).size(LabelSize::Large).weight(FontWeight::SEMIBOLD)))
                    // The original's Active and Archived tabs, centered over the timers.
                    .child(div().absolute().left_0().right_0().top_0().h_full().flex().items_center().justify_center().child(h_flex().p(px(2.)).rounded(px(18.)).bg(theme.colors().element_hover).children(
                        [(false, strings.habit_active), (true, strings.habit_archived)].map(|(archived, label)| {
                            let on = self.archived_tab == archived;
                            div()
                                .id(SharedString::from(format!("timers-tab-{archived}")))
                                .px_5()
                                .h(px(32.))
                                .flex()
                                .items_center()
                                .rounded(px(16.))
                                .cursor_pointer()
                                .when(on, |el| el.bg(theme.colors().element_selected))
                                .when(!on, |el| el.hover(|s| s.bg(theme.colors().ghost_element_hover)))
                                .child(Label::new(label).color(if on { Color::Default } else { Color::Muted }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.archived_tab = archived;
                                    this.reload(cx);
                                }))
                        }),
                    )))
                    .child(
                        IconButton::new("timer-add", IconName::Plus)
                            .tooltip(Tooltip::text(strings.add_timer))
                            .on_click(cx.listener(|this, _, window, cx| this.open_timer(None, window, cx))),
                    )
                    .child(
                        div()
                            .relative()
                            .child(
                                IconButton::new("pomo-more", IconName::Ellipsis)
                                    .tooltip(Tooltip::text(strings.more))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_record_menu(Some(RecordMenu::HeaderMore), cx)
                                    })),
                            )
                            .when_some(
                                self.record_menu.filter(|m| matches!(m, RecordMenu::HeaderMore | RecordMenu::Settings)),
                                |el, menu| el.child(PopoverAttachment::new(Placement::BelowRight, self.render_record_menu(menu, cx))),
                            ),
                    ),
            )
            .child(
                v_scroll(
                    "focus-tasks-scroll",
                    v_flex()
                        .id("focus-tasks")
                        .px_3()
                        .children(timer_rows)
                        .when(!self.archived_tab && rows.is_empty() && self.timers.is_empty(), |el| {
                            el.child(
                                div().w_full().py_6().child(
                                    EmptyState::new(IconName::CircleDot, strings.focus_no_tasks).action(
                                        "focus-empty-add-timer",
                                        strings.add_timer,
                                        cx.listener(|this, _, window, cx| this.open_timer(None, window, cx)),
                                    ),
                                ),
                            )
                        })
                        .when(self.archived_tab && self.timers.is_empty(), |el| {
                            el.child(
                                div()
                                    .w_full()
                                    .py_6()
                                    .child(EmptyState::new(IconName::Archive, strings.nothing_found)),
                            )
                        })
                        .when(!self.archived_tab && !self.timers.is_empty() && !rows.is_empty(), |el| el.child(div().my_1().child(Divider::horizontal())))
                        .when(!self.archived_tab, |el| el.children(rows)),
                )
                .flex_1()
                .min_h_0(),
            )
            .when(running, |el| {
                el.child(
                    v_flex()
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .py_6()
                        .child(div().text_size(px(64.)).font_weight(FontWeight::LIGHT).child(clock.clone()))
                        .child(Label::new(label.clone()).color(Color::Muted)),
                )
            })
            .child(
                h_flex()
                    .h(px(64.))
                    .flex_none()
                    .px(px(22.))
                    .gap_3()
                    .border_t_1()
                    .border_color(theme.colors().border)
                    .child(Icon::new(IconName::CircleDot).size(IconSize::XLarge).color(Color::Accent))
                    // As in the original, the bar opens the full focus page.
                    .child(
                        v_flex()
                            .id("focus-bar-open")
                            .flex_1()
                            .min_w_0()
                            .px_1p5()
                            .rounded(px(6.))
                            .hover(|s| s.bg(theme.colors().ghost_element_hover))
                            .cursor_pointer()
                            .child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted).truncate())
                            .child(div().text_size(px(20.)).child(clock))
                            .on_click(cx.listener(|this, _, _, cx| this.set_expanded(true, cx))),
                    )
                    // Pomo or Stopwatch, chosen while no timer runs.
                    .when(!running, |el| el.child(self.render_mode_switch(LabelSize::XSmall, cx)))
                    .child(self.render_white_noise_button("focus-white-noise-btn", cx))
                    .when(running, |el| {
                        el.child(
                            IconButton::new("focus-stop", IconName::Square)
                                .tooltip(Tooltip::text(strings.focus_end))
                                .on_click(cx.listener(|this, _, window, cx| this.request_finish(window, cx))),
                        )
                    })
                    // The original's round button, tinted with the accent.
                    .child(
                        div().size(px(40.)).flex().items_center().justify_center().rounded_full().bg(theme.colors().accent.opacity(0.18)).child(
                            IconButton::new("focus-play", if running && !paused { IconName::Pause } else { IconName::Play })
                                .icon_color(Color::Accent)
                                .tooltip(Tooltip::text(if running && !paused {
                                    strings.focus_pause
                                } else if paused {
                                    strings.focus_continue
                                } else {
                                    strings.focus_start
                                }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if running { this.toggle_pause(cx) } else { this.start(None, cx) }
                                })),
                        ),
                    ),
            );

        let record = self.render_record(strings, cx);
        // As in the original, the overview and records take 40% of the width.
        let right = v_flex()
            .w(gpui::relative(0.4))
            .flex_none()
            .h_full()
            .px_5()
            .child(h_flex().mt(crate::title_bar::header_inset(window, cx)).h(px(52.)).child(Label::new(strings.focus_overview).size(LabelSize::Large).weight(FontWeight::MEDIUM)))
            .child(self.render_overview(strings, cx))
            .child(self.render_record_header(true, cx))
            .when_some(self.message.clone(), |el, message| {
                el.child(Label::new(message).size(LabelSize::XSmall).color(Color::Muted))
            })
            .child(
                v_scroll(
                    "focus-record-scroll",
                    v_flex()
                        .id("focus-record")
                        .pr(px(14.))
                        .when(record.is_empty(), |el| {
                            el.child(
                                div()
                                    .w_full()
                                    .py_4()
                                    .child(EmptyState::new(IconName::Timer, strings.focus_no_record)),
                            )
                        })
                        .children(record),
                )
                .flex_1()
                .min_h_0(),
            );

        self.bind_focus_root(h_flex().relative().size_full(), cx)
            .items_start()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(left)
            .child(right)
            .children(self.render_record_dialog(cx))
            .children(self.render_modal(cx))
            .when(self.confirm_batch_delete, |el| {
                let strings = i18n::t(cx);
                el.child(
                    ConfirmDialog::new(
                        "focus-batch-delete",
                        strings.focus_batch_delete_title,
                        strings.cancel,
                        strings.delete,
                    )
                        .width(px(360.))
                        .description(strings.focus_batch_delete_message)
                        .destructive(true)
                        .on_cancel(cx.listener(|this, _, _, cx| {
                            this.confirm_batch_delete = false;
                            cx.notify();
                        }))
                        .on_confirm(cx.listener(|this, _, _, cx| {
                            this.confirm_batch_delete = false;
                            let ids: Vec<Id> = this.batch.drain().collect();
                            if let Err(err) = this.store.delete_focus_sessions(&ids) {
                                desktop_common::logging::log_error("focus_ui", format!("failed to delete the records: {err:#}"));
                            }
                            this.set_record_menu(None, cx);
                            this.reload(cx);
                            desktop_common::data_version::data_changed_scoped(
                            cx.entity_id(),
                            desktop_common::data_version::DataScope::FOCUS,
                            cx,
                        );
                        })),
                )
            })
            .when(self.confirm_delete_all, |el| el.child(self.render_confirm_delete_all(cx)))
            .when_some(self.confirm_delete_timer.clone(), |el, (id, name)| {
                let strings = i18n::t(cx);
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .when(cfg!(test), |el| {
                            el.debug_selector(|| "focus-timer-delete-modal".to_owned())
                        })
                        .child(
                            ConfirmDialog::new(
                                "focus-timer-delete",
                                strings.confirm_delete_timer,
                                strings.cancel,
                                strings.delete,
                            )
                                .width(px(340.))
                                .description(name)
                                .destructive(true)
                                .on_cancel(cx.listener(|this, _, _, cx| {
                                    this.confirm_delete_timer = None;
                                    cx.notify();
                                }))
                                .on_confirm(cx.listener(move |this, _, _, cx| {
                                    this.confirm_delete_timer = None;
                                    if let Err(err) = this.store.delete_focus_timer(id) {
                                        desktop_common::logging::log_error("focus_ui", format!("failed to delete the timer: {err:#}"));
                                    }
                                    this.reload(cx);
                                    desktop_common::data_version::data_changed_scoped(
                            cx.entity_id(),
                            desktop_common::data_version::DataScope::FOCUS,
                            cx,
                        );
                                })),
                        ),
                )
            })
            .when_some(self.confirm_delete_record.clone(), |el, (id, desc)| {
                el.child(self.render_confirm_delete_record(id, desc, cx))
            })
            .into_any_element()
    }
}
