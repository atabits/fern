//! Loads user-defined JSONC themes from `<data_dir>/themes/`, applies theme selections, and
//! watches for live edits while a window is active.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};
use fern_core::{Store, ThemeMode, ThemeSelection};
use gpui::{AnyWindowHandle, App, Global, SharedString, Window, WindowAppearance};
use theme::{ActiveTheme as _, Theme, ThemeRegistry};

pub const THEMES_DIR: &str = "themes";
pub const SAMPLE_THEME_FILE: &str = "custom-theme.json";
/// Poll interval while a Fern window is active (paused completely when hidden/inactive).
const POLL_INTERVAL: Duration = Duration::from_millis(2500);
const DEBOUNCE: Duration = Duration::from_millis(100);

/// An explicit "just this time" choice expires on restart or the next system appearance change.
#[derive(Clone)]
pub struct TemporaryTheme {
    pub system_dark: bool,
    pub dark: bool,
    pub theme_id: SharedString,
}

impl Global for TemporaryTheme {}

pub fn system_dark(window: &Window) -> bool {
    matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

pub fn remembered(selection: &ThemeSelection, dark: bool, cx: &App) -> Arc<Theme> {
    let id = if dark { &selection.dark } else { &selection.light };
    ThemeRegistry::global(cx).resolve(dark, id.as_deref())
}

pub fn set_temporary(choice: TemporaryTheme, cx: &mut App) {
    cx.set_global(choice);
}

pub fn clear_temporary(cx: &mut App) {
    if cx.has_global::<TemporaryTheme>() {
        cx.remove_global::<TemporaryTheme>();
    }
}

/// Applies `selection` to the active theme, returning `true` if the theme changed and windows
/// were refreshed.
pub fn apply_theme(selection: &ThemeSelection, window: &Window, cx: &mut App) -> bool {
    let dark = system_dark(window);
    let theme = match cx.try_global::<TemporaryTheme>() {
        Some(choice) if selection.mode == ThemeMode::System && choice.system_dark == dark => {
            ThemeRegistry::global(cx).resolve(choice.dark, Some(choice.theme_id.as_ref()))
        }
        _ => {
            clear_temporary(cx);
            remembered(selection, selection.mode.is_dark(dark), cx)
        }
    };
    if !Arc::ptr_eq(cx.theme(), &theme) {
        theme::set_arc(theme, cx);
        cx.refresh_windows();
        true
    } else {
        false
    }
}

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

fn enabled_theme_plugins(cx: &App) -> Vec<plugins::runtime::PluginInfo> {
    plugin_ui::PluginHost::global(cx)
        .map(|host| {
            host.plugins()
                .iter()
                .filter(|p| p.enabled && !p.manifest.contributions.themes.is_empty())
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn load_user_and_plugin_themes(
    dir: &Path,
    theme_plugins: &[plugins::runtime::PluginInfo],
) -> (Vec<Theme>, Vec<String>) {
    let (mut themes, mut errors) = theme::load_themes_from_dir(dir);
    if !theme_plugins.is_empty() {
        let (plugin_themes, plugin_errors) = plugin_ui::collect_plugin_themes(theme_plugins);
        themes.extend(plugin_themes);
        errors.extend(plugin_errors);
    }
    (themes, errors)
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
    let theme_plugins = enabled_theme_plugins(cx);
    let task = cx
        .background_executor()
        .spawn(async move { load_user_and_plugin_themes(&dir, &theme_plugins) });
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
    let theme_plugins = enabled_theme_plugins(cx);
    let task = cx.background_executor().spawn(async move {
        let path = ensure_sample_theme_file()?;
        let loaded = themes_dir()
            .map(|dir| load_user_and_plugin_themes(&dir, &theme_plugins))
            .unwrap_or_default();
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
    themes: Vec<Theme>,
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
    let mut refreshed = false;
    if let Some(target) = target
        && let Ok(did_refresh) = target.update(cx, |_, window, cx| apply_theme(&selection, window, cx))
    {
        refreshed = did_refresh;
    }
    if !refreshed {
        cx.refresh_windows();
    }
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
            bg.timer(POLL_INTERVAL).await;
            // Exit immediately once all windows are closed, and pause FS polling while
            // the app is minimized / hidden in the tray / unfocused.
            enum WatchState {
                Stop,
                Idle,
                Active,
            }
            let state = cx.update(|cx| {
                if cx.windows().is_empty() {
                    WatchState::Stop
                } else if cx.active_window().is_none() {
                    WatchState::Idle
                } else {
                    WatchState::Active
                }
            });
            match state {
                WatchState::Stop => break,
                WatchState::Idle => continue,
                WatchState::Active => {}
            }

            let poll_dir = dir.clone();
            let prev_fp = last_fp.clone();
            let bg_for_debounce = bg.clone();
            let final_fp = bg
                .spawn(async move {
                    let next_fp = dir_fingerprint(&poll_dir);
                    if next_fp == prev_fp {
                        return None;
                    }
                    bg_for_debounce.timer(DEBOUNCE).await;
                    Some(dir_fingerprint(&poll_dir))
                })
                .await;
            let Some(final_fp) = final_fp else {
                continue;
            };
            last_fp = final_fp;

            let theme_plugins = cx.update(|cx| enabled_theme_plugins(cx));
            let load_dir = dir.clone();
            let (themes, errors) = bg
                .spawn(async move { load_user_and_plugin_themes(&load_dir, &theme_plugins) })
                .await;

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
