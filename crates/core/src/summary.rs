//! The Summary smart list: a report of the tasks of a period, grouped and filtered as in the
//! original's Summary. The desktop renders the groups as text and copies them as Markdown.

use chrono::{Datelike, Duration, Local, Months, NaiveDate, TimeZone, Weekday};

use crate::model::{Id, Priority, Task};

/// The original's Date filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Period {
    Today,
    Tomorrow,
    Yesterday,
    #[default]
    ThisWeek,
    NextWeek,
    LastWeek,
    ThisMonth,
    LastMonth,
}

impl Period {
    pub const ALL: [Period; 8] = [
        Period::Today,
        Period::Tomorrow,
        Period::Yesterday,
        Period::ThisWeek,
        Period::NextWeek,
        Period::LastWeek,
        Period::ThisMonth,
        Period::LastMonth,
    ];

    /// The first and last day of the period.
    pub fn range(self, today: NaiveDate, week_start: Weekday) -> (NaiveDate, NaiveDate) {
        let offset = (today.weekday().num_days_from_monday() + 7 - week_start.num_days_from_monday()) % 7;
        let week = today - Duration::days(offset as i64);
        let month = today.with_day(1).expect("every month has a first day");
        let month_end = |first: NaiveDate| first + Months::new(1) - Duration::days(1);
        let day = |d: NaiveDate| (d, d);
        match self {
            Period::Today => day(today),
            Period::Tomorrow => day(today + Duration::days(1)),
            Period::Yesterday => day(today - Duration::days(1)),
            Period::ThisWeek => (week, week + Duration::days(6)),
            Period::NextWeek => (week + Duration::days(7), week + Duration::days(13)),
            Period::LastWeek => (week - Duration::days(7), week - Duration::days(1)),
            Period::ThisMonth => (month, month_end(month)),
            Period::LastMonth => {
                let first = month - Months::new(1);
                (first, month_end(first))
            }
        }
    }
}

/// The original's Status filter (without "In Progress", which needs checklist progress).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusFilter {
    #[default]
    All,
    Completed,
    Uncompleted,
    WontDo,
}

impl StatusFilter {
    pub const ALL: [StatusFilter; 4] =
        [StatusFilter::All, StatusFilter::Completed, StatusFilter::Uncompleted, StatusFilter::WontDo];
}

/// The original's Grouping (without "By Assignee": there are no shared lists).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Grouping {
    #[default]
    CompletionStatus,
    List,
    CompletionDate,
    TaskDate,
    Tag,
    Priority,
}

impl Grouping {
    pub const ALL: [Grouping; 6] = [
        Grouping::CompletionStatus,
        Grouping::List,
        Grouping::CompletionDate,
        Grouping::TaskDate,
        Grouping::Tag,
        Grouping::Priority,
    ];
}

/// Fields shown on each line, as in the original's "Fields".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Status,
    CompletionTime,
    TaskTime,
    List,
    Tag,
}

