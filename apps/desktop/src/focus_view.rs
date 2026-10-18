//! The Focus (Pomodoro) section, matched to TickTick's: things to focus on on the left with a timer
//! bar at the bottom, an overview and the focus record on the right.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, NaiveDate};
use fern_core::stats::{self, Bucket};
use fern_core::{FocusSession, Id, Store, Task, View, focus_stats, format_minutes};
use gpui::{Entity, FontWeight, Task as GpuiTask};
use ui::{Placement, PopoverAttachment, popover};
use ui::prelude::*;

use crate::focus_timer::{BreakKind, FocusTimer, break_after, format_clock};
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
    /// "Delete All" asks for a second click.
    delete_all_armed: bool,
    add_title: Entity<TextInput>,
    add_start: Entity<TextInput>,
    add_minutes: i64,
    /// The last export's result.
    message: Option<String>,
    /// The Statistics page, while it is open.
    stats: Option<StatsPage>,
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

/// The Focus Record's "+" form and "…" menu, as in the original.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RecordMenu {
    Add,
    More,
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
            delete_all_armed: false,
            add_title,
            add_start,
            add_minutes: 25,
            message: None,
            stats: None,
        };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        self.tasks = self.store.tasks_in(&View::Today, today).unwrap_or_default();
        self.sessions = self.store.focus_sessions().unwrap_or_default();
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
        self.finish(cx);
        self.set_stopwatch_mode(stopwatch, cx);
        let timer = if stopwatch {
            FocusTimer::stopwatch(now_ms())
        } else {
            FocusTimer::start(now_ms(), self.minutes(POMO_SETTING) * 60_000)
        };
        self.start_timer(Some((None, title)), None, timer, cx);
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
            if let Err(err) = self.store.add_focus_session(task_id, &title, start, start + elapsed) {
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
        let strings = i18n::t(cx);
        let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(|| ".".into());
        let path = dir.join(format!("fern-focus-{}.csv", Local::now().format("%Y-%m-%d")));
        self.message = Some(match std::fs::write(&path, fern_core::focus::export_csv(&self.sessions)) {
            Ok(()) => strings.exported_to.replace("{path}", &path.display().to_string()),
            Err(err) => err.to_string(),
        });
        self.set_record_menu(None, cx);
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
            RecordMenu::More => popover(cx)
                .w(px(240.))
                .on_mouse_down_out(close)
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
                1 => el.children(self.render_task_tab(page.day, today, cx)),
                _ => el
                    .child(overview)
                    .child(h_flex().items_start().gap_4().child(trends).child(most_focused))
                    .child(year_grid),
            }))
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
    fn render_task_tab(&self, day: NaiveDate, today: NaiveDate, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let tasks = self.store.tasks().unwrap_or_default();
        let lists = self.store.lists().unwrap_or_default();
        let yesterday = fern_core::stats::yesterday(day);
        let count = |d: NaiveDate| fern_core::stats::completed_by_day(&tasks, d, 1)[0] as i64;
        let rate = |d: NaiveDate| fern_core::stats::completion_rate(&tasks, d).unwrap_or(0) as i64;
        let signed = |n: i64, unit: &str| if n > 0 { format!("+{n}{unit}") } else { format!("{n}{unit}") };
        let card = || v_flex().min_w_0().p_4().gap_2().rounded(px(10.)).bg(theme.colors().surface_background);
        let value = |v: String, label: &'static str, change: String| {
            v_flex()
                .flex_1()
                .items_center()
                .child(div().text_size(px(26.)).text_color(theme.colors().accent).child(v))
                .child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted))
                .child(Label::new(strings.stats_from_yesterday.replace("{n}", &change)).size(LabelSize::XSmall).color(Color::Muted))
        };
        let day_label = if day == today { strings.today.to_owned() } else { crate::task_rows::short_date(day, strings) };
        let by_list = fern_core::stats::completed_by_list(&tasks, day);
        let most = by_list.first().map_or(1, |(_, n)| *n).max(1) as f32;
        vec![
            h_flex()
                .gap_1()
                .child(IconButton::new("task-day-back", IconName::ChevronLeft).on_click(cx.listener(|this, _, _, cx| {
                    this.update_stats(cx, |p| p.day -= chrono::Duration::days(1))
                })))
                .child(Label::new(day_label).size(LabelSize::Small))
                .child(IconButton::new("task-day-forward", IconName::ChevronRight).on_click(cx.listener(|this, _, _, cx| {
                    this.update_stats(cx, |p| p.day += chrono::Duration::days(1))
                })))
                .into_any_element(),
            card()
                .child(Label::new(strings.focus_overview).weight(FontWeight::SEMIBOLD))
                .child(
                    h_flex()
                        .child(value(count(day).to_string(), strings.stats_completed_tasks, signed(count(day) - count(yesterday), "")))
                        .child(value(format!("{}%", rate(day)), strings.stats_completion_rate, signed(rate(day) - rate(yesterday), "%"))),
                )
                .into_any_element(),
            card()
                .child(Label::new(strings.stats_by_list).weight(FontWeight::SEMIBOLD))
                .when(by_list.is_empty(), |el| el.child(Label::new(strings.stats_no_data).color(Color::Muted)))
                .children(by_list.into_iter().map(|(list, n)| {
                    let name: SharedString = list
                        .and_then(|id| lists.iter().find(|l| l.id == id))
                        .map_or(strings.inbox.into(), |l| l.name.clone().into());
                    h_flex()
                        .gap_2()
                        .child(div().w(px(140.)).child(Label::new(name).size(LabelSize::Small).truncate()))
                        .child(
                            div().flex_1().h(px(10.)).rounded(px(5.)).bg(theme.colors().element_hover).child(
                                div().h_full().w(gpui::relative(n as f32 / most)).rounded(px(5.)).bg(theme.colors().accent),
                            ),
                        )
                        .child(Label::new(n.to_string()).size(LabelSize::Small))
                }))
                .into_any_element(),
        ]
    }

    #[cfg(debug_assertions)]
    pub fn dev_menu(&mut self, menu: &str, cx: &mut Context<Self>) {
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
    pub fn dev_record(&mut self, title: &str, minutes_ago: i64, minutes: i64, cx: &mut Context<Self>) {
        let start = now_ms() - minutes_ago * 60_000;
        self.store.add_focus_session(None, title, start, start + minutes * 60_000).ok();
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
                    .child(div().mt(px(3.)).size(px(10.)).flex_none().rounded_full().bg(theme.colors().accent))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                Label::new(format!(
                                    "{} - {}",
                                    crate::calendar::format_time(start),
                                    crate::calendar::format_time(session.end_local())
                                ))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                            )
                            .child(Label::new(session.title.clone()).truncate()),
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
                                Label::new(format_minutes(focused.get(&task.id).copied().unwrap_or(0)))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(Icon::new(IconName::Play).size(IconSize::Small).color(Color::Accent)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.start(Some(target.clone()), cx)))
                    .into_any_element()
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
                    .when(rows.is_empty(), |el| {
                        el.child(div().px_2().pt_4().child(Label::new(strings.focus_no_tasks).color(Color::Muted)))
                    })
                    .children(rows),
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
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted).truncate())
                            .child(div().text_size(px(20.)).child(clock)),
                    )
                    // Pomo or Stopwatch, chosen while no timer runs.
                    .when(!running, |el| {
                        let stopwatch = self.stopwatch_mode();
                        el.child(h_flex().p_0p5().rounded(px(12.)).bg(theme.colors().surface_background).children(
                            strings.focus_modes.iter().enumerate().map(|(i, label)| {
                                let selected = (i == 1) == stopwatch;
                                div()
                                    .id(SharedString::from(format!("focus-mode-{i}")))
                                    .px_3()
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(10.))
                                    .cursor_pointer()
                                    .when(selected, |el| el.bg(theme.colors().element_selected))
                                    .child(Label::new(*label).size(LabelSize::XSmall).color(if selected { Color::Default } else { Color::Muted }))
                                    .on_click(cx.listener(move |this, _, _, cx| this.set_stopwatch_mode(i == 1, cx)))
                            }),
                        ))
                    })
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
            .child(
                h_flex()
                    .pt_5()
                    .child(div().flex_1().child(Label::new(strings.focus_record).size(LabelSize::Large).weight(FontWeight::SEMIBOLD)))
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
                            .when(self.record_menu == Some(RecordMenu::More), |el| {
                                el.child(PopoverAttachment::new(Placement::BelowRight, self.render_record_menu(RecordMenu::More, cx)))
                            }),
                    ),
            )
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
            .size_full()
            .items_start()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(left)
            .child(right)
            .into_any_element()
    }
}
