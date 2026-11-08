//! Review long stopwatch sessions using EnsureTimingDialog's minute rounding and
//! save rules. Method RVAs and the remaining UI differences are in MECHANICS.md.
use super::*;
use gpui::{Bounds, TitlebarOptions, WindowBounds, WindowOptions, size};

// EnsureTimingDialog async MoveNext methods: RVAs 0x00386c9c / 0x00386e3c.
const FIELD_CORRECTION_DELAY_MS: u64 = 10;
const MINUTE_MINIMUM_DELAY_MS: u64 = 1000;
const REVIEW_HOUR_LIMIT: i32 = 12;
const REVIEW_MINUTE_LIMIT: i32 = 59;

// Utils::SecondToMinute, RVA 0x002a48d3: round halves upward.
fn rounded_minutes(ms: i64) -> i64 {
    let seconds = ms / 1000;
    seconds / 60 + i64::from(seconds % 60 >= 30)
}

fn clock(ms: i64) -> String {
    let minutes = rounded_minutes(ms);
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

fn duration(text: &str, maximum: i64) -> Option<i64> {
    let fields: Vec<_> = text.trim().split(':').collect();
    if fields.len() != 2 || fields.iter().any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit())) { return None; }
    let hours = fields[0].parse::<i64>().ok()?;
    let minutes = fields[1].parse::<i64>().ok()?;
    if hours > 12 || minutes >= 60 { return None; }
    let selected = hours * 60 + minutes;
    let original = rounded_minutes(maximum);
    if !(MIN_FOCUS_MS / 60_000..=original.min(720)).contains(&selected) { return None; }
    // EnsureTimingDialog::TrySaveAndClose, RVA 0x000cd81c: retaining the
    // rounded display retains the original whole seconds, including when rounded up.
    Some(if selected == original { maximum / 1000 * 1000 } else { selected * 60_000 })
}

impl FocusView {
    pub(super) fn open_stopwatch_review(&mut self, title: String, elapsed: i64, paused: bool, limited: bool, parent: &mut Window, cx: &mut Context<Self>) {
        let focus = cx.entity();
        let ru = i18n::t(cx).inbox == "Входящие";
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(parent.display(cx).map(|display| display.id()), size(px(460.), px(400.)), cx))),
            titlebar: Some(TitlebarOptions { title: Some(if ru { "Сохранить фокус" } else { "Save focus" }.into()), ..Default::default() }),
            is_resizable: false,
            app_id: Some(std::env::var("FERN_APP_ID").unwrap_or_else(|_| "fern-tasks".into())),
            ..Default::default()
        };
        self.finish_prompt = true;
        match cx.open_window(options, |window, cx| cx.new(|cx| FocusReview::new(focus, title, elapsed, paused, limited, window, cx))) {
            Ok(handle) => self.review_window = Some(handle),
            Err(error) => {
                self.finish_prompt = false;
                if !paused && !limited && let Some(running) = self.running.as_mut() { running.timer.resume(now_ms()); }
                self.message = Some(error.to_string());
            }
        }
        cx.notify();
    }
}

pub(super) struct FocusReview {
    focus: Entity<FocusView>,
    title: String,
    hours: Entity<TextInput>,
    minutes: Entity<TextInput>,
    maximum: i64,
    paused: bool,
    limited: bool,
    resolved: bool,
    error: Option<String>,
}

