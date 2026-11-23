use super::*;
use desktop_common::actions::SyncNow;
use fern_core::ListDraft;
use gpui::{Modifiers, TestAppContext, size};
use std::cell::Cell;

fn setup_env(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
}

#[gpui::test]
fn things_view_construction_navigation_and_open_task(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();
    let proj = store
        .add_list(&ListDraft {
            name: "Ship v2".into(),
            ..Default::default()
        })
        .unwrap();
    let inbox_task = store.add_task("Review PR").unwrap();
    let proj_task = store
        .create_task(
            TaskDraft {
                title: "Write docs".into(),
                list_id: Some(proj.id),
                start_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let done_task = store
        .create_task(
            TaskDraft {
                title: "Released alpha".into(),
                list_id: Some(proj.id),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    store.set_completed(done_task.id, true).unwrap();

    setup_env(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(960.), px(640.)));
    cx.run_until_parked();

    cx.update(|_, cx| {
        let v = view.read(cx);
        assert_eq!(v.view(), ThingsViewKind::Today);
        assert_eq!(v.counts.inbox, 1);
        assert_eq!(v.counts.today, 1);
        assert_eq!(v.progress.get(&proj.id).map(|p| (p.completed, p.total)), Some((1, 2)));
    });

    // Navigate via open_task to project task -> opens Project view & expands inline card.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.open_task(proj_task.id, window, cx));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        assert_eq!(v.view(), ThingsViewKind::Project(proj.id));
        assert_eq!(v.expanded_task(), Some(proj_task.id));
    });
    let expanded_sel: &'static str = format!("things-expanded-{}", proj_task.id).leak();
    assert!(cx.debug_bounds(expanded_sel).is_some());

    // Navigate via open_task to Inbox task -> switches to Inbox & expands it.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.open_task(inbox_task.id, window, cx));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        assert_eq!(v.view(), ThingsViewKind::Inbox);
        assert_eq!(v.expanded_task(), Some(inbox_task.id));
    });
}

#[gpui::test]
fn inline_card_keyboard_expand_schedule_mutation_and_collapse(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();
    let task = store
        .create_task(
            TaskDraft {
                title: "Draft spec".into(),
                start_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .unwrap();

    setup_env_with_keymap(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(960.), px(640.)));
    cx.run_until_parked();

    // Focus root, Down selects task, Enter expands inline card.
    cx.update(|window, cx| {
        let fh = view.read(cx).focus_handle(cx);
        window.focus(&fh, cx);
    });
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(view.read(cx).selected_task(), Some(task.id)));

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(view.read(cx).expanded_task(), Some(task.id)));

    // Typing updates the task title directly in SQLite.
    cx.simulate_input(" v2");
    cx.run_until_parked();
    assert_eq!(store.task(task.id).unwrap().unwrap().title, "Draft spec v2");

    // Outside TextInput, `s` opens the When picker; clicking "This Evening" mutates schedule_bucket.
    cx.update(|window, cx| {
        let fh = view.read(cx).focus_handle(cx);
        window.focus(&fh, cx);
    });
    cx.simulate_keystrokes("s");
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).when_picker.is_some()));

    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.handle_when_action(
                WhenAction::Schedule {
                    start_date: Some(today),
                    bucket: ScheduleBucket::Evening,
                },
                window,
                cx,
            );
        });
    });
    cx.run_until_parked();
    let updated = store.task(task.id).unwrap().unwrap();
    assert_eq!(updated.schedule_bucket, ScheduleBucket::Evening);
    assert_eq!(updated.start_date, Some(today));

    // Esc collapses the expanded inline card.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(view.read(cx).expanded_task(), None));
}

struct DualThingsView {
    a: Entity<ThingsView>,
    b: Entity<ThingsView>,
}

impl Render for DualThingsView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.a.clone())
    }
}

#[gpui::test]
fn data_version_syncs_across_views_and_magic_plus_creates_task(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();

    setup_env(cx);
    let (dual, cx) = cx.add_window_view(|window, cx| DualThingsView {
        a: cx.new(|cx| ThingsView::new(store.clone(), window, cx)),
        b: cx.new(|cx| ThingsView::new(store.clone(), window, cx)),
    });
    cx.simulate_resize(size(px(1400.), px(640.)));
    cx.run_until_parked();

    let (view_a, view_b) = cx.update(|_, cx| {
        let d = dual.read(cx);
        (d.a.clone(), d.b.clone())
    });

    let search_count = Rc::new(Cell::new(0usize));
    let search_count_sub = search_count.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view_a, move |_, event, _| {
            if matches!(event, ThingsEvent::OpenSearch) {
                search_count_sub.set(search_count_sub.get() + 1);
            }
        })
        .detach();
    });

    // Clicking Magic Plus in view_a creates & expands a Today task and notifies view_b via DataVersion.
    let plus_bounds = cx.debug_bounds("things-magic-plus").unwrap();
    cx.simulate_click(plus_bounds.center(), Modifiers::default());
    cx.run_until_parked();

    let tasks = store.tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].start_date, Some(today));
    cx.update(|_, cx| {
        assert_eq!(view_a.read(cx).expanded_task(), Some(tasks[0].id));
        assert_eq!(view_b.read(cx).tasks().len(), 1);
        assert_eq!(view_b.read(cx).tasks()[0].id, tasks[0].id);
    });

    // Ctrl/Cmd+F emits ThingsEvent::OpenSearch.
    cx.update(|window, cx| {
        let fh = view_a.read(cx).focus_handle(cx);
        window.focus(&fh, cx);
    });
    #[cfg(target_os = "macos")]
    cx.simulate_keystrokes("cmd-f");
    #[cfg(not(target_os = "macos"))]
    cx.simulate_keystrokes("ctrl-f");
    cx.run_until_parked();
    assert_eq!(search_count.get(), 1);
}

