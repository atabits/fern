use super::*;

use crate::actions::{
    AddSubtask, ClearDate, DeleteTask, ExtendSelectionNext, ExtendSelectionPrevious, OpenAsSticky,
    PinTask, PriorityHigh, PriorityLow, PriorityMedium, PriorityNone, SelectNext, SelectPrevious,
    SetDate, SetNextWeek, SetToday, SetTomorrow, ToggleAllGroups, ToggleAllSubtasks,
    ToggleComplete, ViewKanban, ViewList, ViewTimeline,
};

impl TodoView {
    /// Task ids in the order they are shown, for keyboard navigation.
    pub(super) fn visible_task_ids(&self, strings: &Strings) -> Vec<Id> {
        if self.kanban && !self.timeline && !matches!(self.view, View::Trash | View::Completed | View::WontDo | View::Search(_)) {
            let mut ids = Vec::new();
            let push_cards = |cards: &[BoardCard], ids: &mut Vec<Id>| {
                ids.extend(self.kanban_visible_rows(cards).into_iter().map(|r| r.task.id));
            };
            for col in self.board_columns(strings).iter() {
                if !col.pinned_cards.is_empty() {
                    if !self.kanban_folded_sub.contains(&format!("{}:pinned", col.key)) {
                        push_cards(&col.pinned_cards, &mut ids);
                    }
                    if !col.open_cards.is_empty() && !self.kanban_folded_sub.contains(&format!("{}:unpinned", col.key)) {
                        push_cards(&col.open_cards, &mut ids);
                    }
                } else {
                    push_cards(&col.open_cards, &mut ids);
                }
                if !col.completed_cards.is_empty() && !self.kanban_folded_sub.contains(&format!("{}:completed", col.key)) {
                    push_cards(&col.completed_cards, &mut ids);
                }
            }
            return ids;
        }
        self.rows_rc(strings)
            .iter()
            .filter_map(|row| match row {
                Row::Task { task, .. } => Some(task.id),
                Row::Section { .. } | Row::Day { .. } => None,
            })
            .collect()
    }

