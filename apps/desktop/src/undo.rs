//! "Undo" after completing or deleting a task: what to revert and how.

use anyhow::Result;
use chrono::NaiveDate;
use fern_core::{Id, Store, Task};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UndoAction {
    /// The task was completed.
    Reopen(Id),
    /// The task went to the trash.
    Restore(Id),
    /// A repeating task was completed: its completed copy goes and its dates come back.
    ReopenRepeat { id: Id, copy: Id, due: Option<NaiveDate>, end: Option<NaiveDate> },
}

impl UndoAction {
    pub fn apply(self, store: &Store) -> Result<()> {
        match self {
            Self::Reopen(id) => store.set_completed(id, false),
            Self::Restore(id) => store.restore_task(id),
            Self::ReopenRepeat { id, copy, due, end } => store.undo_repeat(id, copy, due, end),
        }
    }
}

/// Completes a task and returns how to undo it; `task` is its state before.
pub fn complete(store: &Store, task: &Task) -> Result<UndoAction> {
    Ok(match store.complete(task.id, true)? {
        Some(copy) => UndoAction::ReopenRepeat { id: task.id, copy, due: task.due_date, end: task.end_date },
        None => UndoAction::Reopen(task.id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_reverts() {
        let store = Store::open_in_memory().unwrap();
        let task = store.add_task("water the plants").unwrap();
        store.set_completed(task.id, true).unwrap();
        UndoAction::Reopen(task.id).apply(&store).unwrap();
        assert!(!store.task(task.id).unwrap().unwrap().is_completed());
        store.delete_task(task.id).unwrap();
        UndoAction::Restore(task.id).apply(&store).unwrap();
        assert!(!store.task(task.id).unwrap().unwrap().is_deleted());

        let day = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        store.set_due(task.id, Some(day), None).unwrap();
        store.set_repeat(task.id, Some(fern_core::Recurrence::new(fern_core::Frequency::Weekly, 1)), day).unwrap();
        let before = store.task(task.id).unwrap().unwrap();
        let undo = complete(&store, &before).unwrap();
        assert!(matches!(undo, UndoAction::ReopenRepeat { .. }));
        undo.apply(&store).unwrap();
        assert_eq!(store.task(task.id).unwrap().unwrap().due_date, Some(day));
        assert_eq!(store.tasks().unwrap().iter().filter(|t| t.is_completed()).count(), 0);
    }
}
