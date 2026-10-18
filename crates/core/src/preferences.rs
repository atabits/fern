//! App preferences from the Settings window, stored as key-value pairs in the `settings` table.

use anyhow::Result;
use chrono::Weekday;
use rusqlite::OptionalExtension as _;

use crate::model::Id;
use crate::store::Store;

/// The color theme: fixed, or following the system's dark mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    System,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 3] = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark];

    pub fn code(self) -> &'static str {
        match self {
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
            ThemeMode::System => "system",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.code() == code)
    }
}

/// The days a week can start on, as in the original's "Start Week on".
pub const WEEK_STARTS: [Weekday; 3] = [Weekday::Mon, Weekday::Sun, Weekday::Sat];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub theme: ThemeMode,
    pub week_start: Weekday,
    /// Where tasks added outside a list go (smart lists, tags, filters); `None` is the Inbox.
    pub default_list: Option<Id>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self { theme: ThemeMode::default(), week_start: Weekday::Mon, default_list: None }
    }
}

const THEME: &str = "theme";
const WEEK_START: &str = "week_start";
const DEFAULT_LIST: &str = "default_list";

impl Store {
    /// The saved preferences; unknown values and a deleted default list fall back to defaults.
    pub fn preferences(&self) -> Result<Preferences> {
        let defaults = Preferences::default();
        let theme = self.setting(THEME)?.and_then(|c| ThemeMode::from_code(&c)).unwrap_or(defaults.theme);
        let week_start = self
            .setting(WEEK_START)?
            .and_then(|c| c.parse::<Weekday>().ok())
            .filter(|day| WEEK_STARTS.contains(day))
            .unwrap_or(defaults.week_start);
        let default_list = match self.setting(DEFAULT_LIST)?.and_then(|c| c.parse::<Id>().ok()) {
            Some(id) => self
                .conn
                .query_row("SELECT 1 FROM lists WHERE id = ?1", [id.to_string()], |_| Ok(()))
                .optional()?
                .map(|_| id),
            None => None,
        };
        Ok(Preferences { theme, week_start, default_list })
    }

    pub fn save_preferences(&self, preferences: &Preferences) -> Result<()> {
        self.set_setting(THEME, preferences.theme.code())?;
        self.set_setting(WEEK_START, &preferences.week_start.to_string())?;
        self.set_setting(DEFAULT_LIST, &preferences.default_list.map(|id| id.to_string()).unwrap_or_default())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_never_saved() -> Result<()> {
        let store = Store::open_in_memory()?;
        assert_eq!(store.preferences()?, Preferences::default());
        Ok(())
    }

    #[test]
    fn roundtrip() -> Result<()> {
        let store = Store::open_in_memory()?;
        let list = store.create_list("Work")?;
        let preferences =
            Preferences { theme: ThemeMode::Light, week_start: Weekday::Sun, default_list: Some(list.id) };
        store.save_preferences(&preferences)?;
        assert_eq!(store.preferences()?, preferences);

        store.save_preferences(&Preferences { default_list: None, ..preferences.clone() })?;
        assert_eq!(store.preferences()?.default_list, None);
        Ok(())
    }

    #[test]
    fn deleted_default_list_falls_back_to_inbox() -> Result<()> {
        let store = Store::open_in_memory()?;
        let list = store.create_list("Work")?;
        store.save_preferences(&Preferences { default_list: Some(list.id), ..Default::default() })?;
        store.delete_list(list.id)?;
        assert_eq!(store.preferences()?.default_list, None);
        Ok(())
    }

    #[test]
    fn unsupported_values_are_ignored() -> Result<()> {
        let store = Store::open_in_memory()?;
        store.set_setting(THEME, "neon")?;
        store.set_setting(WEEK_START, "Wed")?;
        assert_eq!(store.preferences()?, Preferences::default());
        Ok(())
    }
}
