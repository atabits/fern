//! Task statistics for the Statistics page's Overview and Task tabs.

use chrono::{Datelike, Duration, Local, Months, NaiveDate, TimeZone, Weekday};

use crate::focus::FocusSession;
use crate::model::{Id, Task};

fn completed_on(task: &Task) -> Option<NaiveDate> {
    task.completed_at.and_then(|ms| Local.timestamp_millis_opt(ms).single()).map(|t| t.date_naive())
}

/// Tasks completed on each of `days` days from `start`.
pub fn completed_by_day(tasks: &[Task], start: NaiveDate, days: usize) -> Vec<usize> {
    let mut out = vec![0; days];
    for day in tasks.iter().filter(|t| !t.is_deleted()).filter_map(completed_on) {
        let offset = (day - start).num_days();
        if (0..days as i64).contains(&offset) {
            out[offset as usize] += 1;
        }
    }
    out
}

/// The day's tasks: completed that day, or due that day and still open.
fn tasks_of_day(tasks: &[Task], day: NaiveDate) -> impl Iterator<Item = &Task> {
    tasks.iter().filter(move |t| {
        !t.is_deleted()
            && !t.is_wont_do()
            && (completed_on(t) == Some(day) || (!t.is_completed() && t.due_date == Some(day)))
    })
}

/// The share of the day's tasks that were completed, in whole percent; `None` without any.
pub fn completion_rate(tasks: &[Task], day: NaiveDate) -> Option<u32> {
    let (done, total) =
        tasks_of_day(tasks, day).fold((0, 0), |(done, total), t| (done + t.is_completed() as u32, total + 1));
    (total > 0).then(|| done * 100 / total)
}

/// Tasks completed on `day` per list (`None` is the inbox), most first.
pub fn completed_by_list(tasks: &[Task], day: NaiveDate) -> Vec<(Option<Id>, usize)> {
    let mut counts: Vec<(Option<Id>, usize)> = Vec::new();
    for task in tasks.iter().filter(|t| !t.is_deleted() && completed_on(t) == Some(day)) {
        match counts.iter_mut().find(|(list, _)| *list == task.list_id) {
            Some((_, n)) => *n += 1,
            None => counts.push((task.list_id, 1)),
        }
    }
    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    counts
}

/// The days since the first task was added, counting today: "N Days" in the overview.
pub fn days_using(tasks: &[Task], today: NaiveDate) -> i64 {
    tasks
        .iter()
        .filter_map(|t| Local.timestamp_millis_opt(t.created_at).single())
        .map(|t| t.date_naive())
        .min()
        .map_or(0, |first| (today - first).num_days() + 1)
}

/// The previous day, for "from yesterday".
pub fn yesterday(day: NaiveDate) -> NaiveDate {
    day - Duration::days(1)
}

/// The period of each point on the Overview's curves: the original's Day / Week / Month switch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bucket {
    #[default]
    Day,
    Week,
    Month,
}

impl Bucket {
    pub const ALL: [Bucket; 3] = [Bucket::Day, Bucket::Week, Bucket::Month];
}

/// The last `count` periods up to the one containing `today`, oldest first, as (first, last) days.
pub fn buckets(bucket: Bucket, today: NaiveDate, week_start: Weekday, count: usize) -> Vec<(NaiveDate, NaiveDate)> {
    (0..count as i64)
        .rev()
        .map(|back| match bucket {
            Bucket::Day => {
                let day = today - Duration::days(back);
                (day, day)
            }
            Bucket::Week => {
                let offset = (today.weekday().num_days_from_monday() + 7 - week_start.num_days_from_monday()) % 7;
                let first = today - Duration::days(offset as i64) - Duration::weeks(back);
                (first, first + Duration::days(6))
            }
            Bucket::Month => {
                let first = today.with_day(1).expect("every month has a first day") - Months::new(back as u32);
                (first, first + Months::new(1) - Duration::days(1))
            }
        })
        .collect()
}