fn setup_env_with_keymap(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        let errors = desktop_common::keymap::load_default(&assets::Assets, cx).unwrap();
        assert!(errors.is_empty(), "default keymap errors: {errors:?}");
    });
}

struct WorkspaceHarness {
    things: Entity<ThingsView>,
    sync_count: Rc<Cell<usize>>,
}

impl Render for WorkspaceHarness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sync_count = self.sync_count.clone();
        div()
            .size_full()
            .key_context("Workspace")
            .on_action(cx.listener(move |_, _: &SyncNow, _, _| {
                sync_count.set(sync_count.get() + 1);
            }))
            .child(self.things.clone())
    }
}

/// C9: Modal dialogs (TagDialog / ListDialog) must receive Space & shortcuts cleanly without
/// toggling/completing background tasks or firing NewTask/Search/When behind the modal.
#[gpui::test]
fn modal_dialogs_do_not_swallow_space_or_complete_background_tasks(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();
    let task = store
        .create_task(
            TaskDraft {
                title: "Keep open".into(),
                start_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .unwrap();

    setup_env_with_keymap(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(960.), px(640.)));
    cx.run_until_parked();

    let search_count = Rc::new(Cell::new(0usize));
    let search_sub = search_count.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, ev, _| {
            if matches!(ev, ThingsEvent::OpenSearch) {
                search_sub.set(search_sub.get() + 1);
            }
        })
        .detach();
    });

    // Select & expand task, then open TagDialog (which focuses its own TextInput).
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.expand_task(task.id, window, cx);
            v.handle_card_action(CardAction::OpenTagDialog, window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).tag_dialog.is_some()));

    // Pressing Space + typing must NOT complete the background task and must reach the modal input.
    cx.simulate_keystrokes("f o o space b a r");
    cx.run_until_parked();
    assert!(!store.task(task.id).unwrap().unwrap().is_closed());

    // Ctrl+N, Ctrl+F, Ctrl+S must be blocked while modal is open.
    #[cfg(target_os = "macos")]
    let (k_n, k_f, k_s) = ("cmd-n", "cmd-f", "cmd-s");
    #[cfg(not(target_os = "macos"))]
    let (k_n, k_f, k_s) = ("ctrl-n", "ctrl-f", "ctrl-s");
    cx.simulate_keystrokes(k_n);
    cx.simulate_keystrokes(k_f);
    cx.simulate_keystrokes(k_s);
    cx.run_until_parked();
    assert_eq!(store.tasks().unwrap().len(), 1);
    assert_eq!(search_count.get(), 0);
    cx.update(|_, cx| assert!(view.read(cx).when_picker.is_none()));

    // Submitting TagDialog saves "foo bar" (with the space preserved!) and tags the task.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let saved = store.task(task.id).unwrap().unwrap();
    assert!(!saved.is_closed());
    assert!(saved.tags.iter().any(|t| t == "foo bar"), "tags were {:?}", saved.tags);
}

