// A release build on Windows opens no console window next to the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(test)]
mod feature_regressions;
mod app_view;
#[cfg(debug_assertions)]
mod dev_state;

pub(crate) use desktop_common::{
    actions, calendar, export, keymap, portable, subscriptions, sync, text_input, touch,
};
pub(crate) use desktop_common::paths::{
    DAILY_BACKUPS, attachments_dir, backups_dir, data_dir, pending_restore_path, scratch_dir,
};
pub(crate) use task_ui::{
    lists_panel, task_detail, task_rows,
};
pub(crate) use calendar_ui as calendar_view;
pub(crate) use countdown_ui as countdown_view;
pub(crate) use focus_ui as focus_view;
pub(crate) use habit_ui as habit_view;
pub(crate) use matrix_ui as matrix_view;
pub(crate) use sticky_ui as sticky_note;
pub(crate) use todo_ui as todo_view;
mod hotkeys;
mod http;
pub(crate) use fern_i18n as i18n;
mod release_notes;
mod quick_add_window;
mod reminders;
mod search_modal;
mod settings_window;
mod sidebar;
#[cfg(windows)]
mod single_instance;
mod title_bar;
mod tray;
mod updater;
mod user_themes;

use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result};
use fern_core::Store;
use gpui::{App, Bounds, Global, WindowBounds, WindowOptions, prelude::*, px, size};

use crate::app_view::AppView;
use assets::Assets;
use crate::i18n::{CurrentLocale, Locale};

/// Path to the on-disk SQLite database so background workers can open a WAL connection
/// off the foreground UI thread.
pub(crate) struct DbPath(pub PathBuf);

impl Global for DbPath {}

/// Latest sync status, shared globally so Settings diagnostics reflect live sync state.
#[derive(Clone, Debug, Default)]
pub(crate) struct LiveSyncStatus(pub crate::sync::SyncStatus);

