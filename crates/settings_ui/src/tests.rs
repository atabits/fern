use std::rc::Rc;

use desktop_sync::subscriptions::{self, ChinaHolidaySource};
use fern_core::Store;
use fern_i18n::{self as i18n, CurrentLocale, Locale};
use gpui::{AppContext as _, point, px};

use super::backups_diagnostics::format_bytes;
use super::{ActiveConfirm, Menu, Page, SettingsHost, SettingsWindow, shortcuts_settings_view};

#[gpui::test]
fn task_positions_share_a_painted_settings_surface(cx: &mut gpui::TestAppContext) {
    cx.update(theme::init);
    let store = Rc::new(Store::open_in_memory().unwrap());
    let (settings, cx) = cx.add_window_view(|_, cx| {
        let mut settings = SettingsWindow::new(store, SettingsHost::noop(cx), cx);
        settings.set_page(Page::More, cx);
        settings
    });
    cx.simulate_resize(gpui::size(px(900.), px(720.)));

    for theme in [theme::Theme::light(), theme::Theme::dark()] {
        let surface = theme.colors().surface_background;
        cx.update(|_, cx| {
            theme::set(theme, cx);
            settings.update(cx, |settings, cx| {
                settings.page_scroll.scroll_to_bottom();
                cx.notify();
            });
        });
        let add_to = cx.debug_bounds("settings-row-AddTo").unwrap();
        let overdue = cx.debug_bounds("settings-row-OverdueAt").unwrap();
        cx.update(|window, _| {
            let scale = window.scale_factor();
            let positions = [add_to.center().scale(scale), overdue.center().scale(scale)];
            assert!(
                window.painted_quads().iter().any(|quad| {
                    quad.background.as_solid() == Some(surface)
                        && positions.iter().all(|point| {
                            quad.bounds.contains(point) && quad.content_mask.bounds.contains(point)
                        })
                }),
                "both task position rows must be painted on the same settings surface"
            );
        });
    }
}

#[gpui::test]
fn china_holidays_offer_both_sources_in_settings(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        assert!(
            desktop_common::keymap::load_default(&assets::Assets, cx)
                .unwrap()
                .is_empty()
        );
        cx.set_global(CurrentLocale(Locale::ZhCn));
        subscriptions::init(store.clone(), cx);
    });
    let (settings, cx) = cx.add_window_view(|window, cx| {
        let mut settings = SettingsWindow::new(store, SettingsHost::noop(cx), cx);
        settings.set_page(Page::Integrations, cx);
        window.focus(&settings.focus_handle, cx);
        settings
    });
    cx.simulate_resize(gpui::size(px(900.), px(720.)));

    let trigger = cx.debug_bounds("china-holiday-source").unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::default());
    for source in ChinaHolidaySource::ALL {
        assert!(cx.debug_bounds(source.label()).is_some());
    }
    cx.simulate_keystrokes("escape");
    assert!(cx.debug_bounds("china-holiday-source-menu").is_none());
    cx.update(|_, cx| assert_eq!(settings.read(cx).page, Page::Integrations));
}

