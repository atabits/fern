use super::*;
use gpui::{EntityInputHandler, Modifiers, TestAppContext, point};

#[gpui::test]
fn long_quick_add_keeps_the_caret_inside_both_input_layouts(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        crate::keymap::load_default(&assets::Assets, cx).unwrap();
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    let title = format!("{}конец", "Длинное название 日本語 🧑‍💻 ".repeat(80));
    for detailed in [false, true] {
        cx.update(|window, cx| view.update(cx, |view, cx| {
            view.preferences.detailed_input = detailed;
            window.focus(&view.input.focus_handle(cx), cx);
            cx.notify();
        }));
        cx.simulate_resize(gpui::size(px(540.), px(480.)));
        let empty_bounds = cx.debug_bounds("quick-add-input").unwrap();
        cx.simulate_input(&title);
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("quick-add-input").unwrap(), empty_bounds);
        for width in [540., 340.] {
            cx.simulate_resize(gpui::size(px(width), px(480.)));
            let field = cx.debug_bounds("quick-add-input").unwrap();
            let end = title.encode_utf16().count();
            for (key, offset) in [("home", 0), ("end", end)] {
                cx.simulate_keystrokes(key);
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let input = view.read(cx).input.clone();
                    let caret = input.update(cx, |input, cx| {
                        input.bounds_for_range(offset..offset, gpui::Bounds::default(), window, cx).unwrap()
                    });
                    assert!(caret.left() >= field.left() && caret.right() <= field.right(), "{detailed}: {caret:?} outside {field:?}");
                });
            }
        }
        cx.simulate_keystrokes("enter");
        cx.update(|_, cx| assert!(view.read(cx).input.read(cx).text().is_empty()));
    }
    let tasks = store.tasks().unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().all(|task| task.title == title));
}

#[gpui::test]
fn long_inline_title_stays_inside_its_column_and_saves_in_full(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let title = "Long task title 日本語 🧑‍💻 ".repeat(80).trim().to_owned();
    let task = store.add_task(&title).unwrap();
    store.set_completed(task.id, true).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        crate::keymap::load_default(&assets::Assets, cx).unwrap();
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = TodoView::new(store.clone(), window, cx);
        view.set_view(View::Completed, cx);
        view
    });
    cx.simulate_resize(gpui::size(px(900.), px(480.)));
    let selector = format!("task-title-field-{}", task.id).leak();
    let original_field = cx.debug_bounds(selector).unwrap();
    assert!(original_field.size.width > px(50.));
    cx.update(|window, cx| view.update(cx, |view, cx| view.start_title_edit(task.id, None, window, cx)));
    cx.run_until_parked();
    let field = cx.debug_bounds(selector).unwrap();
    assert_eq!(field.left(), original_field.left());
    assert_eq!(field.right(), original_field.right());
    cx.simulate_input("!");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let (_, input, _) = view.read(cx).title_edit.as_ref().unwrap();
        let input = input.clone();
        let end = title.encode_utf16().count() + 1;
        let caret = input.update(cx, |input, cx| {
            input.bounds_for_range(end..end, gpui::Bounds::default(), window, cx).unwrap()
        });
        assert!(caret.left() >= original_field.left() && caret.right() <= original_field.right(), "{caret:?} outside {original_field:?}");
    });
    cx.simulate_keystrokes("enter");
    assert_eq!(store.task(task.id).unwrap().unwrap().title, format!("{title}!"));
}

#[gpui::test]
fn completion_stays_in_place_until_the_check_settles(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let second = store.add_task("Finish two").unwrap();
    let first = store.add_task("Finish one").unwrap();
    store.set_setting("show_completed:today", "0").unwrap();
    for task in [&first, &second] {
        store.set_due(task.id, Some(TodoView::today()), None).unwrap();
    }
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    // Exercise the virtual list and the board, also with completed tasks filtered out.
    for kanban in [false, true] {
        for task in [&first, &second] { store.set_completed(task.id, false).unwrap(); }
        cx.update(|_, cx| view.update(cx, |view, cx| {
            view.set_view(View::Today, cx);
            view.set_layout(kanban, false, cx);
            view.reload(cx);
        }));
        cx.run_until_parked();
        let selector = |id| -> &'static str {
            format!("{}-{id}", if kanban { "kanban-checkbox" } else { "task-title" }).leak()
        };
        let first_selector = selector(first.id);
        let second_selector = selector(second.id);
        let first_bounds = cx.debug_bounds(first_selector).unwrap();
        let second_bounds = cx.debug_bounds(second_selector).unwrap();
        assert!(first_bounds.top() < second_bounds.top());
        let at = |bounds: gpui::Bounds<Pixels>| {
            if kanban { bounds.center() } else { point(bounds.left() - px(18.), bounds.center().y) }
        };
        cx.simulate_click(at(first_bounds), Modifiers::default());
        assert!(store.task(first.id).unwrap().unwrap().is_completed());
        assert_eq!(cx.debug_bounds(first_selector), Some(first_bounds));
        assert_eq!(cx.debug_bounds(second_selector), Some(second_bounds));
        cx.simulate_click(at(second_bounds), Modifiers::default());
        assert!(store.task(second.id).unwrap().unwrap().is_completed());
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(view.completing[&first.id].index < view.completing[&second.id].index);
            assert!(crate::completion_mark::started(first.id, cx).is_some());
            assert!(crate::completion_mark::started(second.id, cx).is_some());
        });
        cx.executor().advance_clock(std::time::Duration::from_millis(200));
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds(first_selector), Some(first_bounds));
        assert_eq!(cx.debug_bounds(second_selector), Some(second_bounds));
        cx.executor().advance_clock(std::time::Duration::from_millis(80));
        cx.run_until_parked();
        assert!(cx.debug_bounds(first_selector).is_none());
        assert!(cx.debug_bounds(second_selector).is_none());
    }
}

#[gpui::test]
fn a_second_click_reverts_a_repeating_completion_during_the_animation(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Daily task").unwrap();
    let today = TodoView::today();
    store.set_repeat(task.id, Some(fern_core::Recurrence::new(fern_core::Frequency::Daily, 1)), today).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    let selector = format!("task-title-{}", task.id).leak();
    let bounds = cx.debug_bounds(selector).unwrap();
    let position = point(bounds.left() - px(18.), bounds.center().y);
    cx.simulate_click(position, Modifiers::default());
    assert_eq!(store.task(task.id).unwrap().unwrap().due_date, Some(today + Duration::days(1)));
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.simulate_click(position, Modifiers::default());
    assert_eq!(store.task(task.id).unwrap().unwrap().due_date, Some(today));
    assert_eq!(store.tasks().unwrap().len(), 1);
    cx.update(|_, cx| assert!(view.read(cx).completing.is_empty()));
    // Completing again must survive the cancelled transition's original deadline.
    cx.simulate_click(position, Modifiers::default());
    cx.executor().advance_clock(std::time::Duration::from_millis(180));
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).completing.contains_key(&task.id)));
    cx.update(|_, cx| view.update(cx, |view, cx| view.apply_undo(cx)));
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(store.task(task.id).unwrap().unwrap().due_date, Some(today));
    assert_eq!(store.tasks().unwrap().len(), 1);
}

#[gpui::test]
fn completion_respects_navigation_and_reduced_motion(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Complete immediately").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    cx.update(|_, cx| view.update(cx, |view, cx| {
        view.toggle(task.id, true, cx);
        assert!(view.completing.contains_key(&task.id));
        view.set_view(View::Completed, cx);
        assert!(view.completing.is_empty());
        view.toggle(task.id, false, cx);
        view.set_view(View::Inbox, cx);
        cx.set_reduce_motion(true);
        view.toggle(task.id, true, cx);
        assert!(view.completing.is_empty());
        assert!(view.tasks[0].is_completed());
    }));
}

#[gpui::test]
fn unchecking_a_kanban_card_reopens_the_task(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Reopen on board").unwrap();
    store.set_completed(task.id, true).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = TodoView::new(store.clone(), window, cx);
        view.set_layout(true, false, cx);
        view
    });
    let selector = format!("kanban-checkbox-{}", task.id).leak();
    let position = cx.debug_bounds(selector).expect("completed kanban card is rendered").center();
    let modifiers = Modifiers::default();
    cx.simulate_mouse_move(position, None, modifiers);
    cx.simulate_click(position, modifiers);
    assert!(!store.task(task.id).unwrap().unwrap().is_closed());
    cx.update(|_, cx| {
        assert!(view.read(cx).tasks.iter().any(|task| !task.is_closed()));
    });
}

