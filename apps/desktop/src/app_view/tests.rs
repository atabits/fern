use super::*;
use super::sync_loop::format_sync_error;
use crate::reminders::Target;

fn builtins_of(items: &[RailItemId]) -> Vec<fern_core::RailSection> {
    items
        .iter()
        .filter_map(|item| match item {
            RailItemId::BuiltIn(section) => Some(*section),
            RailItemId::Plugin { .. } => None,
        })
        .collect()
}

#[gpui::test]
fn update_notification_opens_the_installed_notes_without_an_updater(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    ReleaseNotes::new(release_notes::CURRENT_VERSION, "https://example.com/fern", "- Cached release changes.")
        .cache_for_restart(&store)
        .unwrap();
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));

    cx.simulate_resize(gpui::size(px(900.), px(400.)));
    let bell = cx.debug_bounds("notification-bell").unwrap();
    let notice = cx.debug_bounds("update-notice").unwrap();
    let button = cx.debug_bounds("updated-whats-new").unwrap();
    assert!(notice.left() >= bell.right());
    assert_eq!(notice.bottom(), bell.bottom());
    assert!(notice.top() >= title_bar::height() && notice.bottom() <= px(400.));
    assert!(cx.update(|_, cx| app.read(cx).settings.is_none()));
    cx.simulate_click(button.center(), gpui::Modifiers::default());
    let settings_window = cx.update(|_, cx| {
        assert!(app.read(cx).update_notice.is_none());
        app.read(cx).settings.unwrap()
    });
    let mut settings_cx = gpui::VisualTestContext::from_window(settings_window.into(), cx);
    assert!(settings_cx.debug_bounds("release-notes-page").is_some());
}

#[gpui::test]
fn escape_dismisses_focused_ui_and_keeps_the_main_view(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, true, window, cx));

    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| assert!(!app.read(cx).todo.focus_handle(cx).is_focused(window)));

    let settings_window = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(None, window, cx);
            app.settings.unwrap()
        })
    });
    let mut settings_cx = gpui::VisualTestContext::from_window(settings_window.into(), cx);
    settings_cx.simulate_keystrokes("escape");
    cx.update(|_, cx| {
        assert!(app.read(cx).settings.is_none());
        assert_eq!(cx.windows().len(), 1);
    });
    cx.simulate_keystrokes("escape escape");
    cx.update(|_, cx| {
        assert_eq!(app.read(cx).section, Section::Tasks);
        assert!(app.read(cx).settings.is_none());
    });
}

#[gpui::test]
fn user_escape_minimize_respects_text_inputs_and_transient_ui(cx: &mut gpui::TestAppContext) {
    let user_keymap = r#"[{"context": "Workspace && !TextInput", "bindings": {"escape": "fern::MinimizeWindow"}}]"#;
    cx.update(|cx| {
        theme::init(cx);
        let errors = crate::keymap::load_with_user(&assets::Assets, Some(user_keymap), cx).unwrap();
        assert!(errors.is_empty(), "keymap errors: {errors:?}");
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, true, window, cx));

    // 1. Focus is initially in quick-add TextInput: Esc blurs to task list without minimizing.
    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| {
        assert!(!app.read(cx).todo.focus_handle(cx).is_focused(window));
        assert_eq!(app.read(cx).minimized_count, 0);
    });

    // 2. When a transient overlay (notices popover) is open, Esc closes it instead of minimizing.
    cx.update(|_, cx| app.update(cx, |app, cx| app.set_notices_open(true, cx)));
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| {
        assert!(!app.read(cx).notices_open);
        assert_eq!(app.read(cx).minimized_count, 0);
    });

    // 3. When no TextInput or transient UI is active, Esc triggers MinimizeWindow.
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| assert_eq!(app.read(cx).minimized_count, 1));
}

#[gpui::test]
fn switching_modules_finishes_the_calendar_draft_and_keeps_the_destination_focus(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| {
        let mut app = AppView::new(store.clone(), true, window, cx);
        app.select(Section::Calendar, window, cx);
        app
    });
    cx.simulate_resize(gpui::size(px(1200.), px(650.)));
    let modifiers = gpui::Modifiers::default();
    let add = cx.debug_bounds("calendar-add-frame").unwrap().center();
    cx.simulate_click(add, modifiers);
    cx.simulate_input("Save before switching modules");
    let date = cx.debug_bounds("task-detail-due-date").unwrap().center();
    cx.simulate_click(date, modifiers);
    assert!(cx.debug_bounds("due-popup-ok").is_some());

    cx.update(|window, cx| app.update(cx, |app, cx| assert!(app.select(Section::Matrix, window, cx))));

    assert!(cx.debug_bounds("calendar-task-editor").is_none());
    assert!(cx.debug_bounds("matrix-header-menu-trigger").is_some());
    cx.update(|window, cx| assert!(app.read(cx).matrix.focus_handle(cx).is_focused(window)));
    let tasks = store.tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "Save before switching modules");
}