impl Field {
    pub const ALL: [Field; 5] = [Field::Status, Field::CompletionTime, Field::TaskTime, Field::List, Field::Tag];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryOptions {
    pub period: Period,
    pub status: StatusFilter,
    pub grouping: Grouping,
    /// The original's default: completion time (and the title, always shown).
    pub fields: Vec<Field>,
}

impl Default for SummaryOptions {
    fn default() -> Self {
        Self {
            period: Period::default(),
            status: StatusFilter::default(),
            grouping: Grouping::default(),
            fields: vec![Field::CompletionTime],
        }
    }
}

/// What a group is about; the desktop turns it into a translated heading.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GroupKey {
    Completed,
    Uncompleted,
    WontDo,
    List(Option<Id>),
    Date(Option<NaiveDate>),
    Tag(Option<String>),
    Priority(Priority),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SummaryGroup {
    pub key: GroupKey,
    pub tasks: Vec<Task>,
}

fn local_date(ms: i64) -> Option<NaiveDate> {
    Local.timestamp_millis_opt(ms).single().map(|t| t.date_naive())
}

/// The day a task belongs to in a summary: when it was closed, or when it is due.
fn report_date(task: &Task) -> Option<NaiveDate> {
    task.completed_at.or(task.wont_do_at).and_then(local_date).or(task.due_date)
}

/// The period's tasks, filtered and grouped.
pub fn summarize(tasks: &[Task], options: &SummaryOptions, today: NaiveDate, week_start: Weekday) -> Vec<SummaryGroup> {
    let (first, last) = options.period.range(today, week_start);
    let mut picked: Vec<&Task> = tasks
        .iter()
        .filter(|t| !t.is_deleted())
        .filter(|t| report_date(t).is_some_and(|d| d >= first && d <= last))
        .filter(|t| match options.status {
            StatusFilter::All => true,
            StatusFilter::Completed => t.is_completed(),
            StatusFilter::Uncompleted => !t.is_closed(),
            StatusFilter::WontDo => t.is_wont_do(),
        })
        .collect();
    // Newest first within a group, as in the original.
    picked.sort_by_key(|t| std::cmp::Reverse((report_date(t), t.completed_at.or(t.wont_do_at))));

    let keys = |task: &Task| -> Vec<GroupKey> {
        match options.grouping {
            Grouping::CompletionStatus if task.is_completed() => vec![GroupKey::Completed],
            Grouping::CompletionStatus if task.is_wont_do() => vec![GroupKey::WontDo],
            Grouping::CompletionStatus => vec![GroupKey::Uncompleted],
            Grouping::List => vec![GroupKey::List(task.list_id)],
            Grouping::CompletionDate => vec![GroupKey::Date(task.completed_at.and_then(local_date))],
            Grouping::TaskDate => vec![GroupKey::Date(task.due_date)],
            Grouping::Tag if task.tags.is_empty() => vec![GroupKey::Tag(None)],
            Grouping::Tag => task.tags.iter().map(|tag| GroupKey::Tag(Some(tag.clone()))).collect(),
            Grouping::Priority => vec![GroupKey::Priority(task.priority)],
        }
    };
    let mut groups: Vec<SummaryGroup> = Vec::new();
    for task in picked {
        for key in keys(task) {
            match groups.iter_mut().find(|g| g.key == key) {
                Some(group) => group.tasks.push(task.clone()),
                None => groups.push(SummaryGroup { key, tasks: vec![task.clone()] }),
            }
        }
    }
    groups.sort_by(|a, b| match (&a.key, &b.key) {
        // Dates newest first and undated last; priorities highest first.
        (GroupKey::Date(x), GroupKey::Date(y)) => match (x, y) {
            (Some(x), Some(y)) => y.cmp(x),
            _ => x.is_none().cmp(&y.is_none()),
        },
        (GroupKey::Priority(x), GroupKey::Priority(y)) => y.cmp(x),
        (GroupKey::Tag(x), GroupKey::Tag(y)) => x.is_none().cmp(&y.is_none()).then(x.cmp(y)),
        (x, y) => x.cmp(y),
    });
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn period_ranges() {
        let sunday = d(2026, 10, 18);
        assert_eq!(Period::ThisWeek.range(sunday, Weekday::Mon), (d(2026, 10, 12), d(2026, 10, 18)));
        assert_eq!(Period::NextWeek.range(sunday, Weekday::Mon), (d(2026, 10, 19), d(2026, 10, 25)));
        assert_eq!(Period::LastWeek.range(sunday, Weekday::Mon), (d(2026, 10, 5), d(2026, 10, 11)));
        assert_eq!(Period::ThisWeek.range(sunday, Weekday::Sun), (d(2026, 10, 18), d(2026, 10, 24)));
        assert_eq!(Period::ThisMonth.range(sunday, Weekday::Mon), (d(2026, 10, 1), d(2026, 10, 31)));
        assert_eq!(Period::LastMonth.range(sunday, Weekday::Mon), (d(2026, 9, 1), d(2026, 9, 30)));
        assert_eq!(Period::Yesterday.range(sunday, Weekday::Mon), (d(2026, 10, 17), d(2026, 10, 17)));
    }

    #[test]
    fn groups_the_periods_tasks() {
        let store = Store::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let done = store.add_task("done").unwrap();
        store.set_completed(done.id, true).unwrap();
        let open = store.add_task("open").unwrap();
        store.set_due(open.id, Some(today), None).unwrap();
        store.set_tags(open.id, &["home".to_owned(), "work".to_owned()]).unwrap();
        let later = store.add_task("next month").unwrap();
        store.set_due(later.id, Some(today + Duration::days(40)), None).unwrap();
        store.add_task("undated, open").unwrap();
        let tasks = store.tasks().unwrap();

        let titles = |g: &SummaryGroup| g.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>();
        let options = SummaryOptions { period: Period::Today, ..Default::default() };
        let groups = summarize(&tasks, &options, today, Weekday::Mon);
        assert_eq!(groups.iter().map(|g| g.key.clone()).collect::<Vec<_>>(), [GroupKey::Completed, GroupKey::Uncompleted]);
        assert_eq!(titles(&groups[0]), ["done"]);
        assert_eq!(titles(&groups[1]), ["open"]);

        let completed = SummaryOptions { status: StatusFilter::Completed, ..options.clone() };
        assert_eq!(summarize(&tasks, &completed, today, Weekday::Mon).len(), 1);

        // A task with two tags is listed under both.
        let by_tag = SummaryOptions { grouping: Grouping::Tag, ..options };
        let groups = summarize(&tasks, &by_tag, today, Weekday::Mon);
        assert_eq!(
            groups.iter().map(|g| g.key.clone()).collect::<Vec<_>>(),
            [GroupKey::Tag(Some("home".into())), GroupKey::Tag(Some("work".into())), GroupKey::Tag(None)]
        );
    }
}