#[gpui::test]
fn unchecking_a_completed_row_reopens_the_task(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Reopen me").unwrap();
    let today = TodoView::today();
    store.set_due(task.id, Some(today), None).unwrap();
    store.set_completed(task.id, true).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = TodoView::new(store.clone(), window, cx);
        view.set_view(View::Completed, cx);
        view
    });
    let selector = format!("task-title-{}", task.id).leak();
    let click_checkbox = |cx: &mut gpui::VisualTestContext| {
        let title = cx.debug_bounds(selector).expect("completed task is rendered");
        // The 16px checkbox is immediately before the title, with a 10px gap.
        let position = point(title.left() - px(18.), title.center().y);
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(position, None, modifiers);
        cx.simulate_click(position, modifiers);
    };
    click_checkbox(cx);
    assert!(!store.task(task.id).unwrap().unwrap().is_closed());
    cx.update(|_, cx| assert!(view.read(cx).tasks.is_empty()));
    cx.update(|_, cx| view.update(cx, |view, cx| view.set_view(View::Today, cx)));
    cx.update(|_, cx| {
        let tasks = &view.read(cx).tasks;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, task.id);
        assert_eq!(tasks[0].due_date, Some(today));
        assert!(!tasks[0].is_closed());
    });

    store.set_completed(task.id, true).unwrap();
    cx.update(|_, cx| view.update(cx, |view, cx| {
        view.toggle_view_flag("show_completed", true, cx);
        assert!(view.tasks.is_empty());
        view.toggle_view_flag("show_completed", true, cx);
        assert!(view.tasks[0].is_completed());
    }));
    cx.run_until_parked();
    click_checkbox(cx);
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(view.tasks[0].id, task.id);
        assert!(!view.tasks[0].is_closed());
        assert!(!view.rows(i18n::t(cx)).iter().any(|row| {
            matches!(row, Row::Section { section: Section::Completed, .. })
        }));
    });

    let list = store.create_list("Reopen test").unwrap();
    for (page, list_id) in [
        (View::Inbox, None),
        (View::All, None),
        (View::Search("Reopen me".into()), None),
        (View::List(list.id), Some(list.id)),
    ] {
        store.set_list(task.id, list_id).unwrap();
        store.set_completed(task.id, true).unwrap();
        cx.update(|_, cx| view.update(cx, |view, cx| view.set_view(page.clone(), cx)));
        cx.run_until_parked();
        click_checkbox(cx);
        cx.update(|_, cx| {
            let tasks = &view.read(cx).tasks;
            assert_eq!(tasks.len(), 1, "{page:?}");
            assert_eq!(tasks[0].id, task.id, "{page:?}");
            assert!(!tasks[0].is_closed(), "{page:?}");
            assert_eq!(tasks[0].list_id, list_id, "{page:?}");
        });
    }
}

#[gpui::test]
fn clicking_the_selected_title_edits_and_saves_it_in_the_list(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Original").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        crate::keymap::load_default(&assets::Assets, cx).unwrap();
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    let selector = format!("task-title-{}", task.id).leak();
    let bounds = cx.debug_bounds(selector).expect("task title is rendered");
    // Single click in the middle of the title immediately starts editing at the clicked offset.
    let position = bounds.center();
    let modifiers = Modifiers::default();
    cx.simulate_mouse_move(position, None, modifiers);
    cx.simulate_click(position, modifiers);
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert_eq!(view.read(cx).selected, Some(task.id));
        let (_, input, _) = view.read(cx).title_edit.as_ref().expect("inline editor opens on first click");
        assert!(input.focus_handle(cx).is_focused(window));
        let selection = input.read(cx).selection();
        assert!(selection.is_empty());
        assert!(
            selection.start > 0 && selection.start < "Original".len(),
            "caret should land inside the clicked word, got {selection:?}"
        );
    });
    cx.simulate_input("!");
    cx.simulate_keystrokes("enter");
    let saved = store.task(task.id).unwrap().unwrap().title;
    assert!(saved.contains('!') && !saved.starts_with('!') && !saved.ends_with('!'), "saved title: {saved}");
    cx.update(|_, cx| assert!(view.read(cx).title_edit.is_none()));
}

#[gpui::test]
fn escape_closes_the_sort_menu_and_restores_focus(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    let sort = cx.debug_bounds("task-sort-menu-trigger").unwrap().center();
    let modifiers = Modifiers::default();

    let previous = cx.update(|window, cx| window.focused(cx).expect("quick-add field has focus"));
    cx.simulate_mouse_move(sort, None, modifiers);
    cx.simulate_click(sort, modifiers);
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.sort_menu, Some(None));
        assert!(view.sort_menu_focus.is_focused(window));
    });

    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| {
        assert!(view.read(cx).sort_menu.is_none());
        assert!(previous.is_focused(window));
    });
}

#[gpui::test]
fn escape_returns_focus_to_the_board_when_it_closes_a_card(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Board task").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = TodoView::new(store.clone(), window, cx);
        view.set_layout(true, false, cx);
        view.open_card(task.id, cx);
        view
    });
    let body = cx.debug_bounds("task-detail-body").expect("board card is rendered").center();
    let modifiers = Modifiers::default();
    cx.simulate_mouse_move(body, None, modifiers);
    cx.simulate_click(body, modifiers);
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.card, Some(task.id));
        assert!(view.list_focus.contains_focused(window, cx));
        assert!(!view.list_focus.is_focused(window));
    });

    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert!(view.card.is_none());
        assert!(view.list_focus.is_focused(window));
    });
}

#[gpui::test]
fn a_rejected_quick_add_attachment_does_not_repeat_the_saved_task(cx: &mut TestAppContext) {
    use fern_core::attachment::MAX_ATTACHMENT_BYTES;

    let store = Rc::new(Store::open_in_memory().unwrap());
    let file = std::env::temp_dir().join(format!("fern-oversized-{}.bin", Id::now_v7()));
    std::fs::File::create(&file).unwrap().set_len(MAX_ATTACHMENT_BYTES + 1).unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = TodoView::new(store.clone(), window, cx);
        view.add_draft.files.push(file.clone());
        window.focus(&view.input.focus_handle(cx), cx);
        view
    });
    cx.simulate_input("One task");
    cx.simulate_keystrokes("enter enter");
    std::fs::remove_file(file).unwrap();
    let tasks = store.tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(store.attachments(tasks[0].id).unwrap().is_empty());
    cx.update(|_, cx| {
        assert!(view.read(cx).input.read(cx).text().is_empty());
        assert!(view.read(cx).add_attachment_error.as_deref().is_some_and(|error| error.contains("32 MiB")));
    });
}

#[gpui::test]
fn a_task_without_tags_can_open_the_picker_and_save_its_first_tag(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("No tags yet").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        crate::keymap::load_default(&assets::Assets, cx).unwrap();
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    cx.update(|window, cx| view.update(cx, |view, cx| {
        window.focus(&view.list_focus, cx);
        view.start_detail(task.id, "tags", window, cx);
    }));
    assert!(cx.debug_bounds("task-tag-picker").is_some());
    cx.simulate_input("work");
    cx.simulate_keystrokes("enter");
    assert!(store.task(task.id).unwrap().unwrap().tags.is_empty());
    assert!(cx.debug_bounds("task-tag-picker").is_some());
    cx.simulate_keystrokes("escape");
    assert!(cx.debug_bounds("task-tag-picker").is_none());
    assert!(store.tags().unwrap().is_empty(), "Esc discards new tags too");
    cx.update(|window, cx| {
        assert!(view.read(cx).list_focus.is_focused(window));
        view.update(cx, |view, cx| view.start_detail(task.id, "tags", window, cx));
    });
    cx.simulate_input("work");
    cx.simulate_keystrokes("enter");
    let ok = cx.debug_bounds("task-tags-ok").unwrap().center();
    cx.simulate_mouse_move(ok, None, Modifiers::default());
    cx.simulate_click(ok, Modifiers::default());
    assert_eq!(store.task(task.id).unwrap().unwrap().tags, ["work"]);
    assert!(cx.debug_bounds("task-tag-picker").is_none());
    cx.update(|window, cx| assert!(view.read(cx).list_focus.is_focused(window)));
}

