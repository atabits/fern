//! The Focus (Pomodoro) section, matched to TickTick's: things to focus on on the left with a timer
//! bar at the bottom, an overview and the focus record on the right.

use std::collections::HashMap;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, NaiveDate};
#[cfg(debug_assertions)]
use fern_core::stats::Bucket;
use fern_core::{FocusSession, Id, Store, Task, View, focus_stats};
use gpui::{Entity, Focusable as _, FontWeight, ScrollHandle, Task as GpuiTask};
use ui::{Modal, Placement, PopoverAttachment, popover};
use ui::prelude::*;

use crate::focus_timer::{
    BreakKind, DEFAULT_LONG_BREAK_EVERY, FocusCycle, FocusTimer, MIN_FOCUS_MS, format_clock,
    format_stopwatch_clock, pomo_length_ms,
};
use crate::habit_view::{HABIT_EMOJI, icon_avatar};
use crate::i18n::{self, Strings};
use crate::text_input::{Mode, TextInput};
#[path = "focus_controls.rs"]
mod focus_controls;
#[path = "focus_review.rs"]
mod focus_review;
mod modals;
mod stats;

use modals::{RecordMenu, TimerDraft};
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
    /// What the Statistics page reads, loaded with the view rather than on every frame.
    stats_data: StatsData,
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
    /// "Delete All" asks for a second click.
    delete_all_armed: bool,
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
    active: bool,
    dirty: bool,
}

/// Cached picker data populated when the picker opens, reloads, or switches task view, so
/// `render_picker()` performs zero SQLite queries per frame.
struct PickerCache {
    lists: Vec<fern_core::List>,
    tasks: Vec<Task>,
    timers: Vec<fern_core::focus::FocusTimer>,
    habits: Vec<(fern_core::Habit, HashSet<NaiveDate>)>,
}

/// The focus page's picker, opened from "Focus >" or the eye over the hours: the original's
/// Task, Timer and Habit tabs over a search field.
struct Picker {
    search: Entity<TextInput>,
    /// Opened from the eye over the hours rather than from "Focus >".
    at_eye: bool,
    /// The Task tab's "Today >" list of views, while open.
    views_open: bool,
    cache: PickerCache,
}

/// What the focus page's Start focuses on.
#[derive(Clone)]
enum NextFocus {
    Task(Id, String),
    Timer(fern_core::focus::FocusTimer),
    Habit(Id, String),
}

impl NextFocus {
    fn title(&self) -> &str {
        match self {
            Self::Task(_, title) | Self::Habit(_, title) => title,
            Self::Timer(timer) => &timer.name,
        }
    }
}

