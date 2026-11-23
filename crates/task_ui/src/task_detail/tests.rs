use super::*;
#[cfg(debug_assertions)]
use gpui::{ExternalPaths, FileDropEvent, InputEvent as _, TestAppContext};

#[cfg(debug_assertions)]
#[gpui::test]
fn blank_lines_keep_their_height_when_task_notes_lose_focus(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Task").unwrap();
    store.set_notes(task.id, "First.\n\nSecond.\n\n\nThird.").unwrap();
    let task = store.task(task.id).unwrap().unwrap();
    cx.update(theme::init);
    let (detail, cx) = cx.add_window_view(|window, cx| {
        let mut detail = TaskDetail::new(store, window, cx);
        detail.show(Some(task), cx);
        window.focus(&detail.notes.focus_handle(cx), cx);
        detail
    });
    let editor = cx.debug_bounds("task-notes-editor").expect("notes editor is visible");

    cx.update(|window, cx| window.focus(&detail.read(cx).title.focus_handle(cx), cx));
    cx.run_until_parked();
    let preview = cx.debug_bounds("task-notes-preview").expect("notes preview is visible");

    assert_eq!(preview.size.height, editor.size.height);
}

#[cfg(debug_assertions)]
#[gpui::test]
fn escape_in_task_detail_closes_the_picker_and_preserves_editor_focus(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Task").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut detail = TaskDetail::new(store, window, cx);
        detail.show(Some(task), cx);
        window.focus(&detail.title.focus_handle(cx), cx);
        detail
    });

    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| assert!(view.read(cx).title.focus_handle(cx).is_focused(window)));

    cx.update(|window, cx| view.update(cx, |detail, cx| {
        window.focus(&detail.notes.focus_handle(cx), cx);
        detail.open_tags(Popup::Tags, window, cx);
    }));
    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| {
        let detail = view.read(cx);
        assert!(!detail.has_popup());
        assert!(detail.notes.focus_handle(cx).is_focused(window));
    });

    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| assert!(view.read(cx).notes.focus_handle(cx).is_focused(window)));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn dropped_paths_stay_with_the_task_that_received_them(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("fern-drop-{}", uuid::Uuid::now_v7()));
    let file = dir.join("receipt.txt");
    let folder = dir.join("plans");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(&file, "paid").unwrap();
    std::fs::write(folder.join("next.txt"), "tomorrow").unwrap();

    let attachments_dir = dir.join("attachments");
    let store = Rc::new(Store::open_in_memory().unwrap());
    let first = store.add_task("First task").unwrap();
    let second = store.add_task("Second task").unwrap();
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(CurrentLocale(i18n::Locale::En));
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut detail = TaskDetail::new(store.clone(), window, cx);
        detail.session.attachments_dir = Some(attachments_dir.clone());
        detail.show(Some(first.clone()), cx);
        detail
    });

    let position = cx.debug_bounds("task-detail-body").unwrap().center();
    cx.simulate_event(FileDropEvent::Entered { position, paths: ExternalPaths([file.clone(), folder].into_iter().collect()) });
    cx.update(|window, cx| {
        window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
        assert!(store.attachments(first.id).unwrap().is_empty());
        view.update(cx, |detail, cx| detail.show(Some(second.clone()), cx));
    });
    cx.run_until_parked();

    let attached = store.attachments(first.id).unwrap();
    assert_eq!(attached.iter().map(|attachment| attachment.name.as_str()).collect::<Vec<_>>(), ["receipt.txt", "plans.zip"]);
    assert_eq!(std::fs::read_to_string(attached[0].path(&attachments_dir)).unwrap(), "paid");
    assert!(store.attachments(second.id).unwrap().is_empty());
    assert!(cx.debug_bounds("task-attachment").is_none());
    cx.update(|_, cx| view.update(cx, |detail, cx| detail.show(Some(first), cx)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("task-attachment").is_some());

    store.delete_task(second.id).unwrap();
    let deleted = store.task(second.id).unwrap();
    cx.update(|_, cx| view.update(cx, |detail, cx| detail.show(deleted, cx)));
    cx.run_until_parked();
    let position = cx.debug_bounds("task-detail-body").unwrap().center();
    cx.simulate_event(FileDropEvent::Entered { position, paths: ExternalPaths([file].into_iter().collect()) });
    cx.simulate_event(FileDropEvent::Submit { position });
    assert!(store.attachments(second.id).unwrap().is_empty());
    assert!(cx.debug_bounds("task-attachment").is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(debug_assertions)]
#[gpui::test]
fn archived_list_and_trashed_tasks_are_read_only_and_delete_forever_requests_confirmation(
    cx: &mut TestAppContext,
) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let list = store.create_list("ArchiveList").unwrap();
    let task = store.add_task("Archived task").unwrap();
    store.set_list(task.id, Some(list.id)).unwrap();
    store.set_notes(task.id, "- [ ] check me").unwrap();
    let check_item = store.add_checklist_item(task.id, "item 1", None).unwrap();
    store.add_comment(task.id, "read-only note").unwrap();
    store.set_list_archived(list.id, true).unwrap();

    let task = store.task(task.id).unwrap().unwrap();
    let task_id = task.id;
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(CurrentLocale(i18n::Locale::En));
    });
    let requested_purge = Rc::new(std::cell::RefCell::new(Vec::<Id>::new()));
    let requested_purge_sub = requested_purge.clone();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut detail = TaskDetail::new(store.clone(), window, cx);
        // Mirror TodoView: only active (non-archived) lists are passed into TaskDetail::set_lists.
        detail.set_lists(Vec::new());
        detail.show(Some(task), cx);
        detail
    });
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, RequestDeleteForever(id), _| {
            requested_purge_sub.borrow_mut().push(*id);
        })
        .detach();
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        let detail = view.read(cx);
        assert!(detail.is_read_only(), "archived-list task must be read-only");
        assert_eq!(detail.session.archived_list_name.as_deref(), Some("ArchiveList"));
        assert!(detail.title.read(cx).is_read_only());
        assert!(detail.notes.read(cx).is_read_only());
        assert!(detail.location_input.read(cx).is_read_only());
        assert!(
            detail.checklist.iter().all(|row| row.input.read(cx).is_read_only()),
            "checklist inputs must be read-only in archived list"
        );

        view.update(cx, |detail, cx| {
            detail.toggle_completed(cx);
            detail.open_popup(Popup::More, cx);
            detail.open_popup_from_ui(Popup::Due, window, cx);
            detail.start_checklist(cx);
            detail.toggle_markdown_check(0, cx);
            detail.toggle_checklist_item(check_item.id, true, cx);
            assert!(!detail.has_popup(), "read-only task must not open edit popups");
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("task-detail-more").is_none(), "read-only detail hides more menu");
    assert!(
        cx.debug_bounds("task-detail-checklist-toggle").is_none(),
        "read-only detail hides checklist mode toggle"
    );
    let stored = store.task(task_id).unwrap().unwrap();
    assert!(!stored.is_completed(), "read-only toggle_completed must be a no-op");
    assert_eq!(stored.notes, "- [ ] check me", "read-only markdown check toggle must be a no-op");
    assert!(!store.checklist(task_id).unwrap()[0].completed);

    // Unarchiving the list restores editability on refresh.
    store.set_list_archived(list.id, false).unwrap();
    let unarchived_lists = store.lists().unwrap();
    let refreshed = store.task(task_id).unwrap();
    cx.update(|_, cx| {
        view.update(cx, |detail, cx| {
            detail.set_lists(unarchived_lists);
            detail.refresh(refreshed, cx);
            assert!(!detail.is_read_only());
            assert!(!detail.title.read(cx).is_read_only());
            assert!(!detail.notes.read(cx).is_read_only());
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("task-detail-more").is_some());

    // Moving to Trash makes the task read-only again, and delete_forever requests confirmation
    // instead of purging immediately.
    store.delete_task(task_id).unwrap();
    let deleted = store.task(task_id).unwrap();
    cx.update(|_, cx| {
        view.update(cx, |detail, cx| {
            detail.refresh(deleted, cx);
            assert!(detail.is_read_only(), "trashed task must be read-only");
            assert!(detail.title.read(cx).is_read_only());
            detail.toggle_completed(cx);
            detail.delete_forever(cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(*requested_purge.borrow(), vec![task_id]);
    assert!(
        store.task(task_id).unwrap().is_some(),
        "delete_forever must wait for host confirmation before purging"
    );
}

#[cfg(debug_assertions)]
#[gpui::test]
fn cross_polarity_sticky_inputs_and_tokens_use_sticky_ink(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Cross-polarity note").unwrap();
    let item = store.add_checklist_item(task.id, "Open item", None).unwrap();
    store.add_subtask(task.id, "Child subtask").unwrap();
    store.add_comment(task.id, "A comment").unwrap();
    let task = store.task(task.id).unwrap().unwrap();

    // Dark app theme + light pastel yellow sticky (color 0 -> fg 0x24262b).
    cx.update(|cx| {
        theme::init(cx);
        theme::set(theme::Theme::dark(), cx);
        cx.set_global(CurrentLocale(i18n::Locale::En));
    });
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut detail = TaskDetail::new(store.clone(), window, cx);
        detail.show(Some(task.clone()), cx);
        detail.set_sticky_style(crate::sticky_settings::StickySettings { color: 0, ..Default::default() }, cx);
        detail
    });
    cx.run_until_parked();

    let light_fg: Hsla = gpui::rgb(0x24262b).into();
    let light_bg: Hsla = gpui::rgb(crate::sticky_settings::PALETTE[0].0).into();
    cx.update(|window, cx| {
        let detail = view.read(cx);
        let tokens = detail.sticky_tokens().expect("sticky tokens set");
        assert_eq!(tokens.bg, light_bg);
        assert_eq!(tokens.ink, light_fg);
        assert_eq!(tokens.muted, light_fg.opacity(0.65));
        assert_eq!(tokens.border, light_fg.opacity(0.20));
        assert_eq!(tokens.checkbox_border, light_fg.opacity(0.58));
        assert_eq!(tokens.surface, light_fg.opacity(0.08));

        for focus_input in [
            detail.title.clone(),
            detail.notes.clone(),
            detail.new_item.clone(),
            detail.subtask_input.clone(),
            detail.checklist[0].input.clone(),
        ] {
            window.focus(&focus_input.focus_handle(cx), cx);
            window.refresh();
            window.draw(cx).clear(cx);
            assert!(
                window.painted_quads().iter().any(|quad| quad.background == gpui::Background::from(light_fg)),
                "sticky caret must paint in light_fg instead of dark-theme accent"
            );
        }
    });

    // Adding a new checklist item inherits the active sticky ink override, and toggling
    // completion switches its text override to muted sticky ink.
    cx.update(|_, cx| {
        view.update(cx, |detail, cx| {
            detail.add_checklist_item("Second item", Some(item.id), cx);
            detail.sync_checklist(cx);
            detail.toggle_checklist_item(item.id, true, cx);
        });
    });
    cx.run_until_parked();

    // Light app theme + dark charcoal sticky (color 11 -> fg 0xf5f5f5).
    let dark_fg: Hsla = gpui::rgb(0xf5f5f5).into();
    cx.update(|window, cx| {
        theme::set(theme::Theme::light(), cx);
        view.update(cx, |detail, cx| {
            detail.set_sticky_style(crate::sticky_settings::StickySettings { color: 11, ..Default::default() }, cx);
            detail.comment_open = true;
            window.focus(&detail.comment_input.focus_handle(cx), cx);
        });
        window.refresh();
        window.draw(cx).clear(cx);
        assert!(
            window.painted_quads().iter().any(|quad| quad.background == gpui::Background::from(dark_fg)),
            "comment input caret on dark sticky must paint in dark_fg"
        );
        let title = view.read(cx).title.clone();
        window.focus(&title.focus_handle(cx), cx);
        window.refresh();
        window.draw(cx).clear(cx);
        assert!(
            window.painted_quads().iter().any(|quad| quad.background == gpui::Background::from(dark_fg)),
            "title caret on dark sticky must paint in dark_fg"
        );
    });
}

#[cfg(debug_assertions)]
#[gpui::test]
fn persisted_attachment_delete_requires_confirmation_and_supports_escape_cancel(
    cx: &mut TestAppContext,
) {
    let dir = std::env::temp_dir().join(format!("fern-att-confirm-{}", uuid::Uuid::now_v7()));
    let source = dir.join("spec.txt");
    let attachments_dir = dir.join("attachments");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&source, "keep me").unwrap();

    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Task with attachment").unwrap();
    let task_id = task.id;
    let prepared = PreparedAttachment::copy(task_id, &source, &attachments_dir).unwrap();
    let att = store.save_attachment(prepared, &attachments_dir).unwrap();
    let att_path = att.path(&attachments_dir);
    assert!(att_path.exists());

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let updated = Rc::new(std::cell::Cell::new(0usize));
    let updated_sub = updated.clone();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut detail = TaskDetail::new(store.clone(), window, cx);
        detail.session.attachments_dir = Some(attachments_dir.clone());
        detail.show(Some(task), cx);
        detail
    });
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, _: &TaskUpdated, _| {
            updated_sub.set(updated_sub.get() + 1);
        })
        .detach();
    });
    cx.run_until_parked();

    let delete_bounds = cx
        .debug_bounds("task-attachment-delete")
        .expect("delete button visible");
    cx.simulate_click(delete_bounds.center(), gpui::Modifiers::default());
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert!(matches!(
            &view.read(cx).pending_delete,
            Some(PendingDelete::Attachment(pending)) if pending.id == att.id
        ));
    });
    assert!(cx.debug_bounds("task-attachment-delete-modal").is_some());
    assert_eq!(store.attachments(task_id).unwrap().len(), 1);
    assert!(att_path.exists());
    assert_eq!(updated.get(), 0);

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).pending_delete.is_none()));
    assert!(cx.debug_bounds("task-attachment-delete-modal").is_none());
    assert_eq!(store.attachments(task_id).unwrap().len(), 1);
    assert!(att_path.exists());
    assert_eq!(updated.get(), 0);

    let delete_bounds = cx
        .debug_bounds("task-attachment-delete")
        .expect("delete button still visible");
    cx.simulate_click(delete_bounds.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let modal_bounds = cx
        .debug_bounds("task-attachment-delete-modal")
        .expect("delete confirmation modal bounds");
    let confirm_btn = gpui::point(
        modal_bounds.center().x + px(120.),
        modal_bounds.center().y + px(38.),
    );
    cx.simulate_click(confirm_btn, gpui::Modifiers::default());
    cx.run_until_parked();

    cx.update(|_, cx| assert!(view.read(cx).pending_delete.is_none()));
    assert!(cx.debug_bounds("task-attachment-delete-modal").is_none());
    assert!(store.attachments(task_id).unwrap().is_empty());
    assert!(!att_path.exists());
    assert_eq!(updated.get(), 1);

    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(debug_assertions)]
