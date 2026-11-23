use std::collections::{HashMap, HashSet};

use chrono::Local;
use fern_core::{Id, Kind, Priority, Task};
use gpui::{Entity, FocusHandle, Focusable, WeakFocusHandle};
use ui::prelude::*;

use crate::calendar;
use crate::due_picker::{self, DueDraft};
use crate::lists_panel;
use crate::text_input::TextInput;

use super::{
    EstimateDraft, PendingDelete, Popup, PopupWindow, TagDraft, TaskDetail, TaskUpdated, same_tag,
};

impl TaskDetail {
    /// Whether one of the details' popovers is open, which a click outside the card closes first.
    pub fn has_popup(&self) -> bool {
        self.session.popup.is_some()
    }

    pub fn sticky_popup_return_focus(&self, window: &Window, cx: &App) -> WeakFocusHandle {
        self.editor_window.popup_return_focus.clone().unwrap_or_else(|| {
            window
                .focused(cx)
                .filter(|focus| self.is_editor_focus(focus, cx))
                .unwrap_or_else(|| self.title.focus_handle(cx))
                .downgrade()
        })
    }

    pub fn sticky_popup_inputs(&self, cx: &App) -> (Option<FocusHandle>, [Entity<TextInput>; 2]) {
        let input = match self.session.popup {
            Some(Popup::Tags | Popup::InlineTags) => Some(&self.tag_input),
            Some(Popup::List) => Some(&self.list_search),
            Some(Popup::LinkParent) => Some(&self.link_search),
            _ => None,
        };
        (
            input.map(|input| input.focus_handle(cx)),
            [self.tag_input.clone(), self.time_input.clone()],
        )
    }

    pub fn bind_popup_window(&self, window: &Window, cx: &mut Context<Self>) -> PopupWindow {
        PopupWindow::new(cx.weak_entity(), &self.tag_input, &self.time_input, window, cx)
    }

    /// The same menus and drafts, hosted outside the small native note window.
    pub fn render_sticky_popup(&mut self, available: gpui::Size<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        let strings = crate::i18n::t(cx);
        match self.session.popup {
            Some(Popup::Due) => self.render_due_popup_in(available, strings, cx),
            Some(Popup::More) => self.render_more_popup_in(Some(available.height), strings, cx),
            Some(Popup::Priority) => self.render_priority_popup(strings, cx),
            Some(Popup::List) => self.render_list_popup(strings, cx),
            Some(Popup::Tags | Popup::InlineTags) => self.render_tags_popup(cx),
            Some(Popup::Templates) => self.render_draft_templates(cx),
            Some(Popup::Estimate) => self.render_estimate(strings, cx),
            Some(Popup::LinkParent) => self.render_link_parent(strings, cx),
            None => div().into_any_element(),
        }
    }

    /// Reserve the side-menu area before it opens. Resizing a native popup on hover
    /// replaces its render surface and causes a visible flash, even for a single frame.
    pub fn sticky_submenu_space(&self) -> Pixels {
        match self.session.popup {
            Some(Popup::More) => px(200.),
            Some(Popup::List) => px(180.),
            _ => px(0.),
        }
    }

    fn link_parent_candidates(&self) -> Vec<Task> {
        let Some(task) = self.session.task.as_ref() else {
            return Vec::new();
        };
        let all_tasks = self.session.store.tasks().unwrap_or_default();
        let tree = fern_core::TaskTree::build(&all_tasks);
        let task_depth = tree.depth(task.id);
        let descendants = tree.descendants(task.id);
        let subtree_height = descendants
            .iter()
            .map(|desc| tree.depth(desc.id).saturating_sub(task_depth))
            .max()
            .unwrap_or(0);
        let excluded: HashSet<Id> = descendants
            .iter()
            .map(|desc| desc.id)
            .chain([task.id])
            .collect();
        let depth_by_id: HashMap<Id, usize> = all_tasks
            .iter()
            .map(|t| (t.id, tree.depth(t.id)))
            .collect();
        drop(descendants);
        drop(tree);
        all_tasks
            .into_iter()
            .filter(|t| {
                let cand_depth = depth_by_id.get(&t.id).copied().unwrap_or(0);
                !t.is_closed()
                    && !t.is_deleted()
                    && t.kind != Kind::Note
                    && !excluded.contains(&t.id)
                    && task.parent_id != Some(t.id)
                    && cand_depth + 1 + subtree_height <= fern_core::MAX_SUBTASK_DEPTH
            })
            .collect()
    }