#[gpui::test]
fn window_close_and_quit_finish_the_calendar_draft(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (_, cx) = cx.add_window_view(|window, cx| {
        let mut app = AppView::new(store.clone(), true, window, cx);
        app.select(Section::Calendar, window, cx);
        app
    });
    cx.update(|window, cx| AppView::register_window_actions(window.window_handle().downcast().unwrap(), cx));
    cx.simulate_resize(gpui::size(px(1200.), px(650.)));

    for (index, (title, action)) in [
        ("Native close", None),
        ("Quit action", Some(Box::new(Quit) as Box<dyn gpui::Action>)),
        ("Close action", Some(Box::new(CloseWindow) as Box<dyn gpui::Action>)),
    ].into_iter().enumerate() {
        let add = cx.debug_bounds("calendar-add-frame").unwrap().center();
        cx.simulate_click(add, gpui::Modifiers::default());
        cx.simulate_input(title);
        assert_eq!(store.tasks().unwrap().len(), index);

        if let Some(action) = action {
            cx.update(|window, cx| window.dispatch_action(action, cx));
        } else {
            assert!(cx.simulate_close());
        }

        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), index + 1);
        assert!(tasks.iter().any(|task| task.title == title));
    }
}

#[gpui::test]
fn tray_quit_finishes_the_calendar_draft(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (_, cx) = cx.add_window_view(|window, cx| {
        let mut app = AppView::new(store.clone(), true, window, cx);
        app.select(Section::Calendar, window, cx);
        app
    });
    let (tx, rx) = futures::channel::mpsc::unbounded();
    cx.update(|window, cx| {
        crate::tray::connect(rx, window.window_handle().downcast().unwrap(), store.clone(), cx);
    });
    cx.simulate_resize(gpui::size(px(1200.), px(650.)));

    let add = cx.debug_bounds("calendar-add-frame").unwrap().center();
    cx.simulate_click(add, gpui::Modifiers::default());
    cx.simulate_input("Tray quit");
    assert!(store.tasks().unwrap().is_empty());

    tx.unbounded_send(crate::tray::TrayAction::Quit).unwrap();
    cx.run_until_parked();
    let tasks = store.tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "Tray quit");
}

#[gpui::test]
fn settings_scroll_below_the_header_in_a_short_window(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TouchPhase, point, size};

    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, true, window, cx));
    let settings_window = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(None, window, cx);
            app.settings.unwrap()
        })
    });
    let mut settings_cx = gpui::VisualTestContext::from_window(settings_window.into(), cx);
    settings_cx.simulate_resize(size(px(900.), px(400.)));
    settings_window.update(&mut settings_cx, |settings, _, cx| {
        settings.set_page(settings_window::Page::More, cx);
    }).unwrap();
    let header = settings_cx.debug_bounds("settings-header").unwrap();
    let page = settings_cx.debug_bounds("settings-page").unwrap();
    let language = settings_cx.debug_bounds("settings-language").unwrap();
    assert_eq!(header.top(), px(0.));
    assert_eq!(page.top(), header.bottom());
    assert!(page.bottom() <= px(400.));
    assert!(language.top() >= page.top());

    let position = page.center();
    let modifiers = Modifiers::default();
    settings_cx.simulate_mouse_move(position, None, modifiers);
    settings_cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(-160.))),
        modifiers,
        touch_phase: TouchPhase::Moved,
    });
    assert!(settings_cx.debug_bounds("settings-language").unwrap().top() < language.top());
    assert_eq!(settings_cx.debug_bounds("settings-header"), Some(header));

    for page in [settings_window::Page::SmartList, settings_window::Page::More] {
        settings_window.update(&mut settings_cx, |settings, _, cx| settings.set_page(page, cx)).unwrap();
    }
    assert_eq!(settings_cx.debug_bounds("settings-language"), Some(language));
}

