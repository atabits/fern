mod actions;
mod app_view;

mod calendar;
mod calendar_view;
mod countdown_view;
mod drag;
mod due_picker;
mod habit_view;
mod i18n;
mod keymap;
mod lists_panel;
mod matrix_view;
mod search_modal;
mod sidebar;
mod task_detail;
mod task_rows;
mod text_input;
mod todo_view;

use std::rc::Rc;

use anyhow::{Context as _, Result};
use fern_core::Store;
use gpui::{App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size};

use crate::actions::Quit;
use crate::app_view::AppView;
use assets::Assets;
use crate::i18n::{CurrentLocale, Locale};


fn open_store() -> Result<Store> {
    let dir = dirs::data_dir()
        .context("cannot determine the user data directory")?
        .join("fern-tasks");
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    Store::open(dir.join("tasks.db"))
}

fn main() -> Result<()> {
    let store = Rc::new(open_store()?);
    let locale = store
        .setting("locale")?
        .and_then(|code| Locale::from_code(&code))
        .unwrap_or_default();

    gpui_platform::application().with_assets(Assets).run(move |cx: &mut App| {
        theme::init(cx);
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
            |window, cx| cx.new(|cx| AppView::new(store, window, cx)),
        )
        .expect("failed to open the main window");
        cx.activate(true);
    });
    Ok(())
}
