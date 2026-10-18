//! The calendar's View Options, as the original's: how tasks are colored and drawn, and which
//! days and items the calendar shows.

use anyhow::Result;
use chrono::{DateTime, Datelike, Local, NaiveDate, Weekday};

use crate::countdown::Countdown;
use crate::focus::FocusSession;
use crate::model::Task;
use crate::store::Store;

/// View Options → Color → "Color By".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorBy {
    /// Each task in its list's color.
    #[default]
    List,
    /// In its first tag's color.
    Tag,
    /// In its priority's color.
    Priority,
}

impl ColorBy {
    pub const ALL: [ColorBy; 3] = [ColorBy::List, ColorBy::Tag, ColorBy::Priority];

    fn code(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Tag => "tag",
            Self::Priority => "priority",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.code() == code)
    }
}

/// The Color page's Habit tab: habits in the theme's color, one fixed color, or each in its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HabitColor {
    #[default]
    Theme,
    Fixed,
    Habit,
}

/// The Color page's Countdown tab: one fixed color (orange by default), or each its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CountdownColor {
    #[default]
    Fixed,
    Countdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalendarOptions {
    pub color_by: ColorBy,
    /// Style → Item Style: Modern (a colored edge on a light tint) rather than Classic (filled).
    pub modern: bool,
    /// Style → Heatmap: the Year view's days tinted by how many items they have.
    pub heatmap: bool,
    pub show_weekends: bool,
    pub show_completed: bool,
    /// "Show All Repeat Cycle": a repeating task on each of its coming dates, not only the next.
    pub show_repeats: bool,
    pub show_countdowns: bool,
    /// Today's habits on today, and focus records on the days they were made.
    pub show_habits: bool,
    pub show_focus: bool,
    pub habit_color: HabitColor,
    pub countdown_color: CountdownColor,
    /// The fixed colors, as indexes into the palette the Color page offers.
    pub habit_fixed: u8,
    pub countdown_fixed: u8,
}

impl Default for CalendarOptions {
    /// The original's defaults.
    fn default() -> Self {
        Self {
            color_by: ColorBy::List,
            modern: false,
            heatmap: false,
            show_weekends: true,
            show_completed: true,
            show_repeats: true,
            show_countdowns: true,
            show_habits: false,
            show_focus: false,
            habit_color: HabitColor::Theme,
            countdown_color: CountdownColor::Fixed,
            habit_fixed: 0,
            countdown_fixed: 0,
        }
    }
}

const COLOR_BY: &str = "calendar_color_by";
const HABIT_COLOR: &str = "calendar_habit_color";
const COUNTDOWN_COLOR: &str = "calendar_countdown_color";
/// The switches, in the order of `CalendarOptions::flags`.
const FLAGS: [&str; 8] = [
    "calendar_modern",
    "calendar_heatmap",
    "calendar_weekends",
    "calendar_completed",
    "calendar_repeats",
    "calendar_countdowns",
    "calendar_habits",
    "calendar_focus",
];

impl CalendarOptions {
    fn flags(&mut self) -> [&mut bool; 8] {
        [
            &mut self.modern,
            &mut self.heatmap,
            &mut self.show_weekends,
            &mut self.show_completed,
            &mut self.show_repeats,
            &mut self.show_countdowns,
            &mut self.show_habits,
            &mut self.show_focus,
        ]
    }

    /// Focus records shown on a day: those started on it, in local time, in order.
    pub fn focus_on<'a>(&self, sessions: &'a [FocusSession], date: NaiveDate) -> Vec<&'a FocusSession> {
        if !self.show_focus {
            return Vec::new();
        }
        let mut on: Vec<&FocusSession> = sessions
            .iter()
            .filter(|s| DateTime::from_timestamp_millis(s.started_at).is_some_and(|t| t.with_timezone(&Local).date_naive() == date))
            .collect();
        on.sort_by_key(|s| s.started_at);
        on
    }

    /// Whether a day has a column: weekends go when they are not shown.
    pub fn shows_day(&self, date: NaiveDate) -> bool {
        self.show_weekends || !matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
    }

    /// Whether a task shows on a day: on the days its date covers, and — for an open repeating
    /// task, with "Show All Repeat Cycle" — on each later date it comes back on.
    pub fn shows_task(&self, task: &Task, date: NaiveDate) -> bool {
        if task.is_deleted() || (!self.show_completed && task.is_completed()) {
            return false;
        }
        if task.occurs_on(date) {
            return true;
        }
        let (Some(repeat), Some(mut due)) = (&task.repeat, task.due_date) else { return false };
        if !self.show_repeats || task.is_closed() || date < due {
            return false;
        }
        while due < date {
            let next = repeat.next_after(due);
            if next <= due {
                return false;
            }
            due = next;
        }
        due == date
    }

    /// Whether a countdown shows on a day.
    pub fn shows_countdown(&self, countdown: &Countdown, date: NaiveDate) -> bool {
        self.show_countdowns && !crate::agenda::agenda(&[], std::slice::from_ref(countdown), date, date).is_empty()
    }
}

