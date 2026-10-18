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

/// An entry of the lists column that Settings → Smart List can show or hide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SmartList {
    All,
    Today,
    Tomorrow,
    Next7Days,
    Inbox,
    Tags,
    Filters,
    Completed,
    WontDo,
    Trash,
}

impl SmartList {
    /// In the order of the Settings page, grouped as there.
    pub const ALL: [SmartList; 10] = [
        SmartList::All,
        SmartList::Today,
        SmartList::Tomorrow,
        SmartList::Next7Days,
        SmartList::Inbox,
        SmartList::Tags,
        SmartList::Filters,
        SmartList::Completed,
        SmartList::WontDo,
        SmartList::Trash,
    ];

    pub fn code(self) -> &'static str {
        match self {
            SmartList::All => "all",
            SmartList::Today => "today",
            SmartList::Tomorrow => "tomorrow",
            SmartList::Next7Days => "next_7_days",
            SmartList::Inbox => "inbox",
            SmartList::Tags => "tags",
            SmartList::Filters => "filters",
            SmartList::Completed => "completed",
            SmartList::WontDo => "wont_do",
            SmartList::Trash => "trash",
        }
    }

    /// The choices the original offers: task lists can hide while empty, the Tags section can
    /// omit tags without open tasks, and the closed-task lists are only shown or hidden.
    pub fn options(self) -> &'static [Visibility] {
        match self {
            SmartList::Completed | SmartList::WontDo | SmartList::Trash | SmartList::Filters => {
                &[Visibility::Show, Visibility::Hide]
            }
            SmartList::Tags => &[Visibility::Show, Visibility::IfNotEmpty, Visibility::Hide],
            _ => &[Visibility::IfNotEmpty, Visibility::Show, Visibility::Hide],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Visibility {
    #[default]
    Show,
    /// "Show if not empty"; for Tags, "Show (Omit Empty Tags)".
    IfNotEmpty,
    Hide,
}

impl Visibility {
    pub fn code(self) -> &'static str {
        match self {
            Visibility::Show => "show",
            Visibility::IfNotEmpty => "if_not_empty",
            Visibility::Hide => "hide",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [Visibility::Show, Visibility::IfNotEmpty, Visibility::Hide].into_iter().find(|v| v.code() == code)
    }

    /// Whether an entry with `count` items (open tasks, or tags with open tasks) is listed.
    pub fn shows(self, count: usize) -> bool {
        match self {
            Visibility::Show => true,
            Visibility::IfNotEmpty => count > 0,
            Visibility::Hide => false,
        }
    }
}

/// The days a week can start on, as in the original's "Start Week on".
pub const WEEK_STARTS: [Weekday; 3] = [Weekday::Mon, Weekday::Sun, Weekday::Sat];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub theme: ThemeMode,
    pub week_start: Weekday,
    /// Settings → Time Format: 12-hour ("1:00 PM") instead of 24-hour ("13:00").
    pub hour12: bool,
    /// Where tasks added outside a list go (smart lists, tags, filters); `None` is the Inbox.
    pub default_list: Option<Id>,
    /// Settings → Smart List, in the order of `SmartList::ALL`.
    pub smart_lists: [Visibility; 10],
}

impl Preferences {
    pub fn visibility(&self, list: SmartList) -> Visibility {
        let index = SmartList::ALL.iter().position(|l| *l == list).expect("every smart list is in ALL");
        self.smart_lists[index]
    }

    /// Sets a list's visibility, ignoring choices the list does not offer.
    pub fn set_visibility(&mut self, list: SmartList, visibility: Visibility) {
        if list.options().contains(&visibility) {
            let index = SmartList::ALL.iter().position(|l| *l == list).expect("every smart list is in ALL");
            self.smart_lists[index] = visibility;
        }
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: ThemeMode::default(),
            week_start: Weekday::Mon,
            hour12: false,
            default_list: None,
            smart_lists: [Visibility::Show; 10],
        }
    }
}

const THEME: &str = "theme";
const WEEK_START: &str = "week_start";
const TIME_FORMAT: &str = "time_format";
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
        let hour12 = self.setting(TIME_FORMAT)?.as_deref() == Some("12");
        let mut preferences = Preferences { theme, week_start, hour12, default_list, ..defaults };
        for list in SmartList::ALL {
            if let Some(visibility) = self.setting(&smart_list_key(list))?.and_then(|c| Visibility::from_code(&c)) {
                preferences.set_visibility(list, visibility);
            }
        }
        Ok(preferences)
    }

    pub fn save_preferences(&self, preferences: &Preferences) -> Result<()> {
        self.set_setting(THEME, preferences.theme.code())?;
        self.set_setting(WEEK_START, &preferences.week_start.to_string())?;
        self.set_setting(TIME_FORMAT, if preferences.hour12 { "12" } else { "24" })?;
        self.set_setting(DEFAULT_LIST, &preferences.default_list.map(|id| id.to_string()).unwrap_or_default())?;
        for list in SmartList::ALL {
            self.set_setting(&smart_list_key(list), preferences.visibility(list).code())?;
        }
        Ok(())
    }
}

fn smart_list_key(list: SmartList) -> String {
    format!("smart_list.{}", list.code())
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
            Preferences {
            theme: ThemeMode::Light,
            week_start: Weekday::Sun,
            hour12: true,
            default_list: Some(list.id),
            ..Default::default()
        };
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
    fn smart_list_visibility() -> Result<()> {
        let store = Store::open_in_memory()?;
        let mut preferences = Preferences::default();
        preferences.set_visibility(SmartList::Tomorrow, Visibility::Hide);
        preferences.set_visibility(SmartList::Today, Visibility::IfNotEmpty);
        // The Trash is only shown or hidden.
        preferences.set_visibility(SmartList::Trash, Visibility::IfNotEmpty);
        store.save_preferences(&preferences)?;

        let loaded = store.preferences()?;
        assert_eq!(loaded.visibility(SmartList::Tomorrow), Visibility::Hide);
        assert_eq!(loaded.visibility(SmartList::Today), Visibility::IfNotEmpty);
        assert_eq!(loaded.visibility(SmartList::Trash), Visibility::Show);
        assert!(!Visibility::IfNotEmpty.shows(0));
        assert!(Visibility::IfNotEmpty.shows(2));
        assert!(!Visibility::Hide.shows(5));
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