/// H30: `secondary-s` in ThingsView inside Workspace fires `Workspace::SyncNow` both with and
/// without an active task, while bare `s` opens When when a task is active (and types `s`
/// inside a TextInput).
#[gpui::test]
fn ctrl_s_opens_when_inside_workspace_and_propagates_sync_when_no_task_active(
    cx: &mut TestAppContext,
) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();
    let task = store
        .create_task(
            TaskDraft {
                title: "Schedule me".into(),
                start_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .unwrap();

    setup_env_with_keymap(cx);
    let sync_count = Rc::new(Cell::new(0usize));
    let sync_clone = sync_count.clone();
    let (harness, cx) = cx.add_window_view(|window, cx| WorkspaceHarness {
        things: cx.new(|cx| ThingsView::new(store.clone(), window, cx)),
        sync_count: sync_clone,
    });
    cx.simulate_resize(size(px(960.), px(640.)));
    cx.run_until_parked();

    let things = cx.update(|_, cx| harness.read(cx).things.clone());
    #[cfg(target_os = "macos")]
    let shortcut = "cmd-s";
    #[cfg(not(target_os = "macos"))]
    let shortcut = "ctrl-s";

    // 1. With no task selected, secondary-s fires SyncNow and bare `s` does not open When.
    cx.update(|window, cx| {
        let fh = things.read(cx).focus_handle(cx);
        window.focus(&fh, cx);
    });
    cx.simulate_keystrokes(shortcut);
    cx.run_until_parked();
    assert_eq!(sync_count.get(), 1);
    cx.update(|_, cx| assert!(things.read(cx).when_picker.is_none()));

    cx.simulate_keystrokes("s");
    cx.run_until_parked();
    assert_eq!(sync_count.get(), 1);
    cx.update(|_, cx| assert!(things.read(cx).when_picker.is_none()));

    // 2. With a task selected, secondary-s still fires SyncNow (never shadowed by OpenWhen).
    cx.update(|window, cx| {
        things.update(cx, |v, cx| v.handle_card_action(CardAction::Select(task.id), window, cx));
    });
    cx.run_until_parked();
    cx.simulate_keystrokes(shortcut);
    cx.run_until_parked();
    assert_eq!(sync_count.get(), 2, "SyncNow must fire on secondary-s even with an active task");
    cx.update(|_, cx| assert!(things.read(cx).when_picker.is_none()));

    // 3. With a task selected (outside TextInput), bare `s` opens When without firing SyncNow.
    cx.simulate_keystrokes("s");
    cx.run_until_parked();
    assert_eq!(sync_count.get(), 2, "SyncNow must not fire when opening When");
    cx.update(|_, cx| assert!(things.read(cx).when_picker.is_some()));

    // 4. Inside a TextInput (expanded card title), bare `s` types 's' rather than opening When.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let before = cx.update(|window, cx| {
        things.update(cx, |v, cx| v.expand_task(task.id, window, cx));
        things
            .read(cx)
            .expanded
            .as_ref()
            .expect("card expanded")
            .title
            .read(cx)
            .text()
            .to_owned()
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("s");
    cx.run_until_parked();
    cx.update(|_, cx| {
        let view = things.read(cx);
        assert!(view.when_picker.is_none(), "`s` inside TextInput must not open When");
        let after = view.expanded.as_ref().expect("card still expanded").title.read(cx).text();
        assert!(!before.contains('s') && after.contains('s') && after.len() == before.len() + 1);
    });
    assert_eq!(sync_count.get(), 2);
}

/// H31: ReorderAfter only syncs the current view's grouping axis and is disabled in Logbook.
#[gpui::test]
fn reorder_after_respects_view_grouping_axis_and_blocks_logbook(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();
    let tomorrow = today + Duration::days(1);
    let proj_a = store
        .add_list(&ListDraft { name: "Alpha".into(), ..Default::default() })
        .unwrap();
    let proj_b = store
        .add_list(&ListDraft { name: "Beta".into(), ..Default::default() })
        .unwrap();
    let sec_b = store.add_list_section(Some(proj_a.id), "Phase 2").unwrap();

    let t_proj = store
        .create_task(
            TaskDraft {
                title: "Proj today".into(),
                list_id: Some(proj_a.id),
                start_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let t_due_only = store
        .create_task(
            TaskDraft {
                title: "Due only today".into(),
                due_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let t_evening = store
        .create_task(
            TaskDraft {
                title: "Evening other proj".into(),
                list_id: Some(proj_b.id),
                start_date: Some(today),
                schedule_bucket: ScheduleBucket::Evening,
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let t_proj_sec = store
        .create_task(
            TaskDraft {
                title: "Unscheduled in Phase 2".into(),
                list_id: Some(proj_a.id),
                section_id: Some(sec_b.id),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let t_done_a = store
        .create_task(
            TaskDraft {
                title: "Done A".into(),
                list_id: Some(proj_a.id),
                start_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let t_done_b = store
        .create_task(
            TaskDraft {
                title: "Done B".into(),
                list_id: Some(proj_b.id),
                start_date: Some(tomorrow),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    store.set_completed(t_done_a.id, true).unwrap();
    store.set_completed(t_done_b.id, true).unwrap();

    setup_env(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.run_until_parked();

    // Today: dropping t_proj after due-only task must preserve list_id AND start_date!
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.handle_card_action(
                CardAction::ReorderAfter {
                    dragged_id: t_proj.id,
                    target_id: t_due_only.id,
                },
                window,
                cx,
            );
        });
    });
    let after_today = store.task(t_proj.id).unwrap().unwrap();
    assert_eq!(after_today.list_id, Some(proj_a.id));
    assert_eq!(after_today.start_date, Some(today));
    assert_eq!(after_today.schedule_bucket, ScheduleBucket::None);

    // Today: dropping t_proj after Evening task changes schedule_bucket only, keeping proj_a!
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.handle_card_action(
                CardAction::ReorderAfter {
                    dragged_id: t_proj.id,
                    target_id: t_evening.id,
                },
                window,
                cx,
            );
        });
    });
    let after_eve = store.task(t_proj.id).unwrap().unwrap();
    assert_eq!(after_eve.list_id, Some(proj_a.id));
    assert_eq!(after_eve.start_date, Some(today));
    assert_eq!(after_eve.schedule_bucket, ScheduleBucket::Evening);

    // Project view: reordering onto unscheduled task in Phase 2 updates section_id ONLY, preserving start_date/bucket!
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.select_view(ThingsViewKind::Project(proj_a.id), window, cx);
            v.handle_card_action(
                CardAction::ReorderAfter {
                    dragged_id: t_proj.id,
                    target_id: t_proj_sec.id,
                },
                window,
                cx,
            );
        });
    });
    let after_proj = store.task(t_proj.id).unwrap().unwrap();
    assert_eq!(after_proj.section_id, Some(sec_b.id));
    assert_eq!(after_proj.start_date, Some(today));
    assert_eq!(after_proj.schedule_bucket, ScheduleBucket::Evening);

    // Logbook: ReorderAfter and MagicDropAfter are ignored (no mutation, no task created).
    let before_count = store.tasks().unwrap().len();
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.select_view(ThingsViewKind::Logbook, window, cx);
            v.handle_card_action(
                CardAction::ReorderAfter {
                    dragged_id: t_done_a.id,
                    target_id: t_done_b.id,
                },
                window,
                cx,
            );
            v.handle_card_action(CardAction::MagicDropAfter(t_done_a.id), window, cx);
        });
    });
    let done_a = store.task(t_done_a.id).unwrap().unwrap();
    assert_eq!(done_a.list_id, Some(proj_a.id));
    assert_eq!(done_a.start_date, Some(today));
    assert_eq!(store.tasks().unwrap().len(), before_count);
}

/// H32: Clicking EmptyState "New To-Do" CTA keeps the new card expanded; clicking
/// "Logged items" toggle does not collapse an expanded open card.
#[gpui::test]
fn empty_state_cta_and_logged_toggle_do_not_bubble_collapse(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    setup_env(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(960.), px(640.)));
    cx.run_until_parked();

    // Empty Today view -> clicking EmptyState CTA creates & keeps expanded card open!
    let empty = cx.debug_bounds("things-empty-state").unwrap();
    let cta_pt = gpui::point(empty.center().x, empty.center().y + px(47.));
    cx.simulate_click(cta_pt, Modifiers::default());
    cx.run_until_parked();
    let created_id = cx.update(|_, cx| view.read(cx).expanded_task());
    assert!(created_id.is_some(), "EmptyState CTA must not immediately collapse new card");

    // Now test Logged items toggle inside a Project while an open task is expanded.
    let proj = store
        .add_list(&ListDraft { name: "Work".into(), ..Default::default() })
        .unwrap();
    let open_t = store
        .create_task(
            TaskDraft { title: "Open".into(), list_id: Some(proj.id), ..Default::default() },
            false,
        )
        .unwrap();
    let done_t = store
        .create_task(
            TaskDraft { title: "Done".into(), list_id: Some(proj.id), ..Default::default() },
            false,
        )
        .unwrap();
    store.set_completed(done_t.id, true).unwrap();

    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.select_view(ThingsViewKind::Project(proj.id), window, cx);
            v.expand_task(open_t.id, window, cx);
        });
    });
    cx.run_until_parked();
    let toggle = cx.debug_bounds("things-logged-toggle").unwrap();
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        assert!(v.show_logged_in_project);
        assert_eq!(v.expanded_task(), Some(open_t.id));
    });
}

