use super::*;
use fern_core::{ListDraft, TaskDraft};
use gpui::{Modifiers, TestAppContext, size};

fn setup_app(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        assert!(
            desktop_common::keymap::load_default(&assets::Assets, cx)
                .expect("load default keymap")
                .is_empty()
        );
    });
}

fn bounds_of(
    cx: &mut gpui::VisualTestContext,
    selector: impl Into<String>,
) -> Option<gpui::Bounds<Pixels>> {
    let key: &'static str = Box::leak(selector.into().into_boxed_str());
    cx.debug_bounds(key)
}

#[gpui::test]
fn horizons_fits_six_columns_edge_to_edge_at_1000px_and_persists_collapsed_strips(
    cx: &mut TestAppContext,
) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let today = HorizonsView::today();
    let week_code = Horizon::current_week(today).as_code();
    let month_code = Horizon::current_month(today).as_code();
    let quarter_code = Horizon::current_quarter(today).as_code();
    let year_code = Horizon::current_year(today).as_code();
    let decade_code = Horizon::current_decade(today).as_code();
    let today_key = format!("day-{}", today.format("%Y-%m-%d"));

    let (_view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
    // Simulates 1920px @ 150% DPI inside TodoView (1280 logical - 50 rail - 230 sidebar = 1000px).
    cx.simulate_resize(size(px(1000.), px(720.)));
    cx.run_until_parked();

    // No duplicate inner sidebar exists; top strip bar is rendered cleanly.
    assert!(cx.debug_bounds("horizons-top-strip").is_some());

    let cols = [
        "horizons-col-inbox".to_owned(),
        format!("horizons-col-{today_key}"),
        format!("horizons-col-horizon-{week_code}"),
        format!("horizons-col-horizon-{month_code}"),
        format!("horizons-col-horizon-{year_code}"),
        "horizons-col-horizon-LIFE".to_owned(),
    ];
    for col in &cols {
        let bounds = bounds_of(cx, col.clone()).unwrap_or_else(|| panic!("missing column {col}"));
        assert!(
            bounds.size.width >= px(PANORAMIC_MIN_WIDTH),
            "column {col} width {:?} < {PANORAMIC_MIN_WIDTH}px",
            bounds.size.width
        );
    }
    let life_bounds = bounds_of(cx, "horizons-col-horizon-LIFE").expect("life bounds");
    assert!(
        life_bounds.right() <= px(1001.),
        "all 6 open columns + 2x28px strips must fit edge-to-edge inside 1000px (life right = {:?})",
        life_bounds.right()
    );

    let q_strip = cx.debug_bounds("horizons-strip-quarter").expect("quarter strip");
    let d_strip = cx.debug_bounds("horizons-strip-decade").expect("decade strip");
    assert!((q_strip.size.width - px(28.)).abs() < px(1.));
    assert!((d_strip.size.width - px(28.)).abs() < px(1.));
    assert!(bounds_of(cx, format!("horizons-col-horizon-{quarter_code}")).is_none());
    assert!(bounds_of(cx, format!("horizons-col-horizon-{decade_code}")).is_none());

    // Expand Quarter and Decade -> persists "" so a fresh view keeps all 8 open.
    cx.simulate_click(q_strip.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(bounds_of(cx, format!("horizons-col-horizon-{quarter_code}")).is_some());
    assert_eq!(store.setting(SETTING_COLLAPSED_COLS).unwrap().as_deref(), Some("decade"));

    cx.simulate_resize(size(px(1400.), px(720.)));
    cx.run_until_parked();
    let d_center = cx.debug_bounds("horizons-strip-decade").expect("d strip").center();
    cx.simulate_click(d_center, Modifiers::default());
    cx.run_until_parked();
    assert!(bounds_of(cx, format!("horizons-col-horizon-{decade_code}")).is_some());
    assert_eq!(store.setting(SETTING_COLLAPSED_COLS).unwrap().as_deref(), Some(""));

    let persisted_open = cx.update(|window, cx| {
        let fresh = cx.new(|cx| HorizonsView::new(store.clone(), window, cx));
        fresh.read(cx).collapsed_cols.is_empty()
    });
    assert!(persisted_open, "empty collapsed setting must keep all 8 columns expanded");
}

#[gpui::test]
fn inline_add_creates_tasks_in_each_column_with_mutual_exclusion(cx: &mut TestAppContext) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let today = HorizonsView::today();
    let week = Horizon::current_week(today);
    let month = Horizon::current_month(today);
    let year = Horizon::current_year(today);

    let (view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(1200.), px(800.)));
    cx.run_until_parked();

    let cases = [
        ("inbox".to_owned(), "Inbox capture"),
        (format!("day-{}", today.format("%Y-%m-%d")), "Today standup"),
        (format!("horizon-{}", week.as_code()), "Weekly release"),
        (format!("horizon-{}", month.as_code()), "Monthly reading"),
        (format!("horizon-{}", year.as_code()), "Yearly marathon"),
        ("horizon-LIFE".to_owned(), "Life dream"),
    ];

    for (col_key, title) in cases {
        let btn = bounds_of(cx, format!("inline-add-{col_key}"))
            .unwrap_or_else(|| panic!("missing inline-add-{col_key}"));
        cx.simulate_click(btn.center(), Modifiers::default());
        cx.run_until_parked();
        cx.simulate_input(title);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
    }

    assert!(view.read_with(cx, |v, _| v.inline_add.is_some()));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.inline_add.is_none()));

    let all = store.tasks().expect("tasks");
    let by_title = |name: &str| all.iter().find(|t| t.title == name).expect(name).clone();

    assert!(is_horizons_inbox(&by_title("Inbox capture")));
    assert_eq!(by_title("Today standup").due_date, Some(today));
    assert_eq!(by_title("Today standup").horizon, None);
    assert_eq!(by_title("Weekly release").horizon, Some(week));
    assert_eq!(by_title("Weekly release").due_date, None);
    assert_eq!(by_title("Monthly reading").horizon, Some(month));
    assert_eq!(by_title("Yearly marathon").horizon, Some(year));
    assert_eq!(by_title("Life dream").horizon, Some(Horizon::Life));
}

