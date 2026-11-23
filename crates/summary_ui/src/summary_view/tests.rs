use super::*;
use crate::i18n::EN;

#[test]
fn report_lines_and_markdown() {
    let store = Store::open_in_memory().unwrap();
    let today = Local::now().date_naive();
    let done = store.add_task("Ship it").unwrap();
    store.set_tags(done.id, &["work".to_owned()]).unwrap();
    store.set_completed(done.id, true).unwrap();
    let tasks = store.tasks().unwrap();
    let options = SummaryOptions {
        period: Period::Today,
        fields: vec![Field::Status, Field::CompletionTime, Field::Tag, Field::Title],
        ..Default::default()
    };
    let groups = summarize(&tasks, &options, today, calendar::week_start());
    let report = report(&groups, &options, &[], today, &EN);
    let day = short_date(today, &EN);
    assert_eq!(report.groups, [("Completed".to_owned(), vec![format!("[x] [{day}] Ship it #work")])]);
    assert_eq!(report.markdown(), format!("# {day}\n\n## Completed\n- [x] [{day}] Ship it #work\n"));
    assert!(report.mailto("Summary").starts_with("mailto:?subject=Summary&body=%23%20"));
    assert_eq!(percent_encode("a b&c"), "a%20b%26c");
}

#[gpui::test]
fn summary_view_caches_report_and_invalidates_on_options_and_data_version(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let t1 = store.add_task("First shipped task").unwrap();
    store.set_completed(t1.id, true).unwrap();

    let (view, cx) = cx.add_window_view(|_, cx| SummaryView::new(store.clone(), cx));
    cx.run_until_parked();

    // Initial render builds cache once.
    view.read_with(cx, |v, _| {
        assert_eq!(v.rebuild_count, 1);
        assert!(v.cache.as_ref().unwrap().report.markdown().contains("First shipped task"));
    });

    // Repeated re-renders (hover/menu/notify) reuse the cached report with zero DB rebuilds.
    view.update(cx, |v, cx| {
        v.save_menu = true;
        cx.notify();
    });
    cx.run_until_parked();
    view.update(cx, |v, cx| {
        v.save_menu = false;
        cx.notify();
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert_eq!(v.rebuild_count, 1));

    // Changing options invalidates and rebuilds once.
    view.update(cx, |v, cx| v.update(cx, |o| o.period = Period::Today));
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert_eq!(v.rebuild_count, 2));

    // External DataVersion change invalidates cache and picks up newly completed task.
    let t2 = store.add_task("Second shipped task").unwrap();
    store.set_completed(t2.id, true).unwrap();
    let other = cx.update(|_, cx| cx.new(|_| ()));
    cx.update(|_, cx| desktop_common::data_version::data_changed(other.entity_id(), cx));
    cx.run_until_parked();

    view.read_with(cx, |v, _| {
        assert_eq!(v.rebuild_count, 3);
        assert!(v.cache.as_ref().unwrap().report.markdown().contains("Second shipped task"));
    });
}

#[gpui::test]
fn edited_report_is_exported_and_untouched_editor_refreshes_on_reload(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let t1 = store.add_task("Alpha task").unwrap();
    store.set_completed(t1.id, true).unwrap();

    let (view, cx) = cx.add_window_view(|_, cx| SummaryView::new(store.clone(), cx));
    cx.run_until_parked();

    // Enter edit mode, make NO edits, and exit edit mode.
    view.update_in(cx, |v, window, cx| {
        v.toggle_editing(window, cx);
        assert!(v.editing);
        v.toggle_editing(window, cx);
        assert!(!v.editing);
        assert!(v.editor.is_none(), "untouched editor must be cleared on exit");
    });

    // Complete a new task externally and emit DataVersion.
    let t2 = store.add_task("Beta task").unwrap();
    store.set_completed(t2.id, true).unwrap();
    let other = cx.update(|_, cx| cx.new(|_| ()));
    cx.update(|_, cx| desktop_common::data_version::data_changed(other.entity_id(), cx));
    cx.run_until_parked();

    // Re-entering edit mode must show fresh data (including Beta task), not a stale snapshot.
    view.update_in(cx, |v, window, cx| {
        v.toggle_editing(window, cx);
        let editor = v.editor.as_ref().unwrap();
        assert!(editor.read(cx).text().contains("Beta task"));

        // Now manually edit the report and verify export_snapshot uses the edited content
        // for PNG/PDF groups, markdown, and mailto.
        let custom_md = "# Custom Summary\n- Hand-edited highlight";
        editor.update(cx, |input, cx| input.set_text(custom_md.to_owned(), cx));
        let (export, md, mailto) = v.export_snapshot(cx);
        assert_eq!(md, custom_md);
        assert_eq!(export.groups, vec![(String::new(), vec!["# Custom Summary".to_owned(), "- Hand-edited highlight".to_owned()])]);
        assert!(mailto.contains(&percent_encode("Hand-edited highlight")));
    });
}

