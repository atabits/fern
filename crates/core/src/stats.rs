//! Task statistics for the Statistics page's Overview and Task tabs.

use chrono::{Duration, Local, NaiveDate, TimeZone};

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
}
