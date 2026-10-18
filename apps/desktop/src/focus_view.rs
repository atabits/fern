//! The Focus (Pomodoro) section, matched to TickTick's: things to focus on on the left with a timer
//! bar at the bottom, an overview and the focus record on the right.

use std::collections::HashMap;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, NaiveDate};
use fern_core::stats::{self, Bucket};
use fern_core::{FocusSession, Id, Store, Task, View, focus_stats, format_minutes};
use gpui::{Entity, Focusable as _, FontWeight, Task as GpuiTask};
use ui::{Donut, Placement, PopoverAttachment, popover};
use ui::prelude::*;

use crate::focus_timer::{BreakKind, FocusTimer, break_after, format_clock};
use crate::habit_view::{HABIT_EMOJI, icon_avatar};
use crate::i18n::{self, Strings};
use crate::text_input::{Mode, TextInput};

/// Timer lengths in minutes, from the Focus Settings: (setting, default, minimum, maximum).
const POMO_SETTING: (&str, i64, i64, i64) = ("focus_pomo_minutes", 25, 5, 180);
const SHORT_BREAK_SETTING: (&str, i64, i64, i64) = ("focus_short_break_minutes", 5, 1, 60);
const LONG_BREAK_SETTING: (&str, i64, i64, i64) = ("focus_long_break_minutes", 15, 1, 90);
/// "pomo" or "stopwatch".
const MODE_SETTING: &str = "focus_mode";
/// Shorter sessions are not recorded.
const MIN_RECORDED_MS: i64 = 60_000;

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

struct Running {
    timer: FocusTimer,
    /// What the session is for: a task, or a name alone (a habit).
    task: Option<(Option<Id>, String)>,
    /// A break after a finished Pomodoro; breaks are not recorded.
    on_break: Option<BreakKind>,
    _ticker: GpuiTask<()>,
}

pub struct FocusView {
    store: Rc<Store>,
    tasks: Vec<Task>,
    sessions: Vec<FocusSession>,
    running: Option<Running>,
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
    /// The focus record whose dialog is open, and its note field.
    open_record: Option<Id>,
    record_note: Entity<TextInput>,
}

/// What the Statistics page shows: the Trends week, the Most Focused Time month and the year.
#[derive(Clone, Copy)]
struct StatsPage {
    tab: usize,
    week: NaiveDate,
    month: NaiveDate,
    year: i32,
    /// The Task tab's day.
    day: NaiveDate,
    /// The Overview's curves (completion, rate, pomos, focus): Day, Week or Month points.
    curves: [Bucket; 4],
    /// The curve whose period menu is open.
    curve_menu: Option<usize>,
    /// The Task tab's Daily / Weekly / Monthly report, and whether its menu is open.
    task_period: Bucket,
    task_menu: bool,
    /// Classified Completion Statistics by tag instead of by list, and whether its menu is open.
    by_tag: bool,
    classify_menu: bool,
    /// The Focus tab's "Details": split by list (0), tag (1) or task (2), its period and day,
    /// and which of its two menus is open.
    details_by: u8,
    details_period: Bucket,
    details_day: NaiveDate,
    details_menu: Option<u8>,
    /// The first day of the Timeline's week.
    timeline_week: NaiveDate,
}

/// Bars for the statistics charts, with a label under each; `highlight` marks one bar.
fn bar_chart(values: &[i64], labels: Vec<String>, highlight: Option<usize>, height: f32, cx: &App) -> Div {
    let colors = cx.theme().colors();
    let top = values.iter().copied().max().unwrap_or(0).max(1) as f32;
    v_flex()
        .gap_1()
        .child(h_flex().h(px(height)).items_end().gap_1().children(values.iter().enumerate().map(|(i, v)| {
            div()
                .flex_1()
                .h(px((height * *v as f32 / top).max(if *v > 0 { 2. } else { 0. })))
                .rounded_t(px(3.))
                .bg(if highlight == Some(i) { colors.accent } else { colors.accent.opacity(0.6) })
        })))
        .child(h_flex().gap_1().children(labels.into_iter().map(|l| {
            div().flex_1().flex().justify_center().child(Label::new(l).size(LabelSize::XSmall).color(Color::Muted))
        })))
}

/// A smooth line with a shaded area under it, scaled between the values' minimum and maximum,
/// as in the original's "My Achievement Score".
/// `from_zero` starts the scale at zero (counts); otherwise at the lowest value (the score).
fn line_chart(values: &[i64], labels: Vec<String>, height: f32, from_zero: bool, cx: &App) -> Div {
    let colors = cx.theme().colors();
    let (accent, dot) = (colors.accent, colors.text);
    let low = if from_zero { 0 } else { values.iter().copied().min().unwrap_or(0) };
    let high = values.iter().copied().max().unwrap_or(0).max(low + 1);
    let values = values.to_vec();
    let axis = |v: i64| Label::new(v.to_string()).size(LabelSize::XSmall).color(Color::Muted);
    let plot = gpui::canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let n = values.len().max(1) as f32;
            let (left, top) = (bounds.origin.x, bounds.origin.y);
            let (width, chart) = (bounds.size.width, bounds.size.height - px(8.));
            let points: Vec<gpui::Point<Pixels>> = values
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let share = (*v - low) as f32 / (high - low) as f32;
                    gpui::point(left + width * ((i as f32 + 0.5) / n), top + px(4.) + chart * (1. - share))
                })
                .collect();
            let Some((first, last)) = points.first().zip(points.last()) else { return };
            let curve = |builder: &mut gpui::PathBuilder| {
                for pair in points.windows(2) {
                    let middle = (pair[0].x + pair[1].x) / 2.;
                    builder.cubic_bezier_to(pair[1], gpui::point(middle, pair[0].y), gpui::point(middle, pair[1].y));
                }
            };
            let bottom = top + bounds.size.height;
            let mut area = gpui::PathBuilder::fill();
            area.move_to(gpui::point(first.x, bottom));
            area.line_to(*first);
            curve(&mut area);
            area.line_to(gpui::point(last.x, bottom));
            area.close();
            if let Ok(path) = area.build() {
                window.paint_path(path, accent.opacity(0.3));
            }
            let mut line = gpui::PathBuilder::stroke(px(2.));
            line.move_to(*first);
            curve(&mut line);
            if let Ok(path) = line.build() {
                window.paint_path(path, accent);
            }
            for p in &points {
                let size = px(6.);
                window.paint_quad(
                    gpui::fill(gpui::Bounds::new(gpui::point(p.x - size / 2., p.y - size / 2.), gpui::size(size, size)), dot)
                        .corner_radii(size / 2.),
                );
            }
        },
    )
    .w_full()
    .h(px(height));
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .items_start()
                .gap_2()
                .child(v_flex().h(px(height)).justify_between().child(axis(high)).child(axis(low)))
                .child(div().flex_1().child(plot)),
        )
        .child(h_flex().pl(px(36.)).children(labels.into_iter().map(|l| {
            div().flex_1().flex().justify_center().child(Label::new(l).size(LabelSize::XSmall).color(Color::Muted))
        })))
}

/// The focus page's picker, opened from "Focus >" or the eye over the hours: the original's
/// Task, Timer and Habit tabs over a search field.
struct Picker {
    search: Entity<TextInput>,
    /// Opened from the eye over the hours rather than from "Focus >".
    at_eye: bool,
    /// The Task tab's "Today >" list of views, while open.
    views_open: bool,
}

/// What the focus page's Start focuses on.
#[derive(Clone)]
enum NextFocus {
    Task(Id, String),
    Timer(fern_core::focus::FocusTimer),
    Habit(String),
}

impl NextFocus {
    fn title(&self) -> &str {
        match self {
            Self::Task(_, title) | Self::Habit(title) => title,
            Self::Timer(timer) => &timer.name,
        }
    }
}

