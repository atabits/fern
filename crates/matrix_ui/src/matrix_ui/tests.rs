use chrono::Duration;
use desktop_common::keymap;
use fern_core::matrix::MatrixDate;
use fern_core::{Priority, Store};
use gpui::EntityInputHandler;
use task_ui::task_rows::{GroupBy, SortBy};

use super::*;

#[gpui::test]
fn long_quadrant_input_stays_visible_and_saves_in_full(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        keymap::load_default(&assets::Assets, cx).unwrap();
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = MatrixView::new(store.clone(), cx);
        view.start_add(0, window, cx);
        view
    });
    cx.simulate_resize(gpui::size(px(700.), px(500.)));
    let empty_field = cx.debug_bounds("matrix-add-input-0").unwrap();
    let title = format!("{}конец", "ДлинноеНазвание日本語🧑‍💻".repeat(100));
    cx.simulate_input(&title);
    cx.run_until_parked();
    assert_eq!(cx.debug_bounds("matrix-add-input-0").unwrap(), empty_field);
    for width in [700., 480.] {
        cx.simulate_resize(gpui::size(px(width), px(500.)));
        let field = cx.debug_bounds("matrix-add-input-0").unwrap();
        cx.update(|window, cx| {
            let input = view.read(cx).adding.as_ref().unwrap().1.clone();
            let end = title.encode_utf16().count();
            let caret = input.update(cx, |input, cx| {
                input.bounds_for_range(end..end, gpui::Bounds::default(), window, cx).unwrap()
            });
            assert!(caret.left() >= field.left() && caret.right() <= field.right(), "{caret:?} outside {field:?}");
        });
    }
    cx.simulate_keystrokes("enter");
    let tasks = store.tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, title);
    cx.update(|_, cx| assert!(view.read(cx).adding.as_ref().unwrap().1.read(cx).text().is_empty()));
}

#[gpui::test]
fn quadrant_quick_add_saves_recognized_tags(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    cx.update(|cx| {
        let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
        let add = |cx: &App, input| {
            let id = matrix.read(cx).add_quadrant_task(0, input, today).unwrap().id;
            store.task(id).unwrap().unwrap()
        };
        let task = add(cx, "Plan the sprint #work #team #Work");
        assert_eq!(task.title, "Plan the sprint");
        assert_eq!(task.tags, ["team", "work"]);
        assert_eq!(task.priority, Priority::High);

        let mut preferences = store.preferences().unwrap();
        preferences.recognition.remove_tag_text = false;
        store.save_preferences(&preferences).unwrap();
        let task = add(cx, "Plan the sprint #work");
        assert_eq!(task.title, "Plan the sprint #work");
        assert_eq!(task.tags, ["work"]);

        preferences.recognition.tags = false;
        store.save_preferences(&preferences).unwrap();
        let task = add(cx, "Plan the sprint #work");
        assert_eq!(task.title, "Plan the sprint #work");
        assert!(task.tags.is_empty());
    });
}

#[gpui::test]
fn quadrant_quick_add_uses_an_available_list_after_deletion(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let removed = store.create_list("Remove me").unwrap();
    let available = store.create_list("Still available").unwrap();
    let mut preferences = store.preferences().unwrap();
    preferences.default_list = Some(removed.id);
    store.save_preferences(&preferences).unwrap();
    let mut rule = QuadrantRule::defaults()[0].clone();
    rule.lists = vec![Some(removed.id), Some(available.id)];
    store.save_matrix_rule(0, Some(&rule)).unwrap();
    let today = Local::now().date_naive();

    cx.update(|cx| {
        let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
        store.delete_list(removed.id).unwrap();
        let id = matrix.read(cx).add_quadrant_task(0, "First #work", today).unwrap().id;
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, id);
        assert_eq!(tasks[0].list_id, Some(available.id));
        assert_eq!(tasks[0].tags, ["work"]);

        rule.lists = vec![Some(removed.id)];
        store.save_matrix_rule(0, Some(&rule)).unwrap();
        matrix.update(cx, |matrix, cx| matrix.reload(cx));
        let view = matrix.read(cx);
        assert_eq!(view.rules[0].lists, rule.lists);
        let unrelated = &tasks[0];
        assert!(!view.rules[0].matches(unrelated, today));

        matrix.update(cx, |matrix, cx| {
            matrix.start_edit(0, cx);
            matrix.name_input.update(cx, |input, cx| input.set_text("Scoped quadrant", cx));
            matrix.finish_edit(Some(true), cx);
        });
        assert_eq!(store.matrix_rules().unwrap()[0].lists, rule.lists);

        let id = matrix.read(cx).add_quadrant_task(0, "Second", today).unwrap().id;
        assert_eq!(store.task(id).unwrap().unwrap().list_id, None);
        assert_eq!(store.tasks().unwrap().len(), 2);
    });
}

