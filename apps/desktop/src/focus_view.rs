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

use crate::focus_timer::{FocusTimer, format_clock};
use crate::i18n::{self, Strings};
use crate::text_input::{Mode, TextInput};

/// The default Pomodoro length.
const POMO_MS: i64 = 25 * 60_000;
/// Shorter sessions are not recorded.
const MIN_RECORDED_MS: i64 = 60_000;

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

struct Running {
    timer: FocusTimer,
    task: Option<(Id, String)>,
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
}

/// The Focus Record's "+" form and "…" menu, as in the original.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RecordMenu {
    Add,
    More,
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

    fn start(&mut self, task: Option<(Id, String)>, cx: &mut Context<Self>) {
        self.finish(cx);
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        });
        self.running = Some(Running { timer: FocusTimer::start(now_ms(), POMO_MS), task, _ticker: ticker });
        cx.notify();
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.running.as_ref().is_some_and(|r| r.timer.finished(now_ms())) {
            self.finish(cx);
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
        if elapsed >= MIN_RECORDED_MS {
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

    #[cfg(debug_assertions)]
    pub fn dev_menu(&mut self, menu: &str, cx: &mut Context<Self>) {
        let menu = match menu {
            "add" => RecordMenu::Add,
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
                r.task.as_ref().map_or(SharedString::new_static(strings.focus), |(_, t)| t.clone().into()),
                true,
                r.timer.is_paused(),
            ),
            None => (format_clock(POMO_MS), SharedString::new_static(strings.focus), false, false),
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
                    .child(div().flex_1().child(Label::new(strings.pomodoro).size(LabelSize::Large).weight(FontWeight::BOLD))),
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
    }
}