/// H33: Popover outside-clicks stop propagation so toggle buttons don't re-open on second
/// click and clicking outside a card popover closes only the popover (keeping the card expanded).
#[gpui::test]
fn popover_outside_clicks_do_not_reopen_or_collapse_expanded_card(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();
    let task = store
        .create_task(
            TaskDraft {
                title: "Popover test".into(),
                start_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .unwrap();

    setup_env(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(960.), px(640.)));
    cx.run_until_parked();

    // 1. Sidebar "+ New Project" menu: click opens, second click closes without re-opening!
    let add_btn = cx.debug_bounds("things-sidebar-add").unwrap();
    cx.simulate_click(add_btn.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).sidebar_add_menu));

    cx.simulate_click(add_btn.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| assert!(!view.read(cx).sidebar_add_menu));

    // 1b. Bottom-bar When & Move buttons: open on click, stay closed on second click.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.handle_card_action(CardAction::Select(task.id), window, cx));
    });
    cx.run_until_parked();
    let bar_when = cx.debug_bounds("things-bar-when").unwrap();
    cx.simulate_click(bar_when.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).when_picker.is_some()));
    cx.simulate_click(bar_when.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).when_picker.is_none()));

    let bar_move = cx.debug_bounds("things-bar-move").unwrap();
    cx.simulate_click(bar_move.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).bottom_move_open));
    cx.simulate_click(bar_move.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| assert!(!view.read(cx).bottom_move_open));

    // 2. Expanded card -> open When popover -> click outside on empty main list closes ONLY popover!
    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.expand_task(task.id, window, cx));
    });
    cx.run_until_parked();
    let card_when = cx.debug_bounds("things-card-when").unwrap();
    cx.simulate_click(card_when.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).when_picker.is_some()));

    // Click on bottom-right empty area of things-main-list (outside both popover and card).
    let list_bounds = cx.debug_bounds("things-main-list").unwrap();
    let outside_pt = gpui::point(list_bounds.right() - px(20.), list_bounds.bottom() - px(20.));
    cx.simulate_click(outside_pt, Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        assert!(v.when_picker.is_none(), "outside click must close When popover");
        assert_eq!(
            v.expanded_task(),
            Some(task.id),
            "first outside click must NOT collapse the expanded card"
        );
    });

    // Second click on empty main list now collapses the expanded card.
    cx.simulate_click(outside_pt, Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(view.read(cx).expanded_task(), None));
}

/// H34: Adding To-Do in an empty Area never spawns an invisible ghost task; MagicDropAfter on
/// due-date-only tasks lands visibly in Today/Upcoming.
#[gpui::test]
fn empty_area_and_due_only_magic_drop_never_spawn_ghost_tasks(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();
    let tomorrow = today + Duration::days(1);
    let folder = store.add_list_folder("Personal").unwrap();

    setup_env(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(960.), px(640.)));
    cx.run_until_parked();

    // Empty Area: EmptyState CTA opens ListDialog for New Project (0 ghost tasks created).
    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.select_view(ThingsViewKind::Area(folder.id), window, cx));
    });
    cx.run_until_parked();
    let empty = cx.debug_bounds("things-empty-state").unwrap();
    let cta_pt = gpui::point(empty.center().x, empty.center().y + px(47.));
    cx.simulate_click(cta_pt, Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).list_dialog.is_some()));
    assert!(store.tasks().unwrap().is_empty());

    // Closing modal and invoking create_new_todo_default in empty Area switches to Inbox & expands visibly.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.dismiss_transient(window, cx);
            v.create_new_todo_default(window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        assert_eq!(v.view(), ThingsViewKind::Inbox);
        assert!(v.expanded_task().is_some());
        assert!(v.tasks().iter().any(|t| Some(t.id) == v.expanded_task()));
    });
    // Escape cleans up the untouched empty task!
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.dismiss_transient(window, cx);
        });
    });
    assert!(store.tasks().unwrap().is_empty());

    // MagicDropAfter on due-date-only task in Today & Upcoming lands visibly in that view.
    let due_today = store
        .create_task(
            TaskDraft { title: "Due today".into(), due_date: Some(today), ..Default::default() },
            false,
        )
        .unwrap();
    let due_upcoming = store
        .create_task(
            TaskDraft { title: "Due tomorrow".into(), due_date: Some(tomorrow), ..Default::default() },
            false,
        )
        .unwrap();

    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.select_view(ThingsViewKind::Today, window, cx);
            v.handle_card_action(CardAction::MagicDropAfter(due_today.id), window, cx);
        });
    });
    let today_created = cx.update(|_, cx| view.read(cx).expanded_task()).unwrap();
    assert_eq!(store.task(today_created).unwrap().unwrap().start_date, Some(today));

    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.select_view(ThingsViewKind::Upcoming, window, cx);
            v.handle_card_action(CardAction::MagicDropAfter(due_upcoming.id), window, cx);
        });
    });
    let upcoming_created = cx.update(|_, cx| view.read(cx).expanded_task()).unwrap();
    assert_eq!(store.task(upcoming_created).unwrap().unwrap().start_date, Some(tomorrow));
}