#[gpui::test]
fn backspace_edits_text_but_only_delete_trashes_the_selected_task(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Original").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        let errors = crate::keymap::load_default(&assets::Assets, cx).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    let selector = format!("task-title-{}", task.id).leak();
    let bounds = cx.debug_bounds(selector).expect("task title is rendered");
    let row_empty_space = point(bounds.right() + px(24.), bounds.center().y);
    let title_click = point(bounds.left() + px(8.), bounds.center().y);
    let modifiers = Modifiers::default();

    cx.simulate_mouse_move(row_empty_space, None, modifiers);
    cx.simulate_click(row_empty_space, modifiers);
    cx.update(|window, cx| {
        let view = view.read(cx);
        assert_eq!(view.selected, Some(task.id));
        assert!(view.title_edit.is_none());
        assert!(view.list_focus.is_focused(window));
    });
    cx.simulate_keystrokes("backspace");
    assert!(!store.task(task.id).unwrap().unwrap().is_deleted());

    cx.simulate_mouse_move(title_click, None, modifiers);
    cx.simulate_click(title_click, modifiers);
    cx.simulate_keystrokes("end backspace enter");
    let edited = store.task(task.id).unwrap().unwrap();
    assert_eq!(edited.title, "Origina");
    assert!(!edited.is_deleted());

    cx.simulate_keystrokes("delete");
    assert!(store.task(task.id).unwrap().unwrap().is_deleted());
    cx.update(|_, cx| {
        assert!(view.read(cx).tasks.is_empty());
        assert_eq!(view.read(cx).selected, None);
    });

    cx.update(|_, cx| view.update(cx, |view, cx| {
        view.set_view(View::Trash, cx);
        view.select(task.id, cx);
        view.reload(cx);
        assert_eq!(view.selected, Some(task.id), "Trash still shows the selected deleted task");
    }));
}

#[gpui::test]
fn switching_views_selects_the_first_visible_row_and_syncs_details(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let l1 = store.create_list("L1").unwrap();
    let l2 = store.create_list("L2").unwrap();
    let empty_list = store.create_list("EmptyList").unwrap();
    let kanban_list = store.create_list("BoardList").unwrap();
    store.set_setting(&format!("kanban:list:{}", kanban_list.id), "1").unwrap();

    let a2 = store.add_task("A2").unwrap();
    let a1 = store.add_task("A1").unwrap();
    for task in [&a1, &a2] {
        store.set_list(task.id, Some(l1.id)).unwrap();
        store.set_tags(task.id, &["AAA".to_owned()]).unwrap();
    }
    let b2 = store.add_task("B2").unwrap();
    let b1 = store.add_task("B1").unwrap();
    for task in [&b1, &b2] {
        store.set_list(task.id, Some(l2.id)).unwrap();
        store.set_tags(task.id, &["BBB".to_owned()]).unwrap();
    }
    let board_task = store.add_task("Board1").unwrap();
    store.set_list(board_task.id, Some(kanban_list.id)).unwrap();
    store.add_tag("EmptyTag", None, None).unwrap();

    let filter_aaa = fern_core::SmartFilter {
        name: "FilterAAA".to_owned(),
        rules: fern_core::FilterRules {
            conditions: vec![fern_core::Condition::new(fern_core::ConditionValue::Tags {
                names: vec!["AAA".to_owned()],
                untagged: false,
            })],
            ..Default::default()
        },
        ..fern_core::SmartFilter::new()
    };
    store.save_filter(&filter_aaa).unwrap();
    let empty_filter = fern_core::SmartFilter {
        name: "EmptyFilter".to_owned(),
        rules: fern_core::FilterRules {
            conditions: vec![fern_core::Condition::new(fern_core::ConditionValue::Tags {
                names: vec!["EmptyTag".to_owned()],
                untagged: false,
            })],
            ..Default::default()
        },
        ..fern_core::SmartFilter::new()
    };
    store.save_filter(&empty_filter).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        let errors = crate::keymap::load_default(&assets::Assets, cx).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    cx.simulate_resize(gpui::size(px(1280.), px(960.)));
    cx.run_until_parked();

    let click_panel = |selector: &str, cx: &mut gpui::VisualTestContext| {
        let selector: &'static str = selector.to_owned().leak();
        let center = cx.debug_bounds(selector).unwrap_or_else(|| panic!("missing {selector}")).center();
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(center, None, modifiers);
        cx.simulate_click(center, modifiers);
        cx.run_until_parked();
    };
    let detail_state = |cx: &mut gpui::VisualTestContext| {
        cx.update(|_, cx| {
            let view = view.read(cx);
            let detail = view.detail.read(cx);
            (view.selected, detail.task_id(), detail.task_title().map(str::to_owned))
        })
    };

    // Switch to tag BBB, then manually select the second row B2 (matching issue #17).
    click_panel("lists-item-tag-BBB", cx);
    assert_eq!(detail_state(cx), (Some(b1.id), Some(b1.id), Some("B1".to_owned())));
    assert!(cx.debug_bounds("task-detail-header").is_some());

    cx.update(|_, cx| view.update(cx, |view, cx| view.select(b2.id, cx)));
    cx.run_until_parked();
    assert_eq!(detail_state(cx), (Some(b2.id), Some(b2.id), Some("B2".to_owned())));

    // Switch to list L1: first visible row A1 is selected and Region C updates to A1.
    click_panel(&format!("lists-item-list-{}", l1.id), cx);
    assert_eq!(detail_state(cx), (Some(a1.id), Some(a1.id), Some("A1".to_owned())));
    cx.update(|window, cx| assert!(view.read(cx).list_focus.is_focused(window)));

    // Keyboard navigation immediately advances from the auto-selected first row to A2.
    cx.simulate_keystrokes("down");
    assert_eq!(detail_state(cx), (Some(a2.id), Some(a2.id), Some("A2".to_owned())));

    // Switch to an empty list: selection and Region C are cleared.
    click_panel(&format!("lists-item-list-{}", empty_list.id), cx);
    assert_eq!(detail_state(cx), (None, None, None));
    assert!(cx.debug_bounds("task-detail-header").is_none());

    // Switch to a saved filter with records, then an empty filter, then an empty tag.
    click_panel(&format!("lists-item-filter-{}", filter_aaa.id), cx);
    assert_eq!(detail_state(cx), (Some(a1.id), Some(a1.id), Some("A1".to_owned())));

    click_panel(&format!("lists-item-filter-{}", empty_filter.id), cx);
    assert_eq!(detail_state(cx), (None, None, None));
    assert!(cx.debug_bounds("task-detail-header").is_none());

    click_panel("lists-item-tag-AAA", cx);
    assert_eq!(detail_state(cx), (Some(a1.id), Some(a1.id), Some("A1".to_owned())));

    click_panel("lists-item-tag-EmptyTag", cx);
    assert_eq!(detail_state(cx), (None, None, None));

    // Pinned/sorted order determines the first row in Region B.
    store.set_pinned(b2.id, true).unwrap();
    click_panel(&format!("lists-item-list-{}", l2.id), cx);
    assert_eq!(detail_state(cx), (Some(b2.id), Some(b2.id), Some("B2".to_owned())));

    // Switching to a kanban list clears the side-panel selection so the board takes full width.
    click_panel(&format!("lists-item-list-{}", kanban_list.id), cx);
    assert_eq!(detail_state(cx), (None, None, None));
    assert!(cx.debug_bounds("task-detail-header").is_none());

    // Narrow window with pop-up detail layout does not cover the list with a modal on view switch.
    cx.update(|_, cx| view.update(cx, |view, cx| view.save_preferences(|p| p.popup_details = true, cx)));
    cx.simulate_resize(gpui::size(px(900.), px(960.)));
    cx.run_until_parked();
    click_panel(&format!("lists-item-list-{}", l1.id), cx);
    assert_eq!(detail_state(cx), (None, None, None));
    assert!(cx.debug_bounds("detail-popup").is_none());
}

