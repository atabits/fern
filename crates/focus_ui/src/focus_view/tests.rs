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

#[gpui::test]
fn new_inactive_preserves_settings_and_reloads_lazily(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;
    let store = Rc::new(Store::open_in_memory().unwrap());
    store.set_setting("focus_pomo_minutes", "40").unwrap();
    let task = store.add_task("Deep work").unwrap();
    store.set_due(task.id, Some(Local::now().date_naive()), None).unwrap();
    let habit = store.add_habit("Meditate").unwrap();
    let now = now_ms();
    store
        .add_focus_session_as(Some(task.id), "Deep work", now - 600_000, now, false)
        .unwrap();

    cx.update(|cx| {
        let view = cx.new(|cx| FocusView::new_inactive(store.clone(), cx));
        {
            let v = view.read(cx);
            assert!(!v.active);
            assert!(v.dirty);
            assert!(v.tasks.is_empty());
            assert!(v.sessions.is_empty());
            assert!(v.stats_data.tasks.is_empty());
            assert!(v.stats_data_dirty);
            assert_eq!(v.clock_state(now, i18n::t(cx)).0, "40:00");
        }

        // Activating FocusView without opening Stats or Picker avoids full-DB stats_data scans.
        view.update(cx, |v, cx| v.set_active(true, cx));
        {
            let v = view.read(cx);
            assert!(v.active);
            assert!(!v.dirty);
            assert_eq!(v.tasks.len(), 1);
            assert_eq!(v.sessions.len(), 1);
            assert!(v.stats_data.tasks.is_empty());
            assert!(v.stats_data_dirty);
        }

        let view_task = cx.new(|cx| FocusView::new_inactive(store.clone(), cx));
        assert!(view_task.read(cx).dirty);
        view_task.update(cx, |v, cx| {
            v.start_for(Some(task.id), task.title.clone(), false, cx);
        });
        assert!(!view_task.read(cx).dirty);
        assert!(view_task.read(cx).running.is_some());

        let view_habit = cx.new(|cx| FocusView::new_inactive(store.clone(), cx));
        assert!(view_habit.read(cx).dirty);
        view_habit.update(cx, |v, cx| {
            v.start_for_habit(Some(habit.id), habit.name.clone(), true, cx);
        });
        assert!(!view_habit.read(cx).dirty);
        assert!(view_habit.read(cx).running.is_some());

        let view_stats = cx.new(|cx| FocusView::new_inactive(store.clone(), cx));
        assert!(view_stats.read(cx).dirty);
        view_stats.update(cx, |v, cx| v.open_stats_overview(cx));
        assert!(!view_stats.read(cx).dirty);
        assert!(!view_stats.read(cx).stats_data_dirty);
        assert!(view_stats.read(cx).stats.is_some());
        assert_eq!(view_stats.read(cx).stats_data.tasks.len(), 1);
    });
}

#[gpui::test]
fn timer_delete_requires_confirmation_and_supports_escape_and_confirm(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, point, size};

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let timer = fern_core::focus::FocusTimer::new("Deep Reading");
    store.save_focus_timer(&timer).unwrap();
    let (view, cx) = cx.add_window_view(|_, cx| FocusView::new(store.clone(), cx));
    cx.simulate_resize(size(px(1000.), px(700.)));
    cx.update(|window, cx| window.focus(&view.read(cx).focus_handle(cx), cx));
    cx.run_until_parked();

    let more_selector: &'static str =
        Box::leak(format!("focus-timer-more-{}", timer.id).into_boxed_str());
    let more_pos = cx.debug_bounds(more_selector).unwrap().center();
    cx.simulate_mouse_move(more_pos, None, Modifiers::default());
    cx.simulate_click(more_pos, Modifiers::default());
    cx.run_until_parked();

    let delete_pos = cx.debug_bounds("focus-timer-delete-item").unwrap().center();
    cx.simulate_click(delete_pos, Modifiers::default());
    cx.run_until_parked();

    assert_eq!(
        view.read_with(cx, |v, _| v.confirm_delete_timer.clone()),
        Some((timer.id, "Deep Reading".to_owned()))
    );
    assert_eq!(store.focus_timers(false).unwrap().len(), 1);
    let modal_bounds = cx
        .debug_bounds("focus-timer-delete-modal")
        .expect("timer delete confirmation modal");

    // Space hotkey must not start a timer while confirmation dialog is open.
    cx.simulate_keystrokes("space");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| !v.is_running()));

    // Escape / DismissTransient cancels without deleting from SQLite.
    cx.update(|window, cx| {
        window.dispatch_action(Box::new(crate::actions::DismissTransient), cx);
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.confirm_delete_timer.clone()), None);
    assert!(cx.debug_bounds("focus-timer-delete-modal").is_none());
    assert_eq!(store.focus_timers(false).unwrap().len(), 1);

    // Re-trigger timer delete and confirm.
    cx.simulate_click(more_pos, Modifiers::default());
    cx.run_until_parked();
    let delete_pos = cx.debug_bounds("focus-timer-delete-item").unwrap().center();
    cx.simulate_click(delete_pos, Modifiers::default());
    cx.run_until_parked();

    let confirm_pos = point(modal_bounds.center().x + px(120.), modal_bounds.center().y + px(34.));
    cx.simulate_click(confirm_pos, Modifiers::default());
    cx.run_until_parked();

    assert_eq!(view.read_with(cx, |v, _| v.confirm_delete_timer.clone()), None);
    assert!(cx.debug_bounds("focus-timer-delete-modal").is_none());
    assert!(store.focus_timers(false).unwrap().is_empty());
    assert!(view.read_with(cx, |v, _| v.timers.is_empty()));
}

