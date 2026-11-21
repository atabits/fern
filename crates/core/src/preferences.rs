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
    Dark,
    Light,
    #[default]
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

    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            Self::Dark => true,
            Self::Light => false,
            Self::System => system_dark,
        }
    }
}

/// The rendering layer resolves the stored preset IDs against its bundled catalog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeSelection {
    pub mode: ThemeMode,
    pub light: Option<String>,
    pub dark: Option<String>,
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

    /// The top smart lists in the lists column, in default display order; draggable to reorder.
    pub const TOP_DRAGGABLE: [SmartList; 6] = [
        SmartList::Inbox,
        SmartList::All,
        SmartList::Today,
        SmartList::Tomorrow,
        SmartList::Next7Days,
        SmartList::Summary,
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

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|list| list.code() == code)
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

/// Reorderable middle sections of the lists column, as in the original's sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SidebarSection {
    Lists,
    Tags,
    Filters,
}

impl SidebarSection {
    pub const ALL: [SidebarSection; 3] = [
        SidebarSection::Lists,
        SidebarSection::Tags,
        SidebarSection::Filters,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Self::Lists => "lists",
            Self::Tags => "tags",
            Self::Filters => "filters",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|section| section.code() == code)
    }
}

/// Reorderable entries in the narrow leftmost navigation rail, as in the original's desktop tab bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RailSection {
    Tasks,
    Calendar,
    Pomodoro,
    Habits,
    Search,
    Matrix,
    Countdown,
}

impl RailSection {
    pub const ALL: [RailSection; 7] = [
        RailSection::Tasks,
        RailSection::Calendar,
        RailSection::Pomodoro,
        RailSection::Habits,
        RailSection::Search,
        RailSection::Matrix,
        RailSection::Countdown,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Self::Tasks => "tasks",
            Self::Calendar => "calendar",
            Self::Pomodoro => "focus",
            Self::Habits => "habits",
            Self::Search => "search",
            Self::Matrix => "matrix",
            Self::Countdown => "countdown",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|section| section.code() == code)
    }
}

fn normalize_order<T: Copy + PartialEq>(stored: &str, defaults: &[T], parse: fn(&str) -> Option<T>) -> Vec<T> {
    let mut order = Vec::with_capacity(defaults.len());
    for code in stored.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(item) = parse(code)
            && defaults.contains(&item)
            && !order.contains(&item)
        {
            order.push(item);
        }
    }
    for &item in defaults {
        if !order.contains(&item) {
            order.push(item);
        }
    }
    order
}

fn reorder_relative<T: Copy + PartialEq>(order: &mut Vec<T>, defaults: &[T], dragged: T, target: T, after: bool) {
    if dragged == target || !defaults.contains(&dragged) || !defaults.contains(&target) {
        return;
    }
    for &item in defaults {
        if !order.contains(&item) {
            order.push(item);
        }
    }
    let Some(from) = order.iter().position(|&item| item == dragged) else { return };
    order.remove(from);
    let Some(to) = order.iter().position(|&item| item == target) else {
        order.insert(from, dragged);
        return;
    };
    order.insert(to + usize::from(after), dragged);
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
    /// 2026-12-31
    IsoYearMonthDay,
}

impl DateFormat {
    pub const ALL: [DateFormat; 4] = [
        DateFormat::MonthDayYear,
        DateFormat::DayMonthYear,
        DateFormat::YearMonthDay,
        DateFormat::IsoYearMonthDay,
    ];

    pub fn code(self) -> &'static str {
        match self {
            DateFormat::MonthDayYear => "mdy",
            DateFormat::DayMonthYear => "dmy",
            DateFormat::YearMonthDay => "ymd",
            DateFormat::IsoYearMonthDay => "iso-ymd",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.code() == code)
    }
}

/// The first day of the next week after `today`, honoring the user's configured `week_start`.
///
/// With `week_start == Weekday::Mon`: Sunday maps to tomorrow (Monday) and Monday maps to
/// next Monday (`+7` days). With `week_start == Weekday::Sun`: Saturday maps to tomorrow (Sunday)
/// and Sunday maps to next Sunday (`+7` days).
pub fn next_week_date(today: NaiveDate, week_start: Weekday) -> NaiveDate {
    let offset = (today.weekday().num_days_from_monday() + 7 - week_start.num_days_from_monday()) % 7;
    today + Duration::days(7 - offset as i64)
}

/// Canonical quick-reschedule date shortcuts shared across menus, pickers, and quick-add.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuickDate {
    Today,
    Tomorrow,
    NextWeek,
    NoDate,
}