#[gpui::test]
fn kanban_keeps_pinned_completed_and_subtasks_inside_their_own_columns(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let list = store.create_list("Project").unwrap();
    let t_open = store.add_task("Open card").unwrap();
    let t_pinned = store.add_task("Pinned card").unwrap();
    let t_done = store.add_task("Completed card").unwrap();
    let sub_done = store.add_task("Completed subtask").unwrap();
    for t in [&t_open, &t_pinned, &t_done, &sub_done] {
        store.set_list(t.id, Some(list.id)).unwrap();
    }
    store.link_parent(sub_done.id, t_open.id).unwrap();
    store.set_priority(t_open.id, Priority::High).unwrap();
    store.set_priority(sub_done.id, Priority::Low).unwrap();
    store.set_completed(sub_done.id, true).unwrap();
    store.set_pinned(t_pinned.id, true).unwrap();
    store.set_completed(t_done.id, true).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = TodoView::new(store.clone(), window, cx);
        view.set_view(View::List(list.id), cx);
        view.set_layout(true, false, cx);
        view
    });
    cx.run_until_parked();

    // Switching to Kanban auto-creates a real ListSection and assigns unsectioned root tasks to it.
    let sections = store.list_sections(Some(list.id)).unwrap();
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].name, "Not Sectioned");
    assert_eq!(store.task(t_open.id).unwrap().unwrap().section_id, Some(sections[0].id));
    assert_eq!(store.task(t_done.id).unwrap().unwrap().section_id, Some(sections[0].id));

    // Add a second section and move t_done into it: no separate rightmost Completed column is created.
    let sec2 = store.add_list_section(Some(list.id), "Review").unwrap();
    store.set_section(t_done.id, Some(sec2.id)).unwrap();
    cx.update(|_, cx| view.update(cx, |view, cx| view.reload(cx)));
    cx.run_until_parked();

    cx.update(|_, cx| {
        let v = view.read(cx);
        let cols = v.board_columns(i18n::t(cx));
        assert_eq!(cols.len(), 2, "only the two real sections exist, no separate Completed column");
        assert_eq!(cols[0].title, "Not Sectioned");
        assert_eq!(cols[0].open_count(), 2);
        assert_eq!(cols[0].pinned_cards.len(), 1);
        assert_eq!(cols[0].open_cards.len(), 1);
        assert_eq!(cols[0].open_cards[0].1.len(), 1, "completed subtask stays nested inside its open parent card");
        assert!(cols[0].completed_cards.is_empty());

        assert_eq!(cols[1].title, "Review");
        assert_eq!(cols[1].open_count(), 0, "header count counts only open tasks");
        assert_eq!(cols[1].completed_cards.len(), 1, "completed task stays inside its own section column");
        assert_eq!(cols[1].completed_cards[0].0.id, t_done.id);
    });

    // Grouping by Priority keeps subtasks nested under their root task's priority column.
    cx.update(|window, cx| view.update(cx, |view, cx| view.set_sort(None, Some(GroupBy::Priority), window, cx)));
    cx.update(|_, cx| {
        let v = view.read(cx);
        let cols = v.board_columns(i18n::t(cx));
        let high = cols.iter().find(|c| c.title == "High").expect("High priority column");
        assert_eq!(high.open_cards.len(), 1);
        assert_eq!(high.open_cards[0].0.id, t_open.id);
        assert_eq!(high.open_cards[0].1.len(), 1, "Low-priority subtask stays inside High-priority root card");
        assert!(!cols.iter().any(|c| c.title == "Low"), "subtask does not split into its own Low column");
    });
}