#[gpui::test]
fn settings_window_is_separate_single_instance_and_updates_main_window_live(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store.clone(), true, window, cx));

    let first = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(None, window, cx);
            app.settings.unwrap()
        })
    });
    cx.update(|_, cx| assert_eq!(cx.windows().len(), 2));
    assert!(cx.debug_bounds("settings-header").is_none(), "settings must not render inside the main window");

    // Opening settings again reuses the existing window instead of spawning a second one.
    let second = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(None, window, cx);
            app.settings.unwrap()
        })
    });
    assert_eq!(first, second);
    cx.update(|_, cx| assert_eq!(cx.windows().len(), 2));

    // Changing a feature in the separate Settings window updates the main window live while
    // keeping Settings open.
    let mut settings_cx = gpui::VisualTestContext::from_window(first.into(), cx);
    first.update(&mut settings_cx, |settings, _, cx| {
        settings.set_page(settings_window::Page::Features, cx);
    }).unwrap();
    cx.update(|window, cx| app.update(cx, |app, cx| assert!(app.select(Section::Calendar, window, cx))));
    let calendar_toggle = settings_cx.debug_bounds("settings-feature-0").unwrap();
    settings_cx.simulate_click(calendar_toggle.center(), gpui::Modifiers::default());
    cx.update(|_, cx| {
        assert!(!app.read(cx).features.calendar);
        assert_eq!(app.read(cx).section, Section::Tasks);
        assert!(app.read(cx).settings.is_some());
        assert_eq!(cx.windows().len(), 2);
    });

    // Clicking Shortcuts in Settings switches to Page::Shortcuts inside the Settings window.
    let shortcuts_item = settings_cx.debug_bounds("settings-shortcuts").unwrap();
    settings_cx.simulate_click(shortcuts_item.center(), gpui::Modifiers::default());
    cx.update(|_, cx| {
        assert!(app.read(cx).settings.is_some());
        assert_eq!(cx.windows().len(), 2);
        let page = first.read_with(cx, |settings, _| settings.current_page()).unwrap();
        assert_eq!(page, settings_window::Page::Shortcuts);
    });
    assert!(settings_cx.debug_bounds("settings-shortcuts-page").is_some());

    // Closing Settings via native close clears AppView::settings.
    assert!(settings_cx.simulate_close());
    // `simulate_close` only runs `on_window_should_close`; mirror the OS removing the window.
    first.update(&mut settings_cx, |_, window, _| window.remove_window()).unwrap();
    cx.update(|_, cx| {
        assert!(app.read(cx).settings.is_none());
        assert_eq!(cx.windows().len(), 1);
    });

    // ShowShortcuts opens Settings directly on Page::Shortcuts.
    let shortcuts_win = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings_page(settings_window::Page::Shortcuts, window, cx);
            app.settings.unwrap()
        })
    });
    cx.update(|_, cx| {
        let page = shortcuts_win.read_with(cx, |settings, _| settings.current_page()).unwrap();
        assert_eq!(page, settings_window::Page::Shortcuts);
    });
    let mut reopened_cx = gpui::VisualTestContext::from_window(shortcuts_win.into(), cx);
    assert!(reopened_cx.simulate_close());
    shortcuts_win.update(&mut reopened_cx, |_, window, _| window.remove_window()).unwrap();
    cx.update(|_, cx| {
        assert!(app.read(cx).settings.is_none());
        assert_eq!(cx.windows().len(), 1);
    });

    // Reopen Settings and verify closing the main window also closes the Settings window.
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_settings(None, window, cx)));
    cx.update(|_, cx| {
        assert!(app.read(cx).settings.is_some());
        assert_eq!(cx.windows().len(), 2);
    });
    assert!(cx.simulate_close());
    cx.update(|_, cx| {
        assert!(app.read(cx).settings.is_none());
        assert_eq!(cx.windows().len(), 1);
    });
}

#[gpui::test]
fn title_bar_overlays_full_height_columns_above_header_controls(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Check merged title bar").unwrap();
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, true, window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));
    cx.update(|_, cx| {
        let todo = app.read(cx).todo.clone();
        todo.update(cx, |todo, cx| todo.select(task.id, cx));
    });

    let bar = cx.debug_bounds("title-bar").unwrap();
    let sidebar = cx.debug_bounds("sidebar").unwrap();
    let lists = cx.debug_bounds("lists-column").unwrap();
    let sort = cx.debug_bounds("task-sort-menu-trigger").unwrap();
    let priority = cx.debug_bounds("task-detail-priority").unwrap();
    assert_eq!(bar.top(), px(0.));
    assert_eq!(bar.bottom(), title_bar::height());
    assert_eq!(sidebar.top(), px(0.));
    assert_eq!(sidebar.bottom(), px(700.));
    assert_eq!(lists.top(), px(0.));
    assert_eq!(lists.bottom(), px(700.));
    assert!(sort.top() >= bar.bottom());
    assert!(priority.top() >= bar.bottom());

    cx.update(|window, cx| app.update(cx, |app, cx| assert!(app.select(Section::Calendar, window, cx))));
    let more = cx.debug_bounds("calendar-more-button").unwrap();
    assert!(more.top() >= bar.bottom());

    cx.update(|window, cx| app.update(cx, |app, cx| assert!(app.select(Section::Matrix, window, cx))));
    let matrix_more = cx.debug_bounds("matrix-header-menu-trigger").unwrap();
    assert!(matrix_more.top() >= bar.bottom());
}