/// H35: Completing an expanded task or collapsing "Logged items" in a Project clears
/// `self.expanded` so hidden tasks cannot hijack bottom-bar actions.
#[gpui::test]
fn completing_expanded_task_or_hiding_logged_clears_expanded_in_project(
    cx: &mut TestAppContext,
) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let proj = store
        .add_list(&ListDraft { name: "Ship".into(), ..Default::default() })
        .unwrap();
    let t1 = store
        .create_task(
            TaskDraft { title: "First".into(), list_id: Some(proj.id), ..Default::default() },
            false,
        )
        .unwrap();
    let t2 = store
        .create_task(
            TaskDraft { title: "Second".into(), list_id: Some(proj.id), ..Default::default() },
            false,
        )
        .unwrap();

    setup_env(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(960.), px(640.)));
    cx.run_until_parked();

    // Expand t1 in Project view and complete it -> self.expanded must clear!
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.select_view(ThingsViewKind::Project(proj.id), window, cx);
            v.expand_task(t1.id, window, cx);
            v.handle_card_action(CardAction::ToggleComplete(t1.id), window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert_eq!(v.expanded_task(), None);
        assert_eq!(v.active_target_task_id(), Some(t2.id));
        assert!(v.focus_handle(cx).is_focused(window));
    });

    // Show Logged items, expand completed t1, then collapse Logged items -> self.expanded clears!
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.show_logged_in_project = true;
            v.expand_task(t1.id, window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(view.read(cx).expanded_task(), Some(t1.id)));

    let toggle = cx.debug_bounds("things-logged-toggle").unwrap();
    cx.simulate_click(toggle.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        assert!(!v.show_logged_in_project);
        assert_eq!(v.expanded_task(), None);
        assert_ne!(v.active_target_task_id(), Some(t1.id));
    });
}

/// Medium #8, #10, #11, #12, #13, #14 & Low #16 regression coverage:
/// When picker focuses date_input without corrupting title, checklist draft flushes on
/// collapse, Inbox/Anytime/Upcoming drops & Deadline clear/clamp work, long names truncate,
/// and Unicode tags toggle case-insensitively.
#[gpui::test]
fn when_focus_checklist_flush_drops_deadline_truncation_and_unicode_tags(
    cx: &mut TestAppContext,
) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();
    let long_name = "Super Long Project Title That Would Otherwise Overflow And Push Buttons Off Screen Easily";
    let proj = store
        .add_list(&ListDraft { name: long_name.into(), ..Default::default() })
        .unwrap();
    let task = store
        .create_task(
            TaskDraft {
                title: "Original title".into(),
                list_id: Some(proj.id),
                start_date: Some(today),
                due_date: Some(today + Duration::days(2)),
                end_date: Some(today + Duration::days(4)),
                timezone: Some("America/New_York".into()),
                reminders: vec![15],
                constant_reminder: true,
                repeat: Some(fern_core::Recurrence::new(fern_core::Frequency::Daily, 1)),
                tags: vec!["Работа".into()],
                ..Default::default()
            },
            false,
        )
        .unwrap();

    setup_env_with_keymap(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(760.), px(640.)));
    cx.run_until_parked();

    // Long project chip in collapsed row must stay capped <= 120px.
    let chip_sel: &'static str = format!("things-project-chip-{}", task.id).leak();
    let chip_bounds = cx.debug_bounds(chip_sel).unwrap();
    assert!(chip_bounds.size.width <= px(120.5));

    // Expand card -> footer actions must stay inside the 760px window!
    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.expand_task(task.id, window, cx));
    });
    cx.run_until_parked();
    let footer_actions = cx.debug_bounds("things-card-footer-actions").unwrap();
    assert!(footer_actions.right() <= px(760.));

    // Open When picker -> date_input is focused, typing a date + Enter schedules WITHOUT editing title!
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.handle_card_action(CardAction::TogglePopover(CardPopover::When), window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let date_input = view.read(cx).when_picker.as_ref().unwrap().1.date_input.clone();
        assert!(date_input.focus_handle(cx).is_focused(window));
        date_input.update(cx, |i, cx| i.set_text("", cx));
    });
    let target_date = today + Duration::days(3);
    cx.simulate_input(&target_date.format("%Y-%m-%d").to_string());
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let after_when = store.task(task.id).unwrap().unwrap();
    assert_eq!(after_when.title, "Original title", "When typing must not corrupt task title");
    assert_eq!(after_when.start_date, Some(target_date));

    // Move Deadline past end_date -> end_date clamps; clear Deadline -> clears due/end_date ONLY,
    // preserving reminders, timezone, constant_reminder, and repeat.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.select_view(ThingsViewKind::Project(proj.id), window, cx);
            v.open_deadline_picker(PopoverAnchor::Card, DeadlineTarget::Task(task.id), cx);
            v.handle_deadline_action(
                DeadlineAction::Apply(Some(today + Duration::days(6))),
                window,
                cx,
            );
        });
    });
    let clamped = store.task(task.id).unwrap().unwrap();
    assert_eq!(clamped.due_date, Some(today + Duration::days(6)));
    assert_eq!(clamped.end_date, Some(today + Duration::days(6)));

    // Header search button must stay inside 760px window even with long Project name!
    cx.run_until_parked();
    let search_btn = cx.debug_bounds("things-header-search").unwrap();
    assert!(search_btn.right() <= px(760.));

    // Clear Deadline -> clears due_date & end_date while preserving reminders, timezone, constant_reminder, repeat.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.open_deadline_picker(PopoverAnchor::Card, DeadlineTarget::Task(task.id), cx);
            v.handle_deadline_action(DeadlineAction::Apply(None), window, cx);
        });
    });
    let cleared = store.task(task.id).unwrap().unwrap();
    assert_eq!(cleared.due_date, None);
    assert_eq!(cleared.end_date, None);
    assert_eq!(cleared.timezone.as_deref(), Some("America/New_York"));
    assert_eq!(cleared.reminders, vec![15]);
    assert!(cleared.constant_reminder);
    assert!(cleared.repeat.is_some());

    // Unicode-aware tag toggle ("работа" removes "Работа").
    cx.update(|_, cx| {
        view.update(cx, |v, cx| {
            assert!(v.toggle_task_tag(task.id, "работа", cx));
        });
    });
    assert!(store.task(task.id).unwrap().unwrap().tags.is_empty());

    // Uncommitted checklist draft text flushes to SQLite when collapsing card.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.expand_task(task.id, window, cx);
            let card = v.expanded.as_ref().unwrap();
            card.new_checklist_item
                .update(cx, |i, cx| i.set_text("Uncommitted step", cx));
            v.handle_card_action(CardAction::Collapse, window, cx);
        });
    });
    let items = store.checklist(task.id).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "Uncommitted step");

    // DropDestination::Inbox clears due_date so task genuinely lands in Inbox.
    store.set_due(task.id, Some(today + Duration::days(2)), None).unwrap();
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.handle_sidebar_action(
                SidebarAction::DropTask {
                    task_id: task.id,
                    target: DropDestination::Inbox,
                },
                window,
                cx,
            );
        });
    });
    let in_inbox = store.task(task.id).unwrap().unwrap();
    assert_eq!(in_inbox.list_id, None);
    assert_eq!(in_inbox.start_date, None);
    assert_eq!(in_inbox.due_date, None);
    assert!(store.things_tasks(&ThingsViewKind::Inbox, today).unwrap().iter().any(|t| t.id == task.id));

    // Anytime project group drop uses start_date: None (does not force Today).
    let proj_only = store
        .create_task(
            TaskDraft { title: "Anytime proj".into(), list_id: Some(proj.id), ..Default::default() },
            false,
        )
        .unwrap();
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.select_view(ThingsViewKind::Anytime, window, cx);
            let groups = v.build_groups(today, i18n::t(cx));
            let pg = groups.iter().find(|g| g.click_project == Some(proj.id)).unwrap();
            match &pg.drop {
                Some(GroupDrop::Schedule { start_date, list_id, .. }) => {
                    assert_eq!(*start_date, None);
                    assert_eq!(*list_id, Some(Some(proj.id)));
                }
                _ => panic!("expected Schedule drop on Anytime project group"),
            }
        });
    });
    assert_eq!(store.task(proj_only.id).unwrap().unwrap().start_date, None);

    // Upcoming month-group drop clamps late-current-month task start_date to >= horizon + 1 day.
    let fixed_today = NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
    let late_nov = NaiveDate::from_ymd_opt(2026, 11, 30).unwrap();
    let _late_task = store
        .create_task(
            TaskDraft { title: "Late Nov".into(), start_date: Some(late_nov), ..Default::default() },
            false,
        )
        .unwrap();
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.select_view(ThingsViewKind::Upcoming, window, cx);
            v.tasks = store.things_tasks(&ThingsViewKind::Upcoming, fixed_today).unwrap();
            let groups = v.build_upcoming_groups(fixed_today, i18n::t(cx));
            let nov_group = groups
                .iter()
                .find(|g| g.id.as_ref() == "group-up-month-2026-10")
                .expect("expected November month group");
            match &nov_group.drop {
                Some(GroupDrop::Schedule { start_date, .. }) => {
                    assert_eq!(*start_date, Some(late_nov));
                }
                _ => panic!("expected Schedule drop on Upcoming month group"),
            }
        });
    });
}

