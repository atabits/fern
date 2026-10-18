//! Task statistics for the Statistics page's Overview and Task tabs.

use chrono::{Duration, Local, NaiveDate, TimeZone};

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
}
