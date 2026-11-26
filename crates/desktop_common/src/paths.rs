use anyhow::{Context as _, Result};

use crate::portable;

/// How many daily copies are kept.
pub const DAILY_BACKUPS: usize = 7;

/// Default freedesktop `app_id` / `StartupWMClass`.
pub const DEFAULT_APP_ID: &str = "fern-tasks";

/// The freedesktop application ID / WM class (`FERN_APP_ID` > `FLATPAK_ID` > `fern-tasks`).
pub fn app_id() -> String {
    crate::runtime_config::RuntimeConfig::get().app_id.clone()
}

pub(crate) fn resolve_app_id(fern_app_id: Option<&str>, flatpak_id: Option<&str>) -> String {
    if let Some(id) = fern_app_id.filter(|s| !s.is_empty()) {
        return id.to_owned();
    }
    if cfg!(target_os = "linux")
        && let Some(id) = flatpak_id.filter(|s| !s.is_empty())
    {
        return id.to_owned();
    }
    DEFAULT_APP_ID.to_owned()
}

pub(crate) fn detect_flatpak() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var_os("FLATPAK_ID").is_some_and(|v| !v.is_empty())
            || std::path::Path::new("/.flatpak-info").exists())
}

/// Returns true when running inside a Linux Flatpak sandbox.
pub fn is_flatpak() -> bool {
    crate::runtime_config::RuntimeConfig::get().is_flatpak
}

/// Icon name exposed to desktop portals (notifications, StatusNotifierItem).
pub fn desktop_icon_name() -> String {
    let cfg = crate::runtime_config::RuntimeConfig::get();
    if cfg.is_flatpak {
        return cfg.app_id.clone();
    }
    "fern".to_owned()
}

pub(crate) fn detect_wayland_session() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    is_wayland_env(
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
    )
}

/// Returns true on Linux when the active graphical session uses Wayland.
pub fn is_wayland_session() -> bool {
    crate::runtime_config::RuntimeConfig::get().is_wayland_session
}

fn is_wayland_env(wayland_display: Option<&str>, session_type: Option<&str>) -> bool {
    wayland_display.is_some_and(|s| !s.is_empty())
        || session_type.is_some_and(|s| s.eq_ignore_ascii_case("wayland"))
}

/// Where Fern keeps its data.
pub fn data_dir() -> Result<std::path::PathBuf> {
    if let Some(dir) = &crate::runtime_config::RuntimeConfig::get().data_dir_override {
        return Ok(dir.clone());
    }
    if let Some(dir) = portable::data_dir() {
        return Ok(dir.to_owned());
    }
    Ok(dirs::data_dir().context("cannot determine the user data directory")?.join("fern-tasks"))
}

/// Where temporary files for printing and backup exports are written.
pub fn scratch_dir() -> std::io::Result<std::path::PathBuf> {
    if let Some(dir) = portable::data_dir() {
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache)?;
        return Ok(cache);
    }
    if is_flatpak()
        && let Some(cache) = dirs::cache_dir().map(|d| d.join("fern-tasks"))
    {
        std::fs::create_dir_all(&cache)?;
        return Ok(cache);
    }
    Ok(std::env::temp_dir())
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

/// Where installed plugins live (`<data_dir>/plugins`).
pub fn plugins_dir() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("plugins"))
}

/// Encrypted secrets vault file, stored outside `tasks.db` and excluded from backups/sync.
pub fn plugin_secrets_path() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("plugin-secrets.vault"))
}

/// Master key file for `plugin-secrets.vault`.
pub fn plugin_secrets_key_path() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("plugin-secrets.key"))
}

/// Encrypted Fern Cloud session vault file, stored outside `tasks.db` and excluded from backups/sync.
pub fn cloud_session_path() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("cloud-session.vault"))
}

/// Master key file for `cloud-session.vault` (DPAPI-protected on Windows, `0600` on Unix).
pub fn cloud_session_key_path() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("cloud-session.key"))
}

/// Offline registry catalog and ETag cache directory (`<data_dir>/plugins/.registry-cache`).
pub fn plugin_registry_cache_dir() -> Result<std::path::PathBuf> {
    Ok(plugins_dir()?.join(".registry-cache"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_respects_precedence() {
        assert_eq!(resolve_app_id(Some("custom.app"), Some("org.flatpak.Fern")), "custom.app");
        assert_eq!(resolve_app_id(Some(""), None), DEFAULT_APP_ID);
        assert_eq!(resolve_app_id(None, None), DEFAULT_APP_ID);
        if cfg!(target_os = "linux") {
            assert_eq!(resolve_app_id(None, Some("io.github.atabits.fern")), "io.github.atabits.fern");
        } else {
            assert_eq!(resolve_app_id(None, Some("io.github.atabits.fern")), DEFAULT_APP_ID);
        }
    }

    #[test]
    fn wayland_env_detection_handles_display_and_session_type() {
        assert!(is_wayland_env(Some("wayland-0"), None));
        assert!(is_wayland_env(None, Some("Wayland")));
        assert!(!is_wayland_env(Some(""), Some("x11")));
        assert!(!is_wayland_env(None, None));
    }
}
