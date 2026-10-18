//! App preferences from the Settings window, stored as key-value pairs in the `settings` table.

use anyhow::Result;
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use rusqlite::OptionalExtension as _;

use crate::model::{Id, Priority};
use crate::quick_add::Recognition;
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
    Summary,
    Tags,
    Filters,
    Completed,
    WontDo,
    Trash,
}

impl SmartList {
    /// In the order of the Settings page, grouped as there.
    pub const ALL: [SmartList; 11] = [
        SmartList::All,
        SmartList::Today,
        SmartList::Tomorrow,
        SmartList::Next7Days,
        SmartList::Inbox,
        SmartList::Summary,
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
            SmartList::Summary => "summary",
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
            SmartList::Completed | SmartList::WontDo | SmartList::Trash | SmartList::Filters | SmartList::Summary => {
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

/// The original's Date Format for numeric dates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateFormat {
    /// 12/31/2026
    MonthDayYear,
    /// 31/12/2026
    DayMonthYear,
    /// 2026/12/31
    YearMonthDay,
}

impl DateFormat {
    pub const ALL: [DateFormat; 3] = [DateFormat::MonthDayYear, DateFormat::DayMonthYear, DateFormat::YearMonthDay];

    pub fn code(self) -> &'static str {
        match self {
            DateFormat::MonthDayYear => "mdy",
            DateFormat::DayMonthYear => "dmy",
            DateFormat::YearMonthDay => "ymd",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.code() == code)
    }
}

/// The original's "Default Date" for new tasks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultDate {
    Today,
    Tomorrow,
    DayAfterTomorrow,
    NextWeek,
}

impl DefaultDate {
    pub const ALL: [DefaultDate; 4] =
        [DefaultDate::Today, DefaultDate::Tomorrow, DefaultDate::DayAfterTomorrow, DefaultDate::NextWeek];

    /// The day it means; "Next Week" is the next Monday, as the original's date shortcuts.
    pub fn date(self, today: NaiveDate) -> NaiveDate {
        match self {
            DefaultDate::Today => today,
            DefaultDate::Tomorrow => today + Duration::days(1),
            DefaultDate::DayAfterTomorrow => today + Duration::days(2),
            DefaultDate::NextWeek => today + Duration::days(7 - today.weekday().num_days_from_monday() as i64),
        }
    }

    fn code(self) -> &'static str {
        match self {
            DefaultDate::Today => "today",
            DefaultDate::Tomorrow => "tomorrow",
            DefaultDate::DayAfterTomorrow => "day_after_tomorrow",
            DefaultDate::NextWeek => "next_week",
        }
    }
}

/// The original's durations for "Default Duration", in minutes.
pub const DURATIONS: [i64; 10] = [10, 15, 30, 45, 60, 90, 120, 150, 180, 24 * 60];

/// Settings → More → Task Default: what a new task starts with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskDefaults {
    pub date: Option<DefaultDate>,
    /// The length of a new time range ("Duration"), in minutes.
    pub duration: i64,
    /// A reminder for tasks with a time, in minutes before it; `None` is no reminder.
    pub timed_reminder: Option<i64>,
    /// A reminder for all-day tasks, in minutes before 09:00 of the day; `None` is no reminder.
    pub all_day_reminder: Option<i64>,
    pub priority: Priority,
    pub tag: Option<String>,
    /// "Default Add to": new tasks at the bottom of the list instead of the top.
    pub add_to_bottom: bool,
    /// "Overdue Section shows at": the Overdue group at the bottom of date-grouped lists.
    pub overdue_last: bool,
}

