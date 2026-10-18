//! Task reminders, as in the original: offsets before the due time. All-day tasks are reminded
//! relative to 09:00 on their due day ("On the day (09:00)", "1 day early (09:00)").

use chrono::{Duration, NaiveDateTime, NaiveTime};

use crate::model::Task;

/// The time of day all-day tasks are reminded at.
pub const ALL_DAY_REMINDER_HOUR: u32 = 9;

/// Offsets (minutes before the due time) offered for tasks with a time: on time, 5 and 30
/// minutes, 1 hour and 1 day early.
pub const TIMED_OFFSETS: [i64; 5] = [0, 5, 30, 60, 24 * 60];

/// Offsets offered for all-day tasks: on the day, 1, 2 and 3 days and 1 week early.
pub const ALL_DAY_OFFSETS: [i64; 5] = [0, 24 * 60, 2 * 24 * 60, 3 * 24 * 60, 7 * 24 * 60];

/// When a task's reminders fire, earliest first. Tasks without a due date have none.
pub fn fire_times(task: &Task) -> Vec<NaiveDateTime> {
    let Some(date) = task.due_date else { return Vec::new() };
    let base = date.and_time(
        task.due_time.unwrap_or_else(|| NaiveTime::from_hms_opt(ALL_DAY_REMINDER_HOUR, 0, 0).unwrap_or_default()),
    );
    let mut times: Vec<NaiveDateTime> =
        task.reminders.iter().map(|minutes| base - Duration::minutes(*minutes)).collect();
    times.sort();
    times.dedup();
    times
}

/// Open tasks with a reminder firing in `(after, until]`, with the time it fires.
pub fn due_between(tasks: &[Task], after: NaiveDateTime, until: NaiveDateTime) -> Vec<(&Task, NaiveDateTime)> {
    tasks
        .iter()
        .filter(|task| !task.is_closed() && !task.is_deleted())
        .filter_map(|task| fire_times(task).into_iter().find(|at| *at > after && *at <= until).map(|at| (task, at)))
        .collect()
}

/// Stored as a comma-separated list of minutes.
pub fn format(reminders: &[i64]) -> Option<String> {
    (!reminders.is_empty()).then(|| reminders.iter().map(i64::to_string).collect::<Vec<_>>().join(","))
}

pub fn parse(text: &str) -> Vec<i64> {
    text.split(',').filter_map(|part| part.trim().parse().ok()).collect()
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn at(day: u32, h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap().and_hms_opt(h, m, 0).unwrap()
    }

    fn task(due: Option<(u32, Option<(u32, u32)>)>, reminders: &[i64]) -> Task {
        let mut task = crate::Store::open_in_memory().unwrap().add_task("water the plants").unwrap();
        task.due_date = due.map(|(day, _)| NaiveDate::from_ymd_opt(2026, 10, day).unwrap());
        task.due_time = due.and_then(|(_, time)| time).map(|(h, m)| NaiveTime::from_hms_opt(h, m, 0).unwrap());
        task.reminders = reminders.to_vec();
        task
    }

    #[test]
    fn fire_times_count_back_from_the_due_time() {
        assert_eq!(fire_times(&task(Some((20, Some((18, 30)))), &[30, 0])), [at(20, 18, 0), at(20, 18, 30)]);
        // All-day: from 09:00 on the day.
        assert_eq!(fire_times(&task(Some((20, None)), &[0, 24 * 60])), [at(19, 9, 0), at(20, 9, 0)]);
        assert!(fire_times(&task(None, &[0])).is_empty());
    }

    #[test]
    fn due_between_skips_closed_tasks() {
        let open = task(Some((20, Some((18, 30)))), &[5]);
        let mut done = open.clone();
        done.completed_at = Some(1);
        let tasks = [open, done];
        let due = due_between(&tasks, at(20, 18, 20), at(20, 18, 25));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1, at(20, 18, 25));
        assert!(due_between(&tasks, at(20, 18, 25), at(20, 18, 30)).is_empty());
    }

    #[test]
    fn storage_format() {
        assert_eq!(format(&[0, 1440]).as_deref(), Some("0,1440"));
        assert_eq!(format(&[]), None);
        assert_eq!(parse("0, 1440,x"), [0, 1440]);
    }
}