impl Store {
    pub fn calendar_options(&self) -> Result<CalendarOptions> {
        let mut options = CalendarOptions::default();
        if let Some(color_by) = self.setting(COLOR_BY)?.and_then(|c| ColorBy::from_code(&c)) {
            options.color_by = color_by;
        }
        for (key, flag) in FLAGS.iter().zip(options.flags()) {
            if let Some(value) = self.setting(key)? {
                *flag = value == "1";
            }
        }
        // "theme", "habit" or "fixed:<palette index>"; "countdown" or "fixed:<index>".
        if let Some(code) = self.setting(HABIT_COLOR)? {
            match code.as_str() {
                "theme" => options.habit_color = HabitColor::Theme,
                "habit" => options.habit_color = HabitColor::Habit,
                fixed => {
                    if let Some(index) = fixed.strip_prefix("fixed:").and_then(|i| i.parse().ok()) {
                        (options.habit_color, options.habit_fixed) = (HabitColor::Fixed, index);
                    }
                }
            }
        }
        if let Some(code) = self.setting(COUNTDOWN_COLOR)? {
            match code.as_str() {
                "countdown" => options.countdown_color = CountdownColor::Countdown,
                fixed => {
                    if let Some(index) = fixed.strip_prefix("fixed:").and_then(|i| i.parse().ok()) {
                        options.countdown_fixed = index;
                    }
                }
            }
        }
        Ok(options)
    }

    pub fn save_calendar_options(&self, options: &CalendarOptions) -> Result<()> {
        self.set_setting(COLOR_BY, options.color_by.code())?;
        let habit = match options.habit_color {
            HabitColor::Theme => "theme".to_owned(),
            HabitColor::Habit => "habit".to_owned(),
            HabitColor::Fixed => format!("fixed:{}", options.habit_fixed),
        };
        self.set_setting(HABIT_COLOR, &habit)?;
        let countdown = match options.countdown_color {
            CountdownColor::Countdown => "countdown".to_owned(),
            CountdownColor::Fixed => format!("fixed:{}", options.countdown_fixed),
        };
        self.set_setting(COUNTDOWN_COLOR, &countdown)?;
        let mut options = *options;
        for (key, flag) in FLAGS.iter().zip(options.flags()) {
            self.set_setting(key, if *flag { "1" } else { "0" })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recurrence::{Frequency, Recurrence};

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).unwrap()
    }

    #[test]
    fn options_are_saved() -> Result<()> {
        let store = Store::open_in_memory()?;
        assert_eq!(store.calendar_options()?, CalendarOptions::default());
        let options = CalendarOptions { color_by: ColorBy::Priority, modern: true, show_weekends: false, ..Default::default() };
        store.save_calendar_options(&options)?;
        assert_eq!(store.calendar_options()?, options);
        let colors = CalendarOptions {
            show_habits: true,
            habit_color: HabitColor::Fixed,
            habit_fixed: 3,
            countdown_color: CountdownColor::Countdown,
            ..Default::default()
        };
        store.save_calendar_options(&colors)?;
        assert_eq!(store.calendar_options()?, colors);
        Ok(())
    }

    #[test]
    fn focus_records_on_their_day() {
        let at = |day: u32, hour: u32| {
            NaiveDate::from_ymd_opt(2026, 10, day).unwrap().and_hms_opt(hour, 0, 0).unwrap().and_local_timezone(Local).unwrap().timestamp_millis()
        };
        let session = |day: u32, hour: u32| FocusSession {
            id: uuid::Uuid::now_v7(),
            task_id: None,
            title: format!("{day} {hour}"),
            started_at: at(day, hour),
            ended_at: at(day, hour) + 25 * 60_000,
            stopwatch: false,
            note: String::new(),
        };
        let sessions = [session(15, 14), session(15, 9), session(16, 9)];
        let shown = CalendarOptions { show_focus: true, ..Default::default() };
        assert_eq!(shown.focus_on(&sessions, d(10, 15)).iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["15 9", "15 14"]);
        assert!(CalendarOptions::default().focus_on(&sessions, d(10, 15)).is_empty(), "off by default, as the original's");
    }

    #[test]
    fn weekends_and_tasks() -> Result<()> {
        let store = Store::open_in_memory()?;
        let options = CalendarOptions { show_weekends: false, ..Default::default() };
        assert!(!options.shows_day(d(10, 24)), "a Saturday");
        assert!(options.shows_day(d(10, 23)));

        let weekly = store.add_task("Weekly")?;
        store.set_due(weekly.id, Some(d(10, 19)), None)?;
        store.set_repeat(weekly.id, Some(Recurrence::new(Frequency::Weekly, 1)), d(10, 19))?;
        let done = store.add_task("Done")?;
        store.set_due(done.id, Some(d(10, 19)), None)?;
        store.set_completed(done.id, true)?;
        let tasks = store.tasks()?;
        let task = |title: &str| tasks.iter().find(|t| t.title == title).unwrap();

        let all = CalendarOptions::default();
        assert!(all.shows_task(task("Weekly"), d(10, 19)));
        assert!(all.shows_task(task("Weekly"), d(10, 26)), "the next cycle");
        assert!(!all.shows_task(task("Weekly"), d(10, 27)));
        let next_only = CalendarOptions { show_repeats: false, ..Default::default() };
        assert!(!next_only.shows_task(task("Weekly"), d(10, 26)));
        assert!(all.shows_task(task("Done"), d(10, 19)));
        let open_only = CalendarOptions { show_completed: false, ..Default::default() };
        assert!(!open_only.shows_task(task("Done"), d(10, 19)));
        Ok(())
    }
}
