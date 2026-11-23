//! The task details panel on the right, modeled on TickTick's.

use std::path::PathBuf;
use std::rc::Rc;

use fern_core::attachment::PreparedAttachment;
#[cfg(test)]
use fern_core::{Frequency, Recurrence};
use fern_core::{ChecklistItem, Id, Kind, List, Priority, Store, Tag, Task};
use chrono::{Local, NaiveDate};
use gpui::{Entity, EventEmitter, FocusHandle, Focusable, Subscription, WeakFocusHandle};
use ui::prelude::*;
use ui::popover;

use crate::calendar;
use crate::due_picker::DueDraft;
#[cfg(all(test, debug_assertions))]
use crate::due_picker::DueMenu;
#[cfg(test)]
use crate::due_picker::DueTab;
use crate::i18n::{self, CurrentLocale, Strings};
use crate::markdown_toolbar::ToolbarAction;
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};

mod comments_attachments;
mod draft;
mod due_popup;
mod popup_actions;
mod popups;
mod render_body;
mod render_chrome;
mod session;
mod subtasks_checklist;
mod window;
pub use draft::DraftClosed;
pub use render_body::{render_markdown, render_release_markdown};
use session::TaskDetailSession;
use window::EditorWindow;
pub use window::PopupWindow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Popup {
    Due,
    Priority,
    Tags,
    InlineTags,
    List,
    More,
    Templates,
    /// "Link Parent Task": a searchable list of tasks to nest this one under.
    LinkParent,
    /// Start Focus → "Estimation": the expected pomos or duration.
    Estimate,
}

struct TagDraft {
    original: Vec<String>,
    picked: Vec<String>,
    save_failed: bool,
}

/// The Estimation popover's draft: minutes instead of pomos, and the amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EstimateDraft {
    minutes: bool,
    amount: u32,
}

impl EstimateDraft {
    fn from(estimate: Option<fern_core::Estimate>) -> Self {
        match estimate {
            Some(fern_core::Estimate::Minutes(n)) => Self { minutes: true, amount: n },
            Some(fern_core::Estimate::Pomos(n)) => Self { minutes: false, amount: n },
            None => Self { minutes: false, amount: 0 },
        }
    }

    /// One step of the stepper: a pomo, or a quarter of an hour.
    fn step(&mut self, up: bool) {
        let step = if self.minutes { 15 } else { 1 };
        self.amount = if up { self.amount + step } else { self.amount.saturating_sub(step) };
    }

    fn estimate(self) -> Option<fern_core::Estimate> {
        match (self.amount, self.minutes) {
            (0, _) => None,
            (n, true) => Some(fern_core::Estimate::Minutes(n)),
            (n, false) => Some(fern_core::Estimate::Pomos(n)),
        }
    }
}