#[gpui::test]
fn sidebar_rail_drag_reorders_navigation_icons(cx: &mut gpui::TestAppContext) {
    use fern_core::RailSection;
    use gpui::{Modifiers, MouseButton, point, size};

    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store.clone(), true, window, cx));
    cx.simulate_resize(size(px(1200.), px(700.)));
    cx.run_until_parked();

    let modifiers = Modifiers::default();

    // Normal click switches sections.
    let habits = cx.debug_bounds("section-habits").unwrap();
    cx.simulate_click(habits.center(), modifiers);
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(app.read(cx).section, Section::Habits));

    // Drag Calendar up onto Tasks through multiple incremental steps: icons reorder LIVE while
    // dragging, never stick after the initial threshold step, and releasing even far outside
    // the narrow 50 px rail still commits without switching sections.
    let cal = cx.debug_bounds("section-calendar").unwrap();
    let tasks = cx.debug_bounds("section-tasks").unwrap();
    cx.simulate_mouse_move(cal.center(), None, modifiers);
    cx.simulate_mouse_down(cal.center(), MouseButton::Left, modifiers);
    for dy in [5., 12., 25.] {
        cx.simulate_mouse_move(
            point(cal.center().x, cal.center().y - px(dy)),
            Some(MouseButton::Left),
            modifiers,
        );
        cx.run_until_parked();
    }
    let lifted_over_tasks = point(tasks.center().x + px(120.), tasks.center().y);
    cx.simulate_mouse_move(lifted_over_tasks, Some(MouseButton::Left), modifiers);
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("section-calendar").unwrap().top() < cx.debug_bounds("section-tasks").unwrap().top(),
        "neighbors slide live before MouseUp"
    );
    cx.simulate_mouse_up(lifted_over_tasks, MouseButton::Left, modifiers);
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert_eq!(app.read(cx).section, Section::Habits, "dragging does not switch active section");
        let builtins = builtins_of(&app.read(cx).rail_items);
        assert_eq!(builtins[0], RailSection::Calendar);
        assert_eq!(builtins[1], RailSection::Tasks);
    });
    assert_eq!(store.preferences().unwrap().rail_order[0], RailSection::Calendar);

    // Escape cancels an in-progress drag and restores the previous order.
    let cal = cx.debug_bounds("section-calendar").unwrap();
    cx.simulate_mouse_move(cal.center(), None, modifiers);
    cx.simulate_mouse_down(cal.center(), MouseButton::Left, modifiers);
    cx.simulate_mouse_move(point(cal.center().x, cal.center().y + px(8.)), Some(MouseButton::Left), modifiers);
    cx.run_until_parked();
    cx.simulate_mouse_move(point(cal.center().x, cal.center().y + px(120.)), Some(MouseButton::Left), modifiers);
    cx.run_until_parked();
    cx.update(|_, cx| assert_ne!(builtins_of(&app.read(cx).rail_items)[0], RailSection::Calendar));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(app.read(cx).dragging_rail.is_none());
        assert_eq!(builtins_of(&app.read(cx).rail_items)[0], RailSection::Calendar);
    });
}

#[gpui::test]
fn active_alert_banner_snoozes_and_completes_task(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let t1 = store.add_task("Call bank").unwrap();
    let t2 = store.add_task("Pay rent").unwrap();
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store.clone(), true, window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));
    cx.run_until_parked();

    let at = chrono::Local::now().naive_local();
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.push_active_alerts(
                vec![Notice { target: Target::Task(t1.id), title: t1.title.clone(), at, constant: true, has_goal: false }],
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("active-reminder-alerts").is_some());

    // Click "Snooze 5m" on active alert card -> sets snoozed_until and removes card.
    let snooze_5m = cx.debug_bounds("alert-0-snooze-5m").unwrap().center();
    cx.simulate_click(snooze_5m, gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(store.task(t1.id).unwrap().unwrap().snoozed_until.is_some());
    assert!(cx.debug_bounds("active-reminder-alerts").is_none());

    // Push second alert -> click "Done" -> marks task completed and removes card.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.push_active_alerts(
                vec![Notice { target: Target::Task(t2.id), title: t2.title.clone(), at, constant: false, has_goal: false }],
                cx,
            );
        });
    });
    cx.run_until_parked();
    let complete = cx.debug_bounds("alert-0-complete").unwrap().center();
    cx.simulate_click(complete, gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(store.task(t2.id).unwrap().unwrap().is_completed());
    assert!(cx.debug_bounds("active-reminder-alerts").is_none());
}

#[gpui::test]
fn tray_focus_starts_pomo_and_stopwatch_without_switching_section(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));
    cx.run_until_parked();

    // Starting Pomo from tray starts a countdown timer while keeping the main section unchanged.
    cx.update(|_, cx| app.update(cx, |app, cx| app.start_focus_from_tray(false, cx)));
    cx.run_until_parked();
    cx.update(|_, cx| {
        let view = app.read(cx);
        assert_eq!(view.section, Section::Tasks);
        let now = chrono::Local::now().timestamp_millis();
        let (_, _, running, paused) = view.focus.read(cx).clock_state(now, i18n::t(cx));
        assert!(running);
        assert!(!paused);
    });

    // Starting Stopwatch switches the timer to stopwatch mode and runs it.
    cx.update(|_, cx| app.update(cx, |app, cx| app.start_focus_from_tray(true, cx)));
    cx.run_until_parked();
    cx.update(|_, cx| {
        let view = app.read(cx);
        assert_eq!(view.section, Section::Tasks);
        let now = chrono::Local::now().timestamp_millis();
        let (clock, _, running, paused) = view.focus.read(cx).clock_state(now, i18n::t(cx));
        assert!(running);
        assert!(!paused);
        assert!(clock.starts_with("00:"), "expected stopwatch starting at 00:.., got {clock}");
    });
}

