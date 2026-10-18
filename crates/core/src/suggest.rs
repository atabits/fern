//! Today's "Suggested Tasks", as the original's light bulb offers them: open tasks not planned
//! for today, the recently added ones and the ones left unfinished for long.

use chrono::{DateTime, Local, NaiveDate};

use crate::model::Task;

/// How many days a task stays "recently added", and from how many it is "long unfinished".
const RECENT_DAYS: i64 = 3;
const LONG_DAYS: i64 = 7;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Suggestions {
    /// Newest first.
    pub recent: Vec<Task>,
    /// Oldest first.
    pub long_unfinished: Vec<Task>,
}

/// The day a task was added, in local time.
pub fn created_day(task: &Task) -> Option<NaiveDate> {
    DateTime::from_timestamp_millis(task.created_at).map(|t| t.with_timezone(&Local).date_naive())
}

/// Open top-level tasks (not notes) that are not due by today: added in the last days, or
/// unfinished for a week or more.
pub fn suggestions(tasks: &[Task], today: NaiveDate) -> Suggestions {
    let candidates = tasks.iter().filter(|t| {
        !t.is_closed() && !t.is_deleted() && t.parent_id.is_none() && t.kind == crate::model::Kind::Task && t.due_date.is_none_or(|d| d > today)
    });
    let mut out = Suggestions::default();
    for task in candidates {
        let Some(day) = created_day(task) else { continue };
        let age = (today - day).num_days();
        if age < RECENT_DAYS {
            out.recent.push(task.clone());
        } else if age >= LONG_DAYS {
            out.long_unfinished.push(task.clone());
        }
    }
    out.recent.sort_by_key(|t| std::cmp::Reverse(t.created_at));
    out.long_unfinished.sort_by_key(|t| t.created_at);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn recent_and_long_unfinished() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let today = Local::now().date_naive();
        let fresh = store.add_task("Fresh")?;
        let planned = store.add_task("Planned")?;
        store.set_due(planned.id, Some(today), None)?;
        let mut tasks = store.tasks()?;
        // An old task, made old by its creation time.
        let mut old = tasks.iter().find(|t| t.id == fresh.id).unwrap().clone();
        old.title = "Old".into();
        old.id = uuid::Uuid::now_v7();
        old.created_at -= 10 * 24 * 3600 * 1000;
        tasks.push(old);
        let s = suggestions(&tasks, today);
        assert_eq!(s.recent.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Fresh"], "planned for today is left out");
        assert_eq!(s.long_unfinished.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Old"]);
        Ok(())
    }
}
