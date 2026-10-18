//! The Summary smart list: a report of the tasks of a period, grouped and filtered as in the
//! original's Summary. The desktop renders the groups as text and copies them as Markdown.

use chrono::{Datelike, Duration, Local, Months, NaiveDate, TimeZone, Weekday};

use std::collections::{HashMap, HashSet};

use crate::habit::Habit;
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
    /// The original's "Custom": the first and last day, both included.
    Custom(NaiveDate, NaiveDate),
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
            Period::Custom(first, last) => (first.min(last), first.max(last)),
        }
    }

    /// The period right after this one, of the same kind: "Next Period Tasks".
    pub fn next_range(self, today: NaiveDate, week_start: Weekday) -> (NaiveDate, NaiveDate) {
        let (first, last) = self.range(today, week_start);
        match self {
            Period::ThisMonth | Period::LastMonth => {
                let start = first + Months::new(1);
                (start, start + Months::new(1) - Duration::days(1))
            }
            _ => {
                let length = (last - first).num_days() + 1;
                (last + Duration::days(1), last + Duration::days(length))
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
    /// The original's Lists filter: `None` is "All Lists"; `Some(None)` in the set is the Inbox.
    pub lists: Option<Vec<Option<Id>>>,
    /// "Next Period Tasks": add the open tasks due in the following period.
    pub next_period: bool,
    /// The Lists filter's "Habit": add the habits' check-ins of the period.
    pub habits: bool,
}

impl Default for SummaryOptions {
    fn default() -> Self {
        Self {
            period: Period::default(),
            status: StatusFilter::default(),
            grouping: Grouping::default(),
            fields: vec![Field::CompletionTime],
            lists: None,
            next_period: false,
            habits: false,
        }
    }
}

const PERIOD_CODES: [&str; 8] =
    ["today", "tomorrow", "yesterday", "this_week", "next_week", "last_week", "this_month", "last_month"];
const STATUS_CODES: [&str; 4] = ["all", "completed", "uncompleted", "wont_do"];
const GROUPING_CODES: [&str; 6] = ["status", "list", "completion_date", "task_date", "tag", "priority"];
const FIELD_CODES: [&str; 5] = ["status", "completion_time", "task_time", "list", "tag"];
/// Where the options are kept between runs.
const SETTING: &str = "summary_options";

fn code_of<T: PartialEq + Copy>(all: &[T], codes: &[&'static str], value: T) -> &'static str {
    all.iter().position(|v| *v == value).map_or(codes[0], |i| codes[i])
}

fn value_of<T: Copy>(all: &[T], codes: &[&str], code: &str) -> Option<T> {
    codes.iter().position(|c| *c == code).map(|i| all[i])
}

impl SummaryOptions {
    /// A compact `key=value;…` form for the settings table.
    pub fn to_storage(&self) -> String {
        let period = match self.period {
            Period::Custom(first, last) => format!("custom:{first}:{last}"),
            other => code_of(&Period::ALL, &PERIOD_CODES, other).to_owned(),
        };
        let fields: Vec<&str> = self.fields.iter().map(|f| code_of(&Field::ALL, &FIELD_CODES, *f)).collect();
        let lists = self.lists.as_ref().map(|lists| {
            lists.iter().map(|l| l.map_or("inbox".to_owned(), |id| id.to_string())).collect::<Vec<_>>().join(",")
        });
        format!(
            "period={period};status={};grouping={};fields={};lists={};next={};habits={}",
            code_of(&StatusFilter::ALL, &STATUS_CODES, self.status),
            code_of(&Grouping::ALL, &GROUPING_CODES, self.grouping),
            fields.join(","),
            lists.unwrap_or_else(|| "all".to_owned()),
            self.next_period as u8,
            self.habits as u8,
        )
    }

    /// Reads `to_storage`'s form; unknown or missing parts keep their defaults.
    pub fn from_storage(text: &str) -> Self {
        let mut options = Self::default();
        for (key, value) in text.split(';').filter_map(|part| part.split_once('=')) {
            match key {
                "period" => {
                    if let Some(range) = value.strip_prefix("custom:") {
                        let dates: Vec<NaiveDate> = range.split(':').filter_map(|d| d.parse().ok()).collect();
                        if let [first, last] = dates[..] {
                            options.period = Period::Custom(first, last);
                        }
                    } else if let Some(period) = value_of(&Period::ALL, &PERIOD_CODES, value) {
                        options.period = period;
                    }
                }
                "status" => options.status = value_of(&StatusFilter::ALL, &STATUS_CODES, value).unwrap_or_default(),
                "grouping" => options.grouping = value_of(&Grouping::ALL, &GROUPING_CODES, value).unwrap_or_default(),
                "fields" => {
                    options.fields = value.split(',').filter_map(|c| value_of(&Field::ALL, &FIELD_CODES, c)).collect();
                }
                "lists" if value != "all" => {
                    options.lists = Some(
                        value
                            .split(',')
                            .filter_map(|l| if l == "inbox" { Some(None) } else { l.parse().ok().map(Some) })
                            .collect(),
                    );
                }
                "next" => options.next_period = value == "1",
                "habits" => options.habits = value == "1",
                _ => {}
            }
        }
        options
    }
}

impl crate::store::Store {
    /// The Summary's options as last chosen.
    pub fn summary_options(&self) -> anyhow::Result<SummaryOptions> {
        Ok(self.setting(SETTING)?.map(|text| SummaryOptions::from_storage(&text)).unwrap_or_default())
    }

    pub fn save_summary_options(&self, options: &SummaryOptions) -> anyhow::Result<()> {
        self.set_setting(SETTING, &options.to_storage())
    }
}

/// What a group is about; the desktop turns it into a translated heading.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GroupKey {
    Completed,
    Uncompleted,
    WontDo,
    /// "Next Period Tasks", after every other group.
    NextPeriod,
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
        .filter(|t| options.lists.as_ref().is_none_or(|lists| lists.contains(&t.list_id)))
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
    if options.next_period {
        let (first, last) = options.period.next_range(today, week_start);
        let mut next: Vec<Task> = tasks
            .iter()
            .filter(|t| !t.is_deleted() && !t.is_closed())
            .filter(|t| options.lists.as_ref().is_none_or(|lists| lists.contains(&t.list_id)))
            .filter(|t| t.due_date.is_some_and(|d| d >= first && d <= last))
            .cloned()
            .collect();
        next.sort_by_key(|t| (t.due_date, t.due_time));
        if !next.is_empty() {
            groups.push(SummaryGroup { key: GroupKey::NextPeriod, tasks: next });
        }
    }
    groups
}

/// The period's check-in days of each active habit, for the report's Habit section.
pub fn habit_checkins(
    habits: &[Habit],
    checkins: &HashMap<Id, HashSet<NaiveDate>>,
    options: &SummaryOptions,
    today: NaiveDate,
    week_start: Weekday,
) -> Vec<(Habit, usize)> {
    if !options.habits {
        return Vec::new();
    }
    let (first, last) = options.period.range(today, week_start);
    habits
        .iter()
        .filter(|h| !h.archived)
        .map(|h| {
            let days = checkins.get(&h.id).map_or(0, |days| days.iter().filter(|d| **d >= first && **d <= last).count());
            (h.clone(), days)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn habit_check_ins_of_the_period() {
        let store = Store::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let run = store.add_habit("Run").unwrap();
        let days: HashSet<NaiveDate> = [today, today - Duration::days(40)].into();
        let checkins = HashMap::from([(run.id, days)]);
        let habits = store.habits().unwrap();
        let off = SummaryOptions { period: Period::Today, ..Default::default() };
        assert!(habit_checkins(&habits, &checkins, &off, today, Weekday::Mon).is_empty());
        let on = SummaryOptions { habits: true, ..off };
        let rows = habit_checkins(&habits, &checkins, &on, today, Weekday::Mon);
        assert_eq!(rows.iter().map(|(h, n)| (h.name.as_str(), *n)).collect::<Vec<_>>(), [("Run", 1)]);
    }

    #[test]
    fn options_are_remembered() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.summary_options().unwrap(), SummaryOptions::default());
        let list = store.create_list("Work").unwrap();
        let options = SummaryOptions {
            period: Period::Custom(d(2026, 10, 1), d(2026, 10, 20)),
            status: StatusFilter::Uncompleted,
            grouping: Grouping::Tag,
            fields: vec![Field::Status, Field::List],
            lists: Some(vec![None, Some(list.id)]),
            next_period: true,
            habits: true,
        };
        store.save_summary_options(&options).unwrap();
        assert_eq!(store.summary_options().unwrap(), options);
        let defaults = SummaryOptions::default();
        assert_eq!(SummaryOptions::from_storage(&defaults.to_storage()), defaults);
        assert_eq!(SummaryOptions::from_storage("period=nonsense;grouping=list"), SummaryOptions { grouping: Grouping::List, ..defaults });
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
    fn custom_periods_lists_and_next_period() {
        let sunday = d(2026, 10, 18);
        assert_eq!(Period::Custom(d(2026, 10, 20), d(2026, 10, 1)).range(sunday, Weekday::Mon), (d(2026, 10, 1), d(2026, 10, 20)));
        assert_eq!(Period::ThisWeek.next_range(sunday, Weekday::Mon), (d(2026, 10, 19), d(2026, 10, 25)));
        assert_eq!(Period::ThisMonth.next_range(sunday, Weekday::Mon), (d(2026, 11, 1), d(2026, 11, 30)));

        let store = Store::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let work = store.create_list("Work").unwrap();
        let inbox_task = store.add_task("inbox").unwrap();
        store.set_due(inbox_task.id, Some(today), None).unwrap();
        let work_task = store.add_task("work").unwrap();
        store.set_list(work_task.id, Some(work.id)).unwrap();
        store.set_due(work_task.id, Some(today), None).unwrap();
        let tomorrow = store.add_task("tomorrow").unwrap();
        store.set_due(tomorrow.id, Some(today + Duration::days(1)), None).unwrap();
        let tasks = store.tasks().unwrap();

        let only_work = SummaryOptions { period: Period::Today, lists: Some(vec![Some(work.id)]), ..Default::default() };
        let groups = summarize(&tasks, &only_work, today, Weekday::Mon);
        assert_eq!(groups[0].tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["work"]);

        let with_next = SummaryOptions { period: Period::Today, next_period: true, ..Default::default() };
        let groups = summarize(&tasks, &with_next, today, Weekday::Mon);
        let last = groups.last().unwrap();
        assert_eq!((last.key.clone(), last.tasks[0].title.as_str()), (GroupKey::NextPeriod, "tomorrow"));
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
