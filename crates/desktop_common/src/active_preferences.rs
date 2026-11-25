//! GPUI global snapshot of user preferences read synchronously during rendering and actions.

use chrono::Weekday;
use fern_core::{DateFormat, Preferences, Store};
use gpui::{App, Global};

use crate::sound;

/// Reactive GPUI global holding lightweight formatting, feature-flag, and audio preferences.
///
/// Replaces scattered `thread_local!` cells and process-wide atomics so multiple tests or
/// windows in a single process remain properly isolated by GPUI `App` context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivePreferences {
    pub week_start: Weekday,
    pub hour12: bool,
    pub date_format: Option<DateFormat>,
    pub week_numbers: bool,
    pub horizons_enabled: bool,
    pub completion_sound: bool,
}

impl Default for ActivePreferences {
    fn default() -> Self {
        Self {
            week_start: Weekday::Mon,
            hour12: false,
            date_format: None,
            week_numbers: false,
            horizons_enabled: true,
            completion_sound: true,
        }
    }
}

impl Global for ActivePreferences {}

impl ActivePreferences {
    /// Returns the active preferences snapshot for `cx` (or defaults if unset).
    pub fn get(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }

    /// Synchronizes the global from a loaded [`Preferences`] struct and completion sound flag.
    pub fn sync_from_preferences(prefs: &Preferences, completion_sound: bool, cx: &mut App) {
        let next = Self {
            week_start: prefs.week_start,
            hour12: prefs.hour12,
            date_format: prefs.date_format,
            week_numbers: prefs.week_numbers,
            horizons_enabled: prefs.features.horizons,
            completion_sound,
        };
        if cx.try_global::<Self>().copied() != Some(next) {
            cx.set_global(next);
        }
    }

    /// Loads [`Preferences`] and completion sound setting from `store` and updates the global.
    pub fn sync_from_store(store: &Store, cx: &mut App) {
        if let Ok(prefs) = store.preferences() {
            let chime = sound::enabled(store);
            Self::sync_from_preferences(&prefs, chime, cx);
        }
    }

    /// Mutates the active preferences global in place, initializing defaults if needed.
    pub fn update(cx: &mut App, f: impl FnOnce(&mut Self)) {
        let mut current = Self::get(cx);
        let before = current;
        f(&mut current);
        if current != before || cx.try_global::<Self>().is_none() {
            cx.set_global(current);
        }
    }
}