impl FocusReview {
    fn new(focus: Entity<FocusView>, title: String, maximum: i64, paused: bool, limited: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rounded = rounded_minutes(maximum);
        let hours = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "0", cx);
            input.set_text((rounded / 60).to_string(), cx);
            input
        });
        let minutes = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "00", cx);
            input.set_text(format!("{:02}", rounded % 60), cx);
            input
        });
        for input in [&hours, &minutes] {
            cx.subscribe_in(input, window, |this, _, _: &crate::text_input::Submitted, window, cx| this.save(window, cx)).detach();
            cx.subscribe_in(input, window, |this, _, _: &crate::text_input::Cancelled, window, cx| this.cancel(window, cx)).detach();
        }
        cx.subscribe_in(&hours, window, |this, _, _: &crate::text_input::Changed, window, cx| this.correct_field(true, window, cx)).detach();
        cx.subscribe_in(&minutes, window, |this, _, _: &crate::text_input::Changed, window, cx| this.correct_field(false, window, cx)).detach();
        window.focus(&hours.focus_handle(cx), cx);
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |this: &mut Self, cx| this.restore(cx)).ok();
            true
        });
        cx.on_release(|this, cx| this.restore(cx)).detach();
        Self { focus, title, hours, minutes, maximum, paused, limited, resolved: false, error: None }
    }

    fn selected_duration(&self, cx: &App) -> Option<i64> {
        duration(&format!("{}:{}", self.hours.read(cx).text().trim(), self.minutes.read(cx).text().trim()), self.maximum)
    }

    fn rewrite(input: &Entity<TextInput>, value: i32, window: &Window, cx: &mut App) {
        let text = value.to_string();
        if input.read(cx).text() != text {
            // Like WPF TextChanged, a corrective edit schedules another pass.
            // Select the correction so the next keystroke replaces it.
            input.update(cx, |input, cx| input.edit(text.clone(), 0..text.len(), window, cx));
        }
    }

    fn correct_field(&mut self, hours: bool, window: &Window, cx: &mut Context<Self>) {
        self.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(FIELD_CORRECTION_DELAY_MS)).await;
            let wait_for_minimum = this.update_in(cx, |this, window, cx| {
                if this.resolved { return false; }
                // The original ignores TryParse's result: invalid/overflowing Int32 is zero.
                let hour = this.hours.read(cx).text().trim().parse::<i32>().unwrap_or(0);
                let minute = this.minutes.read(cx).text().trim().parse::<i32>().unwrap_or(0);
                let minimum = (MIN_FOCUS_MS / 60_000) as i32;
                if hours {
                    if hour >= REVIEW_HOUR_LIMIT {
                        Self::rewrite(&this.hours, REVIEW_HOUR_LIMIT, window, cx);
                        Self::rewrite(&this.minutes, 0, window, cx);
                    } else if hour <= 0 {
                        Self::rewrite(&this.hours, 0, window, cx);
                        if minute < minimum { Self::rewrite(&this.minutes, minimum, window, cx); }
                    }
                } else if minute > REVIEW_MINUTE_LIMIT {
                    Self::rewrite(&this.minutes, REVIEW_MINUTE_LIMIT, window, cx);
                } else if hour == 0 && minute < minimum {
                    return true;
                } else if minute < 0 {
                    Self::rewrite(&this.minutes, 0, window, cx);
                }
                cx.notify();
                false
            }).unwrap_or(false);
            if wait_for_minimum {
                cx.background_executor().timer(Duration::from_millis(MINUTE_MINIMUM_DELAY_MS)).await;
                this.update_in(cx, |this, window, cx| {
                    if this.resolved { return; }
                    // Re-read the current text, allowing a multi-digit value to be completed.
                    let minute = this.minutes.read(cx).text().trim().parse::<i32>().unwrap_or(0);
                    let minimum = (MIN_FOCUS_MS / 60_000) as i32;
                    if minute < minimum { Self::rewrite(&this.minutes, minimum, window, cx); }
                    cx.notify();
                }).ok();
            }
        }).detach();
    }

    fn restore(&mut self, cx: &mut App) {
        if self.resolved { return; }
        self.resolved = true;
        self.focus.update(cx, |focus, cx| {
            focus.finish_prompt = false;
            focus.review_window = None;
            if !self.paused && !self.limited && let Some(running) = focus.running.as_mut() { running.timer.resume(now_ms()); }
            cx.notify();
        });
    }

    pub(super) fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // EnsureTimingDialog::get_CanCancel, RVA 0x000cd666, and its Escape handler.
        if self.limited { return; }
        self.restore(cx);
        window.remove_window();
    }

    pub(super) fn discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.resolved { return; }
        self.resolved = true;
        self.focus.update(cx, |focus, cx| {
            focus.running = None;
            focus.continuation = None;
            focus.cycle.reset();
            focus.finish_prompt = false;
            focus.review_window = None;
            focus.message = None;
            cx.notify();
        });
        window.remove_window();
    }

    pub(super) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.resolved { return; }
        let Some(value) = self.selected_duration(cx) else {
            let ru = i18n::t(cx).inbox == "Входящие";
            self.error = Some(if ru { "Введите целые часы и минуты от 0 до 59 в указанном диапазоне." }
                else { "Enter whole hours and minutes from 0 to 59 within the shown range." }.to_owned());
            cx.notify();
            return;
        };
        let result = self.focus.update(cx, |focus, cx| -> anyhow::Result<()> {
            let running = focus.running.as_ref().ok_or_else(|| anyhow::anyhow!("Focus session is no longer available"))?;
            let (task, title) = running.task.as_ref().map_or((None, i18n::t(cx).focus), |(id, title)| (*id, title.as_str()));
            let start = running.timer.started_at;
            focus.store.add_focus_session_as(task, title, start, start + value, true)?;
            focus.running = None;
            focus.continuation = None;
            focus.cycle.reset();
            focus.finish_prompt = false;
            focus.review_window = None;
            focus.message = None;
            focus.reload(cx);
            Ok(())
        });
        match result {
            Ok(()) => { self.resolved = true; window.remove_window(); }
            Err(error) => { self.error = Some(error.to_string()); cx.notify(); }
        }
    }
}

