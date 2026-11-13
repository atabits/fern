//! Ending a focus session is an explicit save/discard/cancel operation.
use super::*;

// TryAbandonPomo / TryStopTiming / StopTiming delegate, RVAs
// 0x00381f40 / 0x00382308 / 0x005af39c.
const STOP_CONFIRMATION_MS: i64 = 30_000;
const STOPWATCH_DIRECT_SAVE_MS: i64 = 3_600_000;
// OnFTimerElapsed nested MoveNext, RVA 0x005aef4c.
pub(super) const STOPWATCH_LIMIT_MS: i64 = 43_200 * 1000;

impl FocusView {
    /// Keep the stopped session available for review without recording extra time.
    pub(super) fn stop_at_stopwatch_limit(&mut self, now: i64, cx: &mut Context<Self>) -> bool {
        let Some(running) = self.running.as_mut() else { return false };
        if running.on_break.is_some() || !running.timer.is_stopwatch() { return false; }
        let elapsed = running.timer.elapsed(now);
        if elapsed < STOPWATCH_LIMIT_MS { return false; }
        if !running.timer.is_paused() || elapsed > STOPWATCH_LIMIT_MS {
            // A delayed tick can arrive after the deadline; retain prior pause time.
            running.timer.resume(now);
            running.timer.pause(now - (elapsed - STOPWATCH_LIMIT_MS));
            self.message = Some(i18n::t(cx).focus_limit_notice.to_owned());
            cx.notify();
        }
        true
    }

    pub(super) fn interval(&self) -> usize {
        self.minutes(("focus_long_break_every", DEFAULT_LONG_BREAK_EVERY as i64, 1, 60)) as usize
    }

    pub(super) fn automatic(&self, key: &str) -> bool {
        self.store.setting(key).ok().flatten().as_deref() == Some("1")
    }

