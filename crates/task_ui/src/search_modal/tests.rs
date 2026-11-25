use super::*;

#[gpui::test]
fn keyboard_navigation_selects_and_opens_matching_results(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let first = store.add_task("Alpha review").unwrap();
    let second = store.add_task("Alpha deploy").unwrap();
    let opened: Rc<std::cell::RefCell<Option<Id>>> = Rc::new(std::cell::RefCell::new(None));
    let opened_slot = opened.clone();

    let (modal, cx) = cx.add_window_view(|_window, cx| {
        let entity = cx.entity();
        cx.subscribe(&entity, move |_, _, event: &SearchEvent, _| {
            if let SearchEvent::OpenTask(id) = event {
                *opened_slot.borrow_mut() = Some(*id);
            }
        })
        .detach();
        SearchModal::new(store, cx)
    });
    cx.update(|_, cx| {
        modal.update(cx, |modal, cx| {
            modal.set_query("Alpha", cx);
            assert_eq!(modal.selected, 0);
            modal.move_selection(1, cx);
            assert_eq!(modal.selected, 1);
            modal.move_selection(1, cx);
            assert_eq!(modal.selected, 0, "selection wraps at end");
            modal.move_selection(-1, cx);
            assert_eq!(modal.selected, 1, "selection wraps from start");
            modal.open_selected(cx);
        });
    });
    cx.run_until_parked();
    let expected = cx.update(|_, cx| modal.read(cx).tasks[1].id);
    assert!(expected == first.id || expected == second.id);
    assert_eq!(*opened.borrow(), Some(expected));
}

#[gpui::test]
fn completing_repeating_task_from_search_advances_recurrence(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    let task = store.add_task("Weekly sync").unwrap();
    store.set_due(task.id, Some(day), None).unwrap();
    store
        .set_repeat(task.id, Some(fern_core::Recurrence::new(fern_core::Frequency::Weekly, 1)), day)
        .unwrap();
    let before = store.task(task.id).unwrap().unwrap();

    let (modal, cx) = cx.add_window_view(|_window, cx| SearchModal::new(store.clone(), cx));
    cx.update(|_, cx| {
        modal.update(cx, |modal, cx| {
            modal.toggle_task_completion(&before, cx);
        });
    });
    let updated = store.task(task.id).unwrap().unwrap();
    assert!(!updated.is_completed(), "repeating task must stay open for its next occurrence");
    assert_eq!(updated.due_date, Some(day + chrono::Duration::days(7)));
    let completed_copies = store.tasks().unwrap().into_iter().filter(|t| t.is_completed()).count();
    assert_eq!(completed_copies, 1);
}

#[gpui::test]
fn plugin_commands_and_search_hits_emit_navigation_and_command_events(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Unrelated task").unwrap();
    cx.update(|cx| {
        let mut bus = PluginBus::default();
        bus.commands.push(PluginCommandSpec {
            plugin_id: "pomo".into(),
            command_id: "start".into(),
            title: "Start Focus Session".into(),
            plugin_name: "Pomo".into(),
        });
        cx.set_global(bus);
    });

    let events: Rc<std::cell::RefCell<Vec<SearchEvent>>> =
        Rc::new(std::cell::RefCell::new(Vec::new()));
    let events_slot = events.clone();
    let (modal, cx) = cx.add_window_view(|_window, cx| {
        let entity = cx.entity();
        cx.subscribe(&entity, move |_, _, event: &SearchEvent, _| {
            events_slot.borrow_mut().push(event.clone());
        })
        .detach();
        SearchModal::new(store, cx)
    });

    cx.update(|_, cx| {
        modal.update(cx, |modal, cx| {
            modal.set_query("Focus", cx);
        });
        PluginBus::update(cx, |bus| {
            bus.search_hits.push(PluginSearchHit {
                id: "hit-1".into(),
                plugin_id: "notes".into(),
                provider_id: "external".into(),
                title: "External Focus Note".into(),
                subtitle: Some("Notes".into()),
                badge: Some("Sync".into()),
                task_id: Some(task.id),
                command_id: None,
                command_arg: None,
            });
        });
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        modal.update(cx, |modal, cx| {
            assert_eq!(modal.visible_events().len(), 2);
            modal.open_selected(cx);
            // Re-deliver hits for second selection check (open_selected clears hits on close).
            modal.plugin_hits.push(PluginSearchHit {
                id: "hit-1".into(),
                plugin_id: "notes".into(),
                provider_id: "external".into(),
                title: "External Focus Note".into(),
                subtitle: Some("Notes".into()),
                badge: Some("Sync".into()),
                task_id: Some(task.id),
                command_id: None,
                command_arg: None,
            });
            modal.move_selection(1, cx);
            modal.open_selected(cx);
        });
    });
    cx.run_until_parked();

    let emitted = events.borrow();
    assert!(matches!(
        &emitted[0],
        SearchEvent::RunPluginCommand {
            plugin_id,
            command_id,
            arg: None
        } if plugin_id == "pomo" && command_id == "start"
    ));
    assert!(matches!(&emitted[1], SearchEvent::OpenTask(id) if *id == task.id));
}