impl gpui::Focusable for FocusView {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl FocusView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let add_title = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.focus_record_title, cx));
        let add_start = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.focus_record_start, cx));
        let settings = FocusSettings::load(&store);
        cx.observe_global::<desktop_common::data_version::DataVersion>(|this, cx| {
            if desktop_common::data_version::changed_elsewhere(cx.entity_id(), cx) {
                this.sync_external(cx);
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
            delete_all_armed: false,
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
            record_note: cx.new(|cx| TextInput::new(Mode::MultiLine, strings.focus_note_placeholder, cx)),
            active: true,
            dirty: false,
        };
        this.reload(cx);
        this
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if active && self.dirty {
            self.dirty = false;
            self.reload(cx);
        }
    }

    pub fn sync_external(&mut self, cx: &mut Context<Self>) {
        if self.active || self.running_clock().is_some() {
            self.reload(cx);
        } else {
            self.dirty = true;
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        let today = Local::now().date_naive();
        self.settings = FocusSettings::load(&self.store);
        self.tasks = self.store.tasks_in(&View::Today, today).unwrap_or_default();
        self.sessions = self.store.focus_sessions().unwrap_or_default();
        self.session_summary = SessionSummaryCache::build(&self.sessions, today);
        self.timers = self.store.focus_timers(self.archived_tab).unwrap_or_default();
        let habit_records = self.store.all_habit_checkins().unwrap_or_default();
        self.stats_data = StatsData {
            tasks: self.store.tasks().unwrap_or_default(),
            lists: self.store.lists().unwrap_or_default(),
            tags: self.store.tags().unwrap_or_default(),
            habits: self
                .store
                .habits()
                .unwrap_or_default()
                .into_iter()
                .map(|h| {
                    let done = habit_records
                        .get(&h.id)
                        .map(|recs| fern_core::habit::done_dates(recs))
                        .unwrap_or_default();
                    (h, done)
                })
                .collect(),
            habit_records,
        };
        if self.picker.is_some() {
            let cache = self.load_picker_cache(today);
            if let Some(picker) = self.picker.as_mut() {
                picker.cache = cache;
            }
        }
        self.sync_ambient_sound();
        cx.notify();
    }

    fn load_picker_cache(&self, today: NaiveDate) -> PickerCache {
        let tasks = if self.picker_view == View::Today {
            self.tasks.clone()
        } else {
            self.store.tasks_in(&self.picker_view, today).unwrap_or_default()
        };
        let timers = if !self.archived_tab {
            self.timers.clone()
        } else {
            self.store.focus_timers(false).unwrap_or_default()
        };
        PickerCache {
            lists: self.stats_data.lists.iter().filter(|l| !l.archived).cloned().collect(),
            tasks,
            timers,
            habits: self
                .stats_data
                .habits
                .iter()
                .filter(|(h, _)| !h.archived)
                .cloned()
                .collect(),
        }
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
            eprintln!("failed to save the focus setting: {err:#}");
        }
        cx.notify();
    }

    /// Starts a Pomodoro, or a stopwatch in the stopwatch mode.
    pub fn start(&mut self, task: Option<(Id, String)>, cx: &mut Context<Self>) {
        if self.finish_prompt { return; }
        self.finish(cx);
        if self.running.is_some() { return; }
        self.completed_in_chain = 0;
        let timer = if self.stopwatch_mode() {
            FocusTimer::stopwatch(now_ms())
        } else {
            FocusTimer::start(now_ms(), pomo_length_ms(self.minutes(POMO_SETTING)))
        };
        self.start_timer(
            task.map(|(id, title)| FocusTarget { task_id: Some(id), habit_id: None, title }),
            None,
            timer,
            cx,
        );
    }

    /// Starts a Pomodoro (`stopwatch == false`) or stopwatch (`stopwatch == true`) without a target task.
    pub fn start_mode(&mut self, stopwatch: bool, cx: &mut Context<Self>) {
        if self.finish_prompt { return; }
        self.finish(cx);
        if self.running.is_some() { return; }
        self.completed_in_chain = 0;
        self.set_stopwatch_mode(stopwatch, cx);
        let timer = if stopwatch {
            FocusTimer::stopwatch(now_ms())
        } else {
            FocusTimer::start(now_ms(), pomo_length_ms(self.minutes(POMO_SETTING)))
        };
        self.start_timer(None, None, timer, cx);
    }

    /// "Start Focus" on a habit: a Pomodoro or a stopwatch named after it, in that mode.
    pub fn start_named(&mut self, title: String, stopwatch: bool, cx: &mut Context<Self>) {
        self.start_for_habit(None, title, stopwatch, cx);
    }

    /// "Start Focus" on a specific habit, linking the recorded session so time-based goals auto check in.
    pub fn start_for_habit(&mut self, habit_id: Option<Id>, title: String, stopwatch: bool, cx: &mut Context<Self>) {
        if self.finish_prompt { return; }
        self.finish(cx);
        if self.running.is_some() { return; }
        self.completed_in_chain = 0;
        self.set_stopwatch_mode(stopwatch, cx);
        let timer = if stopwatch {
            FocusTimer::stopwatch(now_ms())
        } else {
            FocusTimer::start(now_ms(), pomo_length_ms(self.minutes(POMO_SETTING)))
        };
        self.start_timer(Some(FocusTarget { task_id: None, habit_id, title }), None, timer, cx);
    }

    /// "Start Focus → Start Pomo / Start Stopwatch" for a task, in that mode.
    pub fn start_for(&mut self, task: Option<Id>, title: String, stopwatch: bool, cx: &mut Context<Self>) {
        if self.finish_prompt { return; }
        self.finish(cx);
        if self.running.is_some() { return; }
        self.completed_in_chain = 0;
        self.set_stopwatch_mode(stopwatch, cx);
        let timer = if stopwatch {
            FocusTimer::stopwatch(now_ms())
        } else {
            FocusTimer::start(now_ms(), pomo_length_ms(self.minutes(POMO_SETTING)))
        };
        self.start_timer(Some(FocusTarget { task_id: task, habit_id: None, title }), None, timer, cx);
    }

    /// A timer's play button: a session named after it, with its own Pomo length or as a
    /// stopwatch.
    fn start_from_timer(&mut self, timer: &fern_core::focus::FocusTimer, cx: &mut Context<Self>) {
        if self.finish_prompt { return; }
        self.finish(cx);
        if self.running.is_some() { return; }
        self.completed_in_chain = 0;
        self.set_stopwatch_mode(timer.stopwatch, cx);
        let now = now_ms();
        let clock = match timer.length_ms() {
            Some(length) => FocusTimer::start(now, length),
            None => FocusTimer::stopwatch(now),
        };
        self.start_timer(
            Some(FocusTarget { task_id: None, habit_id: None, title: timer.name.clone() }),
            None,
            clock,
            cx,
        );
    }

    pub(super) fn auto_checkin_habit(&self, habit_id: Option<Id>, elapsed_ms: i64, cx: &mut Context<Self>) {
        let Some(habit_id) = habit_id else { return };
        let focused_minutes = elapsed_ms as f64 / 60_000.0;
        if let Ok(habits) = self.store.habits()
            && let Some(habit) = habits.iter().find(|h| h.id == habit_id)
            && let Some(delta) = fern_core::habit::focus_checkin_delta(habit, focused_minutes)
        {
            if let Err(err) = self.store.add_checkin_amount(habit_id, Local::now().date_naive(), delta) {
                eprintln!("failed to auto check in habit from focus: {err:#}");
            } else {
                desktop_common::data_version::data_changed(cx.entity_id(), cx);
            }
        }
    }

    fn stopwatch_mode(&self) -> bool {
        self.settings.stopwatch_mode
    }

    fn set_stopwatch_mode(&mut self, stopwatch: bool, cx: &mut Context<Self>) {
        self.settings.stopwatch_mode = stopwatch;
        if let Err(err) = self.store.set_setting(MODE_SETTING, if stopwatch { "stopwatch" } else { "pomo" }) {
            eprintln!("failed to save the focus mode: {err:#}");
        }
        cx.notify();
    }

    fn start_timer(&mut self, task: Option<FocusTarget>, on_break: Option<BreakKind>, timer: FocusTimer, cx: &mut Context<Self>) {
        if on_break.is_none() {
            self.cycle.start(timer.started_at, timer.is_stopwatch(), self.interval());
        }
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        });
        self.running = Some(Running { timer, task, on_break, _ticker: ticker });
        self.sync_ambient_sound();
        cx.notify();
    }

    /// A finished Pomodoro is recorded and followed by a break, as in the original; a finished
    /// break just ends.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.finish_prompt { return; }
        if self.running.as_ref().is_some_and(|r| r.timer.is_paused()) {
            return;
        }
        let now = now_ms();
        if self.stop_at_stopwatch_limit(now, cx) { return; }
        if self.running.as_ref().is_some_and(|r| !r.timer.is_paused() && r.timer.finished(now)) {
            let continuation = self.running.as_ref().filter(|r| r.on_break.is_none())
                .map(|r| (r.task.clone(), r.timer.length_ms));
            self.finish_at(now, true, cx);
            if self.running.is_some() { return; }
            desktop_common::sound::play_focus_done(&self.store);
            if let Some(continuation) = continuation {
                self.continuation = Some(continuation);
                self.completed_in_chain += 1;
                let kind = self.cycle.break_kind(now, self.interval());
                let setting = if kind == BreakKind::Long { LONG_BREAK_SETTING } else { SHORT_BREAK_SETTING };
                let length = self.minutes(setting) * 60_000;
                let mut timer = FocusTimer::start(now, length);
                if !self.automatic("focus_auto_break") { timer.pause(now); }
                self.start_timer(None, Some(kind), timer, cx);
            } else if self.automatic("focus_auto_next")
                && self.completed_in_chain < self.minutes(AUTO_COUNT_SETTING) as usize
                && let Some((task, length)) = self.continuation.take() {
                // `start_timer` increments `self.cycle` whenever `on_break.is_none()`.
                self.start_timer(task, None, FocusTimer::start(now, length), cx);
            } else {
                self.continuation = None;
            }
        }
        cx.notify();
    }

    pub(crate) fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// When a focus timer is running, returns its formatted clock string and paused flag for the
    /// sidebar badge.
    pub fn running_clock(&self) -> Option<(String, bool)> {
        let now = now_ms();
        self.running.as_ref().map(|r| (r.timer.format_display(now), r.timer.is_paused()))
    }

    pub(crate) fn white_noise(&self) -> desktop_common::sound::WhiteNoise {
        self.settings.white_noise
    }

    fn set_white_noise(&mut self, preset: desktop_common::sound::WhiteNoise, cx: &mut Context<Self>) {
        self.settings.white_noise = preset;
        if let Err(err) = desktop_common::sound::set_white_noise(&self.store, preset) {
            eprintln!("failed to save white noise setting: {err:#}");
        }
        self.sync_ambient_sound();
        cx.notify();
    }

    pub(crate) fn cycle_white_noise(&mut self, cx: &mut Context<Self>) {
        let next = self.settings.white_noise.next();
        self.set_white_noise(next, cx);
    }

    /// The clock as shown at `now`, what it is for (the task, the habit or the break), and
    /// whether a session runs and is paused; the timer bar, the page and the mini window show it.
    pub fn clock_state(&self, now: i64, strings: &'static i18n::Strings) -> (String, SharedString, bool, bool) {
        match &self.running {
            Some(r) => (
                r.timer.format_display(now),
                match r.on_break {
                    Some(BreakKind::Short) => SharedString::new_static(strings.focus_short_break),
                    Some(BreakKind::Long) => SharedString::new_static(strings.focus_long_break),
                    None => r.task.as_ref().map_or(SharedString::new_static(strings.focus), |t| t.title.clone().into()),
                },
                true,
                r.timer.is_paused(),
            ),
            None => (
                if self.stopwatch_mode() {
                    format_stopwatch_clock(0)
                } else {
                    format_clock(self.minutes(POMO_SETTING) * 60_000)
                },
                SharedString::new_static(strings.focus),
                false,
                false,
            ),
        }
    }

    /// Fraction of the current Pomodoro or break session elapsed (`0.0..=1.0`), or `None` for
    /// stopwatch mode or when idle.
    pub fn clock_progress(&self, now: i64) -> Option<f32> {
        let running = self.running.as_ref()?;
        if running.timer.is_stopwatch() {
            return None;
        }
        let length = running.timer.length_ms;
        if length <= 0 {
            return None;
        }
        Some((running.timer.elapsed(now) as f32 / length as f32).clamp(0.0, 1.0))
    }

    pub fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        if self.finish_prompt { return; }
        if self.stop_at_stopwatch_limit(now_ms(), cx) { return; }
        if let Some(running) = self.running.as_mut() {
            let now = now_ms();
            if running.timer.is_paused() { running.timer.resume(now) } else { running.timer.pause(now) }
        }
        cx.notify();
    }

    pub fn toggle_pause_or_start(&mut self, cx: &mut Context<Self>) {
        if self.running.is_some() {
            self.toggle_pause(cx);
        } else {
            self.start_next(cx);
        }
    }

    pub fn start_or_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            self.request_finish(window, cx);
        } else {
            self.start_next(cx);
        }
    }

    /// Stops the timer and records focus of at least five minutes (normal local capability).
    pub fn finish(&mut self, cx: &mut Context<Self>) {
        if self.finish_prompt { return; }
        let now = now_ms();
        self.stop_at_stopwatch_limit(now, cx);
        self.finish_at(now, false, cx);
        self.continuation = None;
    }

    fn finish_at(&mut self, now: i64, completed: bool, cx: &mut Context<Self>) {
        let Some(mut running) = self.running.take() else { return };
        let elapsed = running.timer.elapsed(now);
        if running.on_break.is_none() && elapsed >= MIN_FOCUS_MS {
            let strings = i18n::t(cx);
            let target = running.task.clone().unwrap_or(FocusTarget {
                task_id: None,
                habit_id: None,
                title: strings.focus.to_owned(),
            });
            let start = running.timer.started_at;
            let stopwatch = running.timer.is_stopwatch();
            if let Err(err) = self
                .store
                .add_focus_session_for(target.task_id, target.habit_id, &target.title, start, start + elapsed, stopwatch)
            {
                eprintln!("failed to save the focus session: {err:#}");
                self.message = Some(strings.focus_save_failed.to_owned());
                running.timer.pause(now);
                self.running = Some(running);
                cx.notify();
                return;
            }
            self.auto_checkin_habit(target.habit_id, elapsed, cx);
        }
        if running.on_break.is_none() {
            if completed && !running.timer.is_stopwatch() { self.cycle.complete(running.timer.completed_at()); }
            else { self.cycle.reset(); }
        }
        self.message = None;
        self.reload(cx);
    }

    /// A timer of the Pomodoro list: its icon, name, today's minutes and play; its "…" has Edit,
    /// Archive (or Restore) and Delete.
    fn render_timer(&self, timer: &fern_core::focus::FocusTimer, today_minutes: i64, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = timer.id;
        let played = timer.clone();
        let group = SharedString::from(format!("timer-{id}"));
        let menu_open = self.timer_menu == Some(id);
        let archived = timer.archived;
        let edited = timer.clone();
        h_flex()
            .id(group.clone())
            .group(group.clone())
            .h(px(40.))
            .px_2()
            .gap_2()
            .rounded(px(6.))
            .hover(|s| s.bg(theme.colors().ghost_element_hover))
            .child(icon_avatar(timer.icon.as_deref(), timer.color.as_deref(), &timer.name, px(26.), cx))
            .child(div().flex_1().min_w_0().child(Label::new(timer.name.clone()).truncate()))
            .child(Label::new(format_minutes(today_minutes, strings)).size(LabelSize::Small).color(Color::Muted))
            .child(
                div()
                    .relative()
                    .opacity(if menu_open || crate::touch::active() { 1. } else { 0. })
                    .group_hover(group, |s| s.opacity(1.))
                    .child(
                        IconButton::new(SharedString::from(format!("timer-more-{id}")), IconName::Ellipsis)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text(strings.more))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.timer_menu = if this.timer_menu == Some(id) { None } else { Some(id) };
                                cx.notify();
                            })),
                    )
                    .when(menu_open, |el| {
                        el.child(PopoverAttachment::new(
                            Placement::BelowRight,
                            popover(cx)
                                .min_w(px(160.))
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.timer_menu = None;
                                    cx.notify();
                                }))
                                .child(
                                    ListItem::new("timer-edit")
                                        .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                                        .child(Label::new(strings.edit))
                                        .on_click(cx.listener(move |this, _, window, cx| this.open_timer(Some(edited.clone()), window, cx))),
                                )
                                .child(
                                    ListItem::new("timer-archive")
                                        .start_slot(Icon::new(if archived { IconName::ArchiveRestore } else { IconName::Archive }).color(Color::Muted))
                                        .child(Label::new(if archived { strings.habit_unarchive } else { strings.habit_archive }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Err(err) = this.store.set_focus_timer_archived(id, !archived) {
                                                eprintln!("failed to archive the timer: {err:#}");
                                            }
                                            this.timer_menu = None;
                                            this.reload(cx);
                                        })),
                                )
                                .child(
                                    ListItem::new("timer-delete")
                                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                                        .child(Label::new(strings.delete).color(Color::Error))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Err(err) = this.store.delete_focus_timer(id) {
                                                eprintln!("failed to delete the timer: {err:#}");
                                            }
                                            this.timer_menu = None;
                                            this.reload(cx);
                                        })),
                                )
                                .into_any_element(),
                        ))
                    }),
            )
            .when(!archived, |el| {
                el.child(
                    IconButton::new(SharedString::from(format!("timer-play-{id}")), IconName::Play)
                        .size(px(22.))
                        .icon_size(IconSize::Small)
                        .icon_color(Color::Accent)
                        .tooltip(Tooltip::text(strings.focus_start))
                        .on_click(cx.listener(move |this, _, _, cx| this.start_from_timer(&played, cx))),
                )
            })
            .into_any_element()
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
        if self.stats.as_mut().is_some_and(|stats| {
            stats.curve_menu.take().is_some()
                || std::mem::take(&mut stats.task_menu)
                || std::mem::take(&mut stats.classify_menu)
                || stats.details_menu.take().is_some()
        }) {
            cx.notify();
            return true;
        }
        if std::mem::take(&mut self.confirm_batch_delete) {
            cx.notify();
            return true;
        }
        if self.batch_custom.take().is_some() || self.batch_menu.take().is_some() {
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
            self.delete_all_armed = false;
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
                                IconButton::new("record-more", IconName::Ellipsis)
                                    .tooltip(Tooltip::text(strings.more))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_record_menu(Some(RecordMenu::More), cx)
                                    })),
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
                                    crate::calendar::format_time(start, strings),
                                    crate::calendar::format_time(session.end_local(), strings)
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
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.delete_record(session_id, cx);
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
    fn set_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.expanded = expanded;
        self.record_menu = None;
        self.picker = None;
        cx.notify();
    }

    fn open_picker(&mut self, at_eye: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.as_ref().is_some_and(|p| p.at_eye == at_eye) {
            self.picker = None;
            cx.notify();
            return;
        }
        let strings = i18n::t(cx);
        let search = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.focus_picker_search, cx));
        cx.subscribe(&search, |_, _, _: &crate::text_input::Changed, cx| cx.notify()).detach();
        cx.subscribe(&search, |this, _, _: &crate::text_input::Cancelled, cx| {
            this.picker = None;
            cx.notify();
        })
        .detach();
        window.focus(&search.focus_handle(cx), cx);
        let cache = self.load_picker_cache(Local::now().date_naive());
        self.picker = Some(Picker { search, at_eye, views_open: false, cache });
        cx.notify();
    }

    fn pick(&mut self, next: NextFocus, cx: &mut Context<Self>) {
        self.next = Some(next);
        self.picker = None;
        cx.notify();
    }

    /// The page's Start: the picked task, timer or habit, or a plain session.
    pub fn start_next(&mut self, cx: &mut Context<Self>) {
        match self.next.take() {
            Some(NextFocus::Task(id, title)) => self.start(Some((id, title)), cx),
            Some(NextFocus::Timer(timer)) => self.start_from_timer(&timer, cx),
            Some(NextFocus::Habit(id, name)) => {
                let stopwatch = self.stopwatch_mode();
                self.start_for_habit(Some(id), name, stopwatch, cx)
            }
            None => self.start(None, cx),
        }
    }

    /// The picker's list: today's tasks and due habits, the active timers, or the active habits,
    /// narrowed by the search. Reads exclusively from `picker.cache` (zero SQLite queries).
    fn render_picker(&self, picker: &Picker, cx: &mut Context<Self>) -> AnyElement {
        use fern_core::focus::matches_search;
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let query = picker.search.read(cx).text().to_owned();
        let today = Local::now().date_naive();
        let habits: Vec<(&fern_core::Habit, &HashSet<NaiveDate>)> = picker
            .cache
            .habits
            .iter()
            .filter(|(h, _)| matches_search(&h.name, &query))
            .map(|(h, done)| (h, done))
            .collect();
        // As in the original, the picked one has a filled circle and stays lit.
        let picked = self.next.as_ref().map(|n| n.title().to_owned());
        let item = |id: SharedString, title: String, next: NextFocus, cx: &mut Context<Self>| {
            let on = picked.as_deref() == Some(next.title());
            ListItem::new(id)
                .toggle_state(on)
                .start_slot(
                    Icon::new(if on { IconName::CircleDot } else { IconName::Circle })
                        .size(IconSize::Small)
                        .color(if on { Color::Accent } else { Color::Muted }),
                )
                .child(Label::new(title).size(LabelSize::Small).truncate())
                .on_click(cx.listener(move |this, _, _, cx| this.pick(next.clone(), cx)))
                .into_any_element()
        };
        let mut items: Vec<AnyElement> = Vec::new();
        // The views the Task tab can list: the smart lists, the Inbox and the lists.
        let views: Vec<(View, SharedString)> = [
            (View::Today, SharedString::new_static(strings.today)),
            (View::Tomorrow, SharedString::new_static(strings.tomorrow)),
            (View::Next7Days, SharedString::new_static(strings.next_7_days)),
            (View::Inbox, SharedString::new_static(strings.inbox)),
        ]
        .into_iter()
        .chain(picker.cache.lists.iter().map(|l| (View::List(l.id), l.name.clone().into())))
        .collect();
        let view_name = views.iter().find(|(v, _)| *v == self.picker_view).map_or(SharedString::new_static(strings.today), |(_, n)| n.clone());
        match self.picker_tab {
            0 if picker.views_open => {
                for (i, (view, name)) in views.into_iter().enumerate() {
                    let on = view == self.picker_view;
                    items.push(
                        ListItem::new(("focus-picker-view", i))
                            .toggle_state(on)
                            .child(Label::new(name).size(LabelSize::Small).color(if on { Color::Accent } else { Color::Default }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.picker_view = view.clone();
                                let tasks = if this.picker_view == View::Today {
                                    this.tasks.clone()
                                } else {
                                    this.store.tasks_in(&this.picker_view, Local::now().date_naive()).unwrap_or_default()
                                };
                                if let Some(picker) = this.picker.as_mut() {
                                    picker.views_open = false;
                                    picker.cache.tasks = tasks;
                                }
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            0 => {
                items.push(
                    h_flex()
                        .id("focus-picker-views")
                        .gap_1()
                        .px_2()
                        .py_1()
                        .cursor_pointer()
                        .child(Icon::new(IconName::CalendarDays).size(IconSize::Small).color(Color::Muted))
                        .child(Label::new(view_name).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                        .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(picker) = this.picker.as_mut() {
                                picker.views_open = true;
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                for task in picker.cache.tasks.iter().filter(|t| matches_search(&t.title, &query)) {
                    items.push(item(format!("pick-task-{}", task.id).into(), task.title.clone(), NextFocus::Task(task.id, task.title.clone()), cx));
                }
                // As in the original, the habits due today follow Today's tasks.
                let week_start = crate::calendar::week_start();
                let due: Vec<&fern_core::Habit> = habits
                    .iter()
                    .filter(|(h, done)| self.picker_view == View::Today && h.is_due_on(today, done, week_start))
                    .map(|(h, _)| *h)
                    .collect();
                if !due.is_empty() {
                    items.push(div().px_2().pt_1().child(Label::new(strings.focus_picker_tabs[2]).size(LabelSize::XSmall).color(Color::Muted)).into_any_element());
                }
                for habit in due {
                    items.push(item(format!("pick-habit-{}", habit.id).into(), habit.name.clone(), NextFocus::Habit(habit.id, habit.name.clone()), cx));
                }
            }
            1 => {
                for timer in picker.cache.timers.iter().filter(|t| matches_search(&t.name, &query)) {
                    items.push(item(format!("pick-timer-{}", timer.id).into(), timer.name.clone(), NextFocus::Timer(timer.clone()), cx));
                }
            }
            _ => {
                for (habit, _) in &habits {
                    items.push(item(format!("pick-habit-{}", habit.id).into(), habit.name.clone(), NextFocus::Habit(habit.id, habit.name.clone()), cx));
                }
            }
        }
        let tabs = h_flex().justify_center().gap_1().children(strings.focus_picker_tabs.iter().enumerate().map(|(i, label)| {
            let on = self.picker_tab == i;
            div()
                .id(("focus-picker-tab", i))
                .px_3()
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(12.))
                .cursor_pointer()
                .when(on, |el| el.bg(theme.colors().accent.opacity(0.2)))
                .child(Label::new(*label).size(LabelSize::Small).color(if on { Color::Accent } else { Color::Muted }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.picker_tab = i;
                    if let Some(picker) = this.picker.as_mut() {
                        picker.views_open = false;
                    }
                    cx.notify();
                }))
        }));
        popover(cx)
            .w(px(300.))
            .h(px(420.))
            .p_2()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.picker = None;
                cx.notify();
            }))
            .child(tabs)
            .child(
                h_flex()
                    .h(px(30.))
                    .px_2()
                    .gap_2()
                    .rounded(px(6.))
                    .bg(theme.colors().element_hover)
                    .child(Icon::new(IconName::Search).size(IconSize::Small).color(Color::Muted))
                    .child(div().flex_1().child(picker.search.clone())),
            )
            .child(
                v_scroll(
                    ("focus-picker-items-scroll", self.picker_tab * 2 + usize::from(picker.views_open)),
                    v_flex()
                        .id(("focus-picker-items", self.picker_tab * 2 + usize::from(picker.views_open)))
                        .children(items),
                )
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }

    /// Pomo or Stopwatch, chosen while no timer runs.
    fn render_mode_switch(&self, size: LabelSize, cx: &mut Context<Self>) -> AnyElement {
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

    /// The original's full focus page: the arrow back, Pomo / Stopwatch and "…" on top, "Focus >"
    /// over a big ring with the time and Start (or Pause / Continue and End), and on the right
    /// the day's hours with its sessions and a red line at the time now.
    fn render_page(&self, clock: String, label: SharedString, running: bool, paused: bool, cx: &mut Context<Self>) -> AnyElement {
        // The original's measures: an hour is 213 px, the ring 350 px, the panel 490 px wide.
        const HOUR: f32 = 213.;
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let now = Local::now();
        let (hour, minute) = (chrono::Timelike::hour(&now), chrono::Timelike::minute(&now));
        let pill = |id: &'static str, text: &'static str, primary: bool| {
            div()
                .id(id)
                .w(px(167.))
                .h(px(47.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(text).when(primary, |label| label.color(Color::OnAccent)))
        };
        // The picked task, timer or habit names the session until it starts.
        let label: SharedString = match (&self.next, running) {
            (Some(next), false) => next.title().to_owned().into(),
            _ => label,
        };
        let picker = self.picker.as_ref().map(|p| (p.at_eye, self.render_picker(p, cx)));
        let (picker_here, picker_eye) = match picker {
            Some((false, el)) => (Some(PopoverAttachment::new(Placement::BelowCenter, el)), None),
            Some((true, el)) => (None, Some(PopoverAttachment::new(Placement::BelowRight, el))),
            None => (None, None),
        };
        let buttons = if running {
            h_flex()
                .gap_3()
                .child(pill("focus-page-pause", if paused { strings.focus_continue } else { strings.focus_pause }, true).on_click(cx.listener(|this, _, _, cx| this.toggle_pause(cx))))
                .child(pill("focus-page-end", strings.focus_end, false).on_click(cx.listener(|this, _, window, cx| this.request_finish(window, cx))))
        } else {
            h_flex().child(pill("focus-page-start", strings.focus_start, true).on_click(cx.listener(|this, _, _, cx| this.start_next(cx))))
        };
        let left = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .border_r_1()
            .border_color(theme.colors().border)
            .child(
                h_flex()
                    .mt(crate::title_bar::header_inset())
                    .h(px(52.))
                    .flex_none()
                    .px_4()
                    .child(
                        div().flex_1().child(
                            IconButton::new("focus-page-collapse", IconName::ChevronDown)
                                .icon_color(Color::Muted)
                                .tooltip(ui::Tooltip::text(strings.focus_collapse))
                                .on_click(cx.listener(|this, _, _, cx| this.set_expanded(false, cx))),
                        ),
                    )
                    .when(!running, |el| el.child(self.render_mode_switch(LabelSize::Small, cx)))
                    .child(
                        h_flex().flex_1().justify_end().gap_1()
                        .child(self.render_white_noise_button("focus-page-white-noise", cx))
                        // The original's mini window: the clock in a small window over the others.
                        .child(
                            IconButton::new("focus-page-mini", IconName::PictureInPicture2)
                                .icon_color(Color::Muted)
                                .tooltip(ui::Tooltip::text(strings.focus_mini_window))
                                // Deferred: the window draws at once and reads this view.
                                .on_click(cx.listener(|_, _, _, cx| {
                                    let focus = cx.entity();
                                    cx.defer(move |cx| crate::focus_mini::open(focus, cx));
                                })),
                        )
                        .child(
                            div()
                                .relative()
                                .child(
                                    IconButton::new("focus-page-more", IconName::Ellipsis)
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
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .relative()
                            .mb(px(110.))
                            .child(
                                h_flex()
                                    .id("focus-page-pick")
                                    .gap_0p5()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(6.))
                                    .when(!running, |el| {
                                        el.cursor_pointer()
                                            .hover(|s| s.bg(theme.colors().ghost_element_hover))
                                            .on_click(cx.listener(|this, _, window, cx| this.open_picker(false, window, cx)))
                                    })
                                    .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
                                    .when(!running, |el| el.child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(Color::Muted))),
                            )
                            .children(picker_here),
                    )
                    .child(
                        div()
                            .size(px(350.))
                            .mb(px(120.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .border(px(5.))
                            .border_color(if running { theme.colors().accent } else { theme.colors().border })
                            .child(div().text_size(px(56.)).child(clock)),
                    )
                    .child(buttons),
            );

        // The hours from the one before now to midnight, as the original opens on now.
        let first = hour.saturating_sub(1);
        let today = now.date_naive();
        let top = |minute: u32| px((minute as f32 / 60. - first as f32) * HOUR);
        let hours = div()
            .relative()
            .h(px((24 - first) as f32 * HOUR))
            .children((first..24).map(|hour| {
                h_flex()
                    .absolute()
                    .top(top(hour * 60))
                    .left_0()
                    .right_0()
                    .gap_2()
                    .child(div().w(px(24.)).child(Label::new(hour.to_string()).size(LabelSize::XSmall).color(Color::Muted)))
                    .child(div().flex_1().h(px(1.)).bg(theme.colors().border_variant))
            }))
            .children(fern_core::focus::day_blocks(&self.sessions, today).into_iter().filter(|(_, to, _)| *to > first * 60).enumerate().map(|(i, (from, to, session))| {
                let dur = format_minutes(to.saturating_sub(from) as i64, strings);
                let tip = format!("{} · {:02}:{:02}–{:02}:{:02} ({dur})", session.title, from / 60, from % 60, to / 60, to % 60);
                div()
                    .id(SharedString::from(format!("focus-page-block-{i}")))
                    .absolute()
                    .top(top(from.max(first * 60)))
                    .left(px(32.))
                    .right_0()
                    .h(px(((to - from.max(first * 60)) as f32 / 60. * HOUR).max(4.)))
                    .px_2()
                    .rounded(px(4.))
                    .bg(theme.colors().accent.opacity(0.25))
                    .hover(|s| s.bg(theme.colors().accent.opacity(0.38)))
                    .border_l_2()
                    .border_color(theme.colors().accent)
                    .overflow_hidden()
                    .tooltip(Tooltip::text(tip))
                    .child(Label::new(session.title.clone()).size(LabelSize::XSmall))
            }))
            .child(
                h_flex()
                    .absolute()
                    .top(top(hour * 60 + minute) - px(3.))
                    .left(px(28.))
                    .right_0()
                    .child(div().size(px(6.)).rounded_full().bg(theme.status().error))
                    .child(div().flex_1().h(px(1.)).bg(theme.status().error)),
            );
        let stopwatch = self.stopwatch_mode();
        let right = v_flex()
            .w(px(490.))
            .flex_none()
            .h_full()
            .px_4()
            .child(
                h_flex()
                    .mt(crate::title_bar::header_inset())
                    .h(px(52.))
                    .gap_2()
                    .child(Icon::new(if stopwatch { IconName::Timer } else { IconName::FernFocus }).color(Color::Accent))
                    .child(div().flex_1().child(Label::new(strings.focus_modes[stopwatch as usize]).size(LabelSize::Large).color(Color::Muted)))
                    .child(
                        div()
                            .relative()
                            .child(
                                IconButton::new("focus-page-eye", IconName::Eye)
                                    .icon_color(Color::Muted)
                                    .tooltip(Tooltip::text(strings.focus))
                                    .on_click(cx.listener(|this, _, window, cx| this.open_picker(true, window, cx))),
                            )
                            .children(picker_eye),
                    ),
            )
            .child(v_scroll("focus-page-hours-scroll", v_flex().id("focus-page-hours").pt_2().child(hours)).flex_1().min_h_0());
        self.bind_focus_root(h_flex().relative().size_full(), cx)
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(left)
            .child(right)
            .into_any_element()
    }

    fn sync_ambient_sound(&mut self) {
        let active = self
            .running
            .as_ref()
            .is_some_and(|r| !r.timer.is_paused() && r.on_break.is_none());
        let desired = (active, self.settings.white_noise);
        if desired != self.last_ambient {
            self.last_ambient = desired;
            desktop_common::sound::sync_white_noise(active, self.settings.white_noise);
        }
    }
}

impl Drop for FocusView {
    fn drop(&mut self) {
        desktop_common::sound::stop_white_noise();
    }
}

impl Render for FocusView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_ambient_sound();
        if let Some(page) = self.stats {
            return self.render_stats(page, cx);
        }
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let now = now_ms();

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

        let (clock, label, running, paused) = self.clock_state(now, strings);

        if self.expanded {
            return self.render_page(clock, label, running, paused, cx);
        }
        let left = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .border_r_1()
            .border_color(theme.colors().border)
            .child(
                h_flex()
                    .relative()
                    .mt(crate::title_bar::header_inset())
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
            .child(h_flex().mt(crate::title_bar::header_inset()).h(px(52.)).child(Label::new(strings.focus_overview).size(LabelSize::Large).weight(FontWeight::MEDIUM)))
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
                    ConfirmDialog::new("focus-batch-delete", strings.focus_batch_delete_title)
                        .width(px(360.))
                        .description(strings.focus_batch_delete_message)
                        .cancel_label(strings.cancel)
                        .confirm_label(strings.delete)
                        .destructive(true)
                        .on_cancel(cx.listener(|this, _, _, cx| {
                            this.confirm_batch_delete = false;
                            cx.notify();
                        }))
                        .on_confirm(cx.listener(|this, _, _, cx| {
                            this.confirm_batch_delete = false;
                            let ids: Vec<Id> = this.batch.drain().collect();
                            if let Err(err) = this.store.delete_focus_sessions(&ids) {
                                eprintln!("failed to delete the records: {err:#}");
                            }
                            this.set_record_menu(None, cx);
                            this.reload(cx);
                            desktop_common::data_version::data_changed(cx.entity_id(), cx);
                        })),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::stats::summary_parts;

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn statistics_scrolls_below_its_header_and_resets_on_tab_change(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, MouseButton, ScrollDelta, ScrollWheelEvent, TouchPhase, point, size};

        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        });
        let store = Rc::new(Store::open_in_memory().unwrap());
        let (_, cx) = cx.add_window_view(|_, cx| {
            let mut view = FocusView::new(store, cx);
            view.open_stats_overview(cx);
            view
        });
        let viewport = cx.debug_bounds("statistics-scroll").unwrap();
        let header = cx.debug_bounds("statistics-header").unwrap();
        let content = cx.debug_bounds("statistics-content").unwrap();
        assert!(content.size.height > viewport.size.height);

        let position = point(viewport.right() - px(9.), viewport.center().y);
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(position, None, modifiers);
        cx.simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), px(-180.))),
            modifiers,
            touch_phase: TouchPhase::Moved,
        });

        assert!(cx.debug_bounds("statistics-content").unwrap().top() < content.top());
        assert_eq!(cx.debug_bounds("statistics-header"), Some(header));

        let scrollbar_end = point(viewport.right() - px(9.), viewport.bottom() - px(6.));
        cx.simulate_mouse_move(scrollbar_end, None, modifiers);
        cx.simulate_click(scrollbar_end, modifiers);
        assert!((cx.debug_bounds("statistics-content").unwrap().bottom() - viewport.bottom()).abs() < px(1.));

        let drag_end = point(scrollbar_end.x, viewport.top() - px(8.));
        cx.simulate_mouse_down(scrollbar_end, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(drag_end, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(drag_end, MouseButton::Left, modifiers);
        assert_eq!(cx.debug_bounds("statistics-content").unwrap().top(), content.top());
        cx.simulate_click(scrollbar_end, modifiers);

        let task_tab = cx.debug_bounds("statistics-tab-1").unwrap().center();
        cx.simulate_mouse_move(task_tab, None, modifiers);
        cx.simulate_click(task_tab, modifiers);
        assert_eq!(cx.debug_bounds("statistics-content").unwrap().top(), content.top());

        cx.simulate_resize(size(px(1280.), px(1800.)));
        let viewport = cx.debug_bounds("statistics-scroll").unwrap();
        let content = cx.debug_bounds("statistics-content").unwrap();
        assert!(content.size.height < viewport.size.height);
        cx.simulate_click(point(viewport.right() - px(9.), viewport.bottom() - px(6.)), modifiers);
        assert_eq!(cx.debug_bounds("statistics-content"), Some(content));
    }

    #[test]
    fn summary_counts_stand_apart() {
        let counts = [("{tasks}", "14".to_owned()), ("{days}", "3".to_owned())];
        let parts = summary_parts("{tasks} Tasks   {days} Days", &counts);
        assert_eq!(parts, [("14".into(), true), (" Tasks   ".into(), false), ("3".into(), true), (" Days".into(), false)]);
        let ru = summary_parts("Задач: {tasks}", &counts);
        assert_eq!(ru, [("Задач: ".into(), false), ("14".into(), true)]);
    }

    #[test]
    fn resolve_local_datetime_handles_single_ambiguous_and_none() {
        use chrono::{FixedOffset, LocalResult, TimeZone};
        let tz_early = FixedOffset::east_opt(2 * 3600).unwrap();
        let tz_late = FixedOffset::east_opt(3600).unwrap();
        let early = tz_early.with_ymd_and_hms(2026, 10, 25, 2, 30, 0).single().unwrap();
        let late = tz_late.with_ymd_and_hms(2026, 10, 25, 2, 30, 0).single().unwrap();

        assert_eq!(resolve_local_datetime(LocalResult::Single(early)), Some(early.timestamp_millis()));
        assert_eq!(
            resolve_local_datetime(LocalResult::Ambiguous(early, late)),
            Some(early.timestamp_millis())
        );
        assert_eq!(resolve_local_datetime::<FixedOffset>(LocalResult::None), None);
    }

    #[gpui::test]
    fn clock_progress_is_none_for_stopwatch_and_bounded_for_pomo(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        cx.update(|cx| {
            let store = Rc::new(Store::open_in_memory().unwrap());
            let view = cx.new(|cx| FocusView::new(store.clone(), cx));
            view.update(cx, |view, cx| {
                // Idle -> None
                assert_eq!(view.clock_progress(now_ms()), None);

                // Stopwatch -> None (never renders a dead 0% progress bar)
                view.start_mode(true, cx);
                let start = view.running.as_ref().unwrap().timer.started_at;
                assert_eq!(view.clock_progress(start + 60_000), None);
                assert_eq!(view.clock_state(start + 500, i18n::t(cx)).0, "00:00");

                // Pomodoro -> Some(0.0..=1.0)
                view.finish(cx);
                view.start_mode(false, cx);
                let timer = view.running.as_ref().unwrap().timer;
                let progress = view.clock_progress(timer.started_at + timer.length_ms / 2).unwrap();
                assert!((progress - 0.5).abs() < 0.01, "expected ~0.5, got {progress}");
            });
        });
    }

    #[gpui::test]
    fn white_noise_is_cached_and_refreshed_on_cycle_and_data_version(cx: &mut gpui::TestAppContext) {
        use desktop_common::sound::{WhiteNoise, set_white_noise};
        use gpui::AppContext;
        let store = Rc::new(Store::open_in_memory().unwrap());
        let (view, other) = cx.update(|cx| {
            let view = cx.new(|cx| FocusView::new(store.clone(), cx));
            let other = cx.new(|_| ());
            view.update(cx, |view, cx| {
                assert_eq!(view.white_noise(), WhiteNoise::Off);
                view.cycle_white_noise(cx);
                assert_eq!(view.white_noise(), WhiteNoise::Ticking);
            });
            // Direct DB write without DataVersion leaves cache untouched (proving render/getter
            // does not query SQLite).
            set_white_noise(&store, WhiteNoise::Off).unwrap();
            assert_eq!(view.read(cx).white_noise(), WhiteNoise::Ticking);
            // Emitting DataVersion from another entity refreshes the cached white noise preset
            // once GPUI flushes global observer effects.
            desktop_common::data_version::data_changed(other.entity_id(), cx);
            (view, other)
        });
        let _ = other;
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(view.read(cx).white_noise(), WhiteNoise::Off);
        });
    }
}
