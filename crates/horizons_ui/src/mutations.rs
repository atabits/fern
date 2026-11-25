//! Task and slot mutations, inline task creation, drag-and-drop scheduling, and undo/redo
//! helpers for [`HorizonsView`].


use desktop_common::data_version::data_changed;
use desktop_common::task_ops;
use desktop_common::text_input::{Cancelled, Mode, Submitted, TextInput};
use desktop_common::undo::UndoAction;
use fern_core::horizon::is_horizons_inbox;
use fern_core::{Id, Store, Task, TaskDraft};
use fern_i18n as i18n;
use gpui::{AppContext as _, Context, Entity, Focusable as _, Window, px};
use task_ui::task_detail::{TaskTextEdited, TaskTextField};

use crate::column::{ColumnTarget, HorizonSlot};
use crate::{HorizonsMode, HorizonsView};

#[derive(Clone, Debug)]
pub(crate) enum UndoEntry {
    Single(UndoAction),
    Batch(Vec<UndoAction>),
}

impl UndoEntry {
    fn apply(&self, store: &Store) -> bool {
        match self {
            Self::Single(action) => action.apply(store).is_ok(),
            Self::Batch(actions) => {
                for action in actions.iter().rev() {
                    if action.apply(store).is_err() {
                        return false;
                    }
                }
                true
            }
        }
    }

    fn redo(&self, store: &Store) -> Option<Self> {
        match self {
            Self::Single(action) => action.redo(store).ok().flatten().map(Self::Single),
            Self::Batch(actions) => {
                let mut redone = Vec::with_capacity(actions.len());
                for action in actions {
                    if let Ok(Some(next)) = action.redo(store) {
                        redone.push(next);
                    }
                }
                (!redone.is_empty()).then_some(Self::Batch(redone))
            }
        }
    }
}

impl HorizonsView {
    pub(crate) fn apply_task_text_edit(&mut self, edit: &TaskTextEdited, cx: &mut Context<Self>) {
        if self.dirty {
            self.reload(cx);
        } else {
            for task in self.tasks.iter_mut().filter(|t| t.id == edit.id) {
                match &edit.field {
                    TaskTextField::Title(title) => task.title = title.clone(),
                    TaskTextField::Notes(notes) => task.notes = notes.clone(),
                    TaskTextField::Location(location) => task.location = location.clone(),
                }
            }
            cx.notify();
        }
    }