#[test]
fn format_sync_error_localizes_protocol_and_e2ee_and_redacts_secrets() {
    let mismatch = anyhow::Error::new(fern_core::sync::IncompatibleProtocol {
        client: 2,
        server: 5,
        min_server: 3,
    });
    for locale in [i18n::Locale::En, i18n::Locale::Ru, i18n::Locale::ZhCn] {
        let strings = locale.strings();
        let msg = format_sync_error(&mismatch, strings);
        assert!(msg.contains("v2") && msg.contains("v5"), "{locale:?}: {msg}");
        assert!(!msg.contains("{client}") && !msg.contains("{server}"));

        let req = anyhow::Error::new(fern_core::e2ee::E2eeError::PassphraseRequired);
        assert_eq!(format_sync_error(&req, strings), strings.sync_e2ee_passphrase_required);

        let wrong = anyhow::Error::new(fern_core::e2ee::E2eeError::WrongPassphrase);
        assert_eq!(format_sync_error(&wrong, strings), strings.sync_e2ee_wrong_passphrase);
    }

    let leaky = anyhow::anyhow!(
        "POST https://private.example.org:8443/sync failed: Bearer tok_secret_99 passphrase=my-pass"
    );
    let redacted = format_sync_error(&leaky, &i18n::EN);
    assert!(redacted.contains("https://<redacted-host>"), "{redacted}");
    assert!(!redacted.contains("private.example.org"), "{redacted}");
    assert!(!redacted.contains("tok_secret_99"), "{redacted}");
    assert!(!redacted.contains("my-pass"), "{redacted}");
}

#[gpui::test]
fn complete_sync_updates_live_sync_status_global_with_localized_error(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.complete_sync(
                Err(anyhow::Error::new(fern_core::e2ee::E2eeError::WrongPassphrase)),
                cx,
            );
        });
    });
    cx.update(|_, cx| {
        let live = &cx.global::<crate::LiveSyncStatus>().0;
        assert_eq!(
            live,
            &crate::sync::SyncStatus::Failed(i18n::EN.sync_e2ee_wrong_passphrase.to_owned())
        );
    });
}

#[gpui::test]
fn open_task_routes_to_things_when_active_or_when_tasks_disabled(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        let _ = crate::keymap::load_default(&assets::Assets, cx);
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Route test task").unwrap();
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store.clone(), false, window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));
    cx.run_until_parked();

    // Enable Things and turn off Classic Tasks: apply_settings_changes switches to Things.
    store
        .update_preferences(|p| {
            p.features.tasks = false;
            p.features.things = true;
        })
        .unwrap();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.apply_settings_changes(window, cx);
            assert_eq!(app.section, Section::Things);
            app.open_task(task.id, window, cx);
            assert_eq!(app.section, Section::Things);
        });
    });
    cx.run_until_parked();

    // Turn Classic Tasks back on and turn Things off: auto-switches to Tasks.
    store
        .update_preferences(|p| {
            p.features.tasks = true;
            p.features.things = false;
        })
        .unwrap();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.apply_settings_changes(window, cx);
            assert_eq!(app.section, Section::Tasks);
        });
    });
}

#[gpui::test]
fn plugin_bus_nav_actions_and_rail_reordering_work_with_plugins(cx: &mut gpui::TestAppContext) {
    use desktop_common::plugin_bus::PluginSidebarViewSpec;

    cx.update(|cx| {
        theme::init(cx);
        let _ = crate::keymap::load_default(&assets::Assets, cx);
        PluginBus::update(cx, |bus| {
            bus.sidebar_views.push(PluginSidebarViewSpec {
                plugin_id: "demo".into(),
                view_id: "panel".into(),
                title: "Demo Panel".into(),
                icon_svg: None,
            });
        });
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let task = store.add_task("Bus task").unwrap();
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store.clone(), false, window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));
    cx.run_until_parked();

    // Rail visible items includes active plugin view and supports reordering it.
    cx.update(|_, cx| {
        let views = cx.global::<PluginBus>().sidebar_views.clone();
        let items = app.read(cx).rail_items.clone();
        let visible = sidebar::visible_rail_items(&items, app.read(cx).features, &views);
        let plugin_item = RailItemId::Plugin {
            plugin_id: "demo".into(),
            view_id: "panel".into(),
        };
        assert!(visible.contains(&plugin_item));
        let last = visible.len() - 1;
        let reordered = reorder_visible_rail_items(&items, &visible, last, 0);
        assert_eq!(reordered[0], plugin_item);
    });

    // PluginBus OpenPluginView and OpenTask drain and dispatch cleanly.
    cx.update(|_, cx| {
        PluginBus::update(cx, |bus| {
            bus.pending_nav.push_back(PluginNavAction::OpenPluginView {
                plugin_id: "demo".into(),
                view_id: "panel".into(),
            });
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            app.read(cx).section,
            Section::Plugin {
                plugin_id: "demo".into(),
                view_id: "panel".into(),
            }
        );
        assert!(app.read(cx).plugin_views.contains_key(&("demo".into(), "panel".into())));
        PluginBus::update(cx, |bus| {
            bus.pending_nav.push_back(PluginNavAction::OpenTask(task.id));
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(app.read(cx).section, Section::Tasks));
}

#[gpui::test]
fn settings_window_reopen_while_closing_spawns_fresh_window_and_ignores_stale_dismiss(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));

    let first = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(None, window, cx);
            app.settings.unwrap()
        })
    });

    // Start closing the first Settings window (`closing = true`, deferred `remove_window()`),
    // then immediately reopen before deferred removal executes.
    let second = cx.update(|window, cx| {
        first
            .update(cx, |settings, settings_win, cx| {
                settings.close(settings_win, cx);
            })
            .unwrap();
        app.update(cx, |app, cx| {
            app.open_settings_page(settings_window::Page::Plugins, window, cx);
            app.settings.unwrap()
        })
    });
    assert_ne!(first, second, "must not reuse a closing SettingsWindow handle");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(app.read(cx).settings, Some(second));
        assert_eq!(
            second.read_with(cx, |s, _| s.current_page()).unwrap(),
            settings_window::Page::Plugins
        );
    });
}