#[gpui::test]
fn stale_plugin_search_hits_cleared_on_new_query_empty_query_and_close(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Alpha task").unwrap();
    let make_hit = |title: &str| PluginSearchHit {
        id: format!("hit-{title}"),
        plugin_id: "notes".into(),
        provider_id: "external".into(),
        title: title.into(),
        subtitle: None,
        badge: None,
        task_id: Some(task.id),
        command_id: None,
        command_arg: None,
    };

    // Pre-populate stale hits before opening SearchModal -> opening must clear them.
    cx.update(|cx| {
        let mut bus = PluginBus::default();
        bus.search_hits.push(make_hit("Stale alpha"));
        cx.set_global(bus);
    });
    let (modal, cx) = cx.add_window_view(|_window, cx| SearchModal::new(store, cx));
    cx.update(|_, cx| {
        assert!(cx.global::<PluginBus>().search_hits.is_empty());
    });

    // Query "alpha", receive plugin hit, then switch to "beta" -> stale alpha hits clear immediately.
    cx.update(|_, cx| {
        modal.update(cx, |m, cx| m.set_query("alpha", cx));
        PluginBus::update(cx, |bus| bus.search_hits.push(make_hit("Alpha hit")));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(modal.read(cx).plugin_hits.len(), 1);
        modal.update(cx, |m, cx| m.set_query("beta", cx));
        assert!(modal.read(cx).plugin_hits.is_empty());
        assert!(cx.global::<PluginBus>().search_hits.is_empty());
    });

    // Clearing to empty query clears PluginBus::search_hits too.
    cx.update(|_, cx| {
        PluginBus::update(cx, |bus| bus.search_hits.push(make_hit("Beta hit")));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(modal.read(cx).plugin_hits.len(), 1);
        modal.update(cx, |m, cx| m.set_query("", cx));
        assert!(modal.read(cx).plugin_hits.is_empty());
        assert!(cx.global::<PluginBus>().search_hits.is_empty());
    });

    // Dismissing SearchModal clears PluginBus::search_hits.
    cx.update(|_, cx| {
        modal.update(cx, |m, cx| m.set_query("gamma", cx));
        PluginBus::update(cx, |bus| bus.search_hits.push(make_hit("Gamma hit")));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(cx.global::<PluginBus>().search_hits.len(), 1);
        modal.update(cx, |m, cx| m.emit_close(SearchEvent::Dismissed, cx));
        assert!(cx.global::<PluginBus>().search_hits.is_empty());
    });
}

