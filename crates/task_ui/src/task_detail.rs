//! The task details panel on the right, modeled on TickTick's.

use std::path::PathBuf;
use std::rc::Rc;

use fern_core::attachment::PreparedAttachment;
use fern_core::{ChecklistItem, Frequency, Id, Kind, List, Priority, Recurrence, Store, Tag, Task};
use chrono::{Local, NaiveDate, TimeZone as _};
use gpui::{Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, FontWeight, MouseButton, Subscription, WeakFocusHandle};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, TipSide, popover, with_tip};

use crate::actions::{self as md, DismissTransient};
use crate::calendar;
use crate::due_picker::{self, CustomReminder, DueDraft, DueMenu};
#[cfg(test)]
use crate::due_picker::DueTab;
use crate::i18n::{self, CurrentLocale, Strings};
use crate::lists_panel;
use crate::task_rows;
use crate::markdown_toolbar::ToolbarAction;
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};

mod comments_attachments;
mod draft;
mod due_popup;
mod popups;
mod session;
mod window;
pub use draft::DraftClosed;
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
    pending_complete: Option<Id>,
    _session_subscriptions: Vec<Subscription>,
}

struct ChecklistRow {
    item: ChecklistItem,
    input: Entity<TextInput>,
    _subscriptions: [Subscription; 2],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusTarget {
    Item(Id),
    NewItem,
}

/// The task was changed in the panel; the list needs to reload.
pub struct TaskUpdated;

/// The user asked to open another task (a subtask) in the panel.
pub struct OpenTask(pub Id);

/// The panel completed or deleted a task; the list offers to undo it.
pub struct Undoable(pub crate::undo::UndoAction, pub String);

/// The user asked to focus on the task ("Start Focus").
pub struct StartFocus(pub Id, pub String, pub bool);

/// The user clicked "Delete forever" on a trashed task; the host asks for confirmation first.
pub struct RequestDeleteForever(pub Id);

impl EventEmitter<TaskUpdated> for TaskDetail {}
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
                this.save(cx, |store, id| store.set_title(id, &text));
            }),
            cx.subscribe(&notes, |this, input, _: &Changed, cx| {
                if this.is_read_only() {
                    return;
                }
                let text = input.read(cx).text().to_owned();
                this.word_count = fern_core::markdown::count_words(&text);
                if this.edit_draft(cx, |task, _| task.notes = text.clone()) { return; }
                this.save(cx, |store, id| store.set_notes(id, &text));
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
                this.save(cx, |store, id| store.set_location(id, value.as_deref()));
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

    /// Whether one of the details' popovers is open, which a click outside the card closes first.
    pub fn has_popup(&self) -> bool {
        self.session.popup.is_some()
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

    pub fn sticky_popup_return_focus(&self, window: &Window, cx: &App) -> WeakFocusHandle {
        self.editor_window.popup_return_focus.clone().unwrap_or_else(|| {
            window.focused(cx).filter(|focus| self.is_editor_focus(focus, cx))
                .unwrap_or_else(|| self.title.focus_handle(cx)).downgrade()
        })
    }

    pub fn sticky_popup_inputs(&self, cx: &App) -> (Option<FocusHandle>, [Entity<TextInput>; 2]) {
        let input = match self.session.popup {
            Some(Popup::Tags | Popup::InlineTags) => Some(&self.tag_input),
            Some(Popup::List) => Some(&self.list_search),
            Some(Popup::LinkParent) => Some(&self.link_search),
            _ => None,
        };
        (input.map(|input| input.focus_handle(cx)), [self.tag_input.clone(), self.time_input.clone()])
    }

    pub fn bind_popup_window(&self, window: &Window, cx: &mut Context<Self>) -> PopupWindow {
        PopupWindow::new(cx.weak_entity(), &self.tag_input, &self.time_input, window, cx)
    }

    /// The same menus and drafts, hosted outside the small native note window.
    pub fn render_sticky_popup(&mut self, available: gpui::Size<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
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

    fn sync_comments(&mut self) {
        self.session.comments = match self.task_id() {
            Some(id) => self.session.store.comments(id).unwrap_or_default(),
            None => Vec::new(),
        };
    }

    /// The description toolbar's buttons: format the notes' selection as Markdown (or type the
    /// time), open the headings menu, or upload an attachment.
    fn toolbar_action(&mut self, action: ToolbarAction, window: &mut Window, cx: &mut Context<Self>) {
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
    fn copy_to_clipboard(&mut self, text: String, cx: &mut Context<Self>) {
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

    fn toggle_markdown_check(&mut self, line_index: usize, cx: &mut Context<Self>) {
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

    fn sync_attachments(&mut self) {
        if let Some(draft) = &self.session.draft {
            self.session.attachments = draft.attachments.iter().map(|prepared| prepared.metadata().clone()).collect();
            return;
        }
        self.session.attachments = match self.task_id() {
            Some(id) => self.session.store.attachments(id).unwrap_or_default(),
            None => Vec::new(),
        };
    }

    /// "Upload Attachment": files picked in the system's dialog are copied to the app's folder.
    fn upload_attachment(&mut self, cx: &mut Context<Self>) {
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

    fn attach_paths(&mut self, task: Id, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
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

    fn sync_subtasks(&mut self) {
        self.session.subtasks = match self.task_id() {
            Some(id) => self.session.store.subtasks(id).unwrap_or_default(),
            None => Vec::new(),
        };
        let parent_id = self.session.task.as_ref().and_then(|task| task.parent_id);
        self.session.parent = parent_id.and_then(|id| self.session.store.task(id).ok().flatten());
        self.session.task_depth = self
            .task_id()
            .and_then(|id| self.session.store.task_depth(id).ok())
            .unwrap_or(0);
    }

    fn task_depth(&self) -> usize {
        self.session.task_depth
    }

    fn can_add_subtask(&self) -> bool {
        !self.is_read_only()
            && self
                .session
                .task
                .as_ref()
                .is_some_and(|task| task.kind != Kind::Note && !task.is_deleted() && self.task_depth() < fern_core::MAX_SUBTASK_DEPTH)
    }

    fn can_link_parent(&self) -> bool {
        !self.is_read_only()
            && self
                .session
                .task
                .as_ref()
                .is_some_and(|task| task.kind != Kind::Note && !task.is_deleted())
    }

    fn reload_current_task(&mut self) {
        if let Some(id) = self.task_id()
            && let Ok(Some(fresh)) = self.session.store.task(id)
        {
            self.session.task = Some(fresh);
        }
        self.sync_subtasks();
    }

    fn unlink_current_parent(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.save(cx, |store, id| store.unlink_parent(id));
        self.reload_current_task();
        cx.notify();
    }

    fn link_current_parent(&mut self, parent: Id, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.save(cx, |store, id| store.link_parent(id, parent));
        self.reload_current_task();
        cx.notify();
    }

    fn unlink_subtask(&mut self, subtask_id: Id, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.save(cx, |store, _| store.unlink_parent(subtask_id));
        self.sync_subtasks();
        cx.notify();
    }

    fn toggle_subtask(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.save(cx, |store, _| store.set_completed(id, completed));
        if completed {
            desktop_common::sound::play_complete(&self.session.store);
        }
        self.reload_current_task();
        cx.notify();
    }

    /// Reloads checklist items, keeping the inputs of items that still exist so that the caret
    /// does not jump while the user types.
    fn sync_checklist(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.task_id() else {
            self.checklist.clear();
            return;
        };
        let items = self.session.draft.as_ref().map_or_else(|| self.session.store.checklist(task).unwrap_or_default(), |draft| draft.checklist.clone());
        let sticky_tokens = self.sticky_tokens();
        let mut old: Vec<ChecklistRow> = std::mem::take(&mut self.checklist);
        for item in items {
            let row = match old.iter().position(|row| row.item.id == item.id) {
                Some(index) => {
                    let mut row = old.swap_remove(index);
                    if row.item.title != item.title && row.input.read(cx).text() != item.title {
                        row.input.update(cx, |input, cx| input.set_text(item.title.clone(), cx));
                    }
                    if row.item.completed != item.completed
                        && let Some(t) = sticky_tokens
                    {
                        let ink = if item.completed { t.muted } else { t.ink };
                        row.input.update(cx, |input, cx| input.set_ink_override(Some(ink), Some(t.muted), Some(t.ink), cx));
                    }
                    row.item = item;
                    row
                }
                None => self.checklist_row(item, cx),
            };
            self.checklist.push(row);
        }
        cx.notify();
    }

    fn checklist_row(&mut self, item: ChecklistItem, cx: &mut Context<Self>) -> ChecklistRow {
        let id = item.id;
        let title = item.title.clone();
        let completed = item.completed;
        let read_only = self.is_read_only();
        let sticky_tokens = self.sticky_tokens();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::Wrapped, "", cx).without_cancel();
            input.set_text(title, cx);
            input.set_read_only(read_only, cx);
            if let Some(t) = sticky_tokens {
                let ink = if completed { t.muted } else { t.ink };
                input.set_ink_override(Some(ink), Some(t.muted), Some(t.ink), cx);
            }
            input
        });
        let subscriptions = [
            cx.subscribe(&input, move |this, input, _: &Changed, cx| {
                if this.is_read_only() {
                    return;
                }
                let text = input.read(cx).text().to_owned();
                if this.edit_draft(cx, |_, draft| {
                    if let Some(item) = draft.checklist.iter_mut().find(|item| item.id == id) { item.title = text.clone(); }
                }) { return; }
                this.save(cx, |store, _| store.set_checklist_item_title(id, &text));
            }),
            self.editor_window.checklist(&input, id, cx),
        ];
        ChecklistRow { item, input, _subscriptions: subscriptions }
    }

    fn toggle_checklist_item(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        if !self.edit_draft(cx, |_, draft| {
            if let Some(item) = draft.checklist.iter_mut().find(|item| item.id == id) { item.completed = completed; }
        }) {
            self.save(cx, |store, _| store.set_checklist_item_completed(id, completed));
        }
        if completed {
            desktop_common::sound::play_complete(&self.session.store);
        }
        self.sync_checklist(cx);
    }

    fn delete_checklist_item(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        if !self.edit_draft(cx, |_, draft| draft.checklist.retain(|item| item.id != id)) {
            self.save(cx, |store, _| store.delete_checklist_item(id));
        }
        self.sync_checklist(cx);
    }

    fn uncheck_all_checklist_items(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        let done_ids: Vec<Id> = self.checklist.iter().filter(|r| r.item.completed).map(|r| r.item.id).collect();
        if done_ids.is_empty() {
            return;
        }
        if !self.edit_draft(cx, |_, draft| {
            for item in &mut draft.checklist {
                item.completed = false;
            }
        }) {
            self.save(cx, |store, _| {
                for id in done_ids {
                    store.set_checklist_item_completed(id, false)?;
                }
                Ok(())
            });
        }
        self.sync_checklist(cx);
    }

    fn clear_completed_checklist_items(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        let done_ids: Vec<Id> = self.checklist.iter().filter(|r| r.item.completed).map(|r| r.item.id).collect();
        if done_ids.is_empty() {
            return;
        }
        if !self.edit_draft(cx, |_, draft| draft.checklist.retain(|item| !item.completed)) {
            self.save(cx, |store, _| {
                for id in done_ids {
                    store.delete_checklist_item(id)?;
                }
                Ok(())
            });
        }
        self.sync_checklist(cx);
    }

    fn start_checklist(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.show_checklist = true;
        self.editor_window.pending_focus = Some(FocusTarget::NewItem);
        cx.notify();
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

    fn save(&mut self, cx: &mut Context<Self>, write: impl FnOnce(&Store, Id) -> anyhow::Result<()>) {
        // A new task is written only by finish_draft, with all of its fields together.
        if self.session.draft.is_some() { return; }
        let Some(id) = self.task_id() else { return };
        if let Err(err) = write(&self.session.store, id) {
            eprintln!("failed to save the task: {err:#}");
            return;
        }
        cx.emit(TaskUpdated);
    }

    fn open_popup(&mut self, popup: Popup, cx: &mut Context<Self>) {
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
                self.session.tag_draft = Some(TagDraft { picked: original.clone(), original, save_failed: false });
                self.tag_input.update(cx, |input, cx| input.set_text("", cx));
            }
            Popup::List => {
                self.list_search.update(cx, |input, cx| input.set_text("", cx));
                self.list_submenu = None;
                self.session.list_sections.clear();
                for list_id in std::iter::once(None).chain(self.session.lists.iter().map(|l| Some(l.id))) {
                    let sections = self.session.store.list_sections(list_id).unwrap_or_default();
                    self.session.list_sections.insert(list_id, sections);
                }
            }
            Popup::Templates => {
                self.session.draft_templates = self.session.store.templates().unwrap_or_default();
            }
            Popup::Priority | Popup::More => {}
            Popup::LinkParent => {
                self.link_search.update(cx, |input, cx| input.set_text("", cx));
                self.session.link_parent_candidates = if let Some(task) = self.session.task.as_ref() {
                    let all_tasks = self.session.store.tasks().unwrap_or_default();
                    let tree = fern_core::TaskTree::build(&all_tasks);
                    let task_depth = tree.depth(task.id);
                    let descendants = tree.descendants(task.id);
                    let subtree_height = descendants
                        .iter()
                        .map(|desc| tree.depth(desc.id).saturating_sub(task_depth))
                        .max()
                        .unwrap_or(0);
                    let excluded: std::collections::HashSet<Id> = descendants
                        .iter()
                        .map(|desc| desc.id)
                        .chain([task.id])
                        .collect();
                    let depth_by_id: std::collections::HashMap<Id, usize> = all_tasks
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
                } else {
                    Vec::new()
                };
            }
            Popup::Estimate => self.session.estimate_draft = EstimateDraft::from(self.session.task.as_ref().and_then(|t| t.estimate)),
        }
        self.session.popup = Some(popup);
        cx.notify();
    }

    fn open_popup_from_ui(&mut self, popup: Popup, window: &Window, cx: &mut Context<Self>) {
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

    fn open_tags(&mut self, popup: Popup, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        self.open_popup_from_ui(popup, window, cx);
        window.focus(&self.tag_input.focus_handle(cx), cx);
    }

    /// Opens the due-date picker (the "Set date" shortcut).
    /// Settings → Task Default → Default Duration, in minutes.
    fn default_duration(&self) -> i64 {
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

    pub fn open_due(&mut self, cx: &mut Context<Self>) {
        if self.session.task.is_some() && !self.is_read_only() {
            self.open_popup(Popup::Due, cx);
        }
    }

    #[cfg(debug_assertions)]
    pub fn dev_open_popup(&mut self, popup: &str, cx: &mut Context<Self>) {
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
                        "due-custom-workday" => Recurrence { nth_workday: Some(1), ..Recurrence::new(Frequency::Monthly, 1) },
                        "due-custom-skip" => Recurrence { skip_weekends: true, ..Recurrence::new(Frequency::Daily, 1) },
                        "due-custom-specific" => {
                            let today = Local::now().date_naive();
                            Recurrence { dates: vec![today, today + chrono::Duration::days(13)], ..Recurrence::new(Frequency::Daily, 1) }
                        }
                        _ => Recurrence { from_completion: true, ..Recurrence::new(Frequency::Weekly, 1) },
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

    fn close_popup(&mut self, cx: &mut Context<Self>) {
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

    /// Dismisses a popover even if focus is still in the task list or calendar.
    pub fn dismiss_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.session.popup.is_none() {
            return false;
        }
        if self.popup_input_focused(window, cx) {
            let focus = self.editor_window.popup_return_focus.take()
                .and_then(|focus| focus.upgrade())
                .unwrap_or_else(|| self.title.focus_handle(cx));
            window.focus(&focus, cx);
        }
        self.close_popup(cx);
        true
    }

    fn update_draft(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut DueDraft)) {
        if let Some(draft) = self.session.due_draft.as_mut() {
            let was_timed = draft.time.is_some();
            let had_date = draft.date.is_some();
            change(draft);
            let timed = draft.time.is_some();
            if !draft.reminders_changed
                && let Some(task_draft) = &self.session.draft
                && (was_timed != timed || had_date != draft.date.is_some())
            {
                draft.reminders = task_draft.default_reminder(timed)
                    .filter(|_| draft.date.is_some())
                    .into_iter()
                    .collect();
            }
            cx.notify();
        }
    }

    /// "OK": saves the draft's date, time, timezone, reminders, and repeat.
    fn apply_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(draft) = self.session.due_draft.take() {
            let today = Local::now().date_naive();
            let changed = self.session.task.as_ref().is_some_and(|task| {
                let before = self.due_preview(task, today);
                draft.date != before.date || draft.time != before.time
                    || draft.end() != before.end() || draft.repeat != before.repeat
                    || draft.timezone != before.timezone || draft.is_floating != before.is_floating
                    || draft.constant_reminder != before.constant_reminder
                    || draft.horizon != before.horizon
            });
            let reminders: Vec<i64> = if draft.date.is_some() { draft.reminders.clone() } else { Vec::new() };
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
    fn clear_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.due_draft = None;
        let today = Local::now().date_naive();
        if !self.edit_draft(cx, |task, state| {
            task.due_date = None; task.due_time = None; task.end_date = None; task.end_time = None;
            task.timezone = None; task.is_floating = false; task.constant_reminder = false;
            task.horizon = None;
            task.reminders.clear(); task.repeat = None; state.date_changed = true;
            state.reminders_changed = false;
        }) { self.save(cx, |store, id| {
            store.set_due(id, None, None)?;
            store.set_due_end(id, None)?;
            store.set_timezone(id, None, false)?;
            store.set_reminders(id, &[])?;
            store.set_task_constant_reminder(id, false)?;
            store.set_horizon(id, None)?;
            store.set_repeat(id, None, today)
        }); } else {
            self.refresh_draft_title(cx);
        }
        self.dismiss_popup(window, cx);
    }

    fn set_priority(&mut self, priority: Priority, cx: &mut Context<Self>) {
        if !self.edit_draft(cx, |task, draft| { task.priority = priority; draft.priority_changed = true; }) {
            self.save(cx, |store, id| store.set_priority(id, priority));
        }
        self.close_popup(cx);
    }

    fn set_list(&mut self, list: Option<Id>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.edit_draft(cx, |task, draft| { task.list_id = list; task.section_id = None; draft.list_changed = true; }) {
            self.save(cx, |store, id| store.set_list(id, list));
        }
        self.dismiss_popup(window, cx);
    }

    fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.update_draft(cx, |d| d.month = calendar::add_months(d.month, delta));
    }

    fn task_tags(&self) -> Vec<String> {
        self.session.task.as_ref().map(|t| t.tags.clone()).unwrap_or_default()
    }

    fn tag_query<'a>(&self, cx: &'a App) -> &'a str {
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
        let name = self.session.all_tags.iter().find(|tag| same_tag(&tag.name, &query)).map_or(query, |tag| tag.name.clone());
        if let Some(draft) = &mut self.session.tag_draft {
            if !draft.picked.iter().any(|picked| same_tag(picked, &name)) {
                draft.picked.push(name);
            }
            draft.save_failed = false;
            self.tag_input.update(cx, |input, cx| input.clear(window, cx));
            cx.notify();
        }
    }

    fn toggle_picked_tag(&mut self, name: String, cx: &mut Context<Self>) {
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

    fn apply_tags(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn tag_color(&self, name: &str, cx: &App) -> Hsla {
        let color = self.session.all_tags.iter().find(|t| t.name == name).and_then(|t| t.color.as_deref());
        lists_panel::color(color, cx)
    }

    fn restore(&mut self, cx: &mut Context<Self>) {
        self.save(cx, |store, id| store.restore_task(id));
    }

    fn delete_forever(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.task_id() {
            cx.emit(RequestDeleteForever(id));
        }
    }

    fn menu_action(&mut self, cx: &mut Context<Self>, write: impl FnOnce(&Store, Id) -> anyhow::Result<()>) {
        if self.is_read_only() {
            return;
        }
        self.close_popup(cx);
        self.save(cx, write);
    }

    fn toggle_wont_do(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        let wont_do = self.session.task.as_ref().is_some_and(|t| t.is_wont_do());
        self.menu_action(cx, |store, id| store.set_wont_do(id, !wont_do));
    }

    fn toggle_in_progress(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() {
            return;
        }
        let in_progress = self.session.task.as_ref().is_some_and(|t| t.is_in_progress());
        self.menu_action(cx, |store, id| store.set_in_progress(id, !in_progress));
        self.reload_current_task();
    }

    fn toggle_kind(&mut self, cx: &mut Context<Self>) {
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

    fn open_activity(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.session.task.as_ref() else { return };
        let mut activity = self.session.store.task_activity(task.id).unwrap_or_default();
        // Tasks from before activity was recorded still show when they were created.
        if activity.first().is_none_or(|a| a.kind != "created") {
            activity.insert(0, fern_core::Activity { at: task.created_at, kind: "created".into() });
        }
        self.session.activity = Some(activity);
        cx.notify();
    }

    /// The "Task Activities" panel at the bottom of the details, as in the original.
    fn render_activity(&self, activity: &[fern_core::Activity], strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let sticky = self.sticky_tokens();

        v_scroll(
            "task-activity-scroll",
            v_flex()
                .id("task-activity")
                .px_5()
                .py_3()
                .gap_2()
                .child(
                    h_flex()
                        .child(
                            div().flex_1().child(
                                Label::new(strings.task_activities)
                                    .weight(FontWeight::SEMIBOLD)
                                    .when_some(sticky, |l, t| l.color(t.ink)),
                            ),
                        )
                        .child(
                            IconButton::new("close-activity", IconName::X)
                                .when_some(sticky, |b, t| b.icon_color(t.muted).hover_color(t.surface))
                                .tooltip(Tooltip::text(strings.close))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.session.activity = None;
                                    cx.notify();
                                })),
                        ),
                )
                .children(activity.iter().rev().map(|entry| {
                    let text = activity_text(&entry.kind, strings);
                    let when = chrono::Local
                        .timestamp_millis_opt(entry.at)
                        .single()
                        .map(|t| format!("{} {}", calendar::format_day_month(t.date_naive(), strings), calendar::format_time(t, strings)))
                        .unwrap_or_default();
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(div().mt(px(5.)).size(px(8.)).flex_none().rounded_full().bg(sticky.map_or(theme.colors().accent, |t| t.accent)))
                        .child(
                            v_flex()
                                .child(Label::new(text).size(LabelSize::Small).when_some(sticky, |l, t| l.color(t.ink)))
                                .child(Label::new(when).size(LabelSize::XSmall).color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted)))),
                        )
                })),
        )
        .max_h(px(240.))
        .border_t_1()
        .border_color(sticky.map_or(theme.colors().border, |t| t.border))
        .into_any_element()
    }

    /// The "Link Parent Task" picker: open tasks other than this one and its subtasks, filtered by
    /// the search field, each with its list.
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

    fn toggle_completed(&mut self, cx: &mut Context<Self>) {
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

    fn render_progress_bar(
        done: usize,
        total: usize,
        theme: &theme::Theme,
        sticky: Option<StickyPaletteTokens>,
    ) -> AnyElement {
        let ratio = if total == 0 { 0.0 } else { (done as f32 / total as f32).clamp(0.0, 1.0) };
        h_flex()
            .gap_2()
            .pb_1()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .h(px(3.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(sticky.map_or(theme.colors().border_variant, |t| t.border))
                    .child(div().h_full().w(gpui::relative(ratio)).rounded_full().bg(sticky.map_or(theme.colors().accent, |t| t.accent))),
            )
            .child(Label::new(format!("{done}/{total}")).size(LabelSize::XSmall).color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted))))
            .into_any_element()
    }

    fn render_checklist(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let sticky = self.sticky_tokens();
        let read_only = self.is_read_only();
        let total = self.checklist.len();
        let done_count = self.checklist.iter().filter(|r| r.item.completed).count();
        let muted_text = sticky.map_or(theme.colors().text_muted, |t| t.muted);
        let delete_hover = sticky.map_or(theme.status().error, |t| t.error);
        v_flex()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(total > 0, |el| {
                el.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(div().flex_1().min_w_0().child(Self::render_progress_bar(done_count, total, &theme, sticky)))
                        .when(done_count > 0 && !read_only, |bar| {
                            bar.child(
                                IconButton::new("checklist-uncheck-all", IconName::RotateCcw)
                                    .size(px(22.))
                                    .icon_size(IconSize::XSmall)
                                    .icon_color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted)))
                                    .when_some(sticky, |b, t| b.hover_color(t.surface))
                                    .tooltip(Tooltip::text(strings.checklist_uncheck_all))
                                    .on_click(cx.listener(|this, _, _, cx| this.uncheck_all_checklist_items(cx))),
                            )
                            .child(
                                IconButton::new("checklist-clear-completed", IconName::BrushCleaning)
                                    .size(px(22.))
                                    .icon_size(IconSize::XSmall)
                                    .icon_color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted)))
                                    .when_some(sticky, |b, t| b.hover_color(t.surface))
                                    .tooltip(Tooltip::text(strings.checklist_clear_completed))
                                    .on_click(cx.listener(|this, _, _, cx| this.clear_completed_checklist_items(cx))),
                            )
                        }),
                )
            })
            .children(self.checklist.iter().map(|row| {
                let id = row.item.id;
                let done = row.item.completed;
                let row_group = SharedString::from(format!("checklist-item-{id}"));
                let checkbox_id = SharedString::from(format!("item-checkbox-{id}"));
                let checkbox = if let Some(t) = sticky {
                    sticky_checkbox(checkbox_id, done, false, false, None, t)
                        .when(!read_only, |cb| {
                            cb.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_checklist_item(id, !done, cx)
                            }))
                        })
                        .into_any_element()
                } else {
                    Checkbox::new(checkbox_id)
                        .size(px(14.))
                        .toggle_state(done)
                        .when(!read_only, |cb| {
                            cb.on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_checklist_item(id, !done, cx)
                            }))
                        })
                        .into_any_element()
                };
                h_flex()
                    .id(SharedString::from(format!("item-{id}")))
                    .debug_selector(move || format!("checklist-item-{id}"))
                    .group(row_group.clone())
                    .min_h(px(30.))
                    .items_start()
                    .gap_2()
                    .py_1()
                    .child(div().mt(px(3.)).child(checkbox))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(done, |el| el.text_color(muted_text).line_through())
                            .child(row.input.clone()),
                    )
                    .when(!read_only, |el| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("item-delete-{id}")))
                                .flex_none()
                                .px_1()
                                .text_color(muted_text)
                                .opacity(if crate::touch::active() { 1. } else { 0. })
                                .group_hover(row_group, |s| s.opacity(1.))
                                .hover(move |s| s.text_color(delete_hover))
                                .cursor_pointer()
                                .tooltip(Tooltip::text(strings.delete))
                                .child("✕")
                                .on_click(cx.listener(move |this, _, _, cx| this.delete_checklist_item(id, cx))),
                        )
                    })
            }))
            .when(!read_only, |el| {
                el.child(
                    h_flex()
                        .min_h(px(30.))
                        .gap_2()
                        .child(Icon::new(IconName::Plus).size(IconSize::Small).when_some(sticky, |i, t| i.color(t.muted)))
                        .child(div().flex_1().child(self.new_item.clone())),
                )
            })
    }

    fn render_subtasks(&self, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let sticky = self.sticky_tokens();
        let can_add = self.can_add_subtask();
        let can_edit = !self.is_read_only();
        let total = self.session.subtasks.len();
        let done_count = self.session.subtasks.iter().filter(|s| s.is_completed()).count();
        let today = Local::now().date_naive();
        let countdown_dates = self.session.preferences.countdown_dates;
        v_flex()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(total > 0, |el| el.child(Self::render_progress_bar(done_count, total, &theme, sticky)))
            .children(self.session.subtasks.iter().map(|subtask| {
                let id = subtask.id;
                let done = subtask.is_completed();
                let group_name = SharedString::from(format!("subtask-row-{id}"));
                let due = subtask
                    .due_date
                    .map(|date| task_rows::row_due_label(date, subtask.due_time, today, strings, countdown_dates));
                let checkbox_id = SharedString::from(format!("subtask-checkbox-{id}"));
                let checkbox = if let Some(t) = sticky {
                    sticky_checkbox(
                        checkbox_id,
                        done,
                        subtask.is_wont_do(),
                        subtask.is_in_progress(),
                        t.priority_color(subtask.priority),
                        t,
                    )
                    .when(can_edit, |cb| {
                        cb.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_subtask(id, !done, cx);
                        }))
                    })
                    .into_any_element()
                } else {
                    Checkbox::new(checkbox_id)
                        .size(px(14.))
                        .toggle_state(done)
                        .in_progress(subtask.is_in_progress())
                        .dismissed(subtask.is_wont_do())
                        .tint(task_rows::priority_color(subtask.priority, &theme))
                        .when(can_edit, |cb| {
                            cb.on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_subtask(id, !done, cx);
                            }))
                        })
                        .into_any_element()
                };
                div().group(group_name.clone()).child(
                    ListItem::new(SharedString::from(format!("subtask-{id}")))
                        .height(px(30.))
                        .when_some(sticky, |item, t| item.hover_color(t.surface))
                        .start_slot(checkbox)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .when(done, |el| el.line_through())
                                .child(
                                    Label::new(if subtask.title.is_empty() {
                                        SharedString::new_static(strings.untitled)
                                    } else {
                                        subtask.title.clone().into()
                                    })
                                    .color(if done {
                                        sticky.map_or(Color::Muted, |t| Color::Custom(t.muted))
                                    } else {
                                        sticky.map_or(Color::Default, |t| Color::Custom(t.ink))
                                    })
                                    .truncate(),
                                ),
                        )
                        .end_slot(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .when_some(due, |el, (label, overdue)| {
                                    el.child(
                                        Label::new(label)
                                            .size(LabelSize::XSmall)
                                            .color(if overdue && !done {
                                                sticky.map_or(Color::Error, |t| Color::Custom(t.error))
                                            } else {
                                                sticky.map_or(Color::Muted, |t| Color::Custom(t.muted))
                                            }),
                                    )
                                })
                                .when(can_edit, |el| {
                                    el.child(
                                        div()
                                            .debug_selector(move || format!("subtask-unlink-{id}"))
                                            .opacity(if crate::touch::active() { 1. } else { 0. })
                                            .group_hover(group_name.clone(), |s| s.opacity(1.))
                                            .child(
                                                IconButton::new(SharedString::from(format!("subtask-unlink-btn-{id}")), IconName::X)
                                                    .size(px(22.))
                                                    .icon_size(IconSize::XSmall)
                                                    .icon_color(sticky.map_or(Color::Muted, |t| Color::Custom(t.muted)))
                                                    .when_some(sticky, |b, t| b.hover_color(t.surface))
                                                    .tooltip(Tooltip::text(strings.unlink_parent))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.unlink_subtask(id, cx);
                                                    })),
                                            ),
                                    )
                                })
                                .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).when_some(sticky, |i, t| i.color(t.muted))),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenTask(id)))),
                )
            }))
            .when(can_add, |el| {
                el.child(
                    h_flex()
                        .min_h(px(30.))
                        .px_2()
                        .gap_2()
                        .child(Icon::new(IconName::Plus).size(IconSize::Small).when_some(sticky, |i, t| i.color(t.muted)))
                        .child(div().flex_1().child(self.subtask_input.clone())),
                )
            })
    }

    /// The Duration tab: start and end with day and time, "All Day", and a calendar or time list
    /// for the field being picked.
    fn render_tags_row(&self, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let sticky = self.sticky_tokens();
        let read_only = self.is_read_only();
        h_flex()
            .flex_wrap()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // The Estimation (Start Focus → Estimation), which opens it again.
            .children(self.session.task.as_ref().and_then(|t| t.estimate).map(|estimate| {
                let text = match estimate {
                    fern_core::Estimate::Pomos(n) => format!("{n} {}", strings.pomo_unit),
                    fern_core::Estimate::Minutes(n) => strings.format_minutes(n as i64),
                };
                let chip_color = sticky.map_or(Color::Accent, |t| Color::Custom(t.accent));
                let chip_bg = sticky.map_or_else(|| theme.colors().accent.opacity(0.2), |t| t.surface);
                let chip_hover = sticky.map_or_else(|| theme.colors().accent.opacity(0.3), |t| t.accent.opacity(0.2));
                h_flex()
                    .id("estimate-chip")
                    .h(px(18.))
                    .px(px(6.))
                    .gap_1()
                    .rounded_full()
                    .bg(chip_bg)
                    .when(!read_only, |el| {
                        el.cursor_pointer()
                            .hover(move |s| s.bg(chip_hover))
                            .tooltip(Tooltip::text(strings.estimation))
                            .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Estimate, cx)))
                    })
                    .child(Icon::new(IconName::CircleDot).size(IconSize::XSmall).color(chip_color))
                    .child(Label::new(text).size(LabelSize::XSmall).color(chip_color))
            }))
            // As the original's, a tag is a plain chip; a click opens the tag picker, where it is
            // unticked.
            .children(self.task_tags().into_iter().map(|tag| {
                div()
                    .id(SharedString::from(format!("tag-chip-{tag}")))
                    .when(!read_only, |el| {
                        el.cursor_pointer().on_click(cx.listener(|this, _, window, cx| {
                            this.open_tags(Popup::InlineTags, window, cx);
                        }))
                    })
                    .child(match sticky {
                        Some(t) => div()
                            .h(px(18.))
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .bg(t.surface)
                            .text_color(t.ink)
                            .text_size(px(11.))
                            .child(tag)
                            .into_any_element(),
                        None => {
                            let color = self.tag_color(&tag, cx);
                            Chip::new(tag, color).into_any_element()
                        }
                    })
            }))
            .when(!read_only, |el| {
                let add_border = sticky.map_or(theme.colors().accent, |t| t.accent);
                let add_hover = sticky.map_or(theme.colors().element_hover, |t| t.surface);
                let add_color = sticky.map_or(Color::Accent, |t| Color::Custom(t.accent));
                el.child(
                    // The original's "+" after the tags: an accent-outlined circle.
                    div()
                        .id("add-tag")
                        .relative()
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .border_1()
                        .border_color(add_border)
                        .cursor_pointer()
                        .hover(move |s| s.bg(add_hover))
                        .child(Icon::new(IconName::Plus).size(IconSize::Custom(px(11.))).color(add_color))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_tags(Popup::InlineTags, window, cx);
                        }))
                        .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::InlineTags), |el| {
                            el.child(PopoverAttachment::new(Placement::BelowLeft, self.render_tags_popup(cx)))
                        }),
                )
            })
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