/// The Focus Record's "+" form and "…" menu, as in the original.
/// The "Add Timer" popover: the timer it edits (or a new one), its name, Pomo length and mode.
struct TimerDraft {
    timer: fern_core::focus::FocusTimer,
    name: Entity<TextInput>,
    new: bool,
    /// The icon picker under the name, while open: 0 Emoji, 1 Text (the name's initial).
    icon_tab: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RecordMenu {
    Add,
    More,
    /// The Focus Record's "Batch Management": records to tick and delete together.
    Batch,
    /// The Pomodoro header's "…" and its Focus Settings.
    HeaderMore,
    Settings,
}

impl FocusView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let add_title = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.focus_record_title, cx));
        let add_start = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.focus_record_start, cx));
        let mut this = Self {
            store,
            tasks: Vec::new(),
            sessions: Vec::new(),
            running: None,
            record_menu: None,
            timers: Vec::new(),
            archived_tab: false,
            timer_draft: None,
            timer_menu: None,
            delete_all_armed: false,
            batch: HashSet::new(),
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
            open_record: None,
            record_note: cx.new(|cx| TextInput::new(Mode::MultiLine, strings.focus_note_placeholder, cx)),
        };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        self.tasks = self.store.tasks_in(&View::Today, today).unwrap_or_default();
        self.sessions = self.store.focus_sessions().unwrap_or_default();
        self.timers = self.store.focus_timers(self.archived_tab).unwrap_or_default();
        cx.notify();
    }

    fn minutes(&self, (key, default, min, max): (&str, i64, i64, i64)) -> i64 {
        self.store.setting(key).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(default).clamp(min, max)
    }

    fn set_minutes(&mut self, (key, default, min, max): (&str, i64, i64, i64), delta: i64, cx: &mut Context<Self>) {
        let value = (self.minutes((key, default, min, max)) + delta).clamp(min, max);
        if let Err(err) = self.store.set_setting(key, &value.to_string()) {
            eprintln!("failed to save the focus setting: {err:#}");
        }
        cx.notify();
    }

    /// Starts a Pomodoro, or a stopwatch in the stopwatch mode.
    pub fn start(&mut self, task: Option<(Id, String)>, cx: &mut Context<Self>) {
        self.finish(cx);
        let timer = if self.stopwatch_mode() {
            FocusTimer::stopwatch(now_ms())
        } else {
            FocusTimer::start(now_ms(), self.minutes(POMO_SETTING) * 60_000)
        };
        self.start_timer(task.map(|(id, title)| (Some(id), title)), None, timer, cx);
    }

    /// "Start Focus" on a habit: a Pomodoro or a stopwatch named after it, in that mode.
    pub fn start_named(&mut self, title: String, stopwatch: bool, cx: &mut Context<Self>) {
        self.start_for(None, title, stopwatch, cx);
    }

    /// "Start Focus → Start Pomo / Start Stopwatch" for a task or a habit, in that mode.
    pub fn start_for(&mut self, task: Option<Id>, title: String, stopwatch: bool, cx: &mut Context<Self>) {
        self.finish(cx);
        self.set_stopwatch_mode(stopwatch, cx);
        let timer = if stopwatch {
            FocusTimer::stopwatch(now_ms())
        } else {
            FocusTimer::start(now_ms(), self.minutes(POMO_SETTING) * 60_000)
        };
        self.start_timer(Some((task, title)), None, timer, cx);
    }

    /// A timer's play button: a session named after it, with its own Pomo length or as a
    /// stopwatch.
    fn start_from_timer(&mut self, timer: &fern_core::focus::FocusTimer, cx: &mut Context<Self>) {
        self.finish(cx);
        self.set_stopwatch_mode(timer.stopwatch, cx);
        let clock = match timer.length_ms() {
            Some(length) => FocusTimer::start(now_ms(), length),
            None => FocusTimer::stopwatch(now_ms()),
        };
        self.start_timer(Some((None, timer.name.clone())), None, clock, cx);
    }

    fn stopwatch_mode(&self) -> bool {
        self.store.setting(MODE_SETTING).ok().flatten().as_deref() == Some("stopwatch")
    }

    fn set_stopwatch_mode(&mut self, stopwatch: bool, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_setting(MODE_SETTING, if stopwatch { "stopwatch" } else { "pomo" }) {
            eprintln!("failed to save the focus mode: {err:#}");
        }
        cx.notify();
    }

    fn start_timer(&mut self, task: Option<(Option<Id>, String)>, on_break: Option<BreakKind>, timer: FocusTimer, cx: &mut Context<Self>) {
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        });
        self.running = Some(Running { timer, task, on_break, _ticker: ticker });
        cx.notify();
    }

    /// A finished Pomodoro is recorded and followed by a break, as in the original; a finished
    /// break just ends.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.running.as_ref().is_some_and(|r| r.timer.finished(now_ms())) {
            let was_focus = self.running.as_ref().is_some_and(|r| r.on_break.is_none());
            self.finish(cx);
            if was_focus {
                let today = Local::now().date_naive();
                let pomos = self.sessions.iter().filter(|s| s.start_local().date_naive() == today).count();
                let kind = break_after(pomos);
                let setting = if kind == BreakKind::Long { LONG_BREAK_SETTING } else { SHORT_BREAK_SETTING };
                let length = self.minutes(setting) * 60_000;
                self.start_timer(None, Some(kind), FocusTimer::start(now_ms(), length), cx);
            }
        }
        cx.notify();
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        if let Some(running) = self.running.as_mut() {
            let now = now_ms();
            if running.timer.is_paused() { running.timer.resume(now) } else { running.timer.pause(now) }
        }
        cx.notify();
    }

    /// Stops the timer and records the session if it lasted at least a minute.
    fn finish(&mut self, cx: &mut Context<Self>) {
        let Some(running) = self.running.take() else { return };
        let now = now_ms();
        let elapsed = running.timer.elapsed(now);
        if running.on_break.is_none() && elapsed >= MIN_RECORDED_MS {
            let strings = i18n::t(cx);
            let (task_id, title) = running.task.map_or((None, strings.focus.to_owned()), |(id, t)| (id, t));
            let start = running.timer.started_at;
            let stopwatch = running.timer.is_stopwatch();
            if let Err(err) = self.store.add_focus_session_as(task_id, &title, start, start + elapsed, stopwatch) {
                eprintln!("failed to record the focus session: {err:#}");
            }
        }
        self.reload(cx);
    }

    fn set_record_menu(&mut self, menu: Option<RecordMenu>, cx: &mut Context<Self>) {
        self.record_menu = menu;
        self.delete_all_armed = false;
        cx.notify();
    }

    /// Saves the "+" form: a session today from the typed start time for the chosen minutes.
    fn add_record(&mut self, cx: &mut Context<Self>) {
        let title = self.add_title.read(cx).text().trim().to_owned();
        let start = crate::due_picker::parse_time(self.add_start.read(cx).text());
        let (Some(start), false) = (start, title.is_empty()) else { return };
        let today = Local::now().date_naive();
        let started = today.and_time(start).and_local_timezone(Local).single().map(|t| t.timestamp_millis());
        if let Some(started) = started {
            let ended = started + self.add_minutes * 60_000;
            if let Err(err) = self.store.add_focus_session(None, &title, started, ended) {
                eprintln!("failed to add the focus record: {err:#}");
            }
        }
        self.add_title.update(cx, |input, cx| input.set_text("", cx));
        self.add_start.update(cx, |input, cx| input.set_text("", cx));
        self.record_menu = None;
        self.reload(cx);
    }

    /// Opens a focus record's dialog, its note in the field.
    fn open_record(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let note = self.sessions.iter().find(|s| s.id == id).map(|s| s.note.clone()).unwrap_or_default();
        self.record_note.update(cx, |input, cx| input.set_text(note, cx));
        window.focus(&self.record_note.focus_handle(cx), cx);
        self.open_record = Some(id);
        cx.notify();
    }

    /// Closes the dialog, keeping what was written in the note.
    fn close_record(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.open_record.take() {
            let note = self.record_note.read(cx).text().to_owned();
            if let Err(err) = self.store.set_focus_note(id, &note) {
                eprintln!("failed to save the focus note: {err:#}");
            }
            self.reload(cx);
        }
        cx.notify();
    }

    /// The original's Focus Record dialog: the record, its time and length, and a note.
    fn render_record_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.open_record?;
        let session = self.sessions.iter().find(|s| s.id == id)?;
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let start = session.start_local();
        let when = format!(
            "{}, {} - {}",
            crate::task_rows::short_date(start.date_naive(), strings),
            crate::calendar::format_time(start),
            crate::calendar::format_time(session.end_local())
        );
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    v_flex()
                        .w(px(420.))
                        .p_4()
                        .gap_2()
                        .rounded(px(10.))
                        .bg(theme.colors().elevated_surface_background)
                        .border_1()
                        .border_color(theme.colors().border)
                        .shadow_lg()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_record(cx)))
                        .child(
                            h_flex()
                                .child(div().flex_1().child(Label::new(strings.focus_record).weight(FontWeight::SEMIBOLD)))
                                .child(IconButton::new("close-record", IconName::X).on_click(cx.listener(|this, _, _, cx| this.close_record(cx)))),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Icon::new(if session.stopwatch { IconName::Timer } else { IconName::CircleDot }).color(Color::Accent),
                                )
                                .child(Label::new(session.title.clone()).truncate()),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Icon::new(IconName::CalendarDays).size(IconSize::Small).color(Color::Muted))
                                .child(Label::new(when).size(LabelSize::Small).color(Color::Muted)),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Icon::new(IconName::Clock).size(IconSize::Small).color(Color::Muted))
                                .child(Label::new(format_minutes(session.minutes())).size(LabelSize::Small).color(Color::Muted)),
                        )
                        .child(
                            div()
                                .min_h(px(110.))
                                .p_2()
                                .rounded(px(6.))
                                .bg(theme.colors().surface_background)
                                .child(self.record_note.clone()),
                        )
                        .child(
                            h_flex().justify_end().child(
                                IconButton::new("delete-open-record", IconName::Trash).on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_record = None;
                                    this.delete_record(id, cx);
                                })),
                            ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn delete_record(&mut self, id: fern_core::Id, cx: &mut Context<Self>) {
        if let Err(err) = self.store.delete_focus_session(id) {
            eprintln!("failed to delete the focus record: {err:#}");
        }
        self.reload(cx);
    }

    fn delete_all(&mut self, cx: &mut Context<Self>) {
        if !self.delete_all_armed {
            self.delete_all_armed = true;
            cx.notify();
            return;
        }
        if let Err(err) = self.store.delete_all_focus_sessions() {
            eprintln!("failed to delete the focus records: {err:#}");
        }
        self.set_record_menu(None, cx);
        self.reload(cx);
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        let name = format!("fern-focus-{}.csv", Local::now().format("%Y-%m-%d"));
        let csv = fern_core::focus::export_csv(&self.sessions).into_bytes();
        crate::export::save_as(name, csv, cx, |this, result, cx| {
            if let Some(message) = crate::export::outcome(result, i18n::t(cx)) {
                this.message = Some(message);
            }
            cx.notify();
        });
        self.set_record_menu(None, cx);
    }

    /// A filter's Custom choice: Start and End days for Date, Min and Max minutes for Duration.
    fn open_batch_custom(&mut self, which: u8, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let (first, second) = if which == 0 { (strings.summary_start, strings.summary_end) } else { (strings.batch_min, strings.batch_max) };
        let field = |placeholder: &'static str, cx: &mut Context<Self>| {
            let input = cx.new(|cx| TextInput::new(Mode::SingleLine, placeholder, cx));
            cx.subscribe(&input, |this, _, _: &crate::text_input::Submitted, cx| this.confirm_batch_custom(cx)).detach();
            input
        };
        let (first, second) = (field(first, cx), field(second, cx));
        window.focus(&first.focus_handle(cx), cx);
        self.batch_menu = None;
        self.batch_custom = Some((which, first, second));
        cx.notify();
    }

    /// Confirm (or OK): applies the Custom fields when they read as a range; otherwise they stay
    /// open.
    fn confirm_batch_custom(&mut self, cx: &mut Context<Self>) {
        let Some((which, first, second)) = self.batch_custom.clone() else { return };
        let (first, second) = (first.read(cx).text().to_owned(), second.read(cx).text().to_owned());
        if which == 0 {
            let today = Local::now().date_naive();
            let strings = i18n::t(cx);
            let parse = |text: &str| crate::calendar::parse_date(text, today, strings);
            let (Some(start), Some(end)) = (parse(&first), parse(&second)) else { return };
            self.batch_filter.date = 7;
            self.batch_filter.days = Some((start.min(end), start.max(end)));
        } else {
            let Some(range) = fern_core::focus::RecordFilter::parse_minutes(&first, &second) else { return };
            self.batch_filter.minutes = Some(range);
            self.batch_filter.under = None;
        }
        self.batch_custom = None;
        cx.notify();
    }

    fn render_batch_custom(&self, which: u8, first: Entity<TextInput>, second: Entity<TextInput>, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let field = |input: Entity<TextInput>| h_flex().flex_1().h(px(28.)).px_2().rounded(px(6.)).border_1().border_color(theme.colors().border).child(input);
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small))
        };
        let fields = if which == 0 {
            v_flex().gap_2().child(field(first)).child(field(second))
        } else {
            v_flex()
                .gap_2()
                .child(Label::new(strings.batch_minutes_hint).size(LabelSize::XSmall).color(Color::Muted))
                .child(h_flex().gap_1().child(field(first)).child(Label::new("–").color(Color::Muted)).child(field(second)))
        };
        PopoverAttachment::new(
            Placement::BelowLeft,
            popover(cx)
                .w(px(if which == 0 { 190. } else { 230. }))
                .p_2()
                .gap_2()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.batch_custom = None;
                    cx.notify();
                }))
                .child(fields)
                .child(
                    h_flex()
                        .gap_2()
                        .child(button("batch-custom-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| {
                            this.batch_custom = None;
                            cx.notify();
                        })))
                        .child(
                            button("batch-custom-ok", if which == 0 { strings.confirm } else { strings.ok }, true)
                                .on_click(cx.listener(|this, _, _, cx| this.confirm_batch_custom(cx))),
                        ),
                )
                .into_any_element(),
        )
        .into_any_element()
    }

    /// The original's Batch Management: the records by day, each and each day with a checkbox,
    /// "Select All", and Delete for the ticked ones.
    /// Batch Management or Add Timer, as the original shows them: in the middle of the view
    /// over a dimmed background.
    fn render_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = if let Some(draft) = self.timer_draft.as_ref() {
            self.render_timer_draft(draft, cx)
        } else if self.record_menu == Some(RecordMenu::Batch) {
            self.render_batch(cx)
        } else {
            return None;
        };
        Some(
            div()
                .id("focus-modal")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.35))
                .occlude()
                .child(dialog)
                .into_any_element(),
        )
    }

    fn render_batch(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let today = chrono::Local::now().date_naive();
        let filter = self.batch_filter;
        let shown: Vec<&FocusSession> =
            self.sessions.iter().filter(|s| filter.matches(s, today, crate::calendar::week_start())).collect();
        let mut days: Vec<(chrono::NaiveDate, Vec<&FocusSession>)> = Vec::new();
        for session in shown.iter().copied() {
            let day = session.start_local().date_naive();
            match days.last_mut() {
                Some((d, list)) if *d == day => list.push(session),
                _ => days.push((day, vec![session])),
            }
        }
        let all: Vec<Id> = shown.iter().map(|s| s.id).collect();
        // The original's Date, Duration and Linked Status filters, each a dropdown.
        const LENGTHS: [Option<i64>; 5] = [None, Some(10), Some(25), Some(45), Some(60)];
        const LINKED: [Option<bool>; 3] = [None, Some(true), Some(false)];
        let current = [
            filter.date as usize,
            if filter.minutes.is_some() { LENGTHS.len() } else { LENGTHS.iter().position(|l| *l == filter.under).unwrap_or(0) },
            LINKED.iter().position(|l| *l == filter.linked).unwrap_or(0),
        ];
        let choices: [&[&'static str]; 3] = [&strings.batch_dates, &strings.batch_lengths, &strings.batch_linked];
        let choices_len = move |which: u8| choices[which as usize].len();
        let theme = cx.theme().clone();
        let filters = h_flex().gap_1().children((0..3u8).map(|which| {
            let open = self.batch_menu == Some(which);
            let label: SharedString = match (which, filter.days, filter.minutes) {
                (0, Some((first, last)), _) if filter.date == 7 => {
                    format!("{} - {}", crate::task_rows::short_date(first, strings), crate::task_rows::short_date(last, strings)).into()
                }
                (1, _, Some((least, most))) => format!("{}–{}", format_minutes(least), format_minutes(most)).into(),
                _ if current[which as usize] == 0 => strings.batch_filters[which as usize].into(),
                _ => choices[which as usize][current[which as usize]].into(),
            };
            let custom = self.batch_custom.clone().filter(|(w, ..)| *w == which).map(|(_, first, second)| self.render_batch_custom(which, first, second, cx));
            div()
                .relative()
                .child(
                    h_flex()
                        .id(SharedString::from(format!("batch-filter-{which}")))
                        .px_2()
                        .h(px(22.))
                        .gap_1()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme.colors().border)
                        .cursor_pointer()
                        .child(Label::new(label).size(LabelSize::XSmall).color(if current[which as usize] == 0 { Color::Muted } else { Color::Accent }))
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.batch_menu = if this.batch_menu == Some(which) { None } else { Some(which) };
                            cx.notify();
                        })),
                )
                .when(open, |el| {
                    el.child(PopoverAttachment::new(
                        Placement::BelowLeft,
                        popover(cx)
                            .w(px(170.))
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.batch_menu = None;
                                cx.notify();
                            }))
                            .children(choices[which as usize].iter().enumerate().map(|(i, choice)| {
                                let selected = current[which as usize] == i;
                                ListItem::new(SharedString::from(format!("batch-filter-{which}-{i}")))
                                    .child(Label::new(*choice).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default }))
                                    .when(selected, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        match which {
                                            // The last choice of Date and Duration is Custom.
                                            0 | 1 if i == choices_len(which) - 1 => {
                                                this.open_batch_custom(which, window, cx);
                                                return;
                                            }
                                            0 => {
                                                this.batch_filter.date = i as u8;
                                                this.batch_filter.days = None;
                                            }
                                            1 => {
                                                this.batch_filter.under = LENGTHS[i];
                                                this.batch_filter.minutes = None;
                                            }
                                            _ => this.batch_filter.linked = LINKED[i],
                                        }
                                        this.batch_menu = None;
                                        cx.notify();
                                    }))
                            }))
                            .into_any_element(),
                    ))
                })
                .children(custom)
        }));
        let chosen = self.batch.len();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (day, sessions) in days {
            let ids: Vec<Id> = sessions.iter().map(|s| s.id).collect();
            let whole = ids.iter().all(|id| self.batch.contains(id));
            rows.push(
                h_flex()
                    .id(SharedString::from(format!("batch-day-{day}")))
                    .gap_2()
                    .py_1()
                    .cursor_pointer()
                    .child(Checkbox::new(SharedString::from(format!("batch-day-box-{day}"))).size(px(14.)).toggle_state(whole))
                    .child(Label::new(crate::task_rows::short_date(day, strings)).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        for id in &ids {
                            if whole {
                                this.batch.remove(id);
                            } else {
                                this.batch.insert(*id);
                            }
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            for session in sessions {
                let id = session.id;
                let on = self.batch.contains(&id);
                let span = format!(
                    "{} - {}",
                    crate::calendar::format_time(session.start_local().time()),
                    crate::calendar::format_time(session.end_local().time())
                );
                rows.push(
                    h_flex()
                        .id(SharedString::from(format!("batch-{id}")))
                        .gap_2()
                        .py_1()
                        .items_start()
                        .cursor_pointer()
                        .child(Checkbox::new(SharedString::from(format!("batch-box-{id}"))).size(px(14.)).toggle_state(on))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(Label::new(span).size(LabelSize::XSmall).color(Color::Muted))
                                .child(Label::new(session.title.clone()).size(LabelSize::Small).truncate()),
                        )
                        .child(Label::new(fern_core::format_minutes(session.minutes())).size(LabelSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.batch.remove(&id) {
                                this.batch.insert(id);
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        // A dialog in the middle of the window, closed with its ×, as in the original.
        popover(cx)
            .w(px(440.))
            .max_h(px(560.))
            .p_4()
            .gap_2()
            .child(
                h_flex()
                    .child(
                        div().flex_1().child(
                            Label::new(if chosen == 0 {
                                strings.batch_management.to_owned()
                            } else {
                                strings.n_selected.replace("{n}", &chosen.to_string())
                            })
                            .weight(FontWeight::SEMIBOLD),
                        ),
                    )
                    .child(IconButton::new("batch-close", IconName::X).size(px(22.)).icon_size(IconSize::Small).on_click(
                        cx.listener(|this, _, _, cx| this.set_record_menu(None, cx)),
                    )),
            )
            .child(filters)
            .child(v_flex().id("batch-list").flex_1().min_h_0().overflow_y_scroll().children(rows))
            .child(
                h_flex()
                    .child(
                        div()
                            .id("batch-select-all")
                            .cursor_pointer()
                            .child(Label::new(strings.select_all).size(LabelSize::Small).color(Color::Accent))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.batch = all.iter().copied().collect();
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .when(chosen > 0, |el| {
                        el.child(
                            div()
                                .id("batch-delete")
                                .cursor_pointer()
                                .child(Label::new(strings.delete).size(LabelSize::Small).color(Color::Error))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let ids: Vec<Id> = this.batch.drain().collect();
                                    if let Err(err) = this.store.delete_focus_sessions(&ids) {
                                        eprintln!("failed to delete the records: {err:#}");
                                    }
                                    this.set_record_menu(None, cx);
                                    this.reload(cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    /// "+" in the Pomodoro header, or Edit in a timer's menu: the original's "Add Timer".
    fn open_timer(&mut self, timer: Option<fern_core::focus::FocusTimer>, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let new = timer.is_none();
        let timer = timer.unwrap_or_else(|| fern_core::focus::FocusTimer::new(""));
        let name = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.timer_name, cx);
            input.set_text(timer.name.clone(), cx);
            input
        });
        cx.subscribe(&name, |this, _, _: &crate::text_input::Submitted, cx| this.save_timer(cx)).detach();
        window.focus(&name.focus_handle(cx), cx);
        self.timer_menu = None;
        self.timer_draft = Some(TimerDraft { timer, name, new, icon_tab: None });
        cx.notify();
    }

    fn save_timer(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.timer_draft.take() else { return };
        let mut timer = draft.timer;
        timer.name = draft.name.read(cx).text().trim().to_owned();
        if let Err(err) = self.store.save_focus_timer(&timer) {
            eprintln!("failed to save the timer: {err:#}");
            if !timer.name.is_empty() {
                return;
            }
        }
        self.reload(cx);
    }

    fn render_timer_draft(&self, draft: &TimerDraft, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let timer = &draft.timer;
        let icons = draft.icon_tab.map(|tab| self.render_timer_icons(timer, tab, cx));
        let avatar = icon_avatar(timer.icon.as_deref(), timer.color.as_deref(), draft.name.read(cx).text(), px(30.), cx);
        let change = |f: fn(&mut fern_core::focus::FocusTimer)| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                if let Some(draft) = this.timer_draft.as_mut() {
                    f(&mut draft.timer);
                }
                cx.notify();
            })
        };
        let radio = |on: bool| {
            div()
                .size(px(14.))
                .rounded_full()
                .border_1()
                .border_color(if on { theme.colors().accent } else { theme.colors().text_muted })
                .when(on, |el| el.bg(theme.colors().accent))
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small))
        };
        // A dialog in the middle of the window, closed with Close, as in the original.
        popover(cx)
            .w(px(440.))
            .p_4()
            .gap_3()
            .child(Label::new(if draft.new { strings.add_timer } else { strings.edit_timer }).weight(FontWeight::SEMIBOLD))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .id("timer-icon-button")
                            .cursor_pointer()
                            .child(avatar)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(draft) = this.timer_draft.as_mut() {
                                    draft.icon_tab = if draft.icon_tab.is_some() { None } else { Some(0) };
                                }
                                cx.notify();
                            })),
                    )
                    .child(h_flex().flex_1().h(px(30.)).px_2().rounded(px(6.)).bg(theme.colors().background).child(draft.name.clone())),
            )
            .children(icons)
            .child(Label::new(strings.timer_mode).size(LabelSize::Small).color(Color::Muted))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .id("timer-pomo")
                            .gap_2()
                            .cursor_pointer()
                            .child(radio(!timer.stopwatch))
                            .child(Label::new(strings.focus_modes[0]).size(LabelSize::Small))
                            .on_click(change(|t| t.stopwatch = false)),
                    )
                    .child(IconButton::new("timer-less", IconName::Minus).size(px(22.)).icon_size(IconSize::XSmall).on_click(change(|t| t.minutes = (t.minutes - 5).max(5))))
                    .child(Label::new(timer.minutes.to_string()).size(LabelSize::Small))
                    .child(IconButton::new("timer-more", IconName::Plus).size(px(22.)).icon_size(IconSize::XSmall).on_click(change(|t| t.minutes = (t.minutes + 5).min(180))))
                    .child(Label::new(strings.minutes).size(LabelSize::XSmall).color(Color::Muted)),
            )
            .child(
                h_flex()
                    .id("timer-stopwatch")
                    .gap_2()
                    .cursor_pointer()
                    .child(radio(timer.stopwatch))
                    .child(Label::new(strings.focus_modes[1]).size(LabelSize::Small))
                    .on_click(change(|t| t.stopwatch = true)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(button("timer-close", strings.close, false).on_click(cx.listener(|this, _, _, cx| {
                        this.timer_draft = None;
                        cx.notify();
                    })))
                    .child(button("timer-save", strings.save, true).on_click(cx.listener(|this, _, _, cx| this.save_timer(cx)))),
            )
            .into_any_element()
    }

    /// The timer's icon picker, like a habit's: an emoji or the name's initial ("Text"), on a
    /// color.
    fn render_timer_icons(&self, timer: &fern_core::focus::FocusTimer, tab: usize, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let edit = |f: Box<dyn Fn(&mut TimerDraft)>| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                if let Some(draft) = this.timer_draft.as_mut() {
                    f(draft);
                }
                cx.notify();
            })
        };
        let tabs = h_flex().p_0p5().rounded(px(6.)).bg(theme.colors().background).children(
            strings.habit_icon_tabs.iter().enumerate().map(|(i, label)| {
                div()
                    .id(("timer-icon-tab", i))
                    .flex_1()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(i == tab, |el| el.bg(theme.colors().element_selected))
                    .child(Label::new(*label).size(LabelSize::Small))
                    .on_click(edit(Box::new(move |d| {
                        d.icon_tab = Some(i);
                        // "Text" shows the name's initial.
                        if i == 1 {
                            d.timer.icon = None;
                        }
                    })))
            }),
        );
        let emoji = div().flex().flex_wrap().gap_0p5().children(HABIT_EMOJI.iter().map(|emoji| {
            let selected = timer.icon.as_deref() == Some(*emoji);
            div()
                .id(SharedString::from(format!("timer-emoji-{emoji}")))
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(17.))
                .when(selected, |el| el.bg(theme.colors().element_selected))
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(*emoji)
                .on_click(edit(Box::new(move |d| {
                    d.timer.icon = Some((*emoji).to_owned());
                    d.icon_tab = None;
                })))
        }));
        let colors = h_flex().gap_1p5().children(
            std::iter::once(None).chain(crate::lists_panel::LIST_COLORS.iter().map(|c| Some(*c))).map(|color| {
                let selected = timer.color.as_deref() == color;
                let fill = crate::lists_panel::parse_color(color).unwrap_or(theme.colors().accent);
                div()
                    .id(SharedString::from(format!("timer-color-{}", color.unwrap_or("none"))))
                    .size(px(20.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(fill.opacity(if color.is_none() { 0.35 } else { 1. }))
                    .when(selected, |el| el.border_2().border_color(theme.colors().text))
                    .on_click(edit(Box::new(move |d| d.timer.color = color.map(str::to_owned))))
            }),
        );
        v_flex()
            .gap_2()
            .p_2()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().border)
            .child(tabs)
            .when(tab == 0, |el| el.child(emoji))
            .child(colors)
            .into_any_element()
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
            .child(Label::new(format_minutes(today_minutes)).size(LabelSize::Small).color(Color::Muted))
            .child(
                div()
                    .relative()
                    .opacity(if menu_open { 1. } else { 0. })
                    .group_hover(group, |s| s.opacity(1.))
                    .child(IconButton::new("timer-more", IconName::Ellipsis).size(px(22.)).icon_size(IconSize::Small).on_click(cx.listener(move |this, _, _, cx| {
                        this.timer_menu = if this.timer_menu == Some(id) { None } else { Some(id) };
                        cx.notify();
                    })))
                    .when(menu_open, |el| {
                        el.child(PopoverAttachment::new(
                            Placement::BelowRight,
                            popover(cx)
                                .w(px(160.))
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
                    IconButton::new("timer-play", IconName::Play)
                        .size(px(22.))
                        .icon_size(IconSize::Small)
                        .icon_color(Color::Accent)
                        .on_click(cx.listener(move |this, _, _, cx| this.start_from_timer(&played, cx))),
                )
            })
            .into_any_element()
    }

    fn render_record_menu(&self, menu: RecordMenu, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let close = cx.listener(|this, _, _, cx| this.set_record_menu(None, cx));
        match menu {
            RecordMenu::HeaderMore => popover(cx)
                .w(px(200.))
                .on_mouse_down_out(close)
                .child(
                    ListItem::new("focus-statistics")
                        .start_slot(Icon::new(IconName::ChartNoAxesColumn).color(Color::Muted))
                        .child(Label::new(strings.statistics))
                        .on_click(cx.listener(|this, _, _, cx| this.open_stats(cx))),
                )
                .child(
                    ListItem::new("focus-settings")
                        .start_slot(Icon::new(IconName::Settings).color(Color::Muted))
                        .child(Label::new(strings.focus_settings))
                        .on_click(cx.listener(|this, _, _, cx| this.set_record_menu(Some(RecordMenu::Settings), cx))),
                )
                .into_any_element(),
            RecordMenu::Settings => {
                let row = |id: &'static str, label: &'static str, setting: (&'static str, i64, i64, i64), cx: &mut Context<Self>| {
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(Label::new(label).size(LabelSize::Small)))
                        .child(
                            IconButton::new(SharedString::from(format!("{id}-less")), IconName::Minus)
                                .on_click(cx.listener(move |this, _, _, cx| this.set_minutes(setting, -5, cx))),
                        )
                        .child(div().min_w(px(28.)).flex().justify_center().child(Label::new(self.minutes(setting).to_string())))
                        .child(
                            IconButton::new(SharedString::from(format!("{id}-more")), IconName::Plus)
                                .on_click(cx.listener(move |this, _, _, cx| this.set_minutes(setting, 5, cx))),
                        )
                        .child(Label::new(strings.minutes).size(LabelSize::XSmall).color(Color::Muted))
                };
                popover(cx)
                    .w(px(340.))
                    .p_3()
                    .gap_2()
                    .on_mouse_down_out(close)
                    .child(Label::new(strings.focus_settings).weight(FontWeight::SEMIBOLD))
                    .child(Label::new(strings.focus_timer_option).size(LabelSize::Small).color(Color::Muted))
                    .child(row("pomo", strings.focus_pomo_duration, POMO_SETTING, cx))
                    .child(row("short-break", strings.focus_short_break, SHORT_BREAK_SETTING, cx))
                    .child(row("long-break", strings.focus_long_break, LONG_BREAK_SETTING, cx))
                    .into_any_element()
            }
            RecordMenu::Batch => self.render_batch(cx),
            RecordMenu::More => popover(cx)
                .w(px(240.))
                .on_mouse_down_out(close)
                .child(
                    ListItem::new("focus-batch")
                        .start_slot(Icon::new(IconName::ListChecks).color(Color::Muted))
                        .child(Label::new(strings.batch_management))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.batch.clear();
                            this.batch_filter = Default::default();
                            this.set_record_menu(Some(RecordMenu::Batch), cx)
                        })),
                )
                .child(
                    ListItem::new("focus-export")
                        .start_slot(Icon::new(IconName::Share).color(Color::Muted))
                        .child(Label::new(strings.export))
                        .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
                )
                .child(
                    ListItem::new("focus-delete-all")
                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                        .child(
                            Label::new(if self.delete_all_armed { strings.delete_all_confirm } else { strings.delete_all })
                                .color(Color::Error),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.delete_all(cx))),
                )
                .into_any_element(),
            RecordMenu::Add => {
                let field = |input: Entity<TextInput>| {
                    h_flex().h(px(30.)).px_2().rounded(px(6.)).bg(theme.colors().background).child(input)
                };
                popover(cx)
                    .w(px(280.))
                    .p_3()
                    .gap_2()
                    .on_mouse_down_out(close)
                    .child(Label::new(strings.focus_add_record).weight(FontWeight::SEMIBOLD))
                    .child(field(self.add_title.clone()))
                    .child(field(self.add_start.clone()))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Label::new(strings.focus_record_minutes).size(LabelSize::Small)))
                            .child(IconButton::new("record-shorter", IconName::Minus).on_click(cx.listener(|this, _, _, cx| {
                                this.add_minutes = (this.add_minutes - 5).max(5);
                                cx.notify();
                            })))
                            .child(Label::new(format_minutes(self.add_minutes)).size(LabelSize::Small))
                            .child(IconButton::new("record-longer", IconName::Plus).on_click(cx.listener(|this, _, _, cx| {
                                this.add_minutes = (this.add_minutes + 5).min(12 * 60);
                                cx.notify();
                            }))),
                    )
                    .child(
                        div()
                            .id("record-save")
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(theme.colors().accent)
                            .cursor_pointer()
                            .child(Label::new(strings.save).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| this.add_record(cx))),
                    )
                    .into_any_element()
            }
        }
    }

    fn open_stats(&mut self, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        self.record_menu = None;
        self.stats = Some(StatsPage {
            tab: 2,
            day: today,
            week: crate::calendar::week_of(today)[0],
            month: crate::calendar::month_start(today),
            year: today.year(),
            curves: [Bucket::Day; 4],
            curve_menu: None,
            task_period: Bucket::Day,
            task_menu: false,
            by_tag: false,
            classify_menu: false,
            details_by: 0,
            details_period: Bucket::Day,
            details_day: today,
            details_menu: None,
            timeline_week: crate::calendar::week_of(today)[0],
        });
        cx.notify();
    }

    /// The Statistics page on its Overview tab, as the account menu opens it.
    pub fn open_stats_overview(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
        self.open_stats(cx);
        self.update_stats(cx, |p| p.tab = 0);
    }

    fn update_stats(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut StatsPage)) {
        if let Some(stats) = self.stats.as_mut() {
            change(stats);
        }
        cx.notify();
    }

    /// The original's Statistics page for focus: overview, Trends, Most Focused Time and the year grid.
    fn render_stats(&self, page: StatsPage, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let stats = focus_stats(&self.sessions, today);
        let (count_change, minutes_change) = fern_core::focus::change_from_yesterday(&self.sessions, today);
        let card = || v_flex().p_4().gap_2().rounded(px(10.)).bg(theme.colors().surface_background);
        let nav = |id: &'static str, label: String, back: fn(&mut StatsPage), forward: fn(&mut StatsPage), cx: &mut Context<Self>| {
            h_flex()
                .gap_1()
                .child(IconButton::new(SharedString::from(format!("{id}-back")), IconName::ChevronLeft).on_click(
                    cx.listener(move |this, _, _, cx| this.update_stats(cx, back)),
                ))
                .child(Label::new(label).size(LabelSize::Small))
                .child(IconButton::new(SharedString::from(format!("{id}-forward")), IconName::ChevronRight).on_click(
                    cx.listener(move |this, _, _, cx| this.update_stats(cx, forward)),
                ))
        };
        let signed = |n: i64, text: String| if n > 0 { format!("+{text}") } else { text };
        let overview_item = |value: String, label: &'static str, change: Option<String>| {
            v_flex()
                .flex_1()
                .items_center()
                .gap_0p5()
                .child(div().text_size(px(26.)).text_color(theme.colors().accent).child(value))
                .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
                .children(change.map(|c| Label::new(c).size(LabelSize::XSmall).color(Color::Muted)))
        };
        let overview = card()
            .child(Label::new(strings.focus_overview).weight(FontWeight::SEMIBOLD))
            .child(
                h_flex()
                    .child(overview_item(
                        stats.today_count.to_string(),
                        strings.focus_today_pomo,
                        Some(strings.stats_from_yesterday.replace("{n}", &signed(count_change, count_change.to_string()))),
                    ))
                    .child(overview_item(stats.total_count.to_string(), strings.focus_total_pomo, None))
                    .child(overview_item(
                        format_minutes(stats.today_minutes),
                        strings.focus_today_focus,
                        Some(strings.stats_from_yesterday.replace(
                            "{n}",
                            &signed(minutes_change, if minutes_change < 0 { format!("-{}", format_minutes(-minutes_change)) } else { format_minutes(minutes_change) }),
                        )),
                    ))
                    .child(overview_item(format_minutes(stats.total_minutes), strings.focus_total_duration, None)),
            );

        // Trends: minutes per day of the chosen week.
        const CHART: f32 = 140.;
        let week = fern_core::focus::minutes_by_day(&self.sessions, page.week, 7);
        let week_top = week.iter().copied().max().unwrap_or(0).max(1) as f32;
        let average = week.iter().sum::<i64>() / 7;
        let week_label = format!(
            "{} - {}",
            crate::task_rows::short_date(page.week, strings),
            crate::task_rows::short_date(page.week + chrono::Duration::days(6), strings)
        );
        let bars = |values: Vec<i64>, top: f32, labels: Vec<String>, highlight: Option<usize>| {
            v_flex()
                .gap_1()
                .child(h_flex().h(px(CHART)).items_end().gap_1().children(values.iter().enumerate().map(|(i, v)| {
                    div()
                        .flex_1()
                        .h(px((CHART * *v as f32 / top).max(if *v > 0 { 2. } else { 0. })))
                        .rounded_t(px(3.))
                        .bg(if highlight == Some(i) { theme.colors().accent } else { theme.colors().accent.opacity(0.6) })
                })))
                .child(h_flex().gap_1().children(labels.into_iter().map(|l| {
                    div().flex_1().flex().justify_center().child(Label::new(l).size(LabelSize::XSmall).color(Color::Muted))
                })))
        };
        let today_index = (today - page.week).num_days();
        let trends = card()
            .flex_1()
            .min_w_0()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.stats_trends).weight(FontWeight::SEMIBOLD)))
                    .child(nav(
                        "trends",
                        week_label,
                        |p| p.week -= chrono::Duration::days(7),
                        |p| p.week += chrono::Duration::days(7),
                        cx,
                    )),
            )
            .child(
                Label::new(strings.stats_daily_average.replace("{t}", &format_minutes(average)))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(bars(
                week,
                week_top,
                crate::calendar::weekday_labels(&strings.weekdays_short).iter().map(|d| (*d).to_owned()).collect(),
                (0..7).contains(&today_index).then_some(today_index as usize),
            ));

        // Timeline: the week's sessions as blocks on a day's hours, as in the original.
        const TIMELINE: f32 = 200.;
        let blocks = fern_core::focus::timeline(&self.sessions, page.timeline_week);
        let timeline_label = if page.timeline_week == crate::calendar::week_of(today)[0] {
            strings.stats_this_week.to_owned()
        } else {
            format!(
                "{} - {}",
                crate::task_rows::short_date(page.timeline_week, strings),
                crate::task_rows::short_date(page.timeline_week + chrono::Duration::days(6), strings)
            )
        };
        let accent = theme.colors().accent;
        let grid_line = theme.colors().border_variant;
        let timeline = card()
            .flex_1()
            .min_w_0()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.stats_timeline).weight(FontWeight::SEMIBOLD)))
                    .child(nav(
                        "timeline",
                        timeline_label,
                        |p| p.timeline_week -= chrono::Duration::days(7),
                        |p| p.timeline_week += chrono::Duration::days(7),
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .child(v_flex().w(px(40.)).h(px(TIMELINE)).justify_between().children((0..=6).map(|i| {
                        Label::new(crate::calendar::format_hour((i * 4) % 24)).size(LabelSize::XSmall).color(Color::Muted)
                    })))
                    .child(
                        div()
                            .flex_1()
                            .h(px(TIMELINE))
                            .relative()
                            .children((0..=6).map(|i| {
                                div().absolute().left_0().right_0().top(px(TIMELINE * i as f32 / 6.)).h(px(1.)).bg(grid_line)
                            }))
                            .child(h_flex().size_full().children((0..7).map(|day| {
                                div().flex_1().h_full().relative().children(blocks.iter().filter(|(d, _, _)| *d == day).map(|(_, from, to)| {
                                    let top = TIMELINE * *from as f32 / 1440.;
                                    let height = (TIMELINE * (to - from) as f32 / 1440.).max(2.);
                                    div().absolute().left(px(4.)).right(px(4.)).top(px(top)).h(px(height)).rounded(px(2.)).bg(accent)
                                }))
                            }))),
                    ),
            )
            .child(h_flex().pl(px(48.)).children(crate::calendar::weekday_labels(&strings.weekdays_short).iter().map(|d| {
                div().flex_1().flex().justify_center().child(Label::new(*d).size(LabelSize::XSmall).color(Color::Muted))
            })));

        // Most Focused Time: minutes per hour of the chosen month.
        let hours = fern_core::focus::minutes_by_hour(&self.sessions, page.month);
        let hours_top = hours.iter().copied().max().unwrap_or(0).max(1) as f32;
        let most = hours.iter().enumerate().max_by_key(|(_, m)| **m).filter(|(_, m)| **m > 0).map(|(h, _)| h);
        let most_focused = card()
            .flex_1()
            .min_w_0()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.stats_most_focused).weight(FontWeight::SEMIBOLD)))
                    .child(nav(
                        "most",
                        crate::calendar::month_title(page.month, strings),
                        |p| p.month = crate::calendar::add_months(p.month, -1),
                        |p| p.month = crate::calendar::add_months(p.month, 1),
                        cx,
                    )),
            )
            .child(bars(
                hours.to_vec(),
                hours_top,
                (0..24).map(|h| if h % 3 == 0 { format!("{h:02}") } else { String::new() }).collect(),
                most,
            ));

        // Year Grids: one cell per day, columns are weeks.
        let first = NaiveDate::from_ymd_opt(page.year, 1, 1).unwrap_or(today);
        let grid_start = crate::calendar::week_of(first)[0];
        let days = fern_core::focus::minutes_by_day(&self.sessions, grid_start, 54 * 7);
        let shade = |minutes: i64| match minutes {
            0 => theme.colors().element_hover,
            1..60 => theme.colors().accent.opacity(0.3),
            60..180 => theme.colors().accent.opacity(0.55),
            180..300 => theme.colors().accent.opacity(0.8),
            _ => theme.colors().accent,
        };
        let year_grid = card()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.stats_year_grids).weight(FontWeight::SEMIBOLD)))
                    .child(nav("year", page.year.to_string(), |p| p.year -= 1, |p| p.year += 1, cx)),
            )
            .child(h_flex().gap(px(3.)).children((0..54).map(|week| {
                v_flex().gap(px(3.)).children((0..7).map(|weekday| {
                    let index = week * 7 + weekday;
                    let date = grid_start + chrono::Duration::days(index as i64);
                    div()
                        .size(px(11.))
                        .rounded(px(2.))
                        .bg(shade(days[index]))
                        .when(date.year() != page.year, |el| el.opacity(0.))
                }))
            })));

        // As in the original, the page keeps a readable width, centered in wide windows.
        div()
            .id("focus-stats")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(v_flex()
            .w_full()
            .max_w(px(960.))
            .px_8()
            .py_5()
            .gap_4()
            .child(
                h_flex()
                    .child(div().flex_1().child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(strings.statistics)))
                    .child(h_flex().p_0p5().rounded(px(16.)).bg(theme.colors().surface_background).children(
                        strings.stats_tabs.iter().enumerate().map(|(i, label)| {
                            div()
                                .id(SharedString::from(format!("stats-tab-{i}")))
                                .px_4()
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .rounded(px(14.))
                                .cursor_pointer()
                                .when(i == page.tab, |el| el.bg(theme.colors().element_selected))
                                .child(Label::new(*label).size(LabelSize::Small))
                                .on_click(cx.listener(move |this, _, _, cx| this.update_stats(cx, |p| p.tab = i)))
                        }),
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("stats-done")
                            .px_5()
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .rounded(px(8.))
                            .bg(theme.colors().accent)
                            .cursor_pointer()
                            .child(Label::new(strings.done))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stats = None;
                                cx.emit(StatsClosed);
                                cx.notify();
                            })),
                    ),
            )
            .map(|el| match page.tab {
                0 => el.children(self.render_overview_tab(page, today, cx)),
                1 => el.children(self.render_task_tab(page, today, cx)),
                _ => el
                    .child(overview)
                    .child(
                        h_flex()
                            .items_stretch()
                            .gap_4()
                            .child(div().flex_1().min_w_0().child(self.render_focus_details(page, today, cx)))
                            // Focus Record beside Details, as in the original.
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .p_4()
                                    .gap_2()
                                    .rounded(px(10.))
                                    .bg(theme.colors().surface_background)
                                    .child(self.render_record_header(false, cx))
                                    .child(
                                        v_flex()
                                            .id("stats-focus-record")
                                            .max_h(px(200.))
                                            .overflow_y_scroll()
                                            .children(self.render_record(strings, cx)),
                                    ),
                            ),
                    )
                    .child(h_flex().items_start().gap_4().child(trends).child(timeline))
                    .child(h_flex().items_start().gap_4().child(most_focused).child(div().flex_1()))
                    .child(year_grid),
            }))
            .into_any_element()
    }

    /// The Focus tab's "Details": focused time of a day, week or month per list, tag or task, as
    /// a ring chart with a legend.
    fn render_focus_details(&self, page: StatsPage, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let status = theme.status().clone();
        let tasks = self.store.tasks().unwrap_or_default();
        let lists = self.store.lists().unwrap_or_default();
        let all_tags = self.store.tags().unwrap_or_default();
        let week_start = crate::calendar::week_start();
        let period = page.details_period;
        let (first, last) = stats::buckets(period, page.details_day, week_start, 1)[0];
        let totals = fern_core::focus::focus_by(&self.sessions, &tasks, page.details_by, first, last);
        let palette = [theme.colors().accent, status.success, status.warning, status.error, theme.colors().text_muted];
        let slices: Vec<(SharedString, i64, Hsla)> = totals
            .into_iter()
            .enumerate()
            .map(|(i, (key, minutes))| {
                let fallback = palette[i % palette.len()];
                let (name, color): (SharedString, Hsla) = match key {
                    fern_core::focus::FocusKey::List(None) => (strings.inbox.into(), fallback),
                    fern_core::focus::FocusKey::List(Some(id)) => {
                        let list = lists.iter().find(|l| l.id == id);
                        (
                            list.map_or(SharedString::default(), |l| l.name.clone().into()),
                            list.and_then(|l| crate::lists_panel::parse_color(l.color.as_deref())).unwrap_or(fallback),
                        )
                    }
                    fern_core::focus::FocusKey::Tag(None) => (strings.group_no_tags.into(), fallback),
                    fern_core::focus::FocusKey::Tag(Some(tag)) => {
                        let color = all_tags
                            .iter()
                            .find(|t| t.name == tag)
                            .and_then(|t| crate::lists_panel::parse_color(t.color.as_deref()))
                            .unwrap_or(fallback);
                        (format!("#{tag}").into(), color)
                    }
                    fern_core::focus::FocusKey::Task(title) => (title.into(), fallback),
                };
                (name, minutes, color)
            })
            .collect();
        // A session with several tags counts once in the middle.
        let total: i64 = stats::focus_in(&self.sessions, &[(first, last)])[0].1;
        let current = stats::buckets(period, today, week_start, 1)[0] == (first, last);
        let label = match period {
            Bucket::Day if current => strings.today.to_owned(),
            Bucket::Week if current => strings.stats_this_week.to_owned(),
            Bucket::Month if current => strings.stats_this_month.to_owned(),
            Bucket::Day => crate::task_rows::short_date(first, strings),
            Bucket::Week => format!("{} - {}", crate::task_rows::short_date(first, strings), crate::task_rows::short_date(last, strings)),
            Bucket::Month => format!("{} {}", strings.months_short[first.month0() as usize], first.year()),
        };
        let by_names = [strings.stats_by_list_option, strings.stats_by_tag_option, strings.stats_by_task_option];
        let period_names = [strings.stats_day, strings.stats_week, strings.stats_month];
        let period_index = Bucket::ALL.iter().position(|b| *b == period).unwrap_or(0);
        let pill = |id: &'static str, menu: u8, text: &'static str, options: Vec<(&'static str, bool, Box<dyn Fn(&mut StatsPage)>)>, cx: &mut Context<Self>| {
            let open = page.details_menu == Some(menu);
            let options = Rc::new(options);
            let dropdown = open.then(|| {
                PopoverAttachment::new(
                    Placement::BelowRight,
                    popover(cx)
                        .w(px(140.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.update_stats(cx, |p| p.details_menu = None)))
                        .children((0..options.len()).map(|i| {
                            let (name, selected, _) = &options[i];
                            let options = options.clone();
                            ListItem::new(SharedString::from(format!("{id}-{i}")))
                                .child(Label::new(*name).color(if *selected { Color::Accent } else { Color::Default }))
                                .when(*selected, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_stats(cx, |p| {
                                        (options[i].2)(p);
                                        p.details_menu = None;
                                    })
                                }))
                        })),
                )
            });
            h_flex()
                .id(id)
                .relative()
                .px_2()
                .h(px(22.))
                .gap_0p5()
                .rounded(px(11.))
                .border_1()
                .border_color(if open { theme.colors().accent } else { theme.colors().border })
                .cursor_pointer()
                .child(Label::new(text).size(LabelSize::XSmall))
                .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                .on_click(cx.listener(move |this, _, _, cx| this.update_stats(cx, |p| p.details_menu = (!open).then_some(menu))))
                .children(dropdown)
        };
        let by_options = (0..3u8)
            .map(|by| {
                let set: Box<dyn Fn(&mut StatsPage)> = Box::new(move |p| p.details_by = by);
                (by_names[by as usize], by == page.details_by, set)
            })
            .collect();
        let period_options = Bucket::ALL
            .into_iter()
            .zip(period_names)
            .map(|(bucket, name)| {
                let set: Box<dyn Fn(&mut StatsPage)> = Box::new(move |p| p.details_period = bucket);
                (name, bucket == period, set)
            })
            .collect();
        let step = move |day: NaiveDate, forward: bool| {
            let (first, last) = stats::buckets(period, day, week_start, 1)[0];
            if forward { last + chrono::Duration::days(1) } else { first - chrono::Duration::days(1) }
        };
        v_flex()
            .p_4()
            .gap_2()
            .rounded(px(10.))
            .bg(theme.colors().surface_background)
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Label::new(strings.stats_details).weight(FontWeight::SEMIBOLD)))
                    .child(pill("details-by", 0, by_names[page.details_by as usize], by_options, cx))
                    .child(pill("details-period", 1, period_names[period_index], period_options, cx))
                    .child(IconButton::new("details-back", IconName::ChevronLeft).size(px(22.)).on_click(cx.listener(
                        move |this, _, _, cx| this.update_stats(cx, |p| p.details_day = step(p.details_day, false)),
                    )))
                    .child(Label::new(label).size(LabelSize::Small))
                    .child(IconButton::new("details-forward", IconName::ChevronRight).size(px(22.)).on_click(cx.listener(
                        move |this, _, _, cx| this.update_stats(cx, |p| p.details_day = step(p.details_day, true)),
                    ))),
            )
            .child(
                h_flex()
                    .py_2()
                    .gap_6()
                    .justify_center()
                    .child(
                        Donut::new(slices.iter().map(|(_, m, color)| (*m as f32, *color))).center(
                            v_flex()
                                .items_center()
                                .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(if total == 0 {
                                    strings.stats_no_data.to_owned()
                                } else {
                                    format_minutes(total)
                                }))
                                .when(total > 0, |el| el.child(Label::new(strings.focus).size(LabelSize::XSmall).color(Color::Muted))),
                        ),
                    )
                    .child(v_flex().gap_1().children(slices.into_iter().map(|(name, minutes, color)| {
                        h_flex()
                            .gap_2()
                            .child(div().size(px(8.)).rounded_full().bg(color))
                            .child(Label::new(format_minutes(minutes)).size(LabelSize::Small))
                            .child(Label::new(name).size(LabelSize::Small).color(Color::Muted).truncate())
                    }))),
            )
            .into_any_element()
    }

    /// The Statistics page's Overview tab: totals, recent curves and the week's habits.
    fn render_overview_tab(&self, page: StatsPage, today: NaiveDate, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let tasks = self.store.tasks().unwrap_or_default();
        let lists = self.store.lists().unwrap_or_default();
        let focus = focus_stats(&self.sessions, today);
        let completed_total = tasks.iter().filter(|t| t.is_completed() && !t.is_deleted()).count();
        let open_total = tasks.iter().filter(|t| !t.is_closed() && !t.is_deleted()).count();
        let start = today - chrono::Duration::days(6);
        let completed = fern_core::stats::completed_by_day(&tasks, start, 7);
        let card = || v_flex().min_w_0().p_4().gap_2().rounded(px(10.)).bg(theme.colors().surface_background);
        let day_labels: Vec<String> =
            (0..7).map(|i| (start + chrono::Duration::days(i)).day().to_string()).collect();
        let value = |v: String, label: &'static str| {
            v_flex()
                .flex_1()
                .items_center()
                .child(div().text_size(px(22.)).text_color(theme.colors().accent).child(v))
                .child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted))
        };
        let summary = strings
            .stats_summary
            .replace("{tasks}", &open_total.to_string())
            .replace("{completed}", &completed_total.to_string())
            .replace("{lists}", &lists.len().to_string())
            .replace("{days}", &fern_core::stats::days_using(&tasks, today).to_string());
        // Each curve has its own Day / Week / Month switch, as in the original.
        let periods = |curve: usize| stats::buckets(page.curves[curve], today, crate::calendar::week_start(), 7);
        let labels = |curve: usize| -> Vec<String> {
            let bucket = page.curves[curve];
            let periods = periods(curve);
            let last = periods.len() - 1;
            periods
                .iter()
                .enumerate()
                .map(|(i, (first, _))| match bucket {
                    Bucket::Day if i == last => strings.today.to_owned(),
                    Bucket::Week if i == last => strings.stats_this_week.to_owned(),
                    Bucket::Month if i == last => strings.stats_this_month.to_owned(),
                    Bucket::Month => strings.months_short[first.month0() as usize].to_owned(),
                    _ => first.day().to_string(),
                })
                .collect()
        };
        let completed_curve: Vec<i64> = stats::completed_in(&tasks, &periods(0)).into_iter().map(|n| n as i64).collect();
        let rates: Vec<i64> = stats::completion_rate_in(&tasks, &periods(1)).into_iter().map(i64::from).collect();
        let pomos: Vec<i64> = stats::focus_in(&self.sessions, &periods(2)).into_iter().map(|(n, _)| n as i64).collect();
        let minutes: Vec<i64> = stats::focus_in(&self.sessions, &periods(3)).into_iter().map(|(_, m)| m).collect();
        let productive = stats::more_productive_than(&tasks, today);
        let checkins: Vec<NaiveDate> = self
            .store
            .habits()
            .unwrap_or_default()
            .iter()
            .flat_map(|h| self.store.checkins(h.id).unwrap_or_default())
            .collect();
        let scores = fern_core::stats::achievement_scores(&tasks, &self.sessions, &checkins, start, 7);
        let score = scores.last().copied().unwrap_or(0);
        let chart = |curve: usize, title: &'static str, values: Vec<i64>, line: bool, cx: &mut Context<Self>| {
            let bucket = page.curves[curve];
            let open = page.curve_menu == Some(curve);
            let names = [strings.stats_day, strings.stats_week, strings.stats_month];
            let index = Bucket::ALL.iter().position(|b| *b == bucket).unwrap_or(0);
            let menu = open.then(|| {
                PopoverAttachment::new(
                    Placement::BelowRight,
                    popover(cx)
                        .w(px(140.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.update_stats(cx, |p| p.curve_menu = None)))
                        .children(Bucket::ALL.into_iter().zip(names).enumerate().map(|(i, (choice, name))| {
                            ListItem::new(SharedString::from(format!("curve-{curve}-{i}")))
                                .child(Label::new(name).color(if choice == bucket { Color::Accent } else { Color::Default }))
                                .when(choice == bucket, |item| {
                                    item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_stats(cx, |p| {
                                        p.curves[curve] = choice;
                                        p.curve_menu = None;
                                    })
                                }))
                        })),
                )
            });
            let labels = labels(curve);
            card()
                .flex_1()
                .child(
                    h_flex()
                        .child(div().flex_1().child(Label::new(title).weight(FontWeight::SEMIBOLD)))
                        .child(
                            h_flex()
                                .id(SharedString::from(format!("curve-period-{curve}")))
                                .relative()
                                .px_2()
                                .h(px(22.))
                                .gap_0p5()
                                .rounded(px(11.))
                                .border_1()
                                .border_color(if open { theme.colors().accent } else { theme.colors().border })
                                .cursor_pointer()
                                .child(Label::new(names[index]).size(LabelSize::XSmall))
                                .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_stats(cx, |p| p.curve_menu = (!open).then_some(curve))
                                }))
                                .children(menu),
                        ),
                )
                .map(|el| {
                    if line {
                        el.child(line_chart(&values, labels, 120., true, cx))
                    } else {
                        el.child(bar_chart(&values, labels, Some(6), 120., cx))
                    }
                })
        };

        // The week's habits: the share of active habits done each day.
        let habits: Vec<fern_core::Habit> = self.store.habits().unwrap_or_default().into_iter().filter(|h| !h.archived).collect();
        let done: Vec<std::collections::HashSet<NaiveDate>> =
            habits.iter().map(|h| self.store.checkins(h.id).unwrap_or_default()).collect();
        let week = crate::calendar::week_of(today);
        let habit_week = h_flex().justify_between().children(week.iter().map(|date| {
            let share = if habits.is_empty() { 0. } else { done.iter().filter(|d| d.contains(date)).count() as f32 / habits.len() as f32 };
            v_flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .size(px(28.))
                        .rounded_full()
                        .border_2()
                        .border_color(if share >= 1. { theme.colors().accent } else { theme.colors().element_hover })
                        .when(share > 0. && share < 1., |el| el.bg(theme.colors().accent.opacity(0.3)))
                        .when(share >= 1., |el| el.bg(theme.colors().accent.opacity(0.6))),
                )
                .child(Label::new(strings.weekdays_short[date.weekday().num_days_from_monday() as usize]).size(LabelSize::XSmall).color(Color::Muted))
        }));

        vec![
            div()
                .px_4()
                .py_2()
                .rounded(px(10.))
                .bg(theme.colors().surface_background)
                .flex()
                .child(div().flex_1().child(Label::new(summary).size(LabelSize::Small)))
                .child(
                    h_flex().gap_1().children({
                        let (before, after) = strings.stats_more_productive.split_once("{n}").unwrap_or((strings.stats_more_productive, ""));
                        [
                            Label::new(before.trim_end()).size(LabelSize::Small).color(Color::Muted),
                            Label::new(format!("{productive}%")).size(LabelSize::Small).color(Color::Warning),
                            Label::new(after.trim_start()).size(LabelSize::Small).color(Color::Muted),
                        ]
                    }),
                )
                .into_any_element(),
            h_flex()
                .items_stretch()
                .gap_4()
                .child(
                    card()
                        .flex_1()
                        .gap_4()
                        .child(Label::new(strings.focus_overview).weight(FontWeight::SEMIBOLD))
                        .child(
                            h_flex()
                                .child(value(completed[6].to_string(), strings.stats_today_completed))
                                .child(value(focus.today_count.to_string(), strings.focus_today_pomo))
                                .child(value(format_minutes(focus.today_minutes), strings.focus_today_focus)),
                        )
                        .child(
                            h_flex()
                                .child(value(completed_total.to_string(), strings.stats_total_completed))
                                .child(value(focus.total_count.to_string(), strings.focus_total_pomo))
                                .child(value(format_minutes(focus.total_minutes), strings.focus_total_duration)),
                        ),
                )
                .child(
                    card()
                        .flex_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().flex_1().child(Label::new(strings.stats_achievement_score).weight(FontWeight::SEMIBOLD)))
                                .child(div().text_size(px(18.)).text_color(theme.status().success).child(score.to_string()))
                                .child(
                                    div()
                                        .px_1p5()
                                        .h(px(20.))
                                        .flex()
                                        .items_center()
                                        .rounded(px(6.))
                                        .bg(theme.colors().accent.opacity(0.25))
                                        .child(
                                            Label::new(strings.stats_level.replace("{level}", &fern_core::stats::achievement_level(score).to_string()))
                                                .size(LabelSize::XSmall)
                                                .color(Color::Accent),
                                        ),
                                ),
                        )
                        .child(line_chart(&scores, day_labels.clone(), 110., false, cx)),
                )
                .into_any_element(),
            h_flex()
                .items_start()
                .gap_4()
                .child(chart(0, strings.stats_completion_curve, completed_curve, true, cx))
                .child(chart(1, strings.stats_rate_curve, rates, false, cx))
                .into_any_element(),
            h_flex()
                .items_start()
                .gap_4()
                .child(chart(2, strings.stats_pomo_curve, pomos, true, cx))
                .child(chart(3, strings.stats_focus_curve, minutes, false, cx))
                .into_any_element(),
            card().child(Label::new(strings.stats_weekly_habits).weight(FontWeight::SEMIBOLD)).child(habit_week).into_any_element(),
        ]
    }

    /// The Statistics page's Task tab: a day's completed tasks and rate, and completion per list.
    /// The Statistics page's Task tab: the original's Daily / Weekly / Monthly report of a period:
    /// completed tasks and rate with the change from the previous period, the completion per day,
    /// how the tasks ended, and completion per list.
    fn render_task_tab(&self, page: StatsPage, today: NaiveDate, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let tasks = self.store.tasks().unwrap_or_default();
        let lists = self.store.lists().unwrap_or_default();
        let week_start = crate::calendar::week_start();
        let period = page.task_period;
        // The period containing a day, and the one before it.
        let range = |day: NaiveDate| stats::buckets(period, day, week_start, 1)[0];
        let (first, last) = range(page.day);
        let (prev_first, prev_last) = range(first - chrono::Duration::days(1));
        let count = |a: NaiveDate, b: NaiveDate| stats::completed_in(&tasks, &[(a, b)])[0] as i64;
        let rate = |a: NaiveDate, b: NaiveDate| stats::completion_rate_in(&tasks, &[(a, b)])[0] as i64;
        let signed = |n: i64, unit: &str| if n > 0 { format!("+{n}{unit}") } else { format!("{n}{unit}") };
        let from = match period {
            Bucket::Day => strings.stats_from_yesterday,
            Bucket::Week => strings.stats_from_last_week,
            Bucket::Month => strings.stats_from_last_month,
        };
        let card = || v_flex().min_w_0().p_4().gap_2().rounded(px(10.)).bg(theme.colors().surface_background);
        let value = |v: String, label: &'static str, change: String| {
            v_flex()
                .flex_1()
                .items_center()
                .child(div().text_size(px(26.)).text_color(theme.colors().accent).child(v))
                .child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted))
                .child(Label::new(from.replace("{n}", &change)).size(LabelSize::XSmall).color(Color::Muted))
        };
        let current = range(today) == (first, last);
        let period_label = match period {
            Bucket::Day if current => strings.today.to_owned(),
            Bucket::Week if current => strings.stats_this_week.to_owned(),
            Bucket::Month if current => strings.stats_this_month.to_owned(),
            Bucket::Day => crate::task_rows::short_date(first, strings),
            Bucket::Week => format!("{} - {}", crate::task_rows::short_date(first, strings), crate::task_rows::short_date(last, strings)),
            Bucket::Month => format!("{} {}", strings.months_short[first.month0() as usize], first.year()),
        };
        let names = [strings.stats_daily, strings.stats_weekly, strings.stats_monthly];
        let index = Bucket::ALL.iter().position(|b| *b == period).unwrap_or(0);
        let menu = page.task_menu.then(|| {
            PopoverAttachment::new(
                Placement::BelowLeft,
                popover(cx)
                    .w(px(140.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.update_stats(cx, |p| p.task_menu = false)))
                    .children(Bucket::ALL.into_iter().zip(names).enumerate().map(|(i, (choice, name))| {
                        ListItem::new(SharedString::from(format!("task-period-{i}")))
                            .child(Label::new(name).color(if choice == period { Color::Accent } else { Color::Default }))
                            .when(choice == period, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_stats(cx, |p| {
                                    p.task_period = choice;
                                    p.task_menu = false;
                                })
                            }))
                    })),
            )
        });
        let step = move |day: NaiveDate, forward: bool| -> NaiveDate {
            let (first, last) = stats::buckets(period, day, week_start, 1)[0];
            if forward { last + chrono::Duration::days(1) } else { first - chrono::Duration::days(1) }
        };

        let by_list = stats::completed_by_list_in(&tasks, first, last);
        let kinds = stats::completion_kinds(&tasks, first, last);
        let status = theme.status().clone();
        let kind_rows = [
            (status.error, kinds.overdue, strings.stats_overdue),
            (theme.colors().accent, kinds.on_time, strings.stats_on_time),
            (status.warning, kinds.undated, strings.stats_undated),
            (theme.colors().text_muted, kinds.uncompleted, strings.stats_uncompleted),
        ];
        let rate_card = card()
            .flex_1()
            .child(Label::new(strings.stats_rate_distribution).weight(FontWeight::SEMIBOLD))
            .child(
                h_flex()
                    .py_2()
                    .gap_6()
                    .justify_center()
                    .child(
                        Donut::new(kind_rows.iter().map(|(color, n, _)| (*n as f32, *color))).center(
                            v_flex()
                                .items_center()
                                .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(if kinds.total() == 0 {
                                    strings.stats_no_data.to_owned()
                                } else {
                                    format!("{}%", rate(first, last))
                                }))
                                .when(kinds.total() > 0, |el| {
                                    el.child(Label::new(strings.stats_completion_rate).size(LabelSize::XSmall).color(Color::Muted))
                                }),
                        ),
                    )
                    .when(kinds.total() > 0, |el| {
                        el.child(v_flex().gap_1().children(kind_rows.iter().map(|(color, n, label)| {
                            h_flex()
                                .gap_2()
                                .child(div().size(px(8.)).rounded_full().bg(*color))
                                .child(Label::new(n.to_string()).size(LabelSize::Small))
                                .child(Label::new(*label).size(LabelSize::Small).color(Color::Muted))
                        })))
                    }),
            );
        // Weekly and Monthly add the completion per day, as in the original.
        let distribution = (period != Bucket::Day).then(|| {
            let days = (last - first).num_days() as usize + 1;
            let values: Vec<i64> = stats::completed_by_day(&tasks, first, days).into_iter().map(|n| n as i64).collect();
            let labels: Vec<String> = (0..days)
                .map(|i| {
                    let day = first + chrono::Duration::days(i as i64);
                    match period {
                        Bucket::Week => strings.weekdays_short[day.weekday().num_days_from_monday() as usize].to_owned(),
                        // A label every fifth day keeps a month readable.
                        _ if i % 5 == 0 => day.day().to_string(),
                        _ => String::new(),
                    }
                })
                .collect();
            card()
                .flex_1()
                .child(Label::new(strings.stats_completion_distribution).weight(FontWeight::SEMIBOLD))
                .child(bar_chart(&values, labels, None, 120., cx))
        });

        // Daily shows how tasks ended beside the overview; Weekly and Monthly put the completion
        // per day there and move it below.
        let (top_right, lower_rate) = match distribution {
            Some(distribution) => (distribution, Some(rate_card)),
            None => (rate_card, None),
        };
        vec![
            h_flex()
                .gap_2()
                .child(
                    h_flex()
                        .id("task-period")
                        .relative()
                        .px_3()
                        .h(px(28.))
                        .gap_1()
                        .rounded(px(8.))
                        .bg(theme.colors().surface_background)
                        .cursor_pointer()
                        .child(Label::new(names[index]).size(LabelSize::Small))
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(|this, _, _, cx| this.update_stats(cx, |p| p.task_menu = !p.task_menu)))
                        .children(menu),
                )
                .child(
                    h_flex()
                        .h(px(28.))
                        .px_1()
                        .gap_1()
                        .rounded(px(8.))
                        .bg(theme.colors().surface_background)
                        .child(IconButton::new("task-day-back", IconName::ChevronLeft).on_click(cx.listener(move |this, _, _, cx| {
                            this.update_stats(cx, |p| p.day = step(p.day, false))
                        })))
                        .child(div().min_w(px(90.)).flex().justify_center().child(Label::new(period_label).size(LabelSize::Small)))
                        .child(IconButton::new("task-day-forward", IconName::ChevronRight).on_click(cx.listener(move |this, _, _, cx| {
                            this.update_stats(cx, |p| p.day = step(p.day, true))
                        }))),
                )
                .into_any_element(),
            h_flex()
                .items_stretch()
                .gap_4()
                .child(
                    card()
                        .flex_1()
                        .child(Label::new(strings.focus_overview).weight(FontWeight::SEMIBOLD))
                        .child(
                            h_flex()
                                .py_4()
                                .child(value(count(first, last).to_string(), strings.stats_completed_tasks, signed(count(first, last) - count(prev_first, prev_last), "")))
                                .child(value(
                                    format!("{}%", rate(first, last)),
                                    strings.stats_completion_rate,
                                    signed(rate(first, last) - rate(prev_first, prev_last), "%"),
                                )),
                        ),
                )
                .child(top_right)
                .into_any_element(),
            h_flex()
                .items_start()
                .gap_4()
                .children(lower_rate)
                .child(
                    card()
                        .flex_1()
                        .child(
                            h_flex()
                                .child(div().flex_1().child(Label::new(strings.stats_by_list).weight(FontWeight::SEMIBOLD)))
                                .child({
                                    let names = [strings.stats_by_list_option, strings.stats_by_tag_option];
                                    let menu = page.classify_menu.then(|| {
                                        PopoverAttachment::new(
                                            Placement::BelowRight,
                                            popover(cx)
                                                .w(px(140.))
                                                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.update_stats(cx, |p| p.classify_menu = false)))
                                                .children([false, true].into_iter().map(|tag| {
                                                    let selected = tag == page.by_tag;
                                                    ListItem::new(SharedString::from(format!("classify-{tag}")))
                                                        .child(Label::new(names[tag as usize]).color(if selected { Color::Accent } else { Color::Default }))
                                                        .when(selected, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                            this.update_stats(cx, |p| {
                                                                p.by_tag = tag;
                                                                p.classify_menu = false;
                                                            })
                                                        }))
                                                })),
                                        )
                                    });
                                    h_flex()
                                        .id("classify-by")
                                        .relative()
                                        .px_2()
                                        .h(px(22.))
                                        .gap_0p5()
                                        .rounded(px(11.))
                                        .border_1()
                                        .border_color(if page.classify_menu { theme.colors().accent } else { theme.colors().border })
                                        .cursor_pointer()
                                        .child(Label::new(names[page.by_tag as usize]).size(LabelSize::XSmall))
                                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                                        .on_click(cx.listener(|this, _, _, cx| this.update_stats(cx, |p| p.classify_menu = !p.classify_menu)))
                                        .children(menu)
                                }),
                        )
                        .child({
                            // Each list in its own color, or one from a palette.
                            let palette = [theme.colors().accent, status.success, status.warning, status.error, theme.colors().text_muted];
                            let slices: Vec<(SharedString, usize, Hsla)> = if page.by_tag {
                                let tags = self.store.tags().unwrap_or_default();
                                stats::completed_by_tag_in(&tasks, first, last)
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, (tag, n))| {
                                        let color = tag
                                            .as_ref()
                                            .and_then(|name| tags.iter().find(|t| &t.name == name))
                                            .and_then(|t| crate::lists_panel::parse_color(t.color.as_deref()))
                                            .unwrap_or(palette[i % palette.len()]);
                                        let name: SharedString = tag.map_or(strings.group_no_tags.into(), |t| format!("#{t}").into());
                                        (name, n, color)
                                    })
                                    .collect()
                            } else {
                                by_list
                                    .iter()
                                    .enumerate()
                                    .map(|(i, (list, n))| {
                                        let list = list.and_then(|id| lists.iter().find(|l| l.id == id));
                                        let name: SharedString = list.map_or(strings.inbox.into(), |l| l.name.clone().into());
                                        let color = list
                                            .and_then(|l| crate::lists_panel::parse_color(l.color.as_deref()))
                                            .unwrap_or(palette[i % palette.len()]);
                                        (name, *n, color)
                                    })
                                    .collect()
                            };
                            // A task with several tags counts once in the middle.
                            let unique: usize = by_list.iter().map(|(_, n)| n).sum();
                            let completed = unique;
                            h_flex()
                                .py_2()
                                .gap_6()
                                .justify_center()
                                .child(
                                    Donut::new(slices.iter().map(|(_, n, color)| (*n as f32, *color))).center(
                                        v_flex()
                                            .items_center()
                                            .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(if completed == 0 {
                                                strings.stats_no_data.to_owned()
                                            } else {
                                                completed.to_string()
                                            }))
                                            .when(completed > 0, |el| {
                                                el.child(Label::new(strings.stats_completed_tasks).size(LabelSize::XSmall).color(Color::Muted))
                                            }),
                                    ),
                                )
                                .child(v_flex().gap_1().children(slices.into_iter().map(|(name, n, color)| {
                                    h_flex()
                                        .gap_2()
                                        .child(div().size(px(8.)).rounded_full().bg(color))
                                        .child(Label::new(n.to_string()).size(LabelSize::Small))
                                        .child(Label::new(name).size(LabelSize::Small).color(Color::Muted).truncate())
                                })))
                        }),
                )
                // Daily keeps the list card at half width, as in the original.
                .when(period == Bucket::Day, |el| el.child(div().flex_1()))
                .into_any_element(),
        ]
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
        let stats = focus_stats(&self.sessions, Local::now().date_naive());
        let card = |label: &'static str, value: String| {
            v_flex()
                .flex_1()
                .h(px(70.))
                .p_2()
                .gap_1()
                .rounded(px(8.))
                .bg(theme.colors().surface_background)
                .child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted))
                .child(div().text_size(px(22.)).child(value))
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(card(strings.focus_today_pomo, stats.today_count.to_string()))
                    .child(card(strings.focus_today_focus, format_minutes(stats.today_minutes))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(card(strings.focus_total_pomo, stats.total_count.to_string()))
                    .child(card(strings.focus_total_duration, format_minutes(stats.total_minutes))),
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
                            .weight(FontWeight::SEMIBOLD),
                    ))
                    .child(
                        div()
                            .relative()
                            .child(IconButton::new("record-add", IconName::Plus).on_click(cx.listener(|this, _, _, cx| {
                                this.set_record_menu(Some(RecordMenu::Add), cx)
                            })))
                            .when(self.record_menu == Some(RecordMenu::Add), |el| {
                                el.child(PopoverAttachment::new(Placement::BelowRight, self.render_record_menu(RecordMenu::Add, cx)))
                            }),
                    )
                    .child(
                        div()
                            .relative()
                            .child(IconButton::new("record-more", IconName::Ellipsis).on_click(cx.listener(|this, _, _, cx| {
                                this.set_record_menu(Some(RecordMenu::More), cx)
                            })))
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
                let month = strings.months_short[date.month0() as usize];
                let mut label =
                    if strings.day_before_month { format!("{} {month}", date.day()) } else { format!("{month} {}", date.day()) };
                if date.year() != today.year() {
                    label = format!("{label}, {}", date.year());
                }
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
                                    crate::calendar::format_time(start),
                                    crate::calendar::format_time(session.end_local())
                                ))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                            )
                            .child(Label::new(session.title.clone()).truncate())
                            .when(!session.note.is_empty(), |el| {
                                el.child(Label::new(session.note.clone()).size(LabelSize::Small).color(Color::Muted).truncate())
                            }),
                    )
                    .child(Label::new(format_minutes(session.minutes())).size(LabelSize::XSmall).color(Color::Muted))
                    .child(
                        div().opacity(0.).group_hover(group, |s| s.opacity(1.)).child(
                            IconButton::new(SharedString::from(format!("delete-record-{session_id}")), IconName::X)
                                .size(px(20.))
                                .icon_size(IconSize::XSmall)
                                .on_click(cx.listener(move |this, _, _, cx| this.delete_record(session_id, cx))),
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
        self.picker = Some(Picker { search, at_eye, views_open: false });
        cx.notify();
    }

    fn pick(&mut self, next: NextFocus, cx: &mut Context<Self>) {
        self.next = Some(next);
        self.picker = None;
        cx.notify();
    }

    /// The page's Start: the picked task, timer or habit, or a plain session.
    fn start_next(&mut self, cx: &mut Context<Self>) {
        match self.next.take() {
            Some(NextFocus::Task(id, title)) => self.start(Some((id, title)), cx),
            Some(NextFocus::Timer(timer)) => self.start_from_timer(&timer, cx),
            Some(NextFocus::Habit(name)) => {
                let stopwatch = self.stopwatch_mode();
                self.start_named(name, stopwatch, cx)
            }
            None => self.start(None, cx),
        }
    }

    /// The picker's list: today's tasks and due habits, the active timers, or the active habits,
    /// narrowed by the search.
    fn render_picker(&self, picker: &Picker, cx: &mut Context<Self>) -> AnyElement {
        use fern_core::focus::matches_search;
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let query = picker.search.read(cx).text().to_owned();
        let today = Local::now().date_naive();
        let habits: Vec<fern_core::Habit> = self.store.habits().unwrap_or_default().into_iter().filter(|h| !h.archived && matches_search(&h.name, &query)).collect();
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
        .chain(self.store.lists().unwrap_or_default().into_iter().map(|l| (View::List(l.id), l.name.into())))
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
                                if let Some(picker) = this.picker.as_mut() {
                                    picker.views_open = false;
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
                let listed = if self.picker_view == View::Today {
                    self.tasks.clone()
                } else {
                    self.store.tasks_in(&self.picker_view, today).unwrap_or_default()
                };
                for task in listed.iter().filter(|t| matches_search(&t.title, &query)) {
                    items.push(item(format!("pick-task-{}", task.id).into(), task.title.clone(), NextFocus::Task(task.id, task.title.clone()), cx));
                }
                // As in the original, the habits due today follow Today's tasks.
                let due: Vec<&fern_core::Habit> = habits
                    .iter()
                    .filter(|h| self.picker_view == View::Today && h.is_due(today, &self.store.checkins(h.id).unwrap_or_default()))
                    .collect();
                if !due.is_empty() {
                    items.push(div().px_2().pt_1().child(Label::new(strings.focus_picker_tabs[2]).size(LabelSize::XSmall).color(Color::Muted)).into_any_element());
                }
                for habit in due {
                    items.push(item(format!("pick-habit-{}", habit.id).into(), habit.name.clone(), NextFocus::Habit(habit.name.clone()), cx));
                }
            }
            1 => {
                let timers = self.store.focus_timers(false).unwrap_or_default();
                for timer in timers.into_iter().filter(|t| matches_search(&t.name, &query)) {
                    items.push(item(format!("pick-timer-{}", timer.id).into(), timer.name.clone(), NextFocus::Timer(timer), cx));
                }
            }
            _ => {
                for habit in &habits {
                    items.push(item(format!("pick-habit-{}", habit.id).into(), habit.name.clone(), NextFocus::Habit(habit.name.clone()), cx));
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
            .child(v_flex().id("focus-picker-items").flex_1().min_h_0().overflow_y_scroll().children(items))
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
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(text))
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
                .child(pill("focus-page-end", strings.focus_end, false).on_click(cx.listener(|this, _, _, cx| this.finish(cx))))
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
                        h_flex().flex_1().justify_end().child(
                            div()
                                .relative()
                                .child(IconButton::new("focus-page-more", IconName::Ellipsis).on_click(cx.listener(|this, _, _, cx| {
                                    this.set_record_menu(Some(RecordMenu::HeaderMore), cx)
                                })))
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
                                    .cursor_pointer()
                                    .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
                                    .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(Color::Muted))
                                    .when(!running, |el| el.on_click(cx.listener(|this, _, window, cx| this.open_picker(false, window, cx)))),
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
            .children(fern_core::focus::day_blocks(&self.sessions, today).into_iter().filter(|(_, to, _)| *to > first * 60).map(|(from, to, session)| {
                div()
                    .absolute()
                    .top(top(from.max(first * 60)))
                    .left(px(32.))
                    .right_0()
                    .h(px(((to - from.max(first * 60)) as f32 / 60. * HOUR).max(4.)))
                    .px_2()
                    .rounded(px(4.))
                    .bg(theme.colors().accent.opacity(0.25))
                    .border_l_2()
                    .border_color(theme.colors().accent)
                    .overflow_hidden()
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
                                    .on_click(cx.listener(|this, _, window, cx| this.open_picker(true, window, cx))),
                            )
                            .children(picker_eye),
                    ),
            )
            .child(v_flex().id("focus-page-hours").flex_1().min_h_0().overflow_y_scroll().pt_2().child(hours));
        h_flex()
            .relative()
            .size_full()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(left)
            .child(right)
            .into_any_element()
    }
}

impl Render for FocusView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(page) = self.stats {
            return self.render_stats(page, cx);
        }
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let now = now_ms();

        // Minutes already focused per task today, shown next to each task.
        let today = Local::now().date_naive();
        let mut focused: HashMap<Id, i64> = HashMap::new();
        for session in self.sessions.iter().filter(|s| s.start_local().date_naive() == today) {
            if let Some(task) = session.task_id {
                *focused.entry(task).or_default() += session.minutes();
            }
        }
        let rows: Vec<AnyElement> = self
            .tasks
            .iter()
            .map(|task| {
                let target = (task.id, task.title.clone());
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
                                        let done = self.sessions.iter().filter(|s| s.task_id == Some(task.id)).count();
                                        format!("{done}/{n} {}", strings.pomo_unit)
                                    }
                                    Some(fern_core::Estimate::Minutes(n)) => {
                                        let done: i64 = self
                                            .sessions
                                            .iter()
                                            .filter(|s| s.task_id == Some(task.id))
                                            .map(|s| (s.ended_at - s.started_at) / 60_000)
                                            .sum();
                                        format!("{}/{}", format_minutes(done), format_minutes(n as i64))
                                    }
                                    None => format_minutes(focused.get(&task.id).copied().unwrap_or(0)),
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
            .clone()
            .iter()
            .map(|timer| {
                let minutes: i64 = self
                    .sessions
                    .iter()
                    .filter(|s| s.start_local().date_naive() == today && s.title == timer.name)
                    .map(|s| s.minutes())
                    .sum();
                self.render_timer(timer, minutes, cx)
            })
            .collect();

        let (clock, label, running, paused) = match &self.running {
            Some(r) => (
                format_clock(r.timer.display(now)),
                match r.on_break {
                    Some(BreakKind::Short) => SharedString::new_static(strings.focus_short_break),
                    Some(BreakKind::Long) => SharedString::new_static(strings.focus_long_break),
                    None => r.task.as_ref().map_or(SharedString::new_static(strings.focus), |(_, t)| t.clone().into()),
                },
                true,
                r.timer.is_paused(),
            ),
            None => (
                format_clock(if self.stopwatch_mode() { 0 } else { self.minutes(POMO_SETTING) * 60_000 }),
                SharedString::new_static(strings.focus),
                false,
                false,
            ),
        };

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
                    .h(px(52.))
                    .flex_none()
                    .px(px(22.))
                    .gap_2()
                    .child(div().flex_1().child(Label::new(strings.pomodoro).size(LabelSize::Large).weight(FontWeight::BOLD)))
                    // The original's Active and Archived tabs over the timers.
                    .child(h_flex().p_0p5().rounded(px(8.)).bg(theme.colors().element_hover).children(
                        [(false, strings.habit_active), (true, strings.habit_archived)].map(|(archived, label)| {
                            let on = self.archived_tab == archived;
                            div()
                                .id(SharedString::from(format!("timers-tab-{archived}")))
                                .px_3()
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .when(on, |el| el.bg(theme.colors().element_selected))
                                .child(Label::new(label).size(LabelSize::Small).color(if on { Color::Default } else { Color::Muted }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.archived_tab = archived;
                                    this.reload(cx);
                                }))
                        }),
                    ))
                    .child(IconButton::new("timer-add", IconName::Plus).on_click(cx.listener(|this, _, window, cx| this.open_timer(None, window, cx))))
                    .child(
                        div()
                            .relative()
                            .child(IconButton::new("pomo-more", IconName::Ellipsis).on_click(cx.listener(|this, _, _, cx| {
                                this.set_record_menu(Some(RecordMenu::HeaderMore), cx)
                            })))
                            .when_some(
                                self.record_menu.filter(|m| matches!(m, RecordMenu::HeaderMore | RecordMenu::Settings)),
                                |el, menu| el.child(PopoverAttachment::new(Placement::BelowRight, self.render_record_menu(menu, cx))),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("focus-tasks")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .children(timer_rows)
                    .when(!self.archived_tab && rows.is_empty() && self.timers.is_empty(), |el| {
                        el.child(div().px_2().pt_4().child(Label::new(strings.focus_no_tasks).color(Color::Muted)))
                    })
                    .when(!self.archived_tab && !self.timers.is_empty() && !rows.is_empty(), |el| el.child(div().my_1().child(Divider::horizontal())))
                    .when(!self.archived_tab, |el| el.children(rows)),
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
                            .cursor_pointer()
                            .child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted).truncate())
                            .child(div().text_size(px(20.)).child(clock))
                            .on_click(cx.listener(|this, _, _, cx| this.set_expanded(true, cx))),
                    )
                    // Pomo or Stopwatch, chosen while no timer runs.
                    .when(!running, |el| el.child(self.render_mode_switch(LabelSize::XSmall, cx)))
                    .when(running, |el| {
                        el.child(
                            IconButton::new("focus-stop", IconName::Square)
                                .on_click(cx.listener(|this, _, _, cx| this.finish(cx))),
                        )
                    })
                    .child(
                        IconButton::new("focus-play", if running && !paused { IconName::Pause } else { IconName::Play })
                            .icon_color(Color::Accent)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if running { this.toggle_pause(cx) } else { this.start(None, cx) }
                            })),
                    ),
            );

        let record = self.render_record(strings, cx);
        let right = v_flex()
            .w(px(380.))
            .flex_none()
            .h_full()
            .px_4()
            .child(h_flex().h(px(52.)).child(Label::new(strings.focus_overview).size(LabelSize::Large).weight(FontWeight::SEMIBOLD)))
            .child(self.render_overview(strings, cx))
            .child(self.render_record_header(true, cx))
            .when_some(self.message.clone(), |el, message| {
                el.child(Label::new(message).size(LabelSize::XSmall).color(Color::Muted))
            })
            .child(
                v_flex()
                    .id("focus-record")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(record.is_empty(), |el| {
                        el.child(div().pt_4().child(Label::new(strings.focus_no_record).color(Color::Muted)))
                    })
                    .children(record),
            );

        h_flex()
            .relative()
            .size_full()
            .items_start()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(left)
            .child(right)
            .children(self.render_record_dialog(cx))
            .children(self.render_modal(cx))
            .into_any_element()
    }
}
