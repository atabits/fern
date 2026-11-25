//! Shared task mutations used across desktop views (Todo, Calendar, Matrix, Sticky, Task Detail).

use anyhow::Result;
use fern_core::{Store, Task};
use gpui::{App, EntityId};

use crate::data_version;
use crate::sound;
use crate::undo::{self, UndoAction};

/// Completes or reopens `task`, keeping repeating advancement, "Won't Do" reopening,
/// completion sound playback (only after DB success), and cross-window `DataVersion`
/// updates consistent across every view.
///
/// - If `task` is currently "Won't Do", toggling reopens it (`set_wont_do(false)`) and
///   returns `Ok(None)` without playing the completion chime.
/// - If `done` is `true`, completes the task via [`undo::complete`] (advancing repeating
///   tasks and cascading to open subtasks), plays the completion sound only after the DB
///   write succeeds, and returns `Ok(Some(undo_action))`.
/// - If `done` is `false`, reopens the task (and any closed ancestors) and returns `Ok(None)`.
/// - Calls [`data_version::data_changed`] on success so other windows refresh.
pub fn toggle_task_completion(
    store: &Store,
    task: &Task,
    done: bool,
    source: EntityId,
    cx: &mut App,
) -> Result<Option<UndoAction>> {
    let undo = if task.is_wont_do() {
        store.set_wont_do(task.id, false)?;
        None
    } else if done {
        let action = undo::complete(store, task)?;
        sound::play_complete(cx);
        Some(action)
    } else {
        store.set_completed(task.id, false)?;
        None
    };
    data_version::data_changed_scoped(source, data_version::DataScope::TASKS, cx);
    Ok(undo)
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use fern_core::{Frequency, Recurrence};
    use gpui::{AppContext as _, TestAppContext};

    use super::*;

    struct Marker;

    #[gpui::test]
    fn toggle_task_completion_handles_regular_repeating_and_wont_do(cx: &mut TestAppContext) {
        let store = Store::open_in_memory().unwrap();
        let (source_id, other_id) = cx.update(|cx| {
            let a = cx.new(|_| Marker);
            let b = cx.new(|_| Marker);
            (a.entity_id(), b.entity_id())
        });

        // 1. Regular task complete -> returns Reopen, marks DataVersion changed for other windows.
        let regular = store.add_task("Buy coffee").unwrap();
        let undo = cx
            .update(|cx| toggle_task_completion(&store, &regular, true, source_id, cx))
            .unwrap();
        assert!(matches!(undo, Some(UndoAction::Reopen { id, .. }) if id == regular.id));
        assert!(store.task(regular.id).unwrap().unwrap().is_completed());
        cx.update(|cx| {
            assert!(!data_version::changed_elsewhere(source_id, cx));
            assert!(data_version::changed_elsewhere(other_id, cx));
        });

        // Reopen regular task -> returns None.
        let completed_regular = store.task(regular.id).unwrap().unwrap();
        let reopen_undo = cx
            .update(|cx| toggle_task_completion(&store, &completed_regular, false, other_id, cx))
            .unwrap();
        assert!(reopen_undo.is_none());
        assert!(!store.task(regular.id).unwrap().unwrap().is_completed());
        cx.update(|cx| assert!(data_version::changed_elsewhere(source_id, cx)));

        // 2. Repeating task complete -> advances due date & returns ReopenRepeat.
        let rep = store.add_task("Weekly review").unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        store.set_due(rep.id, Some(day), None).unwrap();
        store
            .set_repeat(rep.id, Some(Recurrence::new(Frequency::Weekly, 1)), day)
            .unwrap();
        let rep_before = store.task(rep.id).unwrap().unwrap();
        let rep_undo = cx
            .update(|cx| toggle_task_completion(&store, &rep_before, true, source_id, cx))
            .unwrap();
        assert!(matches!(rep_undo, Some(UndoAction::ReopenRepeat { id, .. }) if id == rep.id));
        let rep_after = store.task(rep.id).unwrap().unwrap();
        assert!(!rep_after.is_completed());
        assert!(rep_after.due_date > Some(day));

        // 3. Won't Do task -> toggling reopens it (even if `done == true`) and returns None.
        let wont_do = store.add_task("Skipped chore").unwrap();
        store.set_wont_do(wont_do.id, true).unwrap();
        let wont_do_task = store.task(wont_do.id).unwrap().unwrap();
        assert!(wont_do_task.is_wont_do());
        let wd_undo = cx
            .update(|cx| toggle_task_completion(&store, &wont_do_task, true, source_id, cx))
            .unwrap();
        assert!(wd_undo.is_none());
        let reopened_wd = store.task(wont_do.id).unwrap().unwrap();
        assert!(!reopened_wd.is_wont_do());
        assert!(!reopened_wd.is_completed());
    }
}