/// A text button with an icon, used for the due date and the list.
fn label_button(id: &'static str, icon: IconName, label: SharedString, color: Color, hover: Hsla) -> Stateful<ui::HorizontalStack> {
    h_flex()
        .id(id)
        .relative()
        .gap_1p5()
        .px_1()
        .py_0p5()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(Icon::new(icon).size(IconSize::Large).color(color))
        .child(Label::new(label).color(color))
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

    fn from_settings(style: crate::sticky_settings::StickySettings) -> Self {
        let palette = crate::sticky_settings::PALETTE;
        let bg = gpui::rgb(palette[style.color.min(palette.len() - 1)].0).into();
        let fg = gpui::rgb(style.foreground()).into();
        Self::from_surface(bg, fg)
    }

    fn priority_color(self, priority: Priority) -> Option<Hsla> {
        let is_dark = self.ink.l > 0.5;
        match priority {
            Priority::High => Some(self.error),
            Priority::Medium => Some(gpui::rgb(if is_dark { 0xfbbf24 } else { 0xb45309 }).into()),
            Priority::Low => Some(self.accent),
            Priority::None => None,
        }
    }
}

fn sticky_checkbox(
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

impl Render for TaskDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let sticky_tokens = self.sticky_tokens();
        let button_hover = sticky_tokens.map_or(theme.colors().element_hover, |t| t.surface);
        let muted = sticky_tokens.map_or(Color::Muted, |t| Color::Custom(t.muted));
        let sticky = sticky_tokens.is_some();
        if let Some(target) = self.editor_window.take_pending_focus(window) {
            let input = match target {
                FocusTarget::NewItem => Some(self.new_item.clone()),
                FocusTarget::Item(id) => {
                    self.checklist.iter().find(|row| row.item.id == id).map(|row| row.input.clone())
                }
            };
            if let Some(input) = input {
                window.focus(&input.focus_handle(cx), cx);
            }
        }
        let panel = v_flex()
            .size_full()
            .key_context(if self.session.draft.is_some() { "TaskDetail TaskDraft" } else { "TaskDetail" })
            .on_action(cx.listener(|this, _: &crate::actions::SaveDraft, window, cx| {
                if this.session.draft.is_some() && this.session.popup == Some(Popup::Due) {
                    this.apply_due(window, cx);
                }
                this.finish_draft(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::PrintDetail, _, cx| {
                this.print_current(cx);
            }))
            .on_action(cx.listener(|this, _: &DismissTransient, window, cx| {
                if !this.dismiss_popup(window, cx) {
                    if this.session.draft.is_some() { this.finish_draft(cx); } else { cx.propagate(); }
                }
            }))
            .map(|el| match self.sticky_style {
                Some(style) => el.text_color(gpui::rgb(style.foreground())),
                None => el.bg(theme.colors().background)
                    .text_color(theme.colors().text)
                    .when(!self.card, |el| el.border_l_1().border_color(theme.colors().border)),
            });

        let Some(task) = self.session.task.as_ref() else {
            return panel
                .items_center()
                .justify_center()
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .px_6()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .size(px(52.))
                                .rounded_full()
                                .bg(sticky_tokens.map_or(theme.colors().surface_background, |t| t.surface))
                                .border_1()
                                .border_color(sticky_tokens.map_or(theme.colors().border_variant, |t| t.border))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(IconName::SquareCheck).size(IconSize::Large).color(muted)),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_center()
                                .text_size(px(13.))
                                .text_color(sticky_tokens.map_or(theme.colors().text_muted, |t| t.muted))
                                .child(strings.select_task),
                        ),
                );
        };
        let done = task.is_completed();
        let notes_focus = self.notes.focus_handle(cx);
        let notes_editing = notes_focus.is_focused(window);
        let notes_focus_click = notes_focus.clone();
        let today = Local::now().date_naive();
        let preview = self.session.draft.as_ref().map(|_| self.due_preview(task, today));
        let (date, time, repeats, tz_name, is_floating, horizon) = preview.as_ref().map_or(
            (task.due_date, task.due_time, task.repeat.is_some(), task.timezone.as_deref(), task.is_floating, task.horizon),
            |due| (due.date, due.time, due.repeat.is_some(), due.timezone.as_deref(), due.is_floating, due.horizon),
        );
        let tz_suffix = if time.is_some() {
            due_picker::short_timezone_label(tz_name, is_floating, strings)
        } else {
            None
        };
        let kind = self.kind_preview(task);
        let (due_text, due_color): (SharedString, Color) = match (date, horizon) {
            (Some(date), _) => {
                let (label, overdue) = task_rows::due_label(date, time, today, strings);
                let label = match task.end_date {
                    Some(end) => due_picker::range_label(date, time, end, task.end_time, strings),
                    None => label,
                };
                // The calendar's card writes the date out, as the original's.
                let mut label = if self.calendar_card && task.end_date.is_none() {
                    task_rows::card_due_label(date, time, today, strings)
                } else {
                    task_rows::detail_due_label(label, date, today, strings)
                };
                if let Some(tz) = tz_suffix {
                    label = format!("{label} · {tz}");
                }
                (label.into(), if overdue && !done { Color::Error } else { Color::Accent })
            }
            (None, Some(h)) => {
                let label = calendar::format_horizon_badge(h, strings);
                let is_past = h.is_past_relative_to(today);
                let is_current = h != fern_core::Horizon::Life
                    && h == fern_core::Horizon::current_for(h.granularity(), today);
                let color = if is_past && !done {
                    Color::Warning
                } else if is_current && !done {
                    Color::Accent
                } else {
                    muted
                };
                (label.into(), color)
            }
            // A note, as the original's, offers a reminder rather than a due date.
            (None, None) => ((if kind == Kind::Note { strings.set_reminder } else { strings.due_date }).into(), muted),
        };
        let due_icon = if date.is_none() && horizon.is_some() {
            IconName::FernHorizons
        } else {
            IconName::CalendarDays
        };
        let in_progress = task.is_in_progress();
        let read_only = self.is_read_only();
        let show_location = self.location_open || task.location.is_some();
        let priority_color = task_rows::priority_color(task.priority, &theme).map_or(Color::Muted, Color::Custom);
        let list_id = self.list_preview(task);
        let list_name: SharedString = list_id
            .and_then(|id| self.session.lists.iter().find(|l| l.id == id))
            .map(|l| l.name.clone().into())
            .or_else(|| self.session.archived_list_name.clone().map(Into::into))
            .unwrap_or_else(|| strings.inbox.into());
        let list_icon = if list_id.is_some() { IconName::List } else { IconName::Inbox };
        let task_id = task.id;
        let drop_color = sticky_tokens.map_or_else(|| theme.colors().accent.opacity(0.08), |t| t.surface);

        panel
            .when(!read_only, |el| {
                el.drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(drop_color))
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                        this.attach_paths(task_id, paths.paths().to_vec(), cx);
                    }))
            })
            // Top bar: checkbox | due date ……… priority
            .when(!sticky, |panel| panel.child(
                h_flex()
                    .debug_selector(|| "task-detail-header".into())
                    .when(self.top_inset, |el| el.mt(desktop_common::layout::header_inset()))
                    .gap_2()
                    .h(px(48.))
                    .flex_none()
                    .px_5()
                    .border_b_1()
                    .border_color(theme.colors().border)
                    // The original's 15 px box and the short line after it; a note has neither.
                    .when(task.kind != Kind::Note && self.session.draft.is_none(), |el| {
                        el.child(
                            Checkbox::new("detail-checkbox")
                                .size(px(15.))
                                .toggle_state(done)
                                .animate_completion(crate::completion_mark::started(task.id, cx))
                                .dismissed(task.is_wont_do())
                                .in_progress(in_progress)
                                .lines(!self.checklist.is_empty())
                                .tint(task_rows::priority_color(task.priority, &theme))
                                .when(!read_only, |cb| {
                                    cb.on_click(cx.listener(|this, _, _, cx| this.toggle_completed(cx)))
                                }),
                        )
                        .child(div().w(px(1.)).h(px(14.)).bg(theme.colors().border))
                    })
                    .child(if read_only {
                        h_flex()
                            .id("due-date")
                            .debug_selector(|| "task-detail-due-date".into())
                            .gap_1p5()
                            .px_1p5()
                            .py_1()
                            .rounded(px(6.))
                            .opacity(0.75)
                            .child(Icon::new(due_icon).color(due_color))
                            .child(Label::new(due_text.clone()).color(due_color))
                            .when(repeats, |el| el.child(Icon::new(IconName::Repeat).size(IconSize::Small).color(due_color)))
                            .into_any_element()
                    } else {
                        label_button("due-date", due_icon, due_text.clone(), due_color, button_hover)
                            .debug_selector(|| "task-detail-due-date".into())
                            .when(repeats, |el| el.child(Icon::new(IconName::Repeat).size(IconSize::Small).color(due_color)))
                            .on_click(cx.listener(|this, _, window, cx| this.open_popup_from_ui(Popup::Due, window, cx)))
                            .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::Due), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowLeft,
                                    self.render_due_popup(strings, window, cx),
                                ))
                            })
                            .into_any_element()
                    })
                    .when(in_progress, |el| {
                        el.child(
                            h_flex()
                                .id("detail-in-progress-badge")
                                .px_1p5()
                                .py_0p5()
                                .gap_1()
                                .rounded(px(5.))
                                .bg(theme.colors().accent.opacity(0.14))
                                .child(Icon::new(IconName::Hourglass).size(IconSize::XSmall).color(Color::Accent))
                                .child(Label::new(strings.in_progress).size(LabelSize::XSmall).color(Color::Accent))
                                .when(!read_only, |el| {
                                    el.cursor_pointer()
                                        .tooltip(Tooltip::text(strings.clear_in_progress))
                                        .on_click(cx.listener(|this, _, _, cx| this.toggle_in_progress(cx)))
                                }),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .debug_selector(|| "task-detail-priority".to_owned())
                            .relative()
                            .child(if read_only {
                                h_flex()
                                    .size(px(28.))
                                    .justify_center()
                                    .opacity(0.75)
                                    .child(Icon::new(if kind == Kind::Note { IconName::CircleAlert } else { IconName::Flag }).color(priority_color))
                                    .into_any_element()
                            } else {
                                // A note's priority shows as the original's "!" in a circle.
                                IconButton::new("priority", if kind == Kind::Note { IconName::CircleAlert } else { IconName::Flag })
                                    .icon_color(priority_color)
                                    .tip(strings.summary_priority, TipSide::Below)
                                    .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Priority, cx)))
                                    .into_any_element()
                            })
                            .when(!read_only && self.sticky_style.is_none() && self.session.popup == Some(Popup::Priority), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    self.render_priority_popup(strings, cx),
                                ))
                            }),
                    ),
            ))
            // Title, checklist, notes and tags; clicking empty space puts the caret into the notes.
            .child(
                v_scroll(
                    "detail-body-scroll",
                    v_flex()
                        .id("detail-body")
                        .debug_selector(|| "task-detail-body".to_owned())
                        .when_some(self.sticky_style, |el, style| el.text_size(px(style.font_px())))
                        .px_5()
                        .pt_4()
                        .pb_6()
                        .gap_3()
                        .when(sticky, |el| el.px_3().pt_0p5().pb_2().gap_1())
                        .when(!read_only, |el| {
                            el.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                window.focus(&notes_focus, cx);
                            })
                        })
                        // A subtask names its parent over the title, as the original; a click opens it,
                        // and hovering exposes Edit and Unlink Parent Task buttons.
                        .when_some(self.session.parent.as_ref(), |el, parent| {
                        let parent_id = parent.id;
                        let parent_title: SharedString = if parent.title.is_empty() {
                            strings.untitled.into()
                        } else {
                            parent.title.clone().into()
                        };
                        el.child(
                            h_flex()
                                .id("detail-parent")
                                .group("detail-parent-row")
                                .mb(px(-6.))
                                .gap_1()
                                .min_w_0()
                                .items_center()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(
                                    h_flex()
                                        .id("detail-parent-open")
                                        .min_w_0()
                                        .gap_1()
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenTask(parent_id))))
                                        .child(Label::new(parent_title).color(muted).truncate())
                                        .child(Icon::new(IconName::ChevronRight).size(IconSize::Small).color(muted)),
                                )
                                .when(!read_only && !sticky, |el| {
                                    el.child(
                                        h_flex()
                                            .flex_none()
                                            .gap_0p5()
                                            .opacity(if crate::touch::active() { 1. } else { 0. })
                                            .group_hover("detail-parent-row", |s| s.opacity(1.))
                                            .child(
                                                IconButton::new("detail-edit-parent", IconName::Pencil)
                                                    .size(px(22.))
                                                    .icon_size(IconSize::XSmall)
                                                    .icon_color(Color::Muted)
                                                    .tooltip(Tooltip::text(strings.link_parent))
                                                    .on_click(cx.listener(|this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        this.open_popup_from_ui(Popup::LinkParent, window, cx);
                                                        window.focus(&this.link_search.focus_handle(cx), cx);
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .debug_selector(|| "detail-unlink-parent".to_owned())
                                                    .child(
                                                        IconButton::new("detail-unlink-parent-btn", IconName::X)
                                                            .size(px(22.))
                                                            .icon_size(IconSize::XSmall)
                                                            .icon_color(Color::Muted)
                                                            .tooltip(Tooltip::text(strings.unlink_parent))
                                                            .on_click(cx.listener(|this, _, _, cx| {
                                                                cx.stop_propagation();
                                                                this.unlink_current_parent(cx);
                                                            })),
                                                    ),
                                            ),
                                    )
                                }),
                        )
                    })
                    .child(
                        h_flex()
                            .items_start()
                            .gap_3()
                            .child(
                                div()
                                    .debug_selector(|| "task-detail-title".to_owned())
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(20.))
                                    .when_some(self.sticky_style, |el, style| el.text_size(px(style.font_px())))
                                    .line_height(px(28.))
                                    .when_some(self.sticky_style, |el, style| el.line_height(px(style.font_px() + 5.)))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .when(done, |el| el.text_color(sticky_tokens.map_or(theme.colors().text_muted, |t| t.muted)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .child(self.title.clone()),
                            )
                            .when(!sticky && !read_only, |title| title.child(
                                div()
                                    .debug_selector(|| "task-detail-checklist-toggle".to_owned())
                                    .pt_0p5()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .child(
                                        IconButton::new("checklist", IconName::ListChecks)
                                            .tooltip(Tooltip::text(i18n::t(cx).checklist))
                                            .on_click(cx.listener(|this, _, _, cx| this.start_checklist(cx))),
                                    ),
                            )),
                    )
                    // As in the original, the description comes first and the checklist under it.
                    // While it is not edited, the description is drawn formatted, as the original
                    // shows it; a click opens it for editing.
                    .child(if read_only {
                        if !task.notes.trim().is_empty() {
                            div()
                                .id("notes-formatted")
                                .debug_selector(|| "task-notes-preview".to_owned())
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(render_markdown_interactive(
                                    &task.notes,
                                    self.sticky_style,
                                    Rc::new(|_, _, _| {}),
                                    cx,
                                ))
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        }
                    } else if !notes_editing && !task.notes.trim().is_empty() {
                        div()
                            .id("notes-formatted")
                            .debug_selector(|| "task-notes-preview".to_owned())
                            .cursor_text()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            // On the release, so that a click on a link opens it instead.
                            .on_click(move |_, window, cx| window.focus(&notes_focus_click, cx))
                            .child(render_markdown_interactive(
                                &task.notes,
                                self.sticky_style,
                                {
                                    let view = cx.entity().downgrade();
                                    Rc::new(move |line_index, _, cx| {
                                        view.update(cx, |this, cx| this.toggle_markdown_check(line_index, cx)).ok();
                                    })
                                },
                                cx,
                            ))
                            .into_any_element()
                    } else {
                        div()
                            .debug_selector(|| "task-notes-editor".to_owned())
                            .key_context("SummaryEditor")
                            .on_action(cx.listener(|this, _: &md::Bold, window, cx| this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Bold)), window, cx)))
                            .on_action(cx.listener(|this, _: &md::Highlight, window, cx| this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Highlight)), window, cx)))
                            .on_action(cx.listener(|this, _: &md::CheckItem, window, cx| this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Checklist)), window, cx)))
                            .on_action(cx.listener(|this, _: &md::BulletedList, window, cx| this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Bullets)), window, cx)))
                            .on_action(cx.listener(|this, _: &md::NumberedList, window, cx| this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Numbered)), window, cx)))
                            .on_action(cx.listener(|this, _: &md::Italic, window, cx| this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Italic)), window, cx)))
                            .on_action(cx.listener(|this, _: &md::Underline, window, cx| this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Underline)), window, cx)))
                            .on_action(cx.listener(|this, _: &md::Strike, window, cx| this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Strike)), window, cx)))
                            .on_action(cx.listener(|this, _: &md::Link, window, cx| this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Link)), window, cx)))
                            .on_action(cx.listener(|this, _: &md::Code, window, cx| this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Code)), window, cx)))
                            .on_action(cx.listener(|this, _: &md::Quote, window, cx| this.toolbar_action(ToolbarAction::Format(Some(fern_core::markdown::Format::Quote)), window, cx)))
                            .when(sticky && !notes_editing, |el| el.opacity(0.))
                            .text_size(px(15.))
                            .line_height(px(22.))
                            .when_some(self.sticky_style, |el, style| el.text_size(px(style.font_px())).line_height(px(style.font_px() + 8.)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(self.notes.clone())
                            .into_any_element()
                    })
                    .when(self.show_checklist, |el| el.child(self.render_checklist(cx)))
                    .when(!self.session.attachments.is_empty(), |el| el.child(self.render_attachments(cx)))
                    .children(self.session.attachment_errors.iter().map(|error| {
                        Label::new(error.clone()).color(sticky_tokens.map_or(Color::Error, |t| Color::Custom(t.error)))
                    }))
                    .when(show_location && !sticky, |el| {
                        el.child(
                            h_flex()
                                .id("detail-location-row")
                                .h(px(30.))
                                .px_2()
                                .gap_2()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(theme.colors().border_variant)
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(Icon::new(IconName::MapPin).size(IconSize::Small).color(Color::Muted))
                                .child(div().flex_1().min_w_0().text_size(px(13.)).child(self.location_input.clone()))
                                .when(!read_only, |el| {
                                    el.child(
                                        IconButton::new("detail-location-clear", IconName::X)
                                            .size(px(20.))
                                            .icon_size(IconSize::XSmall)
                                            .icon_color(Color::Muted)
                                            .tooltip(Tooltip::text(strings.clear))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.location_open = false;
                                                this.location_input.update(cx, |input, cx| input.clear(window, cx));
                                                if !this.edit_draft(cx, |task, _| task.location = None) {
                                                    this.save(cx, |store, id| store.set_location(id, None));
                                                }
                                                cx.notify();
                                            })),
                                    )
                                }),
                        )
                    })
                    // Tags sit under the notes, as in the original.
                    // The calendar's card leaves out the empty tags and subtasks, as the original's.
                    // As the original's, the tags row only once there are tags (the "…" menu's Tags
                    // adds the first), and the subtasks only once there are some or one is begun.
                    .when(!task.tags.is_empty() || task.estimate.is_some() || self.session.popup == Some(Popup::InlineTags), |el| {
                        el.child(div().pt(px(18.)).child(self.render_tags_row(strings, cx)))
                    })
                    .when(!self.session.subtasks.is_empty() || self.subtask_open, |el| {
                        let divider = match sticky_tokens {
                            Some(t) => div().flex_none().h(px(1.)).w_full().bg(t.border).into_any_element(),
                            None => Divider::horizontal().into_any_element(),
                        };
                        el.child(div().pt_2().child(divider)).child(self.render_subtasks(strings, cx))
                    })
                    .when(!self.session.comments.is_empty(), |el| el.child(self.render_comments(strings, cx))),
                )
                .track_scroll(&self.body_scroll)
                .flex_1()
                .min_h_0()
                .when_some(self.sticky_style, |el, style| el.color(gpui::rgb(style.foreground()).into())),
            )
            .when(self.session.draft.as_ref().is_some_and(|draft| draft.save_failed), |el| {
                el.child(div().px_5().py_1().child(Label::new(strings.task_save_failed).color(sticky_tokens.map_or(Color::Error, |t| Color::Custom(t.error)))))
            })
            .when(self.session.draft.as_ref().is_some_and(|draft| draft.pending_attachments > 0), |el| {
                el.child(div().px_5().py_1().child(Label::new(strings.attachment_preparing).color(muted)))
            })
            // The bottom bar's "A" shows the description's Markdown toolbar over it, as the original.
            .when(self.format_bar && !read_only, |el| {
                let on_action: crate::markdown_toolbar::OnAction<Self> = Rc::new(|this, action, window, cx| this.toolbar_action(action, window, cx));
                el.child(
                    div().px_3().pb_2().child(
                        crate::markdown_toolbar::render("detail-tool", self.format_headings, true, cx, on_action)
                            .flex_wrap()
                            .px_1()
                            .py_0p5()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .bg(theme.colors().elevated_surface_background),
                    ),
                )
            })
            .when(self.comment_open && !read_only, |el| el.child(self.render_comment_box(cx)))
            .when_some(self.session.activity.clone(), |el, activity| el.child(self.render_activity(&activity, strings, cx)))
            // Bottom bar: list ……… formatting, comments, more; for a trashed task, as in the
            // original: Restore ……… delete forever; for an archived-list task, read-only list label.
            .child(
                h_flex()
                    .gap_2()
                    // As the original's: no line over the bottom bar.
                    .h(px(44.))
                    .flex_none()
                    .px_5()
                    .when(sticky, |bar| bar.h(px(29.)).px_3())
                    .map(|bar| {
                        if task.is_deleted() {
                            bar.child(
                                label_button(
                                    "restore-task",
                                    IconName::ArchiveRestore,
                                    strings.restore.into(),
                                    sticky_tokens.map_or(Color::Default, |t| Color::Custom(t.ink)),
                                    button_hover,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.restore(cx))),
                            )
                            .child(div().flex_1())
                            .child(
                                IconButton::new("delete-forever", IconName::Trash)
                                    .when_some(sticky_tokens, |b, t| b.icon_color(t.muted).hover_color(t.surface))
                                    .tooltip(Tooltip::text(strings.delete_forever))
                                    .on_click(cx.listener(|this, _, _, cx| this.delete_forever(cx))),
                            )
                        } else if read_only {
                            bar.when(sticky, |bar| bar.child(
                                div()
                                    .id("sticky-date")
                                    .debug_selector(|| "task-detail-due-date".into())
                                    .flex()
                                    .items_center()
                                    .min_w_0()
                                    .text_size(px(11.))
                                    .text_color(sticky_tokens.map_or_else(|| theme.colors().text.opacity(0.68), |t| t.muted))
                                    .when(date.is_none(), |date| date.opacity(0.))
                                    .child(div().truncate().child(due_text.clone())),
                            ))
                            .when(!sticky, |bar| bar.child(
                                h_flex()
                                    .id("list")
                                    .debug_selector(|| "task-detail-list".to_owned())
                                    .gap_1p5()
                                    .px_1p5()
                                    .py_1()
                                    .opacity(0.75)
                                    .child(Icon::new(list_icon).color(muted))
                                    .child(Label::new(list_name).color(muted)),
                            ))
                            .child(div().flex_1())
                            .when(!sticky && self.word_count > 0, |bar| {
                                bar.child(
                                    Label::new(strings.word_count.replace("{n}", &self.word_count.to_string()))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            })
                        } else {
                            bar.when(sticky, |bar| bar.child(
                                div()
                                    .id("sticky-date")
                                    .debug_selector(|| "task-detail-due-date".into())
                                    .flex()
                                    .items_center()
                                    .relative()
                                    .min_w_0()
                                    .cursor_pointer()
                                    .text_size(px(11.))
                                    .text_color(sticky_tokens.map_or_else(|| theme.colors().text.opacity(0.68), |t| t.muted))
                                    .when(date.is_none(), |date| date.opacity(0.).group_hover("sticky-window", |style| style.opacity(1.)))
                                    .child(div().truncate().child(due_text.clone()))
                                    .on_click(cx.listener(|this, _, window, cx| this.open_popup_from_ui(Popup::Due, window, cx)))
                                    .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::Due), |el| {
                                        el.child(PopoverAttachment::new(Placement::AboveLeft, self.render_due_popup(strings, window, cx)))
                                    }),
                            ))
                            .when(!sticky, |bar| bar.child(
                                with_tip(
                                    label_button("list", list_icon, list_name, muted, button_hover).debug_selector(|| "task-detail-list".to_owned()),
                                    "list-tip",
                                    strings.move_to,
                                    TipSide::Above,
                                    window,
                                    cx,
                                )
                                    .on_click(cx.listener(|this, _, window, cx| this.open_popup_from_ui(Popup::List, window, cx)))
                                    .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::List), |el| {
                                        el.child(PopoverAttachment::new(
                                            Placement::AboveLeft,
                                            self.render_list_popup(strings, cx),
                                        ))
                                    }),
                            ))
                            .child(div().flex_1())
                            .when(self.copied_feedback, |bar| {
                                let chip_bg = sticky_tokens.map_or_else(|| theme.colors().accent.opacity(0.14), |t| t.surface);
                                let chip_color = sticky_tokens.map_or(Color::Accent, |t| Color::Custom(t.accent));
                                bar.child(
                                    h_flex()
                                        .gap_1()
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .bg(chip_bg)
                                        .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(chip_color))
                                        .child(Label::new(strings.summary_copied).size(LabelSize::XSmall).color(chip_color)),
                                )
                            })
                            .when(!sticky && self.word_count > 0, |bar| {
                                bar.child(
                                    Label::new(strings.word_count.replace("{n}", &self.word_count.to_string()))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            })
                            .when(!sticky && self.session.draft.is_none(), |bar| bar.child(
                                IconButton::new("format", IconName::Baseline)
                                    .toggle_state(self.format_bar)
                                    .tip(strings.text_format, TipSide::Above)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.format_bar = !this.format_bar;
                                        this.format_headings = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                IconButton::new("comments", IconName::MessageSquareText)
                                    .toggle_state(self.comment_open)
                                    .tip(strings.comment_tip, TipSide::Above)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.comment_open = !this.comment_open;
                                        if this.comment_open {
                                            window.focus(&this.comment_input.focus_handle(cx), cx);
                                        }
                                        cx.notify();
                                    })),
                            ))
                            .child(
                                div()
                                    .debug_selector(|| "task-detail-more".to_owned())
                                    .relative()
                                    .child(
                                        IconButton::new("more", IconName::Ellipsis)
                                            .when(sticky, |button| button.size(px(20.)).icon_size(IconSize::XSmall).icon_color(muted).hover_color(button_hover))
                                            .when(!sticky && !matches!(self.session.popup, Some(Popup::More | Popup::Tags)), |b| b.tip(strings.more, TipSide::Above))
                                            .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::More, cx))),
                                    )
                                    .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::More), |el| {
                                        el.child(PopoverAttachment::new(
                                            Placement::AboveRight,
                                            self.render_more_popup(strings, cx),
                                        ))
                                    })
                                    .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::Tags), |el| {
                                        el.child(PopoverAttachment::new(Placement::AboveRight, self.render_tags_popup(cx)))
                                    })
                                    .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::Templates), |el| {
                                        el.child(PopoverAttachment::new(Placement::AboveRight, self.render_draft_templates(cx)))
                                    })
                                    .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::Estimate), |el| {
                                        el.child(PopoverAttachment::new(
                                            Placement::AboveRight,
                                            self.render_estimate(strings, cx),
                                        ))
                                    })
                                    .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::LinkParent), |el| {
                                        el.child(PopoverAttachment::new(
                                            Placement::AboveRight,
                                            self.render_link_parent(strings, cx),
                                        ))
                                    }),
                            )
                        }
                    }),
            )
    }
}

