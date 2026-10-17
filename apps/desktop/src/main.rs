mod app_view;
mod assets;
mod calendar;
mod lists_panel;
mod sidebar;
mod task_detail;
mod task_rows;
mod text_input;
mod theme;
mod todo_view;
mod widgets;

use anyhow::{Context as _, Result};
use amin_core::Store;
use gpui::{
    App, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions, prelude::*,
    px, size,
};

use crate::app_view::AppView;
use crate::assets::Assets;

actions!(amin, [Quit]);

fn open_store() -> Result<Store> {
    let dir = dirs::data_dir()
        .context("не удалось определить каталог данных")?
        .join("amin-task-tracker");
    std::fs::create_dir_all(&dir).with_context(|| format!("не удалось создать {}", dir.display()))?;
    Store::open(dir.join("tasks.db"))
}

fn main() -> Result<()> {
    let store = open_store()?;

    gpui_platform::application().with_assets(Assets).run(move |cx: &mut App| {
        text_input::bind_keys(cx);
        cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1280.), px(760.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions { title: Some("Amin".into()), ..Default::default() }),
                // Переопределение нужно для скриншотов при разработке: по нему ставится правило окна.
                app_id: Some(std::env::var("AMIN_APP_ID").unwrap_or_else(|_| "amin-task-tracker".into())),
                window_min_size: Some(size(px(900.), px(400.))),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| AppView::new(store, window, cx)),
        )
        .expect("не удалось открыть окно");
        cx.activate(true);
    });
    Ok(())
}