#[gpui::test]
fn rapid_copies_cancel_previous_copied_badge_timer(cx: &mut gpui::TestAppContext) {
    use std::time::Duration;
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (view, cx) = cx.add_window_view(|_, cx| SummaryView::new(store, cx));
    cx.run_until_parked();

    view.update(cx, |v, cx| v.copy_text("first".into(), cx));
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert!(v.copied));

    // After 1.5s, copy again -> cancels first 2s timer.
    cx.executor().advance_clock(Duration::from_millis(1500));
    cx.run_until_parked();
    view.update(cx, |v, cx| v.copy_text("second".into(), cx));
    cx.run_until_parked();

    // Advance 1s (2.5s since first copy, 1s since second) -> badge must STILL be shown.
    cx.executor().advance_clock(Duration::from_millis(1000));
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert!(v.copied, "first timer should have been cancelled"));

    // Advance past the second timer's 2s deadline -> badge clears.
    cx.executor().advance_clock(Duration::from_millis(1100));
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert!(!v.copied));
}

#[gpui::test]
fn hidden_summary_view_defers_notify_and_rebuilds_when_activated(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let t1 = store.add_task("Initial done").unwrap();
    store.set_completed(t1.id, true).unwrap();

    let (view, cx) = cx.add_window_view(|_, cx| SummaryView::new(store.clone(), cx));
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert_eq!(v.rebuild_count, 1));

    // Hide SummaryView, then emit external DataVersion change -> marks dirty without rebuilding.
    view.update(cx, |v, cx| v.set_active(false, cx));
    let t2 = store.add_task("Added while hidden").unwrap();
    store.set_completed(t2.id, true).unwrap();
    let other = cx.update(|_, cx| cx.new(|_| ()));
    cx.update(|_, cx| desktop_common::data_version::data_changed(other.entity_id(), cx));
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.cache_dirty);
        assert_eq!(v.rebuild_count, 1);
    });

    // Re-activating refreshes once and picks up the new task.
    view.update(cx, |v, cx| v.set_active(true, cx));
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(!v.cache_dirty);
        assert_eq!(v.rebuild_count, 2);
        assert!(v.cache.as_ref().unwrap().report.markdown().contains("Added while hidden"));
    });

    // Toggling off and back on with no changes avoids redundant rebuilds.
    view.update(cx, |v, cx| v.set_active(false, cx));
    view.update(cx, |v, cx| v.set_active(true, cx));
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert_eq!(v.rebuild_count, 2));
}