impl Default for TaskDefaults {
    /// The original's defaults: no date, an hour, reminded on time.
    fn default() -> Self {
        Self { date: None, duration: 60, timed_reminder: Some(0), all_day_reminder: None, priority: Priority::None, tag: None, add_to_bottom: false, overdue_last: false }
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
    /// Settings → Date Format; `None` follows the UI language.
    pub date_format: Option<DateFormat>,
    /// Settings → Show Week Numbers (W) in the calendar.
    pub week_numbers: bool,
    /// Settings → More → "Show Mini Calendar in Sidebar": the week under the lists column.
    pub mini_calendar: bool,
    /// The add field's "Input Box Setting": "Detailed" (a taller box with a toolbar and an Add
    /// button) rather than "Simple".
    pub detailed_input: bool,
    /// Settings → Features: which of the original's optional sections show in the sidebar.
    pub features: Features,
    /// Settings → Notifications.
    pub notifications: Notifications,
    /// Settings → More → Smart Recognition, for quick add.
    pub recognition: Recognition,
    pub task_defaults: TaskDefaults,
    /// Where tasks added outside a list go (smart lists, tags, filters); `None` is the Inbox.
    pub default_list: Option<Id>,
    /// Settings → Smart List, in the order of `SmartList::ALL`.
    pub smart_lists: [Visibility; 11],
    /// The list's View Options, as the original's: "Show Date by" Countdown ("in 3 days")
    /// rather than Task Time, and the "Pop-up" task detail layout for a narrow window rather
    /// than "Side peek".
    pub countdown_dates: bool,
    pub popup_details: bool,
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
            date_format: None,
            week_numbers: false,
            mini_calendar: true,
            detailed_input: false,
            features: Features::default(),
            notifications: Notifications::default(),
            recognition: Recognition::default(),
            task_defaults: TaskDefaults::default(),
            default_list: None,
            smart_lists: [Visibility::Show; 11],
            countdown_dates: false,
            popup_details: false,
        }
    }
}

const THEME: &str = "theme";
const WEEK_START: &str = "week_start";
const TIME_FORMAT: &str = "time_format";
const DATE_FORMAT: &str = "date_format";
const WEEK_NUMBERS: &str = "week_numbers";
const MINI_CALENDAR: &str = "mini_calendar";
const INPUT_BOX: &str = "input_box";
const SHOW_DATE_BY: &str = "show_date_by";
const DETAIL_LAYOUT: &str = "detail_layout";
const FEATURES: &str = "features";
const DAILY_NOTIFICATION: &str = "daily_notification";
const DESKTOP_REMINDERS: &str = "desktop_reminders";
const CONSTANT_REMINDERS: &str = "constant_reminders";

/// Settings → Notifications, as far as the original's apply to a desktop app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notifications {
    /// The original's "Daily Notifications": today's tasks in one notification at this time.
    pub daily: Option<chrono::NaiveTime>,
    /// The original's "Web Reminder", here the desktop's: reminders pop up as notifications.
    pub desktop: bool,
    /// The original's "Global Constant Reminder": notifications stay until dismissed.
    pub constant: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self { daily: None, desktop: true, constant: false }
    }
}

impl Notifications {
    /// Whether the daily notification is due at `now`, given the day it was last shown: once a
    /// day, from its time on.
    pub fn daily_due(&self, now: chrono::NaiveDateTime, last_shown: Option<chrono::NaiveDate>) -> bool {
        self.daily.is_some_and(|at| now.time() >= at && last_shown != Some(now.date()))
    }
}

/// The original's Features: sections that can be turned off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    pub calendar: bool,
    pub matrix: bool,
    pub habits: bool,
    pub focus: bool,
    pub countdown: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self { calendar: true, matrix: true, habits: true, focus: true, countdown: true }
    }
}

impl Features {
    /// Stored as the names of the sections turned off, e.g. "matrix,countdown".
    fn to_storage(self) -> String {
        let off: Vec<&str> = [
            ("calendar", self.calendar),
            ("matrix", self.matrix),
            ("habits", self.habits),
            ("focus", self.focus),
            ("countdown", self.countdown),
        ]
        .into_iter()
        .filter(|(_, on)| !on)
        .map(|(name, _)| name)
        .collect();
        off.join(",")
    }