#[gpui::test]
fn archiving_the_last_list_keeps_the_quadrant_restricted_until_restore(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = Local::now().date_naive();
    let list = store.create_list("Restricted").unwrap();
    let fallback = store.create_list("Available").unwrap();
    let mut preferences = store.preferences().unwrap();
    preferences.default_list = Some(fallback.id);
    store.save_preferences(&preferences).unwrap();

    let task = |title: &str, list_id| {
        store
            .create_task(
                TaskDraft {
                    title: title.into(),
                    list_id: Some(list_id),
                    priority: Priority::High,
                    due_date: Some(today),
                    ..TaskDraft::default()
                },
                true,
            )
            .unwrap()
    };
    let restricted = task("In the restricted list", list.id);
    let unrelated = task("In another list", fallback.id);
    let mut rule = QuadrantRule::defaults()[0].clone();
    rule.lists = vec![Some(list.id)];
    store.save_matrix_rule(0, Some(&rule)).unwrap();

    cx.update(|cx| {
        let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
        store.set_list_archived(list.id, true).unwrap();
        matrix.update(cx, |matrix, cx| matrix.reload(cx));
        let view = matrix.read(cx);
        assert_eq!(view.rules[0].lists, rule.lists);
        assert!(view.cache[0].open_count == 0);
        let added = view.add_quadrant_task(0, "Saved in available list", today).unwrap();
        assert_eq!(added.list_id, Some(fallback.id));

        store.set_list_archived(list.id, false).unwrap();
        matrix.update(cx, |matrix, cx| matrix.reload(cx));
        let view = matrix.read(cx);
        assert!(view.tasks.iter().any(|task| task.id == restricted.id));
        assert_eq!(quadrant_of(&view.rules, &restricted, today), Some(0));
        assert_ne!(quadrant_of(&view.rules, &unrelated, today), Some(0));
        assert_ne!(quadrant_of(&view.rules, &added, today), Some(0));
    });
}

#[gpui::test]
fn matrix_uses_rule_date_without_an_explicit_date_and_explains_other_destinations(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    let mut rule = QuadrantRule::defaults()[0].clone();
    rule.dates = vec![MatrixDate::Tomorrow];
    store.save_matrix_rule(0, Some(&rule)).unwrap();

    cx.update(|cx| {
        let matrix = cx.new(|cx| MatrixView::new(store, cx));
        let task = matrix.read(cx).add_quadrant_task(0, "Plan 9am", today).unwrap();
        assert_eq!(task.due_date, today.succ_opt());
        assert_eq!(task.due_time, chrono::NaiveTime::from_hms_opt(9, 0, 0));
        let other = matrix.read(cx).add_quadrant_task(0, "Plan today", today).unwrap();
        assert_eq!(other.due_date, Some(today));
        matrix.update(cx, |matrix, cx| {
            matrix.reload(cx);
            let strings = i18n::t(cx);
            assert!(matrix.destination_notice(0, &task, today, strings).is_none());
            assert!(matches!(
                matrix.destination_notice(0, &other, today, strings),
                Some(MatrixNotice::SavedElsewhere(ref list)) if list.as_ref() == strings.inbox,
            ));
        });
    });
}

#[gpui::test]
fn escape_dismisses_the_matrix_header_menu(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let view = MatrixView::new(Rc::new(Store::open_in_memory().unwrap()), cx);
        window.focus(&view.focus_handle(cx), cx);
        view
    });
    let trigger = cx.debug_bounds("matrix-header-menu-trigger").unwrap().center();
    cx.simulate_click(trigger, gpui::Modifiers::default());
    cx.update(|_, cx| assert!(view.read(cx).header_menu));
    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| {
        assert!(!view.read(cx).header_menu);
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
    });
    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| assert!(view.read(cx).focus_handle(cx).is_focused(window)));
}

