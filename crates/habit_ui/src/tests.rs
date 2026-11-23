use super::*;
use fern_core::habit::{Goal, HabitDraft};
use gpui::{TestAppContext, size};

#[gpui::test]
fn restoring_the_last_archived_habit_shows_its_weekly_progress_only_in_active(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = Local::now().date_naive();
    let habit = store.add_habit("Read").unwrap();
    assert!(store.toggle_checkin(habit.id, today).unwrap());
    store.set_habit_archived(habit.id, true).unwrap();

    cx.update(theme::init);
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut view = HabitView::new(store.clone(), cx);
        view.show_archived = true;
        view.selected = Some(habit.id);
        view
    });
    assert!(cx.debug_bounds("habit-week-progress").is_none());

    // Archived habits are strictly read-only: clicking a dot does nothing.
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.handle_dot_click(habit.id, today, window, cx));
    });
    assert!(store.checkins(habit.id).unwrap().contains(&today));

    cx.update(|_, cx| {
        view.update(cx, |view, cx| view.set_archived(habit.id, false, cx));
    });
    assert!(cx.debug_bounds("habit-week-progress").is_none());
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert!(view.show_archived);
        assert!(view.habits.iter().all(|habit| !habit.archived));
        assert!(view.selected.is_none());
    });

    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.show_archived = false;
            cx.notify();
        });
    });
    assert!(cx.debug_bounds("habit-week-progress").is_some());
    cx.update(|_, cx| assert_eq!(view.read(cx).day_progress(today), DayProgress::Fraction(1.0)));
}

#[gpui::test]
fn edit_modal_preserves_unset_start_date_and_navigates_calendar(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let habit = store.add_habit("Stretch").unwrap();
    assert_eq!(habit.start_date, None);

    cx.update(theme::init);
    let (view, cx) = cx.add_window_view(|_, cx| HabitView::new(store.clone(), cx));

    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_edit_modal(habit.id, window, cx);
            assert_eq!(view.editing.as_ref().unwrap().start_date, None);
            view.request_save_edit(cx);
        });
    });
    // Saving without picking a start date must NOT silently overwrite start_date with Some(today).
    assert_eq!(store.habits().unwrap()[0].start_date, None);

    // Opening StartDate row initializes start_calendar so month shifting works.
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_edit_modal(habit.id, window, cx);
            let today = Local::now().date_naive();
            view.update_edit(cx, |e| {
                e.open = Some(modal::EditRow::StartDate);
                e.start_calendar = Some(calendar::month_start(today));
            });
            let initial = view.editing.as_ref().unwrap().start_calendar.unwrap();
            view.update_edit(cx, |e| {
                e.start_calendar = Some(calendar::add_months(initial, -1));
            });
            assert_ne!(view.editing.as_ref().unwrap().start_calendar.unwrap(), initial);
            assert!(view.dismiss_transient(cx));
        });
    });
}

#[gpui::test]
fn checkin_modes_and_reset_confirmation(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = Local::now().date_naive();
    let water = store
        .create_habit(&HabitDraft {
            name: "Water".into(),
            goal: Some(Goal { amount: 2.0, unit: "L".into(), mode: CheckinMode::Auto { step: 1.0 } }),
            ..Default::default()
        })
        .unwrap();

    cx.update(theme::init);
    let (view, cx) = cx.add_window_view(|_, cx| HabitView::new(store.clone(), cx));

    // Step 1 -> 1.0 (partial), Step 2 -> 2.0 (Done), Step 3 -> opens Reset confirm modal!
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_dot_click(water.id, today, window, cx);
            assert!(!view.checkins.get(&water.id).unwrap().contains(&today));
            view.handle_dot_click(water.id, today, window, cx);
            assert!(view.checkins.get(&water.id).unwrap().contains(&today));
            view.handle_dot_click(water.id, today, window, cx);
            assert!(matches!(view.confirm, Some(ConfirmAction::ResetCheckin { .. })));
            view.execute_confirm(cx);
            assert!(!view.checkins.get(&water.id).unwrap().contains(&today));
        });
    });
}

#[gpui::test]
fn day_menu_enforces_90_day_limit_and_calendar_blocks_archived_quantifiable_habits(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = Local::now().date_naive();
    let active = store.add_habit("Meditate").unwrap();
    let archived_goal = store
        .create_habit(&HabitDraft {
            name: "Hydrate".into(),
            goal: Some(Goal { amount: 2.0, unit: "L".into(), mode: CheckinMode::Manual }),
            ..Default::default()
        })
        .unwrap();
    store.set_habit_archived(archived_goal.id, true).unwrap();

    cx.update(theme::init);
    let (view, cx) = cx.add_window_view(|_, cx| HabitView::new(store.clone(), cx));

    // Right-clicking a day older than MAX_EDITABLE_PAST_DAYS must NOT open day_menu and sets detail_notice.
    let too_old = today - Duration::days(habit::MAX_EDITABLE_PAST_DAYS + 1);
    let within_limit = today - Duration::days(habit::MAX_EDITABLE_PAST_DAYS);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_day_menu(active.id, too_old, gpui::point(px(10.), px(10.)), cx);
            assert!(view.day_menu.is_none());
            assert!(view.detail_notice.is_some());

            // Right-clicking at the exact boundary succeeds and clears the notice.
            view.open_day_menu(active.id, within_limit, gpui::point(px(10.), px(10.)), cx);
            assert!(view.day_menu.is_some());
            assert!(view.detail_notice.is_none());

            // Clicking calendar day on an archived quantifiable habit must NOT open amount_modal.
            view.handle_detail_calendar_click(archived_goal.id, today, true, window, cx);
            assert!(view.amount_modal.is_none());
        });
    });
}