    pub(super) fn open_popup(&mut self, popup: Popup, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.focus_submenu = false;
        self.editor_window.popup_return_focus = None;
        self.session.tag_draft = None;
        match popup {
            Popup::Due => {
                self.session.preferences = self.session.store.preferences().unwrap_or_default();
                let today = Local::now().date_naive();
                self.tz_input.update(cx, |input, cx| input.set_text("", cx));
                self.session.due_draft = self.session.task.as_ref().map(|task| self.due_preview(task, today));
            }
            Popup::Tags | Popup::InlineTags => {
                self.session.all_tags = self.session.store.tags().unwrap_or_default();
                let original = self.task_tags();
                self.session.tag_draft = Some(TagDraft {
                    picked: original.clone(),
                    original,
                    save_failed: false,
                });
                self.tag_input.update(cx, |input, cx| input.set_text("", cx));
            }
            Popup::List => {
                self.list_search.update(cx, |input, cx| input.set_text("", cx));
                self.list_submenu = None;
                self.session.list_sections = self.session.store.all_list_sections().unwrap_or_default();
            }
            Popup::Templates => {
                self.session.draft_templates = self.session.store.templates().unwrap_or_default();
            }
            Popup::Priority | Popup::More => {}
            Popup::LinkParent => {
                self.link_search.update(cx, |input, cx| input.set_text("", cx));
                self.session.link_parent_candidates = self.link_parent_candidates();
            }
            Popup::Estimate => {
                self.session.estimate_draft =
                    EstimateDraft::from(self.session.task.as_ref().and_then(|t| t.estimate));
            }
        }
        self.session.popup = Some(popup);
        cx.notify();
    }

    pub(super) fn open_popup_from_ui(&mut self, popup: Popup, window: &Window, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        let return_focus = if self.popup_input_focused(window, cx) {
            self.editor_window.popup_return_focus.clone()
        } else {
            window.focused(cx).map(|focus| focus.downgrade())
        };
        self.open_popup(popup, cx);
        self.editor_window.popup_return_focus = return_focus;
    }

    pub(super) fn open_tags(&mut self, popup: Popup, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.open_popup_from_ui(popup, window, cx);
        window.focus(&self.tag_input.focus_handle(cx), cx);
    }

    /// Settings → Task Default → Default Duration, in minutes.
    pub(super) fn default_duration(&self) -> i64 {
        self.session.preferences.task_defaults.duration
    }

    /// The details' own pickers, for the task list's right-click menu: `subtask` focuses the Add
    /// Subtask field, `due`, `tags` and `link-parent` open their popovers.
    pub fn start(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.task.is_none() || self.is_read_only() {
            return;
        }
        match what {
            "subtask" if self.can_add_subtask() => {
                self.subtask_open = true;
                window.focus(&self.subtask_input.focus_handle(cx), cx);
            }
            "due" => self.open_popup_from_ui(Popup::Due, window, cx),
            "tags" => self.open_tags(Popup::Tags, window, cx),
            "link-parent" if self.can_link_parent() => {
                self.open_popup_from_ui(Popup::LinkParent, window, cx);
                window.focus(&self.link_search.focus_handle(cx), cx);
            }
            _ => {}
        }
    }

    /// Opens the due-date picker (the "Set date" shortcut).
    pub fn open_due(&mut self, cx: &mut Context<Self>) {
        if self.session.task.is_some() && !self.is_read_only() {
            self.open_popup(Popup::Due, cx);
        }
    }

