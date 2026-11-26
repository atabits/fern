// A release build on Windows opens no console window next to the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(test)]
mod feature_regressions;
mod app_view;
#[cfg(debug_assertions)]
mod dev_state;

pub(crate) use desktop_common::{
    DbPath, actions, calendar, keymap, portable, release_notes, text_input, touch, user_themes,
};
pub(crate) use desktop_sync::{LiveSyncStatus, subscriptions, sync};
pub(crate) use desktop_common::paths::{
    DAILY_BACKUPS, attachments_dir, backups_dir, data_dir,
};
pub(crate) use task_ui::{
    lists_panel, search_modal, task_detail, task_rows,
};
pub(crate) use calendar_ui as calendar_view;
pub(crate) use countdown_ui as countdown_view;
pub(crate) use focus_ui as focus_view;
pub(crate) use habit_ui as habit_view;
pub(crate) use life_clock_ui as life_clock_view;
pub(crate) use matrix_ui as matrix_view;
pub(crate) use sticky_ui as sticky_note;
pub(crate) use todo_ui as todo_view;
mod hotkeys;
mod http;
pub(crate) use fern_i18n as i18n;
mod quick_add_window;
mod reminders;
mod settings_window;
mod shutdown;
mod sidebar;
#[cfg(any(windows, unix))]
mod single_instance;
mod title_bar;
mod tray;
mod updater;

use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result};
use fern_core::Store;
use gpui::{App, WindowOptions, prelude::*, px, size};

use crate::app_view::AppView;
use assets::Assets;
use crate::i18n::{CurrentLocale, Locale};

fn preserve_corrupt_database(
    dir: &Path,
    path: &Path,
    open_err: &anyhow::Error,
) -> Result<desktop_common::diagnostics::DbHealth> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let corrupt_name = format!("tasks.db.corrupt-{stamp}");
    let corrupt_path = dir.join(&corrupt_name);
    desktop_common::diagnostics::log_event(
        "db",
        format!("failed to open database ({open_err:#}); preserving as {corrupt_name}"),
    );
    std::fs::rename(path, &corrupt_path).with_context(|| {
        format!(
            "cannot preserve corrupt database {} as {}",
            path.display(),
            corrupt_path.display()
        )
    })?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = dir.join(format!("tasks.db{suffix}"));
        if sidecar.exists() {
            let target = dir.join(format!("{corrupt_name}{suffix}"));
            if let Err(err) = std::fs::rename(&sidecar, &target) {
                desktop_common::diagnostics::log_event(
                    "db",
                    format!("failed to preserve tasks.db{suffix}: {err}"),
                );
            }
        }
    }
    Ok(desktop_common::diagnostics::DbHealth {
        healthy: false,
        messages: vec![format!("corrupt database preserved as {corrupt_name}")],
        recovered_corrupt_backup: Some(corrupt_path),
    })
}

/// Opens the data, whether it was just created (first start), its path, and startup DB health.
fn open_store(dir: &Path) -> Result<(Store, bool, PathBuf, desktop_common::diagnostics::DbHealth)> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join("tasks.db");
    let first_start = !path.exists();
    let (mut store, recovered_health) = match Store::open(&path) {
        Ok(store) => (store, None),
        Err(open_err) if path.exists() => {
            let health = preserve_corrupt_database(dir, &path, &open_err)?;
            let fresh = Store::open(&path)
                .context("failed to open fresh database after preserving corrupt database")?;
            (fresh, Some(health))
        }
        Err(err) => return Err(err),
    };
    let pending = dir.join("restore.db");
    if pending.exists() {
        let attachments = dir.join("attachments");
        match store.restore_from(&pending, Some(&attachments)) {
            Ok(()) => {
                desktop_common::logging::log_info("db", "restored pending backup on startup");
            }
            Err(err) => {
                desktop_common::logging::log_error("db", format!("failed to restore pending backup: {err:#}"));
            }
        }
        std::fs::remove_file(&pending).ok();
    }
    let db_health = match recovered_health {
        Some(health) => health,
        None => match store.quick_check() {
            Ok(msgs) if msgs.len() == 1 && msgs[0].eq_ignore_ascii_case("ok") => {
                desktop_common::diagnostics::DbHealth {
                    healthy: true,
                    messages: msgs,
                    recovered_corrupt_backup: None,
                }
            }
            Ok(msgs) => {
                if msgs.is_empty() {
                    desktop_common::diagnostics::log_event("db", "quick check returned no rows");
                } else {
                    for msg in &msgs {
                        desktop_common::diagnostics::log_event("db", format!("quick check: {msg}"));
                    }
                }
                desktop_common::diagnostics::DbHealth {
                    healthy: false,
                    messages: msgs,
                    recovered_corrupt_backup: None,
                }
            }
            Err(err) => {
                let msg = format!("quick check failed: {err:#}");
                desktop_common::diagnostics::log_event("db", &msg);
                desktop_common::diagnostics::DbHealth {
                    healthy: false,
                    messages: vec![msg],
                    recovered_corrupt_backup: None,
                }
            }
        },
    };
    Ok((store, first_start, path, db_health))
}

