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
    drag, lists_panel, task_detail, task_rows,
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
#[cfg(windows)]
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

/// Opens the data, and whether it was just created: the first start.
fn open_store(dir: &Path) -> Result<(Store, bool, PathBuf)> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join("tasks.db");
    let first_start = !path.exists();
    let mut store = Store::open(&path)?;
    let pending = pending_restore_path()?;
    if pending.exists() {
        match store.restore_from(&pending, attachments_dir().ok().as_deref()) {
            Ok(()) => eprintln!("restored the backup"),
            Err(err) => eprintln!("failed to restore the backup: {err:#}"),
        }
        std::fs::remove_file(&pending).ok();
    }
    // The sync log keeps one entry per row; without a server it would grow with every edit.
    if let Err(err) = store.compact_sync_log() {
        eprintln!("failed to compact the sync log: {err:#}");
    }
    Ok((store, first_start, path))
}

/// The first start fills the Inbox with a few tips, the first one on top.
fn add_welcome_tasks(store: &Store, strings: &i18n::Strings) -> Result<()> {
    for (title, notes) in strings.welcome_tasks.iter().rev() {
        let task = store.add_task(title)?;
        store.set_notes(task.id, notes)?;
    }
    Ok(())
}

fn main() -> Result<()> {
    // `fern fern://task/<id>` opens that task ("Copy Link").
    let link = std::env::args().skip(1).find_map(|arg| fern_core::link::parse_task_link(&arg));
    let dir = data_dir()?;
    #[cfg(windows)]
    let _instance_guard = match single_instance::acquire(&dir, link)? {
        Some(guard) => guard,
        None => return Ok(()),
    };

    let (store, first_start, db_path) = open_store(&dir)?;
    let store = Rc::new(store);
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
    gpui::Application::with_platform(platform).with_assets(Assets).run(move |cx: &mut App| {
        cx.set_global(DbPath(db_path));
        ui::init_label_metrics(text_system, cx);
        theme::init(cx);
        user_themes::load(cx);
        cx.set_http_client(http::UreqHttpClient::new());
        if let Err(err) = Assets.load_fonts(cx) {
            eprintln!("failed to load the fonts: {err:#}");
        }
        cx.set_global(CurrentLocale(locale));
        match keymap::reload(&Assets, cx) {
            Ok(errors) => errors.iter().for_each(|err| eprintln!("keymap: {err}")),
            Err(err) => eprintln!("failed to load the keymap: {err:#}"),
        }
        updater::init(store.clone(), cx);
        subscriptions::init(store.clone(), cx);

        let bounds = Bounds::centered(None, size(px(1280.), px(760.)), cx);
        let (titlebar, decorations) = title_bar::window_options();
        let store_for_hotkeys = store.clone();
        #[cfg(windows)]
        let store_for_tray = store.clone();
        let store_for_themes = store.clone();
        // Prepare the tray channel before creating the window/icon so early clicks are never dropped.
        #[cfg(windows)]
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
                #[cfg(windows)]
                tray::init(window, &store, cx);
                cx.new(|cx| {
                    let mut view = AppView::new(store, first_start, window, cx);
                    if let Some(task) = link {
                        view.open_task(task, window, cx);
                    }
                    view
                })
            },
        )
        .expect("failed to open the main window");
        #[cfg(windows)]
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