#[gpui::test]
fn show_view_and_quick_add_do_not_jump_into_disabled_tasks_section(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        theme::init(cx);
        let _ = crate::keymap::load_default(&assets::Assets, cx);
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    store
        .update_preferences(|p| {
            p.features.tasks = false;
            p.features.things = true;
        })
        .unwrap();
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert_eq!(app.section, Section::Things);
            app.show_view(fern_core::View::Today, window, cx);
            assert_eq!(app.section, Section::Things);
            assert_eq!(app.things.read(cx).view(), things_ui::ThingsViewKind::Today);

            app.show_view(fern_core::View::Inbox, window, cx);
            assert_eq!(app.section, Section::Things);
            assert_eq!(app.things.read(cx).view(), things_ui::ThingsViewKind::Inbox);

            app.show_view(fern_core::View::Trash, window, cx);
            assert_eq!(app.section, Section::Things, "Trash must not jump to disabled Tasks");

            app.quick_add(window, cx);
            assert_eq!(app.section, Section::Things);
        });
    });
}

#[gpui::test]
fn open_plugin_view_validates_target_and_escape_dismisses_plugin_modal_first(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        theme::init(cx);
        let _ = crate::keymap::load_default(&assets::Assets, cx);
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));

    // Unknown plugin view target must be ignored (not switched to).
    cx.update(|_, cx| {
        PluginBus::update(cx, |bus| {
            bus.pending_nav.push_back(PluginNavAction::OpenPluginView {
                plugin_id: "nonexistent".into(),
                view_id: "ghost".into(),
            });
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(app.read(cx).section, Section::Tasks);
        assert!(app.read(cx).plugin_views.is_empty());
    });

    // Open a plugin modal while notices popover is also open -> Escape closes modal first.
    cx.update(|_, cx| {
        use desktop_common::plugin_bus::{PluginModalKind, PluginModalSpec};
        app.update(cx, |app, cx| app.set_notices_open(true, cx));
        PluginBus::update(cx, |bus| {
            bus.open_modal(PluginModalSpec {
                plugin_id: "demo".into(),
                view_id: "panel".into(),
                title: "Confirm".into(),
                kind: PluginModalKind::Confirm {
                    body: "Proceed?".into(),
                    request_id: 7,
                },
            });
        });
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(cx.global::<PluginBus>().active_modal.is_none());
        assert!(app.read(cx).notices_open, "notices must remain open after Escape closes modal");
    });
}

#[gpui::test]
fn life_clock_title_bar_appearance_tracks_palette_section_and_statistics(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert_eq!(app.read(cx).title_bar.read(cx).caption_appearance(), None);
    });

    // Entering Life Clock in default Classic B&W mode sets Light caption appearance.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| assert!(app.select(Section::LifeClock, window, cx)));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(app.read(cx).section, Section::LifeClock);
        assert_eq!(
            app.read(cx).title_bar.read(cx).caption_appearance(),
            Some(theme::Appearance::Light)
        );
    });

    // Opening Statistics over Life Clock resets caption appearance; closing restores it.
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_statistics(window, cx)));
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(app.read(cx).statistics_open);
        assert_eq!(app.read(cx).title_bar.read(cx).caption_appearance(), None);
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(!app.read(cx).statistics_open);
        assert_eq!(
            app.read(cx).title_bar.read(cx).caption_appearance(),
            Some(theme::Appearance::Light)
        );
    });

    // Switching to FernTheme clears the light override; switching back to Classic restores it.
    cx.update(|_, cx| {
        let lc = app.read(cx).life_clock.clone();
        lc.update(cx, |view, cx| view.dev_set_palette(LifeClockPaletteMode::FernTheme, cx));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(app.read(cx).title_bar.read(cx).caption_appearance(), None);
    });
    cx.update(|_, cx| {
        let lc = app.read(cx).life_clock.clone();
        lc.update(cx, |view, cx| view.dev_set_palette(LifeClockPaletteMode::Classic, cx));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            app.read(cx).title_bar.read(cx).caption_appearance(),
            Some(theme::Appearance::Light)
        );
    });

    // Leaving Life Clock clears the override.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| assert!(app.select(Section::Tasks, window, cx)));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(app.read(cx).title_bar.read(cx).caption_appearance(), None);
    });
}

