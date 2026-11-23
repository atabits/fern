//! "Undo" after completing or deleting a task: what to revert and how.

use anyhow::Result;
use chrono::{NaiveDate, NaiveDateTime};
use fern_core::{Horizon, Id, Store, Task};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UndoAction {
    /// The task was completed, along with any open subtasks completed in the same cascade.
    Reopen { id: Id, cascaded: Vec<Id> },
    /// The task went to the trash.
    Restore(Id),
    /// A repeating task was completed: its completed copy goes and its dates/horizon/state come back.
    ReopenRepeat {
        id: Id,
        copy: Id,
        due: Option<NaiveDate>,
        end: Option<NaiveDate>,
        start: Option<NaiveDate>,
        horizon: Option<Horizon>,
        repeat: Option<fern_core::Recurrence>,
        in_progress: bool,
        snoozed_until: Option<NaiveDateTime>,
    },
    /// A task's horizon assignment changed (e.g., rollover or drag-and-drop).
    RestoreHorizon {
        id: Id,
        before: Option<Horizon>,
        after: Option<Horizon>,
    },
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
            Self::ReopenRepeat { id, copy, due, end, start: _, horizon, repeat, in_progress, snoozed_until } => {
                // Never delete the completed copy if the repeating parent was purged or deleted.
                let Some(parent) = store.task(*id)? else {
                    return Ok(());
                };
                if parent.is_deleted() {
                    return Ok(());
                }
                store.undo_repeat_with_horizon(*id, *copy, *due, *end, *horizon, repeat.as_ref())?;
                if *in_progress {
                    store.set_in_progress(*id, true)?;
                }
                if let Some(at) = *snoozed_until {
                    store.snooze_task(*id, at)?;
                }
                Ok(())
            }
            Self::RestoreHorizon { id, before, .. } => {
                let Some(task) = store.task(*id)? else {
                    return Ok(());
                };
                if task.is_deleted() {
                    return Ok(());
                }
                store.set_horizon(*id, *before)
            }
        }
    }

    /// Re-applies the original completion or deletion after an undo, returning a fresh `UndoAction`.
    pub fn redo(&self, store: &Store) -> Result<Option<Self>> {
        match self {
            Self::Reopen { id, .. } => {
                let Some(task) = store.task(*id)? else { return Ok(None) };
                if task.is_deleted() || task.is_completed() {
                    return Ok(None);
                }
                Ok(Some(complete(store, &task)?))
            }
            Self::ReopenRepeat { id, copy, due, end, start, horizon, .. } => {
                let Some(task) = store.task(*id)? else { return Ok(None) };
                if task.is_deleted() || task.is_completed() {
                    return Ok(None);
                }
                // Guard against double-advancing when the repeating task was never undone
                // or has already moved past its pre-completion dates/horizon.
                if store.task(*copy)?.is_some()
                    || task.due_date != *due
                    || task.end_date != *end
                    || task.start_date != *start
                    || task.horizon != *horizon
                {
                    return Ok(None);
                }
                Ok(Some(complete(store, &task)?))
            }
            Self::Restore(id) => {
                let Some(task) = store.task(*id)? else { return Ok(None) };
                if task.is_deleted() {
                    return Ok(None);
                }
                store.delete_task(*id)?;
                Ok(Some(Self::Restore(*id)))
            }
            Self::RestoreHorizon { id, before, after } => {
                let Some(task) = store.task(*id)? else { return Ok(None) };
                if task.is_deleted() || task.horizon != *before {
                    return Ok(None);
                }
                store.set_horizon(*id, *after)?;
                Ok(Some(Self::RestoreHorizon {
                    id: *id,
                    before: *before,
                    after: *after,
                }))
            }
        }
    }
}