#[gpui::test]
fn caldav_form_saves_and_preserves_credentials_on_visibility_toggle(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        assert!(
            desktop_common::keymap::load_default(&assets::Assets, cx)
                .unwrap()
                .is_empty()
        );
        cx.set_global(CurrentLocale(Locale::En));
        subscriptions::init(store.clone(), cx);
    });
    let (settings, cx) = cx.add_window_view(|window, cx| {
        let mut settings = SettingsWindow::new(store.clone(), SettingsHost::noop(cx), cx);
        settings.set_page(Page::Integrations, cx);
        window.focus(&settings.focus_handle, cx);
        settings
    });
    cx.simulate_resize(gpui::size(px(900.), px(720.)));

    // Open add form, switch to CalDAV + Fastmail preset, fill credentials, save.
    cx.update(|window, cx| {
        settings.update(cx, |s, cx| s.open_subscription_form(None, window, cx));
    });
    let kind_tabs = cx.debug_bounds("subscription-kind-tabs").unwrap();
    let caldav_btn = point(kind_tabs.left() + kind_tabs.size.width * 0.75, kind_tabs.center().y);
    cx.simulate_click(caldav_btn, gpui::Modifiers::default());
    let fastmail_btn = cx.debug_bounds("caldav-provider-fastmail").unwrap().center();
    cx.simulate_click(fastmail_btn, gpui::Modifiers::default());
    settings.update(cx, |s, cx| {
        s.subs.name.update(cx, |i, cx| i.set_text("Fastmail Work", cx));
        s.subs
            .username
            .update(cx, |i, cx| i.set_text("alice@fastmail.com", cx));
        s.subs
            .password
            .update(cx, |i, cx| i.set_text("app-secret", cx));
        s.save_subscription_form(cx);
    });
    cx.run_until_parked();

    let subs = store.subscriptions().unwrap();
    assert_eq!(subs.len(), 1);
    let id = subs[0].id;
    assert_eq!(subs[0].kind, fern_core::ical::CalendarKind::CalDav);
    assert_eq!(subs[0].provider, fern_core::ical::CalendarProvider::Fastmail);
    assert_eq!(subs[0].username, "alice@fastmail.com");
    assert_eq!(subs[0].password, "app-secret");
    assert!(subs[0].writable);

    // Toggle visibility -> CalDAV credentials/provider remain intact.
    let show_sel: &'static str = Box::leak(format!("subscription-show-{id}").into_boxed_str());
    let show_btn = cx.debug_bounds(show_sel).unwrap().center();
    cx.simulate_click(show_btn, gpui::Modifiers::default());
    cx.run_until_parked();
    let after = store.subscriptions().unwrap();
    assert!(after[0].hidden);
    assert_eq!(after[0].kind, fern_core::ical::CalendarKind::CalDav);
    assert_eq!(after[0].provider, fern_core::ical::CalendarProvider::Fastmail);
    assert_eq!(after[0].password, "app-secret");
}

#[gpui::test]
fn shortcuts_page_records_cancels_with_escape_and_confirms_reset(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        assert!(
            desktop_common::keymap::load_default(&assets::Assets, cx)
                .unwrap()
                .is_empty()
        );
        cx.set_global(CurrentLocale(Locale::En));
    });
    let (settings, cx) = cx.add_window_view(|window, cx| {
        let mut settings = SettingsWindow::new(store, SettingsHost::noop(cx), cx);
        settings.set_page(Page::Shortcuts, cx);
        window.focus(&settings.focus_handle, cx);
        settings
    });
    cx.simulate_resize(gpui::size(px(900.), px(720.)));

    // Shortcuts page and sticky Reset button are rendered.
    assert!(cx.debug_bounds("settings-shortcuts-page").is_some());
    let first_pill = cx.debug_bounds("shortcut-action-0").unwrap();
    let reset_btn = cx.debug_bounds("shortcuts-reset-all").unwrap();

    // Clicking an action pill enters recording mode and shows the rule hint bar.
    cx.simulate_click(first_pill.center(), gpui::Modifiers::default());
    cx.update(|_, cx| {
        assert_eq!(
            settings.read(cx).shortcuts.recording,
            Some(shortcuts_settings_view::RecordingTarget::Action(0))
        );
        assert!(!settings.read(cx).shortcuts.rejected);
    });
    assert!(cx.debug_bounds("shortcuts-recording-hint").is_some());

    // Pressing a bare letter on a Workspace-scoped shortcut flags rejection without closing.
    cx.simulate_keystrokes("a");
    cx.update(|_, cx| {
        assert_eq!(
            settings.read(cx).shortcuts.recording,
            Some(shortcuts_settings_view::RecordingTarget::Action(0))
        );
        assert!(settings.read(cx).shortcuts.rejected);
    });

    // Escape cancels recording and clears the rejection state without closing Settings.
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| {
        assert_eq!(settings.read(cx).shortcuts.recording, None);
        assert!(!settings.read(cx).shortcuts.rejected);
        assert!(!settings.read(cx).closing);
    });
    assert!(cx.debug_bounds("shortcuts-recording-hint").is_none());

    // Clicking Reset opens the confirmation modal; Escape dismisses it without closing Settings.
    cx.simulate_click(reset_btn.center(), gpui::Modifiers::default());
    cx.update(|_, cx| {
        assert!(matches!(
            settings.read(cx).confirm,
            Some(ActiveConfirm::ResetShortcuts)
        ));
    });
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| {
        assert!(settings.read(cx).confirm.is_none());
        assert!(!settings.read(cx).closing);
    });

    // Keymap parse errors surface cleanly in the banner at the top of Page::Shortcuts.
    cx.update(|_, cx| {
        desktop_common::keymap::set_errors(vec!["keymap.json: test error".to_owned()], cx);
        settings.update(cx, |_, cx| cx.notify());
    });
    assert!(cx.debug_bounds("shortcuts-keymap-errors").is_some());
}