impl Global for LiveSyncStatus {}

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
                eprintln!("restored the backup");
                desktop_common::diagnostics::log_event("db", "restored pending backup on startup");
            }
            Err(err) => {
                eprintln!("failed to restore the backup: {err:#}");
                desktop_common::diagnostics::log_event(
                    "db",
                    format!("failed to restore pending backup: {err:#}"),
                );
            }
        }
        std::fs::remove_file(&pending).ok();
    }
    // The sync log keeps one entry per row; without a server it would grow with every edit.
    if let Err(err) = store.compact_sync_log() {
        eprintln!("failed to compact the sync log: {err:#}");
    }
    let db_health = match recovered_health {
        Some(health) => health,
        None => match store.integrity_check() {
            Ok(msgs) if msgs.len() == 1 && msgs[0].eq_ignore_ascii_case("ok") => {
                desktop_common::diagnostics::DbHealth {
                    healthy: true,
                    messages: msgs,
                    recovered_corrupt_backup: None,
                }
            }
            Ok(msgs) => {
                if msgs.is_empty() {
                    desktop_common::diagnostics::log_event("db", "integrity check returned no rows");
                } else {
                    for msg in &msgs {
                        desktop_common::diagnostics::log_event("db", format!("integrity check: {msg}"));
                    }
                }
                desktop_common::diagnostics::DbHealth {
                    healthy: false,
                    messages: msgs,
                    recovered_corrupt_backup: None,
                }
            }
            Err(err) => {
                let msg = format!("integrity check failed: {err:#}");
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
    safe_mode: bool,
    deep_link: Option<fern_core::link::DeepLink>,
    deep_link_raw: Option<String>,
}

fn parse_cli_args<I, S>(args: I) -> CliOptions
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = CliOptions::default();
    for arg in args {
        let s = arg.as_ref();
        if s == "--safe-mode" {
            out.safe_mode = true;
        } else if out.deep_link.is_none()
            && let Some(link) = fern_core::link::parse_deep_link(s)
        {
            out.deep_link = Some(link);
            out.deep_link_raw = Some(s.to_owned());
        }
    }
    out
}

fn init_plugin_host(safe_mode: bool, db_path: PathBuf, locale: &str, cx: &mut App) {
    use theme::ActiveTheme as _;
    if safe_mode {
        match (
            desktop_common::paths::plugins_dir(),
            desktop_common::paths::plugin_registry_cache_dir(),
        ) {
            (Ok(plugins_dir), Ok(registry_cache_dir)) => {
                if let Err(err) =
                    plugin_ui::PluginHost::init_safe_mode(plugins_dir, registry_cache_dir, cx)
                {
                    eprintln!("failed to initialize plugin host in safe mode: {err:#}");
                }
            }
            (Err(err), _) | (_, Err(err)) => {
                eprintln!("failed to resolve plugin directories: {err:#}");
            }
        }
        return;
    }
    let appearance = if cx.theme().is_dark() { "dark" } else { "light" };
    match plugin_ui::PluginHostConfig::from_desktop_paths(
        Some(db_path),
        false,
        locale,
        appearance,
        true,
    ) {
        Ok(config) => {
            if let Err(err) = plugin_ui::PluginHost::init(config, cx) {
                eprintln!("failed to initialize plugin host: {err:#}");
            } else {
                plugin_ui::PluginHost::sync_plugin_themes(cx);
            }
        }
        Err(err) => eprintln!("failed to configure plugin host: {err:#}"),
    }
}

fn main() -> Result<()> {
    // `fern fern://task/<id>` or `fern fern://plugin/<id>/<cmd>` handles deep links; `--safe-mode` disables plugins.
    let cli = parse_cli_args(std::env::args().skip(1));
    let dir = data_dir()?;
    #[cfg(windows)]
    let _instance_guard = match single_instance::acquire(&dir, cli.deep_link_raw.as_deref())? {
        Some(guard) => guard,
        None => return Ok(()),
    };

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
        eprintln!("failed to add the welcome tasks: {err:#}");
    }
    // Fern keeps the data on this device only, so it saves a copy of it once a day (but not
    // the throwaway data of the debug build's screenshots).
    if std::env::var_os("FERN_DEV_STATE").is_none()
        && let Err(err) = backups_dir().and_then(|dir| store.daily_backup(&dir, chrono::Local::now().date_naive(), DAILY_BACKUPS))
    {
        eprintln!("failed to back up the data: {err:#}");
    }

    let platform = gpui_platform::current_platform(false);
    let text_system = platform.text_system();
    let app = gpui::Application::with_platform(platform).with_assets(Assets);
    app.on_reopen(|_cx| {
        tray::restore_main_window();
    });
    app.run(move |cx: &mut App| {
        cx.set_global(DbPath(db_path));
        cx.set_global(db_health);
        cx.set_global(LiveSyncStatus::default());
        ui::init_label_metrics(text_system, cx);
        theme::init(cx);
        user_themes::load(cx);
        cx.set_http_client(http::UreqHttpClient::new());
        if let Err(err) = Assets.load_fonts(cx) {
            eprintln!("failed to load the fonts: {err:#}");
        }
        cx.set_global(CurrentLocale(locale));
        init_plugin_host(safe_mode, plugin_db_path, locale.code(), cx);
        match keymap::reload(&Assets, cx) {
            Ok(errors) => errors.iter().for_each(|err| eprintln!("keymap: {err}")),
            Err(err) => eprintln!("failed to load the keymap: {err:#}"),
        }
        updater::init(store.clone(), cx);
        subscriptions::init(store.clone(), cx);

        let bounds = Bounds::centered(None, size(px(1280.), px(760.)), cx);
        let (titlebar, decorations) = title_bar::window_options();
        let store_for_hotkeys = store.clone();
        let store_for_tray = store.clone();
        let store_for_themes = store.clone();
        // Prepare the tray channel before creating the window/icon so early clicks are never dropped.
        let tray_rx = tray::prepare_channel();
        let main_window = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(titlebar),
                window_decorations: Some(decorations),
                // macOS: the app moves the window from its own title bar.
                app_owns_titlebar_drag: true,
                // Overridable so that window-manager rules can target dev builds (used for UI screenshots).
                app_id: Some(std::env::var("FERN_APP_ID").unwrap_or_else(|_| "fern-tasks".into())),
                window_min_size: Some(size(px(900.), px(400.))),
                ..Default::default()
            },
            |window, cx| {
                touch::init(window, cx);
                tray::init(window, &store, cx);
                cx.new(|cx| {
                    let mut view = AppView::new(store, first_start, window, cx);
                    if let Some(link) = initial_link {
                        view.open_deep_link(link, window, cx);
                    }
                    view
                })
            },
        )
        .expect("failed to open the main window");
        if !safe_mode {
            plugin_ui::PluginHost::start_hot_reload_poller(cx);
            if let Some(mgr) = plugin_ui::PluginHost::global(cx).and_then(|h| h.manager().cloned()) {
                cx.background_spawn(async move {
                    if let Err(err) = mgr.emit_app_ready() {
                        eprintln!("failed to emit app.ready to plugins: {err:#}");
                    }
                })
                .detach();
            }
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
        cx.activate(true);
    });
    Ok(())
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
    }
}
