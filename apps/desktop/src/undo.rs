//! "Undo" after completing or deleting a task: what to revert and how.

use anyhow::Result;
use fern_core::{Id, Store};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UndoAction {
    /// The task was completed.
    Reopen(Id),
    /// The task went to the trash.
    Restore(Id),
}

impl UndoAction {
    pub fn apply(self, store: &Store) -> Result<()> {
        match self {
            Self::Reopen(id) => store.set_completed(id, false),
            Self::Restore(id) => store.restore_task(id),
        }
    }
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
    }
}