#[test]
fn format_bytes_produces_human_readable_units() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(1024), "1 KB");
    assert_eq!(format_bytes(127_000), "124 KB");
    assert_eq!(format_bytes(1_468_006), "1.4 MB");
    assert_eq!(format_bytes(2_147_483_648), "2.0 GB");
}

#[gpui::test]
fn sync_page_saves_and_reloads_e2ee_passphrase(cx: &mut gpui::TestAppContext) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        assert!(
            desktop_common::keymap::load_default(&assets::Assets, cx)
                .unwrap()
                .is_empty()
        );
        cx.set_global(CurrentLocale(Locale::En));
    });
    let (settings, cx) = cx.add_window_view(|window, cx| {
        let mut settings = SettingsWindow::new(store.clone(), SettingsHost::noop(cx), cx);
        settings.set_page(Page::Sync, cx);
        window.focus(&settings.focus_handle, cx);
        settings
    });
    cx.simulate_resize(gpui::size(px(900.), px(720.)));
    cx.run_until_parked();

    assert!(cx.debug_bounds("sync-server-input").is_some());
    assert!(cx.debug_bounds("sync-token-input").is_some());
    let pass_bounds = cx.debug_bounds("sync-passphrase-input").unwrap();

    // Populate server and token, then enter passphrase via real user input + Enter submit.
    settings.update(cx, |s, cx| {
        s.sync
            .server
            .update(cx, |i, cx| i.set_text("https://sync.example.org", cx));
        s.sync.token.update(cx, |i, cx| i.set_text("tok-123", cx));
    });
    cx.simulate_click(pass_bounds.center(), gpui::Modifiers::default());
    cx.simulate_input("correct-horse-battery-staple");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    let saved = store.sync_config().unwrap().expect("sync config must be saved");
    assert_eq!(saved.server, "https://sync.example.org");
    assert_eq!(saved.token, "tok-123");
    assert_eq!(saved.passphrase, "correct-horse-battery-staple");

    // Reopening SettingsWindow reloads the persisted E2EE passphrase.
    let reloaded = cx.update(|_, cx| cx.new(|cx| SettingsWindow::new(store.clone(), SettingsHost::noop(cx), cx)));
    cx.update(|_, cx| {
        assert_eq!(
            reloaded.read(cx).sync.passphrase.read(cx).text(),
            "correct-horse-battery-staple"
        );
    });
}