#[gpui::test]
fn persisted_comment_delete_requires_confirmation_and_supports_escape_cancel(
    cx: &mut TestAppContext,
) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Task with comment").unwrap();
    let other = store.add_task("Other task").unwrap();
    let task_id = task.id;
    let comment = store
        .add_comment(task_id, &"Комментарий ".repeat(10))
        .unwrap();

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(CurrentLocale(i18n::Locale::En));
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let updated = Rc::new(std::cell::Cell::new(0usize));
    let updated_sub = updated.clone();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut detail = TaskDetail::new(store.clone(), window, cx);
        detail.show(Some(task.clone()), cx);
        window.focus(&detail.title.focus_handle(cx), cx);
        detail
    });
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, _: &TaskUpdated, _| {
            updated_sub.set(updated_sub.get() + 1);
        })
        .detach();
    });
    cx.run_until_parked();

    let click_comment_delete = |cx: &mut gpui::VisualTestContext| {
        let row_bounds = cx
            .debug_bounds("task-comment-row")
            .expect("comment row visible");
        cx.simulate_mouse_move(row_bounds.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        let delete_bounds = cx
            .debug_bounds("task-comment-delete")
            .expect("comment delete button visible");
        cx.simulate_click(delete_bounds.center(), gpui::Modifiers::default());
        cx.run_until_parked();
    };

    click_comment_delete(cx);
    cx.update(|_, cx| {
        assert!(matches!(
            &view.read(cx).pending_delete,
            Some(PendingDelete::Comment { id, preview })
                if *id == comment.id && preview.ends_with('…') && preview.chars().count() <= 81
        ));
    });
    assert!(cx.debug_bounds("task-comment-delete-modal").is_some());
    assert_eq!(store.comments(task_id).unwrap().len(), 1);
    assert_eq!(updated.get(), 0);

    // Escape cancels pending deletion and restores prior focus.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let detail = view.read(cx);
        assert!(detail.pending_delete.is_none());
        assert!(detail.title.focus_handle(cx).is_focused(window));
    });
    assert!(cx.debug_bounds("task-comment-delete-modal").is_none());
    assert_eq!(store.comments(task_id).unwrap().len(), 1);

    // Switching tasks clears any stale pending deletion.
    click_comment_delete(cx);
    assert!(cx.debug_bounds("task-comment-delete-modal").is_some());
    cx.update(|_, cx| view.update(cx, |detail, cx| detail.show(Some(other), cx)));
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).pending_delete.is_none()));

    // Reopen task, trigger delete, and confirm via ConfirmDialog.
    cx.update(|_, cx| view.update(cx, |detail, cx| detail.show(Some(task), cx)));
    cx.run_until_parked();
    click_comment_delete(cx);
    let modal_bounds = cx
        .debug_bounds("task-comment-delete-modal")
        .expect("comment delete confirmation modal bounds");
    let confirm_btn = gpui::point(
        modal_bounds.center().x + px(120.),
        modal_bounds.center().y + px(38.),
    );
    cx.simulate_click(confirm_btn, gpui::Modifiers::default());
    cx.run_until_parked();

    cx.update(|_, cx| assert!(view.read(cx).pending_delete.is_none()));
    assert!(cx.debug_bounds("task-comment-delete-modal").is_none());
    assert!(store.comments(task_id).unwrap().is_empty());
    assert_eq!(updated.get(), 1);
}

#[test]
fn sizes() {
    assert_eq!(format_size(512), "512 B");
    assert_eq!(format_size(12 * 1024 + 5), "12 KB");
    assert_eq!(format_size(3_565_158), "3.4 MB");
}

#[test]
fn list_menu_search() {
    assert!(list_matches("Work", ""));
    assert!(list_matches("Work", " wo"));
    assert!(list_matches("Входящие", "вх"));
    assert!(!list_matches("Work", "home"));
}
