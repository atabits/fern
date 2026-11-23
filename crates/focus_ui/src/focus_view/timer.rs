//! Timer lifecycle, active-session state, white noise, and Pomodoro row rendering.

use std::time::Duration;

use crate::focus_timer::{format_clock, format_stopwatch_clock, pomo_length_ms};

use super::*;

impl FocusView {
    fn prepare_start(&mut self, cx: &mut Context<Self>) -> bool {
        if self.finish_prompt {
            return false;
        }
        if self.dirty {
            self.reload(cx);
        }
        self.finish(cx);
        if self.running.is_some() {
            return false;
        }
        self.completed_in_chain = 0;
        true
    }

    fn new_clock(&self, stopwatch: bool) -> FocusTimer {
        if stopwatch {
            FocusTimer::stopwatch(now_ms())
        } else {
            FocusTimer::start(now_ms(), pomo_length_ms(self.minutes(POMO_SETTING)))
        }
    }

    /// Starts a Pomodoro, or a stopwatch in the stopwatch mode.
    pub fn start(&mut self, task: Option<(Id, String)>, cx: &mut Context<Self>) {
        if !self.prepare_start(cx) {
            return;
        }
        let timer = self.new_clock(self.stopwatch_mode());
        self.start_timer(
            task.map(|(id, title)| FocusTarget { task_id: Some(id), habit_id: None, title }),
            None,
            timer,
            cx,
        );
    }

    /// Starts a Pomodoro (`stopwatch == false`) or stopwatch (`stopwatch == true`) without a target task.
    pub fn start_mode(&mut self, stopwatch: bool, cx: &mut Context<Self>) {
        if !self.prepare_start(cx) {
            return;
        }
        self.set_stopwatch_mode(stopwatch, cx);
        let timer = self.new_clock(stopwatch);
        self.start_timer(None, None, timer, cx);
    }

    /// "Start Focus" on a habit: a Pomodoro or a stopwatch named after it, in that mode.
    pub fn start_named(&mut self, title: String, stopwatch: bool, cx: &mut Context<Self>) {
        self.start_for_habit(None, title, stopwatch, cx);
    }

    /// "Start Focus" on a specific habit, linking the recorded session so time-based goals auto check in.
    pub fn start_for_habit(&mut self, habit_id: Option<Id>, title: String, stopwatch: bool, cx: &mut Context<Self>) {
        if !self.prepare_start(cx) {
            return;
        }
        self.set_stopwatch_mode(stopwatch, cx);
        let timer = self.new_clock(stopwatch);
        self.start_timer(Some(FocusTarget { task_id: None, habit_id, title }), None, timer, cx);
    }

    /// "Start Focus → Start Pomo / Start Stopwatch" for a task, in that mode.
    pub fn start_for(&mut self, task: Option<Id>, title: String, stopwatch: bool, cx: &mut Context<Self>) {
        if !self.prepare_start(cx) {
            return;
        }
        self.set_stopwatch_mode(stopwatch, cx);
        let timer = self.new_clock(stopwatch);
        self.start_timer(Some(FocusTarget { task_id: task, habit_id: None, title }), None, timer, cx);
    }

    /// A timer's play button: a session named after it, with its own Pomo length or as a
    /// stopwatch.
    pub(super) fn start_from_timer(&mut self, timer: &fern_core::focus::FocusTimer, cx: &mut Context<Self>) {
        if !self.prepare_start(cx) {
            return;
        }
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

    pub(super) fn stopwatch_mode(&self) -> bool {
        self.settings.stopwatch_mode
    }

    pub(super) fn set_stopwatch_mode(&mut self, stopwatch: bool, cx: &mut Context<Self>) {
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
    pub(super) fn tick(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn render_timer(&self, timer: &fern_core::focus::FocusTimer, today_minutes: i64, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = timer.id;
        let played = timer.clone();
        let group = SharedString::from(format!("timer-{id}"));
        let menu_open = self.timer_menu == Some(id);
        let archived = timer.archived;
        let edited = timer.clone();
        let timer_name = timer.name.clone();
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
                        div()
                            .when(cfg!(test), |el| {
                                el.debug_selector(move || format!("focus-timer-more-{id}"))
                            })
                            .child(
                                IconButton::new(SharedString::from(format!("timer-more-{id}")), IconName::Ellipsis)
                                    .size(px(22.))
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(strings.more))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.timer_menu = if this.timer_menu == Some(id) { None } else { Some(id) };
                                        cx.notify();
                                    })),
                            ),
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
                                            desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .when(cfg!(test), |el| {
                                            el.debug_selector(|| "focus-timer-delete-item".to_owned())
                                        })
                                        .child(
                                            ListItem::new("timer-delete")
                                                .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                                                .child(Label::new(strings.delete).color(Color::Error))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.timer_menu = None;
                                                    this.confirm_delete_timer = Some((id, timer_name.clone()));
                                                    cx.notify();
                                                })),
                                        ),
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

    pub(super) fn sync_ambient_sound(&mut self) {
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
