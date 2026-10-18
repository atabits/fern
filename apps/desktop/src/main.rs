mod actions;
mod app_view;

mod calendar;
mod calendar_view;
mod countdown_view;
mod drag;
mod emoji;
mod export;
mod due_picker;
mod focus_timer;
mod focus_view;
mod habit_view;
mod i18n;
mod keymap;
mod list_dialog;
mod lists_panel;
mod markdown_toolbar;
mod matrix_view;
mod print;
mod reminders;
mod search_modal;
mod settings_window;
mod sidebar;
mod summary_view;
mod sync;
mod tag_dialog;
mod task_detail;
mod task_menu;
mod task_rows;
mod text_input;
mod undo;
mod todo_view;

use std::rc::Rc;

use anyhow::{Context as _, Result};
use fern_core::Store;
use gpui::{App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size};

use crate::actions::Quit;
use crate::app_view::AppView;
use assets::Assets;
use crate::i18n::{CurrentLocale, Locale};


/// Where Fern keeps its data.
pub fn data_dir() -> Result<std::path::PathBuf> {
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

fn open_store() -> Result<Store> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let mut store = Store::open(dir.join("tasks.db"))?;
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
    Ok(store)
}

fn main() -> Result<()> {
    let store = Rc::new(open_store()?);
    let locale = store
        .setting("locale")?
        .and_then(|code| Locale::from_code(&code))
        .unwrap_or_default();

    gpui_platform::application().with_assets(Assets).run(move |cx: &mut App| {
        theme::init(cx);
        if let Err(err) = Assets.load_fonts(cx) {
            eprintln!("failed to load the fonts: {err:#}");
        }
        cx.set_global(CurrentLocale(locale));
        match keymap::load_default(&Assets, cx) {
            Ok(errors) => errors.iter().for_each(|err| eprintln!("keymap: {err}")),
            Err(err) => eprintln!("failed to load the keymap: {err:#}"),
        }
        cx.on_action(|_: &Quit, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1280.), px(760.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions { title: Some("Fern".into()), ..Default::default() }),
                // Overridable so that window-manager rules can target dev builds (used for UI screenshots).
                app_id: Some(std::env::var("FERN_APP_ID").unwrap_or_else(|_| "fern-tasks".into())),
                window_min_size: Some(size(px(900.), px(400.))),
                ..Default::default()
            },
            |window, cx| {
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