#[gpui::test]
fn sidebar_drag_and_drop_reorders_smart_lists_sections_lists_folders_tags_and_filters(
    cx: &mut TestAppContext,
) {
    use fern_core::{
        FilterRules, SidebarSection, SmartFilter, SmartList,
    };
    use gpui::{Modifiers, MouseButton, point, size};

    let store = Rc::new(Store::open_in_memory().unwrap());
    let l1 = store.create_list("Alpha").unwrap();
    let l2 = store.create_list("Beta").unwrap();
    let l3 = store.create_list("Gamma").unwrap();
    let task = store.add_task("Draggable task").unwrap();
    store.add_tag("work", None, None).unwrap();
    store.add_tag("home", None, None).unwrap();
    store.add_tag("errands", None, None).unwrap();
    let f1 = SmartFilter { id: Id::now_v7(), name: "Filter One".into(), rules: FilterRules::default() };
    let f2 = SmartFilter { id: Id::now_v7(), name: "Filter Two".into(), rules: FilterRules::default() };
    store.save_filter(&f1).unwrap();
    store.save_filter(&f2).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    // Tall window so smart lists, lists, tags, and filters are all visible without scrolling.
    cx.simulate_resize(size(px(1200.), px(1400.)));
    cx.run_until_parked();

    let drag = |cx: &mut gpui::VisualTestContext, from: gpui::Point<Pixels>, to: gpui::Point<Pixels>| {
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(from, None, modifiers);
        cx.simulate_mouse_down(from, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(point(from.x + px(6.), from.y), Some(MouseButton::Left), modifiers);
        cx.simulate_mouse_move(to, Some(MouseButton::Left), modifiers);
        cx.simulate_mouse_up(to, MouseButton::Left, modifiers);
        cx.run_until_parked();
    };

    let bounds = |cx: &mut gpui::VisualTestContext, selector: String| {
        let selector: &'static str = selector.leak();
        cx.debug_bounds(selector).unwrap_or_else(|| panic!("missing {selector}"))
    };

    // 1. Reorder top smart lists with live-sliding BEFORE MouseUp + monotonic 2px sweep.
    let today_b = bounds(cx, "lists-item-view-Today".into());
    let inbox_b = bounds(cx, "lists-item-view-Inbox".into());
    let modifiers = Modifiers::default();
    cx.simulate_mouse_move(today_b.center(), None, modifiers);
    cx.simulate_mouse_down(today_b.center(), MouseButton::Left, modifiers);
    cx.simulate_mouse_move(point(today_b.center().x + px(6.), today_b.center().y), Some(MouseButton::Left), modifiers);
    cx.run_until_parked();
    let mut prev_pos = 2usize;
    let mut y = f32::from(today_b.center().y);
    let target_y = f32::from(inbox_b.top()) + 5.;
    while y > target_y {
        y = (y - 2.).max(target_y);
        cx.simulate_mouse_move(point(inbox_b.center().x, px(y)), Some(MouseButton::Left), modifiers);
        cx.run_until_parked();
        let cur_pos = cx.update(|_, cx| {
            view.read(cx).preferences.smart_list_order.iter().position(|&s| s == SmartList::Today).unwrap_or(2)
        });
        assert!(cur_pos <= prev_pos, "smart list drag must never oscillate: {cur_pos} > {prev_pos}");
        prev_pos = cur_pos;
    }
    // Assert Inbox physically slid down BEFORE MouseUp!
    let inbox_live = bounds(cx, "lists-item-view-Inbox".into());
    assert!(inbox_live.top() > inbox_b.top() + px(20.), "Inbox slid down live before MouseUp");
    cx.simulate_mouse_up(point(inbox_b.center().x, px(target_y)), MouseButton::Left, modifiers);
    cx.run_until_parked();
    assert_eq!(store.preferences().unwrap().smart_list_order[0], SmartList::Today);
    assert_eq!(store.preferences().unwrap().smart_list_order[1], SmartList::Inbox);

    // 2. Reorder variable-height middle sections with 2px incremental sweep (zero jitter!).
    let filters_h = bounds(cx, "filters-heading".into());
    let lists_h = bounds(cx, "lists-heading".into());
    cx.simulate_mouse_move(filters_h.center(), None, modifiers);
    cx.simulate_mouse_down(filters_h.center(), MouseButton::Left, modifiers);
    cx.simulate_mouse_move(point(filters_h.center().x + px(6.), filters_h.center().y), Some(MouseButton::Left), modifiers);
    cx.run_until_parked();
    let mut prev_sec_pos = 2usize;
    let mut y = f32::from(filters_h.center().y);
    let sec_target_y = f32::from(lists_h.top()) + 5.;
    while y > sec_target_y {
        y = (y - 4.).max(sec_target_y);
        cx.simulate_mouse_move(point(lists_h.center().x, px(y)), Some(MouseButton::Left), modifiers);
        cx.run_until_parked();
        let cur_pos = cx.update(|_, cx| {
            view.read(cx).preferences.sidebar_section_order.iter().position(|&s| s == SidebarSection::Filters).unwrap_or(2)
        });
        assert!(cur_pos <= prev_sec_pos, "section block drag must never oscillate: {cur_pos} > {prev_sec_pos}");
        prev_sec_pos = cur_pos;
    }
    let lists_h_live = bounds(cx, "lists-heading".into());
    assert!(lists_h_live.top() > lists_h.top() + px(20.), "Lists section slid down live before MouseUp");
    cx.simulate_mouse_up(point(lists_h.center().x, px(sec_target_y)), MouseButton::Left, modifiers);
    cx.run_until_parked();
    assert_eq!(
        store.preferences().unwrap().sidebar_section_order,
        vec![SidebarSection::Filters, SidebarSection::Lists, SidebarSection::Tags]
    );

    // 3. Reorder filters (live-sliding before MouseUp).
    let f2_b = bounds(cx, format!("lists-item-filter-{}", f2.id));
    let f1_b = bounds(cx, format!("lists-item-filter-{}", f1.id));
    cx.simulate_mouse_move(f2_b.center(), None, modifiers);
    cx.simulate_mouse_down(f2_b.center(), MouseButton::Left, modifiers);
    cx.simulate_mouse_move(point(f2_b.center().x + px(6.), f2_b.center().y), Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_move(point(f1_b.center().x, f1_b.top() + px(5.)), Some(MouseButton::Left), modifiers);
    cx.run_until_parked();
    assert!(bounds(cx, format!("lists-item-filter-{}", f1.id)).top() > f1_b.top() + px(20.));
    cx.simulate_mouse_up(point(f1_b.center().x, f1_b.top() + px(5.)), MouseButton::Left, modifiers);
    cx.run_until_parked();
    let filter_names: Vec<String> = store.filters().unwrap().into_iter().map(|f| f.name).collect();
    assert_eq!(filter_names, vec!["Filter Two", "Filter One"]);

    // 4a. Escape cancels an in-progress list drag and restores pre-drag order cleanly.
    let list_names = |s: &Store| -> Vec<String> { s.lists().unwrap().into_iter().map(|l| l.name).collect() };
    let l3_b = bounds(cx, format!("lists-item-list-{}", l3.id));
    let l1_b = bounds(cx, format!("lists-item-list-{}", l1.id));
    cx.simulate_mouse_move(l3_b.center(), None, modifiers);
    cx.simulate_mouse_down(l3_b.center(), MouseButton::Left, modifiers);
    cx.simulate_mouse_move(point(l3_b.center().x + px(6.), l3_b.center().y), Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_move(point(l1_b.center().x, l1_b.top() + px(4.)), Some(MouseButton::Left), modifiers);
    cx.run_until_parked();
    assert!(bounds(cx, format!("lists-item-list-{}", l1.id)).top() > l1_b.top() + px(20.));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_mouse_up(point(l1_b.center().x, l1_b.top() + px(4.)), MouseButton::Left, modifiers);
    cx.run_until_parked();
    assert_eq!(list_names(&store), vec!["Alpha", "Beta", "Gamma"]);

    // 4b. List 3-zone drag with 2px sweep: reorder Gamma before Alpha (zero oscillation!).
    let l3_b = bounds(cx, format!("lists-item-list-{}", l3.id));
    let l1_b = bounds(cx, format!("lists-item-list-{}", l1.id));
    cx.simulate_mouse_move(l3_b.center(), None, modifiers);
    cx.simulate_mouse_down(l3_b.center(), MouseButton::Left, modifiers);
    cx.simulate_mouse_move(point(l3_b.center().x + px(6.), l3_b.center().y), Some(MouseButton::Left), modifiers);
    cx.run_until_parked();
    let mut prev_idx = 2usize;
    let mut y = f32::from(l3_b.center().y);
    let list_target_y = f32::from(l1_b.top()) + 4.;
    while y > list_target_y {
        y = (y - 2.).max(list_target_y);
        cx.simulate_mouse_move(point(l1_b.center().x, px(y)), Some(MouseButton::Left), modifiers);
        cx.run_until_parked();
        let cur_idx = cx.update(|_, cx| {
            view.read(cx).lists.iter().position(|l| l.id == l3.id).unwrap_or(2)
        });
        assert!(cur_idx <= prev_idx, "list drag must never oscillate: {cur_idx} > {prev_idx}");
        prev_idx = cur_idx;
    }
    cx.simulate_mouse_up(point(l1_b.center().x, px(list_target_y)), MouseButton::Left, modifiers);
    cx.run_until_parked();
    assert_eq!(list_names(&store), vec!["Gamma", "Alpha", "Beta"]);

    // 5. List middle-zone drag: hovering center of Alpha shifts Alpha by 0.0 px, highlights it,
    //    and releasing creates a folder & opens the rename dialog.
    let l2_b = bounds(cx, format!("lists-item-list-{}", l2.id));
    let l1_b = bounds(cx, format!("lists-item-list-{}", l1.id));
    cx.simulate_mouse_move(l2_b.center(), None, modifiers);
    cx.simulate_mouse_down(l2_b.center(), MouseButton::Left, modifiers);
    cx.simulate_mouse_move(point(l2_b.center().x + px(6.), l2_b.center().y), Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_move(l1_b.center(), Some(MouseButton::Left), modifiers);
    cx.run_until_parked();
    assert_eq!(bounds(cx, format!("lists-item-list-{}", l1.id)).top(), l1_b.top(), "highlighted row does not jump");
    cx.simulate_mouse_up(l1_b.center(), MouseButton::Left, modifiers);
    cx.run_until_parked();
    let folders = store.list_folders().unwrap();
    assert_eq!(folders.len(), 1);
    let folder = folders[0].clone();
    cx.update(|_, cx| assert!(view.read(cx).folder_dialog.is_some(), "grouping opens folder rename dialog"));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).folder_dialog.is_none(), "escape closes folder rename dialog"));
    let grouped: Vec<(String, Option<Id>)> = store.lists().unwrap().into_iter().map(|l| (l.name, l.folder)).collect();
    assert_eq!(
        grouped,
        vec![
            ("Gamma".into(), None),
            ("Alpha".into(), Some(folder.id)),
            ("Beta".into(), Some(folder.id)),
        ]
    );

    // 6. Folder drag-reorder: drag folder above Gamma.
    let folder_b = bounds(cx, format!("lists-folder-{}", folder.id));
    let l3_b = bounds(cx, format!("lists-item-list-{}", l3.id));
    drag(cx, folder_b.center(), point(l3_b.center().x, l3_b.top() + px(5.)));
    assert_eq!(list_names(&store), vec!["Alpha", "Beta", "Gamma"]);

    // 7. Collapse folder, hover dragged Gamma over its middle zone for 900ms -> auto-expands!
    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.panel_event(lists_panel::PanelEvent::ToggleFolder(folder.id), window, cx));
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).collapsed_folders.contains(&folder.id)));
    let l3_b = bounds(cx, format!("lists-item-list-{}", l3.id));
    let folder_b = bounds(cx, format!("lists-folder-{}", folder.id));
    let modifiers = Modifiers::default();
    cx.simulate_mouse_move(l3_b.center(), None, modifiers);
    cx.simulate_mouse_down(l3_b.center(), MouseButton::Left, modifiers);
    cx.simulate_mouse_move(point(l3_b.center().x + px(6.), l3_b.center().y), Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_move(folder_b.center(), Some(MouseButton::Left), modifiers);
    cx.background_executor.advance_clock(std::time::Duration::from_millis(950));
    cx.run_until_parked();
    cx.update(|_, cx| assert!(!view.read(cx).collapsed_folders.contains(&folder.id), "folder auto-expanded on drag hover"));
    // Drop Gamma into the folder.
    let folder_b = bounds(cx, format!("lists-folder-{}", folder.id));
    cx.simulate_mouse_up(folder_b.center(), MouseButton::Left, modifiers);
    cx.run_until_parked();
    assert_eq!(store.lists().unwrap().iter().find(|l| l.id == l3.id).unwrap().folder, Some(folder.id));

    // 8. Tag drag: nest `errands` into `work` (middle zone), then drag parent `work` after `home`.
    let errands_b = bounds(cx, "lists-item-tag-errands".into());
    let work_b = bounds(cx, "lists-item-tag-work".into());
    drag(cx, errands_b.center(), work_b.center());
    assert_eq!(
        store.tags().unwrap().into_iter().find(|t| t.name == "errands").unwrap().parent.as_deref(),
        Some("work")
    );

    let work_b = bounds(cx, "lists-item-tag-work".into());
    let home_b = bounds(cx, "lists-item-tag-home".into());
    drag(cx, work_b.center(), point(home_b.center().x, home_b.bottom() - px(5.)));
    let top_tags: Vec<String> = store.tags().unwrap().into_iter().filter(|t| t.parent.is_none()).map(|t| t.name).collect();
    assert_eq!(top_tags, vec!["home", "work"]);

    // 9. Promote sub-tag `errands` to top-level by dropping on Tags heading.
    let errands_b = bounds(cx, "lists-item-tag-errands".into());
    let tags_h = bounds(cx, "tags-heading".into());
    drag(cx, errands_b.center(), tags_h.center());
    let top_tags: Vec<String> = store.tags().unwrap().into_iter().filter(|t| t.parent.is_none()).map(|t| t.name).collect();
    assert_eq!(top_tags, vec!["errands", "home", "work"]);

    // 10. Existing task drop onto a list still works cleanly.
    assert_eq!(store.task(task.id).unwrap().unwrap().list_id, None);
    cx.update(|_, cx| {
        view.update(cx, |v, cx| v.drop_on_view(View::List(l1.id), task.id, cx));
    });
    assert_eq!(store.task(task.id).unwrap().unwrap().list_id, Some(l1.id));
}

