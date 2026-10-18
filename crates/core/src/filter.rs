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

/// A condition of the Advanced tab's chain, named as the original's "Filter" menu names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Lists,
    Tags,
    Date,
    Priority,
    Include,
    Type,
}

impl Field {
    pub const ALL: [Self; 6] = [Self::Lists, Self::Tags, Self::Date, Self::Priority, Self::Include, Self::Type];

    pub fn key(self) -> &'static str {
        ["lists", "tags", "date", "priority", "include", "type"][self as usize]
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.key() == key)
    }
}

/// The original's "Logic" between two conditions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Join {
    #[default]
    And,
    Or,
}

/// A condition in the Advanced chain: its field, the original's NOT (it must not match), and the
/// logic joining it to the condition before (unused for the first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub join: Join,
    pub field: Field,
    pub not: bool,
}

impl Link {
    pub fn new(join: Join, field: Field) -> Self {
        Self { join, field, not: false }
    }
}

/// Writes a chain as text: `lists and:tags! or:date` (`!` is NOT).
pub fn chain_to_storage(chain: &[Link]) -> String {
    chain
        .iter()
        .enumerate()
        .map(|(i, link)| {
            let join = match (i, link.join) {
                (0, _) => "",
                (_, Join::And) => "and:",
                (_, Join::Or) => "or:",
            };
            format!("{join}{}{}", link.field.key(), if link.not { "!" } else { "" })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reads [`chain_to_storage`]'s text; unknown parts are skipped.
pub fn chain_from_storage(text: &str) -> Vec<Link> {
    text.split_whitespace()
        .filter_map(|part| {
            let (join, rest) = match part.split_once(':') {
                Some(("or", rest)) => (Join::Or, rest),
                Some((_, rest)) => (Join::And, rest),
                None => (Join::And, part),
            };
            let (key, not) = match rest.strip_suffix('!') {
                Some(key) => (key, true),
                None => (rest, false),
            };
            Field::from_key(key).map(|field| Link { join, field, not })
        })
        .collect()
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
    /// "Advanced" logic: a task needs any one of the set conditions instead of all of them.
    /// A chain, when there is one, decides instead.
    pub match_any: bool,
    /// The Advanced tab's chain of conditions with AND and OR between them; AND binds first,
    /// so `a OR b AND c` is `a OR (b AND c)`. Empty uses the conditions with `match_any`.
    pub chain: Vec<Link>,
}

impl SmartFilter {
    /// A new filter that matches everything until conditions are added.
    pub fn new() -> Self {
        Self { id: uuid::Uuid::now_v7(), ..Default::default() }
    }

    /// Whether an open task belongs in the filter.
    pub fn matches(&self, task: &Task, today: NaiveDate) -> bool {
        if task.is_deleted() || task.is_closed() {
            return false;
        }
        let text = format!("{}\n{}", task.title, task.notes).to_lowercase();
        // Each set condition and whether the task meets it; unset ones are left out.
        let conditions: [Option<bool>; 6] = [
            (!self.lists.is_empty()).then(|| self.lists.contains(&task.list_id)),
            (!self.tags.is_empty()).then(|| {
                self.tags.iter().any(|tag| task.tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()))
            }),
            (self.date != DateRule::All).then(|| self.date.matches(task.due_date, today)),
            (!self.priorities.is_empty()).then(|| self.priorities.contains(&task.priority)),
            (!self.keywords.trim().is_empty())
                .then(|| self.keywords.split_whitespace().all(|word| text.contains(&word.to_lowercase()))),
            self.kind.map(|kind| task.kind == kind),
        ];
        if !self.chain.is_empty() {
            // OR splits the chain into groups of AND; a condition left unset matches.
            let mut any = false;
            let mut group = true;
            for (i, link) in self.chain.iter().enumerate() {
                if i > 0 && link.join == Join::Or {
                    any |= group;
                    group = true;
                }
                let met = conditions[link.field as usize].unwrap_or(true);
                group &= met != link.not;
            }
            return any || group;
        }
        let mut set = conditions.into_iter().flatten().peekable();
        if set.peek().is_none() {
            return true;
        }
        if self.match_any { set.any(|met| met) } else { set.all(|met| met) }
    }
}

/// Ready-made Advanced filters for the dialog's "Examples" (the original links to its help page
/// instead). Their names are the app's to translate; the order is fixed.
pub fn examples() -> [SmartFilter; 3] {
    let link = |join, field| Link::new(join, field);
    [
        // Due today and important.
        SmartFilter {
            date: DateRule::Today,
            priorities: vec![Priority::High, Priority::Medium],
            chain: vec![link(Join::And, Field::Date), link(Join::And, Field::Priority)],
            ..SmartFilter::default()
        },
        // Overdue or high priority.
        SmartFilter {
            date: DateRule::Overdue,
            priorities: vec![Priority::High],
            chain: vec![link(Join::And, Field::Date), link(Join::Or, Field::Priority)],
            ..SmartFilter::default()
        },
        // Undated tasks outside the Inbox.
        SmartFilter {
            lists: vec![None],
            date: DateRule::NoDate,
            kind: Some(Kind::Task),
            chain: vec![Link { not: true, ..link(Join::And, Field::Lists) }, link(Join::And, Field::Date), link(Join::And, Field::Type)],
            ..SmartFilter::default()
        },
    ]
}

impl SmartFilter {
    /// The tasks the filter shows, in the given order: the dialog's "Preview" before saving.
    pub fn preview<'a>(&self, tasks: &'a [Task], today: NaiveDate) -> Vec<&'a Task> {
        tasks.iter().filter(|task| self.matches(task, today)).collect()
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
        // "Any": high priority or tagged finance, so the untagged, normal errand is out.
        let any = SmartFilter { priorities: vec![Priority::High], tags: vec!["errands".into()], match_any: true, ..Default::default() };
        assert!(any.matches(get(report.id), today) && !any.matches(get(errand.id), today));
        let all = SmartFilter { match_any: false, ..any.clone() };
        assert!(!all.matches(get(report.id), today));
        assert!(DateRule::ThisWeek.matches(Some(NaiveDate::from_ymd_opt(2026, 10, 12).unwrap()), today));
        assert_eq!(DateRule::from_key(DateRule::Overdue.key()), DateRule::Overdue);

        // The Advanced chain: work list AND NOT tagged finance, OR no date.
        let chain = SmartFilter {
            lists: vec![Some(work.id)],
            tags: vec!["finance".into()],
            date: DateRule::NoDate,
            chain: vec![Link::new(Join::And, Field::Lists), Link { not: true, ..Link::new(Join::And, Field::Tags) }, Link::new(Join::Or, Field::Date)],
            ..Default::default()
        };
        assert!(!chain.matches(get(report.id), today), "in Work but tagged finance, and dated");
        assert!(chain.matches(get(errand.id), today), "undated");
        let without_not = SmartFilter { chain: vec![Link::new(Join::And, Field::Lists), Link::new(Join::And, Field::Tags), Link::new(Join::Or, Field::Date)], ..chain.clone() };
        assert!(without_not.matches(get(report.id), today));
        // AND binds before OR: (undated AND high) OR in Work.
        let precedence = SmartFilter {
            lists: vec![Some(work.id)],
            date: DateRule::NoDate,
            priorities: vec![Priority::High],
            chain: vec![Link::new(Join::And, Field::Date), Link::new(Join::And, Field::Priority), Link::new(Join::Or, Field::Lists)],
            ..Default::default()
        };
        assert!(precedence.matches(get(report.id), today) && !precedence.matches(get(errand.id), today));
        let text = chain_to_storage(&chain.chain);
        assert_eq!(text, "lists and:tags! or:date");
        assert_eq!(chain_from_storage(&text), chain.chain);
        assert_eq!(chain_from_storage("bogus or:type"), [Link::new(Join::Or, Field::Type)]);
    }

    #[test]
    fn examples_and_preview() {
        let store = Store::open_in_memory().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let work = store.create_list("Work").unwrap();
        let late = store.add_task("late").unwrap();
        store.set_due(late.id, Some(today - Duration::days(2)), None).unwrap();
        let urgent = store.add_task("urgent").unwrap();
        store.set_priority(urgent.id, Priority::High).unwrap();
        store.set_due(urgent.id, Some(today), None).unwrap();
        let someday = store.add_task("someday").unwrap();
        store.set_list(someday.id, Some(work.id)).unwrap();
        let inbox_undated = store.add_task("inbox").unwrap();
        let tasks = store.tasks().unwrap();
        let titles = |filter: &SmartFilter| {
            let mut titles: Vec<&str> = filter.preview(&tasks, today).iter().map(|t| t.title.as_str()).collect();
            titles.sort();
            titles
        };
        let [today_important, overdue_or_high, undated_elsewhere] = examples();
        assert_eq!(titles(&today_important), ["urgent"]);
        assert_eq!(titles(&overdue_or_high), ["late", "urgent"]);
        assert_eq!(titles(&undated_elsewhere), ["someday"]);
        assert!(!titles(&SmartFilter::default()).is_empty() && titles(&SmartFilter::default()).contains(&inbox_undated.title.as_str()));
    }
}