    #[cfg(debug_assertions)]
    pub fn dev_open_popup(&mut self, popup: &str, cx: &mut Context<Self>) {
        use fern_core::{Frequency, Recurrence};
        use crate::due_picker::{CustomReminder, DueMenu};

        let popup = match popup {
            "due" => Popup::Due,
            "priority" => Popup::Priority,
            "tags" => Popup::Tags,
            "list" => Popup::List,
            // The list menu with the task's list's sections open beside it.
            "list-sections" => {
                self.open_popup(Popup::List, cx);
                self.list_submenu = Some(self.session.task.as_ref().and_then(|t| t.list_id));
                return;
            }
            "more" => Popup::More,
            // The "…" menu with its Start Focus side menu open.
            "more-focus" => {
                self.open_popup(Popup::More, cx);
                self.focus_submenu = true;
                return;
            }
            "link-parent" => Popup::LinkParent,
            "estimate" => Popup::Estimate,
            "checklist" => {
                self.start_checklist(cx);
                return;
            }
            // The due-date popover with its time menu or the custom repeat editor open.
            "due-reminder" | "due-reminder-custom" => {
                self.open_popup(Popup::Due, cx);
                let custom = popup == "due-reminder-custom";
                self.update_draft(cx, |d| {
                    d.menu = Some(if custom {
                        DueMenu::CustomReminder(CustomReminder::new(d.time.is_some()))
                    } else {
                        DueMenu::Reminder
                    })
                });
                return;
            }
            "activity" => {
                self.open_activity(cx);
                return;
            }
            "due-duration" => {
                self.open_popup(Popup::Due, cx);
                let duration = self.default_duration();
                self.update_draft(cx, |d| d.start_duration(Local::now().naive_local(), duration));
                return;
            }
            // The custom repeat's monthly "Each" with the 1st, 15th and last day picked.
            "due-custom-monthly" => {
                self.open_popup(Popup::Due, cx);
                self.update_draft(cx, |d| {
                    let mut rule = Recurrence::new(Frequency::Monthly, 1);
                    rule.month_days = 1 | 1 << 14 | fern_core::recurrence::LAST_DAY;
                    d.menu = Some(DueMenu::Custom(rule));
                });
                return;
            }
            // The custom repeat's monthly "Workday", daily "Skip Weekends" and "By Completion Date".
            "due-custom-workday" | "due-custom-skip" | "due-custom-completion" | "due-custom-specific" => {
                self.open_popup(Popup::Due, cx);
                self.update_draft(cx, |d| {
                    let rule = match popup {
                        "due-custom-workday" => Recurrence {
                            nth_workday: Some(1),
                            ..Recurrence::new(Frequency::Monthly, 1)
                        },
                        "due-custom-skip" => Recurrence {
                            skip_weekends: true,
                            ..Recurrence::new(Frequency::Daily, 1)
                        },
                        "due-custom-specific" => {
                            let today = Local::now().date_naive();
                            Recurrence {
                                dates: vec![today, today + chrono::Duration::days(13)],
                                ..Recurrence::new(Frequency::Daily, 1)
                            }
                        }
                        _ => Recurrence {
                            from_completion: true,
                            ..Recurrence::new(Frequency::Weekly, 1)
                        },
                    };
                    d.menu = Some(DueMenu::Custom(rule));
                });
                return;
            }
            "due-time" | "due-custom" => {
                self.open_popup(Popup::Due, cx);
                let today = Local::now().date_naive();
                let custom = popup == "due-custom";
                self.update_draft(cx, |d| {
                    d.menu = Some(if custom {
                        DueMenu::Custom(due_picker::custom_start(d.repeat.clone(), d.date.unwrap_or(today)))
                    } else {
                        DueMenu::Time
                    })
                });
                return;
            }
            _ => return,
        };
        self.open_popup(popup, cx);
    }

    pub(super) fn close_popup(&mut self, cx: &mut Context<Self>) {
        self.session.popup = None;
        self.editor_window.popup_return_focus = None;
        self.session.tag_draft = None;
        self.focus_submenu = false;
        cx.notify();
    }

    fn popup_input_focused(&self, window: &Window, cx: &App) -> bool {
        match self.session.popup {
            Some(Popup::Tags | Popup::InlineTags) => self.tag_input.focus_handle(cx).is_focused(window),
            Some(Popup::LinkParent) => self.link_search.focus_handle(cx).is_focused(window),
            Some(Popup::Due) => {
                self.time_input.focus_handle(cx).is_focused(window)
                    || self.tz_input.focus_handle(cx).is_focused(window)
            }
            Some(Popup::List) => self.list_search.focus_handle(cx).is_focused(window),
            _ => false,
        }
    }

    pub(super) fn clear_pending_delete(&mut self) {
        self.pending_delete = None;
        self.confirm_return_focus = None;
    }

    pub(super) fn request_delete(
        &mut self,
        target: PendingDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_delete = Some(target);
        self.confirm_return_focus = window.focused(cx).map(|handle| handle.downgrade());
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn dismiss_pending_delete(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pending_delete.take().is_none() {
            return false;
        }
        let return_focus = self
            .confirm_return_focus
            .take()
            .and_then(|focus| focus.upgrade());
        if self.focus.is_focused(window) {
            let focus = return_focus.unwrap_or_else(|| self.title.focus_handle(cx));
            window.focus(&focus, cx);
        }
        cx.notify();
        true
    }

    pub(super) fn confirm_pending_delete(
        &mut self,
        pending: PendingDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_pending_delete(window, cx);
        if self.is_read_only() {
            return;
        }
        match pending {
            PendingDelete::Attachment(att) => {
                if let Some(dir) = &self.session.attachments_dir {
                    match self.session.store.delete_attachment(&att, dir) {
                        Ok(()) => cx.emit(TaskUpdated),
                        Err(err) => {
                            eprintln!("failed to remove the attachment: {err:#}");
                        }
                    }
                    self.sync_attachments();
                    cx.notify();
                }
            }
            PendingDelete::Comment { id, .. } => {
                match self.session.store.delete_comment(id) {
                    Ok(()) => cx.emit(TaskUpdated),
                    Err(err) => {
                        eprintln!("failed to delete the comment: {err:#}");
                    }
                }
                self.sync_comments();
                cx.notify();
            }
        }
    }

    /// Dismisses a popover even if focus is still in the task list or calendar.
    pub fn dismiss_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.dismiss_pending_delete(window, cx) {
            return true;
        }
        if self.session.popup.is_none() {
            return false;
        }
        if self.popup_input_focused(window, cx) {
            let focus = self
                .editor_window
                .popup_return_focus
                .take()
                .and_then(|focus| focus.upgrade())
                .unwrap_or_else(|| self.title.focus_handle(cx));
            window.focus(&focus, cx);
        }
        self.close_popup(cx);
        true
    }