/// Tasks completed in each period.
pub fn completed_in(tasks: &[Task], periods: &[(NaiveDate, NaiveDate)]) -> Vec<usize> {
    let days: Vec<NaiveDate> = tasks.iter().filter(|t| !t.is_deleted()).filter_map(completed_on).collect();
    periods.iter().map(|(first, last)| days.iter().filter(|d| *d >= first && *d <= last).count()).collect()
}

/// The completion rate of each period, in whole percent: completed of the period's tasks.
pub fn completion_rate_in(tasks: &[Task], periods: &[(NaiveDate, NaiveDate)]) -> Vec<u32> {
    periods
        .iter()
        .map(|(first, last)| {
            let (done, total) = tasks
                .iter()
                .filter(|t| !t.is_deleted() && !t.is_wont_do())
                .filter(|t| {
                    let day = completed_on(t).or(if t.is_completed() { None } else { t.due_date });
                    day.is_some_and(|d| d >= *first && d <= *last)
                })
                .fold((0, 0), |(done, total), t| (done + t.is_completed() as u32, total + 1));
            if total == 0 { 0 } else { done * 100 / total }
        })
        .collect()
}

/// Focus sessions and focused minutes in each period.
pub fn focus_in(sessions: &[FocusSession], periods: &[(NaiveDate, NaiveDate)]) -> Vec<(usize, i64)> {
    periods
        .iter()
        .map(|(first, last)| {
            sessions
                .iter()
                .filter(|s| {
                    let day = s.start_local().date_naive();
                    day >= *first && day <= *last
                })
                .fold((0, 0), |(count, minutes), s| (count + 1, minutes + (s.ended_at - s.started_at) / 60_000))
        })
        .collect()
}

/// "You are more productive than N%": the share of earlier days, since the first task, on which
/// fewer tasks were completed than today. The original compares with other people; Fern, being
/// self-hosted, compares with your own history.
pub fn more_productive_than(tasks: &[Task], today: NaiveDate) -> u32 {
    let days = days_using(tasks, today) - 1;
    if days <= 0 {
        return 0;
    }
    let first = today - Duration::days(days);
    let counts = completed_by_day(tasks, first, days as usize + 1);
    let today_count = counts[days as usize];
    let fewer = counts[..days as usize].iter().filter(|c| **c < today_count).count();
    (fewer * 100 / days as usize) as u32
}

/// The Task tab's "Completion Rate Distribution": how the period's tasks ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompletionKinds {
    /// Completed after the due date.
    pub overdue: usize,
    /// Completed by the due date.
    pub on_time: usize,
    /// Completed without a due date.
    pub undated: usize,
    /// Due in the period and still open.
    pub uncompleted: usize,
}

impl CompletionKinds {
    pub fn total(&self) -> usize {
        self.overdue + self.on_time + self.undated + self.uncompleted
    }
}

pub fn completion_kinds(tasks: &[Task], first: NaiveDate, last: NaiveDate) -> CompletionKinds {
    let mut kinds = CompletionKinds::default();
    for task in tasks.iter().filter(|t| !t.is_deleted() && !t.is_wont_do()) {
        match (completed_on(task), task.due_date) {
            (Some(day), _) if day < first || day > last => {}
            (Some(_), None) => kinds.undated += 1,
            (Some(day), Some(due)) if day <= due => kinds.on_time += 1,
            (Some(_), Some(_)) => kinds.overdue += 1,
            (None, Some(due)) if !task.is_completed() && due >= first && due <= last => kinds.uncompleted += 1,
            (None, _) => {}
        }
    }
    kinds
}

/// Tasks completed between `first` and `last` per list (`None` is the inbox), most first.
pub fn completed_by_list_in(tasks: &[Task], first: NaiveDate, last: NaiveDate) -> Vec<(Option<Id>, usize)> {
    let mut counts: Vec<(Option<Id>, usize)> = Vec::new();
    for task in tasks.iter().filter(|t| !t.is_deleted() && completed_on(t).is_some_and(|d| d >= first && d <= last)) {
        match counts.iter_mut().find(|(list, _)| *list == task.list_id) {
            Some((_, n)) => *n += 1,
            None => counts.push((task.list_id, 1)),
        }
    }
    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    counts
}

