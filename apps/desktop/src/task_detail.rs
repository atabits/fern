//! The task details panel on the right, modeled on TickTick's.

use std::rc::Rc;

use fern_core::{ChecklistItem, Frequency, Id, Kind, List, Priority, Recurrence, Store, Tag, Task};
use chrono::{Datelike, Local, NaiveDate, TimeZone as _};
use gpui::{Entity, EventEmitter, Focusable, FontWeight, MouseButton, Subscription};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, SubmenuItem, popover};

use crate::calendar;
use crate::due_picker::{self, CustomReminder, DueDraft, DueMenu, DueTab, RangeField, ReminderUnit};
use crate::i18n::{self, CurrentLocale, Strings};
use crate::lists_panel;
use crate::task_rows;
use crate::markdown_toolbar::ToolbarAction;
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Popup {
    Due,
    Priority,
    Tags,
    List,
    More,
    /// "Link Parent Task": a searchable list of tasks to nest this one under.
    LinkParent,
    /// Start Focus → "Estimation": the expected pomos or duration.
    Estimate,
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
    store: Rc<Store>,
    /// The task's attached files.
    attachments: Vec<fern_core::Attachment>,
    /// The task's comments, the bottom bar's comment field (while open) and its emoji menu.
    comments: Vec<fern_core::Comment>,
    comment_input: Entity<TextInput>,
    comment_open: bool,
    comment_emoji: bool,
    /// The bottom bar's "A": the Markdown toolbar for the description, and its headings menu.
    format_bar: bool,
    format_headings: bool,
    /// The month the custom repeat's "By Specific Dates" calendar shows.
    specific_month: Option<NaiveDate>,
    /// The "…" menu's Start Focus side menu, while hovered open.
    focus_submenu: bool,
    estimate_draft: EstimateDraft,
    task: Option<Task>,
    title: Entity<TextInput>,
    notes: Entity<TextInput>,
    tag_input: Entity<TextInput>,
    /// All tags, for the tag picker and chip colors.
    all_tags: Vec<Tag>,
    /// All lists, for the list picker and the list button label.
    lists: Vec<List>,
    /// Checklist items with an input each, in display order.
    checklist: Vec<ChecklistRow>,
    new_item: Entity<TextInput>,
    /// Direct subtasks of the shown task.
    subtasks: Vec<Task>,
    subtask_input: Entity<TextInput>,
    /// The due-date popover's field for typing a time.
    time_input: Entity<TextInput>,
    /// The "Link Parent Task" search field.
    link_search: Entity<TextInput>,
    /// The checklist is shown when it has items or the user asked to start one.
    show_checklist: bool,
    /// An input to focus on the next render (focusing needs the window).
    pending_focus: Option<FocusTarget>,
    popup: Option<Popup>,
    /// Shown as the calendar's task card, framed by the card instead of a left border.
    card: bool,
    /// The due-date popover's draft, while it is open.
    due_draft: Option<DueDraft>,
    /// The "Task Activities" panel, while it is open.
    activity: Option<Vec<fern_core::Activity>>,
    _subscriptions: Vec<Subscription>,
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

impl EventEmitter<TaskUpdated> for TaskDetail {}
impl EventEmitter<StartFocus> for TaskDetail {}
impl EventEmitter<Undoable> for TaskDetail {}
impl EventEmitter<OpenTask> for TaskDetail {}

impl TaskDetail {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let title = cx.new(|cx| TextInput::new(Mode::Wrapped, strings.untitled, cx));
        let notes = cx.new(|cx| TextInput::new(Mode::MultiLine, strings.notes_placeholder, cx));
        let tag_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.new_tag, cx));
        let new_item = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_item, cx));
        let subtask_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_subtask, cx));
        let time_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.time_placeholder, cx));
        let link_search = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.search_placeholder, cx));
        let comment_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.write_comment, cx));

        let subscriptions = vec![
            cx.subscribe(&title, |this, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                this.save(cx, |store, id| store.set_title(id, &text));
            }),
            // Enter in the title moves to the notes.
            cx.subscribe_in(&title, window, |this, _, _: &Submitted, window, cx| {
                window.focus(&this.notes.focus_handle(cx), cx);
            }),
            cx.subscribe(&notes, |this, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                this.save(cx, |store, id| store.set_notes(id, &text));
            }),
            cx.subscribe_in(&tag_input, window, |this, input, Submitted(text), window, cx| {
                let name = text.trim().trim_start_matches('#').to_owned();
                if !name.is_empty() {
                    this.add_tag(name, cx);
                }
                input.update(cx, |input, cx| input.clear(window, cx));
            }),
            cx.subscribe_in(&new_item, window, |this, input, Submitted(text), window, cx| {
                if !text.trim().is_empty() {
                    let text = text.clone();
                    this.save(cx, |store, id| store.add_checklist_item(id, &text, None).map(|_| ()));
                    input.update(cx, |input, cx| input.clear(window, cx));
                    this.sync_checklist(cx);
                }
            }),
            cx.subscribe_in(&subtask_input, window, |this, input, Submitted(text), window, cx| {
                if !text.trim().is_empty() {
                    let text = text.clone();
                    this.save(cx, |store, id| store.add_subtask(id, &text).map(|_| ()));
                    input.update(cx, |input, cx| input.clear(window, cx));
                    this.sync_subtasks();
                }
            }),
            cx.subscribe(&link_search, |_, _, _: &Changed, cx| cx.notify()),
            // Enter posts the comment; Escape closes the field.
            cx.subscribe_in(&comment_input, window, |this, input, Submitted(text), window, cx| {
                if let Some(task) = this.task_id()
                    && !text.trim().is_empty()
                {
                    match this.store.add_comment(task, text) {
                        Ok(_) => input.update(cx, |input, cx| input.clear(window, cx)),
                        Err(err) => eprintln!("failed to add the comment: {err:#}"),
                    }
                    this.sync_comments();
                    cx.notify();
                }
            }),
            cx.subscribe(&comment_input, |this, _, _: &Cancelled, cx| {
                this.comment_open = false;
                cx.notify();
            }),
            // A time typed by hand in the due-date popover's time menu.
            cx.subscribe_in(&time_input, window, |this, input, Submitted(text), window, cx| {
                if let Some(time) = due_picker::parse_time(text) {
                    this.update_draft(cx, |d| d.pick_time(Some(time), Local::now().date_naive()));
                    input.update(cx, |input, cx| input.clear(window, cx));
                }
            }),
            cx.observe_global::<CurrentLocale>(|this, cx| {
                let strings = i18n::t(cx);
                this.title.update(cx, |input, cx| input.set_placeholder(strings.untitled, cx));
                this.notes.update(cx, |input, cx| input.set_placeholder(strings.notes_placeholder, cx));
                this.tag_input.update(cx, |input, cx| input.set_placeholder(strings.new_tag, cx));
                this.new_item.update(cx, |input, cx| input.set_placeholder(strings.add_item, cx));
                this.subtask_input.update(cx, |input, cx| input.set_placeholder(strings.add_subtask, cx));
                this.time_input.update(cx, |input, cx| input.set_placeholder(strings.time_placeholder, cx));
            }),
        ];

        Self {
            store,
            focus_submenu: false,
            attachments: Vec::new(),
            comments: Vec::new(),
            comment_input,
            comment_open: false,
            comment_emoji: false,
            format_bar: false,
            format_headings: false,
            specific_month: None,
            estimate_draft: EstimateDraft { minutes: false, amount: 0 },
            task: None,
            title,
            notes,
            tag_input,
            all_tags: Vec::new(),
            lists: Vec::new(),
            checklist: Vec::new(),
            new_item,
            subtasks: Vec::new(),
            subtask_input,
            time_input,
            link_search,
            show_checklist: false,
            pending_focus: None,
            popup: None,
            card: false,
            due_draft: None,
            activity: None,
            _subscriptions: subscriptions,
        }
    }

    /// Makes this the calendar's task card, which draws its own frame.
    pub fn set_card(&mut self) {
        self.card = true;
    }

    /// Whether one of the details' popovers is open, which a click outside the card closes first.
    pub fn has_popup(&self) -> bool {
        self.popup.is_some()
    }

    pub fn task_id(&self) -> Option<Id> {
        self.task.as_ref().map(|t| t.id)
    }

    pub fn set_lists(&mut self, lists: Vec<List>) {
        self.lists = lists;
    }

    /// Shows a task, or the empty panel for `None`.
    pub fn show(&mut self, task: Option<Task>, cx: &mut Context<Self>) {
        let (title, notes) = task
            .as_ref()
            .map_or((String::new(), String::new()), |t| (t.title.clone(), t.notes.clone()));
        self.title.update(cx, |input, cx| input.set_text(title, cx));
        self.notes.update(cx, |input, cx| input.set_text(notes, cx));
        self.task = task;
        self.popup = None;
        self.activity = None;
        self.all_tags = self.store.tags().unwrap_or_default();
        self.checklist.clear();
        self.sync_checklist(cx);
        self.show_checklist = !self.checklist.is_empty();
        self.sync_subtasks();
        self.sync_attachments();
        self.sync_comments();
        cx.notify();
    }

    fn sync_comments(&mut self) {
        self.comments = match self.task_id() {
            Some(id) => self.store.comments(id).unwrap_or_default(),
            None => Vec::new(),
        };
    }

    /// The description toolbar's buttons: format the notes' selection as Markdown (or type the
    /// time), open the headings menu, or upload an attachment.
    fn toolbar_action(&mut self, action: ToolbarAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            ToolbarAction::ToggleHeadings => self.format_headings = !self.format_headings,
            ToolbarAction::Attach => return self.upload_attachment(cx),
            ToolbarAction::Format(format) => {
                self.format_headings = false;
                self.notes.update(cx, |input, cx| {
                    let selection = input.selection();
                    let (text, selection) = match format {
                        Some(format) => fern_core::markdown::apply(input.text(), selection, format),
                        None => fern_core::markdown::insert(input.text(), selection, &crate::calendar::format_time(Local::now().time())),
                    };
                    input.edit(text, selection, window, cx);
                });
                window.focus(&self.notes.focus_handle(cx), cx);
            }
        }
        cx.notify();
    }

    /// For the dev state: `detail-bar=format|comment|format-headings`, and a comment to show.
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
                    let _ = self.store.add_comment(task, &text["comment:".len()..]);
                    self.sync_comments();
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// The task's comments, as the original lists them under the task: the text, when it was
    /// written, and × on hover to delete it.
    fn render_comments(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .pt_3()
            .gap_2()
            .child(Label::new(format!("{} {}", strings.comments, self.comments.len())).size(LabelSize::Small).color(Color::Muted))
            .children(self.comments.iter().map(|comment| {
                let id = comment.id;
                let when = chrono::DateTime::from_timestamp_millis(comment.created_at)
                    .map(|t| t.with_timezone(&Local).naive_local())
                    .map(|t| format!("{} {}", crate::calendar::format_date(t.date(), strings), crate::calendar::format_time(t.time())))
                    .unwrap_or_default();
                h_flex()
                    .id(SharedString::from(format!("comment-{id}")))
                    .group("comment")
                    .items_start()
                    .gap_2()
                    .p_2()
                    .rounded(px(6.))
                    .bg(theme.colors().surface_background)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(Label::new(comment.text.clone()))
                            .child(Label::new(when).size(LabelSize::XSmall).color(Color::Muted)),
                    )
                    .child(
                        div().invisible().group_hover("comment", |s| s.visible()).child(
                            IconButton::new(SharedString::from(format!("comment-delete-{id}")), IconName::X)
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text(strings.delete))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let _ = this.store.delete_comment(id);
                                    this.sync_comments();
                                    cx.notify();
                                })),
                        ),
                    )
            }))
            .into_any_element()
    }

    /// The original's comment field over the bottom bar: "Write a comment" with an emoji button.
    fn render_comment_box(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let emoji_menu = self.comment_emoji.then(|| {
            let picks: Vec<_> = crate::emoji::search("").into_iter().take(40).map(|e| {
                div()
                    .id(SharedString::from(format!("comment-emoji-{e}")))
                    .size(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.colors().element_hover))
                    .child(div().text_size(px(18.)).child(e))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.comment_emoji = false;
                        this.comment_input.update(cx, |input, cx| {
                            let (text, selection) = fern_core::markdown::insert(input.text(), input.selection(), e);
                            input.edit(text, selection, window, cx);
                        });
                        window.focus(&this.comment_input.focus_handle(cx), cx);
                        cx.notify();
                    }))
            }).collect();
            PopoverAttachment::new(
                Placement::AboveRight,
                popover(cx)
                    .w(px(260.))
                    .p_2()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.comment_emoji = false;
                        cx.notify();
                    }))
                    .child(h_flex().flex_wrap().children(picks)),
            )
        });
        div()
            .px_5()
            .pb_2()
            .child(
                h_flex()
                    .h(px(36.))
                    .pl_3()
                    .pr_1()
                    .gap_1()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.colors().accent)
                    .bg(theme.colors().surface_background)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().flex_1().child(self.comment_input.clone()))
                    .child(
                        div().relative().child(
                            IconButton::new("comment-emoji", IconName::Smile)
                                .icon_color(Color::Muted)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.comment_emoji = !this.comment_emoji;
                                    cx.notify();
                                })),
                        )
                        .children(emoji_menu),
                    ),
            )
            .into_any_element()
    }

    fn sync_attachments(&mut self) {
        self.attachments = match self.task_id() {
            Some(id) => self.store.attachments(id).unwrap_or_default(),
            None => Vec::new(),
        };
    }

    /// "Upload Attachment": files picked in the system's dialog are copied to the app's folder.
    fn upload_attachment(&mut self, cx: &mut Context<Self>) {
        self.popup = None;
        let Some(task) = self.task_id() else { return };
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |this, cx| {
            let Some(paths) = chosen.await.ok().and_then(Result::ok).flatten() else { return };
            this.update(cx, |this, cx| {
                let added = crate::attachments_dir()
                    .and_then(|dir| paths.iter().try_for_each(|path| this.store.add_attachment(task, path, &dir).map(|_| ())));
                if let Err(err) = added {
                    eprintln!("failed to attach the file: {err:#}");
                }
                this.sync_attachments();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The attached files under the notes: a click opens one with the system's app.
    fn render_attachments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let dir = crate::attachments_dir().ok();
        v_flex().gap_1().children(self.attachments.iter().map(|attachment| {
            let path = dir.as_ref().map(|dir| attachment.path(dir));
            let removed = attachment.clone();
            h_flex()
                .id(SharedString::from(format!("attachment-{}", attachment.id)))
                .group("attachment")
                .h(px(36.))
                .px_2()
                .gap_2()
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border_variant)
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(Icon::new(IconName::Paperclip).size(IconSize::Small).color(Color::Muted))
                .child(div().flex_1().min_w_0().child(Label::new(attachment.name.clone()).size(LabelSize::Small).truncate()))
                .child(Label::new(format_size(attachment.size)).size(LabelSize::XSmall).color(Color::Muted))
                .child(
                    IconButton::new("attachment-delete", IconName::X).size(px(20.)).icon_size(IconSize::XSmall).on_click(
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let result = crate::attachments_dir().and_then(|dir| this.store.delete_attachment(&removed, &dir));
                            if let Err(err) = result {
                                eprintln!("failed to remove the attachment: {err:#}");
                            }
                            this.sync_attachments();
                            cx.notify();
                        }),
                    ),
                )
                .on_click(move |_, _, cx| {
                    if let Some(path) = &path {
                        cx.open_with_system(path);
                    }
                })
        }))
    }

    fn sync_subtasks(&mut self) {
        self.subtasks = match self.task_id() {
            Some(id) => self.store.subtasks(id).unwrap_or_default(),
            None => Vec::new(),
        };
    }

    fn toggle_subtask(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
        self.save(cx, |store, _| store.set_completed(id, completed));
        self.sync_subtasks();
        cx.notify();
    }

    /// Reloads checklist items, keeping the inputs of items that still exist so that the caret
    /// does not jump while the user types.
    fn sync_checklist(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.task_id() else {
            self.checklist.clear();
            return;
        };
        let items = self.store.checklist(task).unwrap_or_default();
        let mut old: Vec<ChecklistRow> = std::mem::take(&mut self.checklist);
        for item in items {
            let row = match old.iter().position(|row| row.item.id == item.id) {
                Some(index) => {
                    let mut row = old.swap_remove(index);
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
        let input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::Wrapped, "", cx);
            input.set_text(title, cx);
            input
        });
        let subscriptions = [
            cx.subscribe(&input, move |this, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                this.save(cx, |store, _| store.set_checklist_item_title(id, &text));
            }),
            // Enter adds the next item right below, as in TickTick.
            cx.subscribe(&input, move |this, _, _: &Submitted, cx| {
                let created = this
                    .task_id()
                    .and_then(|task| this.store.add_checklist_item(task, "", Some(id)).ok());
                if let Some(created) = created {
                    this.pending_focus = Some(FocusTarget::Item(created.id));
                    cx.emit(TaskUpdated);
                    this.sync_checklist(cx);
                }
            }),
        ];
        ChecklistRow { item, input, _subscriptions: subscriptions }
    }

    fn toggle_checklist_item(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
        self.save(cx, |store, _| store.set_checklist_item_completed(id, completed));
        self.sync_checklist(cx);
    }

    fn delete_checklist_item(&mut self, id: Id, cx: &mut Context<Self>) {
        self.save(cx, |store, _| store.delete_checklist_item(id));
        self.sync_checklist(cx);
    }

    fn start_checklist(&mut self, cx: &mut Context<Self>) {
        self.show_checklist = true;
        self.pending_focus = Some(FocusTarget::NewItem);
        cx.notify();
    }

    /// Updates the task data without touching the inputs, so the caret stays where it is.
    pub fn refresh(&mut self, task: Option<Task>, cx: &mut Context<Self>) {
        match task {
            Some(task) if self.task_id() == Some(task.id) => {
                self.task = Some(task);
                self.sync_checklist(cx);
                self.sync_subtasks();
                self.sync_attachments();
            }
            other => self.show(other, cx),
        }
    }

    fn save(&mut self, cx: &mut Context<Self>, write: impl FnOnce(&Store, Id) -> anyhow::Result<()>) {
        let Some(id) = self.task_id() else { return };
        if let Err(err) = write(&self.store, id) {
            eprintln!("failed to save the task: {err:#}");
            return;
        }
        cx.emit(TaskUpdated);
    }

    fn open_popup(&mut self, popup: Popup, cx: &mut Context<Self>) {
        self.focus_submenu = false;
        match popup {
            Popup::Due => {
                let today = Local::now().date_naive();
                self.due_draft = self.task.as_ref().map(|task| DueDraft::from_task(task, today));
            }
            Popup::Tags => self.all_tags = self.store.tags().unwrap_or_default(),
            Popup::Priority | Popup::List | Popup::More => {}
            Popup::LinkParent => self.link_search.update(cx, |input, cx| input.set_text("", cx)),
            Popup::Estimate => self.estimate_draft = EstimateDraft::from(self.task.as_ref().and_then(|t| t.estimate)),
        }
        self.popup = Some(popup);
        cx.notify();
    }

    /// Opens the due-date picker (the "Set date" shortcut).
    /// Settings → Task Default → Default Duration, in minutes.
    fn default_duration(&self) -> i64 {
        self.store.preferences().map_or(60, |p| p.task_defaults.duration)
    }

    /// The details' own pickers, for the task list's right-click menu: `subtask` focuses the Add
    /// Subtask field, `due`, `tags` and `link-parent` open their popovers.
    pub fn start(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.task.is_none() {
            return;
        }
        match what {
            "subtask" => window.focus(&self.subtask_input.focus_handle(cx), cx),
            "due" => self.open_popup(Popup::Due, cx),
            "tags" => self.open_popup(Popup::Tags, cx),
            "link-parent" => {
                self.open_popup(Popup::LinkParent, cx);
                window.focus(&self.link_search.focus_handle(cx), cx);
            }
            _ => {}
        }
    }

    pub fn open_due(&mut self, cx: &mut Context<Self>) {
        if self.task.is_some() {
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
        self.popup = None;
        self.focus_submenu = false;
        cx.notify();
    }

    fn update_draft(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut DueDraft)) {
        if let Some(draft) = self.due_draft.as_mut() {
            change(draft);
            cx.notify();
        }
    }

    /// "OK": saves the draft's date, time and repeat.
    fn apply_due(&mut self, cx: &mut Context<Self>) {
        if let Some(draft) = self.due_draft.take() {
            let today = Local::now().date_naive();
            self.save(cx, |store, id| {
                store.set_due(id, draft.date, draft.time)?;
                store.set_due_end(id, draft.end())?;
                store.set_reminders(id, if draft.date.is_some() { &draft.reminders } else { &[] })?;
                store.set_repeat(id, draft.repeat, today)
            });
        }
        self.close_popup(cx);
    }

    /// "Clear": removes the due date, time and repeat.
    fn clear_due(&mut self, cx: &mut Context<Self>) {
        self.due_draft = None;
        let today = Local::now().date_naive();
        self.save(cx, |store, id| {
            store.set_due(id, None, None)?;
            store.set_due_end(id, None)?;
            store.set_reminders(id, &[])?;
            store.set_repeat(id, None, today)
        });
        self.close_popup(cx);
    }

    fn set_priority(&mut self, priority: Priority, cx: &mut Context<Self>) {
        self.save(cx, |store, id| store.set_priority(id, priority));
        self.close_popup(cx);
    }

    fn set_list(&mut self, list: Option<Id>, cx: &mut Context<Self>) {
        self.save(cx, |store, id| store.set_list(id, list));
        self.close_popup(cx);
    }

    fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.update_draft(cx, |d| d.month = calendar::add_months(d.month, delta));
    }

    fn task_tags(&self) -> Vec<String> {
        self.task.as_ref().map(|t| t.tags.clone()).unwrap_or_default()
    }

    fn add_tag(&mut self, name: String, cx: &mut Context<Self>) {
        let mut tags = self.task_tags();
        if !tags.iter().any(|t| t.to_lowercase() == name.to_lowercase()) {
            tags.push(name);
            self.save(cx, |store, id| store.set_tags(id, &tags));
            self.all_tags = self.store.tags().unwrap_or_default();
        }
    }

    fn remove_tag(&mut self, name: &str, cx: &mut Context<Self>) {
        let tags: Vec<String> = self.task_tags().into_iter().filter(|t| t != name).collect();
        self.save(cx, |store, id| store.set_tags(id, &tags));
    }

    fn toggle_tag(&mut self, name: String, cx: &mut Context<Self>) {
        if self.task_tags().contains(&name) {
            self.remove_tag(&name, cx);
        } else {
            self.add_tag(name, cx);
        }
    }

    fn tag_color(&self, name: &str, cx: &App) -> Hsla {
        let color = self.all_tags.iter().find(|t| t.name == name).and_then(|t| t.color.as_deref());
        lists_panel::color(color, cx)
    }

    fn restore(&mut self, cx: &mut Context<Self>) {
        self.save(cx, |store, id| store.restore_task(id));
    }

    fn delete_forever(&mut self, cx: &mut Context<Self>) {
        self.save(cx, |store, id| store.purge_task(id));
    }

    fn menu_action(&mut self, cx: &mut Context<Self>, write: impl FnOnce(&Store, Id) -> anyhow::Result<()>) {
        self.close_popup(cx);
        self.save(cx, write);
    }

    fn toggle_wont_do(&mut self, cx: &mut Context<Self>) {
        let wont_do = self.task.as_ref().is_some_and(|t| t.is_wont_do());
        self.menu_action(cx, |store, id| store.set_wont_do(id, !wont_do));
    }

    fn toggle_kind(&mut self, cx: &mut Context<Self>) {
        let kind = match self.task.as_ref().map(|t| t.kind) {
            Some(Kind::Note) => Kind::Task,
            _ => Kind::Note,
        };
        self.menu_action(cx, |store, id| store.set_kind(id, kind));
    }

    /// The panel's "…" menu, a subset of the original's: the entries Fern supports so far.
    /// A "…" menu entry; hovering it closes the Start Focus side menu.
    fn close_side_menu_on_hover(&self, item: ListItem, cx: &mut Context<Self>) -> impl IntoElement {
        div().id(item.id()).child(item).on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if *hovered && this.focus_submenu {
                this.focus_submenu = false;
                cx.notify();
            }
        }))
    }

    fn render_more_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let Some(task) = self.task.as_ref() else { return div().into_any_element() };
        let wont_do = task.is_wont_do();
        let is_note = task.kind == Kind::Note;
        let item = |id: &'static str, icon: IconName, label: &'static str| {
            ListItem::new(id).start_slot(Icon::new(icon).color(Color::Muted)).child(Label::new(label))
        };
        popover(cx)
            .w(px(220.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(self.close_side_menu_on_hover(item("more-subtask", IconName::CornerDownRight, strings.add_subtask).on_click(cx.listener(
                |this, _, window, cx| {
                    this.close_popup(cx);
                    window.focus(&this.subtask_input.focus_handle(cx), cx);
                },
            )), cx))
            .child(self.close_side_menu_on_hover(item("more-link-parent", IconName::CornerDownRight, strings.link_parent).on_click(cx.listener(
                |this, _, window, cx| {
                    this.open_popup(Popup::LinkParent, cx);
                    window.focus(&this.link_search.focus_handle(cx), cx);
                },
            )), cx))
            .child(self.close_side_menu_on_hover({
                let pinned = task.pinned_at.is_some();
                item("more-pin", if pinned { IconName::PinOff } else { IconName::Pin }, if pinned { strings.unpin } else { strings.pin })
                    .on_click(cx.listener(move |this, _, _, cx| this.menu_action(cx, move |store, id| store.set_pinned(id, !pinned))))
            }, cx))
            .child(self.close_side_menu_on_hover(
                item("more-wont-do", if wont_do { IconName::RotateCcw } else { IconName::SquareX }, if wont_do {
                    strings.reopen
                } else {
                    strings.wont_do
                })
                .on_click(cx.listener(|this, _, _, cx| this.toggle_wont_do(cx))),
            cx,
            ))
            .child(self.close_side_menu_on_hover(item("more-tags", IconName::Tag, strings.tags).on_click(cx.listener(|this, _, _, cx| {
                this.open_popup(Popup::Tags, cx);
            })), cx))
            .child(self.close_side_menu_on_hover(
                item("more-attachment", IconName::Paperclip, strings.upload_attachment)
                    .on_click(cx.listener(|this, _, _, cx| this.upload_attachment(cx))),
                cx,
            ))
            .child(
                SubmenuItem::new("more-focus", strings.start_focus)
                    .icon(IconName::CircleDot)
                    .open(self.focus_submenu)
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered && !this.focus_submenu {
                            this.focus_submenu = true;
                            cx.notify();
                        }
                    }))
                    .items([(false, strings.start_pomo), (true, strings.start_stopwatch)].map(|(stopwatch, label)| {
                        ListItem::new(SharedString::from(format!("more-focus-{stopwatch}")))
                            .child(Label::new(label))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.close_popup(cx);
                                if let Some(task) = this.task.as_ref() {
                                    cx.emit(StartFocus(task.id, task.title.clone(), stopwatch));
                                }
                            }))
                    }))
                    .items([ListItem::new("more-focus-estimate")
                        .child(Label::new(strings.estimation))
                        .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Estimate, cx)))]),
            )
            .child(div().py_1().child(Divider::horizontal()))
            .child(self.close_side_menu_on_hover(item("more-activities", IconName::ListTree, strings.task_activities).on_click(cx.listener(
                |this, _, _, cx| {
                    this.close_popup(cx);
                    this.open_activity(cx);
                },
            )), cx))
            .child(self.close_side_menu_on_hover(item("more-template", IconName::LayoutTemplate, strings.save_as_template).on_click(cx.listener(
                |this, _, _, cx| this.menu_action(cx, |store, id| store.save_template(id).map(|_| ())),
            )), cx))
            .child(self.close_side_menu_on_hover(item("more-copy-link", IconName::Link, strings.copy_link).on_click(cx.listener(|this, _, _, cx| {
                this.close_popup(cx);
                if let Some(task) = this.task.as_ref() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(fern_core::link::task_link(task.id)));
                }
            })), cx))
            .child(self.close_side_menu_on_hover(item("more-duplicate", IconName::Copy, strings.duplicate).on_click(cx.listener(
                |this, _, _, cx| this.menu_action(cx, |store, id| store.duplicate_task(id).map(|_| ())),
            )), cx))
            .child(self.close_side_menu_on_hover(
                item(
                    "more-convert",
                    if is_note { IconName::SquareCheck } else { IconName::FileText },
                    if is_note { strings.convert_to_task } else { strings.convert_to_note },
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_kind(cx))),
            cx,
            ))
            .child(div().py_1().child(Divider::horizontal()))
            .child(self.close_side_menu_on_hover(item("more-print", IconName::Printer, strings.print).on_click(cx.listener(|this, _, _, cx| {
                this.close_popup(cx);
                if let Some(task) = this.task.clone() {
                    let checklist = this.store.checklist(task.id).unwrap_or_default();
                    let html = crate::print::task_html(&task, &checklist, Local::now().date_naive(), i18n::t(cx));
                    if let Err(err) = crate::print::open("task", &html) {
                        eprintln!("failed to print: {err}");
                    }
                }
            })), cx))
            .child(self.close_side_menu_on_hover(item("more-delete", IconName::Trash, strings.delete).on_click(cx.listener(|this, _, _, cx| {
                let task = this.task.as_ref().map(|t| (t.id, t.title.clone()));
                this.menu_action(cx, |store, id| store.delete_task(id));
                if let Some((id, title)) = task {
                    let label = i18n::t(cx).undo_deleted.replace("{title}", &title);
                    cx.emit(Undoable(crate::undo::UndoAction::Restore(id), label));
                }
            })), cx))
            .into_any_element()
    }

    fn open_activity(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.task.as_ref() else { return };
        let mut activity = self.store.task_activity(task.id).unwrap_or_default();
        // Tasks from before activity was recorded still show when they were created.
        if activity.first().is_none_or(|a| a.kind != "created") {
            activity.insert(0, fern_core::Activity { at: task.created_at, kind: "created".into() });
        }
        self.activity = Some(activity);
        cx.notify();
    }

    /// The "Task Activities" panel at the bottom of the details, as in the original.
    fn render_activity(&self, activity: &[fern_core::Activity], strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();

        v_flex()
            .id("task-activity")
            .max_h(px(240.))
            .overflow_y_scroll()
            .px_5()
            .py_3()
            .gap_2()
            .border_t_1()
            .border_color(theme.colors().border)
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.task_activities).weight(FontWeight::SEMIBOLD)))
                    .child(IconButton::new("close-activity", IconName::X).on_click(cx.listener(|this, _, _, cx| {
                        this.activity = None;
                        cx.notify();
                    }))),
            )
            .children(activity.iter().rev().map(|entry| {
                let text = activity_text(&entry.kind, strings);
                let when = chrono::Local
                    .timestamp_millis_opt(entry.at)
                    .single()
                    .map(|t| format!("{} {}", calendar::format_day_month(t.date_naive(), strings), calendar::format_time(t)))
                    .unwrap_or_default();
                h_flex()
                    .items_start()
                    .gap_2()
                    .child(div().mt(px(5.)).size(px(8.)).flex_none().rounded_full().bg(theme.colors().accent))
                    .child(
                        v_flex()
                            .child(Label::new(text).size(LabelSize::Small))
                            .child(Label::new(when).size(LabelSize::XSmall).color(Color::Muted)),
                    )
            }))
            .into_any_element()
    }

    /// The "Link Parent Task" picker: open tasks other than this one and its subtasks, filtered by
    /// the search field, each with its list.
    /// The original's Estimation popover: Estimated Pomo or Estimated Duration, with Cancel / OK.
    fn render_estimate(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let draft = self.estimate_draft;
        let theme = cx.theme().clone();
        let kind = |minutes: bool, label: &'static str| {
            div()
                .id(if minutes { "estimate-duration" } else { "estimate-pomo" })
                .flex_1()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(draft.minutes == minutes, |el| el.bg(theme.colors().element_selected))
                .child(Label::new(label).size(LabelSize::Small).color(if draft.minutes == minutes { Color::Default } else { Color::Muted }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.estimate_draft.minutes != minutes {
                        this.estimate_draft = EstimateDraft { minutes, amount: 0 };
                        cx.notify();
                    }
                }))
        };
        let step = |id: &'static str, icon: IconName, up: bool| {
            IconButton::new(id, icon).size(px(24.)).icon_size(IconSize::Small).on_click(cx.listener(move |this, _, _, cx| {
                this.estimate_draft.step(up);
                cx.notify();
            }))
        };
        let value = if draft.minutes { fern_core::format_minutes(draft.amount as i64) } else { draft.amount.to_string() };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |l| l.color(Color::Custom(gpui::white()))))
        };
        popover(cx)
            .w(px(280.))
            .p_3()
            .gap_3()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(
                h_flex()
                    .p_0p5()
                    .rounded(px(8.))
                    .bg(theme.colors().surface_background)
                    .child(kind(false, strings.estimated_pomo))
                    .child(kind(true, strings.estimated_duration)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(step("estimate-less", IconName::Minus, false))
                    .child(
                        div()
                            .min_w(px(64.))
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .child(Label::new(value)),
                    )
                    .child(step("estimate-more", IconName::Plus, true))
                    .when(!draft.minutes, |el| el.child(Label::new(strings.pomo_unit).color(Color::Muted))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(button("estimate-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| this.close_popup(cx))))
                    .child(button("estimate-ok", strings.ok, true).on_click(cx.listener(|this, _, _, cx| {
                        let estimate = this.estimate_draft.estimate();
                        this.menu_action(cx, move |store, id| store.set_estimate(id, estimate));
                    }))),
            )
            .into_any_element()
    }

    fn render_link_parent(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(task) = self.task.as_ref() else { return div().into_any_element() };
        let query = self.link_search.read(cx).text().trim().to_lowercase();
        let excluded: std::collections::HashSet<Id> =
            self.store.descendants(task.id).unwrap_or_default().into_iter().chain([task.id]).collect();
        let candidates: Vec<Task> = self
            .store
            .tasks()
            .unwrap_or_default()
            .into_iter()
            .filter(|t| !t.is_closed() && !t.is_deleted() && !excluded.contains(&t.id) && task.parent_id != Some(t.id))
            .filter(|t| query.is_empty() || t.title.to_lowercase().contains(&query))
            .take(30)
            .collect();
        popover(cx)
            .w(px(340.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(
                h_flex()
                    .h(px(34.))
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.colors().border)
                    .child(Icon::new(IconName::Search).color(Color::Muted))
                    .child(div().flex_1().child(self.link_search.clone())),
            )
            .child(v_flex().id("link-parent-list").max_h(px(320.)).overflow_y_scroll().children(candidates.into_iter().map(|candidate| {
                let parent = candidate.id;
                let list: SharedString = candidate
                    .list_id
                    .and_then(|id| self.lists.iter().find(|l| l.id == id))
                    .map_or(strings.inbox.into(), |l| l.name.clone().into());
                ListItem::new(SharedString::from(format!("link-{parent}")))
                    .start_slot(Icon::new(IconName::Circle).size(IconSize::Small).color(Color::Muted))
                    .child(Label::new(candidate.title.clone()).truncate())
                    .end_slot(Label::new(list).size(LabelSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.close_popup(cx);
                        this.save(cx, |store, id| store.link_parent(id, parent));
                    }))
            })))
            .into_any_element()
    }

    fn toggle_completed(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.task.as_mut() else { return };
        let completed = !task.is_completed();
        let mut undo = None;
        // Unchecking a "Won't Do" task reopens it.
        let result = if task.is_wont_do() {
            self.store.set_wont_do(task.id, false)
        } else if completed {
            crate::undo::complete(&self.store, task).map(|action| undo = Some(action))
        } else {
            self.store.set_completed(task.id, false)
        };
        if let Err(err) = result {
            eprintln!("failed to update the task: {err:#}");
            return;
        }
        if let Some(action) = undo {
            let label = i18n::t(cx).undo_completed.replace("{title}", &task.title);
            cx.emit(Undoable(action, label));
        }
        if let Ok(Some(fresh)) = self.store.task(task.id) {
            *task = fresh;
        }
        cx.emit(TaskUpdated);
        cx.notify();
    }

    fn render_checklist(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(self.checklist.iter().map(|row| {
                let id = row.item.id;
                let done = row.item.completed;
                h_flex()
                    .id(SharedString::from(format!("item-{id}")))
                    .group("checklist-item")
                    .min_h(px(30.))
                    .items_start()
                    .gap_2()
                    .py_1()
                    .child(
                        div().mt(px(3.)).child(
                            Checkbox::new("item-checkbox")
                                .size(px(14.))
                                .toggle_state(done)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_checklist_item(id, !done, cx)
                                })),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(done, |el| el.text_color(theme.colors().text_muted))
                            .child(row.input.clone()),
                    )
                    .child(
                        div()
                            .id("item-delete")
                            .flex_none()
                            .px_1()
                            .text_color(theme.colors().text_muted)
                            .opacity(0.)
                            .group_hover("checklist-item", |s| s.opacity(1.))
                            .hover(|s| s.text_color(theme.status().error))
                            .cursor_pointer()
                            .child("✕")
                            .on_click(cx.listener(move |this, _, _, cx| this.delete_checklist_item(id, cx))),
                    )
            }))
            .child(
                h_flex()
                    .min_h(px(30.))
                    .gap_2()
                    .child(Icon::new(IconName::Plus).size(IconSize::Small))
                    .child(div().flex_1().child(self.new_item.clone())),
            )
    }

    fn render_subtasks(&self, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(self.subtasks.iter().map(|subtask| {
                let id = subtask.id;
                let done = subtask.is_completed();
                ListItem::new(SharedString::from(format!("subtask-{id}")))
                    .height(px(30.))
                    .start_slot(
                        Checkbox::new("subtask-checkbox")
                            .size(px(14.))
                            .toggle_state(done)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_subtask(id, !done, cx);
                            })),
                    )
                    .child(
                        Label::new(if subtask.title.is_empty() {
                            SharedString::new_static(strings.untitled)
                        } else {
                            subtask.title.clone().into()
                        })
                        .color(if done { Color::Muted } else { Color::Default })
                        .strikethrough(done)
                        .truncate(),
                    )
                    .end_slot(Icon::new(IconName::ChevronRight).size(IconSize::XSmall))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenTask(id))))
            }))
            .child(
                h_flex()
                    .min_h(px(30.))
                    .px_2()
                    .gap_2()
                    .child(Icon::new(IconName::Plus).size(IconSize::Small))
                    .child(div().flex_1().child(self.subtask_input.clone())),
            )
    }

    /// The Duration tab: start and end with day and time, "All Day", and a calendar or time list
    /// for the field being picked.
    fn render_duration(
        &self,
        draft: &DueDraft,
        calendar_block: AnyElement,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let all_day = draft.is_all_day();
        let chip = |id: &'static str, label: SharedString, field: RangeField, cx: &mut Context<Self>| {
            let active = draft.range_field == Some(field);
            div()
                .id(id)
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .border_1()
                .border_color(if active { theme.colors().accent } else { theme.colors().border })
                .cursor_pointer()
                .child(Label::new(label).size(LabelSize::Small))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_draft(cx, |d| {
                        d.range_field = if d.range_field == Some(field) { None } else { Some(field) };
                        let shown = if field == RangeField::EndDate { d.end_date } else { d.date };
                        if let Some(date) = shown {
                            d.month = calendar::month_start(date);
                        }
                    })
                }))
        };
        let day = |date: Option<NaiveDate>| -> SharedString {
            date.map_or(SharedString::default(), |d| task_rows::short_date(d, strings).into())
        };
        let time = |t: Option<chrono::NaiveTime>| -> SharedString { t.map_or(SharedString::default(), |t| crate::calendar::format_time(t).into()) };
        let line = |label: &'static str, date_chip: AnyElement, time_chip: Option<AnyElement>| {
            h_flex()
                .gap_2()
                .child(div().w(px(40.)).child(Label::new(label).size(LabelSize::Small)))
                .child(date_chip)
                .children(time_chip)
        };
        let start_date = chip("range-start-date", day(draft.date), RangeField::StartDate, cx).into_any_element();
        let start_time = (!all_day).then(|| chip("range-start-time", time(draft.time), RangeField::StartTime, cx).into_any_element());
        let end_date = chip("range-end-date", day(draft.end_date), RangeField::EndDate, cx).into_any_element();
        let end_time = (!all_day).then(|| chip("range-end-time", time(draft.end_time), RangeField::EndTime, cx).into_any_element());
        let picker: Option<AnyElement> = match draft.range_field {
            Some(RangeField::StartDate | RangeField::EndDate) => Some(calendar_block),
            Some(field @ (RangeField::StartTime | RangeField::EndTime)) => {
                let current = if field == RangeField::EndTime { draft.end_time } else { draft.time };
                Some(
                    v_flex()
                        .id("range-times")
                        .max_h(px(180.))
                        .overflow_y_scroll()
                        .children(due_picker::time_options().into_iter().map(|t| {
                            ListItem::new(SharedString::from(format!("range-time-{t}")))
                                .height(px(28.))
                                .child(Label::new(crate::calendar::format_time(t)).size(LabelSize::Small))
                                .when(current == Some(t), |item| item.end_slot(check_mark()))
                                .on_click(cx.listener(move |this, _, _, cx| this.update_draft(cx, |d| d.pick_range_time(t))))
                        }))
                        .into_any_element(),
                )
            }
            None => None,
        };
        v_flex()
            .gap_2()
            .child(line(strings.range_start, start_date, start_time))
            .child(line(strings.range_end, end_date, end_time))
            .children(picker)
            .child(
                h_flex()
                    .id("range-all-day")
                    .cursor_pointer()
                    .child(div().flex_1().child(Label::new(strings.range_all_day).size(LabelSize::Small)))
                    .child(Switch::new().toggle_state(all_day))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_draft(cx, |d| d.set_all_day(!all_day)))),
            )
            .child(Divider::horizontal())
            .into_any_element()
    }

    fn render_due_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let Some(draft) = self.due_draft.clone() else { return div().into_any_element() };
        let theme = cx.theme().clone();
        let locale = i18n::locale(cx);
        let today = Local::now().date_naive();
        let month = draft.month;
        let [d_today, d_tomorrow, d_week, d_month] = due_picker::quick_dates(today);

        let quick = |id: &'static str, icon: IconName, tip: &'static str, date: NaiveDate, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex_1()
                .h(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .tooltip(Tooltip::text(tip))
                .child(Icon::new(icon).size(IconSize::XLarge))
                .on_click(cx.listener(move |this, _, _, cx| this.update_draft(cx, |d| d.pick_date(date))))
        };

        let weeks: Vec<AnyElement> = calendar::month_grid(month)
            .chunks(7)
            .map(|week| {
                h_flex()
                    .children(week.iter().map(|&date| {
                        let in_month = date.month() == month.month();
                        let picked = if draft.range_field == Some(RangeField::EndDate) { draft.end_date } else { draft.date };
                        let is_selected = picked == Some(date);
                        let duration = draft.tab == DueTab::Duration;
                        let is_today = date == today;
                        div().flex_1().flex().justify_center().child(
                            div()
                                .id(SharedString::from(format!("day-{date}")))
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .cursor_pointer()
                                .when(is_selected, |el| el.bg(theme.colors().accent))
                                .when(!is_selected && is_today, |el| el.bg(theme.colors().accent.opacity(0.18)))
                                .when(!is_selected, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                                .child(Label::new(date.day().to_string()).size(LabelSize::Small).color(
                                    if is_selected {
                                        Color::Default
                                    } else if is_today {
                                        Color::Accent
                                    } else if in_month {
                                        Color::Default
                                    } else {
                                        Color::Muted
                                    },
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_draft(cx, |d| if duration { d.pick_range_date(date) } else { d.pick_date(date) })
                                })),
                        )
                    }))
                    .into_any_element()
            })
            .collect();

        let row = |id: &'static str, icon: IconName, label: SharedString, active: bool, menu: DueMenu, cx: &mut Context<Self>| {
            let color = if active { Color::Accent } else { Color::Default };
            h_flex()
                .id(id)
                .h(px(32.))
                .px_1()
                .gap_2()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(Icon::new(icon).color(if active { Color::Accent } else { Color::Muted }))
                .child(div().flex_1().child(Label::new(label).size(LabelSize::Small).color(color)))
                .child(
                    Icon::new(if draft.menu.as_ref() == Some(&menu) || (menu == DueMenu::Repeat && matches!(draft.menu, Some(DueMenu::Custom(_))))
                        || (menu == DueMenu::Reminder && matches!(draft.menu, Some(DueMenu::CustomReminder(_)))) {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                        .size(IconSize::XSmall),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_draft(cx, |d| d.menu = if d.menu.as_ref() == Some(&menu) { None } else { Some(menu.clone()) })
                }))
        };
        let time_label: SharedString =
            draft.time.map_or(strings.time.into(), |t| crate::calendar::format_time(t).into());
        let repeat_label: SharedString =
            draft.repeat.as_ref().map_or(strings.repeat.into(), |r| due_picker::repeat_label(r, strings).into());

        let menu: Option<AnyElement> = match &draft.menu {
            Some(DueMenu::Time) => Some(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .h(px(28.))
                            .px_2()
                            .rounded(px(6.))
                            .bg(theme.colors().background)
                            .text_size(px(12.))
                            .child(self.time_input.clone()),
                    )
                    .child(v_flex()
                    .id("time-options")
                    .max_h(px(180.))
                    .overflow_y_scroll()
                    .children(
                        std::iter::once(None)
                            .chain(due_picker::time_options().into_iter().map(Some))
                            .map(|time| {
                                let label: SharedString = time.map_or(strings.no_time.into(), |t| crate::calendar::format_time(t).into());
                                ListItem::new(SharedString::from(format!("time-{label}")))
                                    .height(px(28.))
                                    .child(Label::new(label).size(LabelSize::Small))
                                    .when(draft.time == time, |item| item.end_slot(check_mark()))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_draft(cx, |d| d.pick_time(time, Local::now().date_naive()))
                                    }))
                            })
                            .collect::<Vec<_>>(),
                    ))
                    .into_any_element(),
            ),
            Some(DueMenu::Custom(rule)) => Some(self.render_custom_repeat(rule.clone(), strings, cx)),
            Some(DueMenu::Reminder) => Some(
                v_flex()
                    .children(due_picker::reminder_options(draft.time.is_some(), strings).into_iter().map(
                        |(minutes, label, hint)| {
                            ListItem::new(SharedString::from(format!("reminder-{minutes}")))
                                .height(px(28.))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(Label::new(label).size(LabelSize::Small))
                                        .when(!hint.is_empty(), |el| {
                                            el.child(Label::new(format!("({hint})")).size(LabelSize::Small).color(Color::Muted))
                                        }),
                                )
                                .when(draft.reminders.contains(&minutes), |item| item.end_slot(check_mark()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_draft(cx, |d| d.toggle_reminder(minutes, Local::now().date_naive()))
                                }))
                        },
                    ))
                    // Custom reminders already chosen, so they can be turned off again.
                    .children(
                        draft
                            .reminders
                            .iter()
                            .filter(|m| !due_picker::reminder_options(draft.time.is_some(), strings).iter().any(|(o, _, _)| o == *m))
                            .map(|&minutes| {
                                ListItem::new(SharedString::from(format!("reminder-custom-{minutes}")))
                                    .height(px(28.))
                                    .child(
                                        Label::new(due_picker::describe_reminder(minutes, draft.time.is_some(), strings))
                                            .size(LabelSize::Small),
                                    )
                                    .end_slot(check_mark())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_draft(cx, |d| d.toggle_reminder(minutes, Local::now().date_naive()))
                                    }))
                            })
                            .collect::<Vec<_>>(),
                    )
                    .child(div().py_1().child(Divider::horizontal()))
                    .child(
                        ListItem::new("reminder-custom")
                            .height(px(28.))
                            .child(Label::new(strings.repeat_custom).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_draft(cx, |d| {
                                    d.menu = Some(DueMenu::CustomReminder(CustomReminder::new(d.time.is_some())))
                                })
                            })),
                    )
                    .into_any_element(),
            ),
            Some(DueMenu::CustomReminder(custom)) => Some(self.render_custom_reminder(*custom, &draft, strings, cx)),
            Some(DueMenu::Repeat) => {
                let base = draft.date.unwrap_or(today);
                let options = due_picker::repeat_options(base, strings, locale);
                Some(
                    v_flex()
                        .child(
                            ListItem::new("repeat-none")
                                .height(px(28.))
                                .child(Label::new(strings.repeat_none).size(LabelSize::Small))
                                .when(draft.repeat.is_none(), |item| item.end_slot(check_mark()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_draft(cx, |d| d.pick_repeat(None, Local::now().date_naive()))
                                })),
                        )
                        .children(options.into_iter().map(|(rule, label, hint)| {
                            ListItem::new(SharedString::from(format!("repeat-{rule}")))
                                .height(px(28.))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(Label::new(label).size(LabelSize::Small))
                                        .when(!hint.is_empty(), |el| {
                                            el.child(Label::new(format!("({hint})")).size(LabelSize::Small).color(Color::Muted))
                                        }),
                                )
                                .when(draft.repeat.as_ref() == Some(&rule), |item| item.end_slot(check_mark()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_draft(cx, |d| d.pick_repeat(Some(rule.clone()), Local::now().date_naive()))
                                }))
                        }))
                        .child(div().py_1().child(Divider::horizontal()))
                        .child(
                            ListItem::new("repeat-custom")
                                .height(px(28.))
                                .child(Label::new(strings.repeat_custom).size(LabelSize::Small))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let today = Local::now().date_naive();
                                    this.update_draft(cx, |d| {
                                        d.menu = Some(DueMenu::Custom(due_picker::custom_start(d.repeat.clone(), d.date.unwrap_or(today))))
                                    })
                                })),
                        )
                        .into_any_element(),
                )
            }
            None => None,
        };
        let (time_menu, reminder_menu, repeat_menu) = match &draft.menu {
            Some(DueMenu::Time) => (menu, None, None),
            Some(DueMenu::Reminder | DueMenu::CustomReminder(_)) => (None, menu, None),
            Some(DueMenu::Repeat | DueMenu::Custom(_)) => (None, None, menu),
            None => (None, None, None),
        };
        let reminder_label: SharedString = due_picker::reminder_label(&draft.reminders, draft.time.is_some(), strings)
            .map_or(strings.reminder.into(), Into::into);

        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small))
        };

        let quick_row = h_flex()
                    .child(quick("due-today", IconName::Sun, strings.today, d_today, cx))
                    .child(quick("due-tomorrow", IconName::Sunrise, strings.tomorrow, d_tomorrow, cx))
                    .child(quick("due-week", IconName::CalendarPlus, strings.next_week, d_week, cx))
                    .child(quick("due-month", IconName::Moon, strings.next_month, d_month, cx));
        let calendar_block = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .px_1()
                    .child(div().flex_1().child(Label::new(calendar::month_title(month, strings)).weight(FontWeight::SEMIBOLD)))
                    .child(
                        IconButton::new("prev-month", IconName::ChevronLeft)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.shift_month(-1, cx))),
                    )
                    .child(
                        IconButton::new("this-month", IconName::Circle)
                            .size(px(22.))
                            .icon_size(IconSize::XSmall)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let today = Local::now().date_naive();
                                this.update_draft(cx, |d| d.month = calendar::month_start(today))
                            })),
                    )
                    .child(
                        IconButton::new("next-month", IconName::ChevronRight)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.shift_month(1, cx))),
                    ),
            )
            .child(h_flex().children(crate::calendar::weekday_labels(&strings.weekdays_narrow).iter().map(|day| {
                div().flex_1().flex().justify_center().child(Label::new(*day).size(LabelSize::XSmall).color(Color::Muted))
            })))
            .child(v_flex().gap_0p5().children(weeks))
            .into_any_element();
        popover(cx)
            .w(px(260.))
            .p_2()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(h_flex().p_0p5().rounded(px(6.)).bg(theme.colors().background).children(
                [(DueTab::Date, "due-tab-date", strings.date_tab), (DueTab::Duration, "due-tab-duration", strings.duration_tab)].map(
                    |(tab, id, label)| {
                        div()
                            .id(id)
                            .flex_1()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .cursor_pointer()
                            .when(draft.tab == tab, |el| el.bg(theme.colors().element_selected))
                            .child(Label::new(label).size(LabelSize::Small).color(if draft.tab == tab { Color::Default } else { Color::Muted }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let duration = this.default_duration();
                                this.update_draft(cx, |d| match tab {
                                    DueTab::Duration => d.start_duration(Local::now().naive_local(), duration),
                                    DueTab::Date => d.tab = DueTab::Date,
                                })
                            }))
                    },
                ),
            ))
            .map(|el| match draft.tab {
                DueTab::Date => el
                    .child(quick_row)
                    .child(calendar_block)
                    .child(Divider::horizontal())
                    .child(row("due-time-row", IconName::Clock, time_label, draft.time.is_some(), DueMenu::Time, cx))
                    .children(time_menu),
                DueTab::Duration => el.child(self.render_duration(&draft, calendar_block, strings, cx)),
            })
            .child(row(
                "due-reminder-row",
                IconName::AlarmClock,
                reminder_label,
                !draft.reminders.is_empty(),
                DueMenu::Reminder,
                cx,
            ))
            .children(reminder_menu)
            .child(row("due-repeat-row", IconName::Repeat, repeat_label, draft.repeat.is_some(), DueMenu::Repeat, cx))
            .children(repeat_menu)
            .child(
                h_flex()
                    .gap_2()
                    .pt_1()
                    .child(button("due-clear", strings.clear, false).on_click(cx.listener(|this, _, _, cx| this.clear_due(cx))))
                    .child(button("due-ok", strings.ok, true).on_click(cx.listener(|this, _, _, cx| this.apply_due(cx)))),
            )
            .into_any_element()
    }

    fn update_custom_reminder(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut CustomReminder)) {
        self.update_draft(cx, |d| {
            if let Some(DueMenu::CustomReminder(custom)) = d.menu.as_mut() {
                change(custom);
            }
        })
    }

    /// The custom reminder editor, as in the original: unit, amount in advance, the time of day
    /// for all-day tasks, and when it will fire.
    fn render_custom_reminder(
        &self,
        custom: CustomReminder,
        draft: &DueDraft,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let has_time = draft.time.is_some();
        let units: &[ReminderUnit] = if has_time { &ReminderUnit::TIMED } else { &ReminderUnit::ALL_DAY };
        let offset = custom.offset(has_time);
        let fires = draft.date.map(|date| {
            let base = draft.time.unwrap_or_else(|| {
                chrono::NaiveTime::from_hms_opt(fern_core::reminder::ALL_DAY_REMINDER_HOUR, 0, 0).unwrap_or_default()
            });
            date.and_time(base) - chrono::Duration::minutes(offset)
        });
        let summary = fires.map(|at| {
            strings
                .reminder_summary
                .replace("{time}", &crate::calendar::format_time(at))
                .replace("{date}", &format!("{}, {}", task_rows::short_date(at.date(), strings), at.year()))
        });
        let stepper_row = |label: &'static str, value: String, minus: AnyElement, plus: AnyElement| {
            h_flex()
                .gap_2()
                .child(div().flex_1().child(Label::new(label).size(LabelSize::Small)))
                .child(minus)
                .child(div().min_w(px(44.)).flex().justify_center().child(Label::new(value).size(LabelSize::Small)))
                .child(plus)
        };
        let small = |id: &'static str, icon: IconName| IconButton::new(id, icon).size(px(22.)).icon_size(IconSize::XSmall);
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small))
        };
        v_flex()
            .gap_2()
            .p_2()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().border)
            .child(h_flex().gap_1().children(units.iter().map(|&unit| {
                let selected = unit == custom.unit;
                div()
                    .id(SharedString::from(format!("reminder-unit-{unit:?}")))
                    .flex_1()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(selected, |el| el.bg(theme.colors().element_selected))
                    .hover(|s| s.bg(theme.colors().element_hover))
                    .child(Label::new(unit.label(strings)).size(LabelSize::XSmall))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_custom_reminder(cx, |c| c.unit = unit)))
            })))
            .child(stepper_row(
                strings.reminder_in_advance,
                custom.amount.to_string(),
                small("reminder-fewer", IconName::Minus)
                    .on_click(cx.listener(|this, _, _, cx| this.update_custom_reminder(cx, |c| c.amount = c.amount.saturating_sub(1))))
                    .into_any_element(),
                small("reminder-more", IconName::Plus)
                    .on_click(cx.listener(|this, _, _, cx| this.update_custom_reminder(cx, |c| c.amount = (c.amount + 1).min(999))))
                    .into_any_element(),
            ))
            .when(!has_time, |el| {
                el.child(stepper_row(
                    strings.reminder_at,
                    crate::calendar::format_time(custom.at),
                    small("reminder-earlier", IconName::Minus)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update_custom_reminder(cx, |c| c.at = c.at - chrono::Duration::minutes(30))
                        }))
                        .into_any_element(),
                    small("reminder-later", IconName::Plus)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update_custom_reminder(cx, |c| c.at = c.at + chrono::Duration::minutes(30))
                        }))
                        .into_any_element(),
                ))
            })
            .child(Label::new(due_picker::describe_reminder(offset, has_time, strings)).size(LabelSize::XSmall).color(Color::Muted))
            .when_some(summary, |el, summary| el.child(Label::new(summary).size(LabelSize::XSmall).color(Color::Accent)))
            .child(
                h_flex()
                    .gap_2()
                    .child(button("reminder-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| {
                        this.update_draft(cx, |d| d.menu = Some(DueMenu::Reminder))
                    })))
                    .child(button("reminder-ok", strings.ok, true).on_click(cx.listener(move |this, _, _, cx| {
                        this.update_draft(cx, |d| d.add_reminder(offset, Local::now().date_naive()))
                    }))),
            )
            .into_any_element()
    }

    /// "Custom" repeat: every N days, weeks, months or years, and the weekdays for weekly rules.
    fn render_custom_repeat(&self, rule: Recurrence, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let unit = due_picker::unit_index(rule.frequency);
        let units = h_flex().gap_1().children(due_picker::UNITS.into_iter().enumerate().map(|(i, frequency)| {
            let selected = i == unit;
            div()
                .id(SharedString::from(format!("repeat-unit-{i}")))
                .flex_1()
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.colors().element_selected))
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(Label::new(strings.repeat_units[i]).size(LabelSize::XSmall))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_draft(cx, |d| {
                        if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                            rule.frequency = frequency;
                            if frequency != Frequency::Weekly {
                                rule.weekdays = 0;
                            }
                            if frequency != Frequency::Monthly {
                                rule.month_days = 0;
                                rule.nth_weekday = None;
                                rule.nth_workday = None;
                            }
                            if frequency != Frequency::Daily {
                                rule.skip_weekends = false;
                            }
                        }
                    })
                }))
        }));
        let weekdays = h_flex().justify_between().children((0..7u8).map(|i| {
            let on = rule.weekdays & (1 << i) != 0;
            div()
                .id(SharedString::from(format!("repeat-day-{i}")))
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .when(on, |el| el.bg(theme.colors().accent))
                .when(!on, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(strings.weekdays_narrow[i as usize]).size(LabelSize::XSmall))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_draft(cx, |d| {
                        if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                            rule.weekdays ^= 1 << i;
                        }
                    })
                }))
        }));
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small))
        };
        // The original's "By Due Dates" / "By Completion Date"; counting from completion drops
        // the weekdays and days of the month, as there.
        let mode = div()
            .id("repeat-mode")
            .h(px(26.))
            .px_2()
            .flex()
            .items_center()
            .justify_between()
            .rounded(px(5.))
            .border_1()
            .border_color(theme.colors().border)
            .cursor_pointer()
            .child(
                Label::new(if rule.is_specific_dates() {
                    strings.repeat_by_specific
                } else if rule.from_completion {
                    strings.repeat_by_completion
                } else {
                    strings.repeat_by_due
                })
                .size(LabelSize::Small),
            )
            .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
            // Goes through the original's three modes: due dates, completion date, specific dates.
            .on_click(cx.listener(|this, _, _, cx| {
                let today = Local::now().date_naive();
                this.update_draft(cx, |d| {
                    let start = d.date.unwrap_or(today);
                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                        if rule.is_specific_dates() {
                            rule.dates.clear();
                        } else if rule.from_completion {
                            rule.from_completion = false;
                            rule.dates = vec![start];
                        } else {
                            rule.from_completion = true;
                            rule.weekdays = 0;
                            rule.month_days = 0;
                            rule.nth_weekday = None;
                            rule.nth_workday = None;
                            rule.skip_weekends = false;
                        }
                    }
                })
            }));
        let skip_weekends = h_flex()
            .id("repeat-skip-weekends")
            .gap_2()
            .cursor_pointer()
            .child(Checkbox::new("repeat-skip-weekends-box").size(px(14.)).toggle_state(rule.skip_weekends))
            .child(Label::new(strings.repeat_skip_weekends).size(LabelSize::Small))
            .on_click(cx.listener(|this, _, _, cx| {
                this.update_draft(cx, |d| {
                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                        rule.skip_weekends = !rule.skip_weekends;
                    }
                })
            }));
        let by_due = !rule.from_completion;
        let actions = h_flex()
            .gap_2()
            .child(button("repeat-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| {
                this.update_draft(cx, |d| d.menu = Some(DueMenu::Repeat))
            })))
            .child(button("repeat-ok", strings.ok, true).on_click(cx.listener({
                let rule = rule.clone();
                move |this, _, _, cx| this.update_draft(cx, |d| d.pick_repeat(Some(rule.clone()), Local::now().date_naive()))
            })));
        if rule.is_specific_dates() {
            return v_flex()
                .gap_2()
                .p_2()
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border)
                .child(mode)
                .child(self.render_specific_dates(&rule, cx))
                .child(actions)
                .into_any_element();
        }
        v_flex()
            .gap_2()
            .p_2()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().border)
            .child(mode)
            .child(
                h_flex()
                    .gap_2()
                    .child(Label::new(strings.repeat_every).size(LabelSize::Small))
                    .child(div().flex_1())
                    .child(
                        IconButton::new("repeat-fewer", IconName::Minus)
                            .size(px(22.))
                            .icon_size(IconSize::XSmall)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_draft(cx, |d| {
                                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                                        rule.interval = rule.interval.saturating_sub(1).max(1);
                                    }
                                })
                            })),
                    )
                    .child(div().min_w(px(20.)).flex().justify_center().child(Label::new(rule.interval.to_string())))
                    .child(
                        IconButton::new("repeat-more", IconName::Plus)
                            .size(px(22.))
                            .icon_size(IconSize::XSmall)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_draft(cx, |d| {
                                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                                        rule.interval = (rule.interval + 1).min(99);
                                    }
                                })
                            })),
                    ),
            )
            .child(units)
            .when(by_due && rule.frequency == Frequency::Daily, |el| el.child(skip_weekends))
            .when(by_due && rule.frequency == Frequency::Weekly, |el| el.child(weekdays))
            .when(by_due && rule.frequency == Frequency::Monthly, |el| el.child(self.render_monthly_rule(rule.clone(), strings, cx)))
            .child(actions)
            .into_any_element()
    }

    /// "By Specific Dates", as in the original: a month calendar where each click picks or drops
    /// a date; past days cannot be picked.
    fn render_specific_dates(&self, rule: &Recurrence, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let month = self.specific_month.unwrap_or_else(|| calendar::month_start(rule.dates.first().copied().unwrap_or(today)));
        let nav = |id: &'static str, icon: IconName, delta: i32| {
            IconButton::new(id, icon).size(px(22.)).icon_size(IconSize::XSmall).on_click(cx.listener(move |this, _, _, cx| {
                this.specific_month = Some(calendar::add_months(month, delta));
                cx.notify();
            }))
        };
        let days = calendar::month_grid(month);
        let weeks: Vec<_> = days.chunks(7).map(|week| {
            h_flex().children(week.iter().map(|&date| {
                let picked = rule.dates.contains(&date);
                let past = date < today;
                let in_month = date.month() == month.month();
                div().flex_1().flex().justify_center().child(
                    div()
                        .id(SharedString::from(format!("specific-{date}")))
                        .size(px(26.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .when(picked, |el| el.bg(theme.colors().accent))
                        .when(!picked && !past, |el| el.cursor_pointer().hover(|s| s.bg(theme.colors().element_hover)))
                        .child(Label::new(date.day().to_string()).size(LabelSize::Small).color(
                            if picked || (in_month && !past) { Color::Default } else { Color::Muted },
                        ))
                        .when(!past, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| {
                                this.update_draft(cx, |d| {
                                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                                        if let Some(i) = rule.dates.iter().position(|d| *d == date) {
                                            // The last date stays: no dates would end "By Specific Dates".
                                            if rule.dates.len() > 1 {
                                                rule.dates.remove(i);
                                            }
                                        } else {
                                            rule.dates.push(date);
                                            rule.dates.sort();
                                        }
                                    }
                                })
                            }))
                        }),
                )
            }))
        }).collect();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(calendar::month_title(month, strings)).size(LabelSize::Small)))
                    .child(nav("specific-prev", IconName::ChevronLeft, -1))
                    .child(nav("specific-next", IconName::ChevronRight, 1)),
            )
            .child(h_flex().children(calendar::weekday_labels(&strings.weekdays_narrow).map(|day| {
                div().flex_1().flex().justify_center().child(Label::new(day).size(LabelSize::XSmall).color(Color::Muted))
            })))
            .children(weeks)
            .into_any_element()
    }

    /// Monthly rules, as in the original: "Each" picks days of the month (and the last day),
    /// "On the" picks the nth weekday, "Workday" the first or last workday.
    fn render_monthly_rule(&self, rule: Recurrence, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let on_the = rule.nth_weekday.is_some();
        let workday = rule.nth_workday.is_some();
        let update = move |this: &mut Self, cx: &mut Context<Self>, change: &dyn Fn(&mut Recurrence)| {
            this.update_draft(cx, |d| {
                if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                    change(rule);
                }
            })
        };
        // Tabs: 0 "Each", 1 "On the", 2 "Workday".
        let tab = |id: &'static str, label: &'static str, selected: bool, to: u8| {
            div()
                .id(id)
                .flex_1()
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.colors().element_selected))
                .child(Label::new(label).size(LabelSize::XSmall))
                .on_click(cx.listener(move |this, _, _, cx| {
                    update(this, cx, &|rule| {
                        if to != 1 {
                            rule.nth_weekday = None;
                        }
                        if to != 2 {
                            rule.nth_workday = None;
                        }
                        if to != 0 {
                            rule.month_days = 0;
                        }
                        match to {
                            1 => _ = rule.nth_weekday.get_or_insert((1, chrono::Weekday::Mon)),
                            2 => _ = rule.nth_workday.get_or_insert(1),
                            _ => {}
                        }
                    })
                }))
        };
        let chip = |id: SharedString, label: String, on: bool, bit: u32| {
            div()
                .id(id)
                .h(px(22.))
                .min_w(px(22.))
                .px_1()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .when(on, |el| el.bg(theme.colors().accent))
                .when(!on, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).size(LabelSize::XSmall))
                .on_click(cx.listener(move |this, _, _, cx| update(this, cx, &|rule| rule.month_days ^= bit)))
        };
        let cycler = |id: &'static str, label: String, next: fn(&mut Recurrence)| {
            div()
                .id(id)
                .flex_1()
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .border_1()
                .border_color(theme.colors().border)
                .cursor_pointer()
                .child(Label::new(label).size(LabelSize::XSmall))
                .on_click(cx.listener(move |this, _, _, cx| update(this, cx, &|rule| next(rule))))
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_1()
                    .child(tab("monthly-each", strings.repeat_each, !on_the && !workday, 0))
                    .child(tab("monthly-on-the", strings.repeat_on_the, on_the, 1))
                    .child(tab("monthly-workday", strings.repeat_workday, workday, 2)),
            )
            .map(|el| match rule.nth_workday {
                Some(nth) => el.child(h_flex().child(cycler(
                    "monthly-workday-nth",
                    strings.repeat_workday_nth[usize::from(nth < 0)].to_owned(),
                    |rule| {
                        if let Some(nth) = rule.nth_workday.as_mut() {
                            *nth = if *nth < 0 { 1 } else { -1 };
                        }
                    },
                ))),
                None => el,
            })
            .when(!workday, |el| el.map(|el| match rule.nth_weekday {
                Some((nth, weekday)) => {
                    let nth_label = if nth < 0 { strings.repeat_nth[4] } else { strings.repeat_nth[(nth as usize).clamp(1, 4) - 1] };
                    el.child(
                        h_flex()
                            .gap_2()
                            .child(cycler("monthly-nth", nth_label.to_owned(), |rule| {
                                if let Some((nth, _)) = rule.nth_weekday.as_mut() {
                                    *nth = match *nth { 1 => 2, 2 => 3, 3 => 4, 4 => -1, _ => 1 };
                                }
                            }))
                            .child(cycler(
                                "monthly-weekday",
                                strings.weekdays_full[weekday.num_days_from_monday() as usize].to_owned(),
                                |rule| {
                                    if let Some((_, day)) = rule.nth_weekday.as_mut() {
                                        *day = day.succ();
                                    }
                                },
                            )),
                    )
                }
                None => el.child(
                    div().flex().flex_wrap().gap_1().children((0..31u32).map(|i| {
                        chip(SharedString::from(format!("month-day-{i}")), (i + 1).to_string(), rule.month_days & (1 << i) != 0, 1 << i)
                    }))
                    .child(chip(
                        "month-day-last".into(),
                        strings.repeat_last_day_title.to_owned(),
                        rule.month_days & fern_core::recurrence::LAST_DAY != 0,
                        fern_core::recurrence::LAST_DAY,
                    )),
                ),
            }))
    }

    fn render_priority_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = self.task.as_ref().map(|t| t.priority).unwrap_or_default();
        let items = [
            (Priority::High, "priority-high", strings.priority_high),
            (Priority::Medium, "priority-medium", strings.priority_medium),
            (Priority::Low, "priority-low", strings.priority_low),
            (Priority::None, "priority-none", strings.priority_none),
        ];
        popover(cx)
            .w(px(190.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .children(items.into_iter().map(|(priority, id, label)| {
                let color = task_rows::priority_color(priority, &theme).unwrap_or(theme.colors().checkbox_border);
                ListItem::new(id)
                    .start_slot(Icon::new(IconName::Flag).size(IconSize::Large).color(color))
                    .child(Label::new(label))
                    .when(priority == current, |item| item.end_slot(check_mark()))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_priority(priority, cx)))
            }))
            .into_any_element()
    }

    fn render_list_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let current = self.task.as_ref().and_then(|t| t.list_id);
        popover(cx)
            .w(px(200.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(
                ListItem::new("move-to-inbox")
                    .start_slot(Icon::new(IconName::Inbox))
                    .child(Label::new(strings.inbox))
                    .when(current.is_none(), |item| item.end_slot(check_mark()))
                    .on_click(cx.listener(|this, _, _, cx| this.set_list(None, cx))),
            )
            .children(self.lists.iter().flat_map(|list| {
                let id = list.id;
                let current_section = self.task.as_ref().and_then(|t| t.section_id);
                // A list's sections are offered under it, as in the original.
                let sections = self.store.list_sections(Some(id)).unwrap_or_default();
                std::iter::once(
                    ListItem::new(SharedString::from(format!("move-to-{id}")))
                        .start_slot(Icon::new(IconName::List))
                        .child(Label::new(list.name.clone()).truncate())
                        .when(current == Some(id) && current_section.is_none(), |item| item.end_slot(check_mark()))
                        .on_click(cx.listener(move |this, _, _, cx| this.set_list(Some(id), cx)))
                        .into_any_element(),
                )
                .chain(sections.into_iter().map(|section| {
                    let section_id = section.id;
                    ListItem::new(SharedString::from(format!("move-to-section-{section_id}")))
                        .child(div().pl_6().child(Label::new(section.name).size(LabelSize::Small).truncate()))
                        .when(current_section == Some(section_id), |item| item.end_slot(check_mark()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.save(cx, |store, task| {
                                store.set_list(task, Some(id))?;
                                store.set_section(task, Some(section_id))
                            });
                            this.close_popup(cx);
                        }))
                        .into_any_element()
                }))
                .collect::<Vec<_>>()
            }))
            .into_any_element()
    }

    fn render_tags_popup(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = self.task_tags();
        popover(cx)
            .w(px(220.))
            .gap_1()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(
                h_flex()
                    .h(px(30.))
                    .px_2()
                    .rounded(px(4.))
                    .bg(theme.colors().background)
                    .child(self.tag_input.clone()),
            )
            .children(self.all_tags.iter().map(|tag| {
                let name = tag.name.clone();
                ListItem::new(SharedString::from(format!("tag-option-{}", tag.name)))
                    .start_slot(
                        Icon::new(IconName::Tag)
                            .size(IconSize::Large)
                            .color(lists_panel::color(tag.color.as_deref(), cx)),
                    )
                    .child(Label::new(tag.name.clone()))
                    .when(current.contains(&tag.name), |item| item.end_slot(check_mark()))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_tag(name.clone(), cx)))
            }))
            .into_any_element()
    }

    fn render_tags_row(&self, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .flex_wrap()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // The Estimation (Start Focus → Estimation), which opens it again.
            .children(self.task.as_ref().and_then(|t| t.estimate).map(|estimate| {
                let text = match estimate {
                    fern_core::Estimate::Pomos(n) => format!("{n} {}", strings.pomo_unit),
                    fern_core::Estimate::Minutes(n) => fern_core::format_minutes(n as i64),
                };
                h_flex()
                    .id("estimate-chip")
                    .h(px(18.))
                    .px(px(6.))
                    .gap_1()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(theme.colors().accent.opacity(0.2))
                    .child(Icon::new(IconName::CircleDot).size(IconSize::XSmall).color(Color::Accent))
                    .child(Label::new(text).size(LabelSize::XSmall).color(Color::Accent))
                    .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Estimate, cx)))
            }))
            .children(self.task_tags().into_iter().map(|tag| {
                let name = tag.clone();
                Chip::new(tag.clone(), self.tag_color(&tag, cx)).end_slot(
                    div()
                        .id(SharedString::from(format!("remove-tag-{tag}")))
                        .cursor_pointer()
                        .opacity(0.6)
                        .hover(|s| s.opacity(1.))
                        .child("×")
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_tag(&name, cx))),
                )
            }))
            .child(
                div()
                    .id("add-tag")
                    .relative()
                    .h(px(18.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.colors().element_hover))
                    .child(Label::new(strings.add_tag).size(LabelSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_popup(Popup::Tags, cx);
                        window.focus(&this.tag_input.focus_handle(cx), cx);
                    }))
                    .when(self.popup == Some(Popup::Tags), |el| {
                        el.child(PopoverAttachment::new(Placement::BelowLeft, self.render_tags_popup(cx)))
                    }),
            )
    }
}