#[gpui::test]
fn rollover_strip_moves_past_month_and_overdue_tasks_and_undoes(cx: &mut TestAppContext) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let today = HorizonsView::today();
    let cur_month = Horizon::current_month(today);
    let prev_month = cur_month.shift(-1);

    let past_month_task = store
        .create_task(
            TaskDraft {
                title: "Carryover goal".into(),
                horizon: Some(prev_month),
                ..Default::default()
            },
            false,
        )
        .expect("past month task");
    let overdue_task = store
        .create_task(
            TaskDraft {
                title: "Overdue invoice".into(),
                due_date: Some(today - Duration::days(2)),
                ..Default::default()
            },
            false,
        )
        .expect("overdue task");

    let (view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(1000.), px(720.)));
    cx.run_until_parked();

    let month_btn_id = format!("rollover-move-all-horizon-{}", cur_month.as_code());
    let today_btn_id = format!("rollover-move-all-day-{}", today.format("%Y-%m-%d"));
    let month_btn = bounds_of(cx, month_btn_id.clone()).expect("month rollover move-all button");
    assert!(bounds_of(cx, today_btn_id.clone()).is_some());

    cx.simulate_click(month_btn.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        store.task(past_month_task.id).unwrap().unwrap().horizon,
        Some(cur_month)
    );
    assert!(bounds_of(cx, month_btn_id).is_none());

    cx.update(|_, cx| view.update(cx, |v, cx| { v.perform_undo(cx); }));
    cx.run_until_parked();
    assert_eq!(
        store.task(past_month_task.id).unwrap().unwrap().horizon,
        Some(prev_month)
    );
    cx.update(|_, cx| view.update(cx, |v, cx| { v.perform_redo(cx); }));
    cx.run_until_parked();
    assert_eq!(
        store.task(past_month_task.id).unwrap().unwrap().horizon,
        Some(cur_month)
    );

    let today_btn = bounds_of(cx, today_btn_id).expect("today move-all button");
    cx.simulate_click(today_btn.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        store.task(overdue_task.id).unwrap().unwrap().due_date,
        Some(today)
    );
}