impl QuickDate {
    /// Resolves this shortcut into a due date relative to `today` and `week_start`.
    pub fn resolve(self, today: NaiveDate, week_start: Weekday) -> Option<NaiveDate> {
        match self {
            Self::Today => Some(today),
            Self::Tomorrow => Some(today + Duration::days(1)),
            Self::NextWeek => Some(next_week_date(today, week_start)),
            Self::NoDate => None,
        }
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
        self.date_with_week_start(today, Weekday::Mon)
    }

    /// Like [`Self::date`], honoring the user's configured `week_start` for [`Self::NextWeek`].
    pub fn date_with_week_start(self, today: NaiveDate, week_start: Weekday) -> NaiveDate {
        match self {
            DefaultDate::Today => today,
            DefaultDate::Tomorrow => today + Duration::days(1),
            DefaultDate::DayAfterTomorrow => today + Duration::days(2),
            DefaultDate::NextWeek => next_week_date(today, week_start),
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
    pub theme: ThemeSelection,
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
    /// Order of the top draggable smart lists in the lists column.
    pub smart_list_order: Vec<SmartList>,
    /// Order of the middle sections (`Lists`, `Tags`, `Filters`) in the lists column.
    pub sidebar_section_order: Vec<SidebarSection>,
    /// Order of the icons in the narrow leftmost rail.
    pub rail_order: Vec<RailSection>,
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

    pub fn move_smart_list(&mut self, dragged: SmartList, target: SmartList, after: bool) {
        reorder_relative(&mut self.smart_list_order, &SmartList::TOP_DRAGGABLE, dragged, target, after);
    }

    pub fn move_sidebar_section(&mut self, dragged: SidebarSection, target: SidebarSection, after: bool) {
        reorder_relative(&mut self.sidebar_section_order, &SidebarSection::ALL, dragged, target, after);
    }

    pub fn move_rail_section(&mut self, dragged: RailSection, target: RailSection, after: bool) {
        reorder_relative(&mut self.rail_order, &RailSection::ALL, dragged, target, after);
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: ThemeSelection::default(),
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
            smart_list_order: SmartList::TOP_DRAGGABLE.to_vec(),
            sidebar_section_order: SidebarSection::ALL.to_vec(),
            rail_order: RailSection::ALL.to_vec(),
            countdown_dates: false,
            popup_details: false,
        }
    }
}

const THEME: &str = "theme";
const LIGHT_THEME: &str = "theme.light";
const DARK_THEME: &str = "theme.dark";
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
const SMART_LIST_ORDER: &str = "smart_list_order";
const SIDEBAR_SECTION_ORDER: &str = "sidebar_section_order";
const RAIL_ORDER: &str = "sidebar_rail_order";

impl Store {
    /// The saved preferences; unknown values and a deleted default list fall back to defaults.
    pub fn preferences(&self) -> Result<Preferences> {
        let mut settings: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        {
            let mut stmt = self.conn.prepare_cached("SELECT key, value FROM settings")?;
            let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
            for row in rows {
                let (k, v) = row?;
                settings.insert(k, v);
            }
        }
        let get = |key: &str| -> Option<&str> { settings.get(key).map(String::as_str) };

        let defaults = Preferences::default();
        let theme = ThemeSelection {
            mode: get(THEME).and_then(ThemeMode::from_code).unwrap_or_default(),
            light: get(LIGHT_THEME).filter(|s| !s.is_empty()).map(str::to_owned),
            dark: get(DARK_THEME).filter(|s| !s.is_empty()).map(str::to_owned),
        };
        let week_start = get(WEEK_START)
            .and_then(|c| c.parse::<Weekday>().ok())
            .filter(|day| WEEK_STARTS.contains(day))
            .unwrap_or(defaults.week_start);
        let default_list = match get(DEFAULT_LIST).and_then(|c| c.parse::<Id>().ok()) {
            Some(id) => self
                .conn
                .query_row("SELECT 1 FROM lists WHERE id = ?1", [id.to_string()], |_| Ok(()))
                .optional()?
                .map(|_| id),
            None => None,
        };
        let hour12 = get(TIME_FORMAT) == Some("12");
        let date_format = get(DATE_FORMAT).and_then(DateFormat::from_code);
        let week_numbers = get(WEEK_NUMBERS) == Some("1");
        let mini_calendar = get(MINI_CALENDAR) != Some("0");
        let detailed_input = get(INPUT_BOX) == Some("detailed");
        let countdown_dates = get(SHOW_DATE_BY) == Some("countdown");
        let popup_details = get(DETAIL_LAYOUT) == Some("popup");
        let features = get(FEATURES).map(Features::from_storage).unwrap_or_default();
        let notifications = Notifications {
            daily: get(DAILY_NOTIFICATION).and_then(|t| chrono::NaiveTime::parse_from_str(t, "%H:%M").ok()),
            desktop: get(DESKTOP_REMINDERS) != Some("0"),
            constant: get(CONSTANT_REMINDERS) == Some("1"),
        };
        let flag = |key: &str| -> bool { get(key) != Some("0") };
        let recognition = Recognition {
            dates: flag(RECOGNITION[0]),
            remove_date_text: flag(RECOGNITION[1]),
            tags: flag(RECOGNITION[2]),
            remove_tag_text: flag(RECOGNITION[3]),
        };
        let defaults_of_tasks = TaskDefaults::default();
        // A stored offset, "none" for no reminder, or unset for the default.
        let reminder = |key: &str, default: Option<i64>| -> Option<i64> {
            match get(key) {
                None => default,
                Some("none") => None,
                Some(v) => v.parse().ok().or(default),
            }
        };
        let task_defaults = TaskDefaults {
            date: get(DEFAULT_DATE).and_then(|c| DefaultDate::ALL.into_iter().find(|d| d.code() == c)),
            duration: get(DEFAULT_DURATION)
                .and_then(|v| v.parse().ok())
                .filter(|m| *m > 0)
                .unwrap_or(defaults_of_tasks.duration),
            timed_reminder: reminder(DEFAULT_TIMED_REMINDER, defaults_of_tasks.timed_reminder),
            all_day_reminder: reminder(DEFAULT_ALL_DAY_REMINDER, defaults_of_tasks.all_day_reminder),
            priority: get(DEFAULT_PRIORITY)
                .and_then(|v| v.parse::<i64>().ok())
                .map(Priority::from_i64)
                .unwrap_or_default(),
            tag: get(DEFAULT_TAG).filter(|t| !t.is_empty()).map(str::to_owned),
            add_to_bottom: get(DEFAULT_ADD_TO) == Some("bottom"),
            overdue_last: get(OVERDUE_AT) == Some("bottom"),
        };
        let smart_list_order = get(SMART_LIST_ORDER)
            .map(|s| normalize_order(s, &SmartList::TOP_DRAGGABLE, SmartList::from_code))
            .unwrap_or_else(|| defaults.smart_list_order.clone());
        let sidebar_section_order = get(SIDEBAR_SECTION_ORDER)
            .map(|s| normalize_order(s, &SidebarSection::ALL, SidebarSection::from_code))
            .unwrap_or_else(|| defaults.sidebar_section_order.clone());
        let rail_order = get(RAIL_ORDER)
            .map(|s| normalize_order(s, &RailSection::ALL, RailSection::from_code))
            .unwrap_or_else(|| defaults.rail_order.clone());
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
            smart_list_order,
            sidebar_section_order,
            rail_order,
            ..defaults
        };
        for list in SmartList::ALL {
            if let Some(visibility) = get(&smart_list_key(list)).and_then(Visibility::from_code) {
                preferences.set_visibility(list, visibility);
            }
        }
        Ok(preferences)
    }