#[gpui::test]
fn about_page_renders_auto_backups_db_health_and_restore_confirmation(
    cx: &mut gpui::TestAppContext,
) {
    let tmp = std::env::temp_dir().join(format!("fern-settings-about-{}", uuid::Uuid::now_v7()));
    let backups_dir = tmp.join("backups");
    let restore_target = tmp.join("restore.db");
    std::fs::create_dir_all(&backups_dir).unwrap();

    let store = Rc::new(Store::open_in_memory().unwrap());
    store.add_task("Important task").unwrap();
    let older_backup = backups_dir.join("fern-2026-11-20.db");
    let newer_backup = backups_dir.join("fern-2026-11-21.db");
    store.backup_to(&older_backup, None).unwrap();
    store.backup_to(&newer_backup, None).unwrap();

    cx.update(|cx| {
        theme::init(cx);
        assert!(
            desktop_common::keymap::load_default(&assets::Assets, cx)
                .unwrap()
                .is_empty()
        );
        cx.set_global(CurrentLocale(Locale::En));
    });
    let (settings, cx) = cx.add_window_view(|window, cx| {
        let mut settings = SettingsWindow::new(store.clone(), SettingsHost::noop(cx), cx);
        settings.backups.backups_dir_override = Some(backups_dir.clone());
        settings.backups.restore_target_override = Some(restore_target.clone());
        settings.set_page(Page::About, cx);
        window.focus(&settings.focus_handle, cx);
        settings
    });
    cx.simulate_resize(gpui::size(px(900.), px(780.)));
    cx.run_until_parked();

    // Auto-backups list, DB integrity row, and Save Diagnostic Log button render cleanly.
    assert!(cx.debug_bounds("settings-auto-backups").is_some());
    assert!(cx.debug_bounds("auto-backup-0").is_some());
    assert!(cx.debug_bounds("auto-backup-1").is_some());
    assert!(cx.debug_bounds("settings-db-integrity").is_some());
    assert!(cx.debug_bounds("settings-db-integrity-detail").is_none());
    assert!(cx.debug_bounds("save-diagnostic-log").is_some());

    // Unhealthy DbHealth surfaces warning detail.
    cx.update(|_, cx| {
        cx.set_global(desktop_common::diagnostics::DbHealth {
            healthy: false,
            messages: vec!["corrupt database preserved as tasks.db.corrupt-20261121-101500".to_owned()],
            recovered_corrupt_backup: Some(tmp.join("tasks.db.corrupt-20261121-101500")),
        });
        settings.update(cx, |_, cx| cx.notify());
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-db-integrity-detail").is_some());

    // Clicking Restore on the newest auto-backup opens the confirmation modal.
    let restore_btn = cx.debug_bounds("auto-backup-restore-0").unwrap();
    cx.simulate_click(restore_btn.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(matches!(
            &settings.read(cx).confirm,
            Some(ActiveConfirm::RestoreBackup(path)) if path == &newer_backup
        ));
    });
    assert!(cx.debug_bounds("confirm-restore-backup").is_some());

    // Pressing Escape dismisses confirmation without closing Settings.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(settings.read(cx).confirm.is_none());
        assert!(!settings.read(cx).closing);
    });
    assert!(cx.debug_bounds("confirm-restore-backup").is_none());

    // Clicking Restore again and confirming stages restore.db off the UI thread.
    let restore_btn = cx.debug_bounds("auto-backup-restore-0").unwrap();
    cx.simulate_click(restore_btn.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    settings.update(cx, |s, cx| s.confirm_restore(cx));
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(settings.read(cx).confirm.is_none());
        assert_eq!(
            settings.read(cx).backups.message.as_deref(),
            Some(i18n::EN.restore_on_restart)
        );
    });
    assert!(restore_target.exists(), "confirmed restore must stage restore.db");
    Store::check_backup(&restore_target).unwrap();

    let _ = std::fs::remove_dir_all(&tmp);
}

#[gpui::test]
fn features_page_prevents_disabling_last_task_frontend_and_plugins_page_renders(
    cx: &mut gpui::TestAppContext,
) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        cx.set_global(CurrentLocale(Locale::En));
        let _ = desktop_common::keymap::reload(&assets::Assets, cx);
    });
    let (settings, cx) = cx.add_window_view(|window, cx| {
        let mut settings = SettingsWindow::new(store.clone(), SettingsHost::noop(cx), cx);
        settings.set_page(Page::Features, cx);
        window.focus(&settings.focus_handle, cx);
        settings
    });
    cx.simulate_resize(gpui::size(px(900.), px(780.)));
    cx.run_until_parked();

    // Initially Tasks is on and Things is off: clicking Tasks must keep it on.
    let tasks_btn = cx.debug_bounds("settings-feature-tasks").unwrap();
    cx.simulate_click(tasks_btn.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let features = settings.read(cx).preferences.features;
        assert!(features.tasks);
        assert!(!features.things);
        assert!(features.has_any_task_frontend());
    });

    // Turn Things on, then Tasks can be turned off cleanly.
    let things_btn = cx.debug_bounds("settings-feature-things").unwrap();
    cx.simulate_click(things_btn.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let tasks_btn = cx.debug_bounds("settings-feature-tasks").unwrap();
    cx.simulate_click(tasks_btn.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let features = settings.read(cx).preferences.features;
        assert!(!features.tasks);
        assert!(features.things);
    });

    // Now Things is the last enabled frontend: clicking Things keeps it on.
    let things_btn = cx.debug_bounds("settings-feature-things").unwrap();
    cx.simulate_click(things_btn.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let features = settings.read(cx).preferences.features;
        assert!(!features.tasks);
        assert!(features.things);
    });

    // Switching to Page::Plugins renders the hosted plugin settings view cleanly.
    let plugins_nav = cx.debug_bounds("settings-plugins");
    if let Some(bounds) = plugins_nav {
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
    } else {
        settings.update(cx, |s, cx| s.set_page(Page::Plugins, cx));
    }
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(settings.read(cx).page, Page::Plugins));
}