#[test]
fn quadrant_views_roundtrip_including_list_and_date() {
    let store = Store::open_in_memory().unwrap();
    let work = store.create_list("Work").unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    for (title, days) in [("later", 5), ("soon", 1), ("now", 0)] {
        let task = store.add_task(title).unwrap();
        store.set_list(task.id, Some(work.id)).unwrap();
        store.set_due(task.id, Some(today + Duration::days(days)), None).unwrap();
    }
    let tasks = store.tasks().unwrap();
    let lists = store.lists().unwrap();
    let strings = &i18n::EN;
    let titles = |groups: &[(String, Vec<Task>)]| -> Vec<String> {
        groups.iter().flat_map(|(_, tasks)| tasks.iter().map(|t| t.title.clone())).collect()
    };

    // Regression test for bug 1: GroupBy::List round-trips cleanly and keeps real list names.
    let list_view = QuadrantView { group: GroupBy::List, sort: SortBy::Date, newest_first: false };
    assert_eq!(QuadrantView::from_storage(&list_view.to_storage()), list_view);
    let by_list = arrange(tasks.clone(), list_view, &lists, today, strings);
    assert_eq!(by_list.len(), 1);
    assert_eq!(by_list[0].0, "Work");
    assert_eq!(titles(&by_list), ["now", "soon", "later"]);

    let newest = arrange(tasks.clone(), QuadrantView { newest_first: true, ..list_view }, &lists, today, strings);
    assert_eq!(titles(&newest), ["later", "soon", "now"]);

    let by_date = arrange(tasks, QuadrantView::default(), &lists, today, strings);
    assert_eq!(by_date.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(), ["Today", "Tomorrow", "Next 7 Days"]);
    assert_eq!(QuadrantView::from_storage(&QuadrantView::default().to_storage()), QuadrantView::default());
    assert_eq!(QuadrantView::from_storage(""), QuadrantView::default());
}

#[gpui::test]
fn show_completed_separates_and_toggles_completion(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Finish spec").unwrap();
    store.set_priority(task.id, Priority::High).unwrap();
    store.set_completed(task.id, true).unwrap();
    store.set_setting("matrix_show_completed", "1").unwrap();

    cx.update(|cx| {
        let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
        assert_eq!(matrix.read(cx).cache[0].open_count, 0);
        assert_eq!(matrix.read(cx).cache[0].completed.len(), 1);

        // Toggling a completed task reopens it into the open list.
        matrix.update(cx, |m, cx| m.toggle_complete(task.id, cx));
        assert_eq!(matrix.read(cx).cache[0].open_count, 1);
        assert!(matrix.read(cx).cache[0].completed.is_empty());
    });
}

#[gpui::test]
fn overflowing_quadrant_scrolls_when_clicking_scrollbar(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let mut ids = Vec::new();
    for i in 0..25 {
        ids.push(store.add_task(&format!("Quadrant item {i:02}")).unwrap().id);
    }

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (_, cx) = cx.add_window_view(|_, cx| MatrixView::new(store, cx));
    cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(480.)));
    cx.run_until_parked();

    // Tasks without priority land in quadrant IV ("Not Urgent & Not Important").
    let quadrant = cx.debug_bounds("matrix-quadrant-IV").unwrap();
    let (bottom_sel, bottom_row) = ids
        .iter()
        .filter_map(|id| {
            let sel: &'static str = format!("matrix-task-{id}").leak();
            cx.debug_bounds(sel).map(|b| (sel, b))
        })
        .max_by(|a, b| a.1.bottom().partial_cmp(&b.1.bottom()).unwrap())
        .expect("quadrant renders task rows");
    let initial_top = bottom_row.top();
    assert!(bottom_row.bottom() > quadrant.bottom());

    let bar_bottom = gpui::point(quadrant.right() - gpui::px(6.), quadrant.bottom() - gpui::px(8.));
    let modifiers = gpui::Modifiers::default();
    cx.simulate_mouse_move(bar_bottom, None, modifiers);
    cx.simulate_click(bar_bottom, modifiers);
    let scrolled_row = cx.debug_bounds(bottom_sel).unwrap();
    assert!(
        scrolled_row.top() < initial_top,
        "clicking quadrant scrollbar should scroll rows upward: {initial_top:?} -> {:?}",
        scrolled_row.top()
    );
}