#[gpui::test]
fn amount_modal_preserves_typo_and_auto_pops_log_on_completion(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = Local::now().date_naive();
    let run = store
        .create_habit(&HabitDraft {
            name: "Run".into(),
            goal: Some(Goal { amount: 5.0, unit: "km".into(), mode: CheckinMode::Manual }),
            record_enable: true,
            ..Default::default()
        })
        .unwrap();

    cx.update(theme::init);
    let (view, cx) = cx.add_window_view(|_, cx| HabitView::new(store.clone(), cx));

    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_amount_modal(run.id, today, true, window, cx);
            let input = view.amount_modal.as_ref().unwrap().input.clone();
            input.update(cx, |inp, cx| inp.set_text("5..km", cx));

            // Submitting invalid text keeps the modal open and preserves the typed string.
            view.submit_amount_modal(Some(window), cx);
            let still_open = view.amount_modal.as_ref().expect("modal must remain open on invalid input");
            assert_eq!(still_open.input.read(cx).text(), "5..km");
            assert!(!view.checkins.get(&run.id).unwrap().contains(&today));
            assert!(view.log_modal.is_none());

            // Fixing the typo and submitting completes the habit and auto-pops Habit Log.
            input.update(cx, |inp, cx| inp.set_text("5", cx));
            view.submit_amount_modal(Some(window), cx);
            assert!(view.amount_modal.is_none());
            assert!(view.checkins.get(&run.id).unwrap().contains(&today));
            let log = view.log_modal.as_ref().expect("auto-pop Habit Log should open");
            assert!(log.auto_popped);
            assert_eq!(log.habit_id, run.id);
            assert_eq!(log.date, today);
        });
    });
}

#[gpui::test]
fn new_inactive_defers_reload_until_activated_or_selected(cx: &mut TestAppContext) {
    use gpui::AppContext;
    let store = Rc::new(Store::open_in_memory().unwrap());
    let active_habit = store.add_habit("Read").unwrap();
    let archived_habit = store.add_habit("Run").unwrap();
    store.set_habit_archived(archived_habit.id, true).unwrap();

    cx.update(|cx| {
        let view = cx.new(|cx| HabitView::new_inactive(store.clone(), cx));
        {
            let v = view.read(cx);
            assert!(!v.active);
            assert!(v.dirty);
            assert!(v.habits.is_empty());
        }

        view.update(cx, |v, cx| v.set_active(true, cx));
        let v = view.read(cx);
        assert!(v.active);
        assert!(!v.dirty);
        assert_eq!(v.habits.len(), 2);

        let view_select = cx.new(|cx| HabitView::new_inactive(store.clone(), cx));
        assert!(view_select.read(cx).dirty);
        view_select.update(cx, |v, cx| v.select(archived_habit.id, cx));
        let vs = view_select.read(cx);
        assert!(!vs.dirty);
        assert!(vs.show_archived);
        assert_eq!(vs.selected, Some(archived_habit.id));
        assert!(vs.habits.iter().any(|h| h.id == active_habit.id));
    });
}

#[gpui::test]
fn select_relative_follows_visual_section_order_skips_collapsed_and_scrolls(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    // Created in reverse section order so DB sort_order != visual section order.
    let night = store
        .create_habit(&HabitDraft { name: "Night Read".into(), section: HabitSection::Night, ..Default::default() })
        .unwrap();
    let morning = store
        .create_habit(&HabitDraft { name: "Morning Walk".into(), section: HabitSection::Morning, ..Default::default() })
        .unwrap();
    let mut extra = Vec::new();
    for i in 0..8 {
        extra.push(
            store
                .create_habit(&HabitDraft { name: format!("Other {i}"), section: HabitSection::Others, ..Default::default() })
                .unwrap(),
        );
    }

    cx.update(theme::init);
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut view = HabitView::new(store.clone(), cx);
        view.set_list_layout(true, cx);
        view.collapsed_sections.insert(HabitSection::Night.key());
        view
    });
    cx.simulate_resize(size(px(840.), px(280.)));
    cx.run_until_parked();

    // First selection starts at Morning (not Night, which is collapsed and later in section order).
    cx.update(|_, cx| {
        view.update(cx, |v, cx| {
            v.select_relative(1, cx);
            assert_eq!(v.selected, Some(morning.id));
            // Next moves straight into Others, skipping collapsed Night.
            v.select_relative(1, cx);
            assert_eq!(v.selected, Some(extra[0].id));
            assert_ne!(v.selected, Some(night.id));
        });
    });

    // Stepping to the bottom overflows the short viewport and scrolls the handle down.
    cx.update(|_, cx| {
        view.update(cx, |v, cx| {
            for _ in 0..extra.len() {
                v.select_relative(1, cx);
            }
            assert_eq!(v.selected, Some(extra.last().unwrap().id));
            assert!(v.grid_scroll.offset().y < px(0.));
        });
    });
}

#[gpui::test]
fn modal_blocks_background_habit_keyboard_actions(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let first = store.add_habit("First").unwrap();
    let _second = store.add_habit("Second").unwrap();

    cx.update(theme::init);
    let (view, cx) = cx.add_window_view(|_, cx| {
        let mut view = HabitView::new(store.clone(), cx);
        view.selected = Some(first.id);
        view.confirm = Some(ConfirmAction::DeleteHabit { habit_id: first.id });
        view
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        let handle = view.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
    });
    cx.dispatch_action(actions::SelectNextHabit);
    cx.dispatch_action(actions::CheckInSelectedHabit);
    cx.dispatch_action(actions::NewHabit);

    cx.update(|_, cx| {
        let v = view.read(cx);
        assert_eq!(v.selected, Some(first.id));
        assert!(v.checkins.get(&first.id).is_none_or(HashSet::is_empty));
        assert!(v.editing.is_none());
    });
}