    pub(super) fn update_draft(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut DueDraft)) {
        if let Some(draft) = self.session.due_draft.as_mut() {
            let was_timed = draft.time.is_some();
            let had_date = draft.date.is_some();
            change(draft);
            let timed = draft.time.is_some();
            if !draft.reminders_changed
                && let Some(task_draft) = &self.session.draft
                && (was_timed != timed || had_date != draft.date.is_some())
            {
                draft.reminders = task_draft
                    .default_reminder(timed)
                    .filter(|_| draft.date.is_some())
                    .into_iter()
                    .collect();
            }
            cx.notify();
        }
    }

    /// "OK": saves the draft's date, time, timezone, reminders, and repeat.
    pub(super) fn apply_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(draft) = self.session.due_draft.take() {
            let today = Local::now().date_naive();
            let changed = self.session.task.as_ref().is_some_and(|task| {
                let before = self.due_preview(task, today);
                draft.date != before.date
                    || draft.time != before.time
                    || draft.end() != before.end()
                    || draft.repeat != before.repeat
                    || draft.timezone != before.timezone
                    || draft.is_floating != before.is_floating
                    || draft.constant_reminder != before.constant_reminder
                    || draft.horizon != before.horizon
            });
            let reminders: Vec<i64> = if draft.date.is_some() {
                draft.reminders.clone()
            } else {
                Vec::new()
            };
            let constant = draft.constant_reminder && !reminders.is_empty();
            let target_horizon = if draft.date.is_some() { None } else { draft.horizon };
            if !self.edit_draft(cx, |task, state| {
                if changed {
                    task.due_date = draft.date;
                    task.due_time = draft.date.and(draft.time);
                    task.end_date = draft.end().map(|(date, _)| date);
                    task.end_time = draft.end().and_then(|(_, time)| time);
                    task.repeat = draft.repeat.clone();
                    task.timezone = draft.timezone.clone();
                    task.is_floating = draft.is_floating;
                    task.horizon = target_horizon;
                    state.date_changed = true;
                }
                task.reminders = reminders.clone();
                task.constant_reminder = constant;
                state.reminders_changed = draft.reminders_changed;
            }) {
                self.save(cx, |store, id| {
                    if draft.date.is_some() {
                        store.set_due(id, draft.date, draft.time)?;
                        store.set_due_end(id, draft.end())?;
                        store.set_timezone(id, draft.timezone.as_deref(), draft.is_floating)?;
                        store.set_reminders(id, &reminders)?;
                        store.set_task_constant_reminder(id, constant)?;
                        store.set_repeat(id, draft.repeat, today)
                    } else {
                        store.set_due(id, None, None)?;
                        store.set_due_end(id, None)?;
                        store.set_timezone(id, None, false)?;
                        store.set_reminders(id, &[])?;
                        store.set_task_constant_reminder(id, false)?;
                        store.set_horizon(id, target_horizon)?;
                        store.set_repeat(id, draft.repeat, today)
                    }
                });
            } else {
                self.refresh_draft_title(cx);
            }
        }
        self.dismiss_popup(window, cx);
    }

    /// "Clear": removes the due date, time, timezone, reminders, horizon, and repeat.
    pub(super) fn clear_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.due_draft = None;
        let today = Local::now().date_naive();
        if !self.edit_draft(cx, |task, state| {
            task.due_date = None;
            task.due_time = None;
            task.end_date = None;
            task.end_time = None;
            task.timezone = None;
            task.is_floating = false;
            task.constant_reminder = false;
            task.horizon = None;
            task.reminders.clear();
            task.repeat = None;
            state.date_changed = true;
            state.reminders_changed = false;
        }) {
            self.save(cx, |store, id| {
                store.set_due(id, None, None)?;
                store.set_due_end(id, None)?;
                store.set_timezone(id, None, false)?;
                store.set_reminders(id, &[])?;
                store.set_task_constant_reminder(id, false)?;
                store.set_horizon(id, None)?;
                store.set_repeat(id, None, today)
            });
        } else {
            self.refresh_draft_title(cx);
        }
        self.dismiss_popup(window, cx);
    }

    pub(super) fn set_priority(&mut self, priority: Priority, cx: &mut Context<Self>) {
        if !self.edit_draft(cx, |task, draft| {
            task.priority = priority;
            draft.priority_changed = true;
        }) {
            self.save(cx, |store, id| store.set_priority(id, priority));
        }
        self.close_popup(cx);
    }

    pub(super) fn set_list(&mut self, list: Option<Id>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.edit_draft(cx, |task, draft| {
            task.list_id = list;
            task.section_id = None;
            draft.list_changed = true;
        }) {
            self.save(cx, |store, id| store.set_list(id, list));
        }
        self.dismiss_popup(window, cx);
    }

    pub(super) fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.update_draft(cx, |d| d.month = calendar::add_months(d.month, delta));
    }

    pub(super) fn task_tags(&self) -> Vec<String> {
        self.session.task.as_ref().map(|t| t.tags.clone()).unwrap_or_default()
    }

    pub(super) fn tag_query<'a>(&self, cx: &'a App) -> &'a str {
        self.tag_input.read(cx).text().trim().trim_start_matches('#').trim()
    }

    pub fn pick_typed_time(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(time) = due_picker::parse_time(text) {
            self.update_draft(cx, |d| d.pick_time(Some(time), Local::now().date_naive()));
            self.time_input.update(cx, |input, cx| input.clear(window, cx));
        }
    }

    pub fn pick_typed_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.tag_query(cx).to_owned();
        if query.is_empty() {
            return;
        }
        let name = self
            .session
            .all_tags
            .iter()
            .find(|tag| same_tag(&tag.name, &query))
            .map_or(query, |tag| tag.name.clone());
        if let Some(draft) = &mut self.session.tag_draft {
            if !draft.picked.iter().any(|picked| same_tag(picked, &name)) {
                draft.picked.push(name);
            }
            draft.save_failed = false;
            self.tag_input.update(cx, |input, cx| input.clear(window, cx));
            cx.notify();
        }
    }

    pub(super) fn toggle_picked_tag(&mut self, name: String, cx: &mut Context<Self>) {
        if let Some(draft) = &mut self.session.tag_draft {
            if let Some(index) = draft.picked.iter().position(|picked| same_tag(picked, &name)) {
                draft.picked.remove(index);
            } else {
                draft.picked.push(name);
            }
            draft.save_failed = false;
            cx.notify();
        }
    }

    pub(super) fn apply_tags(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.draft.is_some() {
            if let Some(tags) = self.session.tag_draft.as_ref().map(|draft| draft.picked.clone()) {
                self.edit_draft(cx, |task, _| task.tags = tags);
            }
            self.dismiss_popup(window, cx);
            return;
        }
        let (Some(id), Some(draft)) = (self.task_id(), &mut self.session.tag_draft) else { return };
        let contains = |tags: &[String], name: &str| tags.iter().any(|tag| same_tag(tag, name));
        let save = (|| -> anyhow::Result<bool> {
            let Some(mut task) = self.session.store.task(id)?.filter(|task| !task.is_deleted()) else {
                anyhow::bail!("cannot save tags for a missing or deleted task");
            };
            // Apply the user's changes to the current tags, which may have synced since opening.
            let before = task.tags.clone();
            task.tags.retain(|tag| !contains(&draft.original, tag) || contains(&draft.picked, tag));
            for tag in &draft.picked {
                if !contains(&draft.original, tag) && !contains(&task.tags, tag) {
                    task.tags.push(tag.clone());
                }
            }
            if task.tags == before {
                return Ok(false);
            }
            self.session.store.set_tags(id, &task.tags)?;
            Ok(true)
        })();
        match save {
            Ok(changed) => {
                self.dismiss_popup(window, cx);
                if changed {
                    cx.emit(TaskUpdated);
                }
            }
            Err(error) => {
                eprintln!("failed to save task tags: {error:#}");
                draft.save_failed = true;
                cx.notify();
            }
        }
    }

    pub(super) fn tag_color(&self, name: &str, cx: &App) -> Hsla {
        let color = self
            .session
            .all_tags
            .iter()
            .find(|t| t.name == name)
            .and_then(|t| t.color.as_deref());
        lists_panel::color(color, cx)
    }
}