/// The first start fills the Inbox with a few tips, the first one on top.
fn add_welcome_tasks(store: &Store, strings: &i18n::Strings) -> Result<()> {
    for (title, notes) in strings.welcome_tasks.iter().rev() {
        let task = store.add_task(title)?;
        store.set_notes(task.id, notes)?;
    }
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CliOptions {
    show_version: bool,
    show_help: bool,
    safe_mode: bool,
    quick_add: bool,
    toggle: bool,
    focus_mini: bool,
    deep_link: Option<fern_core::link::DeepLink>,
    deep_link_raw: Option<String>,
}

impl CliOptions {
    #[cfg(any(windows, unix))]
    fn ipc_command(&self) -> single_instance::IpcCommand {
        if let Some(uri) = &self.deep_link_raw {
            single_instance::IpcCommand::DeepLink(uri.clone())
        } else if self.quick_add {
            single_instance::IpcCommand::QuickAdd
        } else if self.toggle {
            single_instance::IpcCommand::Toggle
        } else if self.focus_mini {
            single_instance::IpcCommand::FocusMini
        } else {
            single_instance::IpcCommand::Activate
        }
    }
}

fn parse_cli_args<I, S>(args: I) -> CliOptions
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = CliOptions::default();
    for arg in args {
        let s = arg.as_ref();
        match s {
            "--version" | "-V" => out.show_version = true,
            "--help" | "-h" => out.show_help = true,
            "--safe-mode" => out.safe_mode = true,
            "--quick-add" => out.quick_add = true,
            "--toggle" => out.toggle = true,
            "--focus-mini" => out.focus_mini = true,
            _ if out.deep_link.is_none() => {
                if let Some(link) = fern_core::link::parse_deep_link(s) {
                    out.deep_link = Some(link);
                    out.deep_link_raw = Some(s.to_owned());
                }
            }
            _ => {}
        }
    }
    out
}