#[gpui::test]
fn drop_on_applies_full_rule_patch_and_detects_already_in_quadrant(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let work = store.add_list(&fern_core::ListDraft { name: "Work".into(), ..Default::default() }).unwrap();
    let task = store.add_task("Draft quarterly plan").unwrap();

    // Configure Q1 with custom list, tag, date, and priority.
    let mut q1 = QuadrantRule::defaults()[0].clone();
    q1.lists = vec![Some(work.id)];
    q1.tags = vec!["focus".into()];
    q1.dates = vec![fern_core::matrix::MatrixDate::Today];
    q1.priorities = vec![Priority::High];
    store.save_matrix_rule(0, Some(&q1)).unwrap();

    cx.update(|cx| {
        let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
        matrix.update(cx, |m, cx| m.drop_on(task.id, 0, cx));

        let updated = store.task(task.id).unwrap().unwrap();
        assert_eq!(updated.priority, Priority::High);
        assert_eq!(updated.list_id, Some(work.id));
        assert_eq!(updated.due_date, Some(Local::now().date_naive()));
        assert!(updated.tags.iter().any(|t| t.eq_ignore_ascii_case("focus")));
        assert!(matrix.read(cx).notice.is_none());

        // Dropping into Q1 again when the task already matches shows AlreadyInQuadrant.
        matrix.update(cx, |m, cx| m.drop_on(task.id, 0, cx));
        assert!(matches!(matrix.read(cx).notice, Some(MatrixNotice::AlreadyInQuadrant)));
    });
}

#[gpui::test]
fn swap_quadrants_persists_custom_order(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
        assert_eq!(matrix.read(cx).quadrant_order, [0, 1, 2, 3]);

        matrix.update(cx, |m, cx| m.swap_quadrants(0, 3, cx));
        assert_eq!(matrix.read(cx).quadrant_order, [3, 1, 2, 0]);

        // Reloading from store preserves the swapped order.
        let reloaded = cx.new(|cx| MatrixView::new(store.clone(), cx));
        assert_eq!(reloaded.read(cx).quadrant_order, [3, 1, 2, 0]);
    });
}

#[gpui::test]
fn manual_task_order_persists_and_resets(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let a = store.add_task("Alpha").unwrap();
    let b = store.add_task("Beta").unwrap();
    let c = store.add_task("Gamma").unwrap();
    for id in [a.id, b.id, c.id] {
        store.set_priority(id, Priority::High).unwrap();
    }

    cx.update(|cx| {
        let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
        matrix.update(cx, |m, cx| {
            m.views[0] = QuadrantView { group: GroupBy::None, sort: SortBy::Title, newest_first: false };
            m.rebuild_cache(cx);
        });
        let before: Vec<Id> =
            matrix.read(cx).cache[0].groups[0].1.iter().map(|t| t.id).collect();
        assert_eq!(before, vec![a.id, b.id, c.id]);

        // Drag Gamma onto Alpha -> Gamma moves before Alpha.
        matrix.update(cx, |m, cx| m.drop_task_on_task(c.id, a.id, 0, cx));
        let after: Vec<Id> =
            matrix.read(cx).cache[0].groups[0].1.iter().map(|t| t.id).collect();
        assert_eq!(after, vec![c.id, a.id, b.id]);
        assert!(!matrix.read(cx).manual_order[0].is_empty());

        // Reset restores normal sort order.
        matrix.update(cx, |m, cx| m.reset_manual_order(0, cx));
        let reset: Vec<Id> =
            matrix.read(cx).cache[0].groups[0].1.iter().map(|t| t.id).collect();
        assert_eq!(reset, vec![a.id, b.id, c.id]);
        assert!(matrix.read(cx).manual_order[0].is_empty());
    });
}

