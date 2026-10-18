//! The search's Task filter, as the original's: the lists, tags, date and priorities the found
//! tasks must have.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::model::{Id, Priority, Task};

/// The Tag dropdown: all tasks, tasks with any tag, tasks with none, or with one of these tags.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TagRule {
    #[default]
    All,
    WithTags,
    NoTags,
    Any(Vec<String>),
}

/// The Date dropdown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DateRange {
    #[default]
    All,
    ThisWeek,
    NextWeek,
    LastWeek,
    ThisMonth,
    LastMonth,
    /// From the first day through the last.
    Custom(NaiveDate, NaiveDate),
}

impl DateRange {
    /// The days the range covers, weeks starting on `week_start`; `None` for All.
    pub fn days(self, today: NaiveDate, week_start: Weekday) -> Option<(NaiveDate, NaiveDate)> {
        let back = (7 + today.weekday().num_days_from_monday() - week_start.num_days_from_monday()) % 7;
        let week = today - Duration::days(back as i64);
        let month = today.with_day(1).expect("every month has a first day");
        let next_month = |d: NaiveDate| if d.month() == 12 { NaiveDate::from_ymd_opt(d.year() + 1, 1, 1) } else { NaiveDate::from_ymd_opt(d.year(), d.month() + 1, 1) };
        Some(match self {
            Self::All => return None,
            Self::ThisWeek => (week, week + Duration::days(6)),
            Self::NextWeek => (week + Duration::days(7), week + Duration::days(13)),
            Self::LastWeek => (week - Duration::days(7), week - Duration::days(1)),
            Self::ThisMonth => (month, next_month(month)? - Duration::days(1)),
            Self::LastMonth => {
                let last = month - Duration::days(1);
                (last.with_day(1)?, last)
            }
            Self::Custom(from, until) => (from.min(until), from.max(until)),
        })
    }
}

/// The Task filter's choices; empty lists and priorities mean all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskFilter {
    /// Lists by id, `None` being the Inbox.
    pub lists: Vec<Option<Id>>,
    pub tags: TagRule,
    pub date: DateRange,
    pub priorities: Vec<Priority>,
}

impl TaskFilter {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn matches(&self, task: &Task, today: NaiveDate, week_start: Weekday) -> bool {
        if !self.lists.is_empty() && !self.lists.contains(&task.list_id) {
            return false;
        }
        let tagged = match &self.tags {
            TagRule::All => true,
            TagRule::WithTags => !task.tags.is_empty(),
            TagRule::NoTags => task.tags.is_empty(),
            TagRule::Any(tags) => task.tags.iter().any(|t| tags.iter().any(|w| w.to_lowercase() == t.to_lowercase())),
        };
        if !tagged || (!self.priorities.is_empty() && !self.priorities.contains(&task.priority)) {
            return false;
        }
        match self.date.days(today, week_start) {
            None => true,
            Some((from, until)) => task.due_date.is_some_and(|due| due >= from && due <= until),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).unwrap()
    }

    #[test]
    fn ranges_as_the_original_names_them() {
        let today = d(10, 18);
        assert_eq!(DateRange::ThisWeek.days(today, Weekday::Mon), Some((d(10, 12), d(10, 18))), "This Week (Oct 12 - Oct 18)");
        assert_eq!(DateRange::NextWeek.days(today, Weekday::Mon), Some((d(10, 19), d(10, 25))));
        assert_eq!(DateRange::LastWeek.days(today, Weekday::Mon), Some((d(10, 5), d(10, 11))));
        assert_eq!(DateRange::ThisWeek.days(today, Weekday::Sun), Some((d(10, 18), d(10, 24))));
        assert_eq!(DateRange::ThisMonth.days(today, Weekday::Mon), Some((d(10, 1), d(10, 31))));
        assert_eq!(DateRange::LastMonth.days(today, Weekday::Mon), Some((d(9, 1), d(9, 30))));
        assert_eq!(DateRange::LastMonth.days(d(1, 5), Weekday::Mon).map(|r| r.0), NaiveDate::from_ymd_opt(2025, 12, 1));
    }

    #[test]
    fn filters_tasks() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let today = d(10, 18);
        let work = store.create_list("Work")?;
        let report = store.add_task("Report")?;
        store.set_list(report.id, Some(work.id))?;
        store.set_priority(report.id, Priority::High)?;
        store.set_due(report.id, Some(d(10, 20)), None)?;
        let milk = store.add_task("Milk")?;
        store.set_tags(milk.id, &["home".to_owned()])?;
        let tasks = store.tasks()?;
        let found = |filter: &TaskFilter| tasks.iter().filter(|t| filter.matches(t, today, Weekday::Mon)).map(|t| t.title.as_str()).collect::<Vec<_>>();
        assert_eq!(found(&TaskFilter { lists: vec![None], ..Default::default() }), ["Milk"], "the Inbox");
        assert_eq!(found(&TaskFilter { tags: TagRule::NoTags, ..Default::default() }), ["Report"]);
        assert_eq!(found(&TaskFilter { tags: TagRule::Any(vec!["Home".into()]), ..Default::default() }), ["Milk"]);
        assert_eq!(found(&TaskFilter { date: DateRange::NextWeek, ..Default::default() }), ["Report"]);
        assert_eq!(found(&TaskFilter { priorities: vec![Priority::None], ..Default::default() }), ["Milk"]);
        assert!(TaskFilter::default().is_empty());
        Ok(())
    }
}