#[gpui::test]
fn delete_all_requires_confirmation_and_record_delete_bumps_data_version(cx: &mut gpui::TestAppContext) {
    use gpui::{AppContext, Modifiers, size};

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let now = now_ms();
    let s1 = store.add_focus_session(None, "First session", now - 900_000, now - 600_000).unwrap();
    store.add_focus_session(None, "Second session", now - 300_000, now).unwrap();

    let (view, cx) = cx.add_window_view(|_, cx| FocusView::new(store.clone(), cx));
    cx.simulate_resize(size(px(1000.), px(700.)));
    let other = cx.update(|window, cx| {
        window.focus(&view.read(cx).focus_handle(cx), cx);
        let other = cx.new(|_| ());
        desktop_common::data_version::data_changed(other.entity_id(), cx);
        other
    });
    cx.run_until_parked();

    // Deleting a single record bumps DataVersion.
    view.update(cx, |v, cx| v.delete_record(s1.id, cx));
    cx.run_until_parked();
    assert_eq!(store.focus_sessions().unwrap().len(), 1);
    assert!(cx.update(|_, cx| desktop_common::data_version::changed_elsewhere(other.entity_id(), cx)));

    // Reset DataVersion marker, then open records "More" menu -> "Delete All".
    cx.update(|_, cx| desktop_common::data_version::data_changed(other.entity_id(), cx));
    let more_pos = cx.debug_bounds("focus-record-more").unwrap().center();
    cx.simulate_click(more_pos, Modifiers::default());
    cx.run_until_parked();
    let delete_all_pos = cx.debug_bounds("focus-delete-all-item").unwrap().center();
    cx.simulate_click(delete_all_pos, Modifiers::default());
    cx.run_until_parked();

    view.read_with(cx, |v, _| {
        assert!(v.record_menu.is_none());
        assert!(v.confirm_delete_all);
    });
    assert!(cx.debug_bounds("focus-delete-all-modal").is_some());
    assert_eq!(store.focus_sessions().unwrap().len(), 1, "sessions must remain before confirmation");

    // Space hotkey must not start a timer while confirmation dialog is open.
    cx.simulate_keystrokes("space");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| !v.is_running()));

    // Escape / DismissTransient cancels without deleting.
    cx.update(|window, cx| {
        window.dispatch_action(Box::new(crate::actions::DismissTransient), cx);
    });
    cx.run_until_parked();
    assert!(!view.read_with(cx, |v, _| v.confirm_delete_all));
    assert!(cx.debug_bounds("focus-delete-all-modal").is_none());
    assert_eq!(store.focus_sessions().unwrap().len(), 1);

    // Re-trigger Delete All and confirm -> deletes all sessions and bumps DataVersion.
    cx.simulate_click(more_pos, Modifiers::default());
    cx.run_until_parked();
    let delete_all_pos = cx.debug_bounds("focus-delete-all-item").unwrap().center();
    cx.simulate_click(delete_all_pos, Modifiers::default());
    cx.run_until_parked();
    let modal_bounds = cx.debug_bounds("focus-delete-all-modal").unwrap();
    let confirm_pos = gpui::point(modal_bounds.center().x + px(125.), modal_bounds.center().y + px(38.));
    cx.simulate_click(confirm_pos, Modifiers::default());
    cx.run_until_parked();

    assert!(!view.read_with(cx, |v, _| v.confirm_delete_all));
    assert!(cx.debug_bounds("focus-delete-all-modal").is_none());
    assert!(store.focus_sessions().unwrap().is_empty());
    assert!(view.read_with(cx, |v, _| v.sessions.is_empty()));
    assert!(cx.update(|_, cx| desktop_common::data_version::changed_elsewhere(other.entity_id(), cx)));
}