/// Completes a task and returns how to undo it; `task` is its state before.
pub fn complete(store: &Store, task: &Task) -> Result<UndoAction> {
    let (copy, cascaded) = store.complete_with_cascaded(task.id, true)?;
    Ok(match copy {
        Some(copy) => UndoAction::ReopenRepeat {
            id: task.id,
            copy,
            due: task.due_date,
            end: task.end_date,
            start: task.start_date,
            horizon: task.horizon,
            repeat: task.repeat.clone(),
            in_progress: task.in_progress_at.is_some(),
            snoozed_until: task.snoozed_until,
        },
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

    #[test]
    fn undo_repeat_preserves_in_progress_and_snooze_and_guards_purged_parent() {
        let store = Store::open_in_memory().unwrap();
        let task = store.add_task("Standup").unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let snooze = day.and_hms_opt(14, 30, 0).unwrap();
        store.set_due(task.id, Some(day), None).unwrap();
        store
            .set_repeat(task.id, Some(fern_core::Recurrence::new(fern_core::Frequency::Daily, 1)), day)
            .unwrap();
        store.set_in_progress(task.id, true).unwrap();
        store.snooze_task(task.id, snooze).unwrap();

        let before = store.task(task.id).unwrap().unwrap();
        assert!(before.is_in_progress());
        assert_eq!(before.snoozed_until, Some(snooze));

        let undo = complete(&store, &before).unwrap();
        let after_complete = store.task(task.id).unwrap().unwrap();
        assert!(!after_complete.is_in_progress());
        assert_eq!(after_complete.snoozed_until, None);

        // Undo restores due date, in_progress_at, and snoozed_until.
        undo.apply(&store).unwrap();
        let restored = store.task(task.id).unwrap().unwrap();
        assert_eq!(restored.due_date, Some(day));
        assert!(restored.is_in_progress());
        assert_eq!(restored.snoozed_until, Some(snooze));

        // Guard: if the repeating parent was purged, undo_repeat must NOT delete the completed copy.
        let undo2 = complete(&store, &restored).unwrap();
        let copy_id = match &undo2 {
            UndoAction::ReopenRepeat { copy, .. } => *copy,
            other => panic!("expected ReopenRepeat, got {other:?}"),
        };
        store.delete_task(task.id).unwrap();
        store.purge_task(task.id).unwrap();
        assert!(store.task(task.id).unwrap().is_none());
        undo2.apply(&store).unwrap();
        assert!(
            store.task(copy_id).unwrap().is_some(),
            "completed copy must not be deleted when parent was purged"
        );
    }

    #[test]
    fn redo_does_not_double_advance_already_completed_repeating_task() {
        let store = Store::open_in_memory().unwrap();
        let task = store.add_task("Pay rent").unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        store.set_due(task.id, Some(day), None).unwrap();
        store
            .set_repeat(task.id, Some(fern_core::Recurrence::new(fern_core::Frequency::Weekly, 1)), day)
            .unwrap();
        let before = store.task(task.id).unwrap().unwrap();
        let undo = complete(&store, &before).unwrap();
        let next_due = store.task(task.id).unwrap().unwrap().due_date;
        assert!(next_due > Some(day));

        // Calling redo without undoing first must NOT advance the task a second time.
        assert!(undo.redo(&store).unwrap().is_none());
        assert_eq!(store.task(task.id).unwrap().unwrap().due_date, next_due);
        assert_eq!(store.tasks().unwrap().iter().filter(|t| t.is_completed()).count(), 1);

        // After undoing, redo succeeds once, then a second redo is a no-op.
        undo.apply(&store).unwrap();
        assert_eq!(store.task(task.id).unwrap().unwrap().due_date, Some(day));
        let redone = undo.redo(&store).unwrap();
        assert!(redone.is_some());
        assert_eq!(store.task(task.id).unwrap().unwrap().due_date, next_due);
        assert!(undo.redo(&store).unwrap().is_none());
        assert_eq!(store.task(task.id).unwrap().unwrap().due_date, next_due);
    }

    #[test]
    fn undo_and_redo_work_for_start_date_only_repeating_task() {
        let store = Store::open_in_memory().unwrap();
        let today = chrono::Local::now().date_naive();
        let tomorrow = today + chrono::Duration::days(1);
        let task = store
            .create_task(
                fern_core::TaskDraft {
                    title: "Things habit".into(),
                    start_date: Some(today),
                    repeat: Some(fern_core::Recurrence::new(fern_core::Frequency::Daily, 1)),
                    ..Default::default()
                },
                true,
            )
            .unwrap();
        let undo = complete(&store, &task).unwrap();
        assert_eq!(store.task(task.id).unwrap().unwrap().start_date, Some(tomorrow));
        // Double redo before undo must be a no-op.
        assert!(undo.redo(&store).unwrap().is_none());
        // Undo restores start_date back to today.
        undo.apply(&store).unwrap();
        assert_eq!(store.task(task.id).unwrap().unwrap().start_date, Some(today));
        // Redo re-advances cleanly.
        let redone = undo.redo(&store).unwrap().expect("redo must re-advance");
        assert_eq!(store.task(task.id).unwrap().unwrap().start_date, Some(tomorrow));
        // Second redo is guarded by start_date mismatch even if copy were deleted.
        assert!(redone.redo(&store).unwrap().is_none());
    }

    #[test]
    fn undo_and_redo_work_for_repeating_horizon_task_and_restore_horizon() {
        let store = Store::open_in_memory().unwrap();
        let w48 = Horizon::Week { year: 2026, week: 48 };
        let w49 = Horizon::Week { year: 2026, week: 49 };
        let task = store
            .create_task(
                fern_core::TaskDraft {
                    title: "Weekly horizon habit".into(),
                    horizon: Some(w48),
                    repeat: Some(fern_core::Recurrence::new(fern_core::Frequency::Weekly, 1)),
                    ..Default::default()
                },
                true,
            )
            .unwrap();
        let undo = complete(&store, &task).unwrap();
        assert!(matches!(undo, UndoAction::ReopenRepeat { horizon: Some(h), .. } if h == w48));
        assert_eq!(store.task(task.id).unwrap().unwrap().horizon, Some(w49));

        // Double redo before undo must be a no-op.
        assert!(undo.redo(&store).unwrap().is_none());

        // Undo restores horizon back to W48 and removes completed copy.
        undo.apply(&store).unwrap();
        assert_eq!(store.task(task.id).unwrap().unwrap().horizon, Some(w48));
        assert_eq!(store.tasks().unwrap().iter().filter(|t| t.is_completed()).count(), 0);

        // Redo advances cleanly back to W49.
        let redone = undo.redo(&store).unwrap().expect("redo must advance horizon repeat");
        assert_eq!(store.task(task.id).unwrap().unwrap().horizon, Some(w49));
        assert!(redone.redo(&store).unwrap().is_none());

        // RestoreHorizon undo/redo roundtrip.
        let m12 = Horizon::Month { year: 2026, month: 12 };
        let move_task = store.add_task("Horizon move").unwrap();
        store.set_horizon(move_task.id, Some(m12)).unwrap();
        let restore = UndoAction::RestoreHorizon {
            id: move_task.id,
            before: None,
            after: Some(m12),
        };
        restore.apply(&store).unwrap();
        assert_eq!(store.task(move_task.id).unwrap().unwrap().horizon, None);
        let redone_move = restore.redo(&store).unwrap().expect("redo must reapply horizon");
        assert_eq!(store.task(move_task.id).unwrap().unwrap().horizon, Some(m12));
        // Redo again without undo is guarded.
        assert!(redone_move.redo(&store).unwrap().is_none());
    }
}
