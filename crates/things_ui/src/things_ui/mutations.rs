use chrono::Duration;
use desktop_common::plugin_bus::{PluginHooksGlobal, run_before_task_complete_hook};
use desktop_common::text_input::{Changed, Mode, Submitted, TextInput};
use desktop_common::{calendar, completion_mark, data_version, task_ops};
use fern_core::{Id, ScheduleBucket, Task, TaskDraft};
use fern_i18n as i18n;
use gpui::{Context, Focusable, Window};
use task_ui::due_picker::DueDraft;
use task_ui::list_dialog::{DialogEvent, FolderDialog, ListDialog};
use task_ui::tag_dialog::{TagDialog, TagEvent};
use ui::prelude::*;

use super::bottom_bar::BottomBarAction;
use super::inline_card::{CardAction, CardPopover, ChecklistRowState, ExpandedCardState};
use super::magic_plus::MagicPlusDropTarget;
use super::sidebar::{DropDestination, SidebarAction};
use super::when_picker::{
    DeadlineAction, DeadlinePickerState, DeadlineTarget, WhenAction, WhenPickerState,
};
use super::{GroupDrop, PopoverAnchor, ThingsEvent, ThingsView, ThingsViewKind};

impl ThingsView {
    fn mark_expanded_touched(&mut self, task_id: Id) {
        if let Some(card) = &mut self.expanded
            && card.task_id == task_id
        {
            card.created_empty = false;
        }
    }

    /// Flushes any uncommitted text typed in the expanded card's "add checklist item" input.
    fn flush_expanded_checklist_input(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(card) = &self.expanded else {
            return false;
        };
        let task_id = card.task_id;
        let pending = card.new_checklist_item.read(cx).text().trim().to_owned();
        if pending.is_empty() {
            return false;
        }
        if self.store.add_checklist_item(task_id, &pending, None).is_ok() {
            card.new_checklist_item
                .update(cx, |input, cx| input.set_text("", cx));
            true
        } else {
            false
        }
    }

    /// Collapses `self.expanded`, flushing any uncommitted checklist draft and purging untouched
    /// newly created empty tasks so accidental `+ New To-Do` clicks never leak `"Untitled"` rows.
    /// Returns `(was_expanded, did_reload)`.
    pub(super) fn finish_expanded_card(&mut self, cx: &mut Context<Self>) -> (bool, bool) {
        self.pending_checklist_focus = None;
        let Some(card) = self.expanded.take() else {
            return (false, false);
        };
        let task_id = card.task_id;
        let mut mutated = false;

        let pending_item = card.new_checklist_item.read(cx).text().trim().to_owned();
        if !pending_item.is_empty()
            && self
                .store
                .add_checklist_item(task_id, &pending_item, None)
                .is_ok()
        {
            mutated = true;
        }

        if card.created_empty {
            let title_empty = card.title.read(cx).text().trim().is_empty();
            let notes_empty = card.notes.read(cx).text().trim().is_empty();
            let checklist_empty = pending_item.is_empty()
                && self
                    .store
                    .checklist(task_id)
                    .map(|items| items.is_empty())
                    .unwrap_or(false);
            let untouched_meta = self.store.task(task_id).ok().flatten().is_some_and(|t| {
                t.tags.is_empty() && t.due_date.is_none() && t.horizon.is_none() && !t.is_closed()
            });
            if title_empty
                && notes_empty
                && checklist_empty
                && untouched_meta
                && self.store.purge_task(task_id).is_ok()
            {
                mutated = true;
                if self.selected_id == Some(task_id) {
                    self.selected_id = None;
                }
            }
        }

        if mutated {
            data_version::data_changed(cx.entity_id(), cx);
            self.reload(cx);
            (true, true)
        } else {
            (true, false)
        }
    }

    fn apply_task_deadline(&self, id: Id, date: Option<chrono::NaiveDate>) -> anyhow::Result<()> {
        let existing = self
            .tasks
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .or_else(|| self.store.task(id).ok().flatten());
        match date {
            None => {
                self.store.set_due(id, None, None)?;
                self.store.set_due_end(id, None)?;
                Ok(())
            }
            Some(new_date) => {
                let time = existing.as_ref().and_then(|t| t.due_time);
                self.store.set_due(id, Some(new_date), time)?;
                if let Some(task) = &existing
                    && let Some(end_date) = task.end_date
                    && new_date > end_date
                {
                    let clamped_end_time = match (time, task.end_time) {
                        (Some(st), Some(et)) if et < st => Some(st),
                        _ => task.end_time,
                    };
                    self.store
                        .set_due_end(id, Some((new_date, clamped_end_time)))?;
                }
                Ok(())
            }
        }
    }

    pub(super) fn open_when_for_active_task(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(task_id) = self.active_target_task_id() else {
            return false;
        };
        let anchor = if self.expanded.as_ref().is_some_and(|e| e.task_id == task_id) {
            PopoverAnchor::Card
        } else {
            PopoverAnchor::BottomBar
        };
        self.open_when_picker(anchor, task_id, window, cx);
        true
    }