#[gpui::test]
fn top_strip_scale_tabs_pagers_and_header_zoom(cx: &mut TestAppContext) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let today = HorizonsView::today();
    let inbox_item = store.add_task("Triage me").expect("inbox task");

    let (view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(1200.), px(760.)));
    cx.run_until_parked();

    // Click Week header in panoramic view -> zooms into Weeks scale.
    let week_title = bounds_of(
        cx,
        format!("horizons-col-title-horizon-{}", Horizon::current_week(today).as_code()),
    )
    .expect("week title");
    cx.simulate_click(week_title.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.mode), HorizonsMode::Weeks);

    // Switch through all 6 scale tabs via top strip bar and verify period stepper.
    let scale_checks = [
        ("horizons-tab-days", HorizonsMode::Days, format!("horizons-col-day-{}", (today + Duration::days(1)).format("%Y-%m-%d"))),
        ("horizons-tab-weeks", HorizonsMode::Weeks, format!("horizons-col-horizon-{}", Horizon::current_week(today).shift(1).as_code())),
        ("horizons-tab-months", HorizonsMode::Months, format!("horizons-col-horizon-{}", Horizon::current_month(today).shift(1).as_code())),
        ("horizons-tab-quarters", HorizonsMode::Quarters, format!("horizons-col-horizon-{}", Horizon::current_quarter(today).shift(1).as_code())),
        ("horizons-tab-years", HorizonsMode::Years, format!("horizons-col-horizon-{}", Horizon::current_year(today).shift(1).as_code())),
        ("horizons-tab-decades", HorizonsMode::Decades, format!("horizons-col-horizon-{}", Horizon::current_decade(today).shift(1).as_code())),
    ];
    for (tab_id, expected_mode, next_col_id) in scale_checks {
        let tab = cx.debug_bounds(tab_id).unwrap_or_else(|| panic!("missing {tab_id}"));
        cx.simulate_click(tab.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.mode), expected_mode);
        assert!(bounds_of(cx, next_col_id.clone()).is_some(), "missing {next_col_id}");

        let next_btn = cx.debug_bounds("horizons-pager-next").expect("next pager");
        cx.simulate_click(next_btn.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.period_offset), 1);

        let cur_btn = cx.debug_bounds("horizons-pager-today").expect("current pager");
        cx.simulate_click(cur_btn.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.period_offset), 0);
    }

    // Return to Horizons tab, click Inbox column header -> Inbox triage mode.
    let h_tab = cx.debug_bounds("horizons-tab-horizons").expect("horizons tab");
    cx.simulate_click(h_tab.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.mode), HorizonsMode::Horizons);

    let inbox_title = cx.debug_bounds("horizons-col-title-inbox").expect("inbox title");
    cx.simulate_click(inbox_title.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.mode), HorizonsMode::Inbox);

    let triage_week = bounds_of(cx, format!("triage-{}-week", inbox_item.id))
        .expect("triage week chip");
    cx.simulate_click(triage_week.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        store.task(inbox_item.id).unwrap().unwrap().horizon,
        Some(Horizon::current_week(today))
    );
}