#[gpui::test]
fn delete_template_requires_confirmation_and_supports_escape_cancel(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let template = store.save_summary_template("Weekly Review", &SummaryOptions::default()).unwrap();

    let (view, cx) = cx.add_window_view(|_, cx| SummaryView::new(store.clone(), cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));
    cx.run_until_parked();

    // Start with the markdown editor focused so we verify opening confirmation dialogs
    // moves focus back onto SummaryView's focus_handle for Escape dismissal.
    view.update_in(cx, |v, window, cx| {
        v.toggle_editing(window, cx);
        let editor = v.editor.as_ref().unwrap().clone();
        editor.update(cx, |input, cx| input.set_text("# Manual Edit".to_owned(), cx));
        assert!(editor.focus_handle(cx).is_focused(window));
    });
    cx.run_until_parked();

    // Hover template chip to reveal the delete button, then click delete -> opens ConfirmDialog only.
    let pill_sel: &'static str = Box::leak(format!("template-{}", template.id).into_boxed_str());
    let delete_sel: &'static str = Box::leak(format!("delete-template-{}", template.id).into_boxed_str());
    let pill_center = cx.debug_bounds(pill_sel).unwrap().center();
    cx.simulate_mouse_move(pill_center, None, gpui::Modifiers::default());
    cx.run_until_parked();
    let delete_btn = cx.debug_bounds(delete_sel).unwrap().center();
    cx.simulate_click(delete_btn, gpui::Modifiers::default());
    cx.run_until_parked();

    view.read_with(cx, |v, _| {
        assert_eq!(v.confirm_delete_template, Some((template.id, "Weekly Review".to_owned())));
    });
    assert!(cx.debug_bounds("summary-template-delete-modal").is_some());
    assert_eq!(store.summary_templates().unwrap().len(), 1, "template must remain in SQLite before confirmation");

    // Escape / DismissTransient reaches SummaryView::dismiss_transient and cancels without deleting.
    cx.update(|window, cx| {
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        window.dispatch_action(Box::new(md::DismissTransient), cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert!(v.confirm_delete_template.is_none()));
    assert!(cx.debug_bounds("summary-template-delete-modal").is_none());
    assert_eq!(store.summary_templates().unwrap().len(), 1);

    // Re-trigger delete and confirm -> deletes from SQLite and refreshes cache.
    cx.simulate_mouse_move(pill_center, None, gpui::Modifiers::default());
    cx.run_until_parked();
    cx.simulate_click(delete_btn, gpui::Modifiers::default());
    cx.run_until_parked();
    let modal_bounds = cx.debug_bounds("summary-template-delete-modal").unwrap();
    let confirm_btn = gpui::point(modal_bounds.center().x + px(120.), modal_bounds.center().y + px(38.));
    cx.simulate_click(confirm_btn, gpui::Modifiers::default());
    cx.run_until_parked();

    view.read_with(cx, |v, _| {
        assert!(v.confirm_delete_template.is_none());
        assert!(v.cache.as_ref().unwrap().templates.is_empty());
    });
    assert!(store.summary_templates().unwrap().is_empty());

    // Clicking Reset with editor focused moves focus to SummaryView so Escape dismisses pending_options.
    cx.update(|window, cx| {
        let editor = view.read(cx).editor.as_ref().unwrap().clone();
        window.focus(&editor.focus_handle(cx), cx);
        assert!(editor.focus_handle(cx).is_focused(window));
    });
    let reset_btn = cx.debug_bounds("summary-reset-edits").unwrap().center();
    cx.simulate_click(reset_btn, gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(view.read(cx).pending_options.is_some());
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        window.dispatch_action(Box::new(md::DismissTransient), cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert!(v.pending_options.is_none()));
}

#[gpui::test]
fn custom_period_and_template_inputs_close_on_escape_and_restore_focus(cx: &mut gpui::TestAppContext) {
    use gpui::KeyBinding;
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        cx.bind_keys([
            KeyBinding::new("escape", crate::text_input::Cancel, Some("TextInput")),
            KeyBinding::new("escape", md::DismissTransient, None),
        ]);
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (view, cx) = cx.add_window_view(|_, cx| SummaryView::new(store, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));
    cx.run_until_parked();

    for focus_end in [false, true] {
        view.update_in(cx, |v, window, cx| {
            v.open_custom(window, cx);
            let (start, end) = v.custom.as_ref().unwrap();
            let target = if focus_end { end } else { start };
            window.focus(&target.focus_handle(cx), cx);
            assert!(target.focus_handle(cx).is_focused(window));
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let v = view.read(cx);
            assert!(v.custom.is_none());
            assert!(v.focus_handle(cx).is_focused(window));
        });
    }

    view.update_in(cx, |v, window, cx| {
        v.start_template(window, cx);
        let naming = v.naming.as_ref().unwrap();
        assert!(naming.focus_handle(cx).is_focused(window));
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert!(v.naming.is_none());
        assert!(v.focus_handle(cx).is_focused(window));
    });
}