#[gpui::test]
fn nested_tasks_drag_unnest_unlink_and_toggle_all(cx: &mut TestAppContext) {
    use gpui::{Modifiers, point, size};

    let store = Rc::new(Store::open_in_memory().unwrap());
    let parent = store.add_task("Parent").unwrap();
    let child = store.add_subtask(parent.id, "Child").unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(1100.), px(700.)));
    cx.run_until_parked();

    let bounds = |cx: &mut gpui::VisualTestContext, selector: String| {
        let selector: &'static str = selector.leak();
        cx.debug_bounds(selector).unwrap_or_else(|| panic!("missing {selector}"))
    };
    let drag = |cx: &mut gpui::VisualTestContext, from: gpui::Point<Pixels>, to: gpui::Point<Pixels>| {
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(from, None, modifiers);
        cx.simulate_mouse_down(from, MouseButton::Left, modifiers);
        let step = if to.x < from.x { -6. } else { 6. };
        cx.simulate_mouse_move(point(from.x + px(step), from.y), Some(MouseButton::Left), modifiers);
        cx.simulate_mouse_move(to, Some(MouseButton::Left), modifiers);
        cx.run_until_parked();
        cx.simulate_mouse_up(to, MouseButton::Left, modifiers);
        cx.run_until_parked();
    };
    let visible_ids = |cx: &mut gpui::VisualTestContext| -> Vec<Id> {
        cx.update(|_, cx| view.read(cx).visible_task_ids(i18n::t(cx)))
    };

    // 1. Two-task list: drag Child left by grabbing directly on its title text -> unnested without opening title edit.
    let child_title_b = bounds(cx, format!("task-title-{}", child.id));
    let grab_title = child_title_b.center();
    drag(cx, grab_title, point(grab_title.x - px(48.), grab_title.y));
    assert_eq!(store.task(child.id).unwrap().unwrap().parent_id, None, "drag left from title unnests child");
    cx.update(|_, cx| assert!(view.read(cx).title_edit.is_none(), "dragging title does not enter inline edit"));
    assert_eq!(visible_ids(cx), vec![parent.id, child.id]);

    // 2. Drag Child right on its own row -> nests back under Parent.
    let child_b = bounds(cx, format!("task-row-{}", child.id));
    let grab = point(child_b.left() + px(160.), child_b.center().y);
    drag(cx, grab, point(grab.x + px(48.), grab.y));
    assert_eq!(store.task(child.id).unwrap().unwrap().parent_id, Some(parent.id), "drag right nests child");

    // 3. Drag Child down into empty whitespace below the list -> unnested at bottom.
    let child_b = bounds(cx, format!("task-row-{}", child.id));
    let grab = point(child_b.left() + px(160.), child_b.center().y);
    drag(cx, grab, point(grab.x, child_b.bottom() + px(80.)));
    assert_eq!(store.task(child.id).unwrap().unwrap().parent_id, None, "drag below list unnests child");

    // 4. Drag Child onto bottom half of Parent moved right -> nests; then above Parent -> unnested first.
    let child_b = bounds(cx, format!("task-row-{}", child.id));
    let parent_b = bounds(cx, format!("task-row-{}", parent.id));
    let grab = point(child_b.left() + px(160.), child_b.center().y);
    drag(cx, grab, point(grab.x + px(48.), parent_b.bottom() - px(6.)));
    assert_eq!(store.task(child.id).unwrap().unwrap().parent_id, Some(parent.id));

    let child_b = bounds(cx, format!("task-row-{}", child.id));
    let parent_b = bounds(cx, format!("task-row-{}", parent.id));
    let grab = point(child_b.left() + px(160.), child_b.center().y);
    drag(cx, grab, point(grab.x, parent_b.top() + px(6.)));
    assert_eq!(store.task(child.id).unwrap().unwrap().parent_id, None, "drag above parent unnests");
    assert_eq!(visible_ids(cx), vec![child.id, parent.id], "moved before parent");

    // 5. Unlink parent via TaskDetail parent breadcrumb X button.
    store.link_parent(child.id, parent.id).unwrap();
    cx.update(|_, cx| view.update(cx, |v, cx| { v.reload(cx); v.select(child.id, cx); }));
    cx.run_until_parked();
    let unlink_b = bounds(cx, "detail-unlink-parent".into());
    cx.simulate_click(unlink_b.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(store.task(child.id).unwrap().unwrap().parent_id, None, "breadcrumb unlink clears parent");

    // 6. Unlink parent via Link Parent popover X button.
    store.link_parent(child.id, parent.id).unwrap();
    cx.update(|window, cx| view.update(cx, |v, cx| {
        v.reload(cx);
        v.start_detail(child.id, "link-parent", window, cx);
    }));
    cx.run_until_parked();
    let pop_unlink_b = bounds(cx, "link-parent-unlink".into());
    cx.simulate_click(pop_unlink_b.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(store.task(child.id).unwrap().unwrap().parent_id, None, "popover unlink clears parent");

    // 7. Unlink subtask from Parent's subtask list in TaskDetail.
    store.link_parent(child.id, parent.id).unwrap();
    cx.update(|_, cx| view.update(cx, |v, cx| { v.reload(cx); v.select(parent.id, cx); }));
    cx.run_until_parked();
    let sub_unlink_b = bounds(cx, format!("subtask-unlink-{}", child.id));
    cx.simulate_click(sub_unlink_b.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(store.task(child.id).unwrap().unwrap().parent_id, None, "subtask list unlink clears parent");

    // 8. ToggleAllSubtasks collapses and re-expands all parents.
    store.link_parent(child.id, parent.id).unwrap();
    cx.update(|_, cx| view.update(cx, |v, cx| v.reload(cx)));
    cx.run_until_parked();
    assert_eq!(visible_ids(cx), vec![parent.id, child.id]);
    cx.update(|_, cx| view.update(cx, |v, cx| v.toggle_all_subtasks_now(cx)));
    cx.run_until_parked();
    assert_eq!(visible_ids(cx), vec![parent.id], "all subtasks collapsed");
    cx.update(|_, cx| view.update(cx, |v, cx| v.toggle_all_subtasks_now(cx)));
    cx.run_until_parked();
    assert_eq!(visible_ids(cx), vec![parent.id, child.id], "all subtasks expanded again");

    // 9. Kanban: dragging a subtask row out of its parent card unnests it.
    cx.update(|_, cx| view.update(cx, |v, cx| v.set_layout(true, false, cx)));
    cx.run_until_parked();
    let sub_card_b = bounds(cx, format!("card-sub-{}", child.id));
    // Drag the subtask above its own parent card to unnest it.
    drag(cx, sub_card_b.center(), point(sub_card_b.left() + px(24.), sub_card_b.top() - px(24.)));
    assert_eq!(store.task(child.id).unwrap().unwrap().parent_id, None, "kanban subtask drag unnests child");
}

#[gpui::test]
fn kanban_nested_tasks_drag_nest_outdent_reorder_and_cross_column(cx: &mut TestAppContext) {
    use gpui::{Modifiers, point, size};

    let store = Rc::new(Store::open_in_memory().unwrap());
    let list = store.create_list("Board").unwrap();
    let sec1 = store.add_list_section(Some(list.id), "Todo").unwrap();
    let sec2 = store.add_list_section(Some(list.id), "Doing").unwrap();
    let parent = store.add_task("Parent").unwrap();
    let child = store.add_subtask(parent.id, "Child").unwrap();
    let sibling = store.add_task("Sibling").unwrap();
    let other = store.add_task("Other").unwrap();
    for t in [&parent, &child, &sibling, &other] {
        store.set_list(t.id, Some(list.id)).unwrap();
    }
    store.set_section(parent.id, Some(sec1.id)).unwrap();
    store.set_section(sibling.id, Some(sec1.id)).unwrap();
    store.set_section(other.id, Some(sec2.id)).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = TodoView::new(store.clone(), window, cx);
        view.set_view(View::List(list.id), cx);
        view.set_layout(true, false, cx);
        view
    });
    cx.simulate_resize(size(px(1100.), px(700.)));
    cx.run_until_parked();

    let bounds = |cx: &mut gpui::VisualTestContext, selector: String| {
        let selector: &'static str = selector.leak();
        cx.debug_bounds(selector).unwrap_or_else(|| panic!("missing {selector}"))
    };
    let drag = |cx: &mut gpui::VisualTestContext, from: gpui::Point<Pixels>, to: gpui::Point<Pixels>| {
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(from, None, modifiers);
        cx.simulate_mouse_down(from, MouseButton::Left, modifiers);
        let step = if to.x < from.x { -6. } else { 6. };
        cx.simulate_mouse_move(point(from.x + px(step), from.y), Some(MouseButton::Left), modifiers);
        cx.simulate_mouse_move(to, Some(MouseButton::Left), modifiers);
        cx.run_until_parked();
        cx.simulate_mouse_up(to, MouseButton::Left, modifiers);
        cx.run_until_parked();
    };
    let visible_ids = |cx: &mut gpui::VisualTestContext| -> Vec<Id> {
        cx.update(|_, cx| view.read(cx).visible_task_ids(i18n::t(cx)))
    };

    // 1. Nest Sibling under Child (depth 2) by dragging right onto Child's bottom half.
    let sib_b = bounds(cx, format!("kanban-card-{}", sibling.id));
    let child_b = bounds(cx, format!("card-sub-{}", child.id));
    drag(cx, sib_b.center(), point(sib_b.center().x + px(56.), child_b.bottom() - px(6.)));
    assert_eq!(store.task(sibling.id).unwrap().unwrap().parent_id, Some(child.id), "nests at depth 2 under child");
    assert_eq!(visible_ids(cx), vec![parent.id, child.id, sibling.id, other.id]);

    // 2. Per-level collapse: collapsing Child hides Sibling while keeping Child visible.
    cx.update(|_, cx| {
        view.update(cx, |v, cx| {
            v.collapsed_tasks.insert(child.id);
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert_eq!(visible_ids(cx), vec![parent.id, child.id, other.id], "collapsing depth-1 parent hides depth-2 child");
    cx.update(|_, cx| {
        view.update(cx, |v, cx| {
            v.collapsed_tasks.remove(&child.id);
            cx.notify();
        });
    });
    cx.run_until_parked();

    // 3. Outdent Sibling from depth 2 -> depth 1 (sibling of Child under Parent) by dragging left 24 px.
    let sib_b = bounds(cx, format!("card-sub-{}", sibling.id));
    drag(cx, sib_b.center(), point(sib_b.center().x - px(24.), sib_b.center().y));
    assert_eq!(store.task(sibling.id).unwrap().unwrap().parent_id, Some(parent.id), "outdents depth 2 -> depth 1");

    // 4. Reorder subtasks within Parent: drag Sibling onto top half of Child -> [Parent, Sibling, Child].
    let sib_b = bounds(cx, format!("card-sub-{}", sibling.id));
    let child_b = bounds(cx, format!("card-sub-{}", child.id));
    drag(cx, sib_b.center(), point(sib_b.center().x, child_b.top() + px(6.)));
    assert_eq!(store.task(sibling.id).unwrap().unwrap().parent_id, Some(parent.id), "stays under same parent");
    assert_eq!(visible_ids(cx), vec![parent.id, sibling.id, child.id, other.id], "reordered subtasks within card");

    // 5. Drag Child into empty space below column 0 -> unnests to root at bottom of column 0.
    let child_b = bounds(cx, format!("card-sub-{}", child.id));
    drag(cx, child_b.center(), point(child_b.center().x, child_b.bottom() + px(90.)));
    assert_eq!(store.task(child.id).unwrap().unwrap().parent_id, None, "drag below column unnests child");
    assert_eq!(store.task(child.id).unwrap().unwrap().section_id, Some(sec1.id), "stays in same section");

    // 6. Cross-column drag of a subtask (Sibling) onto column 1: unlinks from Parent and moves to sec2.
    let sib_b = bounds(cx, format!("card-sub-{}", sibling.id));
    let col1_b = bounds(cx, "kanban-column-1".into());
    drag(cx, sib_b.center(), point(col1_b.center().x, col1_b.top() + px(80.)));
    let sib_after = store.task(sibling.id).unwrap().unwrap();
    assert_eq!(sib_after.parent_id, None, "cross-column subtask drag unlinks from old parent");
    assert_eq!(sib_after.section_id, Some(sec2.id), "cross-column subtask drag moves to target section");

    // 7. Re-nest Child under Parent, then drag Parent across to column 1 -> Parent AND Child move together!
    store.link_parent(child.id, parent.id).unwrap();
    cx.update(|_, cx| view.update(cx, |v, cx| v.reload(cx)));
    cx.run_until_parked();
    let parent_b = bounds(cx, format!("kanban-card-{}", parent.id));
    let col1_b = bounds(cx, "kanban-column-1".into());
    drag(cx, parent_b.center(), point(col1_b.center().x, col1_b.top() + px(80.)));
    assert_eq!(store.task(parent.id).unwrap().unwrap().section_id, Some(sec2.id));
    assert_eq!(store.task(child.id).unwrap().unwrap().parent_id, Some(parent.id), "subtree stays attached");
    assert_eq!(store.task(child.id).unwrap().unwrap().section_id, Some(sec2.id), "subtask moves with parent");
}

#[gpui::test]
fn task_list_scrollbar_scrolls_and_lists_resize_edge_stays_unobstructed(cx: &mut TestAppContext) {
    use gpui::{Modifiers, point, size};

    let store = Rc::new(Store::open_in_memory().unwrap());
    for i in 0..30 {
        store.add_task(&format!("Task {i:02}")).unwrap();
    }
    for i in 0..25 {
        store.create_list(&format!("List {i:02}")).unwrap();
    }

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(1100.), px(520.)));
    cx.run_until_parked();

    let (first_id, last_id) = cx.update(|_, cx| {
        let entries = &view.read(cx).list_entries;
        let mut ids = entries.iter().filter_map(|e| match e {
            ListEntry::Row(Row::Task { task, .. }) => Some(task.id),
            _ => None,
        });
        let first = ids.next().expect("at least one task row");
        let last = ids.next_back().expect("multiple task rows");
        (first, last)
    });
    let first_sel: &'static str = format!("task-row-{first_id}").leak();
    let last_sel: &'static str = format!("task-row-{last_id}").leak();
    assert!(cx.debug_bounds(first_sel).is_some());
    assert!(cx.debug_bounds(last_sel).is_none());

    let viewport = cx.update(|_, cx| view.read(cx).list_state.viewport_bounds());
    let bar_bottom = point(viewport.right() - px(6.), viewport.bottom() - px(8.));
    let modifiers = Modifiers::default();
    cx.simulate_mouse_move(bar_bottom, None, modifiers);
    cx.simulate_click(bar_bottom, modifiers);
    assert!(
        cx.debug_bounds(last_sel).is_some(),
        "last task row should become visible after clicking task list scrollbar bottom"
    );

    let bar_top = point(bar_bottom.x, viewport.top() - px(10.));
    cx.simulate_mouse_down(bar_bottom, MouseButton::Left, modifiers);
    cx.simulate_mouse_move(bar_top, Some(MouseButton::Left), modifiers);
    cx.simulate_mouse_up(bar_top, MouseButton::Left, modifiers);
    assert!(
        cx.debug_bounds(first_sel).is_some(),
        "first task row should be visible again after dragging scrollbar to top"
    );

    // Verify lists_panel rows stay clear of the inset scrollbar thumb (8..14 px from the inner right edge)
    // and lists-resize at the right 6 px edge stays unobstructed.
    let initial_lists_y = cx.update(|_, cx| view.read(cx).lists_scroll.offset().y);
    let lists_bounds = cx.debug_bounds("lists-column").unwrap();
    let inbox_bounds = cx.debug_bounds("lists-item-view-Inbox").unwrap();
    assert!(
        inbox_bounds.right() <= lists_bounds.right() - px(16.),
        "sidebar rows must not extend under the inset scrollbar thumb: row right {:?}, panel right {:?}",
        inbox_bounds.right(),
        lists_bounds.right()
    );
    let lists_bar = point(lists_bounds.right() - px(10.), lists_bounds.top() + px(240.));
    cx.simulate_mouse_move(lists_bar, None, modifiers);
    cx.simulate_click(lists_bar, modifiers);
    let scrolled_lists_y = cx.update(|_, cx| view.read(cx).lists_scroll.offset().y);
    assert!(
        scrolled_lists_y < initial_lists_y,
        "lists-panel scrollbar should scroll when clicked inside inset track: {initial_lists_y:?} -> {scrolled_lists_y:?}"
    );

    let initial_width = cx.update(|_, cx| view.read(cx).lists_width);
    let resize_start = point(lists_bounds.right() - px(3.), lists_bounds.top() + px(140.));
    cx.simulate_mouse_move(resize_start, None, modifiers);
    cx.simulate_mouse_down(resize_start, MouseButton::Left, modifiers);
    cx.simulate_mouse_move(
        point(resize_start.x + px(6.), resize_start.y),
        Some(MouseButton::Left),
        modifiers,
    );
    cx.simulate_mouse_move(
        point(resize_start.x + px(40.), resize_start.y),
        Some(MouseButton::Left),
        modifiers,
    );
    cx.simulate_mouse_up(
        point(resize_start.x + px(40.), resize_start.y),
        MouseButton::Left,
        modifiers,
    );
    cx.run_until_parked();
    let resized_width = cx.update(|_, cx| view.read(cx).lists_width);
    assert!(
        resized_width > initial_width + 20.,
        "lists-resize should remain unobstructed at the right 6 px edge: {initial_width:?} -> {resized_width:?}"
    );
}

#[gpui::test]
fn multi_select_and_batch_edit_actions(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let t1 = store.add_task("First task").unwrap();
    let t2 = store.add_task("Second task").unwrap();
    let t3 = store.add_task("Third task").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        crate::keymap::load_default(&assets::Assets, cx).unwrap();
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    cx.simulate_resize(gpui::size(px(1300.), px(720.)));
    cx.run_until_parked();

    // Select all visible tasks via SelectAllTasks action -> opens batch edit panel.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| window.focus(&v.list_focus, cx));
        window.dispatch_action(Box::new(SelectAllTasks), cx);
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(view.read(cx).selected_task_ids().len(), 3));
    assert!(cx.debug_bounds("batch-edit-panel").is_some());

    // Escape clears multi-selection.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| assert!(!view.read(cx).is_multi_selecting()));
    assert!(cx.debug_bounds("batch-edit-panel").is_none());

    // Shift+Click from first row to second row selects only those two tasks.
    let s1: &'static str = Box::leak(format!("task-row-{}", t1.id).into_boxed_str());
    let s2: &'static str = Box::leak(format!("task-row-{}", t2.id).into_boxed_str());
    let r1 = cx.debug_bounds(s1).unwrap();
    let r2 = cx.debug_bounds(s2).unwrap();
    let no_mod = Modifiers::default();
    let shift = Modifiers { shift: true, ..Default::default() };
    cx.simulate_click(point(r1.right() - px(60.), r1.center().y), no_mod);
    cx.simulate_click(point(r2.right() - px(60.), r2.center().y), shift);
    cx.run_until_parked();
    cx.update(|_, cx| {
        let ids = view.read(cx).selected_task_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&t1.id) && ids.contains(&t2.id));
    });
    assert!(cx.debug_bounds("batch-edit-panel").is_some());

    // Batch set high priority, in-progress, and complete on the two selected tasks.
    let prio_high = cx.debug_bounds("batch-prio-high").unwrap().center();
    cx.simulate_click(prio_high, no_mod);
    cx.run_until_parked();
    let in_prog = cx.debug_bounds("batch-in-progress").unwrap().center();
    cx.simulate_click(in_prog, no_mod);
    cx.run_until_parked();

    assert_eq!(store.task(t1.id).unwrap().unwrap().priority, Priority::High);
    assert_eq!(store.task(t2.id).unwrap().unwrap().priority, Priority::High);
    assert_eq!(store.task(t3.id).unwrap().unwrap().priority, Priority::None);
    assert!(store.task(t1.id).unwrap().unwrap().is_in_progress());
    assert!(store.task(t2.id).unwrap().unwrap().is_in_progress());
    assert!(!store.task(t3.id).unwrap().unwrap().is_in_progress());

    let batch_done = cx.debug_bounds("batch-complete").unwrap().center();
    cx.simulate_click(batch_done, no_mod);
    cx.run_until_parked();
    assert!(store.task(t1.id).unwrap().unwrap().is_completed());
    assert!(store.task(t2.id).unwrap().unwrap().is_completed());
    assert!(!store.task(t3.id).unwrap().unwrap().is_completed());

    // Clear selection restores single-task detail view.
    let clear = cx.debug_bounds("batch-clear").unwrap().center();
    cx.simulate_click(clear, no_mod);
    cx.run_until_parked();
    assert!(cx.debug_bounds("batch-edit-panel").is_none());
}