#[gpui::test]
fn drawer_quick_horizon_reassign_escape_cascade_and_filters(cx: &mut TestAppContext) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let today = HorizonsView::today();
    let week = Horizon::current_week(today);

    let work_list = store
        .add_list(&ListDraft {
            name: "Work".into(),
            ..Default::default()
        })
        .expect("work list");
    let task = store
        .create_task(
            TaskDraft {
                title: "Plan architecture".into(),
                list_id: Some(work_list.id),
                horizon: Some(week),
                tags: vec!["urgent".into()],
                priority: Priority::High,
                ..Default::default()
            },
            false,
        )
        .expect("task");
    let other = store
        .create_task(
            TaskDraft {
                title: "Water plants".into(),
                horizon: Some(week),
                tags: vec!["home".into()],
                ..Default::default()
            },
            false,
        )
        .expect("other task");
    let done_task = store
        .create_task(
            TaskDraft {
                title: "Done goal".into(),
                horizon: Some(week),
                ..Default::default()
            },
            false,
        )
        .expect("done task");
    store.set_completed(done_task.id, true).expect("complete");

    let (view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(1200.), px(800.)));
    cx.run_until_parked();

    // Filter popover opens from top strip bar and filters by tag & show-completed.
    let filter_btn = cx.debug_bounds("horizons-btn-filter").expect("filter btn");
    cx.simulate_click(filter_btn.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("horizons-filter-popover").is_some());

    let tag_chip = cx.debug_bounds("horizons-filter-tag-urgent").expect("urgent chip");
    cx.simulate_click(tag_chip.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(bounds_of(cx, format!("horizons-task-{}", task.id)).is_some());
    assert!(bounds_of(cx, format!("horizons-task-{}", other.id)).is_none());

    let reset = cx.debug_bounds("horizons-filter-reset").expect("reset");
    cx.simulate_click(reset.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(bounds_of(cx, format!("horizons-task-{}", other.id)).is_some());

    let toggle_done = cx.debug_bounds("horizons-toggle-completed").expect("completed toggle");
    cx.simulate_click(toggle_done.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(bounds_of(cx, format!("horizons-task-{}", done_task.id)).is_some());

    // Dismiss popover first.
    assert!(cx.update(|window, cx| view.update(cx, |v, cx| v.dismiss_transient(window, cx))));
    cx.run_until_parked();
    assert!(cx.debug_bounds("horizons-filter-popover").is_none());

    // Click task -> opens TaskDetail drawer, quick chips reassign cleanly.
    let task_row = bounds_of(cx, format!("horizons-task-{}", task.id)).expect("task row");
    cx.simulate_click(task_row.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("horizons-drawer").is_some());

    let chip_year = cx.debug_bounds("horizons-chip-year").expect("year chip");
    cx.simulate_click(chip_year.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(store.task(task.id).unwrap().unwrap().horizon, Some(Horizon::current_year(today)));

    let chip_today = cx.debug_bounds("horizons-chip-today").expect("today chip");
    cx.simulate_click(chip_today.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(store.task(task.id).unwrap().unwrap().due_date, Some(today));
    assert_eq!(store.task(task.id).unwrap().unwrap().horizon, None);

    // Dismiss drawer.
    assert!(cx.update(|window, cx| view.update(cx, |v, cx| v.dismiss_transient(window, cx))));
    cx.run_until_parked();
    assert!(cx.debug_bounds("horizons-drawer").is_none());
    assert!(!cx.update(|window, cx| view.update(cx, |v, cx| v.dismiss_transient(window, cx))));

    // Options popover hides reflection prompt and persists.
    let opt_btn = cx.debug_bounds("horizons-btn-options").expect("options btn");
    cx.simulate_click(opt_btn.center(), Modifiers::default());
    cx.run_until_parked();
    let prompt_toggle = cx.debug_bounds("horizons-opt-prompt").expect("prompt toggle");
    cx.simulate_click(prompt_toggle.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("horizons-prompt").is_none());
    assert_eq!(store.setting(SETTING_SHOW_PROMPT).unwrap().as_deref(), Some("0"));
}

#[gpui::test]
fn typing_in_detail_drawer_updates_card_title_via_task_text_edited(cx: &mut TestAppContext) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let today = HorizonsView::today();
    let task = store
        .create_task(
            TaskDraft {
                title: "Initial horizon".into(),
                due_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .expect("task");

    let (view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(1200.), px(800.)));
    cx.run_until_parked();

    let task_row = bounds_of(cx, format!("horizons-task-{}", task.id)).expect("task row");
    cx.simulate_click(task_row.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("horizons-drawer").is_some());

    let title_bounds = cx.debug_bounds("task-detail-title").expect("detail title visible");
    cx.simulate_click(title_bounds.center(), Modifiers::default());
    cx.simulate_keystrokes("end");
    cx.simulate_input("!");
    cx.run_until_parked();

    cx.update(|_, cx| {
        let v = view.read(cx);
        assert_eq!(v.tasks[0].title, "Initial horizon!");
        assert_eq!(v.detail.read(cx).task_title(), Some("Initial horizon!"));
        assert!(cx.try_global::<DataVersion>().is_none());
    });
    cx.executor().advance_clock(std::time::Duration::from_millis(260));
    cx.run_until_parked();
    assert_eq!(store.task(task.id).unwrap().unwrap().title, "Initial horizon!");

    let observer = cx.update(|_, cx| cx.new(|_| ()));
    let close_btn = cx.debug_bounds("horizons-drawer-close").expect("drawer close button");
    cx.simulate_click(close_btn.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("horizons-drawer").is_none());
    cx.update(|_, cx| assert!(changed_elsewhere(observer.entity_id(), cx)));
}

#[gpui::test]
fn inline_add_open_task_escape_and_undo_focus_guards(cx: &mut TestAppContext) {
    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let today = HorizonsView::today();
    let week = Horizon::current_week(today);
    let prev_week = week.shift(-1);
    let rolled = store
        .create_task(
            TaskDraft {
                title: "Past week item".into(),
                horizon: Some(prev_week),
                ..Default::default()
            },
            false,
        )
        .expect("past week task");
    let today_task = store
        .create_task(
            TaskDraft {
                title: "Open me".into(),
                due_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .expect("today task");

    let (view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
    cx.simulate_resize(size(px(1200.), px(800.)));
    cx.run_until_parked();

    // Perform a rollover so the undo stack is non-empty, then open inline add and press Ctrl+Z:
    // empty TextInput undo must not silently revert the background rollover.
    let move_btn = bounds_of(cx, format!("rollover-move-all-horizon-{}", week.as_code()))
        .expect("week rollover button");
    cx.simulate_click(move_btn.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(store.task(rolled.id).unwrap().unwrap().horizon, Some(week));

    let add_btn = bounds_of(cx, "inline-add-inbox").expect("inbox inline add");
    cx.simulate_click(add_btn.center(), Modifiers::default());
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-z");
    cx.run_until_parked();
    assert_eq!(
        store.task(rolled.id).unwrap().unwrap().horizon,
        Some(week),
        "Ctrl+Z inside focused inline_add must not undo background Horizons entries"
    );

    // Clicking a task while inline_add is open closes inline_add, restores focus, and lets Escape dismiss the drawer.
    let row = bounds_of(cx, format!("horizons-task-{}", today_task.id)).expect("today task row");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.inline_add.is_none() && v.detail_task == Some(today_task.id)));
    assert!(cx.update(|window, cx| view.read(cx).focus_handle.is_focused(window)));

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("horizons-drawer").is_none());
    assert!(cx.update(|window, cx| view.read(cx).focus_handle.is_focused(window)));
}

#[gpui::test]
fn before_task_complete_hook_blocks_horizons_completion_and_toasts(cx: &mut TestAppContext) {
    use desktop_common::plugin_bus::{PluginBus, PluginHookDecision, PluginHooksGlobal};
    use std::sync::Arc;

    setup_app(cx);
    let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
    let today = HorizonsView::today();
    let task = store
        .create_task(
            fern_core::TaskDraft {
                title: "Blocked horizon task".into(),
                due_date: Some(today),
                ..Default::default()
            },
            false,
        )
        .expect("task");

    cx.update(|cx| {
        PluginHooksGlobal::set_global(
            PluginHooksGlobal::new(
                Arc::new(|_| Ok(PluginHookDecision::Allow)),
                Arc::new(|_| Ok(PluginHookDecision::Cancel("blocked by rule".into()))),
                Arc::new(|_| Ok(PluginHookDecision::Allow)),
            ),
            cx,
        );
    });

    let (view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
    cx.run_until_parked();

    view.update(cx, |this, cx| this.toggle_task_done(task.id, cx));
    cx.run_until_parked();

    assert!(
        !store.task(task.id).unwrap().unwrap().is_completed(),
        "beforeTaskComplete Cancel hook must block Horizons completion"
    );
    let has_toast = cx.update(|_window, cx| {
        PluginBus::global(cx)
            .is_some_and(|bus| bus.toasts.iter().any(|t| t.message == "blocked by rule"))
    });
    assert!(has_toast, "cancelled Horizons completion must push warning toast");
}