#[gpui::test]
fn escape_inside_record_note_and_add_inputs_closes_modal_and_restores_focus(cx: &mut gpui::TestAppContext) {
    use gpui::{KeyBinding, size};

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        cx.bind_keys([
            KeyBinding::new("escape", crate::text_input::Cancel, Some("TextInput")),
            KeyBinding::new("escape", crate::actions::DismissTransient, None),
        ]);
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let now = now_ms();
    let session = store.add_focus_session(None, "Review notes", now - 600_000, now).unwrap();
    let (view, cx) = cx.add_window_view(|_, cx| FocusView::new(store.clone(), cx));
    cx.simulate_resize(size(px(1000.), px(700.)));
    cx.run_until_parked();

    // Opening a record focuses record_note; Escape (Cancel) saves/closes and restores FocusView focus.
    cx.update(|window, cx| view.update(cx, |v, cx| v.open_record(session.id, window, cx)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert_eq!(v.open_record, Some(session.id));
        assert!(v.record_note.focus_handle(cx).is_focused(window));
    });

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert_eq!(v.open_record, None);
        assert!(v.focus_handle(cx).is_focused(window));
    });

    // Opening Add Record and focusing add_title or add_start: Escape closes popover and restores focus.
    for target_start in [false, true] {
        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.set_record_menu(Some(RecordMenu::Add), cx);
                let input = if target_start { &v.add_start } else { &v.add_title };
                window.focus(&input.focus_handle(cx), cx);
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let v = view.read(cx);
            assert!(v.record_menu.is_none());
            assert!(v.focus_handle(cx).is_focused(window));
        });
    }

    // DismissTransient from FocusView also closes RecordMenu::Add cleanly without losing focus.
    cx.update(|_, cx| view.update(cx, |v, cx| v.set_record_menu(Some(RecordMenu::Add), cx)));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert!(v.record_menu.is_none());
        assert!(v.focus_handle(cx).is_focused(window));
    });
}

#[gpui::test]
fn single_record_delete_requires_confirmation_and_batch_custom_closes_on_escape(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::{KeyBinding, Modifiers, point, size};

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        cx.bind_keys([
            KeyBinding::new("escape", crate::text_input::Cancel, Some("TextInput")),
            KeyBinding::new("escape", crate::actions::DismissTransient, None),
        ]);
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let now = now_ms();
    let session = store
        .add_focus_session(None, "Deep writing", now - 1_500_000, now)
        .unwrap();
    let (view, cx) = cx.add_window_view(|_, cx| FocusView::new(store.clone(), cx));
    cx.simulate_resize(size(px(1000.), px(700.)));
    cx.run_until_parked();

    // Open record, click trash button -> ConfirmDialog overlays while keeping open_record intact.
    cx.update(|window, cx| view.update(cx, |v, cx| v.open_record(session.id, window, cx)));
    cx.run_until_parked();
    let trash_pos = cx.debug_bounds("delete-open-record").unwrap().center();
    cx.simulate_click(trash_pos, Modifiers::default());
    cx.run_until_parked();

    cx.update(|window, cx| {
        let v = view.read(cx);
        assert_eq!(
            v.confirm_delete_record.clone(),
            Some((session.id, "Deep writing · 25m".to_owned()))
        );
        assert_eq!(v.open_record, Some(session.id));
        assert!(v.focus_handle(cx).is_focused(window));
    });
    assert!(cx.debug_bounds("focus-record-delete-modal").is_some());
    assert_eq!(store.focus_sessions().unwrap().len(), 1);

    // Space hotkey must not start a timer while record delete confirmation is open.
    cx.simulate_keystrokes("space");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| !v.is_running()));

    // First Escape cancels only the ConfirmDialog, preserving open_record and restoring note focus.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert_eq!(v.confirm_delete_record, None);
        assert_eq!(v.open_record, Some(session.id));
        assert!(v.record_note.focus_handle(cx).is_focused(window));
    });
    assert!(cx.debug_bounds("focus-record-delete-modal").is_none());
    assert_eq!(store.focus_sessions().unwrap().len(), 1);

    // Re-trigger delete and confirm -> deletes record, closes open_record, restores view focus.
    let trash_pos = cx.debug_bounds("delete-open-record").unwrap().center();
    cx.simulate_click(trash_pos, Modifiers::default());
    cx.run_until_parked();
    let modal_bounds = cx.debug_bounds("focus-record-delete-modal").unwrap();
    let confirm_pos = point(modal_bounds.center().x + px(126.), modal_bounds.center().y + px(34.));
    cx.simulate_click(confirm_pos, Modifiers::default());
    cx.run_until_parked();

    cx.update(|window, cx| {
        let v = view.read(cx);
        assert_eq!(v.confirm_delete_record, None);
        assert_eq!(v.open_record, None);
        assert!(v.sessions.is_empty());
        assert!(v.focus_handle(cx).is_focused(window));
    });
    assert!(cx.debug_bounds("focus-record-delete-modal").is_none());
    assert!(store.focus_sessions().unwrap().is_empty());

    // Escape inside either batch_custom TextInput closes the popover and restores FocusView focus.
    cx.update(|_, cx| view.update(cx, |v, cx| v.set_record_menu(Some(RecordMenu::Batch), cx)));
    cx.run_until_parked();
    for (which, focus_second) in [(0, false), (1, true)] {
        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.open_batch_custom(which, window, cx);
                if focus_second && let Some((_, _, second)) = &v.batch_custom {
                    window.focus(&second.focus_handle(cx), cx);
                }
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let v = view.read(cx);
            assert!(v.batch_custom.is_none());
            assert_eq!(v.record_menu, Some(RecordMenu::Batch));
            assert!(v.focus_handle(cx).is_focused(window));
        });
    }
}