#[gpui::test]
fn multiple_matrix_profiles_keep_independent_rules_and_settings(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let p2 = store.create_matrix_profile("Focus Profile", None).unwrap();

    cx.update(|cx| {
        let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
        assert_eq!(matrix.read(cx).profiles.len(), 2);

        // Rename Q1 on profile 1.
        matrix.update(cx, |m, cx| {
            m.start_edit(0, cx);
            m.name_input.update(cx, |i, cx| i.set_text("Profile 1 Q1", cx));
            m.finish_edit(Some(true), cx);
        });
        assert_eq!(matrix.read(cx).rules[0].name, "Profile 1 Q1");

        // Switch to profile 2 -> independent rules and view settings.
        matrix.update(cx, |m, cx| m.switch_profile(p2.id, cx));
        assert_eq!(matrix.read(cx).active_profile_id, Some(p2.id));
        assert_ne!(matrix.read(cx).rules[0].name, "Profile 1 Q1");

        matrix.update(cx, |m, cx| {
            m.start_edit(0, cx);
            m.name_input.update(cx, |i, cx| i.set_text("Profile 2 Q1", cx));
            m.finish_edit(Some(true), cx);
            m.swap_quadrants(0, 3, cx);
        });
        assert_eq!(matrix.read(cx).rules[0].name, "Profile 2 Q1");
        assert_eq!(matrix.read(cx).quadrant_order, [3, 1, 2, 0]);

        // Switch back -> profile 1 keeps its original order & rules.
        let p1_id = matrix.read(cx).profiles[0].id;
        matrix.update(cx, |m, cx| m.switch_profile(p1_id, cx));
        assert_eq!(matrix.read(cx).rules[0].name, "Profile 1 Q1");
        assert_eq!(matrix.read(cx).quadrant_order, [0, 1, 2, 3]);

        // Delete profile 2 -> only default remains.
        matrix.update(cx, |m, cx| m.delete_profile(p2.id, cx));
        assert_eq!(matrix.read(cx).profiles.len(), 1);
        assert_eq!(matrix.read(cx).active_profile_id, Some(p1_id));
    });
}

#[gpui::test]
fn rename_profile_prefills_existing_name_and_profile_switch_closes_rule_edit(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let work = store.create_matrix_profile("Work Focus", None).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        keymap::load_default(&assets::Assets, cx).unwrap();
    });
    let (view, cx) = cx.add_window_view(|_, cx| MatrixView::new(store.clone(), cx));

    cx.update(|window, cx| {
        view.update(cx, |m, cx| {
            // Opening profile editor for rename pre-fills the TextInput text (not just placeholder).
            m.open_profile_editor(Some(work.id), &work.name, window, cx);
            let (editing_id, input) = m.profile_editor.as_ref().expect("profile editor open");
            assert_eq!(*editing_id, Some(work.id));
            assert_eq!(input.read(cx).text(), "Work Focus");

            // Update and save renamed profile.
            input.update(cx, |inp, cx| inp.set_text("Deep Work", cx));
            m.save_profile_editor(cx);
        });
    });
    let renamed = store.matrix_profiles().unwrap().into_iter().find(|p| p.id == work.id).unwrap();
    assert_eq!(renamed.name, "Deep Work");

    // Opening quadrant rule edit modal and then switching/deleting profile must close it.
    cx.update(|_, cx| {
        view.update(cx, |m, cx| {
            m.start_edit(0, cx);
            assert!(m.editing.is_some());
            m.switch_profile(work.id, cx);
            assert!(m.editing.is_none());

            m.start_edit(1, cx);
            assert!(m.editing.is_some());
            m.delete_profile(work.id, cx);
            assert!(m.editing.is_none());
        });
    });
}

