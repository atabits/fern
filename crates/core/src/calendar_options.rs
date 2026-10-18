//! The calendar's View Options, as the original's: how tasks are colored and drawn, and which
//! days and items the calendar shows.

use anyhow::Result;
use chrono::{Datelike, NaiveDate, Weekday};

use crate::countdown::Countdown;
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
        }
    }
}

const COLOR_BY: &str = "calendar_color_by";
/// The switches, in the order of `CalendarOptions::flags`.
const FLAGS: [&str; 6] = [
    "calendar_modern",
    "calendar_heatmap",
    "calendar_weekends",
    "calendar_completed",
    "calendar_repeats",
    "calendar_countdowns",
];

impl CalendarOptions {
    fn flags(&mut self) -> [&mut bool; 6] {
        [
            &mut self.modern,
            &mut self.heatmap,
            &mut self.show_weekends,
            &mut self.show_completed,
            &mut self.show_repeats,
            &mut self.show_countdowns,
        ]
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
        Ok(options)
    }

    pub fn save_calendar_options(&self, options: &CalendarOptions) -> Result<()> {
        self.set_setting(COLOR_BY, options.color_by.code())?;
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
        Ok(())
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