    pub(super) fn render_automation(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let number = |id: &'static str, title: &'static str, setting: (&'static str, i64, i64, i64), cx: &mut Context<Self>| {
            h_flex().gap_2()
                .child(div().flex_1().child(Label::new(title).size(LabelSize::Small)))
                .child(IconButton::new(SharedString::from(format!("{id}-less")), IconName::Minus)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_minutes(setting, -1, cx))))
                .child(Label::new(self.minutes(setting).to_string()))
                .child(IconButton::new(SharedString::from(format!("{id}-more")), IconName::Plus)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_minutes(setting, 1, cx))))
        };
        let toggle = |key: &'static str, title: &'static str, cx: &mut Context<Self>| {
            h_flex().id(key).min_h(px(32.)).gap_2().cursor_pointer()
                .child(div().flex_1().child(Label::new(title).size(LabelSize::Small)))
                .child(Switch::new().toggle_state(self.automatic(key)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let value = if this.automatic(key) { "0" } else { "1" };
                    if let Err(err) = this.store.set_setting(key, value) {
                        eprintln!("failed to save the focus setting: {err:#}");
                        this.message = Some(i18n::t(cx).focus_settings_save_failed.to_owned());
                    }
                    cx.notify();
                }))
        };
        v_flex().gap_2()
            .child(number("long-break-every", strings.focus_long_break_every, ("focus_long_break_every", 4, 1, 60), cx))
            .child(toggle("focus_auto_break", strings.focus_auto_break, cx))
            .child(toggle("focus_auto_next", strings.focus_auto_next, cx))
            .child(number("auto-pomo-count", strings.focus_auto_count, ("focus_auto_count", 4, 2, 10), cx))
            .into_any_element()
    }

    pub fn request_finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let limited = self.stop_at_stopwatch_limit(now_ms(), cx);
        let Some(running) = self.running.as_mut() else { return };
        if self.finish_prompt {
            if let Some(handle) = self.review_window {
                handle.update(cx, |_, window, _| window.activate_window()).ok();
            }
            return;
        }
        if running.on_break.is_some() { self.finish(cx); return; }
        let now = now_ms();
        let elapsed = running.timer.elapsed(now);
        if elapsed < STOP_CONFIRMATION_MS
            || (running.timer.is_stopwatch() && (MIN_FOCUS_MS..=STOPWATCH_DIRECT_SAVE_MS).contains(&elapsed)) {
            self.finish(cx);
            return;
        }
        let paused = running.timer.is_paused();
        running.timer.pause(now);
        if running.timer.is_stopwatch() && elapsed > STOPWATCH_DIRECT_SAVE_MS {
            let title = running.task.as_ref().map(|(_, title)| title.clone()).unwrap_or_else(|| i18n::t(cx).focus.to_owned());
            self.open_stopwatch_review(title, elapsed, paused, limited, window, cx);
            return;
        }
        let enough = elapsed >= MIN_FOCUS_MS;
        let strings = i18n::t(cx);
        let (title, detail, answers) = match (enough, limited) {
            (_, true) => (
                strings.focus_limit_title,
                strings.focus_end_limit_detail,
                vec![strings.save, strings.focus_discard, strings.focus_later],
            ),
            (true, false) => (
                strings.focus_end_question,
                strings.focus_end_save_detail,
                vec![strings.save, strings.focus_discard, strings.focus_continue],
            ),
            (false, false) => (
                strings.focus_end_question,
                strings.focus_end_short_detail,
                vec![strings.focus_end, strings.focus_continue],
            ),
        };
        let chosen = window.prompt(gpui::PromptLevel::Info, title, Some(detail), &answers, cx);
        self.finish_prompt = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let choice = chosen.await.ok();
            this.update(cx, |this, cx| {
                this.finish_prompt = false;
                match (enough, choice) {
                    (_, Some(0)) => this.finish(cx),
                    (true, Some(1)) => {
                        this.running = None;
                        this.continuation = None;
                        this.cycle.reset();
                        if limited { this.message = None; }
                    }
                    _ => {
                        if !paused && let Some(running) = this.running.as_mut() { running.timer.resume(now_ms()); }
                    }
                }
                cx.notify();
            }).ok();
        }).detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    fn complete_current(view: &mut FocusView, cx: &mut Context<FocusView>) {
        let running = view.running.as_mut().unwrap();
        let length = running.timer.length_ms;
        running.timer = FocusTimer::start(now_ms() - length - 100, length);
        view.tick(cx);
    }

    #[gpui::test]
    fn stopwatch_limit_excludes_pauses_and_caps_delayed_ticks(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = Rc::new(Store::open_in_memory().unwrap());
            let view = cx.new(|cx| FocusView::new(store.clone(), cx));
            view.update(cx, |view, cx| {
                view.start_for(None, "Long session".to_owned(), true, cx);
                let start = now_ms() - STOPWATCH_LIMIT_MS - 120_000;
                let timer = &mut view.running.as_mut().unwrap().timer;
                *timer = FocusTimer::stopwatch(start);
                timer.pause(start + 60_000);
                timer.resume(start + 120_000);
                let deadline = start + 60_000 + STOPWATCH_LIMIT_MS;
                assert!(!view.stop_at_stopwatch_limit(deadline - 1, cx));
                assert!(!view.running.as_ref().unwrap().timer.is_paused());
                let original = view.running.as_ref().unwrap().timer;
                assert!(view.stop_at_stopwatch_limit(deadline, cx));
                assert_eq!(view.running.as_ref().unwrap().timer.elapsed(deadline), STOPWATCH_LIMIT_MS);
                view.running.as_mut().unwrap().timer = original;
                assert!(view.stop_at_stopwatch_limit(deadline + 45_000, cx));
                let timer = &view.running.as_ref().unwrap().timer;
                assert!(timer.is_paused());
                assert_eq!(timer.elapsed(deadline + 90_000), STOPWATCH_LIMIT_MS);
                view.toggle_pause(cx);
                assert!(view.running.as_ref().unwrap().timer.is_paused());
                assert!(store.focus_sessions().unwrap().is_empty());
                view.tick(cx);
                assert!(view.running.as_ref().unwrap().on_break.is_none());
                view.finish(cx);
                let sessions = store.focus_sessions().unwrap();
                assert_eq!(sessions.len(), 1);
                assert!(sessions[0].stopwatch);
                assert_eq!(sessions[0].ended_at - sessions[0].started_at, STOPWATCH_LIMIT_MS);
                view.tick(cx);
                assert!(view.running.is_none());
                assert!(view.message.is_none());
                assert_eq!(store.focus_sessions().unwrap().len(), 1);
            });
        });
    }

    #[gpui::test]
    async fn limited_stopwatch_cannot_cancel_review_and_can_save_or_discard(cx: &mut TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        });
        for answer in ["Save", "Discard"] {
            let store = Rc::new(Store::open_in_memory().unwrap());
            let handle = cx.add_window(|_, cx| FocusView::new(store.clone(), cx));
            handle.update(cx, |view, window, cx| {
                view.start_for(None, "Long session".to_owned(), true, cx);
                view.running.as_mut().unwrap().timer = FocusTimer::stopwatch(now_ms() - STOPWATCH_LIMIT_MS - 1000);
                view.tick(cx);
                assert!(view.running.as_ref().unwrap().timer.is_paused());
                assert_eq!(view.running.as_ref().unwrap().timer.elapsed(now_ms()), STOPWATCH_LIMIT_MS);
                view.request_finish(window, cx);
            }).unwrap();
            let review = handle.update(cx, |view, _, _| view.review_window.unwrap()).unwrap();
            review.update(cx, |review, window, cx| review.cancel(window, cx)).unwrap();
            handle.update(cx, |view, window, cx| {
                assert!(view.finish_prompt);
                assert_eq!(view.review_window, Some(review));
                view.toggle_pause(cx);
                assert!(view.running.as_ref().unwrap().timer.is_paused());
                assert!(store.focus_sessions().unwrap().is_empty());
                view.request_finish(window, cx);
            }).unwrap();
            let review = handle.update(cx, |view, _, _| view.review_window.unwrap()).unwrap();
            review.update(cx, |review, window, cx| {
                if answer == "Save" { review.save(window, cx); } else { review.discard(window, cx); }
            }).unwrap();
            handle.update(cx, |view, window, _| {
                assert!(view.running.is_none());
                assert!(view.message.is_none());
                assert_eq!(store.focus_sessions().unwrap().len(), usize::from(answer == "Save"));
                if answer == "Save" {
                    let sessions = store.focus_sessions().unwrap();
                    assert_eq!(sessions[0].ended_at - sessions[0].started_at, STOPWATCH_LIMIT_MS);
                }
                window.remove_window();
            }).unwrap();
        }
    }

    #[gpui::test]
    async fn stop_confirmation_matches_original_boundaries(cx: &mut TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::ZhCn));
        });
        // Pause at exact elapsed boundaries so wall-clock scheduling cannot change them.
        for (stopwatch, seconds, prompt, saved) in [
            (false, 29, false, false),
            (false, 30, true, false),
            (false, 300, true, false),
            (true, 29, false, false),
            (true, 30, true, false),
            (true, 299, true, false),
            (true, 300, false, true),
            (true, 3600, false, true),
            (true, 3601, true, false),
        ] {
            let store = Rc::new(Store::open_in_memory().unwrap());
            let handle = cx.add_window(|_, cx| FocusView::new(store.clone(), cx));
            handle.update(cx, |view, window, cx| {
                view.start_for(None, "Boundary session".to_owned(), stopwatch, cx);
                let now = now_ms();
                let timer = &mut view.running.as_mut().unwrap().timer;
                *timer = if stopwatch { FocusTimer::stopwatch(now - seconds * 1000) }
                    else { FocusTimer::start(now - seconds * 1000, 25 * 60_000) };
                timer.pause(now);
                view.request_finish(window, cx);
                assert_eq!(view.finish_prompt, prompt, "stopwatch={stopwatch}, seconds={seconds}");
                assert_eq!(view.running.is_some(), prompt);
            }).unwrap();
            let review = handle.update(cx, |view, _, _| view.review_window).unwrap();
            assert_eq!(cx.has_pending_prompt(), prompt && review.is_none());
            if let Some(review) = review {
                review.update(cx, |review, window, cx| review.cancel(window, cx)).unwrap();
                handle.update(cx, |view, _, _| assert!(view.running.as_ref().unwrap().timer.is_paused())).unwrap();
            } else if prompt {
                cx.simulate_prompt_answer("继续");
                cx.run_until_parked();
                handle.update(cx, |view, _, _| {
                    assert!(view.running.as_ref().unwrap().timer.is_paused());
                }).unwrap();
            }
            let sessions = store.focus_sessions().unwrap();
            assert_eq!(sessions.len(), usize::from(saved));
            if saved {
                assert!(sessions[0].stopwatch);
                assert_eq!(sessions[0].ended_at - sessions[0].started_at, seconds * 1000);
            }
            handle.update(cx, |_, window, _| window.remove_window()).unwrap();
        }
    }

    #[gpui::test]
    async fn ending_can_continue_discard_or_save(cx: &mut TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        });
        for answer in ["Continue", "Discard", "Save"] {
            let store = Rc::new(Store::open_in_memory().unwrap());
            let handle = cx.add_window(|_, cx| FocusView::new(store.clone(), cx));
            handle.update(cx, |view, window, cx| {
                view.start(None, cx);
                view.running.as_mut().unwrap().timer = FocusTimer::start(now_ms() - MIN_FOCUS_MS - 1000, 25 * 60_000);
                view.request_finish(window, cx);
                assert!(view.running.as_ref().unwrap().timer.is_paused());
            }).unwrap();
            assert!(cx.has_pending_prompt());
            cx.simulate_prompt_answer(answer);
            cx.run_until_parked();
            handle.update(cx, |view, _, _| {
                assert!(!view.finish_prompt);
                match answer {
                    "Continue" => assert!(!view.running.as_ref().unwrap().timer.is_paused()),
                    "Discard" => { assert!(view.running.is_none()); assert!(store.focus_sessions().unwrap().is_empty()); }
                    "Save" => { assert!(view.running.is_none()); assert!(!store.focus_sessions().unwrap().is_empty()); }
                    _ => unreachable!(),
                }
            }).unwrap();
        }
    }

    #[gpui::test]
    fn default_break_waits_for_user(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = Rc::new(Store::open_in_memory().unwrap());
            let view = cx.new(|cx| FocusView::new(store.clone(), cx));
            view.update(cx, |view, cx| {
                view.start(None, cx);
                complete_current(view, cx);
                let running = view.running.as_ref().unwrap();
                assert_eq!(running.on_break, Some(BreakKind::Short));
                assert!(running.timer.is_paused());
                assert_eq!(store.focus_sessions().unwrap().len(), 1);
                complete_current(view, cx);
                assert!(view.running.is_none());
            });
        });
    }

    #[gpui::test]
    fn automatic_chain_keeps_task_and_respects_limit_and_interval(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = Rc::new(Store::open_in_memory().unwrap());
            for (key, value) in [("focus_auto_break", "1"), ("focus_auto_next", "1"), ("focus_auto_count", "2"), ("focus_long_break_every", "2")] {
                store.set_setting(key, value).unwrap();
            }
            let task = store.add_task("Deep work").unwrap();
            let view = cx.new(|cx| FocusView::new(store.clone(), cx));
            view.update(cx, |view, cx| {
                view.start(Some((task.id, task.title.clone())), cx);
                complete_current(view, cx);
                assert!(!view.running.as_ref().unwrap().timer.is_paused());
                complete_current(view, cx);
                assert_eq!(view.running.as_ref().unwrap().task, Some((Some(task.id), task.title)));
                assert_eq!(view.running.as_ref().unwrap().on_break, None);
                complete_current(view, cx);
                assert_eq!(view.running.as_ref().unwrap().on_break, Some(BreakKind::Long));
                complete_current(view, cx);
                assert!(view.running.is_none());
                assert_eq!(store.focus_sessions().unwrap().len(), 2);
                assert!(store.focus_sessions().unwrap().iter().all(|s| s.task_id == Some(task.id)));
            });
        });
    }
}
