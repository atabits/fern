//! The list's Timeline view: tasks as bars across days, from their date to the end of their
//! duration.

use chrono::NaiveDate;

use crate::model::Task;

/// Where a task's bar goes among `days` day columns starting at `from`: its first column and
/// how many columns it spans, clipped to the shown days. `None` for an undated task or one
/// outside them.
pub fn bar(task: &Task, from: NaiveDate, days: usize) -> Option<(usize, usize)> {
    let start = task.due_date?;
    let end = task.end_date.unwrap_or(start).max(start);
    let last = from + chrono::Duration::days(days as i64 - 1);
    if end < from || start > last {
        return None;
    }
    let first = (start.max(from) - from).num_days() as usize;
    let span = (end.min(last) - start.max(from)).num_days() as usize + 1;
    Some((first, span))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
    }

    #[test]
    fn bars_are_clipped_to_the_shown_days() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("Trip")?;
        assert_eq!(bar(&store.task(task.id)?.unwrap(), d(18), 10), None, "no date, no bar");

        store.set_due(task.id, Some(d(20)), None)?;
        assert_eq!(bar(&store.task(task.id)?.unwrap(), d(18), 10), Some((2, 1)));
        store.set_due_end(task.id, Some((d(23), None)))?;
        assert_eq!(bar(&store.task(task.id)?.unwrap(), d(18), 10), Some((2, 4)));
        // Starting before the shown days, ending after them.
        assert_eq!(bar(&store.task(task.id)?.unwrap(), d(21), 2), Some((0, 2)));
        assert_eq!(bar(&store.task(task.id)?.unwrap(), d(24), 5), None);
        Ok(())
    }

    #[test]
    fn moving_keeps_the_duration() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let task = store.add_task("Trip")?;
        let time = chrono::NaiveTime::from_hms_opt(9, 0, 0);
        store.set_due(task.id, Some(d(20)), time)?;
        store.set_due_end(task.id, Some((d(23), None)))?;
        store.move_dates(task.id, d(25))?;
        let moved = store.task(task.id)?.unwrap();
        assert_eq!((moved.due_date, moved.due_time, moved.end_date), (Some(d(25)), time, Some(d(28))));

        // An undated task just gets the date.
        let plain = store.add_task("Call")?;
        store.move_dates(plain.id, d(19))?;
        let plain = store.task(plain.id)?.unwrap();
        assert_eq!((plain.due_date, plain.end_date), (Some(d(19)), None));
        Ok(())
    }
}