/// Tasks completed between `first` and `last` per tag (`None`: without tags), most first; a
/// task with several tags counts for each.
pub fn completed_by_tag_in(tasks: &[Task], first: NaiveDate, last: NaiveDate) -> Vec<(Option<String>, usize)> {
    let mut counts: Vec<(Option<String>, usize)> = Vec::new();
    for task in tasks.iter().filter(|t| !t.is_deleted() && completed_on(t).is_some_and(|d| d >= first && d <= last)) {
        let keys: Vec<Option<String>> =
            if task.tags.is_empty() { vec![None] } else { task.tags.iter().cloned().map(Some).collect() };
        for key in keys {
            match counts.iter_mut().find(|(tag, _)| *tag == key) {
                Some((_, n)) => *n += 1,
                None => counts.push((key, 1)),
            }
        }
    }
    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    counts
}

/// Points for "My Achievement Score". The original does not publish its rules, so Fern uses its
/// own: a completed task is worth 2 points and 1 more when finished by its due date, a focus
/// session 2 points and a habit check-in 1.
pub const TASK_POINTS: i64 = 2;
pub const ON_TIME_BONUS: i64 = 1;
pub const FOCUS_POINTS: i64 = 2;
pub const CHECKIN_POINTS: i64 = 1;

/// Points earned on each day, keyed by the day they were earned.
fn points(tasks: &[Task], sessions: &[FocusSession], checkins: &[NaiveDate]) -> Vec<(NaiveDate, i64)> {
    let tasks = tasks.iter().filter(|t| !t.is_deleted()).filter_map(|t| {
        let day = completed_on(t)?;
        let on_time = t.due_date.is_some_and(|due| day <= due);
        Some((day, TASK_POINTS + if on_time { ON_TIME_BONUS } else { 0 }))
    });
    let focus = sessions.iter().map(|s| (s.start_local().date_naive(), FOCUS_POINTS));
    let habits = checkins.iter().map(|day| (*day, CHECKIN_POINTS));
    tasks.chain(focus).chain(habits).collect()
}

/// The score at the end of each of `days` days from `start`: everything earned up to that day.
pub fn achievement_scores(
    tasks: &[Task],
    sessions: &[FocusSession],
    checkins: &[NaiveDate],
    start: NaiveDate,
    days: usize,
) -> Vec<i64> {
    let points = points(tasks, sessions, checkins);
    (0..days as i64)
        .map(|i| {
            let day = start + Duration::days(i);
            points.iter().filter(|(d, _)| *d <= day).map(|(_, p)| p).sum()
        })
        .collect()
}