impl Render for FocusReview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ru = i18n::t(cx).inbox == "Входящие";
        let colors = cx.theme().colors().clone();
        let can_save = self.selected_duration(cx).is_some();
        v_flex().size_full().p_5().gap_3().bg(colors.background).text_color(colors.text)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab" {
                    let hours = this.hours.focus_handle(cx);
                    let minutes = this.minutes.focus_handle(cx);
                    window.focus(if hours.is_focused(window) { &minutes } else { &hours }, cx);
                    cx.stop_propagation();
                }
            }))
            .child(Label::new(if ru { "Сохранить время фокуса" } else { "Save focused time" }).size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
            .child(Label::new(self.title.clone()).truncate())
            .child(Label::new(if self.limited {
                if ru { "Достигнут лимит 12 часов. Уточните время перед сохранением." } else { "The 12-hour limit was reached. Review the time before saving." }
            } else if ru { "Секундомер на паузе. При необходимости уменьшите длительность." }
            else { "The stopwatch is paused. Reduce the duration if needed." }).size(LabelSize::Small))
            .child(h_flex().gap_3()
                .child(v_flex().flex_1().gap_2()
                    .child(Label::new(if ru { "Часы" } else { "Hours" }).size(LabelSize::Small))
                    .child(div().p_2().rounded(px(6.)).border_1().border_color(colors.border).child(self.hours.clone())))
                .child(v_flex().flex_1().gap_2()
                    .child(Label::new(if ru { "Минуты" } else { "Minutes" }).size(LabelSize::Small))
                    .child(div().p_2().rounded(px(6.)).border_1().border_color(colors.border).child(self.minutes.clone()))))
            .child(Label::new(format!("{} – {}", clock(MIN_FOCUS_MS), clock(self.maximum))).size(LabelSize::Small).color(Color::Muted))
            .when_some(self.error.clone(), |el, error| el.child(Label::new(error).size(LabelSize::Small)))
            .child(div().flex_1())
            .child(h_flex().gap_2()
                .child(div().id("focus-review-discard").px_3().py_2().rounded(px(6.)).cursor_pointer().hover(|el| el.bg(colors.element_hover))
                    .child(Label::new(if ru { "Не сохранять" } else { "Discard" }))
                    .on_click(cx.listener(|this, _, window, cx| this.discard(window, cx))))
                .child(div().flex_1())
                .when(!self.limited, |el| el.child(div().id("focus-review-cancel").px_3().py_2().rounded(px(6.)).cursor_pointer().hover(|el| el.bg(colors.element_hover))
                    .child(Label::new(if ru { "Продолжить" } else { "Continue" }))
                    .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx)))))
                .child(div().id("focus-review-save").px_3().py_2().rounded(px(6.)).bg(colors.accent)
                    .when(can_save, |el| el.cursor_pointer())
                    .when(!can_save, |el| el.opacity(0.5))
                    .child(Label::new(if ru { "Сохранить" } else { "Save" }).color(Color::Custom(gpui::rgb(0xffffff).into())))
                    .when(can_save, |el| el.on_click(cx.listener(|this, _, window, cx| this.save(window, cx))))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn delayed_field_corrections_follow_original_bounds_and_reread_text(cx: &mut TestAppContext) {
        cx.update(|cx| { theme::init(cx); cx.set_global(i18n::CurrentLocale(i18n::Locale::En)); });
        let store = Rc::new(Store::open_in_memory().unwrap());
        let handle = cx.add_window(|_, cx| FocusView::new(store.clone(), cx));
        let review = handle.update(cx, |focus, window, cx| {
            focus.start_for(None, "Field corrections".to_owned(), true, cx);
            let now = now_ms();
            focus.running.as_mut().unwrap().timer = FocusTimer::stopwatch(now - 7_506_000);
            focus.running.as_mut().unwrap().timer.pause(now);
            focus.request_finish(window, cx);
            focus.review_window.unwrap()
        }).unwrap();
        review.update(cx, |review, window, cx| {
            review.hours.update(cx, |input, cx| input.edit("13".into(), 2..2, window, cx));
            assert!(review.selected_duration(cx).is_none());
        }).unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(9));
        review.update(cx, |review, _, cx| assert_eq!(review.hours.read(cx).text(), "13")).unwrap();
        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        review.update(cx, |review, window, cx| {
            assert_eq!(review.hours.read(cx).text(), "12");
            assert_eq!(review.hours.read(cx).selection(), 0..2);
            assert_eq!(review.minutes.read(cx).text(), "0");
            assert!(review.selected_duration(cx).is_none());
            review.hours.update(cx, |input, cx| input.edit("1".into(), 1..1, window, cx));
            review.minutes.update(cx, |input, cx| input.edit("75".into(), 2..2, window, cx));
        }).unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(20));
        cx.run_until_parked();
        review.update(cx, |review, window, cx| {
            assert_eq!(review.minutes.read(cx).text(), "59");
            assert_eq!(review.minutes.read(cx).selection(), 0..2);
            assert!(review.selected_duration(cx).is_some());
            review.minutes.update(cx, |input, cx| input.edit("-1".into(), 2..2, window, cx));
        }).unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(20));
        cx.run_until_parked();
        review.update(cx, |review, window, cx| {
            assert_eq!(review.minutes.read(cx).text(), "0");
            review.hours.update(cx, |input, cx| input.edit("-1".into(), 2..2, window, cx));
        }).unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(20));
        cx.run_until_parked();
        review.update(cx, |review, window, cx| {
            assert_eq!(review.hours.read(cx).text(), "0");
            assert_eq!(review.minutes.read(cx).text(), "5");
            review.minutes.update(cx, |input, cx| input.edit("1".into(), 1..1, window, cx));
        }).unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(10));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(999));
        review.update(cx, |review, window, cx| {
            assert_eq!(review.minutes.read(cx).text(), "1");
            review.minutes.update(cx, |input, cx| input.edit("15".into(), 2..2, window, cx));
        }).unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(11));
        cx.run_until_parked();
        review.update(cx, |review, window, cx| {
            assert_eq!(review.minutes.read(cx).text(), "15");
            review.minutes.update(cx, |input, cx| input.edit("1".into(), 1..1, window, cx));
        }).unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(10));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(1000));
        cx.run_until_parked();
        review.update(cx, |review, window, cx| {
            assert_eq!(review.minutes.read(cx).text(), "5");
            assert_eq!(review.minutes.read(cx).selection(), 0..1);
            review.save(window, cx);
        }).unwrap();
        let sessions = store.focus_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].ended_at - sessions[0].started_at, MIN_FOCUS_MS);
        handle.update(cx, |_, window, _| window.remove_window()).unwrap();
    }

    #[gpui::test]
    fn review_preserves_task_and_saves_edited_time_only_once(cx: &mut TestAppContext) {
        cx.update(|cx| { theme::init(cx); cx.set_global(i18n::CurrentLocale(i18n::Locale::En)); });
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Prepare project").unwrap();
        let handle = cx.add_window(|_, cx| FocusView::new(store.clone(), cx));
        let review = handle.update(cx, |focus, window, cx| {
            focus.start_for(Some(task.id), task.title.clone(), true, cx);
            let now = now_ms();
            focus.running.as_mut().unwrap().timer = FocusTimer::stopwatch(now - 7_506_000);
            focus.running.as_mut().unwrap().timer.pause(now);
            focus.request_finish(window, cx);
            focus.review_window.unwrap()
        }).unwrap();
        review.update(cx, |review, window, cx| {
            review.hours.update(cx, |input, cx| input.set_text("3", cx));
            review.minutes.update(cx, |input, cx| input.set_text("00", cx));
            review.save(window, cx);
            assert!(review.error.is_some());
            assert!(store.focus_sessions().unwrap().is_empty());
            review.hours.update(cx, |input, cx| input.set_text("1", cx));
            for invalid in ["", "60", "-1", "1:15", "9999999999999999999999999"] {
                review.minutes.update(cx, |input, cx| input.set_text(invalid, cx));
                review.save(window, cx);
                assert!(review.error.is_some(), "minutes={invalid}");
                assert!(store.focus_sessions().unwrap().is_empty());
            }
            review.minutes.update(cx, |input, cx| input.set_text("15", cx));
            review.save(window, cx);
            review.save(window, cx);
        }).unwrap();
        handle.update(cx, |focus, window, cx| {
            assert!(!focus.finish_prompt);
            assert!(focus.review_window.is_none());
            assert!(focus.running.is_none());
            focus.finish(cx);
            window.remove_window();
        }).unwrap();
        let sessions = store.focus_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].task_id, Some(task.id));
        assert_eq!(sessions[0].title, task.title);
        assert!(sessions[0].stopwatch);
        assert_eq!(sessions[0].ended_at - sessions[0].started_at, 4_500_000);
    }

    #[gpui::test]
    fn separate_fields_preserve_rounded_up_seconds_when_unchanged(cx: &mut TestAppContext) {
        cx.update(|cx| { theme::init(cx); cx.set_global(i18n::CurrentLocale(i18n::Locale::En)); });
        let store = Rc::new(Store::open_in_memory().unwrap());
        let handle = cx.add_window(|_, cx| FocusView::new(store.clone(), cx));
        let review = handle.update(cx, |focus, window, cx| {
            focus.start_for(None, "Rounded session".to_owned(), true, cx);
            let now = now_ms();
            focus.running.as_mut().unwrap().timer = FocusTimer::stopwatch(now - 7_199_000);
            focus.running.as_mut().unwrap().timer.pause(now);
            focus.request_finish(window, cx);
            focus.review_window.unwrap()
        }).unwrap();
        cx.simulate_keystrokes(review.into(), "tab");
        review.update(cx, |review, window, cx| {
            assert!(review.minutes.focus_handle(cx).is_focused(window));
        }).unwrap();
        cx.simulate_keystrokes(review.into(), "shift-tab");
        review.update(cx, |review, window, cx| {
            assert_eq!(review.hours.read(cx).text(), "2");
            assert_eq!(review.minutes.read(cx).text(), "00");
            assert!(review.hours.focus_handle(cx).is_focused(window));
            review.save(window, cx);
        }).unwrap();
        let sessions = store.focus_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].ended_at - sessions[0].started_at, 7_199_000);
        handle.update(cx, |_, window, _| window.remove_window()).unwrap();
    }

    #[gpui::test]
    fn closing_review_restores_pause_and_blocks_other_timer_actions(cx: &mut TestAppContext) {
        cx.update(|cx| { theme::init(cx); cx.set_global(i18n::CurrentLocale(i18n::Locale::En)); });
        for paused in [false, true] {
            let store = Rc::new(Store::open_in_memory().unwrap());
            let handle = cx.add_window(|_, cx| FocusView::new(store.clone(), cx));
            let review = handle.update(cx, |focus, window, cx| {
                focus.start_for(None, "Keep this session".to_owned(), true, cx);
                let now = now_ms();
                focus.running.as_mut().unwrap().timer = FocusTimer::stopwatch(now - 7_506_000);
                if paused { focus.running.as_mut().unwrap().timer.pause(now); }
                focus.request_finish(window, cx);
                let review = focus.review_window.unwrap();
                focus.request_finish(window, cx);
                assert_eq!(focus.review_window, Some(review));
                focus.start_for(None, "Replacement".to_owned(), false, cx);
                focus.finish(cx);
                focus.toggle_pause(cx);
                assert!(focus.running.as_ref().unwrap().timer.is_paused());
                assert!(store.focus_sessions().unwrap().is_empty());
                review
            }).unwrap();
            // Removing the window also covers release without an explicit button action.
            review.update(cx, |_, window, _| window.remove_window()).unwrap();
            cx.run_until_parked();
            handle.update(cx, |focus, window, _| {
                assert!(!focus.finish_prompt);
                assert!(focus.review_window.is_none());
                let running = focus.running.as_ref().unwrap();
                assert_eq!(running.timer.is_paused(), paused);
                assert_eq!(running.task.as_ref().unwrap().1, "Keep this session");
                assert!(store.focus_sessions().unwrap().is_empty());
                window.remove_window();
            }).unwrap();
        }
    }

    #[test]
    fn edited_duration_rejects_invalid_overflow_and_out_of_range_values() {
        assert_eq!(duration(" 1:00 ", 3_601_999), Some(3_601_000));
        assert_eq!(duration("0:05", 3_601_000), Some(MIN_FOCUS_MS));
        for value in ["0:04", "1:01", "-1:00", "1:60", "1:00:00", "::", "1.5:00", "999999999999999:00", "1:999999999999999", "12:01", "13:00"] {
            assert_eq!(duration(value, 3_601_000), None, "{value}");
        }
    }

    #[test]
    fn review_rounds_display_but_preserves_seconds_when_unchanged() {
        for (elapsed, display) in [(3_629_999, "1:00"), (3_630_000, "1:01"), (7_199_000, "2:00"), (43_200_000, "12:00")] {
            assert_eq!(clock(elapsed), display);
            assert_eq!(duration(display, elapsed), Some(elapsed / 1000 * 1000));
        }
        assert_eq!(duration("1:00", 3_630_000), Some(3_600_000));
        assert_eq!(duration("0:59", 3_629_999), Some(3_540_000));
        assert_eq!(duration("12:01", 43_200_000), None);
    }
}