#[gpui::test]
fn life_clock_sidebar_toggle_and_escape_restore(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));
    cx.run_until_parked();

    cx.update(|window, cx| {
        app.update(cx, |app, cx| assert!(app.select(Section::LifeClock, window, cx)));
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("sidebar").is_some());
    cx.update(|_, cx| assert!(!app.read(cx).life_clock_rail_hidden));

    // ToggleSidebar action hides the left rail on Life Clock.
    cx.update(|window, cx| window.dispatch_action(Box::new(ToggleSidebar), cx));
    cx.run_until_parked();
    cx.update(|_, cx| assert!(app.read(cx).life_clock_rail_hidden));
    assert!(cx.debug_bounds("sidebar").is_none());

    // Escape restores the hidden rail via DismissTransient.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| assert!(!app.read(cx).life_clock_rail_hidden));
    assert!(cx.debug_bounds("sidebar").is_some());

    // Clicking the in-view sidebar toggle emits LifeClockEvent::ToggleSidebar.
    let toggle = cx.debug_bounds("life-clock-sidebar-toggle").unwrap();
    cx.simulate_click(toggle.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| assert!(app.read(cx).life_clock_rail_hidden));
    assert!(cx.debug_bounds("sidebar").is_none());

    // With popover open and rail hidden, first Escape closes popover and second restores rail.
    cx.update(|_, cx| {
        let lc = app.read(cx).life_clock.clone();
        lc.update(cx, |view, cx| view.dev_open_popover(true, cx));
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| assert!(app.read(cx).life_clock_rail_hidden));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| assert!(!app.read(cx).life_clock_rail_hidden));
    assert!(cx.debug_bounds("sidebar").is_some());

    // Leaving Life Clock while rail is hidden resets rail visibility cleanly.
    cx.update(|window, cx| window.dispatch_action(Box::new(ToggleSidebar), cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("sidebar").is_none());
    cx.update(|window, cx| {
        app.update(cx, |app, cx| assert!(app.select(Section::Tasks, window, cx)));
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert!(!app.read(cx).life_clock_rail_hidden));
    assert!(cx.debug_bounds("sidebar").is_some());
}

#[gpui::test]
fn disabling_life_clock_feature_falls_back_to_task_section(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(700.)));
    cx.run_until_parked();

    assert!(cx.debug_bounds("section-life-clock").is_some());
    cx.update(|window, cx| {
        app.update(cx, |app, cx| assert!(app.select(Section::LifeClock, window, cx)));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(app.read(cx).section, Section::LifeClock);
        assert_eq!(
            app.read(cx).title_bar.read(cx).caption_appearance(),
            Some(theme::Appearance::Light)
        );
    });

    let settings_window = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings_page(settings_window::Page::Features, window, cx);
            app.settings.unwrap()
        })
    });
    let mut settings_cx = gpui::VisualTestContext::from_window(settings_window.into(), cx);
    settings_cx.simulate_resize(gpui::size(px(900.), px(780.)));
    settings_cx.run_until_parked();

    let life_clock_toggle = settings_cx.debug_bounds("settings-feature-5").unwrap();
    settings_cx.simulate_click(life_clock_toggle.center(), gpui::Modifiers::default());
    settings_cx.run_until_parked();
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert!(!app.read(cx).features.life_clock);
        assert_eq!(app.read(cx).section, Section::Tasks);
        assert_eq!(app.read(cx).title_bar.read(cx).caption_appearance(), None);
    });
    assert!(cx.debug_bounds("section-life-clock").is_none());
}

#[gpui::test]
fn horizons_view_embeds_in_tasks_and_disabling_feature_falls_back_cleanly(
    cx: &mut gpui::TestAppContext,
) {
    let _horizons_guard = lock_horizons_flag();
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(760.)));
    cx.run_until_parked();

    // Horizons has no duplicate left-rail icon; it lives inside Tasks as View::Horizons.
    assert!(cx.debug_bounds("section-horizons").is_none());
    assert!(crate::calendar::horizons_enabled());

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.show_view(fern_core::View::Horizons, window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(app.read(cx).section, Section::Tasks);
    });
    assert!(cx.debug_bounds("lists-column").is_some());
    assert!(cx.debug_bounds("horizons-view").is_some());

    // ToggleSidebar collapses and restores the unified Tasks sidebar while on View::Horizons.
    cx.update(|window, cx| window.dispatch_action(Box::new(ToggleSidebar), cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("lists-column").is_none());
    assert!(cx.debug_bounds("horizons-view").is_some());
    cx.update(|window, cx| window.dispatch_action(Box::new(ToggleSidebar), cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("lists-column").is_some());

    // Turning off Settings -> Features -> Horizons syncs calendar::horizons_enabled() == false
    // and falls back out of View::Horizons cleanly.
    let settings_window = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings_page(settings_window::Page::Features, window, cx);
            app.settings.unwrap()
        })
    });
    let mut settings_cx = gpui::VisualTestContext::from_window(settings_window.into(), cx);
    settings_cx.simulate_resize(gpui::size(px(900.), px(880.)));
    settings_cx.run_until_parked();

    let horizons_toggle = settings_cx.debug_bounds("settings-feature-6").unwrap();
    settings_cx.simulate_click(horizons_toggle.center(), gpui::Modifiers::default());
    settings_cx.run_until_parked();
    cx.run_until_parked();

    cx.update(|_, cx| {
        assert!(!app.read(cx).features.horizons);
        assert!(!crate::calendar::horizons_enabled());
        assert_eq!(app.read(cx).section, Section::Tasks);
    });
    assert!(cx.debug_bounds("horizons-view").is_none());

    // Re-enabling Features -> Horizons restores calendar::horizons_enabled() == true.
    let horizons_toggle = settings_cx.debug_bounds("settings-feature-6").unwrap();
    settings_cx.simulate_click(horizons_toggle.center(), gpui::Modifiers::default());
    settings_cx.run_until_parked();
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(app.read(cx).features.horizons);
        assert!(crate::calendar::horizons_enabled());
    });
}