#[gpui::test]
fn before_task_complete_hook_blocks_things_completion_and_toasts(cx: &mut TestAppContext) {
    use std::sync::Arc;
    use desktop_common::plugin_bus::{PluginBus, PluginHookDecision, PluginHooksGlobal};

    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(CurrentLocale(i18n::Locale::En));
        let hooks = PluginHooksGlobal::new(
            Arc::new(|_| Ok(PluginHookDecision::Allow)),
            Arc::new(|task| {
                if task.get("title").and_then(|v| v.as_str()) == Some("Blocked task") {
                    Ok(PluginHookDecision::Cancel("Cannot finish yet".into()))
                } else {
                    Ok(PluginHookDecision::Allow)
                }
            }),
            Arc::new(|_| Ok(PluginHookDecision::Allow)),
        );
        PluginHooksGlobal::set_global(hooks, cx);
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();
    let blocked = store
        .create_task(
            TaskDraft {
                title: "Blocked task".into(),
                start_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let allowed = store
        .create_task(
            TaskDraft {
                title: "Allowed task".into(),
                start_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .unwrap();

    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.toggle_task_complete(blocked.id, window, cx);
            v.toggle_task_complete(allowed.id, window, cx);
        });
    });
    cx.run_until_parked();

    assert!(
        !store.task(blocked.id).unwrap().unwrap().is_completed(),
        "beforeTaskComplete Cancel must prevent completion in ThingsView"
    );
    assert!(
        store.task(allowed.id).unwrap().unwrap().is_completed(),
        "beforeTaskComplete Allow must complete task in ThingsView"
    );
    cx.update(|_, cx| {
        let bus = PluginBus::global(cx).expect("PluginBus must record cancel toast");
        assert!(
            bus.toasts.iter().any(|t| t.message == "Cannot finish yet"),
            "expected warning toast with hook cancel reason"
        );
        PluginHooksGlobal::clear_global(cx);
    });
}

static HORIZONS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct RestoreHorizonsEnabled;

impl Drop for RestoreHorizonsEnabled {
    fn drop(&mut self) {
        calendar::set_horizons_enabled(true);
    }
}