#[gpui::test]
fn subscription_delete_confirms_locale_refreshes_placeholder_and_select_scrolls(
    cx: &mut gpui::TestAppContext,
) {
    let store = Rc::new(Store::open_in_memory().unwrap());
    cx.update(|cx| {
        theme::init(cx);
        assert!(
            desktop_common::keymap::load_default(&assets::Assets, cx)
                .unwrap()
                .is_empty()
        );
        cx.set_global(CurrentLocale(Locale::En));
        subscriptions::init(store.clone(), cx);
    });
    let (settings, cx) = cx.add_window_view(|window, cx| {
        let mut settings = SettingsWindow::new(store.clone(), SettingsHost::noop(cx), cx);
        settings.attach_window(window, cx);
        settings.set_page(Page::Integrations, cx);
        window.focus(&settings.focus_handle, cx);
        settings
    });
    cx.simulate_resize(gpui::size(px(800.), px(400.)));
    cx.run_until_parked();

    // Add a subscription directly, then test Trash confirmation + Escape + Confirm.
    cx.update(|_, cx| {
        subscriptions::feeds(cx).unwrap().update(cx, |feeds, cx| {
            feeds.add("Team Calendar", "https://example.com/team.ics", None, cx);
        });
    });
    cx.run_until_parked();
    let sub_id = store.subscriptions().unwrap()[0].id;
    let del_sel: &'static str = Box::leak(format!("subscription-delete-{sub_id}").into_boxed_str());
    let del_btn = cx.debug_bounds(del_sel).unwrap().center();
    cx.simulate_click(del_btn, gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("confirm-delete-subscription").is_some());
    assert_eq!(store.subscriptions().unwrap().len(), 1);

    // Escape cancels deletion without closing Settings.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("confirm-delete-subscription").is_none());
    assert_eq!(store.subscriptions().unwrap().len(), 1);
    cx.update(|_, cx| assert!(!settings.read(cx).is_closing()));

    // Confirming deletes the subscription.
    let del_btn = cx.debug_bounds(del_sel).unwrap().center();
    cx.simulate_click(del_btn, gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("confirm-delete-subscription").is_some());
    settings.update(cx, |s, cx| s.confirm_delete_subscription(cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("confirm-delete-subscription").is_none());
    assert!(store.subscriptions().unwrap().is_empty());

    // Switching locale updates window & subscription_name placeholder cleanly.
    cx.update(|window, cx| {
        settings.update(cx, |s, cx| {
            s.open_subscription_form(None, window, cx);
            s.set_locale(Locale::Ru, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(i18n::locale(cx), Locale::Ru);
        settings.update(cx, |s, _| s.subs.form = None);
    });

    // 19-row DailyTime select popover caps height in a 400px-tall Settings window.
    cx.update(|window, cx| {
        settings.update(cx, |s, cx| {
            s.update(window, cx, |p| {
                p.notifications.daily = chrono::NaiveTime::from_hms_opt(9, 0, 0);
            });
            s.set_page(Page::Notifications, cx);
            s.set_menu(Some(Menu::DailyTime), cx);
        });
    });
    cx.run_until_parked();
    let row = cx.debug_bounds("settings-row-DailyTime").unwrap();
    let pop = cx.debug_bounds("settings-select-popover-DailyTime").unwrap();
    assert!(row.bottom() <= px(400.));
    assert!(pop.size.height <= px(280.), "popover must stay bounded: {pop:?}");
}