    pub(super) fn build_checklist_row(
        item: &fern_core::ChecklistItem,
        cx: &mut Context<Self>,
    ) -> ChecklistRowState {
        let item_id = item.id;
        let initial_title = item.title.clone();
        let input = cx.new(|cx| {
            let mut ti = TextInput::new(Mode::SingleLine, "", cx).without_cancel();
            ti.set_text(initial_title.clone(), cx);
            ti
        });
        let sub = cx.subscribe(&input, move |this, input, _: &Changed, cx| {
            let text = input.read(cx).text().to_owned();
            if let Some(card) = &mut this.expanded
                && let Some(row) = card.checklist_rows.iter_mut().find(|r| r.id == item_id)
            {
                row.title_text = text.clone();
            }
            if this.store.set_checklist_item_title(item_id, &text).is_ok() {
                data_version::data_changed(cx.entity_id(), cx);
            }
        });
        let sub_submit = cx.subscribe(&input, move |this, _, _: &Submitted, cx| {
            let Some(task_id) = this.expanded.as_ref().map(|c| c.task_id) else {
                return;
            };
            if let Ok(created) = this.store.add_checklist_item(task_id, "", Some(item_id)) {
                this.mark_expanded_touched(task_id);
                this.pending_checklist_focus = Some(created.id);
                data_version::data_changed(cx.entity_id(), cx);
                this.reload(cx);
            }
        });
        ChecklistRowState {
            id: item_id,
            completed: item.completed,
            title_text: initial_title,
            input,
            _subscriptions: vec![sub, sub_submit],
        }
    }

    pub(super) fn expand_task(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        self.expand_task_inner(id, false, window, cx);
    }