fn print_help() {
    println!(
        "Fern {} — Tasks, calendar, habits and focus in one native app\n\n\
         USAGE:\n\
         \x20   fern [OPTIONS] [DEEP_LINK]\n\n\
         OPTIONS:\n\
         \x20   --quick-add     Open the Quick Add popup (forwards to running instance)\n\
         \x20   --toggle        Show or hide the main Fern window (forwards to running instance)\n\
         \x20   --focus-mini    Toggle the floating Focus timer mini window\n\
         \x20   --safe-mode     Start with plugins disabled\n\
         \x20   -V, --version   Print version information and exit\n\
         \x20   -h, --help      Print this help message and exit\n\n\
         WAYLAND SHORTCUTS:\n\
         \x20   Bind compositor shortcuts directly to `fern --quick-add`, `fern --toggle`, or `fern --focus-mini`.",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_open_urls<I, S>(urls: I) -> Vec<fern_core::link::DeepLink>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    urls.into_iter()
        .filter_map(|raw| fern_core::link::parse_deep_link(raw.as_ref()))
        .collect()
}

fn apply_initial_theme(selection: &fern_core::ThemeSelection, system_dark: bool, cx: &mut App) {
    let dark = selection.mode.is_dark(system_dark);
    let resolved = user_themes::remembered(selection, dark, cx);
    theme::set_arc(resolved, cx);
}

fn init_plugin_host(safe_mode: bool, db_path: PathBuf, locale: &str, cx: &mut App) {
    use theme::ActiveTheme as _;
    user_themes::set_plugin_themes_provider(
        |cx| {
            let plugins = plugin_ui::PluginHost::global(cx)?.plugins_arc();
            Some(Box::new(move || plugin_ui::collect_plugin_themes(&plugins)))
        },
        cx,
    );
    let appearance = if cx.theme().is_dark() { "dark" } else { "light" };
    match plugin_ui::PluginHostConfig::from_desktop_paths(
        Some(db_path),
        safe_mode,
        locale,
        appearance,
        true,
    ) {
        Ok(config) => {
            if let Err(err) = plugin_ui::PluginHost::init(config, cx) {
                desktop_common::logging::log_error("plugin", format!("failed to initialize plugin host: {err:#}"));
            }
        }
        Err(err) => desktop_common::logging::log_error("plugin", format!("failed to configure plugin host: {err:#}")),
    }
}

fn main() -> Result<()> {
    // `fern fern://task/<id>` or `fern fern://plugin/<id>/<cmd>` handles deep links; `--safe-mode` disables plugins.
    let cli = parse_cli_args(std::env::args().skip(1));
    if cli.show_version {
        println!("fern {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if cli.show_help {
        print_help();
        return Ok(());
    }
    let dir = data_dir()?;
    #[cfg(any(windows, unix))]
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut instance_guard = match single_instance::acquire(&dir, cli.ipc_command())? {
        Some(guard) => guard,
        None => return Ok(()),
    };
    #[cfg(unix)]
    let ipc_rx = instance_guard.take_receiver();

    let (store, first_start, db_path, db_health) = open_store(&dir)?;
    let store = Rc::new(store);
    let safe_mode_setting = store
        .setting("plugins_safe_mode")?
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let safe_mode = cli.safe_mode || safe_mode_setting;
    let initial_link = cli.deep_link;
    let plugin_db_path = db_path.clone();
    let locale = store
        .setting("locale")?
        .and_then(|code| Locale::from_code(&code))
        .unwrap_or_else(Locale::system);
    if first_start && let Err(err) = add_welcome_tasks(&store, locale.strings()) {
        desktop_common::logging::log_error("app", format!("failed to add the welcome tasks: {err:#}"));
    }

    let platform = gpui_platform::current_platform(false);
    let text_system = platform.text_system();
    let app = gpui::Application::with_platform(platform).with_assets(Assets);
    app.on_reopen(|_cx| {
        tray::restore_main_window();
    });
    #[cfg(target_os = "macos")]
    let (open_urls_tx, open_urls_rx) = futures::channel::mpsc::unbounded::<Vec<String>>();
    #[cfg(target_os = "macos")]
    app.on_open_urls(move |urls| {
        let _ = open_urls_tx.unbounded_send(urls);
    });
    app.run(move |cx: &mut App| {
        let _keep_guard = &instance_guard;
        desktop_common::RuntimeConfig::init(cx);
        cx.on_app_quit(|cx| {
            crate::shutdown::AppShutdown::run_once(cx);
            async {}
        })
        .detach();
        cx.set_global(DbPath(db_path));
        cx.set_global(db_health);
        cx.set_global(LiveSyncStatus::default());
        cx.set_global(desktop_common::sync::LiveCloudSnapshot::default());
        ui::init_label_metrics(text_system, cx);
        theme::init(cx);
        user_themes::load(cx);
        let initial_prefs = store.preferences().unwrap_or_default();
        desktop_common::ActivePreferences::sync_from_preferences(
            &initial_prefs,
            desktop_common::sound::enabled(&store),
            cx,
        );
        let system_dark = matches!(
            cx.window_appearance(),
            gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
        );
        apply_initial_theme(&initial_prefs.theme, system_dark, cx);
        cx.set_http_client(http::UreqHttpClient::new());
        desktop_common::cloud_vault::bootstrap_os_keychain(cx);
        if let Err(err) = Assets.load_fonts(cx) {
            desktop_common::logging::log_error("app", format!("failed to load the fonts: {err:#}"));
        }
        cx.set_global(CurrentLocale(locale));
        // Initializes PluginHost and registers desktop_common's PluginHooksGlobal bridge.
        init_plugin_host(safe_mode, plugin_db_path, locale.code(), cx);
        match keymap::reload(&Assets, cx) {
            Ok(errors) => errors
                .iter()
                .for_each(|err| desktop_common::logging::log_warn("keymap", err)),
            Err(err) => desktop_common::logging::log_error("keymap", format!("failed to load the keymap: {err:#}")),
        }
        updater::init(store.clone(), cx);
        subscriptions::init(store.clone(), cx);

        let window_bounds = desktop_common::window_state::initial_window_bounds(&store, cx);
        let (titlebar, decorations) = title_bar::window_options();
        let store_for_hotkeys = store.clone();
        let store_for_tray = store.clone();
        let store_for_themes = store.clone();
        let store_for_maint = store.clone();
        // Prepare the tray channel before creating the window/icon so early clicks are never dropped.
        let tray_rx = tray::prepare_channel();
        let main_window = cx.open_window(
            WindowOptions {
                window_bounds: Some(window_bounds),
                titlebar: Some(titlebar),
                window_decorations: Some(decorations),
                // macOS: the app moves the window from its own title bar.
                app_owns_titlebar_drag: true,
                // Overridable so that window-manager rules can target dev builds (used for UI screenshots).
                app_id: Some(desktop_common::app_id()),
                window_min_size: Some(size(
                    px(desktop_common::window_state::MIN_WIDTH),
                    px(desktop_common::window_state::MIN_HEIGHT),
                )),
                ..Default::default()
            },
            |window, cx| {
                touch::init(window, cx);
                tray::init(window, &store, cx);
                cx.new(|cx| {
                    let mut view =
                        AppView::new_with_preferences(store, initial_prefs, first_start, window, cx);
                    if let Some(link) = initial_link {
                        view.open_deep_link(link, window, cx);
                    }
                    view
                })
            },
        )
        .expect("failed to open the main window");
        // Fern keeps the data on this device only, so it saves a copy of it once a day (but not
        // the throwaway data of the debug build's screenshots). Run off the UI thread after the
        // main window is open so cold startup stays instant even with a large tasks.db.
        if !desktop_common::is_dev_state() {
            let maint_task = desktop_common::DbRunner::run_global(&store_for_maint, cx, |maint_store| {
                // The sync log keeps one entry per row; without a server it would grow with every edit.
                if let Err(err) = maint_store.compact_sync_log() {
                    desktop_common::logging::log_error("db", format!("failed to compact the sync log: {err:#}"));
                }
                let today = chrono::Local::now().date_naive();
                backups_dir().and_then(|dir| maint_store.daily_backup(&dir, today, DAILY_BACKUPS))
            });
            cx.spawn(async move |cx| {
                if let Err(err) = maint_task.await {
                    desktop_common::logging::log_error("db", format!("failed to back up the data: {err:#}"));
                }
                let _ = cx;
            })
            .detach();
        }
        if !safe_mode
            && let Some(mgr) = plugin_ui::PluginHost::global(cx).and_then(|h| h.manager().cloned())
        {
            cx.background_spawn(async move {
                if let Err(err) = mgr.emit_app_ready() {
                    desktop_common::logging::log_error("plugin", format!("failed to emit app.ready to plugins: {err:#}"));
                }
            })
            .detach();
        }
        #[cfg(target_os = "macos")]
        cx.spawn(async move |cx| {
            use futures::StreamExt as _;
            let mut open_urls_rx = open_urls_rx;
            while let Some(urls) = open_urls_rx.next().await {
                let links = parse_open_urls(urls);
                if links.is_empty() {
                    continue;
                }
                let _ = main_window.update(cx, |view, window, cx| {
                    tray::restore_main_window();
                    window.activate_window();
                    for link in links {
                        view.open_deep_link(link, window, cx);
                    }
                });
            }
        })
        .detach();
        #[cfg(unix)]
        if let Some(mut ipc_rx) = ipc_rx {
            cx.spawn(async move |cx| {
                use futures::StreamExt as _;
                while let Some(cmd) = ipc_rx.next().await {
                    handle_ipc_command(cmd, main_window, cx);
                }
            })
            .detach();
        }
        tray::connect(tray_rx, main_window, store_for_tray, cx);
        AppView::register_window_actions(main_window, cx);
        updater::set_before_restart_hook(
            move |cx| {
                let _ = main_window.update(cx, |view, window, cx| {
                    view.prepare_for_restart(window, cx);
                });
            },
            cx,
        );
        user_themes::watch(store_for_themes, main_window.into(), cx);
        cx.set_global(desktop_common::StickyOpener(Rc::new(sticky_note::open)));
        sticky_note::restore(store_for_hotkeys.clone(), cx);
        hotkeys::init(main_window, store_for_hotkeys, cx);
        if cli.quick_add {
            quick_add_window::open(main_window, cx);
        }
        if cli.focus_mini {
            let _ = main_window.update(cx, |view, _, cx| view.toggle_focus_mini(cx));
        }
        cx.activate(true);
    });
    Ok(())
}

#[cfg(any(windows, unix))]
pub(crate) fn handle_ipc_command(
    cmd: single_instance::IpcCommand,
    main_window: gpui::WindowHandle<AppView>,
    cx: &mut gpui::AsyncApp,
) {
    match cmd {
        single_instance::IpcCommand::Activate => {
            let _ = main_window.update(cx, |_, window, _| {
                tray::restore_main_window();
                window.activate_window();
            });
        }
        single_instance::IpcCommand::QuickAdd => {
            cx.update(|cx| quick_add_window::open(main_window, cx));
        }
        single_instance::IpcCommand::Toggle => {
            let _ = main_window.update(cx, |_, window, _| {
                if window.is_window_active() {
                    if !tray::hide_to_tray() {
                        window.minimize_window();
                    }
                } else {
                    tray::restore_main_window();
                    window.activate_window();
                }
            });
        }
        single_instance::IpcCommand::FocusMini => {
            let _ = main_window.update(cx, |view, _, cx| view.toggle_focus_mini(cx));
        }
        single_instance::IpcCommand::DeepLink(raw) => {
            if let Some(link) = fern_core::link::parse_deep_link(&raw) {
                let _ = main_window.update(cx, |view, window, cx| {
                    tray::restore_main_window();
                    window.activate_window();
                    view.open_deep_link(link, window, cx);
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_store_reports_healthy_db_on_fresh_and_existing_store() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("fern-open-healthy-{}", uuid::Uuid::now_v7()));
        let (store, first_start, db_path, health) = open_store(&dir)?;
        assert!(first_start);
        assert_eq!(db_path, dir.join("tasks.db"));
        assert!(health.healthy);
        assert_eq!(health.messages, vec!["ok"]);
        assert!(health.recovered_corrupt_backup.is_none());
        store.add_task("Keep me")?;
        drop(store);

        let (reopened, first_start2, _, health2) = open_store(&dir)?;
        assert!(!first_start2);
        assert!(health2.healthy);
        assert_eq!(reopened.tasks()?.len(), 1);
        drop(reopened);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn open_store_preserves_corrupt_db_then_opens_fresh_store() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("fern-open-corrupt-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir)?;
        let db = dir.join("tasks.db");
        std::fs::write(&db, b"definitely not a valid sqlite database file")?;

        let (store, first_start, db_path, health) = open_store(&dir)?;
        assert!(!first_start);
        assert_eq!(db_path, db);
        assert!(!health.healthy);
        let preserved = health
            .recovered_corrupt_backup
            .as_ref()
            .expect("corrupt backup path must be recorded");
        let preserved_name = preserved
            .file_name()
            .and_then(|n| n.to_str())
            .expect("valid utf-8 filename");
        assert!(preserved_name.starts_with("tasks.db.corrupt-"), "unexpected name: {preserved_name}");
        assert_eq!(
            std::fs::read(preserved).with_context(|| format!("reading {}", preserved.display()))?,
            b"definitely not a valid sqlite database file"
        );
        assert!(
            health
                .messages
                .iter()
                .any(|m| m.contains(preserved_name)),
            "expected preserved filename in health messages: {:?}",
            health.messages
        );

        // Fresh store must be healthy and writable.
        assert_eq!(store.integrity_check()?, vec!["ok"]);
        store.add_task("Fresh start after recovery")?;
        assert_eq!(store.tasks()?.len(), 1);

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn preserve_corrupt_database_moves_wal_and_shm_sidecars_alongside_db() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("fern-preserve-sidecars-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir)?;
        let db = dir.join("tasks.db");
        let wal = dir.join("tasks.db-wal");
        let shm = dir.join("tasks.db-shm");
        std::fs::write(&db, b"corrupt main db")?;
        std::fs::write(&wal, b"corrupt wal bytes")?;
        std::fs::write(&shm, b"corrupt shm bytes")?;

        let health = preserve_corrupt_database(&dir, &db, &anyhow::anyhow!("corrupt page"))?;
        assert!(!health.healthy);
        let preserved = health
            .recovered_corrupt_backup
            .as_ref()
            .expect("corrupt backup path must be recorded");
        let preserved_name = preserved
            .file_name()
            .and_then(|n| n.to_str())
            .expect("valid utf-8 filename");
        assert!(preserved_name.starts_with("tasks.db.corrupt-"));
        assert!(!db.exists());
        assert!(!wal.exists());
        assert!(!shm.exists());
        assert_eq!(std::fs::read(preserved)?, b"corrupt main db");
        assert_eq!(std::fs::read(dir.join(format!("{preserved_name}-wal")))?, b"corrupt wal bytes");
        assert_eq!(std::fs::read(dir.join(format!("{preserved_name}-shm")))?, b"corrupt shm bytes");

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn parse_cli_args_handles_safe_mode_and_deep_links() {
        let task_id = uuid::Uuid::now_v7();
        let task_uri = fern_core::link::task_link(task_id);
        let parsed = parse_cli_args(["--unknown", "--safe-mode", &task_uri]);
        assert!(parsed.safe_mode);
        assert_eq!(parsed.deep_link, Some(fern_core::link::DeepLink::Task(task_id)));
        assert_eq!(parsed.deep_link_raw.as_deref(), Some(task_uri.as_str()));

        let plugin_uri = "fern://plugin/pomodoro/start?minutes=25";
        let plugin_parsed = parse_cli_args([plugin_uri]);
        assert!(!plugin_parsed.safe_mode);
        assert_eq!(
            plugin_parsed.deep_link,
            Some(fern_core::link::DeepLink::Plugin {
                plugin_id: "pomodoro".into(),
                command: "start".into(),
                params: vec![("minutes".into(), "25".into())],
            })
        );
        assert_eq!(plugin_parsed.deep_link_raw.as_deref(), Some(plugin_uri));

        let flags = parse_cli_args(["--quick-add", "--toggle", "--focus-mini", "-V", "-h"]);
        assert!(flags.quick_add);
        assert!(flags.toggle);
        assert!(flags.focus_mini);
        assert!(flags.show_version);
        assert!(flags.show_help);
    }

    #[test]
    fn parse_open_urls_extracts_all_valid_deep_links() {
        let task_id = uuid::Uuid::now_v7();
        let task_uri = fern_core::link::task_link(task_id);
        let links = parse_open_urls([
            "https://example.com/ignored",
            task_uri.as_str(),
            "fern://plugin/sync/now?force=1",
        ]);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0], fern_core::link::DeepLink::Task(task_id));
        assert_eq!(
            links[1],
            fern_core::link::DeepLink::Plugin {
                plugin_id: "sync".into(),
                command: "now".into(),
                params: vec![("force".into(), "1".into())],
            }
        );
    }

    #[gpui::test]
    fn apply_initial_theme_resolves_dark_and_light_before_plugin_host(cx: &mut gpui::TestAppContext) {
        use theme::ActiveTheme as _;

        cx.update(|cx| {
            theme::init(cx);
            let dark_sel = fern_core::ThemeSelection {
                mode: fern_core::ThemeMode::Dark,
                ..Default::default()
            };
            apply_initial_theme(&dark_sel, false, cx);
            assert!(cx.theme().is_dark(), "explicit Dark mode must resolve dark at startup");

            let system_sel = fern_core::ThemeSelection {
                mode: fern_core::ThemeMode::System,
                ..Default::default()
            };
            apply_initial_theme(&system_sel, true, cx);
            assert!(cx.theme().is_dark(), "System mode with dark OS must resolve dark");
            apply_initial_theme(&system_sel, false, cx);
            assert!(!cx.theme().is_dark(), "System mode with light OS must resolve light");
        });
    }
}