#[gpui::test]
fn workflow_kanban_partitions_and_transitions_task_status(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let t_todo = store.add_task("Backlog task").unwrap();
    let t_prog = store.add_task("Active work").unwrap();
    store.set_in_progress(t_prog.id, true).unwrap();
    let t_done = store.add_task("Finished").unwrap();
    store.set_completed(t_done.id, true).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        crate::keymap::load_default(&assets::Assets, cx).unwrap();
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    view.update(cx, |v, cx| {
        v.set_layout(true, false, cx);
        v.toggle_kanban_workflow(cx);
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        let cols = view.read(cx).board_columns(&i18n::EN);
        assert_eq!(cols.len(), 3);
        assert_eq!(cols[0].key, "workflow:todo");
        assert_eq!(cols[0].open_cards.iter().map(|(t, _)| t.id).collect::<Vec<_>>(), vec![t_todo.id]);
        assert_eq!(cols[1].key, "workflow:in_progress");
        assert_eq!(cols[1].open_cards.iter().map(|(t, _)| t.id).collect::<Vec<_>>(), vec![t_prog.id]);
        assert_eq!(cols[2].key, "workflow:completed");
        assert_eq!(cols[2].completed_cards.iter().map(|(t, _)| t.id).collect::<Vec<_>>(), vec![t_done.id]);
    });

    // Drop Backlog task onto In Progress, then Completed, then back to To Do.
    view.update(cx, |v, cx| v.drop_on_column(t_todo.id, DropTarget::InProgress, cx));
    assert!(store.task(t_todo.id).unwrap().unwrap().is_in_progress());
    view.update(cx, |v, cx| v.drop_on_column(t_todo.id, DropTarget::Complete, cx));
    assert!(store.task(t_todo.id).unwrap().unwrap().is_completed());
    view.update(cx, |v, cx| v.drop_on_column(t_todo.id, DropTarget::Todo, cx));
    let reopened = store.task(t_todo.id).unwrap().unwrap();
    assert!(!reopened.is_completed() && !reopened.is_in_progress());
}