    fn from_storage(text: &str) -> Self {
        let off: Vec<&str> = text.split(',').collect();
        Self {
            calendar: !off.contains(&"calendar"),
            matrix: !off.contains(&"matrix"),
            habits: !off.contains(&"habits"),
            focus: !off.contains(&"focus"),
            countdown: !off.contains(&"countdown"),
        }
    }
}
const DEFAULT_DATE: &str = "default_date";
const DEFAULT_DURATION: &str = "default_duration";
const DEFAULT_TIMED_REMINDER: &str = "default_timed_reminder";
const DEFAULT_ALL_DAY_REMINDER: &str = "default_all_day_reminder";
const DEFAULT_PRIORITY: &str = "default_priority";
const DEFAULT_TAG: &str = "default_tag";
const DEFAULT_ADD_TO: &str = "default_add_to";
const OVERDUE_AT: &str = "overdue_at";
/// Smart Recognition switches, in the order of `Recognition`'s fields.
const RECOGNITION: [&str; 4] =
    ["recognize_dates", "remove_date_text", "recognize_tags", "remove_tag_text"];
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
        let date_format = self.setting(DATE_FORMAT)?.and_then(|c| DateFormat::from_code(&c));
        let week_numbers = self.setting(WEEK_NUMBERS)?.as_deref() == Some("1");
        let mini_calendar = self.setting(MINI_CALENDAR)?.as_deref() != Some("0");
        let detailed_input = self.setting(INPUT_BOX)?.as_deref() == Some("detailed");
        let countdown_dates = self.setting(SHOW_DATE_BY)?.as_deref() == Some("countdown");
        let popup_details = self.setting(DETAIL_LAYOUT)?.as_deref() == Some("popup");
        let features = self.setting(FEATURES)?.map(|t| Features::from_storage(&t)).unwrap_or_default();
        let notifications = Notifications {
            daily: self.setting(DAILY_NOTIFICATION)?.and_then(|t| chrono::NaiveTime::parse_from_str(&t, "%H:%M").ok()),
            desktop: self.setting(DESKTOP_REMINDERS)?.as_deref() != Some("0"),
            constant: self.setting(CONSTANT_REMINDERS)?.as_deref() == Some("1"),
        };
        let flag = |key: &str| -> Result<bool> { Ok(self.setting(key)?.as_deref() != Some("0")) };
        let recognition = Recognition {
            dates: flag(RECOGNITION[0])?,
            remove_date_text: flag(RECOGNITION[1])?,
            tags: flag(RECOGNITION[2])?,
            remove_tag_text: flag(RECOGNITION[3])?,
        };
        let defaults_of_tasks = TaskDefaults::default();
        // A stored offset, "none" for no reminder, or unset for the default.
        let reminder = |key: &str, default: Option<i64>| -> Result<Option<i64>> {
            Ok(match self.setting(key)?.as_deref() {
                None => default,
                Some("none") => None,
                Some(v) => v.parse().ok().or(default),
            })
        };
        let task_defaults = TaskDefaults {
            date: self.setting(DEFAULT_DATE)?.and_then(|c| DefaultDate::ALL.into_iter().find(|d| d.code() == c)),
            duration: self
                .setting(DEFAULT_DURATION)?
                .and_then(|v| v.parse().ok())
                .filter(|m| *m > 0)
                .unwrap_or(defaults_of_tasks.duration),
            timed_reminder: reminder(DEFAULT_TIMED_REMINDER, defaults_of_tasks.timed_reminder)?,
            all_day_reminder: reminder(DEFAULT_ALL_DAY_REMINDER, defaults_of_tasks.all_day_reminder)?,
            priority: self
                .setting(DEFAULT_PRIORITY)?
                .and_then(|v| v.parse::<i64>().ok())
                .map(Priority::from_i64)
                .unwrap_or_default(),
            tag: self.setting(DEFAULT_TAG)?.filter(|t| !t.is_empty()),
            add_to_bottom: self.setting(DEFAULT_ADD_TO)?.as_deref() == Some("bottom"),
            overdue_last: self.setting(OVERDUE_AT)?.as_deref() == Some("bottom"),
        };
        let mut preferences = Preferences {
            theme,
            week_start,
            hour12,
            date_format,
            week_numbers,
            mini_calendar,
            detailed_input,
            countdown_dates,
            popup_details,
            features,
            notifications,
            recognition,
            task_defaults,
            default_list,
            ..defaults
        };
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
        self.set_setting(DATE_FORMAT, preferences.date_format.map_or("", DateFormat::code))?;
        self.set_setting(WEEK_NUMBERS, if preferences.week_numbers { "1" } else { "0" })?;
        self.set_setting(MINI_CALENDAR, if preferences.mini_calendar { "1" } else { "0" })?;
        self.set_setting(FEATURES, &preferences.features.to_storage())?;
        let n = &preferences.notifications;
        self.set_setting(DAILY_NOTIFICATION, &n.daily.map_or(String::new(), |t| t.format("%H:%M").to_string()))?;
        self.set_setting(DESKTOP_REMINDERS, if n.desktop { "1" } else { "0" })?;
        self.set_setting(CONSTANT_REMINDERS, if n.constant { "1" } else { "0" })?;
        self.set_setting(INPUT_BOX, if preferences.detailed_input { "detailed" } else { "simple" })?;
        self.set_setting(SHOW_DATE_BY, if preferences.countdown_dates { "countdown" } else { "task_time" })?;
        self.set_setting(DETAIL_LAYOUT, if preferences.popup_details { "popup" } else { "side" })?;
        let t = &preferences.task_defaults;
        let offset = |o: Option<i64>| o.map_or("none".to_owned(), |o| o.to_string());
        self.set_setting(DEFAULT_DATE, t.date.map_or("", DefaultDate::code))?;
        self.set_setting(DEFAULT_DURATION, &t.duration.to_string())?;
        self.set_setting(DEFAULT_TIMED_REMINDER, &offset(t.timed_reminder))?;
        self.set_setting(DEFAULT_ALL_DAY_REMINDER, &offset(t.all_day_reminder))?;
        self.set_setting(DEFAULT_PRIORITY, &t.priority.to_i64().to_string())?;
        self.set_setting(DEFAULT_TAG, t.tag.as_deref().unwrap_or(""))?;
        self.set_setting(DEFAULT_ADD_TO, if t.add_to_bottom { "bottom" } else { "top" })?;
        self.set_setting(OVERDUE_AT, if t.overdue_last { "bottom" } else { "top" })?;
        let r = preferences.recognition;
        for (key, on) in RECOGNITION.iter().zip([r.dates, r.remove_date_text, r.tags, r.remove_tag_text]) {
            self.set_setting(key, if on { "1" } else { "0" })?;
        }
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
            date_format: Some(DateFormat::YearMonthDay),
            week_numbers: true,
            mini_calendar: false,
            detailed_input: true,
            countdown_dates: true,
            popup_details: true,
            features: Features { matrix: false, countdown: false, ..Features::default() },
            notifications: Notifications { daily: chrono::NaiveTime::from_hms_opt(8, 0, 0), desktop: false, constant: true },
            recognition: Recognition { remove_date_text: false, tags: false, ..Default::default() },
            task_defaults: TaskDefaults {
                date: Some(DefaultDate::Tomorrow),
                duration: 30,
                timed_reminder: None,
                all_day_reminder: Some(24 * 60),
                priority: Priority::High,
                tag: Some("work".into()),
                add_to_bottom: true,
                overdue_last: true,
            },
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
    fn default_dates() {
        let sunday = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        assert_eq!(DefaultDate::Tomorrow.date(sunday), NaiveDate::from_ymd_opt(2026, 10, 19).unwrap());
        assert_eq!(DefaultDate::NextWeek.date(sunday), NaiveDate::from_ymd_opt(2026, 10, 19).unwrap());
        let monday = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        assert_eq!(DefaultDate::NextWeek.date(monday), NaiveDate::from_ymd_opt(2026, 10, 26).unwrap());
        assert_eq!(TaskDefaults::default().timed_reminder, Some(0));
    }

    #[test]
    fn unsupported_values_are_ignored() -> Result<()> {
        let store = Store::open_in_memory()?;
        store.set_setting(THEME, "neon")?;
        store.set_setting(WEEK_START, "Wed")?;
        assert_eq!(store.preferences()?, Preferences::default());
        Ok(())
    }

    #[test]
    fn daily_notification_once_a_day() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let at = |h| day.and_hms_opt(h, 0, 0).unwrap();
        let off = Notifications::default();
        assert!(!off.daily_due(at(9), None));
        let on = Notifications { daily: chrono::NaiveTime::from_hms_opt(8, 0, 0), ..off };
        assert!(!on.daily_due(at(7), None), "not before its time");
        assert!(on.daily_due(at(9), None));
        assert!(on.daily_due(at(9), day.pred_opt()));
        assert!(!on.daily_due(at(9), Some(day)), "once a day");
    }
}