pub struct TaskDetail {
    session: TaskDetailSession,
    editor_window: EditorWindow,
    /// The task's comments, the bottom bar's comment field (while open) and its emoji menu.
    comment_input: Entity<TextInput>,
    comment_open: bool,
    comment_emoji: bool,
    /// The bottom bar's "A": the Markdown toolbar for the description, and its headings menu.
    format_bar: bool,
    format_headings: bool,
    copied_feedback: bool,
    copied_reset: Option<gpui::Task<()>>,
    word_count: usize,
    /// The "…" menu's Start Focus side menu, while hovered open.
    focus_submenu: bool,
    title: Entity<TextInput>,
    notes: Entity<TextInput>,
    tag_input: Entity<TextInput>,
    /// Checklist items with an input each, in display order.
    checklist: Vec<ChecklistRow>,
    new_item: Entity<TextInput>,
    subtask_input: Entity<TextInput>,
    /// The due-date popover's field for typing a time.
    time_input: Entity<TextInput>,
    /// The due-date popover's time-zone search input.
    tz_input: Entity<TextInput>,
    /// The task's location field and whether it is expanded when empty.
    location_input: Entity<TextInput>,
    location_open: bool,
    /// The "Link Parent Task" search field.
    link_search: Entity<TextInput>,
    /// The list menu's search field, and the list (`Some(None)` for the Inbox) whose sections
    /// show in the side menu.
    list_search: Entity<TextInput>,
    list_submenu: Option<Option<Id>>,
    /// The checklist is shown when it has items or the user asked to start one.
    show_checklist: bool,
    /// Shown as a task card (the calendar's or the board's), framed by the card instead of a
    /// left border.
    card: bool,
    /// Leaves top clearance for the overlaid window buttons when docked at the window's right edge.
    top_inset: bool,
    sticky_style: Option<crate::sticky_settings::StickySettings>,
    sticky_surface: Option<(Hsla, Hsla)>,
    /// The calendar's card writes its date out with the weekday.
    calendar_card: bool,
    /// The Add Subtask field shown while the task has no subtasks, from the "…" menu.
    subtask_open: bool,
    body_scroll: gpui::ScrollHandle,
    focus: FocusHandle,
    pending_delete: Option<PendingDelete>,
    confirm_return_focus: Option<WeakFocusHandle>,
    pending_complete: Option<Id>,
    _session_subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
pub(super) enum PendingDelete {
    Attachment(fern_core::Attachment),
    Comment { id: Id, preview: String },
}

struct ChecklistRow {
    pub(super) item: ChecklistItem,
    pub(super) input: Entity<TextInput>,
    pub(super) _subscriptions: [Subscription; 2],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FocusTarget {
    Item(Id),
    NewItem,
}

/// The task was changed in the panel; the list needs to reload.
pub struct TaskUpdated;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskTextField {
    Title(String),
    Notes(String),
    Location(Option<String>),
}

/// A text field of the shown task changed in the panel; hosts can patch in-place without a full reload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskTextEdited {
    pub id: Id,
    pub field: TaskTextField,
}

/// The user asked to open another task (a subtask) in the panel.
pub struct OpenTask(pub Id);

/// The panel completed or deleted a task; the list offers to undo it.
pub struct Undoable(pub crate::undo::UndoAction, pub String);

/// The user asked to focus on the task ("Start Focus").
pub struct StartFocus(pub Id, pub String, pub bool);

/// The user clicked "Delete forever" on a trashed task; the host asks for confirmation first.
pub struct RequestDeleteForever(pub Id);

impl EventEmitter<TaskUpdated> for TaskDetail {}
impl EventEmitter<TaskTextEdited> for TaskDetail {}
impl EventEmitter<StartFocus> for TaskDetail {}
impl EventEmitter<Undoable> for TaskDetail {}
impl EventEmitter<OpenTask> for TaskDetail {}
impl EventEmitter<RequestDeleteForever> for TaskDetail {}

impl TaskDetail {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let title = cx.new(|cx| TextInput::new(Mode::Wrapped, strings.untitled, cx).without_cancel());
        let notes = cx.new(|cx| TextInput::new(Mode::MultiLine, strings.notes_placeholder, cx).without_cancel());
        let tag_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.type_in_tag, cx).without_cancel());
        let new_item = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_item, cx).without_cancel());
        let subtask_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_subtask, cx).without_cancel());
        let time_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.time_placeholder, cx).without_cancel());
        let tz_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.timezone_search, cx).without_cancel());
        let location_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.location_placeholder, cx).without_cancel());
        let link_search = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.search_placeholder, cx).without_cancel());
        let list_search = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.section_search, cx).without_cancel());
        let comment_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.write_comment, cx));

        let subscriptions = vec![
            cx.subscribe(&title, |this, input, _: &Changed, cx| {
                if this.is_read_only() {
                    return;
                }
                let text = input.read(cx).text().to_owned();
                if this.edit_draft(cx, |task, _| task.title = text.clone()) {
                    this.refresh_draft_title(cx);
                    return;
                }
                this.save_text(cx, TaskTextField::Title(text.clone()), |store, id| store.set_title(id, &text));
            }),
            cx.subscribe(&notes, |this, input, _: &Changed, cx| {
                if this.is_read_only() {
                    return;
                }
                let text = input.read(cx).text().to_owned();
                this.word_count = fern_core::markdown::count_words(&text);
                if this.edit_draft(cx, |task, _| task.notes = text.clone()) { return; }
                this.save_text(cx, TaskTextField::Notes(text.clone()), |store, id| store.set_notes(id, &text));
            }),
            cx.subscribe(&location_input, |this, input, _: &Changed, cx| {
                if this.is_read_only() {
                    return;
                }
                let text = input.read(cx).text().trim().to_owned();
                let value = (!text.is_empty()).then_some(text);
                if this.edit_draft(cx, |task, _| task.location = value.clone()) {
                    return;
                }
                this.save_text(cx, TaskTextField::Location(value.clone()), |store, id| {
                    store.set_location(id, value.as_deref())
                });
            }),
            cx.subscribe(&tag_input, |_, _, _: &Changed, cx| cx.notify()),
            cx.subscribe(&tz_input, |_, _, _: &Changed, cx| cx.notify()),
            cx.subscribe(&link_search, |_, _, _: &Changed, cx| cx.notify()),
            cx.subscribe(&list_search, |this, _, _: &Changed, cx| {
                this.list_submenu = None;
                cx.notify();
            }),
            cx.observe_global::<CurrentLocale>(|this, cx| {
                let strings = i18n::t(cx);
                let placeholder = if this.session.draft.is_some() { strings.new_task_prompt } else { strings.untitled };
                this.title.update(cx, |input, cx| input.set_placeholder(placeholder, cx));
                this.notes.update(cx, |input, cx| input.set_placeholder(strings.notes_placeholder, cx));
                this.tag_input.update(cx, |input, cx| input.set_placeholder(strings.type_in_tag, cx));
                this.new_item.update(cx, |input, cx| input.set_placeholder(strings.add_item, cx));
                this.subtask_input.update(cx, |input, cx| input.set_placeholder(strings.add_subtask, cx));
                this.time_input.update(cx, |input, cx| input.set_placeholder(strings.time_placeholder, cx));
                this.tz_input.update(cx, |input, cx| input.set_placeholder(strings.timezone_search, cx));
                this.location_input.update(cx, |input, cx| input.set_placeholder(strings.location_placeholder, cx));
                this.link_search.update(cx, |input, cx| input.set_placeholder(strings.search_placeholder, cx));
                this.list_search.update(cx, |input, cx| input.set_placeholder(strings.section_search, cx));
                this.comment_input.update(cx, |input, cx| input.set_placeholder(strings.write_comment, cx));
            }),
        ];

        let mut editor_window = EditorWindow::new(&title, &new_item, &subtask_input, &comment_input, window, cx);
        editor_window.inline_popup = Some(PopupWindow::new(cx.weak_entity(), &tag_input, &time_input, window, cx));
        Self {
            session: TaskDetailSession::new(store),
            editor_window,
            focus_submenu: false,
            comment_input,
            comment_open: false,
            comment_emoji: false,
            format_bar: false,
            format_headings: false,
            copied_feedback: false,
            copied_reset: None,
            word_count: 0,
            title,
            notes,
            tag_input,
            checklist: Vec::new(),
            new_item,
            subtask_input,
            time_input,
            tz_input,
            location_input,
            location_open: false,
            link_search,
            list_search,
            list_submenu: None,
            show_checklist: false,
            card: false,
            top_inset: false,
            sticky_style: None,
            sticky_surface: None,
            calendar_card: false,
            subtask_open: false,
            body_scroll: gpui::ScrollHandle::new(),
            focus: cx.focus_handle(),
            pending_delete: None,
            confirm_return_focus: None,
            pending_complete: None,
            _session_subscriptions: subscriptions,
        }
    }

    /// Makes this a task card, which draws its own frame; `calendar` for the calendar's.
    pub fn set_card(&mut self, calendar: bool) {
        self.card = true;
        self.calendar_card = calendar;
    }

    /// Sets whether the header leaves top clearance for the overlaid window buttons.
    pub fn set_top_inset(&mut self, inset: bool, cx: &mut Context<Self>) {
        if self.top_inset != inset {
            self.top_inset = inset;
            cx.notify();
        }
    }

    pub fn is_editor_focus(&self, focus: &FocusHandle, cx: &App) -> bool {
        [
            &self.title,
            &self.notes,
            &self.comment_input,
            &self.subtask_input,
            &self.new_item,
            &self.location_input,
        ]
        .into_iter()
        .any(|input| input.focus_handle(cx) == *focus)
            || self.checklist.iter().any(|row| row.input.focus_handle(cx) == *focus)
    }

    pub fn task_id(&self) -> Option<Id> {
        self.session.task.as_ref().map(|t| t.id)
    }

    pub fn task_title(&self) -> Option<&str> {
        self.session.task.as_ref().map(|task| task.title.as_str())
    }

    pub fn set_lists(&mut self, lists: Vec<List>) {
        self.session.lists = lists;
    }

    /// Whether the displayed task is read-only (in the trash or in an archived list), matching the
    /// original's `isReadOnly` behavior.
    pub fn is_read_only(&self) -> bool {
        self.session.draft.is_none()
            && self
                .session
                .task
                .as_ref()
                .is_some_and(|task| task.is_deleted() || self.session.task_in_archived_list)
    }

    fn sync_read_only(&mut self, cx: &mut Context<Self>) {
        let (archived, archived_name) = if self.session.draft.is_some() {
            (false, None)
        } else if let Some(list_id) = self.session.task.as_ref().and_then(|t| t.list_id) {
            match self.session.store.list_name_and_archived(list_id).ok().flatten() {
                Some((name, true)) => (true, Some(name)),
                _ => (false, None),
            }
        } else {
            (false, None)
        };
        self.session.task_in_archived_list = archived;
        self.session.archived_list_name = archived_name;
        let read_only = self.is_read_only();
        if read_only {
            self.format_bar = false;
            self.format_headings = false;
            self.comment_open = false;
            self.comment_emoji = false;
            self.subtask_open = false;
            self.session.popup = None;
            self.session.tag_draft = None;
            self.clear_pending_delete();
        }
        self.sync_read_only_inputs(cx);
    }

    fn sync_read_only_inputs(&self, cx: &mut Context<Self>) {
        let read_only = self.is_read_only();
        self.title.update(cx, |input, cx| input.set_read_only(read_only, cx));
        self.notes.update(cx, |input, cx| input.set_read_only(read_only, cx));
        self.location_input.update(cx, |input, cx| input.set_read_only(read_only, cx));
        for row in &self.checklist {
            row.input.update(cx, |input, cx| input.set_read_only(read_only, cx));
        }
    }

    /// Shows a task, or the empty panel for `None`.
    pub fn show(&mut self, task: Option<Task>, cx: &mut Context<Self>) {
        let task_changed = self.session.task.as_ref().map(|t| t.id) != task.as_ref().map(|t| t.id);
        if task_changed {
            self.body_scroll.set_offset(gpui::point(px(0.), px(0.)));
        }
        self.session.draft = None;
        self.title.update(cx, |input, cx| input.set_placeholder(i18n::t(cx).untitled, cx));
        let (title, notes, location) = task
            .as_ref()
            .map_or((String::new(), String::new(), String::new()), |t| {
                (t.title.clone(), t.notes.clone(), t.location.clone().unwrap_or_default())
            });
        self.title.update(cx, |input, cx| input.set_text(title, cx));
        self.word_count = fern_core::markdown::count_words(&notes);
        self.notes.update(cx, |input, cx| input.set_text(notes, cx));
        self.location_open = !location.is_empty();
        self.location_input.update(cx, |input, cx| input.set_text(location, cx));
        self.new_item.update(cx, |input, cx| input.set_text("", cx));
        self.subtask_input.update(cx, |input, cx| input.set_text("", cx));
        self.comment_input.update(cx, |input, cx| input.set_text("", cx));
        self.comment_open = false;
        self.comment_emoji = false;
        self.format_headings = false;
        self.focus_submenu = false;
        self.copied_feedback = false;
        self.copied_reset = None;
        self.session.task = task;
        self.session.popup = None;
        self.clear_pending_delete();
        self.editor_window.popup_return_focus = None;
        self.session.tag_draft = None;
        self.session.attachment_errors.clear();
        self.subtask_open = false;
        self.session.activity = None;
        self.session.preferences = self.session.store.preferences().unwrap_or_default();
        self.session.all_tags = self.session.store.tags().unwrap_or_default();
        self.checklist.clear();
        self.sync_read_only(cx);
        self.sync_checklist(cx);
        self.show_checklist = !self.checklist.is_empty();
        self.sync_subtasks();
        self.sync_attachments();
        self.sync_comments();
        self.sync_sticky_inputs(cx);
        cx.notify();
    }

    pub(super) fn sync_comments(&mut self) {
        self.session.comments = match self.task_id() {
            Some(id) => self.session.store.comments(id).unwrap_or_default(),
            None => Vec::new(),
        };
        if let Some(PendingDelete::Comment { id, .. }) = &self.pending_delete
            && !self.session.comments.iter().any(|comment| comment.id == *id)
        {
            self.clear_pending_delete();
        }
    }

    /// The description toolbar's buttons: format the notes' selection as Markdown (or type the
    /// time), open the headings menu, or upload an attachment.
    pub(super) fn toolbar_action(&mut self, action: ToolbarAction, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        match action {
            ToolbarAction::ToggleHeadings => self.format_headings = !self.format_headings,
            ToolbarAction::Attach => return self.upload_attachment(cx),
            ToolbarAction::Format(format) => {
                self.format_headings = false;
                self.notes.update(cx, |input, cx| {
                    let selection = input.selection();
                    let (text, selection) = match format {
                        Some(format) => fern_core::markdown::apply(input.text(), selection, format),
                        None => fern_core::markdown::insert(input.text(), selection, &crate::calendar::format_time(Local::now().time(), i18n::t(cx))),
                    };
                    input.edit(text, selection, window, cx);
                });
                window.focus(&self.notes.focus_handle(cx), cx);
            }
        }
        cx.notify();
    }

    /// For the dev state: `detail-bar=format|comment|format-headings`, and a comment to show.
    #[cfg(debug_assertions)]
    pub fn dev_bar(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            "format" => self.format_bar = true,
            "format-headings" => (self.format_bar, self.format_headings) = (true, true),
            "comment" => {
                self.comment_open = true;
                window.focus(&self.comment_input.focus_handle(cx), cx);
            }
            "comment-emoji" => (self.comment_open, self.comment_emoji) = (true, true),
            text if text.starts_with("comment:") => {
                if let Some(task) = self.task_id() {
                    let _ = self.session.store.add_comment(task, &text["comment:".len()..]);
                    self.sync_comments();
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// The task's comments, as the original lists them under the task: the text, when it was
    /// written, and × on hover to delete it.
    pub(super) fn copy_to_clipboard(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.copied_feedback = true;
        cx.notify();
        self.copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.copied_feedback = false;
                this.copied_reset = None;
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn toggle_markdown_check(&mut self, line_index: usize, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        let current = self.session.task.as_ref().map(|t| t.notes.clone()).unwrap_or_default();
        let Some((updated, checked)) = fern_core::markdown::toggle_check_line(&current, line_index) else {
            return;
        };
        if checked {
            desktop_common::sound::play_complete(&self.session.store);
        }
        self.word_count = fern_core::markdown::count_words(&updated);
        self.notes.update(cx, |input, cx| input.set_text(updated.clone(), cx));
        if self.edit_draft(cx, |task, _| task.notes = updated.clone()) {
            return;
        }
        self.save(cx, |store, id| store.set_notes(id, &updated));
    }

    pub fn print_current(&mut self, cx: &mut Context<Self>) {
        if let Some(task) = self.session.task.clone() {
            let checklist = self.session.store.checklist(task.id).unwrap_or_default();
            let html = crate::print::task_html(&task, &checklist, Local::now().date_naive(), i18n::t(cx));
            crate::print::open_in_background("task", html, cx);
        }
    }

    pub(super) fn sync_attachments(&mut self) {
        if let Some(draft) = &self.session.draft {
            self.session.attachments = draft.attachments.iter().map(|prepared| prepared.metadata().clone()).collect();
        } else {
            self.session.attachments = match self.task_id() {
                Some(id) => self.session.store.attachments(id).unwrap_or_default(),
                None => Vec::new(),
            };
        }
        if let Some(PendingDelete::Attachment(att)) = &self.pending_delete
            && !self.session.attachments.iter().any(|item| item.id == att.id)
        {
            self.clear_pending_delete();
        }
    }

    /// "Upload Attachment": files picked in the system's dialog are copied to the app's folder.
    pub(super) fn upload_attachment(&mut self, cx: &mut Context<Self>) {
        self.session.popup = None;
        if self.is_read_only() {
            return;
        }
        let Some(task) = self.session.task.as_ref().filter(|task| !task.is_deleted()).map(|task| task.id) else { return };
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |this, cx| {
            let Some(paths) = chosen.await.ok().and_then(Result::ok).flatten() else { return };
            this.update(cx, |this, cx| this.attach_paths(task, paths, cx)).ok();
        })
        .detach();
    }

    pub(super) fn attach_paths(&mut self, task: Id, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.task_id() == Some(task) && self.is_read_only() {
            return;
        }
        if self.task_id() == Some(task) {
            self.session.attachment_errors.clear();
            cx.notify();
        }
        let Some(dir) = self.session.attachments_dir.clone() else {
            eprintln!("failed to locate the attachment folder");
            return;
        };
        let staging_dir = dir.clone();
        let is_draft = if self.task_id() == Some(task)
            && let Some(draft) = self.session.draft.as_mut()
        {
            draft.pending_attachments += 1;
            true
        } else {
            false
        };
        let copies = cx.background_spawn(async move {
            paths
                .into_iter()
                .map(|path| {
                    let prepared = PreparedAttachment::copy(task, &path, &staging_dir);
                    (path, prepared)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let copies = copies.await;
            this.update(cx, |this, cx| {
                // A file picker or copy can finish after its draft has been closed/replaced.
                if is_draft {
                    if this.task_id() != Some(task) {
                        return;
                    }
                    let Some(draft) = this.session.draft.as_mut() else { return };
                    draft.pending_attachments = draft.pending_attachments.saturating_sub(1);
                }
                let strings = i18n::t(cx);
                let mut saved_any = false;
                for (path, prepared) in copies {
                    let result = prepared.and_then(|copy| {
                        if is_draft {
                            if let Some(draft) = this.session.draft.as_mut() {
                                draft.attachments.push(copy);
                            }
                            Ok(())
                        } else {
                            this.session.store.save_attachment(copy, &dir).map(|_| {
                                saved_any = true;
                            })
                        }
                    });
                    if let Err(err) = result {
                        eprintln!("failed to attach {}: {err:#}", path.display());
                        if this.task_id() == Some(task) {
                            let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
                            this.session.attachment_errors.push(strings.attachment_failed
                                .replace("{file}", &name)
                                .replace("{error}", &crate::sync::attachment_error(&err, strings)));
                        }
                    }
                }
                this.sync_attachments();
                if saved_any {
                    cx.emit(TaskUpdated);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Changes from another window can edit the task's fields. Keep the caret for edits already in the inputs.
    pub fn refresh(&mut self, task: Option<Task>, cx: &mut Context<Self>) {
        if self.session.draft.is_some() {
            return;
        }
        self.session.preferences = self.session.store.preferences().unwrap_or_default();
        match task {
            Some(task) if self.task_id() == Some(task.id) => {
                if self.task_title() != Some(task.title.as_str()) && self.title.read(cx).text() != task.title {
                    self.title.update(cx, |input, cx| input.set_text(task.title.clone(), cx));
                }
                let old_notes = self.session.task.as_ref().map(|t| t.notes.as_str());
                if old_notes != Some(task.notes.as_str()) && self.notes.read(cx).text() != task.notes {
                    self.word_count = fern_core::markdown::count_words(&task.notes);
                    self.notes.update(cx, |input, cx| input.set_text(task.notes.clone(), cx));
                }
                let old_loc = self.session.task.as_ref().and_then(|t| t.location.as_deref());
                let new_loc = task.location.as_deref().unwrap_or("");
                if old_loc != task.location.as_deref() && self.location_input.read(cx).text() != new_loc {
                    self.location_input.update(cx, |input, cx| input.set_text(new_loc.to_owned(), cx));
                }
                self.location_open |= task.location.is_some();
                self.session.task = Some(task);
                self.sync_read_only(cx);
                self.sync_checklist(cx);
                self.show_checklist |= !self.checklist.is_empty();
                self.sync_subtasks();
                self.sync_attachments();
                self.sync_comments();
                self.sync_sticky_inputs(cx);
                cx.notify();
            }
            other => self.show(other, cx),
        }
    }

    pub(super) fn save(&mut self, cx: &mut Context<Self>, write: impl FnOnce(&Store, Id) -> anyhow::Result<()>) {
        // A new task is written only by finish_draft, with all of its fields together.
        if self.session.draft.is_some() { return; }
        let Some(id) = self.task_id() else { return };
        if let Err(err) = write(&self.session.store, id) {
            eprintln!("failed to save the task: {err:#}");
            return;
        }
        cx.emit(TaskUpdated);
    }

    fn save_text(
        &mut self,
        cx: &mut Context<Self>,
        field: TaskTextField,
        write: impl FnOnce(&Store, Id) -> anyhow::Result<()>,
    ) {
        if self.session.draft.is_some() {
            return;
        }
        let Some(id) = self.task_id() else {
            return;
        };
        if let Err(err) = write(&self.session.store, id) {
            eprintln!("failed to save the task: {err:#}");
            return;
        }
        if let Some(task) = self.session.task.as_mut() {
            match &field {
                TaskTextField::Title(text) => task.title = text.clone(),
                TaskTextField::Notes(text) => task.notes = text.clone(),
                TaskTextField::Location(value) => task.location = value.clone(),
            }
        }
        cx.emit(TaskTextEdited { id, field });
        cx.notify();
    }

    pub(super) fn restore(&mut self, cx: &mut Context<Self>) {
        self.save(cx, |store, id| store.restore_task(id));
    }

    pub(super) fn delete_forever(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.task_id() {
            cx.emit(RequestDeleteForever(id));
        }
    }

    pub(super) fn menu_action(&mut self, cx: &mut Context<Self>, write: impl FnOnce(&Store, Id) -> anyhow::Result<()>) {
        if self.is_read_only() {
            return;
        }
        self.close_popup(cx);
        self.save(cx, write);
    }

    pub(super) fn toggle_wont_do(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        let wont_do = self.session.task.as_ref().is_some_and(|t| t.is_wont_do());
        self.menu_action(cx, |store, id| store.set_wont_do(id, !wont_do));
    }

    pub(super) fn toggle_in_progress(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        let in_progress = self.session.task.as_ref().is_some_and(|t| t.is_in_progress());
        self.menu_action(cx, |store, id| store.set_in_progress(id, !in_progress));
        self.reload_current_task();
    }

    pub(super) fn toggle_kind(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        let kind = match self.session.task.as_ref().map(|task| self.kind_preview(task)) {
            Some(Kind::Note) => Kind::Task,
            _ => Kind::Note,
        };
        if self.edit_draft(cx, |task, draft| {
            task.kind = kind;
            draft.kind_changed = true;
        }) {
            self.close_popup(cx);
        } else {
            self.menu_action(cx, |store, id| store.set_kind(id, kind));
        }
    }

    pub(super) fn open_activity(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.session.task.as_ref() else { return };
        let mut activity = self.session.store.task_activity(task.id).unwrap_or_default();
        // Tasks from before activity was recorded still show when they were created.
        if activity.first().is_none_or(|a| a.kind != "created") {
            activity.insert(0, fern_core::Activity { at: task.created_at, kind: "created".into() });
        }
        self.session.activity = Some(activity);
        cx.notify();
    }

    fn finish_toggle_completed(&mut self, task_id: Id, completed: bool, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        let Some(task) = self.session.task.as_mut() else { return };
        if task.id != task_id {
            return;
        }
        let mut undo = None;
        // Unchecking a "Won't Do" task reopens it.
        let result = if task.is_wont_do() {
            self.session.store.set_wont_do(task.id, false)
        } else if completed {
            crate::undo::complete(&self.session.store, task).map(|action| undo = Some(action))
        } else {
            self.session.store.set_completed(task.id, false)
        };
        if let Err(err) = result {
            eprintln!("failed to update the task: {err:#}");
            return;
        }
        if let Some(action) = undo {
            let label = i18n::t(cx).undo_completed.replace("{title}", &task.title);
            cx.emit(Undoable(action, label));
            crate::completion_mark::record(task.id, cx);
            desktop_common::sound::play_complete(&self.session.store);
        }
        if let Ok(Some(fresh)) = self.session.store.task(task.id) {
            *task = fresh;
        }
        self.sync_sticky_inputs(cx);
        cx.emit(TaskUpdated);
        cx.notify();
    }

    pub(super) fn toggle_completed(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        let Some(task) = self.session.task.clone() else { return };
        let completed = !task.is_completed();
        if completed
            && !task.is_wont_do()
            && desktop_common::plugin_bus::PluginHooksGlobal::global(cx)
                .is_some_and(|h| h.is_active())
        {
            if self.pending_complete == Some(task.id) {
                return;
            }
            let task_id = task.id;
            self.pending_complete = Some(task_id);
            let weak = cx.weak_entity();
            desktop_common::plugin_bus::run_before_task_complete_hook(
                &task,
                cx,
                move |allowed, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if this.pending_complete == Some(task_id) {
                            this.pending_complete = None;
                        }
                        if allowed {
                            this.finish_toggle_completed(task_id, true, cx);
                        }
                    });
                },
            );
            return;
        }
        self.finish_toggle_completed(task.id, completed, cx);
    }
}

/// The activity log's kinds, in the order of `Strings::activity_kinds`.
const ACTIVITY_KINDS: [&str; 12] = [
    "created", "renamed", "completed", "reopened", "date", "date_cleared", "priority", "moved", "deleted", "restored",
    "wont_do", "in_progress",
];

/// What the activity log says for a kind: "created the task".
pub fn activity_text<'a>(kind: &'a str, strings: &'static Strings) -> &'a str {
    ACTIVITY_KINDS.iter().position(|k| *k == kind).map_or(kind, |i| strings.activity_kinds[i])
}

/// Whether a list's name matches the list menu's search, ignoring case.
fn list_matches(name: &str, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty() || name.to_lowercase().contains(&query)
}

fn same_tag(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

fn check_mark() -> impl IntoElement {
    Label::new("✓").color(Color::Accent)
}

#[derive(Clone, Copy)]
pub(super) struct StickyPaletteTokens {
    pub bg: Hsla,
    pub ink: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    pub checkbox_border: Hsla,
    pub surface: Hsla,
    pub accent: Hsla,
    pub error: Hsla,
}

impl StickyPaletteTokens {
    fn from_surface(bg: Hsla, fg: Hsla) -> Self {
        let is_dark = fg.l > 0.5;
        Self {
            bg,
            ink: fg,
            muted: fg.opacity(0.65),
            border: fg.opacity(0.20),
            checkbox_border: fg.opacity(0.58),
            surface: fg.opacity(0.08),
            accent: gpui::rgb(if is_dark { 0x95b7ff } else { 0x254c9e }).into(),
            error: gpui::rgb(if is_dark { 0xff8a8e } else { 0x991b1b }).into(),
        }
    }

    pub(super) fn from_settings(style: crate::sticky_settings::StickySettings) -> Self {
        let palette = crate::sticky_settings::PALETTE;
        let bg = gpui::rgb(palette[style.color.min(palette.len() - 1)].0).into();
        let fg = gpui::rgb(style.foreground()).into();
        Self::from_surface(bg, fg)
    }

    pub(super) fn priority_color(self, priority: Priority) -> Option<Hsla> {
        let is_dark = self.ink.l > 0.5;
        match priority {
            Priority::High => Some(self.error),
            Priority::Medium => Some(gpui::rgb(if is_dark { 0xfbbf24 } else { 0xb45309 }).into()),
            Priority::Low => Some(self.accent),
            Priority::None => None,
        }
    }
}

pub(super) fn sticky_checkbox(
    id: impl Into<gpui::ElementId>,
    checked: bool,
    dismissed: bool,
    in_progress: bool,
    priority_color: Option<Hsla>,
    tokens: StickyPaletteTokens,
) -> gpui::Stateful<gpui::Div> {
    let border = if dismissed {
        tokens.muted
    } else if checked {
        tokens.checkbox_border
    } else if in_progress {
        priority_color.unwrap_or(tokens.accent)
    } else {
        priority_color.unwrap_or(tokens.checkbox_border)
    };
    div()
        .id(id)
        .size(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .overflow_hidden()
        .border_1()
        .border_color(border)
        .when(dismissed, |el| {
            el.bg(tokens.surface).child(
                Icon::new(IconName::X)
                    .size(IconSize::Custom(px(12.)))
                    .color(tokens.muted),
            )
        })
        .when(!dismissed && checked, |el| {
            el.bg(tokens.checkbox_border).child(
                Icon::new(IconName::Check)
                    .size(IconSize::Custom(px(12.)))
                    .color(tokens.bg),
            )
        })
        .when(!dismissed && !checked && in_progress, |el| {
            let fill = priority_color.unwrap_or(tokens.accent);
            el.bg(fill.opacity(0.14))
                .items_end()
                .child(div().w_full().h(px(14.) * 0.44).bg(fill.opacity(0.75)))
        })
        .when(!dismissed && !checked && !in_progress, |el| {
            el.when_some(priority_color, |el, c| el.bg(c.opacity(0.15)))
        })
}

impl TaskDetail {
    pub(super) fn sticky_tokens(&self) -> Option<StickyPaletteTokens> {
        self.sticky_surface.map(|(bg, fg)| StickyPaletteTokens::from_surface(bg, fg))
    }

    fn sync_sticky_inputs(&self, cx: &mut Context<Self>) {
        let tokens = self.sticky_tokens();
        let (ink, muted, caret) = match tokens {
            Some(t) => (Some(t.ink), Some(t.muted), Some(t.ink)),
            None => (None, None, None),
        };
        let task_done = self.session.task.as_ref().is_some_and(|t| t.is_completed());
        let title_ink = tokens.map(|t| if task_done { t.muted } else { t.ink });
        self.title.update(cx, |input, cx| input.set_ink_override(title_ink, muted, caret, cx));
        for input in [
            &self.notes,
            &self.new_item,
            &self.subtask_input,
            &self.comment_input,
            &self.location_input,
        ] {
            input.update(cx, |input, cx| input.set_ink_override(ink, muted, caret, cx));
        }
        for row in &self.checklist {
            let row_ink = tokens.map(|t| if row.item.completed { t.muted } else { t.ink });
            row.input.update(cx, |input, cx| input.set_ink_override(row_ink, muted, caret, cx));
        }
    }

    pub fn set_sticky_style(&mut self, settings: crate::sticky_settings::StickySettings, cx: &mut Context<Self>) {
        let palette = crate::sticky_settings::PALETTE;
        let bg: Hsla = gpui::rgb(palette[settings.color.min(palette.len() - 1)].0).into();
        let fg: Hsla = gpui::rgb(settings.foreground()).into();
        self.sticky_style = Some(settings);
        self.sticky_surface = Some((bg, fg));
        self.editor_window.inline_popup = None;
        self.sync_sticky_inputs(cx);
        cx.notify();
    }
}

/// A file size the way people read it: "512 B", "12 KB", "3.4 MB".
fn format_size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

#[cfg(test)]
mod tests;