#[gpui::test]
fn toggle_complete_advances_repeating_tasks_and_reopens_wont_do(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = Local::now().date_naive();
    let rep = store.add_task("Weekly review").unwrap();
    store.set_priority(rep.id, Priority::High).unwrap();
    store.set_due(rep.id, Some(today), None).unwrap();
    store
        .set_repeat(rep.id, Some(fern_core::Recurrence::new(fern_core::Frequency::Weekly, 1)), today)
        .unwrap();

    let wd = store.add_task("Skipped chore").unwrap();
    store.set_priority(wd.id, Priority::High).unwrap();
    store.set_wont_do(wd.id, true).unwrap();

    let done = store.add_task("Finished item").unwrap();
    store.set_priority(done.id, Priority::High).unwrap();
    store.set_completed(done.id, true).unwrap();
    store.set_setting("matrix_show_completed", "1").unwrap();

    cx.update(|cx| {
        let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
        let observer = cx.new(|_| ());

        // Completing a repeating task advances its due date and keeps it open.
        matrix.update(cx, |m, cx| m.toggle_complete(rep.id, cx));
        let rep_after = store.task(rep.id).unwrap().unwrap();
        assert!(!rep_after.is_completed());
        assert!(rep_after.due_date > Some(today));
        assert!(desktop_common::data_version::changed_elsewhere(observer.entity_id(), cx));

        // Toggling a Won't Do task reopens it cleanly.
        matrix.update(cx, |m, cx| m.toggle_complete(wd.id, cx));
        let wd_after = store.task(wd.id).unwrap().unwrap();
        assert!(!wd_after.is_wont_do());
        assert!(!wd_after.is_completed());

        // Dropping a completed task in the same quadrant must not flash AlreadyInQuadrant.
        matrix.update(cx, |m, cx| {
            m.notice = None;
            m.drop_task_on_task(done.id, rep.id, 0, cx);
            assert!(m.notice.is_none());
            m.drop_on(done.id, 0, cx);
            assert!(m.notice.is_none());
        });
    });
}

#[gpui::test]
fn new_inactive_skips_initial_reload_until_activated(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Deferred quadrant item").unwrap();
    store.set_priority(task.id, Priority::High).unwrap();

    cx.update(|cx| {
        let matrix = cx.new(|cx| MatrixView::new_inactive(store.clone(), cx));
        {
            let m = matrix.read(cx);
            assert!(!m.refresh.is_active());
            assert!(m.refresh.is_dirty());
            assert!(m.tasks.is_empty());
            assert_eq!(m.cache[0].open_count, 0);
        }

        matrix.update(cx, |m, cx| m.set_active(true, cx));
        let m = matrix.read(cx);
        assert!(m.refresh.is_active());
        assert!(!m.refresh.is_dirty());
        assert_eq!(m.tasks.len(), 1);
        assert_eq!(m.cache[0].open_count, 1);
    });
}

#[gpui::test]
fn typing_in_detail_card_updates_matrix_tasks_and_cache_via_task_text_edited(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Original quadrant title").unwrap();
    store.set_priority(task.id, Priority::High).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|_, cx| MatrixView::new(store.clone(), cx));
    cx.simulate_resize(gpui::size(px(900.), px(600.)));
    cx.run_until_parked();

    let task_bounds = cx
        .debug_bounds(Box::leak(format!("matrix-task-{}", task.id).into_boxed_str()))
        .expect("matrix task visible");
    cx.simulate_click(task_bounds.center(), gpui::Modifiers::default());
    cx.run_until_parked();

    let title_bounds = cx.debug_bounds("task-detail-title").expect("detail title visible");
    cx.simulate_click(title_bounds.center(), gpui::Modifiers::default());
    cx.simulate_keystrokes("end");
    cx.simulate_input("!");
    cx.run_until_parked();

    cx.update(|_, cx| {
        let m = view.read(cx);
        assert_eq!(m.tasks[0].title, "Original quadrant title!");
        assert_eq!(m.cache[0].groups[0].1[0].title, "Original quadrant title!");
        assert!(cx.try_global::<desktop_common::data_version::DataVersion>().is_none());
    });
    cx.executor().advance_clock(std::time::Duration::from_millis(260));
    cx.run_until_parked();
    assert_eq!(store.task(task.id).unwrap().unwrap().title, "Original quadrant title!");

    let observer = cx.update(|_, cx| cx.new(|_| ()));
    cx.update(|_, cx| view.update(cx, |m, cx| m.close_detail_card(cx)));
    cx.run_until_parked();
    cx.update(|_, cx| assert!(desktop_common::data_version::changed_elsewhere(observer.entity_id(), cx)));
}
