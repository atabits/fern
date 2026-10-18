//! The Focus (Pomodoro) section, matched to TickTick's: things to focus on on the left with a timer
//! bar at the bottom, an overview and the focus record on the right.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, NaiveDate};
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
/// Shorter sessions are not recorded.
const MIN_RECORDED_MS: i64 = 60_000;

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

struct Running {
    timer: FocusTimer,
    task: Option<(Id, String)>,
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
    week: NaiveDate,
    month: NaiveDate,
    year: i32,
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

    fn start(&mut self, task: Option<(Id, String)>, cx: &mut Context<Self>) {
        self.finish(cx);
        let length = self.minutes(POMO_SETTING) * 60_000;
        self.start_timer(task, None, length, cx);
    }

    fn start_timer(&mut self, task: Option<(Id, String)>, on_break: Option<BreakKind>, length_ms: i64, cx: &mut Context<Self>) {
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        });
        self.running = Some(Running { timer: FocusTimer::start(now_ms(), length_ms), task, on_break, _ticker: ticker });
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
                self.start_timer(None, Some(kind), length, cx);
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
            let (task_id, title) = running.task.map_or((None, strings.focus.to_owned()), |(id, t)| (Some(id), t));
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
            week: crate::calendar::week_of(today)[0],
            month: crate::calendar::month_start(today),
            year: today.year(),
        });
        cx.notify();
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
                strings.weekdays_short.iter().map(|d| (*d).to_owned()).collect(),
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

        v_flex()
            .id("focus-stats")
            .size_full()
            .overflow_y_scroll()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .px_8()
            .py_5()
            .gap_4()
            .child(
                h_flex()
                    .child(div().flex_1().child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(strings.statistics)))
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
                                cx.notify();
                            })),
                    ),
            )
            .child(overview)
            .child(h_flex().items_start().gap_4().child(trends).child(most_focused))
            .child(year_grid)
            .into_any_element()
    }

    #[cfg(debug_assertions)]
    pub fn dev_menu(&mut self, menu: &str, cx: &mut Context<Self>) {
        let menu = match menu {
            "add" => RecordMenu::Add,
            "settings" => RecordMenu::Settings,
            "stats" => {
                self.open_stats(cx);
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
                                    start.format("%H:%M"),
                                    session.end_local().format("%H:%M")
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
                format_clock(r.timer.remaining(now)),
                match r.on_break {
                    Some(BreakKind::Short) => SharedString::new_static(strings.focus_short_break),
                    Some(BreakKind::Long) => SharedString::new_static(strings.focus_long_break),
                    None => r.task.as_ref().map_or(SharedString::new_static(strings.focus), |(_, t)| t.clone().into()),
                },
                true,
                r.timer.is_paused(),
            ),
            None => (format_clock(self.minutes(POMO_SETTING) * 60_000), SharedString::new_static(strings.focus), false, false),
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
