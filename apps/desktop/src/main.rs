// A release build on Windows opens no console window next to the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod app_view;

mod calendar;
mod calendar_options_view;
mod calendar_view;
mod countdown_view;
mod drag;
mod emoji;
mod export;
mod due_picker;
mod focus_timer;
mod focus_view;
mod http;
mod habit_view;
mod i18n;
mod keymap;
mod list_dialog;
mod lists_panel;
mod markdown_toolbar;
mod matrix_view;
mod print;
mod reminders;
mod resize;
mod search_modal;
mod settings_window;
mod sidebar;
mod sticky_note;
mod subscriptions;
mod summary_view;
mod sync;
mod tag_dialog;
mod task_detail;
mod task_menu;
mod task_rows;
mod text_input;
mod title_bar;
mod touch;
mod touch_keyboard;
mod undo;
mod updater;
mod todo_view;

use std::rc::Rc;

use anyhow::{Context as _, Result};
use fern_core::Store;
use gpui::{App, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};

use crate::actions::Quit;
use crate::app_view::AppView;
use assets::Assets;
use crate::i18n::{CurrentLocale, Locale};


/// Where Fern keeps its data.
pub fn data_dir() -> Result<std::path::PathBuf> {
    // Debug builds can be pointed at throwaway data, so that checking a change leaves the real
    // data alone.
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("FERN_DATA_DIR") {
        return Ok(dir.into());
    }
    Ok(dirs::data_dir().context("cannot determine the user data directory")?.join("fern-tasks"))
}

/// Where attached files are kept, as copies.
pub fn attachments_dir() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("attachments"))
}

/// A backup chosen with "Import Backups", restored on the next start before anything opens.
pub fn pending_restore_path() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("restore.db"))
}

/// Where the daily copies of the data are kept.
pub fn backups_dir() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("backups"))
}

/// How many daily copies are kept.
const DAILY_BACKUPS: usize = 7;

/// Opens the data, and whether it was just created: the first start.
fn open_store() -> Result<(Store, bool)> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join("tasks.db");
    let first_start = !path.exists();
    let mut store = Store::open(path)?;
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
    Ok((store, first_start))
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
    let (store, first_start) = open_store()?;
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

    gpui_platform::application().with_assets(Assets).run(move |cx: &mut App| {
        theme::init(cx);
        cx.set_http_client(http::UreqHttpClient::new());
        if let Err(err) = Assets.load_fonts(cx) {
            eprintln!("failed to load the fonts: {err:#}");
        }
        cx.set_global(CurrentLocale(locale));
        match keymap::load_default(&Assets, cx) {
            Ok(errors) => errors.iter().for_each(|err| eprintln!("keymap: {err}")),
            Err(err) => eprintln!("failed to load the keymap: {err:#}"),
        }
        cx.on_action(|_: &Quit, cx| cx.quit());
        updater::init(store.clone(), cx);
        subscriptions::init(store.clone(), cx);

        let bounds = Bounds::centered(None, size(px(1280.), px(760.)), cx);
        let (titlebar, decorations) = title_bar::window_options();
        cx.open_window(
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
                // `fern fern://task/<id>` opens that task ("Copy Link").
                let link = std::env::args().skip(1).find_map(|arg| fern_core::link::parse_task_link(&arg));
                cx.new(|cx| {
                    let mut view = AppView::new(store, window, cx);
                    if let Some(task) = link {
                        view.open_task(task, window, cx);
                    }
                    view
                })
            },
        )
        .expect("failed to open the main window");
        cx.activate(true);
    });
    Ok(())
}