/// The activity log's kinds, in the order of `Strings::activity_kinds`.
const ACTIVITY_KINDS: [&str; 11] = [
    "created", "renamed", "completed", "reopened", "date", "date_cleared", "priority", "moved", "deleted", "restored",
    "wont_do",
];

/// What the activity log says for a kind: "created the task".
pub fn activity_text<'a>(kind: &'a str, strings: &'static Strings) -> &'a str {
    ACTIVITY_KINDS.iter().position(|k| *k == kind).map_or(kind, |i| strings.activity_kinds[i])
}

fn check_mark() -> impl IntoElement {
    Label::new("✓").color(Color::Accent)
}

/// A text button with an icon, used for the due date and the list.
fn label_button(id: &'static str, icon: IconName, label: SharedString, color: Color, cx: &App) -> Stateful<Div> {
    let hover = cx.theme().colors().element_hover;
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

impl Render for TaskDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        if let Some(target) = self.pending_focus.take() {
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
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .when(!self.card, |el| el.border_l_1().border_color(theme.colors().border));

        let Some(task) = self.task.as_ref() else {
            return panel
                .items_center()
                .justify_center()
                .child(Label::new(strings.select_task).color(Color::Muted));
        };
        let done = task.is_completed();
        let notes_focus = self.notes.focus_handle(cx);
        let today = Local::now().date_naive();
        let repeats = task.repeat.is_some();
        let (due_text, due_color): (SharedString, Color) = match task.due_date {
            Some(date) => {
                let (label, overdue) = task_rows::due_label(date, task.due_time, today, strings);
                let label = match task.end_date {
                    Some(end) => due_picker::range_label(date, task.due_time, end, task.end_time, strings),
                    None => label,
                };
                // The calendar's card writes the date out, as the original's.
                let label = if self.card && task.end_date.is_none() { task_rows::card_due_label(date, task.due_time, today, strings) } else { label };
                (label.into(), if overdue && !done { Color::Error } else { Color::Accent })
            }
            None => (strings.due_date.into(), Color::Muted),
        };
        let priority_color = task_rows::priority_color(task.priority, &theme).map_or(Color::Muted, Color::Custom);
        let list_name: SharedString = task
            .list_id
            .and_then(|id| self.lists.iter().find(|l| l.id == id))
            .map(|l| l.name.clone().into())
            .unwrap_or_else(|| strings.inbox.into());
        let list_icon = if task.list_id.is_some() { IconName::List } else { IconName::Inbox };

        panel
            // Top bar: checkbox | due date ……… priority
            .child(
                h_flex()
                    .gap_2()
                    .h(px(48.))
                    .px_5()
                    .border_b_1()
                    .border_color(theme.colors().border)
                    .child(
                        // The original's 15 px box in the details' top bar.
                        Checkbox::new("detail-checkbox")
                            .size(px(15.))
                            .toggle_state(done)
                            .dismissed(task.is_wont_do())
                            .lines(!self.checklist.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_completed(cx))),
                    )
                    // The original's short line between the box and the date.
                    .child(div().w(px(1.)).h(px(14.)).bg(theme.colors().border))
                    .child(
                        label_button("due-date", IconName::CalendarDays, due_text, due_color, cx)
                            .when(repeats, |el| el.child(Icon::new(IconName::Repeat).size(IconSize::Small).color(due_color)))
                            .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Due, cx)))
                            .when(self.popup == Some(Popup::Due), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowLeft,
                                    self.render_due_popup(strings, cx),
                                ))
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .relative()
                            .child(
                                IconButton::new("priority", IconName::Flag)
                                    .icon_color(priority_color)
                                    .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Priority, cx))),
                            )
                            .when(self.popup == Some(Popup::Priority), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    self.render_priority_popup(strings, cx),
                                ))
                            }),
                    ),
            )
            // Title, checklist, notes and tags; clicking empty space puts the caret into the notes.
            .child(
                v_flex()
                    .id("detail-body")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_5()
                    .pt_4()
                    .pb_6()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&notes_focus, cx);
                    })
                    .child(
                        h_flex()
                            .items_start()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(20.))
                                    .line_height(px(28.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .when(done, |el| el.text_color(theme.colors().text_muted))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .child(self.title.clone()),
                            )
                            .child(
                                div()
                                    .pt_0p5()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .child(
                                        IconButton::new("checklist", IconName::ListChecks)
                                            .tooltip(Tooltip::text(i18n::t(cx).checklist))
                                            .on_click(cx.listener(|this, _, _, cx| this.start_checklist(cx))),
                                    ),
                            ),
                    )
                    // As in the original, the description comes first and the checklist under it.
                    .child(
                        div()
                            .text_size(px(15.))
                            .line_height(px(22.))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(self.notes.clone()),
                    )
                    .when(self.show_checklist, |el| el.child(self.render_checklist(cx)))
                    .when(!self.attachments.is_empty(), |el| el.child(self.render_attachments(cx)))
                    // Tags sit under the notes, as in the original.
                    // The calendar's card leaves out the empty tags and subtasks, as the original's.
                    .when(!self.card || !task.tags.is_empty(), |el| el.child(self.render_tags_row(strings, cx)))
                    .when(!self.card || !self.subtasks.is_empty(), |el| {
                        el.child(div().pt_2().child(Divider::horizontal())).child(self.render_subtasks(strings, cx))
                    })
                    .when(!self.comments.is_empty(), |el| el.child(self.render_comments(strings, cx))),
            )
            // The bottom bar's "A" shows the description's Markdown toolbar over it, as the original.
            .when(self.format_bar && !task.is_deleted(), |el| {
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
            .when(self.comment_open && !task.is_deleted(), |el| el.child(self.render_comment_box(cx)))
            .when_some(self.activity.clone(), |el, activity| el.child(self.render_activity(&activity, strings, cx)))
            // Bottom bar: list ……… formatting, comments, more; for a trashed task, as in the
            // original: Restore ……… delete forever.
            .child(
                h_flex()
                    .gap_2()
                    // As the original's: no line over the bottom bar.
                    .h(px(44.))
                    .px_5()
                    .map(|bar| {
                        if task.is_deleted() {
                            bar.child(
                                label_button("restore-task", IconName::ArchiveRestore, strings.restore.into(), Color::Default, cx)
                                    .on_click(cx.listener(|this, _, _, cx| this.restore(cx))),
                            )
                            .child(div().flex_1())
                            .child(
                                IconButton::new("delete-forever", IconName::Trash)
                                    .tooltip(Tooltip::text(strings.delete_forever))
                                    .on_click(cx.listener(|this, _, _, cx| this.delete_forever(cx))),
                            )
                        } else {
                            bar.child(
                                label_button("list", list_icon, list_name, Color::Muted, cx)
                                    .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::List, cx)))
                                    .when(self.popup == Some(Popup::List), |el| {
                                        el.child(PopoverAttachment::new(
                                            Placement::BelowLeft,
                                            self.render_list_popup(strings, cx),
                                        ))
                                    }),
                            )
                            .child(div().flex_1())
                            .child(
                                IconButton::new("format", IconName::Baseline)
                                    .toggle_state(self.format_bar)
                                    .tooltip(Tooltip::text(strings.text_format))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.format_bar = !this.format_bar;
                                        this.format_headings = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                IconButton::new("comments", IconName::MessageSquareText)
                                    .toggle_state(self.comment_open)
                                    .tooltip(Tooltip::text(strings.comments))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.comment_open = !this.comment_open;
                                        if this.comment_open {
                                            window.focus(&this.comment_input.focus_handle(cx), cx);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .relative()
                                    .child(
                                        IconButton::new("more", IconName::Ellipsis)
                                            .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::More, cx))),
                                    )
                                    .when(self.popup == Some(Popup::More), |el| {
                                        el.child(PopoverAttachment::new(
                                            Placement::AboveRight,
                                            self.render_more_popup(strings, cx),
                                        ))
                                    })
                                    .when(self.popup == Some(Popup::Estimate), |el| {
                                        el.child(PopoverAttachment::new(
                                            Placement::AboveRight,
                                            self.render_estimate(strings, cx),
                                        ))
                                    })
                                    .when(self.popup == Some(Popup::LinkParent), |el| {
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
fn format_size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

#[cfg(test)]
mod size_tests {
    #[test]
    fn sizes() {
        assert_eq!(super::format_size(512), "512 B");
        assert_eq!(super::format_size(12 * 1024 + 5), "12 KB");
        assert_eq!(super::format_size(3_565_158), "3.4 MB");
    }
}