    pub fn print_list(&mut self, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        let strings = i18n::t(cx);
        let title = self.view_title(strings);
        let html = crate::print::list_html(&title, &self.tasks, Self::today(), strings);
        let task = crate::print::open_async("list", html, cx);
        cx.spawn(async move |this, cx| {
            if let Err(err) = task.await {
                this.update(cx, |this, cx| {
                    this.error = Some(err.to_string());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    pub fn print_selected_detail(&mut self, cx: &mut Context<Self>) {
        if self.card.is_some() {
            self.card_detail.update(cx, |detail, cx| detail.print_current(cx));
        } else {
            self.detail.update(cx, |detail, cx| detail.print_current(cx));
        }
    }

    pub(super) fn active_task_id(&self) -> Option<Id> {
        self.selected.or(self.card)
    }

    pub(super) fn select_in_current_layout(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.kanban && !self.timeline && self.selected.is_none() {
            self.open_card(id, cx);
        } else {
            self.select(id, cx);
        }
    }

    pub(super) fn move_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let ids = self.visible_task_ids(i18n::t(cx));
        if ids.is_empty() {
            return;
        }
        self.multi_selected.clear();
        self.batch_menu = None;
        let next = match self.active_task_id().and_then(|id| ids.iter().position(|i| *i == id)) {
            Some(index) => (index as isize + step).clamp(0, ids.len() as isize - 1) as usize,
            None if step > 0 => 0,
            None => ids.len() - 1,
        };
        let id = ids[next];
        self.selection_anchor = Some(id);
        self.select_in_current_layout(id, cx);
        if let Some(ix) = self.list_entries.iter().position(|e| matches!(e, ListEntry::Row(Row::Task { task, .. }) if task.id == id)) {
            self.list_state.scroll_to_reveal_item(ix);
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    fn toggle_selected(&mut self, _: &ToggleComplete, _: &mut Window, cx: &mut Context<Self>) {
        if self.title_edit.is_some() || self.view == View::Trash {
            return;
        }
        if self.is_multi_selecting() {
            let ids = self.selected_task_ids();
            let all_completed = !ids.is_empty()
                && ids
                    .iter()
                    .filter_map(|id| self.tasks.iter().find(|t| t.id == *id))
                    .all(Task::is_completed);
            self.run_batch(false, cx, move |store, ids| store.batch_complete(ids, !all_completed));
            return;
        }
        if let Some(id) = self.active_task_id() {
            if self.is_task_id_read_only(id) {
                return;
            }
            if self.completing.contains_key(&id) {
                self.toggle(id, false, cx);
            } else if let Some(task) = self.tasks.iter().find(|task| task.id == id) {
                self.toggle(id, !task.is_completed(), cx);
            }
        }
    }

    /// Applies a change to the selected task, for the task edit shortcuts.
    fn edit_selected(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&Store, &Task) -> anyhow::Result<()>) {
        if self.title_edit.is_some() || self.view == View::Trash {
            return;
        }
        let Some(task) = self.active_task_id().and_then(|id| self.tasks.iter().find(|t| t.id == id)).cloned() else { return };
        if self.is_task_read_only(&task) {
            return;
        }
        let result = change(&self.store, &task);
        self.check(result);
        self.reload(cx);
        self.select_in_current_layout(task.id, cx);
    }

    fn pin_selected(&mut self, _: &PinTask, _: &mut Window, cx: &mut Context<Self>) {
        if self.title_edit.is_some() || self.view == View::Trash {
            return;
        }
        if self.is_multi_selecting() {
            let ids = self.selected_task_ids();
            let all_pinned = !ids.is_empty()
                && ids
                    .iter()
                    .filter_map(|id| self.tasks.iter().find(|t| t.id == *id))
                    .all(|t| t.pinned_at.is_some());
            self.run_batch(false, cx, move |store, ids| store.batch_set_pinned(ids, !all_pinned));
            return;
        }
        self.edit_selected(cx, |store, task| store.set_pinned(task.id, task.pinned_at.is_none()));
    }

    fn date_selected(&mut self, _: &SetDate, _: &mut Window, cx: &mut Context<Self>) {
        if self.title_edit.is_some() || self.view == View::Trash {
            return;
        }
        if let Some(id) = self.active_task_id()
            && self.is_task_id_read_only(id)
        {
            return;
        }
        if self.selected.is_some() {
            self.detail.update(cx, |detail, cx| detail.open_due(cx));
        } else if self.card.is_some() {
            self.card_detail.update(cx, |detail, cx| detail.open_due(cx));
        }
    }

    fn set_selected_due(&mut self, days: Option<i64>, cx: &mut Context<Self>) {
        if self.title_edit.is_some() || self.view == View::Trash {
            return;
        }
        let today = Self::today();
        if self.is_multi_selecting() {
            let date = days.map(|d| today + Duration::days(d));
            self.run_batch(false, cx, move |store, ids| match date {
                Some(d) => store.batch_move_dates(ids, d),
                None => store.batch_set_due(ids, None, None),
            });
            return;
        }
        self.edit_selected(cx, move |store, task| {
            let date = days.map(|d| today + Duration::days(d));
            store.set_due(task.id, date, date.and(task.due_time))
        });
    }

    fn set_selected_priority(&mut self, priority: Priority, cx: &mut Context<Self>) {
        if self.title_edit.is_some() || self.view == View::Trash {
            return;
        }
        if self.is_multi_selecting() {
            self.run_batch(false, cx, move |store, ids| store.batch_set_priority(ids, priority));
            return;
        }
        self.edit_selected(cx, move |store, task| store.set_priority(task.id, priority));
    }

    fn delete_selected(&mut self, _: &DeleteTask, _: &mut Window, cx: &mut Context<Self>) {
        // As in the original: Trash and read-only (archived-list) tasks cannot be deleted with
        // the keyboard shortcut; Trash requires explicit "Delete forever" with confirmation.
        if self.title_edit.is_some() || self.view == View::Trash {
            return;
        }
        if self.is_multi_selecting() {
            self.run_batch(true, cx, |store, ids| store.batch_delete(ids));
            return;
        }
        let Some(id) = self.active_task_id() else { return };
        if self.is_task_id_read_only(id) {
            return;
        }
        // Keep a selection so repeated presses keep working down the list.
        let ids = self.visible_task_ids(i18n::t(cx));
        let neighbor = ids
            .iter()
            .position(|i| *i == id)
            .and_then(|index| ids.get(index + 1).or_else(|| index.checked_sub(1).and_then(|i| ids.get(i))))
            .copied();
        self.delete(id, cx);
        if let Some(neighbor) = neighbor {
            self.select_in_current_layout(neighbor, cx);
        }
    }

    pub(super) fn reset_view_transients(&mut self, cx: &mut Context<Self>) {
        self.multi_selected.clear();
        self.selection_anchor = None;
        self.batch_menu = None;
        self.header_menu_open = false;
        self.list_activity = None;
        self.close_card(cx);
        self.context_menu = None;
        self.context_side = None;
        self.closed_menu = None;
        self.sort_menu = None;
        self.sort_menu_return_focus = None;
        self.scale_menu_open = false;
        self.timeline_tz_menu = false;
        self.arrange_open = false;
        self.suggestions_open = None;
        self.section_menu = None;
        self.postpone_menu = false;
        self.column_menu = None;
        self.column_add = None;
        self.kanban_folded_sub.clear();
        self.new_section = None;
        self.new_section_index = None;
        self.section_rename = None;
        self.title_edit = None;
        self.kanban_pan = None;
        self.kanban_quick_adds.clear();
        self.kanban_col_scrolls.clear();
        self.clear_task_drag_state();
        self.completing.clear();
        self.invalidate_rows();
    }

    pub fn select_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_viewport(window, cx);
        let switching = self.view != view;
        self.set_view(view, cx);
        if switching {
            if self.view == View::Horizons {
                window.focus(&self.horizons.focus_handle(cx), cx);
            } else if self.view != View::Summary {
                window.focus(&self.list_focus, cx);
            }
        }
    }

    #[cfg(debug_assertions)]
    pub fn dev_horizons_mode(&mut self, spec: &str, cx: &mut Context<Self>) {
        self.horizons.update(cx, |h, cx| h.dev_select_mode(spec, cx));
    }

    pub fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        self.list_activity = None;
        if self.view != view {
            self.finish_title_edit(true, cx);
            self.reset_view_transients(cx);
            self.view = view;
            self.reload_inner(true, false, cx);
        } else if self.dirty {
            self.reload_inner(true, false, cx);
        }
    }

    /// Switches to `View::All` and selects `id` directly without loading the first row's details twice.
    pub fn open_task_in_all(&mut self, id: Id, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        self.list_activity = None;
        if self.view != View::All {
            self.finish_title_edit(true, cx);
            self.reset_view_transients(cx);
            self.view = View::All;
            self.list_state = Self::new_list_state();
            self.list_entries.clear();
            self.selected = Some(id);
            self.reload_inner(false, false, cx);
        } else if self.dirty {
            self.selected = Some(id);
            self.reload_inner(false, false, cx);
        } else {
            self.select(id, cx);
        }
    }

    pub(super) fn update_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        if !matches!(self.view, View::Search(_)) {
            self.set_view(View::Search(query), cx);
            return;
        }
        if self.view == View::Search(query.clone()) {
            return;
        }
        self.finish_title_edit(true, cx);
        self.view = View::Search(query);
        if let Some(tasks) = self.check(self.store.tasks_in(&self.view, Self::today())) {
            self.tasks = tasks;
        }
        self.list_state = Self::new_list_state();
        self.list_entries.clear();
        self.invalidate_rows();
        self.sync_layout_selection(cx);
        cx.notify();
    }

    /// Switches the list to search results and focuses the search field.
    #[cfg(debug_assertions)]
    pub fn start_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).text().to_owned();
        self.set_view(View::Search(query), cx);
        window.focus(&self.search_input.focus_handle(cx), cx);
        cx.notify();
    }

    /// Shows the full results for `query` in the list (the popover's "View more").
    pub fn show_search_results(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_input.update(cx, |input, cx| input.set_text(query.clone(), cx));
        self.set_view(View::Search(query), cx);
    }

    /// Leaves search results for the Inbox.
    pub fn stop_search(&mut self, cx: &mut Context<Self>) {
        if matches!(self.view, View::Search(_)) {
            self.set_view(View::Inbox, cx);
        }
    }

    pub(super) fn bind_task_list_actions<E: InteractiveElement>(&self, el: E, cx: &mut Context<Self>) -> E {
        el.key_context("TaskList")
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_all_visible))
            .on_action(cx.listener(|this, _: &ExtendSelectionNext, _, cx| this.extend_selection(1, cx)))
            .on_action(cx.listener(|this, _: &ExtendSelectionPrevious, _, cx| this.extend_selection(-1, cx)))
            .on_action(cx.listener(Self::toggle_selected))
            .on_action(cx.listener(Self::toggle_in_progress_selected))
            .on_action(cx.listener(Self::delete_selected))
            .on_action(cx.listener(Self::pin_selected))
            .on_action(cx.listener(Self::date_selected))
            .on_action(cx.listener(|this, _: &ClearDate, _, cx| this.set_selected_due(None, cx)))
            .on_action(cx.listener(|this, _: &SetToday, _, cx| this.set_selected_due(Some(0), cx)))
            .on_action(cx.listener(|this, _: &SetTomorrow, _, cx| this.set_selected_due(Some(1), cx)))
            .on_action(cx.listener(|this, _: &SetNextWeek, _, cx| this.set_selected_due(Some(7), cx)))
            .on_action(cx.listener(|this, _: &PriorityNone, _, cx| this.set_selected_priority(Priority::None, cx)))
            .on_action(cx.listener(|this, _: &PriorityLow, _, cx| this.set_selected_priority(Priority::Low, cx)))
            .on_action(cx.listener(|this, _: &PriorityMedium, _, cx| this.set_selected_priority(Priority::Medium, cx)))
            .on_action(cx.listener(|this, _: &PriorityHigh, _, cx| this.set_selected_priority(Priority::High, cx)))
            .on_action(cx.listener(Self::toggle_all_groups))
            .on_action(cx.listener(Self::toggle_all_subtasks))
            .on_action(cx.listener(Self::add_subtask_selected))
            .on_action(cx.listener(Self::open_selected_as_sticky))
            .on_action(cx.listener(|this, _: &ViewList, _, cx| {
                if this.title_edit.is_none() {
                    this.set_layout(false, false, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ViewKanban, _, cx| {
                if this.title_edit.is_none() && this.supports_board_layouts() {
                    this.set_layout(true, false, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ViewTimeline, _, cx| {
                if this.title_edit.is_none() && this.supports_board_layouts() {
                    this.set_layout(false, true, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &crate::actions::Undo, _, cx| this.apply_undo(cx)))
            .on_action(cx.listener(|this, _: &crate::actions::Redo, _, cx| this.apply_redo(cx)))
    }

    pub(super) fn toggle_task_collapsed(&mut self, id: Id, cx: &mut Context<Self>) {
        if !self.collapsed_tasks.remove(&id) {
            self.collapsed_tasks.insert(id);
        }
        self.invalidate_rows();
        cx.notify();
    }

    pub(super) fn toggle_day(&mut self, label: String, cx: &mut Context<Self>) {
        if !self.collapsed_days.remove(&label) {
            self.collapsed_days.insert(label);
        }
        self.invalidate_rows();
        cx.notify();
    }

    pub(super) fn toggle_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if !self.collapsed_sections.remove(&section) {
            self.collapsed_sections.insert(section);
        }
        self.invalidate_rows();
        cx.notify();
    }

    pub fn toggle_lists(&mut self, cx: &mut Context<Self>) {
        self.show_lists = !self.show_lists;
        cx.notify();
    }

    pub fn select(&mut self, id: Id, cx: &mut Context<Self>) {
        self.multi_selected.clear();
        self.batch_menu = None;
        self.selection_anchor = Some(id);
        if self.selected == Some(id) {
            return;
        }
        self.selected = Some(id);
        // The task may be outside the current view, for example a subtask opened from the panel.
        let task = self
            .tasks
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .or_else(|| self.store.task(id).ok().flatten());
        self.detail.update(cx, |detail, cx| detail.show(task, cx));
        cx.notify();
    }

    fn finish_complete_task(&mut self, task: &Task, cx: &mut Context<Self>) {
        let result = crate::undo::complete(&self.store, task);
        if let Some(action) = self.check(result) {
            let label = i18n::t(cx).undo_completed.replace("{title}", &task.title);
            self.offer_undo(action, label, cx);
            crate::completion_mark::record(task.id, cx);
            desktop_common::sound::play_complete(&self.store);
        }
    }

    pub(super) fn toggle(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
        if self.is_task_id_read_only(id) {
            return;
        }
        if let Some(pending) = self.completing.get(&id) {
            // A second click cancels this completion, including a repeating occurrence.
            // Repeated complete commands must not advance the recurrence again.
            if completed { return; }
            let action = pending.undo.clone();
            if self.check(action.apply(&self.store)).is_some() {
                self.completing.remove(&id);
                if self.undo.as_ref().is_some_and(|(undo, _, _)| *undo == action) {
                    self.undo = None;
                }
            }
            self.reload(cx);
            return;
        }
        // Unchecking a "Won't Do" task reopens it, as in the original.
        let wont_do = self.tasks.iter().any(|t| t.id == id && t.is_wont_do());
        let task = self.tasks.iter().find(|t| t.id == id).cloned();
        match task {
            // Completing offers Undo, also for a repeating task, which moves to its next date.
            Some(task) if completed && !wont_do => {
                if desktop_common::plugin_bus::PluginHooksGlobal::global(cx)
                    .is_some_and(|h| h.is_active())
                {
                    if !self.pending_complete.insert(id) {
                        return;
                    }
                    let weak = cx.weak_entity();
                    desktop_common::plugin_bus::run_before_task_complete_hook(
                        &task,
                        cx,
                        move |allowed, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.pending_complete.remove(&id);
                                if !allowed || this.is_task_id_read_only(id) {
                                    cx.notify();
                                    return;
                                }
                                let current = this
                                    .tasks
                                    .iter()
                                    .find(|t| t.id == id)
                                    .cloned()
                                    .or_else(|| this.store.task(id).ok().flatten());
                                if let Some(current) = current
                                    && !current.is_closed()
                                {
                                    this.finish_complete_task(&current, cx);
                                    this.reload(cx);
                                }
                            });
                        },
                    );
                    return;
                }
                self.finish_complete_task(&task, cx);
            }
            _ => {
                let result =
                    if wont_do { self.store.set_wont_do(id, false) } else { self.store.set_completed(id, completed) };
                self.check(result);
            }
        }
        self.reload(cx);
    }

    /// Shows the "Undo" toast for a few seconds.
    pub(super) fn offer_undo(&mut self, action: crate::undo::UndoAction, label: String, cx: &mut Context<Self>) {
        let completed = match &action {
            crate::undo::UndoAction::Reopen { id, .. } | crate::undo::UndoAction::ReopenRepeat { id, .. } => Some(*id),
            _ => None,
        };
        if !cx.reduce_motion()
            && let Some(id) = completed
            && let Some((index, before)) = self.tasks_for_rows().into_iter().enumerate()
                .find(|(_, task)| task.id == id && !task.is_closed())
        {
            let timer = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(crate::completion_mark::SETTLE_DURATION).await;
                this.update(cx, |this, cx| {
                    this.completing.remove(&id);
                    this.invalidate_rows();
                    cx.notify();
                }).ok();
            });
            self.completing.insert(id, CompletingTask {
                before, index, view: self.view.clone(), undo: action.clone(), _timer: timer,
            });
            self.invalidate_rows();
        }
        let expected = action.clone();
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(5)).await;
            this.update(cx, |this, cx| {
                if this.undo.as_ref().is_some_and(|(a, _, _)| *a == expected) {
                    this.undo = None;
                    cx.notify();
                }
            })
            .ok();
        });
        self.redo = None;
        self.undo = Some((action, label, timer));
        cx.notify();
    }

    pub(super) fn apply_undo(&mut self, cx: &mut Context<Self>) {
        if let Some((action, label, _)) = self.undo.take() {
            let result = action.apply(&self.store);
            if self.check(result).is_some() {
                self.completing.retain(|_, pending| pending.undo != action);
                self.redo = Some((action, label));
            }
            self.reload(cx);
        }
    }

    fn apply_redo(&mut self, cx: &mut Context<Self>) {
        if let Some((action, label)) = self.redo.take() {
            let result = action.redo(&self.store);
            if let Some(Some(new_undo)) = self.check(result) {
                self.offer_undo(new_undo, label, cx);
            }
            self.reload(cx);
        }
    }

    /// Moves a task to the trash, or asks for confirmation to delete it forever when already there.
    /// Read-only archived-list tasks cannot be deleted.
    pub(super) fn delete(&mut self, id: Id, cx: &mut Context<Self>) {
        let in_trash = self.view == View::Trash
            || self.tasks.iter().find(|t| t.id == id).is_some_and(Task::is_deleted);
        if in_trash {
            self.request_purge_task(id, cx);
            return;
        }
        if self.is_task_id_read_only(id) {
            return;
        }
        let title = self.tasks.iter().find(|t| t.id == id).map(|t| t.title.clone()).unwrap_or_default();
        let result = self.store.delete_task(id);
        if self.check(result).is_some() {
            let label = i18n::t(cx).undo_deleted.replace("{title}", &title);
            self.offer_undo(crate::undo::UndoAction::Restore(id), label, cx);
        }
        self.reload(cx);
    }

    /// Completes a task dropped onto a Completed target via `desktop_common::task_ops::toggle_task_completion`
    /// so repeating tasks advance cleanly, completion sound plays after DB success, and Undo is offered.
    /// Returns `Ok(true)` when a repeating task advanced to its next occurrence (and stayed open).
    pub(super) fn complete_task_from_drop(&mut self, task: &Task, cx: &mut Context<Self>) -> anyhow::Result<bool> {
        if task.is_completed() {
            return Ok(false);
        }
        let undo = desktop_common::task_ops::toggle_task_completion(
            &self.store,
            task,
            true,
            cx.entity_id(),
            cx,
        )?;
        let advanced_repeat = matches!(undo, Some(crate::undo::UndoAction::ReopenRepeat { .. }));
        if let Some(action) = undo {
            let label = i18n::t(cx).undo_completed.replace("{title}", &task.title);
            self.offer_undo(action, label, cx);
            crate::completion_mark::record(task.id, cx);
        }
        Ok(advanced_repeat)
    }

    /// A task dropped on an entry of the lists column takes that entry's meaning: a list moves it,
    /// a tag tags it, Today/Tomorrow set its date, Completed completes it, Trash deletes it.
    pub(super) fn drop_on_view(&mut self, view: View, id: Id, cx: &mut Context<Self>) {
        self.clear_task_drag_state();
        if self.is_task_id_read_only(id) {
            return;
        }
        if matches!(view, View::List(list_id) if self.is_list_archived(list_id)) {
            return;
        }
        let today = Self::today();
        let existing = self.store.task(id).ok().flatten();
        let time = existing.as_ref().and_then(|t| t.due_time);
        let result = match &view {
            View::Inbox => self.store.set_list(id, None),
            View::List(list) => self.store.set_list(id, Some(*list)),
            View::Tag(tag) => match existing {
                Some(task) if !task.tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) => {
                    let mut tags = task.tags;
                    tags.push(tag.clone());
                    self.store.set_tags(id, &tags)
                }
                _ => Ok(()),
            },
            View::Today | View::Next7Days => self.store.set_due(id, Some(today), time),
            View::Tomorrow => self.store.set_due(id, Some(today + Duration::days(1)), time),
            View::Completed => match existing {
                Some(task) => self.complete_task_from_drop(&task, cx).map(|_| ()),
                None => Ok(()),
            },
            View::WontDo => self.store.set_wont_do(id, true),
            View::Trash => self.store.delete_task(id),
            View::Day(day) => self.store.set_due(id, Some(*day), time),
            View::All | View::Search(_) | View::Filter(_) | View::Horizons | View::Summary => Ok(()),
        };
        self.check(result);
        self.reload(cx);
    }

    /// Drops a dragged task right before `target`, inheriting `target`'s parent (used by dev-state `move=A>B`).
    #[cfg(debug_assertions)]
    pub(super) fn drop_task(&mut self, dragged: Id, target: Id, cx: &mut Context<Self>) {
        let parent = self.store.task(target).ok().flatten().and_then(|t| t.parent_id);
        let result = self.store.move_task_in_tree(dragged, parent, Some((target, false)));
        self.check(result);
        self.reload(cx);
    }

    /// Parents in the current view that have at least one visible subtask.
    pub(super) fn parent_task_ids(&self, strings: &Strings) -> Vec<Id> {
        self.rows_with(strings, &HashSet::new(), &HashSet::new())
            .into_iter()
            .filter_map(|row| match row {
                Row::Task { task, has_children: true, .. } => Some(task.id),
                _ => None,
            })
            .collect()
    }

    pub(super) fn toggle_all_subtasks_now(&mut self, cx: &mut Context<Self>) {
        let parents = self.parent_task_ids(i18n::t(cx));
        if parents.is_empty() {
            return;
        }
        let any_expanded = parents.iter().any(|id| !self.collapsed_tasks.contains(id));
        if any_expanded {
            self.collapsed_tasks.extend(parents);
        } else {
            for id in parents {
                self.collapsed_tasks.remove(&id);
            }
        }
        self.invalidate_rows();
        cx.notify();
    }

    pub(super) fn supports_board_layouts(&self) -> bool {
        !matches!(self.view, View::Trash | View::Completed | View::WontDo | View::Search(_))
    }

    fn toggle_all_groups(&mut self, _: &ToggleAllGroups, _: &mut Window, cx: &mut Context<Self>) {
        if self.title_edit.is_some() {
            return;
        }
        let strings = i18n::t(cx);
        let rows = self.rows_with(strings, &self.collapsed_tasks, &HashSet::new());
        let mut sections: Vec<Section> = Vec::new();
        let mut days: Vec<String> = Vec::new();
        for row in rows {
            match row {
                Row::Section { section, .. } => {
                    if !sections.contains(&section) {
                        sections.push(section);
                    }
                }
                Row::Day { label, .. } => {
                    if !days.contains(&label) {
                        days.push(label);
                    }
                }
                Row::Task { .. } => {}
            }
        }
        if sections.is_empty() && days.is_empty() {
            return;
        }
        let any_expanded = sections.iter().any(|s| !self.collapsed_sections.contains(s))
            || days.iter().any(|d| !self.collapsed_days.contains(d));
        if any_expanded {
            self.collapsed_sections.extend(sections);
            self.collapsed_days.extend(days);
        } else {
            for s in sections {
                self.collapsed_sections.remove(&s);
            }
            for d in days {
                self.collapsed_days.remove(&d);
            }
        }
        self.invalidate_rows();
        cx.notify();
    }

    fn open_selected_as_sticky(&mut self, _: &OpenAsSticky, _: &mut Window, cx: &mut Context<Self>) {
        if self.title_edit.is_some() || matches!(self.view, View::Trash) {
            return;
        }
        if let Some(id) = self.selected {
            if self.is_task_id_read_only(id) {
                return;
            }
            desktop_common::open_sticky_note(self.store.clone(), id, cx);
        }
    }

    fn toggle_all_subtasks(&mut self, _: &ToggleAllSubtasks, _: &mut Window, cx: &mut Context<Self>) {
        if self.title_edit.is_some() {
            return;
        }
        self.toggle_all_subtasks_now(cx);
    }

    fn add_subtask_selected(&mut self, _: &AddSubtask, window: &mut Window, cx: &mut Context<Self>) {
        if self.title_edit.is_some() || self.view == View::Trash {
            return;
        }
        if let Some(id) = self.selected {
            if self.is_task_id_read_only(id) {
                return;
            }
            self.start_detail(id, "subtask", window, cx);
        }
    }

    /// The details' own picker for a task, for the right-click menu and the calendar's.
    pub fn start_detail(&mut self, id: Id, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.kanban && !self.timeline && self.selected.is_none() {
            self.open_card(id, cx);
            self.card_detail.update(cx, |detail, cx| detail.start(what, window, cx));
        } else {
            self.select(id, cx);
            self.detail.update(cx, |detail, cx| detail.start(what, window, cx));
        }
    }
}
