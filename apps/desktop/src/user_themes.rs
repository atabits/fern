//! Loads user-defined JSONC themes from `<data_dir>/themes/` and watches for live edits.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};
use fern_core::Store;
use gpui::{AnyWindowHandle, App, Window};

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

fn is_theme_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "json" | "jsonc"))
}

pub fn load(cx: &mut App) {
    let Ok(dir) = themes_dir() else { return };
    let (themes, errors) = theme::load_themes_from_dir(&dir);
    for error in &errors {
        eprintln!("theme: {error}");
    }
    theme::reload_user_themes(themes, errors, cx);
}

pub fn reload_and_apply(store: &Store, window: &Window, cx: &mut App) {
    load(cx);
    let selection = store.preferences().unwrap_or_default().theme;
    crate::settings_window::apply_theme_only(&selection, window, cx);
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
        if !is_theme_file(&path) {
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
        let scan_dir = dir.clone();
        let mut last_fp = cx.background_executor().spawn(async move { dir_fingerprint(&scan_dir) }).await;

        loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            let scan_dir = dir.clone();
            let next_fp = cx.background_executor().spawn(async move { dir_fingerprint(&scan_dir) }).await;
            if next_fp == last_fp {
                continue;
            }

            cx.background_executor().timer(DEBOUNCE).await;
            let load_dir = dir.clone();
            let (final_fp, (themes, errors)) = cx
                .background_executor()
                .spawn(async move { (dir_fingerprint(&load_dir), theme::load_themes_from_dir(&load_dir)) })
                .await;
            last_fp = final_fp;

            cx.update(|cx| {
                for error in &errors {
                    eprintln!("theme: {error}");
                }
                theme::reload_user_themes(themes, errors, cx);
                let selection = store.preferences().unwrap_or_default().theme;
                let _ = window.update(cx, |_, window, cx| {
                    crate::settings_window::apply_theme_only(&selection, window, cx);
                });
                cx.refresh_windows();
            });
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