/// A file size the way people read it: "512 B", "12 KB", "3.4 MB".
#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use gpui::{FileDropEvent, InputEvent as _, TestAppContext};

    #[gpui::test]
    fn blank_lines_keep_their_height_when_task_notes_lose_focus(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Task").unwrap();
        store.set_notes(task.id, "First.\n\nSecond.\n\n\nThird.").unwrap();
        let task = store.task(task.id).unwrap().unwrap();
        cx.update(theme::init);
        let (detail, cx) = cx.add_window_view(|window, cx| {
            let mut detail = TaskDetail::new(store, window, cx);
            detail.show(Some(task), cx);
            window.focus(&detail.notes.focus_handle(cx), cx);
            detail
        });
        let editor = cx.debug_bounds("task-notes-editor").expect("notes editor is visible");

        cx.update(|window, cx| window.focus(&detail.read(cx).title.focus_handle(cx), cx));
        cx.run_until_parked();
        let preview = cx.debug_bounds("task-notes-preview").expect("notes preview is visible");

        assert_eq!(preview.size.height, editor.size.height);
    }

    #[gpui::test]
    fn escape_in_task_detail_closes_the_picker_and_preserves_editor_focus(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Task").unwrap();
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(CurrentLocale(i18n::Locale::En));
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut detail = TaskDetail::new(store, window, cx);
            detail.show(Some(task), cx);
            window.focus(&detail.title.focus_handle(cx), cx);
            detail
        });

        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| assert!(view.read(cx).title.focus_handle(cx).is_focused(window)));

        cx.update(|window, cx| view.update(cx, |detail, cx| {
            window.focus(&detail.notes.focus_handle(cx), cx);
            detail.open_tags(Popup::Tags, window, cx);
        }));
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            let detail = view.read(cx);
            assert!(!detail.has_popup());
            assert!(detail.notes.focus_handle(cx).is_focused(window));
        });

        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| assert!(view.read(cx).notes.focus_handle(cx).is_focused(window)));
    }

    #[gpui::test]
    fn dropped_paths_stay_with_the_task_that_received_them(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("fern-drop-{}", uuid::Uuid::now_v7()));
        let file = dir.join("receipt.txt");
        let folder = dir.join("plans");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(&file, "paid").unwrap();
        std::fs::write(folder.join("next.txt"), "tomorrow").unwrap();

        let attachments_dir = dir.join("attachments");
        let store = Rc::new(Store::open_in_memory().unwrap());
        let first = store.add_task("First task").unwrap();
        let second = store.add_task("Second task").unwrap();
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(CurrentLocale(i18n::Locale::En));
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.session.attachments_dir = Some(attachments_dir.clone());
            detail.show(Some(first.clone()), cx);
            detail
        });

        let position = cx.debug_bounds("task-detail-body").unwrap().center();
        cx.simulate_event(FileDropEvent::Entered { position, paths: ExternalPaths([file.clone(), folder].into_iter().collect()) });
        cx.update(|window, cx| {
            window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
            assert!(store.attachments(first.id).unwrap().is_empty());
            view.update(cx, |detail, cx| detail.show(Some(second.clone()), cx));
        });
        cx.run_until_parked();

        let attached = store.attachments(first.id).unwrap();
        assert_eq!(attached.iter().map(|attachment| attachment.name.as_str()).collect::<Vec<_>>(), ["receipt.txt", "plans.zip"]);
        assert_eq!(std::fs::read_to_string(attached[0].path(&attachments_dir)).unwrap(), "paid");
        assert!(store.attachments(second.id).unwrap().is_empty());
        assert!(cx.debug_bounds("task-attachment").is_none());
        cx.update(|_, cx| view.update(cx, |detail, cx| detail.show(Some(first), cx)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("task-attachment").is_some());

        store.delete_task(second.id).unwrap();
        let deleted = store.task(second.id).unwrap();
        cx.update(|_, cx| view.update(cx, |detail, cx| detail.show(deleted, cx)));
        cx.run_until_parked();
        let position = cx.debug_bounds("task-detail-body").unwrap().center();
        cx.simulate_event(FileDropEvent::Entered { position, paths: ExternalPaths([file].into_iter().collect()) });
        cx.simulate_event(FileDropEvent::Submit { position });
        assert!(store.attachments(second.id).unwrap().is_empty());
        assert!(cx.debug_bounds("task-attachment").is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn archived_list_and_trashed_tasks_are_read_only_and_delete_forever_requests_confirmation(
        cx: &mut TestAppContext,
    ) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let list = store.create_list("ArchiveList").unwrap();
        let task = store.add_task("Archived task").unwrap();
        store.set_list(task.id, Some(list.id)).unwrap();
        store.set_notes(task.id, "- [ ] check me").unwrap();
        let check_item = store.add_checklist_item(task.id, "item 1", None).unwrap();
        store.add_comment(task.id, "read-only note").unwrap();
        store.set_list_archived(list.id, true).unwrap();

        let task = store.task(task.id).unwrap().unwrap();
        let task_id = task.id;
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(CurrentLocale(i18n::Locale::En));
        });
        let requested_purge = Rc::new(std::cell::RefCell::new(Vec::<Id>::new()));
        let requested_purge_sub = requested_purge.clone();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            // Mirror TodoView: only active (non-archived) lists are passed into TaskDetail::set_lists.
            detail.set_lists(Vec::new());
            detail.show(Some(task), cx);
            detail
        });
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, RequestDeleteForever(id), _| {
                requested_purge_sub.borrow_mut().push(*id);
            })
            .detach();
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            let detail = view.read(cx);
            assert!(detail.is_read_only(), "archived-list task must be read-only");
            assert_eq!(detail.session.archived_list_name.as_deref(), Some("ArchiveList"));
            assert!(detail.title.read(cx).is_read_only());
            assert!(detail.notes.read(cx).is_read_only());
            assert!(detail.location_input.read(cx).is_read_only());
            assert!(
                detail.checklist.iter().all(|row| row.input.read(cx).is_read_only()),
                "checklist inputs must be read-only in archived list"
            );

            view.update(cx, |detail, cx| {
                detail.toggle_completed(cx);
                detail.open_popup(Popup::More, cx);
                detail.open_popup_from_ui(Popup::Due, window, cx);
                detail.start_checklist(cx);
                detail.toggle_markdown_check(0, cx);
                detail.toggle_checklist_item(check_item.id, true, cx);
                assert!(!detail.has_popup(), "read-only task must not open edit popups");
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("task-detail-more").is_none(), "read-only detail hides more menu");
        assert!(
            cx.debug_bounds("task-detail-checklist-toggle").is_none(),
            "read-only detail hides checklist mode toggle"
        );
        let stored = store.task(task_id).unwrap().unwrap();
        assert!(!stored.is_completed(), "read-only toggle_completed must be a no-op");
        assert_eq!(stored.notes, "- [ ] check me", "read-only markdown check toggle must be a no-op");
        assert!(!store.checklist(task_id).unwrap()[0].completed);

        // Unarchiving the list restores editability on refresh.
        store.set_list_archived(list.id, false).unwrap();
        let unarchived_lists = store.lists().unwrap();
        let refreshed = store.task(task_id).unwrap();
        cx.update(|_, cx| {
            view.update(cx, |detail, cx| {
                detail.set_lists(unarchived_lists);
                detail.refresh(refreshed, cx);
                assert!(!detail.is_read_only());
                assert!(!detail.title.read(cx).is_read_only());
                assert!(!detail.notes.read(cx).is_read_only());
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("task-detail-more").is_some());

        // Moving to Trash makes the task read-only again, and delete_forever requests confirmation
        // instead of purging immediately.
        store.delete_task(task_id).unwrap();
        let deleted = store.task(task_id).unwrap();
        cx.update(|_, cx| {
            view.update(cx, |detail, cx| {
                detail.refresh(deleted, cx);
                assert!(detail.is_read_only(), "trashed task must be read-only");
                assert!(detail.title.read(cx).is_read_only());
                detail.toggle_completed(cx);
                detail.delete_forever(cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(*requested_purge.borrow(), vec![task_id]);
        assert!(
            store.task(task_id).unwrap().is_some(),
            "delete_forever must wait for host confirmation before purging"
        );
    }

    #[gpui::test]
    fn cross_polarity_sticky_inputs_and_tokens_use_sticky_ink(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Cross-polarity note").unwrap();
        let item = store.add_checklist_item(task.id, "Open item", None).unwrap();
        store.add_subtask(task.id, "Child subtask").unwrap();
        store.add_comment(task.id, "A comment").unwrap();
        let task = store.task(task.id).unwrap().unwrap();

        // Dark app theme + light pastel yellow sticky (color 0 -> fg 0x24262b).
        cx.update(|cx| {
            theme::init(cx);
            theme::set(theme::Theme::dark(), cx);
            cx.set_global(CurrentLocale(i18n::Locale::En));
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.show(Some(task.clone()), cx);
            detail.set_sticky_style(crate::sticky_settings::StickySettings { color: 0, ..Default::default() }, cx);
            detail
        });
        cx.run_until_parked();

        let light_fg: Hsla = gpui::rgb(0x24262b).into();
        let light_bg: Hsla = gpui::rgb(crate::sticky_settings::PALETTE[0].0).into();
        cx.update(|window, cx| {
            let detail = view.read(cx);
            let tokens = detail.sticky_tokens().expect("sticky tokens set");
            assert_eq!(tokens.bg, light_bg);
            assert_eq!(tokens.ink, light_fg);
            assert_eq!(tokens.muted, light_fg.opacity(0.65));
            assert_eq!(tokens.border, light_fg.opacity(0.20));
            assert_eq!(tokens.checkbox_border, light_fg.opacity(0.58));
            assert_eq!(tokens.surface, light_fg.opacity(0.08));

            for focus_input in [
                detail.title.clone(),
                detail.notes.clone(),
                detail.new_item.clone(),
                detail.subtask_input.clone(),
                detail.checklist[0].input.clone(),
            ] {
                window.focus(&focus_input.focus_handle(cx), cx);
                window.refresh();
                window.draw(cx).clear(cx);
                assert!(
                    window.painted_quads().iter().any(|quad| quad.background == gpui::Background::from(light_fg)),
                    "sticky caret must paint in light_fg instead of dark-theme accent"
                );
            }
        });

        // Adding a new checklist item inherits the active sticky ink override, and toggling
        // completion switches its text override to muted sticky ink.
        cx.update(|_, cx| {
            view.update(cx, |detail, cx| {
                detail.add_checklist_item("Second item", Some(item.id), cx);
                detail.sync_checklist(cx);
                detail.toggle_checklist_item(item.id, true, cx);
            });
        });
        cx.run_until_parked();

        // Light app theme + dark charcoal sticky (color 11 -> fg 0xf5f5f5).
        let dark_fg: Hsla = gpui::rgb(0xf5f5f5).into();
        cx.update(|window, cx| {
            theme::set(theme::Theme::light(), cx);
            view.update(cx, |detail, cx| {
                detail.set_sticky_style(crate::sticky_settings::StickySettings { color: 11, ..Default::default() }, cx);
                detail.comment_open = true;
                window.focus(&detail.comment_input.focus_handle(cx), cx);
            });
            window.refresh();
            window.draw(cx).clear(cx);
            assert!(
                window.painted_quads().iter().any(|quad| quad.background == gpui::Background::from(dark_fg)),
                "comment input caret on dark sticky must paint in dark_fg"
            );
            let title = view.read(cx).title.clone();
            window.focus(&title.focus_handle(cx), cx);
            window.refresh();
            window.draw(cx).clear(cx);
            assert!(
                window.painted_quads().iter().any(|quad| quad.background == gpui::Background::from(dark_fg)),
                "title caret on dark sticky must paint in dark_fg"
            );
        });
    }
}

fn format_size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

/// A description drawn as the original shows it while it is not edited: headings, numbered items
/// with their numbers in the accent color, bullets, check items, quotes and lines, with bold,
/// italic, underlined, struck, highlighted, code and link runs.
type MarkdownCheckToggle = Rc<dyn Fn(usize, &mut Window, &mut App)>;

pub fn render_markdown(text: &str, sticky: Option<crate::sticky_settings::StickySettings>, cx: &App) -> AnyElement {
    render_markdown_blocks(fern_core::markdown::blocks(text), sticky, true, None, cx)
}

fn render_markdown_interactive(
    text: &str,
    sticky: Option<crate::sticky_settings::StickySettings>,
    on_toggle_check: MarkdownCheckToggle,
    cx: &App,
) -> AnyElement {
    render_markdown_blocks(fern_core::markdown::blocks(text), sticky, true, Some(on_toggle_check), cx)
}

pub fn render_release_markdown(text: &str, cx: &App) -> AnyElement {
    use fern_core::markdown::{Block, BlockKind, blocks};

    // The published Markdown is hard-wrapped in the source. Let the window wrap its prose,
    // keeping paragraph breaks and list markers where the author put them.
    let source = text.lines().map(str::trim_start).collect::<Vec<_>>().join("\n");
    let mut reflowed: Vec<Block> = Vec::new();
    for mut block in blocks(&source) {
        for (_, style) in &mut block.runs {
            if style.link.as_deref().is_some_and(|url| !url.starts_with("https://") && !url.starts_with("http://")) {
                style.link = None;
            }
        }
        if block.kind == BlockKind::Paragraph
            && let Some(previous) = reflowed.last_mut()
            && matches!(previous.kind, BlockKind::Paragraph | BlockKind::Bullet | BlockKind::Numbered(_))
        {
            previous.text.push(' ');
            let offset = previous.text.len();
            previous.text.push_str(&block.text);
            previous.runs.extend(block.runs.into_iter().map(|(range, style)| {
                ((range.start + offset)..(range.end + offset), style)
            }));
        } else {
            reflowed.push(block);
        }
    }
    render_markdown_blocks(reflowed, None, false, None, cx)
}

fn render_markdown_blocks(
    blocks: Vec<fern_core::markdown::Block>,
    sticky: Option<crate::sticky_settings::StickySettings>,
    show_images: bool,
    on_toggle_check: Option<MarkdownCheckToggle>,
    cx: &App,
) -> AnyElement {
    use fern_core::markdown::BlockKind;
    let theme = cx.theme();
    let sticky_tokens = sticky.map(StickyPaletteTokens::from_settings);
    let line_height = px(sticky.map_or(22., |style| style.font_px() + 8.));
    let accent = sticky_tokens.map_or(theme.colors().text_accent, |t| t.accent);
    let muted = sticky_tokens.map_or(theme.colors().text_muted, |t| t.muted);
    let styled = |text: String, runs: Vec<(std::ops::Range<usize>, fern_core::markdown::Style)>| {
        let highlights = runs
            .into_iter()
            .map(|(range, style)| {
                let mut h = gpui::HighlightStyle::default();
                if style.bold {
                    h.font_weight = Some(FontWeight::BOLD);
                }
                if style.italic {
                    h.font_style = Some(gpui::FontStyle::Italic);
                }
                if style.strike {
                    h.strikethrough = Some(gpui::StrikethroughStyle { thickness: px(1.), color: None });
                }
                if style.underline || style.link.is_some() {
                    h.underline = Some(gpui::UnderlineStyle { thickness: px(1.), color: None, wavy: false });
                }
                if style.link.is_some() {
                    h.color = Some(accent);
                }
                if style.highlight {
                    h.background_color = Some(theme.status().warning.opacity(0.35));
                }
                if style.code {
                    h.background_color = Some(sticky_tokens.map_or(theme.colors().element_hover, |t| t.surface));
                }
                (range, h)
            })
            .collect::<Vec<_>>();
        gpui::StyledText::new(text).with_highlights(highlights)
    };
    // Each Markdown block already accounts for its source line, including blank lines.
    v_flex()
        .text_size(px(15.))
        .line_height(line_height)
        .when_some(sticky, |el, settings| el.text_size(px(settings.font_px())))
        .children(blocks.into_iter().enumerate().map(|(ix, block)| {
            // Links open in the browser on a click, as Zed's Markdown opens them. The click is
            // taken on the release by the block, before the description's own click opens it for
            // editing; the text only gives links the pointing hand.
            let links: Vec<_> = block.runs.iter().filter_map(|(range, style)| Some((range.clone(), style.link.clone()?))).collect();
            let ranges = links.iter().map(|(range, _)| range.clone()).collect();
            let styled_text = styled(block.text.clone(), block.runs);
            let layout = styled_text.layout().clone();
            let text = gpui::InteractiveText::new(("md-block", ix), styled_text).on_click(ranges, |_, _, _| {});
            let body = div().id(("md-body", ix)).flex_1().min_w_0().child(text).when(!links.is_empty(), |el| {
                el.on_click(move |event, _, cx| {
                    let Ok(at) = layout.index_for_position(event.position()) else { return };
                    if let Some((_, url)) = links.iter().find(|(range, _)| range.contains(&at)) {
                        cx.stop_propagation();
                        cx.open_url(url);
                    }
                })
            });
            match block.kind {
                BlockKind::BlankLine => div().h(line_height).flex_none().into_any_element(),
                BlockKind::Heading(level) => div()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(match level {
                        1 => 20.,
                        2 => 18.,
                        _ => 16.,
                    }))
                    .child(body)
                    .into_any_element(),
                BlockKind::Numbered(n) => h_flex()
                    .items_start()
                    .child(div().w(px(19.)).flex_none().text_color(accent).child(format!("{n}.")))
                    .child(body)
                    .into_any_element(),
                BlockKind::Bullet => h_flex()
                    .items_start()
                    .child(div().w(px(22.)).flex_none().text_color(accent).child("•"))
                    .child(body)
                    .into_any_element(),
                BlockKind::Check(done) => {
                    let cb = if let Some(t) = sticky_tokens {
                        let cb = sticky_checkbox(("md-check", ix), done, false, false, None, t);
                        if let Some(toggle) = on_toggle_check.clone() {
                            cb.cursor_pointer()
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    toggle(ix, window, cx);
                                })
                                .into_any_element()
                        } else {
                            cb.into_any_element()
                        }
                    } else {
                        let cb = Checkbox::new(("md-check", ix)).size(px(14.)).toggle_state(done);
                        if let Some(toggle) = on_toggle_check.clone() {
                            cb.on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                toggle(ix, window, cx);
                            })
                            .into_any_element()
                        } else {
                            cb.into_any_element()
                        }
                    };
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(div().pt(px(4.)).child(cb))
                        .child(body.when(done, |el| el.line_through().text_color(muted)))
                        .into_any_element()
                }
                BlockKind::Quote => div().pl_3().border_l_2().border_color(accent).text_color(muted).child(body).into_any_element(),
                BlockKind::Divider => div().my_1().h(px(1.)).bg(sticky_tokens.map_or(theme.colors().border, |t| t.border)).into_any_element(),
                BlockKind::Image(_) if !show_images => body.text_color(muted).into_any_element(),
                // An image line shows the image at most as wide as the panel, as the original; a
                // relative address is a file among the attachments.
                BlockKind::Image(url) => {
                    let source: gpui::ImageSource = if url.starts_with("http://") || url.starts_with("https://") {
                        gpui::SharedUri::from(url).into()
                    } else {
                        let path = std::path::PathBuf::from(url.strip_prefix("file://").unwrap_or(&url));
                        let path = if path.is_relative() { crate::attachments_dir().map(|dir| dir.join(&path)).unwrap_or(path) } else { path };
                        path.into()
                    };
                    let alt = SharedString::from(block.text);
                    // In a block of its own, as Zed's Markdown wraps its images, so that the
                    // column does not squeeze it to nothing.
                    div()
                        .flex_none()
                        .child(
                            gpui::img(source)
                                .id(("md-image", ix))
                                .min_w_0()
                                .max_w_full()
                                .rounded_md()
                                .with_fallback(move || div().text_color(muted).child(alt.clone()).into_any_element()),
                        )
                        .into_any_element()
                }
                BlockKind::Paragraph => body.into_any_element(),
            }
        }))
        .into_any_element()
}

#[cfg(test)]
mod size_tests {
    #[test]
    fn sizes() {
        assert_eq!(super::format_size(512), "512 B");
        assert_eq!(super::format_size(12 * 1024 + 5), "12 KB");
        assert_eq!(super::format_size(3_565_158), "3.4 MB");
    }

    #[test]
    fn list_menu_search() {
        assert!(super::list_matches("Work", ""));
        assert!(super::list_matches("Work", " wo"));
        assert!(super::list_matches("Входящие", "вх"));
        assert!(!super::list_matches("Work", "home"));
    }
}
