//! Filters: saved smart lists (the original's "Add Filter"), each a set of conditions on lists,
//! tags, the due date, priorities, keywords and the kind of item. An empty condition matches all.

use chrono::{Datelike, Duration, NaiveDate};

use crate::model::{Id, Kind, Priority, Task};

/// The "Date" condition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DateRule {
    #[default]
    All,
    Overdue,
    Today,
    Tomorrow,
    ThisWeek,
    NoDate,
}

impl DateRule {
    pub const ALL: [Self; 6] = [Self::All, Self::Overdue, Self::Today, Self::Tomorrow, Self::ThisWeek, Self::NoDate];

    pub fn key(self) -> &'static str {
        ["all", "overdue", "today", "tomorrow", "week", "none"][self as usize]
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|r| r.key() == key).unwrap_or_default()
    }

    fn matches(self, due: Option<NaiveDate>, today: NaiveDate) -> bool {
        let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
        match (self, due) {
            (Self::All, _) => true,
            (Self::NoDate, due) => due.is_none(),
            (_, None) => false,
            (Self::Overdue, Some(d)) => d < today,
            (Self::Today, Some(d)) => d == today,
            (Self::Tomorrow, Some(d)) => d == today + Duration::days(1),
            (Self::ThisWeek, Some(d)) => d >= monday && d < monday + Duration::weeks(1),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SmartFilter {
    pub id: Id,
    pub name: String,
    /// Lists to include (`None` is the inbox); empty is all.
    pub lists: Vec<Option<Id>>,
    /// Tags to include, any of them; empty is all.
    pub tags: Vec<String>,
    pub date: DateRule,
    /// Priorities to include; empty is all.
    pub priorities: Vec<Priority>,
    /// Words that must all appear in the title or notes, ignoring case.
    pub keywords: String,
    /// Only tasks or only notes; `None` is both.
    pub kind: Option<Kind>,
}

impl SmartFilter {
    /// A new filter that matches everything until conditions are added.
    pub fn new() -> Self {
        Self { id: uuid::Uuid::now_v7(), ..Default::default() }
    }

    /// Whether an open task belongs in the filter.
    pub fn matches(&self, task: &Task, today: NaiveDate) -> bool {
        let text = format!("{}\n{}", task.title, task.notes).to_lowercase();
        !task.is_deleted()
            && !task.is_closed()
            && (self.lists.is_empty() || self.lists.contains(&task.list_id))
            && (self.tags.is_empty()
                || self.tags.iter().any(|tag| task.tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase())))
            && self.date.matches(task.due_date, today)
            && (self.priorities.is_empty() || self.priorities.contains(&task.priority))
            && self.keywords.split_whitespace().all(|word| text.contains(&word.to_lowercase()))
            && self.kind.is_none_or(|kind| task.kind == kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    #[test]
    fn conditions_combine() {
        let store = Store::open_in_memory().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let work = store.create_list("Work").unwrap();
        let report = store.add_task("Quarterly report").unwrap();
        store.set_list(report.id, Some(work.id)).unwrap();
        store.set_priority(report.id, Priority::High).unwrap();
        store.set_due(report.id, Some(today), None).unwrap();
        store.set_tags(report.id, &["Finance".to_owned()]).unwrap();
        let errand = store.add_task("buy milk").unwrap();
        let tasks = store.tasks().unwrap();
        let get = |id: Id| tasks.iter().find(|t| t.id == id).unwrap();

        let all = SmartFilter::default();
        assert!(all.matches(get(report.id), today) && all.matches(get(errand.id), today));

        let urgent_work = SmartFilter {
            lists: vec![Some(work.id)],
            priorities: vec![Priority::High],
            date: DateRule::Today,
            tags: vec!["finance".into()],
            keywords: "REPORT quarterly".into(),
            kind: Some(Kind::Task),
            ..Default::default()
        };
        assert!(urgent_work.matches(get(report.id), today));
        assert!(!urgent_work.matches(get(errand.id), today));
        let inbox_undated = SmartFilter { lists: vec![None], date: DateRule::NoDate, ..Default::default() };
        assert!(inbox_undated.matches(get(errand.id), today) && !inbox_undated.matches(get(report.id), today));
        assert!(DateRule::ThisWeek.matches(Some(NaiveDate::from_ymd_opt(2026, 10, 12).unwrap()), today));
        assert_eq!(DateRule::from_key(DateRule::Overdue.key()), DateRule::Overdue);
    }
}
