//! Loads user-defined JSONC themes from `<data_dir>/themes/` and watches for live edits.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};
use fern_core::Store;
use gpui::{AnyWindowHandle, App};

pub const THEMES_DIR: &str = "themes";
pub const SAMPLE_THEME_FILE: &str = "custom-theme.json";
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const DEBOUNCE: Duration = Duration::from_millis(100);

pub fn themes_dir() -> Result<PathBuf> {
    Ok(crate::data_dir()?.join(THEMES_DIR))
}

pub fn ensure_themes_dir() -> Result<PathBuf> {
    let dir = themes_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    Ok(dir)
}

/// Ensures `<data_dir>/themes/custom-theme.json` exists without overwriting an existing file.
pub fn ensure_sample_theme_file() -> Result<PathBuf> {
    let dir = ensure_themes_dir()?;
    ensure_sample_theme_file_at(&dir)
}

fn ensure_sample_theme_file_at(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join(SAMPLE_THEME_FILE);
    if !path.exists() {
        std::fs::write(&path, theme::EXAMPLE_THEME_TEMPLATE)
            .with_context(|| format!("cannot create {}", path.display()))?;
    }
    Ok(path)
}

pub fn load(cx: &mut App) {
    let Ok(dir) = themes_dir() else { return };
    let (themes, errors) = theme::load_themes_from_dir(&dir);
    for error in &errors {
        eprintln!("theme: {error}");
    }
    theme::reload_user_themes(themes, errors, cx);
}

/// Reloads user themes off the UI thread and applies the active theme selection.
pub fn reload_and_apply_async(store: Rc<Store>, window: AnyWindowHandle, cx: &mut App) {
    let Ok(dir) = themes_dir() else { return };
    let task = cx.background_executor().spawn(async move { theme::load_themes_from_dir(&dir) });
    cx.spawn(async move |cx| {
        let (themes, errors) = task.await;
        cx.update(|cx| {
            apply_loaded(&store, window, themes, errors, cx);
        });
    })
    .detach();
}

/// Ensures the sample theme file exists off the UI thread, reloads themes, and opens the file.
pub fn create_and_open_sample_async(store: Rc<Store>, window: AnyWindowHandle, cx: &mut App) {
    let task = cx.background_executor().spawn(async move {
        let path = ensure_sample_theme_file()?;
        let loaded = themes_dir().map(|dir| theme::load_themes_from_dir(&dir)).unwrap_or_default();
        Ok::<_, anyhow::Error>((path, loaded))
    });
    cx.spawn(async move |cx| match task.await {
        Ok((path, (themes, errors))) => {
            cx.update(|cx| {
                apply_loaded(&store, window, themes, errors, cx);
                cx.open_with_system(&path);
            });
        }
        Err(err) => eprintln!("theme: {err:#}"),
    })
    .detach();
}

/// Ensures the themes directory exists off the UI thread and opens it in the system file manager.
pub fn open_themes_dir_async(cx: &mut App) {
    let task = cx.background_executor().spawn(async move { ensure_themes_dir() });
    cx.spawn(async move |cx| match task.await {
        Ok(dir) => {
            cx.update(|cx| cx.open_with_system(&dir));
        }
        Err(err) => eprintln!("theme: {err:#}"),
    })
    .detach();
}

fn apply_loaded(
    store: &Store,
    window: AnyWindowHandle,
    themes: Vec<theme::Theme>,
    errors: Vec<String>,
    cx: &mut App,
) {
    for error in &errors {
        eprintln!("theme: {error}");
    }
    theme::reload_user_themes(themes, errors, cx);
    let selection = store.preferences().unwrap_or_default().theme;
    let target = if cx.windows().contains(&window) {
        Some(window)
    } else {
        cx.windows().into_iter().next()
    };
    if let Some(target) = target {
        let _ = target.update(cx, |_, window, cx| {
            crate::settings_window::apply_theme_only(&selection, window, cx);
        });
    }
    cx.refresh_windows();
}

fn dir_fingerprint(dir: &Path) -> Vec<(OsString, u64, Option<SystemTime>)> {
    if !dir.exists() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !theme::is_theme_file(&path) {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            items.push((entry.file_name(), meta.len(), meta.modified().ok()));
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
}

pub fn watch(store: Rc<Store>, window: AnyWindowHandle, cx: &mut App) {
    let Ok(dir) = themes_dir() else { return };
    cx.spawn(async move |cx| {
        let bg = cx.background_executor().clone();
        let initial_dir = dir.clone();
        let mut last_fp = bg.spawn(async move { dir_fingerprint(&initial_dir) }).await;

        loop {
            let poll_dir = dir.clone();
            let prev_fp = last_fp.clone();
            let bg_for_poll = bg.clone();
            let (final_fp, themes, errors) = bg
                .spawn(async move {
                    loop {
                        bg_for_poll.timer(POLL_INTERVAL).await;
                        let next_fp = dir_fingerprint(&poll_dir);
                        if next_fp != prev_fp {
                            bg_for_poll.timer(DEBOUNCE).await;
                            let final_fp = dir_fingerprint(&poll_dir);
                            let (themes, errors) = theme::load_themes_from_dir(&poll_dir);
                            return (final_fp, themes, errors);
                        }
                    }
                })
                .await;
            last_fp = final_fp;

            let alive = cx.update(|cx| {
                if cx.windows().is_empty() {
                    return false;
                }
                apply_loaded(&store, window, themes, errors, cx);
                true
            });
            if !alive {
                break;
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn ensure_sample_theme_file_creates_valid_template_without_overwriting() {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("fern-user-themes-test-{unique}"));

        let path = ensure_sample_theme_file_at(&dir).unwrap();
        assert_eq!(path, dir.join(SAMPLE_THEME_FILE));
        let (themes, errors) = theme::load_themes_from_dir(&dir);
        assert_eq!(themes.len(), 2);
        assert!(errors.is_empty(), "{errors:?}");

        std::fs::write(&path, "// user customized\n{}").unwrap();
        let same_path = ensure_sample_theme_file_at(&dir).unwrap();
        assert_eq!(same_path, path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "// user customized\n{}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