#[gpui::test]
fn cached_search_updates_on_data_version_filters_and_scrolls(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    store
        .add_list(&fern_core::ListDraft {
            name: "Alpha List".into(),
            ..Default::default()
        })
        .unwrap();
    store.add_tag("alpha-tag", None, None).unwrap();
    store.add_habit("Alpha Habit").unwrap();
    let high = store.add_task("Alpha priority").unwrap();
    store.set_priority(high.id, Priority::High).unwrap();
    for i in 1..=5 {
        store.add_task(&format!("Alpha extra {i}")).unwrap();
    }

    let (modal, cx) = cx.add_window_view(|_window, cx| SearchModal::new(store.clone(), cx));
    cx.simulate_resize(gpui::size(px(900.), px(640.)));
    cx.run_until_parked();

    cx.update(|_, cx| {
        modal.update(cx, |m, cx| {
            m.set_query("Alpha", cx);
            assert_eq!(m.tasks.len(), 6);
            assert_eq!(m.habits.len(), 1);
            assert_eq!(m.tags.len(), 1);
            assert_eq!(m.lists.len(), 1);

            m.set_filter(Filter::Tasks, cx);
            m.draft.priorities = vec![Priority::High];
            m.apply(cx);
            assert_eq!(m.tasks.len(), 1);
            assert_eq!(m.tasks[0].id, high.id);

            m.set_filter(Filter::All, cx);
            assert_eq!(m.tasks.len(), 6);
        });
    });

    let live_task = store.add_task("Alpha live task").unwrap();
    store
        .add_list(&fern_core::ListDraft {
            name: "Alpha Live List".into(),
            ..Default::default()
        })
        .unwrap();
    store.add_tag("alpha-live-tag", None, None).unwrap();
    for i in 1..=8 {
        store.add_habit(&format!("Alpha Habit {i}")).unwrap();
    }
    let other = cx.update(|_, cx| cx.new(|_| ()));
    cx.update(|_, cx| desktop_common::data_version::data_changed(other.entity_id(), cx));
    cx.run_until_parked();

    cx.update(|_, cx| {
        let m = modal.read(cx);
        assert_eq!(m.tasks.len(), 7);
        assert_eq!(m.habits.len(), 9);
        assert_eq!(m.tags.len(), 2);
        assert_eq!(m.lists.len(), 2);
        let top = m.results_list_state.logical_scroll_top();
        assert_eq!(top.item_ix, 0);
        assert_eq!(top.offset_in_item, px(0.));
    });

    cx.update(|_, cx| {
        modal.update(cx, |m, cx| {
            for _ in 0..14 {
                m.move_selection(1, cx);
            }
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let top = modal.read(cx).results_list_state.logical_scroll_top();
        assert!(
            top.item_ix > 0 || top.offset_in_item > px(0.),
            "moving selection past visible fold must scroll results list: {top:?}"
        );
    });

    cx.update(|_, cx| {
        modal.update(cx, |m, cx| {
            let remaining = m.visible_events().len() - m.selected;
            for _ in 0..remaining {
                m.move_selection(1, cx);
            }
            assert_eq!(m.selected, 0);
            m.toggle_task_completion(&live_task, cx);
            assert!(
                m.tasks.iter().any(|t| t.id == live_task.id && t.is_completed()),
                "inline completion must refresh cached search results immediately"
            );
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let top = modal.read(cx).results_list_state.logical_scroll_top();
        assert_eq!(top.item_ix, 0);
        assert_eq!(top.offset_in_item, px(0.));
    });
}

#[test]
fn strip_markdown_inline_removes_markers_without_breaking_plain_text() {
    assert_eq!(
        SearchModal::strip_markdown_inline("Check **Markdown**, __under__, `code`, ~~old~~ & foo_bar * 2"),
        "Check Markdown, under, code, old & foo_bar * 2"
    );
}

#[gpui::test]
fn unified_menu_and_create_task_buttons_stay_bounded_and_dispatch_actions(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let created: Rc<std::cell::RefCell<Option<String>>> =
        Rc::new(std::cell::RefCell::new(None));
    let created_slot = created.clone();

    let (modal, cx) = cx.add_window_view(|_window, cx| {
        let entity = cx.entity();
        cx.subscribe(&entity, move |_, _, event: &SearchEvent, _| {
            if let SearchEvent::CreateTask(q) = event {
                *created_slot.borrow_mut() = Some(q.clone());
            }
        })
        .detach();
        SearchModal::new(store, cx)
    });
    cx.simulate_resize(gpui::size(px(900.), px(640.)));
    cx.run_until_parked();

    // Filter dropdown Cancel/OK buttons share width equally and apply/close cleanly.
    cx.update(|_, cx| {
        modal.update(cx, |m, cx| {
            m.set_filter(Filter::Tasks, cx);
            m.open_menu(Menu::Priority, cx);
            m.draft.priorities = vec![Priority::High];
        });
    });
    cx.run_until_parked();
    let cancel_bounds = cx.debug_bounds("search-menu-cancel").unwrap();
    let ok_bounds = cx.debug_bounds("search-menu-ok").unwrap();
    assert!((cancel_bounds.size.width - ok_bounds.size.width).abs() <= px(1.));
    cx.simulate_click(cancel_bounds.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let m = modal.read(cx);
        assert!(m.menu.is_none());
        assert!(m.task_filter.priorities.is_empty());
    });

    cx.update(|_, cx| {
        modal.update(cx, |m, cx| {
            m.open_menu(Menu::Priority, cx);
            m.draft.priorities = vec![Priority::High];
        });
    });
    cx.run_until_parked();
    let ok_bounds = cx.debug_bounds("search-menu-ok").unwrap();
    cx.simulate_click(ok_bounds.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let m = modal.read(cx);
        assert!(m.menu.is_none());
        assert_eq!(m.task_filter.priorities, vec![Priority::High]);
    });

    // Long query CreateTask button truncates label inside modal bounds while emitting full query.
    let long_query = "Very long unmatched query ".repeat(4).trim().to_owned();
    let (short_label, truncated) = SearchModal::create_task_button_label("Add task", &long_query);
    assert!(truncated);
    assert!(short_label.ends_with("…\""));
    cx.update(|_, cx| {
        modal.update(cx, |m, cx| m.set_query(&long_query, cx));
    });
    cx.run_until_parked();
    let create_bounds = cx.debug_bounds("search-create-task").unwrap();
    assert!(create_bounds.size.width < px(536.));
    cx.simulate_click(create_bounds.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(created.borrow().as_deref(), Some(long_query.as_str()));
}