#[gpui::test]
fn go_to_horizons_and_summary_actions_respect_horizons_flag(cx: &mut gpui::TestAppContext) {
    let _horizons_guard = lock_horizons_flag();
    cx.update(|cx| {
        theme::init(cx);
        assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
    });
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));
    cx.simulate_resize(gpui::size(px(1200.), px(760.)));
    cx.run_until_parked();

    // GoToHorizons switches from Calendar into Tasks + View::Horizons when enabled.
    cx.update(|window, cx| app.update(cx, |app, cx| assert!(app.select(Section::Calendar, window, cx))));
    cx.update(|window, cx| window.dispatch_action(Box::new(GoToHorizons), cx));
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(app.read(cx).section, Section::Tasks));
    assert!(cx.debug_bounds("horizons-view").is_some());

    // GoToSummary switches from Calendar into Tasks + View::Summary.
    cx.update(|window, cx| app.update(cx, |app, cx| assert!(app.select(Section::Calendar, window, cx))));
    cx.update(|window, cx| window.dispatch_action(Box::new(GoToSummary), cx));
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(app.read(cx).section, Section::Tasks));
    assert!(cx.debug_bounds("horizons-view").is_none());
    assert!(cx.debug_bounds("quick-add-input").is_none());

    // When Horizons is disabled, GoToHorizons is a no-op and stays on Calendar.
    set_horizons_enabled(false);
    cx.update(|window, cx| app.update(cx, |app, cx| assert!(app.select(Section::Calendar, window, cx))));
    cx.update(|window, cx| window.dispatch_action(Box::new(GoToHorizons), cx));
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(app.read(cx).section, Section::Calendar));
    assert!(cx.debug_bounds("horizons-view").is_none());
}

mod ux_perf_tests {
    use super::*;

    #[gpui::test]
    fn bell_clear_dismisses_active_alerts_and_renders_alerts_top_right(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            let _ = crate::keymap::load_default(&assets::Assets, cx);
        });
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Pay taxes").unwrap();
        let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));
        cx.simulate_resize(gpui::size(px(1200.), px(700.)));
        cx.run_until_parked();

        let at = chrono::Local::now().naive_local();
        let notice = Notice {
            target: Target::Task(task.id),
            title: task.title.clone(),
            at,
            constant: true,
            has_goal: false,
        };
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.add_notices(vec![notice.clone()], cx);
                app.push_active_alerts(vec![notice], cx);
                app.set_notices_open(true, cx);
            });
        });
        cx.run_until_parked();

        let alerts_bounds = cx.debug_bounds("active-reminder-alerts").unwrap();
        assert!(
            alerts_bounds.top() >= title_bar::height(),
            "alerts banner must anchor below the title bar, got top={:?}",
            alerts_bounds.top()
        );
        assert!(
            alerts_bounds.bottom() < px(350.),
            "alerts banner must stay top-right rather than covering bottom-right footers, got bottom={:?}",
            alerts_bounds.bottom()
        );
        let notice_row_bounds = cx.debug_bounds("notice-0").unwrap();
        assert!(
            notice_row_bounds.size.height > px(44.),
            "bell row with wrapped action buttons should size dynamically without fixed clipping: {:?}",
            notice_row_bounds.size.height
        );

        let clear_bounds = cx.debug_bounds("notices-clear-all").unwrap();
        cx.simulate_click(clear_bounds.center(), gpui::Modifiers::default());
        cx.run_until_parked();

        cx.update(|_, cx| {
            let view = app.read(cx);
            assert!(view.notices.items.is_empty());
            assert_eq!(view.notices.unread, 0);
            assert!(!view.has_active_alert(Target::Task(task.id)));
        });
        assert!(cx.debug_bounds("active-reminder-alerts").is_none());
    }

    #[gpui::test]
    fn unconfigured_sync_skips_background_polling_and_leaves_status_off_with_db_path(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = std::env::temp_dir().join(format!("fern-sync-idle-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("tasks.db");
        let store = Rc::new(Store::open(&db_path).unwrap());
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(crate::DbPath(db_path.clone()));
        });
        let (app, cx) = cx.add_window_view(|window, cx| AppView::new(store, false, window, cx));
        cx.run_until_parked();

        cx.update(|_, cx| {
            let view = app.read(cx);
            assert!(!view.has_sync_config);
            assert!(view.sync_task.is_none());
            assert_eq!(view.sync, crate::sync::SyncStatus::Off);
        });

        // Manual sync_now when unconfigured must stay Off without starting background work.
        cx.update(|_, cx| app.update(cx, |app, cx| app.sync_now(cx)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(app.read(cx).sync, crate::sync::SyncStatus::Off);
        });

        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
