//! "Undo" after completing or deleting a task: what to revert and how.

use anyhow::Result;
use chrono::NaiveDate;
use fern_core::{Id, Store, Task};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UndoAction {
    /// The task was completed, along with any open subtasks completed in the same cascade.
    Reopen { id: Id, cascaded: Vec<Id> },
    /// The task went to the trash.
    Restore(Id),
    /// A repeating task was completed: its completed copy goes and its dates come back.
    ReopenRepeat { id: Id, copy: Id, due: Option<NaiveDate>, end: Option<NaiveDate>, repeat: Option<fern_core::Recurrence> },
}

impl UndoAction {
    pub fn apply(&self, store: &Store) -> Result<()> {
        match self {
            Self::Reopen { id, cascaded } => {
                let mut ids = Vec::with_capacity(1 + cascaded.len());
                ids.push(*id);
                ids.extend_from_slice(cascaded);
                store.reopen_tasks(&ids)
            }
            Self::Restore(id) => store.restore_task(*id),
            Self::ReopenRepeat { id, copy, due, end, repeat } => store.undo_repeat(*id, *copy, *due, *end, repeat.as_ref()),
        }
    }

    /// Re-applies the original completion or deletion after an undo, returning a fresh `UndoAction`.
    pub fn redo(&self, store: &Store) -> Result<Option<Self>> {
        match self {
            Self::Reopen { id, .. } | Self::ReopenRepeat { id, .. } => {
                let Some(task) = store.task(*id)? else { return Ok(None) };
                Ok(Some(complete(store, &task)?))
            }
            Self::Restore(id) => {
                store.delete_task(*id)?;
                Ok(Some(Self::Restore(*id)))
            }
        }
    }
}

/// Completes a task and returns how to undo it; `task` is its state before.
pub fn complete(store: &Store, task: &Task) -> Result<UndoAction> {
    let (copy, cascaded) = store.complete_with_cascaded(task.id, true)?;
    Ok(match copy {
        Some(copy) => UndoAction::ReopenRepeat { id: task.id, copy, due: task.due_date, end: task.end_date, repeat: task.repeat.clone() },
        None => UndoAction::Reopen { id: task.id, cascaded },
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
        UndoAction::Reopen { id: task.id, cascaded: Vec::new() }.apply(&store).unwrap();
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

        let redone = undo.redo(&store).unwrap().unwrap();
        assert!(matches!(redone, UndoAction::ReopenRepeat { .. }));
        assert_eq!(store.tasks().unwrap().iter().filter(|t| t.is_completed()).count(), 1);
        redone.apply(&store).unwrap();
        assert_eq!(store.task(task.id).unwrap().unwrap().due_date, Some(day));
    }

    #[test]
    fn undo_reopens_only_newly_cascaded_subtasks() {
        let store = Store::open_in_memory().unwrap();
        let parent = store.add_task("parent").unwrap();
        let open_child = store.add_subtask(parent.id, "open child").unwrap();
        let done_child = store.add_subtask(parent.id, "already done").unwrap();
        store.set_completed(done_child.id, true).unwrap();

        let before = store.task(parent.id).unwrap().unwrap();
        let undo = complete(&store, &before).unwrap();
        assert!(store.task(parent.id).unwrap().unwrap().is_completed());
        assert!(store.task(open_child.id).unwrap().unwrap().is_completed());
        assert!(store.task(done_child.id).unwrap().unwrap().is_completed());

        undo.apply(&store).unwrap();
        assert!(!store.task(parent.id).unwrap().unwrap().is_completed());
        assert!(!store.task(open_child.id).unwrap().unwrap().is_completed());
        assert!(store.task(done_child.id).unwrap().unwrap().is_completed());
    }
}