/// The level shown on the score's badge: each level needs more points than the last
/// (level 2 at 50, 3 at 150, 4 at 300, …).
pub fn achievement_level(score: i64) -> u32 {
    let mut level = 1;
    let mut needed = 50;
    let mut total = 0;
    while score >= total + needed {
        total += needed;
        needed += 50;
        level += 1;
    }
    level
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    #[test]
    fn task_statistics() {
        let store = Store::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let done = store.add_task("done").unwrap();
        store.set_completed(done.id, true).unwrap();
        let due_today = store.add_task("due today").unwrap();
        store.set_due(due_today.id, Some(today), None).unwrap();
        let list = store.create_list("Home").unwrap();
        let home = store.add_task("home chore").unwrap();
        store.set_list(home.id, Some(list.id)).unwrap();
        store.set_completed(home.id, true).unwrap();
        let tasks = store.tasks().unwrap();

        assert_eq!(completed_by_day(&tasks, yesterday(today), 2), [0, 2]);
        // Two completed today, one open task due today.
        assert_eq!(completion_rate(&tasks, today), Some(66));
        assert_eq!(completion_rate(&tasks, yesterday(today)), None);
        let by_list = completed_by_list(&tasks, today);
        assert_eq!(by_list.len(), 2);
        assert!(by_list.contains(&(Some(list.id), 1)) && by_list.contains(&(None, 1)));
        assert_eq!(days_using(&tasks, today), 1);
    }

    #[test]
    fn achievement_score() {
        let store = Store::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let on_time = store.add_task("on time").unwrap();
        store.set_due(on_time.id, Some(today), None).unwrap();
        store.set_completed(on_time.id, true).unwrap();
        let late = store.add_task("late").unwrap();
        store.set_due(late.id, Some(yesterday(today)), None).unwrap();
        store.set_completed(late.id, true).unwrap();
        store.add_task("open").unwrap();
        let tasks = store.tasks().unwrap();
        let checkins = [yesterday(today), today];

        // Yesterday: one check-in. Today: 3 + 2 for the tasks and one more check-in.
        assert_eq!(achievement_scores(&tasks, &[], &checkins, yesterday(today), 2), [1, 1 + 3 + 2 + 1]);
        assert_eq!(achievement_level(0), 1);
        assert_eq!(achievement_level(49), 1);
        assert_eq!(achievement_level(50), 2);
        assert_eq!(achievement_level(150), 3);
        assert_eq!(achievement_level(299), 3);
        assert_eq!(achievement_level(300), 4);
    }

    #[test]
    fn periods_and_productivity() {
        let sunday = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let d = |m, day| NaiveDate::from_ymd_opt(2026, m, day).unwrap();
        assert_eq!(buckets(Bucket::Day, sunday, Weekday::Mon, 2), [(d(10, 17), d(10, 17)), (sunday, sunday)]);
        assert_eq!(buckets(Bucket::Week, sunday, Weekday::Mon, 2), [(d(10, 5), d(10, 11)), (d(10, 12), d(10, 18))]);
        assert_eq!(buckets(Bucket::Month, sunday, Weekday::Mon, 2), [(d(9, 1), d(9, 30)), (d(10, 1), d(10, 31))]);

        let store = Store::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let done = store.add_task("done").unwrap();
        store.set_due(done.id, Some(today), None).unwrap();
        store.set_completed(done.id, true).unwrap();
        let open = store.add_task("open").unwrap();
        store.set_due(open.id, Some(today), None).unwrap();
        let tasks = store.tasks().unwrap();
        let this_week = buckets(Bucket::Week, today, Weekday::Mon, 1);
        assert_eq!(completed_in(&tasks, &this_week), [1]);
        assert_eq!(completion_rate_in(&tasks, &this_week), [50]);
        // Only today so far: nothing to compare with.
        assert_eq!(more_productive_than(&tasks, today), 0);
    }

    #[test]
    fn completion_kinds_of_a_period() {
        let store = Store::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let on_time = store.add_task("on time").unwrap();
        store.set_due(on_time.id, Some(today), None).unwrap();
        store.set_completed(on_time.id, true).unwrap();
        let late = store.add_task("late").unwrap();
        store.set_due(late.id, Some(yesterday(today)), None).unwrap();
        store.set_completed(late.id, true).unwrap();
        let undated = store.add_task("undated").unwrap();
        store.set_completed(undated.id, true).unwrap();
        let open = store.add_task("open").unwrap();
        store.set_due(open.id, Some(today), None).unwrap();
        let tasks = store.tasks().unwrap();

        let kinds = completion_kinds(&tasks, today, today);
        assert_eq!(kinds, CompletionKinds { overdue: 1, on_time: 1, undated: 1, uncompleted: 1 });
        assert_eq!(kinds.total(), 4);
        assert_eq!(completed_by_list_in(&tasks, today, today), [(None, 3)]);
    }

    #[test]
    fn completion_by_tag() {
        let store = Store::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let both = store.add_task("both").unwrap();
        store.set_tags(both.id, &["home".to_owned(), "work".to_owned()]).unwrap();
        store.set_completed(both.id, true).unwrap();
        let home = store.add_task("home").unwrap();
        store.set_tags(home.id, &["home".to_owned()]).unwrap();
        store.set_completed(home.id, true).unwrap();
        let plain = store.add_task("plain").unwrap();
        store.set_completed(plain.id, true).unwrap();
        let tasks = store.tasks().unwrap();
        let counts = completed_by_tag_in(&tasks, today, today);
        assert_eq!(counts[0], (Some("home".to_owned()), 2));
        assert!(counts.contains(&(Some("work".to_owned()), 1)) && counts.contains(&(None, 1)));
    }
}