#[gpui::test]
fn horizon_tasks_appear_in_anytime_and_someday_with_badges(cx: &mut TestAppContext) {
    let _lock = HORIZONS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _restore = RestoreHorizonsEnabled;
    calendar::set_horizons_enabled(true);

    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();

    let t_week = store
        .create_task(
            TaskDraft {
                title: "Week milestone".into(),
                horizon: Some(Horizon::current_week(today)),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let t_month = store
        .create_task(
            TaskDraft {
                title: "Month milestone".into(),
                horizon: Some(Horizon::current_month(today)),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let t_quarter = store
        .create_task(
            TaskDraft {
                title: "Quarter milestone".into(),
                horizon: Some(Horizon::current_quarter(today)),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let t_year = store
        .create_task(
            TaskDraft {
                title: "Year milestone".into(),
                horizon: Some(Horizon::current_year(today)),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let t_decade = store
        .create_task(
            TaskDraft {
                title: "Decade milestone".into(),
                horizon: Some(Horizon::current_decade(today)),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let t_life = store
        .create_task(
            TaskDraft {
                title: "Life milestone".into(),
                horizon: Some(Horizon::Life),
                ..Default::default()
            },
            false,
        )
        .unwrap();

    setup_env(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(960.), px(640.)));
    cx.run_until_parked();

    // Anytime shows Week/Month/Quarter horizon tasks with their horizon badges.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.select_view(ThingsViewKind::Anytime, window, cx));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let ids: Vec<Id> = view.read(cx).tasks().iter().map(|t| t.id).collect();
        assert!(ids.contains(&t_week.id));
        assert!(ids.contains(&t_month.id));
        assert!(ids.contains(&t_quarter.id));
        assert!(!ids.contains(&t_year.id));
        assert!(!ids.contains(&t_decade.id));
        assert!(!ids.contains(&t_life.id));
    });
    for id in [t_week.id, t_month.id, t_quarter.id] {
        let sel: &'static str = format!("things-horizon-chip-{id}").leak();
        assert!(cx.debug_bounds(sel).is_some(), "expected horizon badge for {id} in Anytime");
    }

    // Someday shows Year/Decade/Life horizon tasks with their horizon badges.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.select_view(ThingsViewKind::Someday, window, cx));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let ids: Vec<Id> = view.read(cx).tasks().iter().map(|t| t.id).collect();
        assert!(ids.contains(&t_year.id));
        assert!(ids.contains(&t_decade.id));
        assert!(ids.contains(&t_life.id));
        assert!(!ids.contains(&t_week.id));
        assert!(!ids.contains(&t_month.id));
        assert!(!ids.contains(&t_quarter.id));
    });
    for id in [t_year.id, t_decade.id, t_life.id] {
        let sel: &'static str = format!("things-horizon-chip-{id}").leak();
        assert!(cx.debug_bounds(sel).is_some(), "expected horizon badge for {id} in Someday");
    }

    // Disabling Horizons hides the horizon badge pills cleanly.
    calendar::set_horizons_enabled(false);
    cx.update(|_, cx| view.update(cx, |v, cx| v.reload(cx)));
    cx.run_until_parked();
    for id in [t_year.id, t_decade.id, t_life.id] {
        let sel: &'static str = format!("things-horizon-chip-{id}").leak();
        assert!(cx.debug_bounds(sel).is_none(), "horizon badge must hide when disabled");
    }
    calendar::set_horizons_enabled(true);
}

#[gpui::test]
fn when_picker_horizon_chips_gate_on_feature_flag_and_persist_horizon(cx: &mut TestAppContext) {
    let _lock = HORIZONS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _restore = RestoreHorizonsEnabled;
    calendar::set_horizons_enabled(true);

    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();
    let task = store
        .create_task(
            TaskDraft {
                title: "Plan horizon".into(),
                start_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .unwrap();

    setup_env(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(960.), px(720.)));
    cx.run_until_parked();

    // Open WhenPicker on the expanded task -> all 4 Horizon chips render when enabled.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.expand_task(task.id, window, cx);
            v.handle_card_action(CardAction::TogglePopover(CardPopover::When), window, cx);
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("things-when-horizons").is_some());
    assert!(cx.debug_bounds("when-horizon-week").is_some());
    assert!(cx.debug_bounds("when-horizon-month").is_some());
    assert!(cx.debug_bounds("when-horizon-year").is_some());
    assert!(cx.debug_bounds("when-horizon-life").is_some());

    // Clicking the Week chip persists Horizon::current_week(today) and clears start_date.
    let week_pos = cx.debug_bounds("when-horizon-week").unwrap().center();
    cx.simulate_click(week_pos, Modifiers::default());
    cx.run_until_parked();
    let after_week = store.task(task.id).unwrap().unwrap();
    assert_eq!(after_week.horizon, Some(Horizon::current_week(today)));
    assert_eq!(after_week.start_date, None);
    assert_eq!(after_week.due_date, None);

    // open_task navigates to Anytime and re-expands; clicking Life persists Horizon::Life.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.open_task(task.id, window, cx);
            v.handle_card_action(CardAction::TogglePopover(CardPopover::When), window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(view.read(cx).view(), ThingsViewKind::Anytime));
    let life_pos = cx.debug_bounds("when-horizon-life").unwrap().center();
    cx.simulate_click(life_pos, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(store.task(task.id).unwrap().unwrap().horizon, Some(Horizon::Life));

    // Disabling Horizons hides the Horizon quick-pick row completely in WhenPicker.
    calendar::set_horizons_enabled(false);
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.open_task(task.id, window, cx);
            v.handle_card_action(CardAction::TogglePopover(CardPopover::When), window, cx);
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("things-when-popover").is_some());
    assert!(cx.debug_bounds("things-when-horizons").is_none());
    assert!(cx.debug_bounds("when-horizon-week").is_none());

    calendar::set_horizons_enabled(true);
}

#[gpui::test]
fn keyboard_jk_scrolls_past_fold_and_delete_works_only_outside_editing(cx: &mut TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();
    let mut created_ids = Vec::new();
    for i in 0..24 {
        let task = store
            .create_task(
                TaskDraft {
                    title: format!("Task {i}"),
                    start_date: Some(today),
                    ..Default::default()
                },
                false,
            )
            .unwrap();
        created_ids.push(task.id);
    }

    setup_env_with_keymap(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(960.), px(360.)));
    cx.run_until_parked();

    cx.update(|window, cx| {
        let fh = view.read(cx).focus_handle(cx);
        window.focus(&fh, cx);
    });

    // Navigate down past the fold with `j`.
    for _ in 0..15 {
        cx.simulate_keystrokes("j");
    }
    cx.run_until_parked();

    let (sel_id, scroll_y) = cx.update(|_, cx| {
        let v = view.read(cx);
        (v.selected_task(), v.list_scroll.offset().y)
    });
    assert_eq!(sel_id, Some(created_ids[14]));
    assert!(scroll_y < px(0.), "list_scroll must scroll down when navigating past fold, got {scroll_y:?}");

    let row_sel: &'static str = format!("things-row-{}", created_ids[14]).leak();
    let row_bounds = cx.debug_bounds(row_sel).unwrap();
    let list_bounds = cx.debug_bounds("things-main-list").unwrap();
    assert!(row_bounds.bottom() <= list_bounds.bottom() + px(1.));
    assert!(row_bounds.top() >= list_bounds.top() - px(1.));

    // Navigate back up with `k` to the top.
    for _ in 0..14 {
        cx.simulate_keystrokes("k");
    }
    cx.run_until_parked();
    let (top_sel, top_scroll_y) = cx.update(|_, cx| {
        let v = view.read(cx);
        (v.selected_task(), v.list_scroll.offset().y)
    });
    assert_eq!(top_sel, Some(created_ids[0]));
    assert_eq!(top_scroll_y, px(0.));

    // Expanding task and typing `j`/`k`/backspace inside title edits text without navigating or deleting task.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("j k backspace");
    cx.run_until_parked();
    assert!(!store.task(created_ids[0]).unwrap().unwrap().is_deleted());
    assert_eq!(store.task(created_ids[0]).unwrap().unwrap().title, "Task 0j");

    // Even if focus returns to root while card is still expanded (editing), Delete/Backspace must not delete.
    cx.update(|window, cx| {
        let fh = view.read(cx).focus_handle(cx);
        window.focus(&fh, cx);
    });
    cx.simulate_keystrokes("delete");
    cx.run_until_parked();
    assert!(!store.task(created_ids[0]).unwrap().unwrap().is_deleted());

    // Collapse card, then press Backspace -> soft-deletes selected task.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let fh = view.read(cx).focus_handle(cx);
        window.focus(&fh, cx);
    });
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    assert!(store.task(created_ids[0]).unwrap().unwrap().is_deleted());
}

#[gpui::test]
fn title_notes_invalidate_groups_checklist_enter_focuses_next_and_invalid_when_shows_error(
    cx: &mut TestAppContext,
) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    let today = ThingsView::today();
    let task = store
        .create_task(
            TaskDraft {
                title: "Initial".into(),
                start_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let item1 = store.add_checklist_item(task.id, "First step", None).unwrap();

    setup_env_with_keymap(cx);
    let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(960.), px(720.)));
    cx.run_until_parked();

    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.expand_task(task.id, window, cx));
    });
    cx.run_until_parked();

    // Mutate title and notes -> cached_groups invalidates immediately.
    cx.simulate_input(" edited");
    cx.update(|window, cx| {
        let notes = view.read(cx).expanded.as_ref().unwrap().notes.clone();
        window.focus(&notes.focus_handle(cx), cx);
    });
    cx.simulate_input("Body note");
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        let groups = v.groups_rc(today, i18n::t(cx));
        let cached_task = groups
            .iter()
            .flat_map(|g| g.tasks.iter())
            .find(|t| t.id == task.id)
            .unwrap();
        assert_eq!(cached_task.title, "Initial edited");
        assert_eq!(cached_task.notes, "Body note");
    });

    // Focus existing checklist item and press Enter -> inserts second item after it and focuses it.
    cx.update(|window, cx| {
        let first_input = view.read(cx).expanded.as_ref().unwrap().checklist_rows[0]
            .input
            .clone();
        window.focus(&first_input.focus_handle(cx), cx);
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    let items = store.checklist(task.id).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, item1.id);
    cx.update(|window, cx| {
        let card = view.read(cx).expanded.as_ref().unwrap();
        assert_eq!(card.checklist_rows.len(), 2);
        assert!(card.checklist_rows[1].input.focus_handle(cx).is_focused(window));
    });

    // Toggling checklist item complete marks row.completed = true.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.handle_card_action(CardAction::ToggleChecklistItem(item1.id, true), window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let card = view.read(cx).expanded.as_ref().unwrap();
        assert!(card.checklist_rows[0].completed);
    });

    // Invalid When date shows parse-error banner and preserves existing start_date until edited/valid.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.handle_card_action(CardAction::TogglePopover(CardPopover::When), window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let date_input = view.read(cx).when_picker.as_ref().unwrap().1.date_input.clone();
        date_input.update(cx, |i, cx| i.set_text("", cx));
    });
    cx.simulate_input("not-a-date");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, cx| {
        let v = view.read(cx);
        assert!(v.when_date_error);
        assert!(v.when_picker.is_some());
    });
    assert!(cx.debug_bounds("things-when-date-error").is_some());
    assert_eq!(store.task(task.id).unwrap().unwrap().start_date, Some(today));

    // Typing valid date clears error and schedules on Enter.
    cx.update(|_, cx| {
        let date_input = view.read(cx).when_picker.as_ref().unwrap().1.date_input.clone();
        date_input.update(cx, |i, cx| i.set_text("", cx));
    });
    let target = today + Duration::days(2);
    cx.simulate_input(&target.format("%Y-%m-%d").to_string());
    cx.run_until_parked();
    cx.update(|_, cx| assert!(!view.read(cx).when_date_error));
    assert!(cx.debug_bounds("things-when-date-error").is_none());
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(store.task(task.id).unwrap().unwrap().start_date, Some(target));
}