    pub fn save_theme_selection(&self, selection: &ThemeSelection) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2), (?3, ?4), (?5, ?6)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [THEME, selection.mode.code(), LIGHT_THEME, selection.light.as_deref().unwrap_or(""),
             DARK_THEME, selection.dark.as_deref().unwrap_or("")],
        )?;
        Ok(())
    }

    /// Edit the current preferences even when another view still has an older snapshot.
    pub fn update_preferences(&self, change: impl FnOnce(&mut Preferences)) -> Result<Preferences> {
        self.in_tx(|| {
            let mut preferences = self.preferences()?;
            change(&mut preferences);
            self.save_preferences(&preferences)?;
            Ok(preferences)
        })
    }

    pub fn save_preferences(&self, preferences: &Preferences) -> Result<()> {
        self.in_tx(|| {
            self.save_theme_selection(&preferences.theme)?;
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
            let smart_order: Vec<&str> = preferences.smart_list_order.iter().map(|l| l.code()).collect();
            self.set_setting(SMART_LIST_ORDER, &smart_order.join(","))?;
            let section_order: Vec<&str> = preferences.sidebar_section_order.iter().map(|s| s.code()).collect();
            self.set_setting(SIDEBAR_SECTION_ORDER, &section_order.join(","))?;
            let rail_order: Vec<&str> = preferences.rail_order.iter().map(|r| r.code()).collect();
            self.set_setting(RAIL_ORDER, &rail_order.join(","))?;
            Ok(())
        })
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
    fn existing_theme_mode_and_new_palettes_survive_unrelated_settings_edits() -> Result<()> {
        let store = Store::open_in_memory()?;
        store.set_setting(THEME, "light")?;
        let mut selection = store.preferences()?.theme;
        assert_eq!(selection, ThemeSelection { mode: ThemeMode::Light, ..Default::default() });
        selection.light = Some("turquoise".into());
        selection.dark = Some("darkpink".into());
        selection.mode = ThemeMode::System;
        store.save_theme_selection(&selection)?;
        store.update_preferences(|p| p.detailed_input = true)?;
        assert_eq!(store.preferences()?.theme, selection);
        Ok(())
    }

    #[test]
    fn roundtrip() -> Result<()> {
        let store = Store::open_in_memory()?;
        let list = store.create_list("Work")?;
        let preferences =
            Preferences {
            theme: ThemeSelection { mode: ThemeMode::System, light: Some("matcha".into()), dark: Some("darkgreen".into()) },
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

        store.save_preferences(&Preferences { date_format: Some(DateFormat::IsoYearMonthDay), ..preferences.clone() })?;
        assert_eq!(store.preferences()?.date_format, Some(DateFormat::IsoYearMonthDay));

        store.save_preferences(&Preferences { default_list: None, ..preferences })?;
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

        // Canonical next_week_date & QuickDate respect week_start.
        assert_eq!(next_week_date(sunday, Weekday::Mon), monday);
        assert_eq!(next_week_date(monday, Weekday::Mon), NaiveDate::from_ymd_opt(2026, 10, 26).unwrap());
        assert_eq!(next_week_date(sunday, Weekday::Sun), NaiveDate::from_ymd_opt(2026, 10, 25).unwrap());
        let saturday = NaiveDate::from_ymd_opt(2026, 10, 17).unwrap();
        assert_eq!(next_week_date(saturday, Weekday::Sun), sunday);
        assert_eq!(QuickDate::Today.resolve(sunday, Weekday::Mon), Some(sunday));
        assert_eq!(QuickDate::Tomorrow.resolve(sunday, Weekday::Mon), Some(monday));
        assert_eq!(QuickDate::NextWeek.resolve(sunday, Weekday::Mon), Some(monday));
        assert_eq!(QuickDate::NoDate.resolve(sunday, Weekday::Mon), None);
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

    #[test]
    fn sidebar_order_moves_and_persists() -> Result<()> {
        let store = Store::open_in_memory()?;
        let mut prefs = Preferences::default();
        assert_eq!(prefs.smart_list_order, SmartList::TOP_DRAGGABLE);
        assert_eq!(prefs.sidebar_section_order, SidebarSection::ALL);
        assert_eq!(prefs.rail_order, RailSection::ALL);

        prefs.move_smart_list(SmartList::Today, SmartList::Inbox, false);
        assert_eq!(prefs.smart_list_order[..3], [SmartList::Today, SmartList::Inbox, SmartList::All]);
        prefs.move_smart_list(SmartList::Inbox, SmartList::Tomorrow, true);
        assert_eq!(
            prefs.smart_list_order[..4],
            [SmartList::Today, SmartList::All, SmartList::Tomorrow, SmartList::Inbox]
        );

        prefs.move_sidebar_section(SidebarSection::Filters, SidebarSection::Lists, false);
        assert_eq!(prefs.sidebar_section_order, [SidebarSection::Filters, SidebarSection::Lists, SidebarSection::Tags]);

        prefs.move_rail_section(RailSection::Countdown, RailSection::Tasks, false);
        assert_eq!(prefs.rail_order[0], RailSection::Countdown);
        assert_eq!(prefs.rail_order[1], RailSection::Tasks);

        store.save_preferences(&prefs)?;
        let loaded = store.preferences()?;
        assert_eq!(loaded.smart_list_order, prefs.smart_list_order);
        assert_eq!(loaded.sidebar_section_order, prefs.sidebar_section_order);
        assert_eq!(loaded.rail_order, prefs.rail_order);

        // Unknown / duplicate entries are ignored and missing items append in default order.
        store.set_setting(SIDEBAR_SECTION_ORDER, "tags,bogus,tags")?;
        let recovered = store.preferences()?;
        assert_eq!(
            recovered.sidebar_section_order,
            [SidebarSection::Tags, SidebarSection::Lists, SidebarSection::Filters]
        );
        Ok(())
    }
}