    pub(crate) fn select_mode(&mut self, mode: HorizonsMode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            self.period_offset = 0;
            self.inline_add = None;
            self.active_popover = None;
            self.h_scroll.set_offset(gpui::point(px(0.), px(0.)));
            cx.notify();
        }
    }

    pub(crate) fn step_period(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.period_offset = self.period_offset.saturating_add(delta);
        self.inline_add = None;
        cx.notify();
    }

    pub(crate) fn jump_to_current(&mut self, cx: &mut Context<Self>) {
        if self.period_offset != 0 {
            self.period_offset = 0;
            self.inline_add = None;
            cx.notify();
        }
    }

    pub(crate) fn toggle_slot_collapsed(&mut self, slot: HorizonSlot, cx: &mut Context<Self>) {
        let expanded = self.collapsed_cols.remove(&slot);
        if !expanded {
            self.collapsed_cols.insert(slot);
            if let Some((target, _, _)) = &self.inline_add
                && *target == slot.target(Self::today())
            {
                self.inline_add = None;
            }
        }
        self.persist_collapsed_cols();
        data_changed(cx.entity_id(), cx);
        cx.notify();
    }

    pub(crate) fn push_undo(&mut self, entry: UndoEntry) {
        self.undo_stack.push(entry);
        if self.undo_stack.len() > 64 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub(crate) fn perform_undo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(entry) = self.undo_stack.pop() else {
            return false;
        };
        if entry.apply(&self.store) {
            self.redo_stack.push(entry);
            data_changed(cx.entity_id(), cx);
            self.reload(cx);
            return true;
        }
        false
    }

    pub(crate) fn perform_redo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(entry) = self.redo_stack.pop() else {
            return false;
        };
        if let Some(next) = entry.redo(&self.store) {
            self.undo_stack.push(next);
            data_changed(cx.entity_id(), cx);
            self.reload(cx);
            return true;
        }
        false
    }

    fn finish_toggle_task_done(&mut self, task: &Task, done: bool, cx: &mut Context<Self>) {
        if let Ok(undo) = task_ops::toggle_task_completion(&self.store, task, done, cx.entity_id(), cx) {
            if let Some(action) = undo {
                self.push_undo(UndoEntry::Single(action));
            }
            self.reload(cx);
        }
    }

    pub(crate) fn toggle_task_done(&mut self, id: Id, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        let done = !task.is_completed();
        if done
            && !task.is_wont_do()
            && desktop_common::plugin_bus::PluginHooksGlobal::global(cx)
                .is_some_and(|h| h.is_active())
        {
            if !self.pending_complete.insert(id) {
                return;
            }
            let weak = cx.weak_entity();
            desktop_common::plugin_bus::run_before_task_complete_hook(&task, cx, move |allowed, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.pending_complete.remove(&id);
                    if !allowed {
                        cx.notify();
                        return;
                    }
                    if let Some(current) = this
                        .tasks
                        .iter()
                        .find(|t| t.id == id)
                        .cloned()
                        .or_else(|| this.store.task(id).ok().flatten())
                        && !current.is_completed()
                    {
                        this.finish_toggle_task_done(&current, true, cx);
                    }
                });
            });
            return;
        }
        self.finish_toggle_task_done(&task, done, cx);
    }

    pub(crate) fn start_inline_add(
        &mut self,
        target: ColumnTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.horizons_add_placeholder, cx));
        let subs = vec![
            cx.subscribe_in(&input, window, move |this, input_entity, ev: &Submitted, window, cx| {
                this.submit_inline_add(target, &ev.0, input_entity, window, cx);
            }),
            cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
                this.inline_add = None;
                window.focus(&this.focus_handle, cx);
                cx.notify();
            }),
        ];
        window.focus(&input.focus_handle(cx), cx);
        self.inline_add = Some((target, input, subs));
        self.active_popover = None;
        cx.notify();
    }

    fn submit_inline_add(
        &mut self,
        target: ColumnTarget,
        raw: &str,
        input: &Entity<TextInput>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = raw.trim();
        if title.is_empty() {
            self.inline_add = None;
            window.focus(&self.focus_handle, cx);
            cx.notify();
            return;
        }

        let mut draft = TaskDraft {
            title: title.to_owned(),
            ..Default::default()
        };
        if let Some(tag) = &self.filter_tag {
            draft.tags.push(tag.clone());
        }
        if let Some(priority) = self.filter_priority {
            draft.priority = priority;
        }

        match target {
            ColumnTarget::Inbox => {}
            ColumnTarget::Day(day) => {
                draft.due_date = Some(day);
                draft.list_id = self.filter_board;
            }
            ColumnTarget::Horizon(horizon) => {
                draft.horizon = Some(horizon);
                draft.list_id = self.filter_board;
            }
        }

        if self.store.create_task(draft, false).is_ok() {
            input.update(cx, |i, cx| i.set_text("", cx));
            window.focus(&input.focus_handle(cx), cx);
            data_changed(cx.entity_id(), cx);
            self.reload(cx);
        }
    }

    pub(crate) fn handle_drop_on_column(
        &mut self,
        task_id: Id,
        target: ColumnTarget,
        cx: &mut Context<Self>,
    ) {
        let Some(task) = self.tasks.iter().find(|t| t.id == task_id).cloned() else {
            return;
        };
        let mut changed = false;
        match target {
            ColumnTarget::Inbox => {
                if !is_horizons_inbox(&task) || task.section_id.is_some() || task.end_date.is_some() {
                    let before_horizon = task.horizon;
                    let ok_due = self.store.set_due(task_id, None, None).is_ok();
                    let ok_end = self.store.set_due_end(task_id, None).is_ok();
                    let ok_horizon = self.store.set_horizon(task_id, None).is_ok();
                    let ok_list = self.store.set_list(task_id, None).is_ok();
                    let ok_sec = self.store.set_section(task_id, None).is_ok();
                    if ok_due && ok_end && ok_horizon && ok_list && ok_sec {
                        if before_horizon.is_some()
                            && task.due_date.is_none()
                            && task.list_id.is_none()
                        {
                            self.push_undo(UndoEntry::Single(UndoAction::RestoreHorizon {
                                id: task_id,
                                before: before_horizon,
                                after: None,
                            }));
                        }
                        changed = true;
                    }
                }
            }
            ColumnTarget::Day(day) => {
                if (task.due_date != Some(day) || task.horizon.is_some())
                    && self.store.set_due(task_id, Some(day), task.due_time).is_ok()
                {
                    changed = true;
                }
            }
            ColumnTarget::Horizon(horizon) => {
                if (task.horizon != Some(horizon) || task.due_date.is_some())
                    && self.store.set_horizon(task_id, Some(horizon)).is_ok()
                {
                    if task.due_date.is_none() {
                        self.push_undo(UndoEntry::Single(UndoAction::RestoreHorizon {
                            id: task_id,
                            before: task.horizon,
                            after: Some(horizon),
                        }));
                    }
                    changed = true;
                }
            }
        }
        if changed {
            data_changed(cx.entity_id(), cx);
            self.reload(cx);
        }
    }
}