#[gpui::test]
fn timeline_resize_weekends_and_secondary_timezone(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let day = TodoView::today();
    let t = store.add_task("Timeline Bar").unwrap();
    store.set_due(t.id, Some(day), chrono::NaiveTime::from_hms_opt(10, 0, 0)).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        crate::keymap::load_default(&assets::Assets, cx).unwrap();
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    view.update(cx, |v, cx| {
        v.set_layout(false, true, cx);
        v.set_timeline_color_by(ColorBy::Priority, cx);
        v.set_timeline_secondary_tz(Some("UTC".into()), cx);
        v.toggle_timeline_weekends(cx);
    });
    cx.run_until_parked();

    let target_end = day + Duration::days(3);
    let target_start = day - Duration::days(1);
    view.update(cx, |v, cx| {
        assert!(!v.timeline_show_weekends);
        assert_eq!(v.timeline_color_by, ColorBy::Priority);
        assert_eq!(v.timeline_secondary_tz.as_deref(), Some("UTC"));
        v.resize_timeline_task(
            &ResizedTask { id: t.id, title: "Timeline Bar".into(), start: day, end: day, edge: ResizeEdge::End },
            target_end,
            cx,
        );
        v.resize_timeline_task(
            &ResizedTask { id: t.id, title: "Timeline Bar".into(), start: day, end: target_end, edge: ResizeEdge::Start },
            target_start,
            cx,
        );
    });
    let updated = store.task(t.id).unwrap().unwrap();
    assert_eq!(updated.due_date, Some(target_start));
    assert_eq!(updated.end_date, Some(target_end));
}

#[gpui::test]
fn empty_trash_requires_confirmation_before_purging(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Deleted item").unwrap();
    store.delete_task(task.id).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        crate::keymap::load_default(&assets::Assets, cx).unwrap();
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));
    cx.update(|window, cx| view.update(cx, |v, cx| v.select_view(View::Trash, window, cx)));
    cx.run_until_parked();

    let btn = cx.debug_bounds("empty-trash-button").unwrap().center();
    cx.simulate_click(btn, Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("confirm-empty-trash-modal").is_some());
    assert!(store.task(task.id).unwrap().is_some(), "trash must not purge before confirmation");

    let cancel = cx.debug_bounds("empty-trash-cancel").unwrap().center();
    cx.simulate_click(cancel, Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("confirm-empty-trash-modal").is_none());
    assert!(store.task(task.id).unwrap().is_some());

    cx.simulate_click(btn, Modifiers::default());
    cx.run_until_parked();
    let confirm = cx.debug_bounds("empty-trash-confirm").unwrap().center();
    cx.simulate_click(confirm, Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("confirm-empty-trash-modal").is_none());
    assert!(store.task(task.id).unwrap().is_none(), "confirming permanently purges trash");
}

#[gpui::test]
fn batch_add_multiline_paste_creates_multiple_tasks(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        crate::keymap::load_default(&assets::Assets, cx).unwrap();
    });
    let (view, cx) = cx.add_window_view(|window, cx| TodoView::new(store.clone(), window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));
    cx.run_until_parked();

    cx.update(|window, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("Buy milk\nCall dentist\n\nSend report".into()));
        let input = view.read(cx).input.clone();
        window.focus(&input.focus_handle(cx), cx);
        window.dispatch_action(Box::new(crate::text_input::Paste), cx);
    });
    cx.run_until_parked();

    assert!(cx.debug_bounds("batch-add-confirm").is_some(), "multi-line paste opens batch add modal");
    let confirm = cx.debug_bounds("batch-add-confirm").unwrap().center();
    cx.simulate_click(confirm, Modifiers::default());
    cx.run_until_parked();

    assert!(cx.debug_bounds("batch-add-confirm").is_none());
    let titles: Vec<String> = store.tasks().unwrap().into_iter().map(|t| t.title).collect();
    assert_eq!(titles.len(), 3);
    assert!(titles.contains(&"Buy milk".to_owned()));
    assert!(titles.contains(&"Call dentist".to_owned()));
    assert!(titles.contains(&"Send report".to_owned()));
}