    fn expand_task_inner(
        &mut self,
        id: Id,
        created_empty: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.expanded.as_ref().is_some_and(|c| c.task_id == id) {
            if let Some(card) = &self.expanded {
                window.focus(&card.title.focus_handle(cx), cx);
            }
            return;
        }
        if self.expanded.is_some() {
            self.finish_expanded_card(cx);
        }
        let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        let checklist = self.store.checklist(id).unwrap_or_default();
        let strings = i18n::t(cx);

        let title = cx.new(|cx| {
            let mut ti = TextInput::new(Mode::Wrapped, strings.untitled, cx).without_cancel();
            if !task.title.is_empty() {
                ti.set_text(task.title.clone(), cx);
            }
            ti
        });
        let notes = cx.new(|cx| {
            let mut ti =
                TextInput::new(Mode::MultiLine, strings.things_notes_placeholder, cx)
                    .without_cancel();
            if !task.notes.is_empty() {
                ti.set_text(task.notes.clone(), cx);
            }
            ti
        });
        let new_checklist_item = cx.new(|cx| {
            TextInput::new(Mode::SingleLine, strings.things_checklist_add, cx).without_cancel()
        });
        let tag_input =
            cx.new(|cx| TextInput::new(Mode::SingleLine, strings.type_in_tag, cx).without_cancel());

        let checklist_rows = checklist
            .iter()
            .map(|item| Self::build_checklist_row(item, cx))
            .collect();

        let subs = vec![
            cx.subscribe(&title, move |this, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                if this.store.set_title(id, &text).is_ok() {
                    if let Some(t) = this.tasks.iter_mut().find(|t| t.id == id) {
                        t.title = text;
                    }
                    this.invalidate_groups();
                    data_version::data_changed(cx.entity_id(), cx);
                    cx.notify();
                }
            }),
            cx.subscribe_in(&title, window, |this, _, _: &Submitted, window, cx| {
                this.close_all_popovers();
                let (_, reloaded) = this.finish_expanded_card(cx);
                window.focus(&this.focus_handle, cx);
                if !reloaded {
                    this.reload(cx);
                }
            }),
            cx.subscribe(&notes, move |this, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                if this.store.set_notes(id, &text).is_ok() {
                    if let Some(t) = this.tasks.iter_mut().find(|t| t.id == id) {
                        t.notes = text;
                    }
                    this.invalidate_groups();
                    data_version::data_changed(cx.entity_id(), cx);
                    cx.notify();
                }
            }),
            cx.subscribe_in(&new_checklist_item, window, move |this, input, Submitted(text), window, cx| {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return;
                }
                if this.store.add_checklist_item(id, trimmed, None).is_ok() {
                    this.mark_expanded_touched(id);
                    input.update(cx, |i, cx| i.set_text("", cx));
                    data_version::data_changed(cx.entity_id(), cx);
                    this.reload(cx);
                    window.focus(&input.focus_handle(cx), cx);
                }
            }),
            cx.subscribe(&tag_input, |_, _, _: &Changed, cx| cx.notify()),
            cx.subscribe(&tag_input, move |this, input, Submitted(text), cx| {
                let tag = text.trim().trim_start_matches('#').trim().to_owned();
                if tag.is_empty() {
                    return;
                }
                this.toggle_task_tag(id, &tag, cx);
                input.update(cx, |i, cx| i.set_text("", cx));
            }),
        ];

        self.close_all_popovers();
        self.selected_id = Some(id);
        window.focus(&title.focus_handle(cx), cx);
        self.expanded = Some(ExpandedCardState {
            task_id: id,
            title,
            notes,
            checklist_rows,
            new_checklist_item,
            show_checklist: false,
            tag_input,
            active_popover: None,
            created_empty,
            _subscriptions: subs,
        });
        cx.notify();
    }

    pub(super) fn toggle_task_tag(&mut self, task_id: Id, tag_name: &str, cx: &mut Context<Self>) -> bool {
        let Some(task) = self.tasks.iter().find(|t| t.id == task_id) else {
            return false;
        };
        let mut next = task.tags.clone();
        let needle = tag_name.to_lowercase();
        if let Some(pos) = next.iter().position(|t| t.to_lowercase() == needle) {
            next.remove(pos);
        } else {
            next.push(tag_name.to_owned());
        }
        if self.store.set_tags(task_id, &next).is_ok() {
            self.mark_expanded_touched(task_id);
            data_version::data_changed(cx.entity_id(), cx);
            self.reload(cx);
            true
        } else {
            false
        }
    }

    fn finish_toggle_task_complete(
        &mut self,
        task: &Task,
        done: bool,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let id = task.id;
        let was_expanded = self.expanded.is_some();
        let completing_expanded =
            done && self.expanded.as_ref().is_some_and(|e| e.task_id == id);
        if completing_expanded {
            self.flush_expanded_checklist_input(cx);
            self.close_all_popovers();
            self.expanded = None;
        } else {
            self.mark_expanded_touched(id);
        }
        if done {
            completion_mark::record(id, cx);
        }
        if task_ops::toggle_task_completion(&self.store, task, done, cx.entity_id(), cx).is_ok() {
            self.reload(cx);
            if was_expanded
                && self.expanded.is_none()
                && let Some(window) = window
            {
                window.focus(&self.focus_handle, cx);
            }
        }
    }

    pub(super) fn toggle_task_complete(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        let done = !task.is_closed();
        if done
            && !task.is_wont_do()
            && PluginHooksGlobal::global(cx).is_some_and(|h| h.is_active())
        {
            if !self.pending_complete.insert(id) {
                return;
            }
            let weak = cx.weak_entity();
            let window_handle = window.window_handle();
            run_before_task_complete_hook(&task, cx, move |allowed, cx| {
                let mut should_refocus = None;
                let _ = weak.update(cx, |this, cx| {
                    this.pending_complete.remove(&id);
                    if !allowed {
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
                        let was_expanded = this.expanded.is_some();
                        this.finish_toggle_task_complete(&current, true, None, cx);
                        if was_expanded && this.expanded.is_none() {
                            should_refocus = Some(this.focus_handle.clone());
                        }
                    }
                });
                if let Some(handle) = should_refocus {
                    let _ = window_handle.update(cx, |_, window, cx| {
                        window.focus(&handle, cx);
                    });
                }
            });
            return;
        }
        self.finish_toggle_task_complete(&task, done, Some(window), cx);
    }

    pub(super) fn delete_task(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        if self.store.delete_task(id).is_ok() {
            if self.expanded.as_ref().is_some_and(|e| e.task_id == id) {
                self.close_all_popovers();
                self.expanded = None;
                window.focus(&self.focus_handle, cx);
            }
            data_version::data_changed(cx.entity_id(), cx);
            self.reload(cx);
        }
    }

    pub(super) fn default_drop_target_for_view(&self) -> MagicPlusDropTarget {
        let today = Self::today();
        match self.view {
            ThingsViewKind::Inbox | ThingsViewKind::Logbook => MagicPlusDropTarget {
                list_id: None,
                section_id: None,
                start_date: None,
                schedule_bucket: ScheduleBucket::None,
                after_task: None,
            },
            ThingsViewKind::Today => MagicPlusDropTarget {
                list_id: None,
                section_id: None,
                start_date: Some(today),
                schedule_bucket: ScheduleBucket::None,
                after_task: None,
            },
            ThingsViewKind::Upcoming => MagicPlusDropTarget {
                list_id: None,
                section_id: None,
                start_date: Some(today + Duration::days(1)),
                schedule_bucket: ScheduleBucket::None,
                after_task: None,
            },
            ThingsViewKind::Anytime => MagicPlusDropTarget {
                list_id: None,
                section_id: None,
                start_date: Some(today),
                schedule_bucket: ScheduleBucket::None,
                after_task: None,
            },
            ThingsViewKind::Someday => MagicPlusDropTarget {
                list_id: None,
                section_id: None,
                start_date: None,
                schedule_bucket: ScheduleBucket::Someday,
                after_task: None,
            },
            ThingsViewKind::Project(list_id) => MagicPlusDropTarget {
                list_id: Some(list_id),
                section_id: None,
                start_date: None,
                schedule_bucket: ScheduleBucket::None,
                after_task: None,
            },
            ThingsViewKind::Area(folder_id) => {
                let first_list = self
                    .lists
                    .iter()
                    .find(|l| l.folder == Some(folder_id) && !l.archived && l.completed_at.is_none())
                    .map(|l| l.id);
                MagicPlusDropTarget {
                    list_id: first_list,
                    section_id: None,
                    start_date: None,
                    schedule_bucket: ScheduleBucket::None,
                    after_task: None,
                }
            }
        }
    }

    pub(super) fn create_todo_at(
        &mut self,
        mut target: MagicPlusDropTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_all_popovers();
        self.finish_expanded_card(cx);
        if self.view == ThingsViewKind::Logbook {
            self.view = ThingsViewKind::Inbox;
            target = MagicPlusDropTarget::default();
        } else if matches!(self.view, ThingsViewKind::Area(_)) && target.list_id.is_none() {
            // An Area with no active projects cannot display unprojected tasks;
            // switch cleanly to Inbox so the new card is immediately visible and editable.
            self.view = ThingsViewKind::Inbox;
            target = MagicPlusDropTarget::default();
        }
        let draft = TaskDraft {
            title: String::new(),
            list_id: target.list_id,
            section_id: target.section_id,
            start_date: target.start_date,
            schedule_bucket: target.schedule_bucket,
            ..Default::default()
        };
        let Ok(created) = self.store.create_task(draft, false) else {
            return;
        };
        if let Some((anchor_id, after)) = target.after_task {
            let _ = self.store.move_task(created.id, anchor_id, after);
        }
        data_version::data_changed(cx.entity_id(), cx);
        self.reload(cx);
        self.selected_id = Some(created.id);
        self.expand_task_inner(created.id, true, window, cx);
    }

    pub(super) fn create_new_todo_default(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.default_drop_target_for_view();
        self.create_todo_at(target, window, cx);
    }

    fn open_when_picker(
        &mut self,
        anchor: PopoverAnchor,
        task_id: Id,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(task) = self.tasks.iter().find(|t| t.id == task_id).cloned() else {
            return;
        };
        let today = Self::today();
        let initial_month = calendar::month_start(task.start_date.unwrap_or(today));
        let strings = i18n::t(cx);
        let initial_text = task
            .start_date
            .map(|d| calendar::format_date(d, strings))
            .unwrap_or_default();
        let date_placeholder = strings.things_date_placeholder;

        let date_input = cx.new(|cx| {
            let mut ti = TextInput::new(Mode::SingleLine, date_placeholder, cx).without_cancel();
            if !initial_text.is_empty() {
                ti.set_text(initial_text, cx);
            }
            ti
        });
        cx.subscribe(&date_input, |this, input, _: &Changed, cx| {
            if this.when_date_error {
                this.when_date_error = false;
                input.update(cx, |ti, cx| ti.set_ink_override(None, None, None, cx));
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&date_input, window, move |this, input, Submitted(text), window, cx| {
            let strings = i18n::t(cx);
            if let Some(date) = calendar::parse_date(text, Self::today(), strings) {
                this.when_date_error = false;
                input.update(cx, |ti, cx| ti.set_ink_override(None, None, None, cx));
                this.apply_when(task_id, Some(date), ScheduleBucket::None, window, cx);
            } else {
                this.when_date_error = true;
                let err_color = cx.theme().status().error;
                input.update(cx, |ti, cx| {
                    ti.set_ink_override(Some(err_color), None, Some(err_color), cx);
                });
                cx.notify();
            }
        })
        .detach();

        self.close_all_popovers();
        if anchor == PopoverAnchor::Card
            && let Some(card) = &mut self.expanded
            && card.task_id == task_id
        {
            card.active_popover = Some(CardPopover::When);
        }
        window.focus(&date_input.focus_handle(cx), cx);
        self.when_picker = Some((
            anchor,
            WhenPickerState {
                task_id,
                month: initial_month,
                start_date: task.start_date,
                bucket: task.schedule_bucket,
                horizon: task.horizon,
                date_input,
            },
        ));
        cx.notify();
    }

    fn apply_when(
        &mut self,
        task_id: Id,
        start_date: Option<chrono::NaiveDate>,
        bucket: ScheduleBucket,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cleared_horizon = if start_date.is_none() && bucket == ScheduleBucket::None {
            self.store.set_horizon(task_id, None)
        } else {
            Ok(())
        };
        if cleared_horizon.is_ok()
            && self
                .store
                .set_task_schedule(task_id, start_date, bucket)
                .is_ok()
        {
            self.mark_expanded_touched(task_id);
            data_version::data_changed(cx.entity_id(), cx);
            self.close_all_popovers();
            self.reload(cx);
            self.restore_focus(window, cx);
        }
    }

    pub(super) fn open_deadline_picker(
        &mut self,
        anchor: PopoverAnchor,
        target: DeadlineTarget,
        cx: &mut Context<Self>,
    ) {
        let today = Self::today();
        let draft = match target {
            DeadlineTarget::Task(id) => {
                let Some(task) = self.tasks.iter().find(|t| t.id == id) else {
                    return;
                };
                DueDraft::from_task(task, today)
            }
            DeadlineTarget::Project(id) => {
                let deadline = self.lists.iter().find(|l| l.id == id).and_then(|l| l.deadline);
                DueDraft {
                    date: deadline,
                    month: calendar::month_start(deadline.unwrap_or(today)),
                    ..Default::default()
                }
            }
        };
        self.close_all_popovers();
        if anchor == PopoverAnchor::Card
            && let Some(card) = &mut self.expanded
        {
            card.active_popover = Some(CardPopover::Deadline);
        }
        self.deadline_picker = Some((anchor, DeadlinePickerState { target, draft }));
        cx.notify();
    }

    pub(super) fn handle_when_action(
        &mut self,
        action: WhenAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            WhenAction::Schedule { start_date, bucket } => {
                if let Some((_, state)) = &self.when_picker {
                    let task_id = state.task_id;
                    self.apply_when(task_id, start_date, bucket, window, cx);
                }
            }
            WhenAction::SetHorizon(horizon) => {
                if let Some((_, state)) = &self.when_picker {
                    let task_id = state.task_id;
                    if self.store.set_horizon(task_id, Some(horizon)).is_ok() {
                        self.mark_expanded_touched(task_id);
                        data_version::data_changed(cx.entity_id(), cx);
                        self.close_all_popovers();
                        self.reload(cx);
                        self.restore_focus(window, cx);
                    }
                }
            }
            WhenAction::ShiftMonth(delta) => {
                if let Some((_, state)) = &mut self.when_picker {
                    state.month = calendar::add_months(state.month, delta);
                    cx.notify();
                }
            }
            WhenAction::Close => {
                self.close_all_popovers();
                self.restore_focus(window, cx);
                cx.notify();
            }
        }
    }

    pub(super) fn handle_deadline_action(
        &mut self,
        action: DeadlineAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            DeadlineAction::PickDate(date) => {
                if let Some((_, state)) = &mut self.deadline_picker {
                    state.draft.pick_date(date);
                    cx.notify();
                }
            }
            DeadlineAction::ShiftMonth(delta) => {
                if let Some((_, state)) = &mut self.deadline_picker {
                    state.draft.month = calendar::add_months(state.draft.month, delta);
                    cx.notify();
                }
            }
            DeadlineAction::Apply(date) => {
                if let Some((_, state)) = self.deadline_picker.take() {
                    let res = match state.target {
                        DeadlineTarget::Task(id) => {
                            self.mark_expanded_touched(id);
                            self.apply_task_deadline(id, date)
                        }
                        DeadlineTarget::Project(id) => {
                            if let Some(proj) = self.lists.iter().find(|l| l.id == id) {
                                self.store.set_list_project_fields(
                                    id,
                                    &proj.notes,
                                    date,
                                    proj.completed_at,
                                )
                            } else {
                                Ok(())
                            }
                        }
                    };
                    self.close_all_popovers();
                    if res.is_ok() {
                        data_version::data_changed(cx.entity_id(), cx);
                        self.reload(cx);
                    }
                    self.restore_focus(window, cx);
                    cx.notify();
                }
            }
            DeadlineAction::Close => {
                self.close_all_popovers();
                self.restore_focus(window, cx);
                cx.notify();
            }
        }
    }

    pub(super) fn open_new_project_dialog(
        &mut self,
        folder: Option<Id>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_all_popovers();
        let dialog = cx.new(|cx| ListDialog::new(self.store.clone(), None, folder, window, cx));
        cx.subscribe_in(&dialog, window, |this, _, event: &DialogEvent, window, cx| {
            if let DialogEvent::Saved(id) = event {
                this.finish_expanded_card(cx);
                this.view = ThingsViewKind::Project(*id);
                this.selected_id = None;
                this.show_logged_in_project = false;
                data_version::data_changed(cx.entity_id(), cx);
                this.reload(cx);
            }
            this.list_dialog = None;
            this.restore_focus(window, cx);
            cx.notify();
        })
        .detach();
        self.list_dialog = Some(dialog);
        cx.notify();
    }

    pub(super) fn handle_sidebar_action(
        &mut self,
        action: SidebarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            SidebarAction::Select(view) => self.select_view(view, window, cx),
            SidebarAction::ToggleArea(id) => {
                if !self.collapsed_areas.remove(&id) {
                    self.collapsed_areas.insert(id);
                }
                cx.notify();
            }
            SidebarAction::DropTask { task_id, target } => {
                let today = Self::today();
                let was_expanded = self.expanded.is_some();
                let res = match target {
                    DropDestination::Inbox => self
                        .store
                        .set_list(task_id, None)
                        .and_then(|_| self.store.set_horizon(task_id, None))
                        .and_then(|_| {
                            self.store
                                .set_task_schedule(task_id, None, ScheduleBucket::None)
                        })
                        .and_then(|_| self.apply_task_deadline(task_id, None)),
                    DropDestination::Today => {
                        self.store.set_task_schedule(task_id, Some(today), ScheduleBucket::None)
                    }
                    DropDestination::Upcoming => self.store.set_task_schedule(
                        task_id,
                        Some(today + Duration::days(1)),
                        ScheduleBucket::None,
                    ),
                    DropDestination::Anytime => {
                        let start = self
                            .store
                            .task(task_id)
                            .ok()
                            .flatten()
                            .and_then(|t| (t.list_id.is_none() && t.due_date.is_none()).then_some(today));
                        self.store.set_task_schedule(task_id, start, ScheduleBucket::None)
                    }
                    DropDestination::Someday => {
                        self.store.set_task_schedule(task_id, None, ScheduleBucket::Someday)
                    }
                    DropDestination::Project(list_id) => self.store.set_list(task_id, Some(list_id)),
                };
                if res.is_ok() {
                    self.mark_expanded_touched(task_id);
                    data_version::data_changed(cx.entity_id(), cx);
                    self.reload(cx);
                    if was_expanded && self.expanded.is_none() {
                        window.focus(&self.focus_handle, cx);
                    }
                }
            }
            SidebarAction::ToggleAddMenu => {
                let open = !self.sidebar_add_menu;
                self.close_all_popovers();
                self.sidebar_add_menu = open;
                if !open {
                    self.restore_focus(window, cx);
                }
                cx.notify();
            }
            SidebarAction::CloseAddMenu => {
                self.sidebar_add_menu = false;
                self.restore_focus(window, cx);
                cx.notify();
            }
            SidebarAction::NewProject(folder) => self.open_new_project_dialog(folder, window, cx),
            SidebarAction::NewArea => {
                self.close_all_popovers();
                let dialog = cx.new(|cx| FolderDialog::new(self.store.clone(), None, window, cx));
                cx.subscribe_in(&dialog, window, |this, _, event: &DialogEvent, window, cx| {
                    if let DialogEvent::FolderSaved(_) = event {
                        data_version::data_changed(cx.entity_id(), cx);
                        this.reload(cx);
                    }
                    this.folder_dialog = None;
                    this.restore_focus(window, cx);
                    cx.notify();
                })
                .detach();
                self.folder_dialog = Some(dialog);
                cx.notify();
            }
        }
    }

    pub(super) fn handle_card_action(
        &mut self,
        action: CardAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            CardAction::Select(id) => {
                if self.expanded.as_ref().is_some_and(|e| e.task_id != id) {
                    self.close_all_popovers();
                    self.finish_expanded_card(cx);
                }
                self.selected_id = Some(id);
                window.focus(&self.focus_handle, cx);
                cx.notify();
            }
            CardAction::Expand(id) => self.expand_task(id, window, cx),
            CardAction::Collapse => {
                self.close_all_popovers();
                self.finish_expanded_card(cx);
                window.focus(&self.focus_handle, cx);
                cx.notify();
            }
            CardAction::ToggleComplete(id) => self.toggle_task_complete(id, window, cx),
            CardAction::Delete(id) => self.delete_task(id, window, cx),
            CardAction::StartFocus(task_id, title, stopwatch) => {
                cx.emit(ThingsEvent::StartFocus {
                    task_id,
                    title,
                    stopwatch,
                });
            }
            CardAction::ToggleChecklist => {
                if let Some(card) = &mut self.expanded {
                    card.show_checklist = !card.show_checklist;
                    if card.show_checklist {
                        window.focus(&card.new_checklist_item.focus_handle(cx), cx);
                    }
                    cx.notify();
                }
            }
            CardAction::ToggleChecklistItem(item_id, completed) => {
                if self.store.set_checklist_item_completed(item_id, completed).is_ok() {
                    data_version::data_changed(cx.entity_id(), cx);
                    self.reload(cx);
                }
            }
            CardAction::DeleteChecklistItem(item_id) => {
                if self.store.delete_checklist_item(item_id).is_ok() {
                    data_version::data_changed(cx.entity_id(), cx);
                    self.reload(cx);
                }
            }
            CardAction::ToggleTag(tag) => {
                if let Some(task_id) = self.expanded.as_ref().map(|c| c.task_id) {
                    self.toggle_task_tag(task_id, &tag, cx);
                }
            }
            CardAction::RemoveTag(tag) => {
                if let Some(task_id) = self.expanded.as_ref().map(|c| c.task_id) {
                    self.toggle_task_tag(task_id, &tag, cx);
                }
            }
            CardAction::OpenTagDialog => {
                let target_task = self.expanded.as_ref().map(|c| c.task_id);
                self.close_all_popovers();
                let dialog = cx.new(|cx| TagDialog::new(self.store.clone(), None, None, window, cx));
                cx.subscribe_in(&dialog, window, move |this, _, event: &TagEvent, window, cx| {
                    if let TagEvent::Saved(name) = event {
                        let toggled = target_task
                            .is_some_and(|task_id| this.toggle_task_tag(task_id, name, cx));
                        if !toggled {
                            data_version::data_changed(cx.entity_id(), cx);
                            this.reload(cx);
                        }
                    }
                    this.tag_dialog = None;
                    this.restore_focus(window, cx);
                    cx.notify();
                })
                .detach();
                self.tag_dialog = Some(dialog);
                cx.notify();
            }
            CardAction::TogglePopover(pop) => {
                let Some(task_id) = self.expanded.as_ref().map(|c| c.task_id) else {
                    return;
                };
                let already = self
                    .expanded
                    .as_ref()
                    .and_then(|c| c.active_popover)
                    == Some(pop);
                if already {
                    self.close_all_popovers();
                    self.restore_focus(window, cx);
                    cx.notify();
                    return;
                }
                match pop {
                    CardPopover::When => self.open_when_picker(PopoverAnchor::Card, task_id, window, cx),
                    CardPopover::Deadline => {
                        self.open_deadline_picker(PopoverAnchor::Card, DeadlineTarget::Task(task_id), cx);
                    }
                    CardPopover::Tags | CardPopover::Move => {
                        self.close_all_popovers();
                        if let Some(card) = &mut self.expanded {
                            card.active_popover = Some(pop);
                            if pop == CardPopover::Tags {
                                card.tag_input.update(cx, |i, cx| i.set_text("", cx));
                                window.focus(&card.tag_input.focus_handle(cx), cx);
                            }
                        }
                        cx.notify();
                    }
                }
            }
            CardAction::ClosePopover => {
                self.close_all_popovers();
                self.restore_focus(window, cx);
                cx.notify();
            }
            CardAction::MoveTo(list_id) => {
                if let Some(task_id) = self.expanded.as_ref().map(|c| c.task_id)
                    && self.store.set_list(task_id, list_id).is_ok()
                {
                    self.mark_expanded_touched(task_id);
                    data_version::data_changed(cx.entity_id(), cx);
                    self.close_all_popovers();
                    self.reload(cx);
                    self.restore_focus(window, cx);
                }
            }
            CardAction::MagicDropAfter(anchor_id) => {
                if self.view == ThingsViewKind::Logbook {
                    return;
                }
                let Some(anchor) = self.tasks.iter().find(|t| t.id == anchor_id).cloned() else {
                    return;
                };
                if anchor.is_closed() {
                    return;
                }
                let today = Self::today();
                let target = match self.view {
                    ThingsViewKind::Inbox => MagicPlusDropTarget {
                        list_id: None,
                        section_id: None,
                        start_date: None,
                        schedule_bucket: ScheduleBucket::None,
                        after_task: Some((anchor_id, true)),
                    },
                    ThingsViewKind::Today => {
                        let start = anchor
                            .start_date
                            .or(anchor.due_date)
                            .map(|d| d.min(today))
                            .unwrap_or(today);
                        MagicPlusDropTarget {
                            list_id: anchor.list_id,
                            section_id: anchor.section_id,
                            start_date: Some(start),
                            schedule_bucket: anchor.schedule_bucket,
                            after_task: Some((anchor_id, true)),
                        }
                    }
                    ThingsViewKind::Upcoming => {
                        let min_upcoming = today + Duration::days(1);
                        let start = anchor
                            .start_date
                            .or(anchor.due_date)
                            .map(|d| d.max(min_upcoming))
                            .unwrap_or(min_upcoming);
                        MagicPlusDropTarget {
                            list_id: anchor.list_id,
                            section_id: anchor.section_id,
                            start_date: Some(start),
                            schedule_bucket: ScheduleBucket::None,
                            after_task: Some((anchor_id, true)),
                        }
                    }
                    ThingsViewKind::Anytime => MagicPlusDropTarget {
                        list_id: anchor.list_id,
                        section_id: anchor.section_id,
                        start_date: if anchor.list_id.is_some() {
                            None
                        } else {
                            Some(
                                anchor
                                    .start_date
                                    .or(anchor.due_date)
                                    .map(|d| d.min(today))
                                    .unwrap_or(today),
                            )
                        },
                        schedule_bucket: ScheduleBucket::None,
                        after_task: Some((anchor_id, true)),
                    },
                    ThingsViewKind::Someday => MagicPlusDropTarget {
                        list_id: anchor.list_id,
                        section_id: anchor.section_id,
                        start_date: None,
                        schedule_bucket: ScheduleBucket::Someday,
                        after_task: Some((anchor_id, true)),
                    },
                    ThingsViewKind::Project(list_id) => MagicPlusDropTarget {
                        list_id: Some(list_id),
                        section_id: anchor.section_id,
                        start_date: None,
                        schedule_bucket: ScheduleBucket::None,
                        after_task: Some((anchor_id, true)),
                    },
                    ThingsViewKind::Area(_) => MagicPlusDropTarget {
                        list_id: anchor.list_id,
                        section_id: anchor.section_id,
                        start_date: None,
                        schedule_bucket: ScheduleBucket::None,
                        after_task: Some((anchor_id, true)),
                    },
                    ThingsViewKind::Logbook => return,
                };
                self.create_todo_at(target, window, cx);
            }
            CardAction::ReorderAfter { dragged_id, target_id } => {
                if self.view == ThingsViewKind::Logbook {
                    return;
                }
                let target_task = self.tasks.iter().find(|t| t.id == target_id).cloned();
                let dragged_task = self.tasks.iter().find(|t| t.id == dragged_id).cloned();
                if let (Some(dragged), Some(target)) = (dragged_task, target_task) {
                    if dragged.is_closed() || target.is_closed() {
                        return;
                    }
                    let today = Self::today();
                    let was_expanded = self.expanded.is_some();
                    let mut mutated = false;
                    match self.view {
                        ThingsViewKind::Inbox | ThingsViewKind::Logbook => {}
                        ThingsViewKind::Project(_) => {
                            if dragged.section_id != target.section_id
                                && self.store.set_section(dragged_id, target.section_id).is_ok()
                            {
                                mutated = true;
                            }
                        }
                        ThingsViewKind::Today => {
                            if dragged.schedule_bucket != target.schedule_bucket {
                                let start = if target.schedule_bucket == ScheduleBucket::Evening {
                                    dragged.start_date.or(Some(today))
                                } else {
                                    dragged.start_date
                                };
                                if self
                                    .store
                                    .set_task_schedule(dragged_id, start, target.schedule_bucket)
                                    .is_ok()
                                {
                                    mutated = true;
                                }
                            }
                        }
                        ThingsViewKind::Upcoming => {
                            let dragged_day = dragged.start_date.or(dragged.due_date);
                            let target_day = target.start_date.or(target.due_date);
                            if let Some(new_start) = target_day
                                && dragged_day != Some(new_start)
                                && self
                                    .store
                                    .set_task_schedule(
                                        dragged_id,
                                        Some(new_start),
                                        ScheduleBucket::None,
                                    )
                                    .is_ok()
                            {
                                mutated = true;
                            }
                        }
                        ThingsViewKind::Anytime => {
                            if dragged.list_id != target.list_id {
                                if self.store.set_list(dragged_id, target.list_id).is_ok() {
                                    mutated = true;
                                }
                                if target.list_id.is_none()
                                    && dragged.start_date.is_none()
                                    && dragged.due_date.is_none()
                                    && self
                                        .store
                                        .set_task_schedule(
                                            dragged_id,
                                            Some(today),
                                            dragged.schedule_bucket,
                                        )
                                        .is_ok()
                                {
                                    mutated = true;
                                }
                            }
                        }
                        ThingsViewKind::Someday | ThingsViewKind::Area(_) => {
                            if dragged.list_id != target.list_id
                                && self.store.set_list(dragged_id, target.list_id).is_ok()
                            {
                                mutated = true;
                            }
                        }
                    }
                    if self.store.move_task(dragged_id, target_id, true).is_ok() {
                        mutated = true;
                    }
                    if mutated {
                        data_version::data_changed(cx.entity_id(), cx);
                        self.reload(cx);
                        if was_expanded && self.expanded.is_none() {
                            window.focus(&self.focus_handle, cx);
                        }
                    }
                }
            }
        }
    }

    pub(super) fn handle_bottom_bar_action(
        &mut self,
        action: BottomBarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            BottomBarAction::NewTodo => self.create_new_todo_default(window, cx),
            BottomBarAction::ToggleWhen => {
                let Some(task_id) = self.active_target_task_id() else {
                    return;
                };
                let already = self
                    .when_picker
                    .as_ref()
                    .is_some_and(|(a, s)| *a == PopoverAnchor::BottomBar && s.task_id == task_id);
                if already {
                    self.close_all_popovers();
                    self.restore_focus(window, cx);
                    cx.notify();
                } else {
                    self.open_when_picker(PopoverAnchor::BottomBar, task_id, window, cx);
                }
            }
            BottomBarAction::ToggleMove => {
                if self.active_target_task_id().is_none() {
                    return;
                }
                let next = !self.bottom_move_open;
                self.close_all_popovers();
                self.bottom_move_open = next;
                if !next {
                    self.restore_focus(window, cx);
                }
                cx.notify();
            }
            BottomBarAction::QuickFind => {
                cx.emit(ThingsEvent::OpenSearch);
            }
            BottomBarAction::MoveTo(list_id) => {
                if let Some(task_id) = self.active_target_task_id()
                    && self.store.set_list(task_id, list_id).is_ok()
                {
                    self.mark_expanded_touched(task_id);
                    data_version::data_changed(cx.entity_id(), cx);
                    self.close_all_popovers();
                    self.reload(cx);
                    self.restore_focus(window, cx);
                }
            }
            BottomBarAction::ClosePopover => {
                self.close_all_popovers();
                self.restore_focus(window, cx);
                cx.notify();
            }
        }
    }

    pub(super) fn apply_group_drop(
        &mut self,
        task_id: Id,
        drop: GroupDrop,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_expanded = self.expanded.is_some();
        let mut mutated = false;
        match drop {
            GroupDrop::Schedule {
                start_date,
                bucket,
                list_id,
            } => {
                if let Some(target_list) = list_id
                    && self.store.set_list(task_id, target_list).is_ok()
                {
                    mutated = true;
                }
                if self
                    .store
                    .set_task_schedule(task_id, start_date, bucket)
                    .is_ok()
                {
                    mutated = true;
                }
            }
            GroupDrop::Project {
                list_id,
                section_id,
            } => {
                if self.store.set_list(task_id, Some(list_id)).is_ok() {
                    mutated = true;
                }
                if self.store.set_section(task_id, section_id).is_ok() {
                    mutated = true;
                }
            }
        }
        if mutated {
            self.mark_expanded_touched(task_id);
            data_version::data_changed(cx.entity_id(), cx);
            self.reload(cx);
            if was_expanded && self.expanded.is_none() {
                window.focus(&self.focus_handle, cx);
            }
        }
    }
}
