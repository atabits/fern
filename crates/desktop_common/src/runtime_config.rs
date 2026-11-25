//! Process startup environment configuration cached once as a GPUI [`Global`]
//! (with process-level [`OnceLock`] access for pre-GPUI startup and background helpers).
//!
//! `FERN_DEV_STATE` is read **strictly in debug builds** (`#[cfg(debug_assertions)]`);
//! release binaries ignore it unconditionally.

use std::path::PathBuf;
use std::sync::OnceLock;

use gpui::{App, Global};

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub dev_state: bool,
    pub dev_state_value: Option<String>,
    pub data_dir_override: Option<PathBuf>,
    pub app_id: String,
    pub is_flatpak: bool,
    pub is_wayland_session: bool,
}

impl Global for RuntimeConfig {}

static PROCESS_CONFIG: OnceLock<RuntimeConfig> = OnceLock::new();

#[cfg(debug_assertions)]
fn read_dev_state_env() -> Option<String> {
    std::env::var("FERN_DEV_STATE").ok()
}

#[cfg(not(debug_assertions))]
fn read_dev_state_env() -> Option<String> {
    None
}

impl RuntimeConfig {
    /// Captures startup environment variables once.
    pub fn from_env() -> Self {
        let dev_state_value = read_dev_state_env();
        let dev_state = dev_state_value.is_some();
        let data_dir_override = std::env::var_os("FERN_DATA_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        let app_id = crate::paths::resolve_app_id(
            std::env::var("FERN_APP_ID").ok().as_deref(),
            std::env::var("FLATPAK_ID").ok().as_deref(),
        );
        let is_flatpak = crate::paths::detect_flatpak();
        let is_wayland_session = crate::paths::detect_wayland_session();
        Self {
            dev_state,
            dev_state_value,
            data_dir_override,
            app_id,
            is_flatpak,
            is_wayland_session,
        }
    }

    /// Returns the cached process-level startup configuration.
    pub fn get() -> &'static Self {
        PROCESS_CONFIG.get_or_init(Self::from_env)
    }

    /// Ensures [`RuntimeConfig`] is installed as a GPUI [`Global`].
    pub fn init(cx: &mut App) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::get().clone());
        }
    }
}

/// Returns `true` when `FERN_DEV_STATE` was set in a debug build (`false` in all release builds).
pub fn is_dev_state() -> bool {
    RuntimeConfig::get().dev_state
}

/// Returns the `FERN_DEV_STATE` string value in debug builds (`None` in all release builds).
pub fn dev_state_value() -> Option<&'static str> {
    RuntimeConfig::get().dev_state_value.as_deref()
}
