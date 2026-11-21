//! The task details panel on the right, modeled on TickTick's.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use fern_core::attachment::PreparedAttachment;
use fern_core::{ChecklistItem, Frequency, Id, Kind, List, Priority, Recurrence, Store, Tag, Task};
use chrono::{Datelike, Local, NaiveDate, TimeZone as _};
use gpui::{Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, FontWeight, MouseButton, Subscription, WeakFocusHandle};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, SubmenuItem, TipSide, popover, with_tip};

use crate::actions::{self as md, DismissTransient};
use crate::calendar;
use crate::due_picker::{self, CustomReminder, DueDraft, DueMenu, DueTab, RangeField, ReminderUnit};
use crate::i18n::{self, CurrentLocale, Strings};
use crate::lists_panel;
use crate::task_rows;
use crate::markdown_toolbar::ToolbarAction;
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};

mod draft;
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
    /// The calendar's card writes its date out with the weekday.
    calendar_card: bool,
    /// The Add Subtask field shown while the task has no subtasks, from the "…" menu.
    subtask_open: bool,
    body_scroll: gpui::ScrollHandle,
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

impl EventEmitter<TaskUpdated> for TaskDetail {}
impl EventEmitter<StartFocus> for TaskDetail {}
impl EventEmitter<Undoable> for TaskDetail {}
impl EventEmitter<OpenTask> for TaskDetail {}

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
                let text = input.read(cx).text().to_owned();
                if this.edit_draft(cx, |task, _| task.title = text.clone()) {
                    this.refresh_draft_title(cx);
                    return;
                }
                this.save(cx, |store, id| store.set_title(id, &text));
            }),
            cx.subscribe(&notes, |this, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                if this.edit_draft(cx, |task, _| task.notes = text.clone()) { return; }
                this.save(cx, |store, id| store.set_notes(id, &text));
            }),
            cx.subscribe(&location_input, |this, input, _: &Changed, cx| {
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
            calendar_card: false,
            subtask_open: false,
            body_scroll: gpui::ScrollHandle::new(),
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
        [&self.title, &self.notes, &self.comment_input, &self.subtask_input, &self.new_item]
            .into_iter().any(|input| input.focus_handle(cx) == *focus)
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
        self.session.task = task;
        self.session.popup = None;
        self.editor_window.popup_return_focus = None;
        self.session.tag_draft = None;
        self.session.attachment_errors.clear();
        self.subtask_open = false;
        self.session.activity = None;
        self.session.all_tags = self.session.store.tags().unwrap_or_default();
        self.checklist.clear();
        self.sync_checklist(cx);
        self.show_checklist = !self.checklist.is_empty();
        self.sync_subtasks();
        self.sync_attachments();
        self.sync_comments();
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
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.copied_feedback = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_markdown_check(&mut self, line_index: usize, cx: &mut Context<Self>) {
        let current = self.session.task.as_ref().map(|t| t.notes.clone()).unwrap_or_default();
        let Some((updated, checked)) = fern_core::markdown::toggle_check_line(&current, line_index) else {
            return;
        };
        if checked {
            desktop_common::sound::play_complete(&self.session.store);
        }
        self.notes.update(cx, |input, cx| input.set_text(updated.clone(), cx));
        if self.edit_draft(cx, |task, _| task.notes = updated.clone()) {
            return;
        }
        self.save(cx, |store, id| store.set_notes(id, &updated));
    }

    /// Formats a task's comments section.
    fn render_comments(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .pt_3()
            .gap_2()
            .child(Label::new(format!("{} {}", strings.comments, self.session.comments.len())).size(LabelSize::Small).color(Color::Muted))
            .children(self.session.comments.iter().map(|comment| {
                let id = comment.id;
                let when = chrono::DateTime::from_timestamp_millis(comment.created_at)
                    .map(|t| t.with_timezone(&Local).naive_local())
                    .map(|t| format!("{} {}", crate::calendar::format_date(t.date(), strings), crate::calendar::format_time(t.time(), strings)))
                    .unwrap_or_default();
                let group_name = SharedString::from(format!("comment-row-{id}"));
                h_flex()
                    .id(SharedString::from(format!("comment-{id}")))
                    .group(group_name.clone())
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
                        div().when(!crate::touch::active(), |el| el.invisible()).group_hover(group_name, |s| s.visible()).child(
                            IconButton::new(SharedString::from(format!("comment-delete-{id}")), IconName::X)
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text(strings.delete))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.session.store.delete_comment(id).is_ok() {
                                        cx.emit(TaskUpdated);
                                    }
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
        let has_comment_text = !self.comment_input.read(cx).text().trim().is_empty();
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
                    )
                    .when(has_comment_text, |el| {
                        el.child(
                            IconButton::new("comment-send", IconName::ArrowUp)
                                .icon_color(Color::Accent)
                                .tooltip(Tooltip::text(i18n::t(cx).add))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let text = this.comment_input.read(cx).text().trim().to_owned();
                                    if !text.is_empty()
                                        && let Some(task) = this.task_id()
                                        && this.session.store.add_comment(task, &text).is_ok()
                                    {
                                        this.comment_input.update(cx, |input, cx| input.clear(window, cx));
                                        this.sync_comments();
                                        cx.emit(TaskUpdated);
                                        cx.notify();
                                    }
                                })),
                        )
                    }),
            )
            .into_any_element()
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
        let Some(task) = self.session.task.as_ref().filter(|task| !task.is_deleted()).map(|task| task.id) else { return };
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |this, cx| {
            let Some(paths) = chosen.await.ok().and_then(Result::ok).flatten() else { return };
            this.update(cx, |this, cx| this.attach_paths(task, paths, cx)).ok();
        })
        .detach();
    }

    fn attach_paths(&mut self, task: Id, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
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

    /// The attached files under the notes: a click opens one with the system's app.
    fn render_attachments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let strings = i18n::t(cx);
        v_flex().gap_1().children(self.session.attachments.iter().map(|attachment| {
            let path = match &self.session.draft {
                Some(draft) => draft.attachments.iter().find(|prepared| prepared.metadata().id == attachment.id).map(|prepared| prepared.path().to_owned()),
                None => self.session.attachments_dir.as_ref().map(|dir| attachment.path(dir)),
            };
            let removed = attachment.clone();
            let att_id = attachment.id;
            let group_name = SharedString::from(format!("attachment-{att_id}"));
            let is_image = attachment
                .name
                .rsplit('.')
                .next()
                .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg"));
            h_flex()
                .id(group_name.clone())
                .debug_selector(|| "task-attachment".to_owned())
                .group(group_name)
                .h(px(36.))
                .px_2()
                .gap_2()
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border_variant)
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(Icon::new(if is_image { IconName::Image } else { IconName::Paperclip }).size(IconSize::Small).color(Color::Muted))
                .child(div().flex_1().min_w_0().child(Label::new(attachment.name.clone()).size(LabelSize::Small).truncate()))
                .child(Label::new(format_size(attachment.size)).size(LabelSize::XSmall).color(Color::Muted))
                .child(
                    IconButton::new(SharedString::from(format!("attachment-delete-{att_id}")), IconName::X)
                        .size(px(20.))
                        .icon_size(IconSize::XSmall)
                        .hover_icon_color(theme.status().error)
                        .tooltip(Tooltip::text(strings.delete))
                        .on_click(
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if let Some(draft) = &mut this.session.draft {
                                draft.attachments.retain(|prepared| prepared.metadata().id != removed.id);
                                this.sync_attachments();
                                cx.notify();
                                return;
                            }
                            let Some(dir) = &this.session.attachments_dir else { return };
                            match this.session.store.delete_attachment(&removed, dir) {
                                Ok(()) => cx.emit(TaskUpdated),
                                Err(err) => eprintln!("failed to remove the attachment: {err:#}"),
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
        self.session
            .task
            .as_ref()
            .is_some_and(|task| task.kind != Kind::Note && !task.is_deleted() && self.task_depth() < fern_core::MAX_SUBTASK_DEPTH)
    }

    fn can_link_parent(&self) -> bool {
        self.session
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
        self.save(cx, |store, id| store.unlink_parent(id));
        self.reload_current_task();
        cx.notify();
    }

    fn link_current_parent(&mut self, parent: Id, cx: &mut Context<Self>) {
        self.save(cx, |store, id| store.link_parent(id, parent));
        self.reload_current_task();
        cx.notify();
    }

    fn unlink_subtask(&mut self, subtask_id: Id, cx: &mut Context<Self>) {
        self.save(cx, |store, _| store.unlink_parent(subtask_id));
        self.sync_subtasks();
        cx.notify();
    }

    fn toggle_subtask(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
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
        let mut old: Vec<ChecklistRow> = std::mem::take(&mut self.checklist);
        for item in items {
            let row = match old.iter().position(|row| row.item.id == item.id) {
                Some(index) => {
                    let mut row = old.swap_remove(index);
                    if row.item.title != item.title && row.input.read(cx).text() != item.title {
                        row.input.update(cx, |input, cx| input.set_text(item.title.clone(), cx));
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
        let input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::Wrapped, "", cx).without_cancel();
            input.set_text(title, cx);
            input
        });
        let subscriptions = [
            cx.subscribe(&input, move |this, input, _: &Changed, cx| {
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
        if !self.edit_draft(cx, |_, draft| draft.checklist.retain(|item| item.id != id)) {
            self.save(cx, |store, _| store.delete_checklist_item(id));
        }
        self.sync_checklist(cx);
    }

    fn uncheck_all_checklist_items(&mut self, cx: &mut Context<Self>) {
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
        self.show_checklist = true;
        self.editor_window.pending_focus = Some(FocusTarget::NewItem);
        cx.notify();
    }

    /// Changes from another window can edit the task's fields. Keep the caret for edits already in the inputs.
    pub fn refresh(&mut self, task: Option<Task>, cx: &mut Context<Self>) {
        match task {
            Some(task) if self.task_id() == Some(task.id) => {
                if self.task_title() != Some(task.title.as_str()) && self.title.read(cx).text() != task.title {
                    self.title.update(cx, |input, cx| input.set_text(task.title.clone(), cx));
                }
                let old_notes = self.session.task.as_ref().map(|t| t.notes.as_str());
                if old_notes != Some(task.notes.as_str()) && self.notes.read(cx).text() != task.notes {
                    self.notes.update(cx, |input, cx| input.set_text(task.notes.clone(), cx));
                }
                let old_loc = self.session.task.as_ref().and_then(|t| t.location.as_deref());
                let new_loc = task.location.as_deref().unwrap_or("");
                if old_loc != task.location.as_deref() && self.location_input.read(cx).text() != new_loc {
                    self.location_input.update(cx, |input, cx| input.set_text(new_loc.to_owned(), cx));
                }
                self.location_open |= task.location.is_some();
                self.session.task = Some(task);
                self.sync_checklist(cx);
                self.show_checklist |= !self.checklist.is_empty();
                self.sync_subtasks();
                self.sync_attachments();
                self.sync_comments();
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
        self.focus_submenu = false;
        self.editor_window.popup_return_focus = None;
        self.session.tag_draft = None;
        match popup {
            Popup::Due => {
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
                    let excluded: std::collections::HashSet<Id> = self
                        .session
                        .store
                        .descendants(task.id)
                        .unwrap_or_default()
                        .into_iter()
                        .chain([task.id])
                        .collect();
                    let all_tasks = self.session.store.tasks().unwrap_or_default();
                    let parent_map: std::collections::HashMap<Id, Option<Id>> =
                        all_tasks.iter().map(|t| (t.id, t.parent_id)).collect();
                    let depth_of = |id: Id| -> usize {
                        let mut depth = 0;
                        let mut cur = id;
                        while let Some(Some(p)) = parent_map.get(&cur) {
                            depth += 1;
                            cur = *p;
                            if depth > fern_core::MAX_SUBTASK_DEPTH {
                                break;
                            }
                        }
                        depth
                    };
                    all_tasks
                        .into_iter()
                        .filter(|t| {
                            !t.is_closed()
                                && !t.is_deleted()
                                && t.kind != Kind::Note
                                && !excluded.contains(&t.id)
                                && task.parent_id != Some(t.id)
                                && depth_of(t.id) < fern_core::MAX_SUBTASK_DEPTH
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
        let return_focus = if self.popup_input_focused(window, cx) {
            self.editor_window.popup_return_focus.clone()
        } else {
            window.focused(cx).map(|focus| focus.downgrade())
        };
        self.open_popup(popup, cx);
        self.editor_window.popup_return_focus = return_focus;
    }

    fn open_tags(&mut self, popup: Popup, window: &mut Window, cx: &mut Context<Self>) {
        self.open_popup_from_ui(popup, window, cx);
        window.focus(&self.tag_input.focus_handle(cx), cx);
    }

    /// Opens the due-date picker (the "Set date" shortcut).
    /// Settings → Task Default → Default Duration, in minutes.
    fn default_duration(&self) -> i64 {
        self.session.store.preferences().map_or(60, |p| p.task_defaults.duration)
    }

    /// The details' own pickers, for the task list's right-click menu: `subtask` focuses the Add
    /// Subtask field, `due`, `tags` and `link-parent` open their popovers.
    pub fn start(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.task.is_none() {
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
        if self.session.task.is_some() {
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
            });
            let reminders: Vec<i64> = if draft.date.is_some() { draft.reminders.clone() } else { Vec::new() };
            let constant = draft.constant_reminder && !reminders.is_empty();
            if !self.edit_draft(cx, |task, state| {
                if changed {
                    task.due_date = draft.date;
                    task.due_time = draft.date.and(draft.time);
                    task.end_date = draft.end().map(|(date, _)| date);
                    task.end_time = draft.end().and_then(|(_, time)| time);
                    task.repeat = draft.repeat.clone();
                    task.timezone = draft.timezone.clone();
                    task.is_floating = draft.is_floating;
                    state.date_changed = true;
                }
                task.reminders = reminders.clone();
                task.constant_reminder = constant;
                state.reminders_changed = draft.reminders_changed;
            }) {
                self.save(cx, |store, id| {
                    store.set_due(id, draft.date, draft.time)?;
                    store.set_due_end(id, draft.end())?;
                    store.set_timezone(id, draft.timezone.as_deref(), draft.is_floating)?;
                    store.set_reminders(id, &reminders)?;
                    store.set_task_constant_reminder(id, constant)?;
                    store.set_repeat(id, draft.repeat, today)
                });
            } else {
                self.refresh_draft_title(cx);
            }
        }
        self.dismiss_popup(window, cx);
    }

    /// "Clear": removes the due date, time, timezone, reminders, and repeat.
    fn clear_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.due_draft = None;
        let today = Local::now().date_naive();
        if !self.edit_draft(cx, |task, state| {
            task.due_date = None; task.due_time = None; task.end_date = None; task.end_time = None;
            task.timezone = None; task.is_floating = false; task.constant_reminder = false;
            task.reminders.clear(); task.repeat = None; state.date_changed = true;
            state.reminders_changed = false;
        }) { self.save(cx, |store, id| {
            store.set_due(id, None, None)?;
            store.set_due_end(id, None)?;
            store.set_timezone(id, None, false)?;
            store.set_reminders(id, &[])?;
            store.set_task_constant_reminder(id, false)?;
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
        self.save(cx, |store, id| store.purge_task(id));
    }

    fn menu_action(&mut self, cx: &mut Context<Self>, write: impl FnOnce(&Store, Id) -> anyhow::Result<()>) {
        self.close_popup(cx);
        self.save(cx, write);
    }

    fn toggle_wont_do(&mut self, cx: &mut Context<Self>) {
        let wont_do = self.session.task.as_ref().is_some_and(|t| t.is_wont_do());
        self.menu_action(cx, |store, id| store.set_wont_do(id, !wont_do));
    }

    fn toggle_in_progress(&mut self, cx: &mut Context<Self>) {
        let in_progress = self.session.task.as_ref().is_some_and(|t| t.is_in_progress());
        self.menu_action(cx, |store, id| store.set_in_progress(id, !in_progress));
        self.reload_current_task();
    }

    fn toggle_kind(&mut self, cx: &mut Context<Self>) {
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

    /// The panel's "…" menu, a subset of the original's: the entries Fern supports so far.
    /// A "…" menu entry; hovering it closes the Start Focus side menu.
    fn close_side_menu_on_hover(&self, item: ListItem, cx: &mut Context<Self>) -> impl IntoElement {
        let selector = format!("task-menu-{}", item.id());
        div().id(item.id()).debug_selector(move || selector).child(item).on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if *hovered && this.focus_submenu {
                this.focus_submenu = false;
                cx.notify();
            }
        }))
    }

    fn render_more_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        self.render_more_popup_in(None, strings, cx)
    }

    fn render_more_popup_in(
        &self,
        max_height: Option<Pixels>,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.session.draft.is_some() { return self.render_draft_menu(strings, cx); }
        let Some(task) = self.session.task.as_ref() else { return div().into_any_element() };
        let wont_do = task.is_wont_do();
        let is_note = task.kind == Kind::Note;
        let submenu_bounds = Rc::new(Cell::new(None::<gpui::Bounds<Pixels>>));
        let outside_submenu = submenu_bounds.clone();
        let item = |id: &'static str, icon: IconName, label: &'static str| {
            ListItem::new(id).height(px(34.)).start_slot(Icon::new(icon).color(Color::Icon)).child(Label::new(label))
        };
        let content = v_flex()
            .id("task-more-popup-items")
            .when(self.sticky_style.is_some(), |menu| {
                menu.when(!is_note, |menu| {
                    let label = if task.is_completed() { strings.reopen } else { strings.completed };
                    menu.child(item("sticky-complete", IconName::SquareCheck, label)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_popup(cx);
                            this.toggle_completed(cx);
                        })))
                })
                .child(item("sticky-checklist", IconName::ListChecks, strings.checklist)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_popup(cx);
                        this.start_checklist(cx);
                    })))
                .child(item("sticky-priority", IconName::Flag, strings.summary_priority)
                    .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Priority, cx))))
                .child(item("sticky-move", IconName::Inbox, strings.move_to)
                    .on_click(cx.listener(|this, _, window, cx| this.open_popup_from_ui(Popup::List, window, cx))))
                .child(item("sticky-format", IconName::Baseline, strings.text_format)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_popup(cx);
                        this.format_bar = !this.format_bar;
                        this.format_headings = false;
                    })))
                .child(item("sticky-comments", IconName::MessageSquareText, strings.comment_tip)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_popup(cx);
                        this.comment_open = !this.comment_open;
                        if this.comment_open {
                            window.focus(&this.comment_input.focus_handle(cx), cx);
                        }
                    })))
            })
            .when(self.can_add_subtask(), |el| {
                el.child(self.close_side_menu_on_hover(item("more-subtask", IconName::CornerDownRight, strings.add_subtask_item).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.close_popup(cx);
                        this.subtask_open = true;
                        window.focus(&this.subtask_input.focus_handle(cx), cx);
                    },
                )), cx))
            })
            .when(!is_note, |el| {
                el.child(self.close_side_menu_on_hover(item("more-link-parent", IconName::ListTree, strings.link_parent).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.open_popup_from_ui(Popup::LinkParent, window, cx);
                        window.focus(&this.link_search.focus_handle(cx), cx);
                    },
                )), cx))
            })
            .when(!is_note && task.parent_id.is_some(), |el| {
                el.child(self.close_side_menu_on_hover(item("more-unlink-parent", IconName::X, strings.unlink_parent).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.close_popup(cx);
                        this.unlink_current_parent(cx);
                    },
                )), cx))
            })
            .child(self.close_side_menu_on_hover({
                let pinned = task.pinned_at.is_some();
                item("more-pin", if pinned { IconName::PinOff } else { IconName::Pin }, if pinned { strings.unpin } else { strings.pin })
                    .on_click(cx.listener(move |this, _, _, cx| this.menu_action(cx, move |store, id| store.set_pinned(id, !pinned))))
            }, cx))
            .when(!is_note, |el| {
                let active = task.is_in_progress();
                el.child(self.close_side_menu_on_hover(
                    item("more-in-progress", IconName::Hourglass, if active { strings.clear_in_progress } else { strings.mark_in_progress })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_in_progress(cx))),
                    cx,
                ))
            })
            .child(self.close_side_menu_on_hover(
                item("more-wont-do", if wont_do { IconName::RotateCcw } else { IconName::SquareX }, if wont_do {
                    strings.reopen
                } else {
                    strings.mark_wont_do
                })
                .on_click(cx.listener(|this, _, _, cx| this.toggle_wont_do(cx))),
            cx,
            ))
            .child(self.close_side_menu_on_hover(item("more-location", IconName::MapPin, strings.add_location).on_click(cx.listener(|this, _, window, cx| {
                this.close_popup(cx);
                this.location_open = true;
                window.focus(&this.location_input.focus_handle(cx), cx);
                cx.notify();
            })), cx))
            .child(self.close_side_menu_on_hover(item("more-tags", IconName::Tag, strings.tags).on_click(cx.listener(|this, _, window, cx| {
                this.open_tags(Popup::Tags, window, cx);
            })), cx))
            .child(self.close_side_menu_on_hover(
                item("more-attachment", IconName::Paperclip, strings.upload_attachment)
                    .on_click(cx.listener(|this, _, _, cx| this.upload_attachment(cx))),
                cx,
            ))
            .child(
                SubmenuItem::new("more-focus", strings.start_focus)
                    .icon(IconName::CircleDot)
                    .height(px(34.))
                    .open(self.focus_submenu)
                    .track_bounds(submenu_bounds)
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered && !this.focus_submenu {
                            this.focus_submenu = true;
                            cx.notify();
                        }
                    }))
                    .items([(false, strings.start_pomo), (true, strings.start_stopwatch)].map(|(stopwatch, label)| {
                        ListItem::new(SharedString::from(format!("more-focus-{stopwatch}")))
                            .height(px(34.))
                            .child(Label::new(label))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.close_popup(cx);
                                if let Some(task) = this.session.task.as_ref() {
                                    cx.emit(StartFocus(task.id, task.title.clone(), stopwatch));
                                }
                            }))
                    }))
                    .items([ListItem::new("more-focus-estimate")
                        .height(px(34.))
                        .child(div().debug_selector(|| "task-focus-estimate".into()).child(Label::new(strings.estimation)))
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
            .child(self.close_side_menu_on_hover(item("more-duplicate", IconName::Copy, strings.duplicate).on_click(cx.listener(
                |this, _, _, cx| this.menu_action(cx, |store, id| store.duplicate_task(id).map(|_| ())),
            )), cx))
            .child(self.close_side_menu_on_hover(item("more-copy-link", IconName::Link, strings.copy_link).on_click(cx.listener(|this, _, _, cx| {
                this.close_popup(cx);
                if let Some(task) = this.session.task.as_ref() {
                    this.copy_to_clipboard(fern_core::link::task_link(task.id), cx);
                }
            })), cx))
            .child(self.close_side_menu_on_hover(item("more-copy-markdown", IconName::Code, strings.copy_markdown).on_click(cx.listener(|this, _, _, cx| {
                this.close_popup(cx);
                if let Some(task) = this.session.task.as_ref() {
                    let checklist = this.session.store.checklist(task.id).unwrap_or_default();
                    let text = fern_core::markdown::format_task_markdown(task, &checklist, &this.session.subtasks);
                    this.copy_to_clipboard(text, cx);
                }
            })), cx))
            .child(self.close_side_menu_on_hover(item("more-copy-plain", IconName::FileText, strings.copy_plain_text).on_click(cx.listener(|this, _, _, cx| {
                this.close_popup(cx);
                if let Some(task) = this.session.task.as_ref() {
                    let checklist = this.session.store.checklist(task.id).unwrap_or_default();
                    let text = fern_core::markdown::format_task_plain_text(task, &checklist, &this.session.subtasks);
                    this.copy_to_clipboard(text, cx);
                }
            })), cx))
            .when(is_note || (task.parent_id.is_none() && self.session.subtasks.is_empty()), |el| {
                el.child(self.close_side_menu_on_hover(
                    item(
                        "more-convert",
                        if is_note { IconName::SquareCheck } else { IconName::FileText },
                        if is_note { strings.convert_to_task } else { strings.convert_to_note },
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_kind(cx))),
                cx,
                ))
            })
            .child(div().py_1().child(Divider::horizontal()))
            .child(self.close_side_menu_on_hover(item("more-print", IconName::Printer, strings.print).on_click(cx.listener(|this, _, _, cx| {
                this.close_popup(cx);
                if let Some(task) = this.session.task.clone() {
                    let checklist = this.session.store.checklist(task.id).unwrap_or_default();
                    let html = crate::print::task_html(&task, &checklist, Local::now().date_naive(), i18n::t(cx));
                    if let Err(err) = crate::print::open("task", &html) {
                        eprintln!("failed to print: {err}");
                    }
                }
            })), cx))
            .child(self.close_side_menu_on_hover(item("more-delete", IconName::Trash, strings.delete).on_click(cx.listener(|this, _, _, cx| {
                let task = this.session.task.as_ref().map(|t| (t.id, t.title.clone()));
                this.menu_action(cx, |store, id| store.delete_task(id));
                if let Some((id, title)) = task {
                    let label = i18n::t(cx).undo_deleted.replace("{title}", &title);
                    cx.emit(Undoable(crate::undo::UndoAction::Restore(id), label));
                }
            })), cx));
        // The original's menu: 178 px wide (wider when a label needs it), a row every 34 px.
        popover(cx)
            .debug_selector(|| "task-more-popup".into())
            .min_w(px(178.))
            .on_mouse_down_out(cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                if !outside_submenu.get().is_some_and(|bounds| bounds.contains(&event.position)) {
                    this.close_popup(cx);
                }
            }))
            .map(|el| match max_height {
                Some(h) => el
                    .p_0()
                    .child(v_scroll("sticky-more-scroll-view", content.p_1()).max_h(h).compact()),
                None => el.child(content),
            })
            .into_any_element()
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

        v_scroll(
            "task-activity-scroll",
            v_flex()
                .id("task-activity")
                .px_5()
                .py_3()
                .gap_2()
                .child(
                    h_flex()
                        .child(div().flex_1().child(Label::new(strings.task_activities).weight(FontWeight::SEMIBOLD)))
                        .child(
                            IconButton::new("close-activity", IconName::X)
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
                        .child(div().mt(px(5.)).size(px(8.)).flex_none().rounded_full().bg(theme.colors().accent))
                        .child(
                            v_flex()
                                .child(Label::new(text).size(LabelSize::Small))
                                .child(Label::new(when).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                })),
        )
        .max_h(px(240.))
        .border_t_1()
        .border_color(theme.colors().border)
        .into_any_element()
    }

    /// The "Link Parent Task" picker: open tasks other than this one and its subtasks, filtered by
    /// the search field, each with its list.
    /// The original's Estimation popover: Estimated Pomo or Estimated Duration, with Cancel / OK.
    fn render_estimate(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let draft = self.session.estimate_draft;
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
                    if this.session.estimate_draft.minutes != minutes {
                        this.session.estimate_draft = EstimateDraft { minutes, amount: 0 };
                        cx.notify();
                    }
                }))
        };
        let step = |id: &'static str, icon: IconName, up: bool| {
            IconButton::new(id, icon).size(px(24.)).icon_size(IconSize::Small).on_click(cx.listener(move |this, _, _, cx| {
                this.session.estimate_draft.step(up);
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
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |l| l.color(Color::OnAccent)))
        };
        popover(cx)
            .debug_selector(|| "task-estimate-popup".into())
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
                    .gap_1p5()
                    .children(
                        (if draft.minutes { [30_u32, 60, 120, 240] } else { [1, 2, 4, 6] })
                            .into_iter()
                            .map(|preset| {
                                let selected = draft.amount == preset;
                                let label = if draft.minutes {
                                    fern_core::format_minutes(preset as i64)
                                } else {
                                    preset.to_string()
                                };
                                div()
                                    .id(SharedString::from(format!("estimate-preset-{preset}")))
                                    .flex_1()
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(5.))
                                    .cursor_pointer()
                                    .bg(if selected {
                                        theme.colors().accent.opacity(0.18)
                                    } else {
                                        theme.colors().element_hover
                                    })
                                    .hover(|s| s.bg(theme.colors().accent.opacity(0.24)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.session.estimate_draft.amount = preset;
                                        cx.notify();
                                    }))
                                    .child(
                                        Label::new(label)
                                            .size(LabelSize::XSmall)
                                            .color(if selected { Color::Accent } else { Color::Muted }),
                                    )
                            }),
                    )
                    .when(draft.amount > 0, |el| {
                        el.child(
                            div()
                                .id("estimate-clear")
                                .px_2()
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.colors().element_hover))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.session.estimate_draft.amount = 0;
                                    cx.notify();
                                }))
                                .child(Label::new(strings.clear).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(button("estimate-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| this.close_popup(cx))))
                    .child(button("estimate-ok", strings.ok, true).on_click(cx.listener(|this, _, _, cx| {
                        let estimate = this.session.estimate_draft.estimate();
                        this.menu_action(cx, move |store, id| store.set_estimate(id, estimate));
                    }))),
            )
            .into_any_element()
    }

    fn render_link_parent(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if self.session.task.is_none() {
            return div().into_any_element();
        }
        let query = self.link_search.read(cx).text().trim().to_lowercase();
        let list_name = |list_id: Option<Id>| -> SharedString {
            list_id
                .and_then(|id| self.session.lists.iter().find(|l| l.id == id))
                .map_or(strings.inbox.into(), |l| l.name.clone().into())
        };
        let candidates: Vec<&Task> = self
            .session
            .link_parent_candidates
            .iter()
            .filter(|t| query.is_empty() || t.title.to_lowercase().contains(&query))
            .take(30)
            .collect();
        let linked_parent = query.is_empty().then(|| self.session.parent.clone()).flatten();
        popover(cx)
            .w(px(340.))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.dismiss_popup(window, cx);
            }))
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
            .when_some(linked_parent, |el, parent| {
                let parent_title: SharedString = if parent.title.is_empty() {
                    strings.untitled.into()
                } else {
                    parent.title.clone().into()
                };
                let parent_list = list_name(parent.list_id);
                el.child(
                    v_flex()
                        .pt_1()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .child(Label::new(strings.linked_parent).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                        .child(
                            h_flex()
                                .id("link-parent-current")
                                .h(px(32.))
                                .px_2()
                                .gap_2()
                                .items_center()
                                .child(Icon::new(IconName::Circle).size(IconSize::Small).color(Color::Accent))
                                .child(div().flex_1().min_w_0().child(Label::new(parent_title).truncate()))
                                .child(Label::new(parent_list).size(LabelSize::XSmall).color(Color::Muted))
                                .child(
                                    div()
                                        .debug_selector(|| "link-parent-unlink".to_owned())
                                        .child(
                                            IconButton::new("link-parent-unlink-btn", IconName::X)
                                                .size(px(24.))
                                                .icon_size(IconSize::XSmall)
                                                .icon_color(Color::Muted)
                                                .tooltip(Tooltip::text(strings.unlink_parent))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    cx.stop_propagation();
                                                    this.dismiss_popup(window, cx);
                                                    this.unlink_current_parent(cx);
                                                })),
                                        ),
                                ),
                        )
                        .child(div().my_1().child(Divider::horizontal()))
                        .child(
                            div()
                                .px_2()
                                .pb_1()
                                .child(Label::new(strings.link_other_parent).size(LabelSize::XSmall).color(Color::Muted)),
                        ),
                )
            })
            .child(
                v_scroll(
                    "link-parent-list-scroll",
                    v_flex().id("link-parent-list").children(candidates.into_iter().map(|candidate| {
                        let parent = candidate.id;
                        let list = list_name(candidate.list_id);
                        ListItem::new(SharedString::from(format!("link-{parent}")))
                            .start_slot(Icon::new(IconName::Circle).size(IconSize::Small).color(Color::Muted))
                            .child(Label::new(candidate.title.clone()).truncate())
                            .end_slot(Label::new(list).size(LabelSize::XSmall).color(Color::Muted))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.dismiss_popup(window, cx);
                                this.link_current_parent(parent, cx);
                            }))
                    })),
                )
                .max_h(px(320.)),
            )
            .into_any_element()
    }

    fn toggle_completed(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.session.task.as_mut() else { return };
        let completed = !task.is_completed();
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
        cx.emit(TaskUpdated);
        cx.notify();
    }

    fn render_progress_bar(done: usize, total: usize, theme: &theme::Theme) -> AnyElement {
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
                    .bg(theme.colors().border_variant)
                    .child(div().h_full().w(gpui::relative(ratio)).rounded_full().bg(theme.colors().accent)),
            )
            .child(Label::new(format!("{done}/{total}")).size(LabelSize::XSmall).color(Color::Muted))
            .into_any_element()
    }

    fn render_checklist(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let total = self.checklist.len();
        let done_count = self.checklist.iter().filter(|r| r.item.completed).count();
        v_flex()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(total > 0, |el| {
                el.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(div().flex_1().min_w_0().child(Self::render_progress_bar(done_count, total, &theme)))
                        .when(done_count > 0, |bar| {
                            bar.child(
                                IconButton::new("checklist-uncheck-all", IconName::RotateCcw)
                                    .size(px(22.))
                                    .icon_size(IconSize::XSmall)
                                    .icon_color(Color::Muted)
                                    .tooltip(Tooltip::text(strings.checklist_uncheck_all))
                                    .on_click(cx.listener(|this, _, _, cx| this.uncheck_all_checklist_items(cx))),
                            )
                            .child(
                                IconButton::new("checklist-clear-completed", IconName::BrushCleaning)
                                    .size(px(22.))
                                    .icon_size(IconSize::XSmall)
                                    .icon_color(Color::Muted)
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
                h_flex()
                    .id(SharedString::from(format!("item-{id}")))
                    .debug_selector(move || format!("checklist-item-{id}"))
                    .group(row_group.clone())
                    .min_h(px(30.))
                    .items_start()
                    .gap_2()
                    .py_1()
                    .child(
                        div().mt(px(3.)).child(
                            Checkbox::new(SharedString::from(format!("item-checkbox-{id}")))
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
                            .when(done, |el| el.text_color(theme.colors().text_muted).line_through())
                            .child(row.input.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("item-delete-{id}")))
                            .flex_none()
                            .px_1()
                            .text_color(theme.colors().text_muted)
                            .opacity(if crate::touch::active() { 1. } else { 0. })
                            .group_hover(row_group, |s| s.opacity(1.))
                            .hover(|s| s.text_color(theme.status().error))
                            .cursor_pointer()
                            .tooltip(Tooltip::text(strings.delete))
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
        let theme = cx.theme().clone();
        let can_add = self.can_add_subtask();
        let can_edit = self.session.task.as_ref().is_some_and(|t| !t.is_deleted());
        let total = self.session.subtasks.len();
        let done_count = self.session.subtasks.iter().filter(|s| s.is_completed()).count();
        let today = Local::now().date_naive();
        let countdown_dates = self.session.store.preferences().map(|p| p.countdown_dates).unwrap_or(false);
        v_flex()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(total > 0, |el| el.child(Self::render_progress_bar(done_count, total, &theme)))
            .children(self.session.subtasks.iter().map(|subtask| {
                let id = subtask.id;
                let done = subtask.is_completed();
                let group_name = SharedString::from(format!("subtask-row-{id}"));
                let due = subtask
                    .due_date
                    .map(|date| task_rows::row_due_label(date, subtask.due_time, today, strings, countdown_dates));
                div().group(group_name.clone()).child(
                    ListItem::new(SharedString::from(format!("subtask-{id}")))
                        .height(px(30.))
                        .start_slot(
                            Checkbox::new(SharedString::from(format!("subtask-checkbox-{id}")))
                                .size(px(14.))
                                .toggle_state(done)
                                .in_progress(subtask.is_in_progress())
                                .dismissed(subtask.is_wont_do())
                                .tint(task_rows::priority_color(subtask.priority, &theme))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_subtask(id, !done, cx);
                                })),
                        )
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
                                    .color(if done { Color::Muted } else { Color::Default })
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
                                            .color(if overdue && !done { Color::Error } else { Color::Muted }),
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
                                                    .icon_color(Color::Muted)
                                                    .tooltip(Tooltip::text(strings.unlink_parent))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.unlink_subtask(id, cx);
                                                    })),
                                            ),
                                    )
                                })
                                .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall)),
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
                        .child(Icon::new(IconName::Plus).size(IconSize::Small))
                        .child(div().flex_1().child(self.subtask_input.clone())),
                )
            })
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
        let time = |t: Option<chrono::NaiveTime>| -> SharedString { t.map_or(SharedString::default(), |t| crate::calendar::format_time(t, strings).into()) };
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
                    v_scroll(
                        "range-times-scroll",
                        v_flex()
                            .id("range-times")
                            .children(due_picker::time_options().into_iter().map(|t| {
                                ListItem::new(SharedString::from(format!("range-time-{t}")))
                                    .height(px(28.))
                                    .child(Label::new(crate::calendar::format_time(t, strings)).size(LabelSize::Small))
                                    .when(current == Some(t), |item| item.end_slot(check_mark()))
                                    .on_click(cx.listener(move |this, _, _, cx| this.update_draft(cx, |d| d.pick_range_time(t))))
                            })),
                    )
                    .max_h(px(180.))
                    .compact()
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

    fn render_due_popup(&self, strings: &'static Strings, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        self.render_due_popup_in(window.viewport_size() - gpui::size(px(16.), px(16.)), strings, cx)
    }

    fn render_due_popup_in(&self, available: gpui::Size<Pixels>, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let Some(draft) = self.session.due_draft.clone() else { return div().into_any_element() };
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
                        let is_other_endpoint = duration
                            && !is_selected
                            && (draft.date == Some(date) || draft.end_date == Some(date));
                        let in_range = duration
                            && matches!((draft.date, draft.end_date), (Some(s), Some(e)) if date > s && date < e);
                        div().flex_1().flex().justify_center().child(
                            div()
                                .id(SharedString::from(format!("day-{date}")))
                                .debug_selector(move || format!("due-day-{date}"))
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .cursor_pointer()
                                .when(is_selected, |el| el.bg(theme.colors().accent))
                                .when(is_other_endpoint, |el| el.bg(theme.colors().accent.opacity(0.32)))
                                .when(in_range, |el| el.bg(theme.colors().accent.opacity(0.14)))
                                .when(!is_selected && !is_other_endpoint && !in_range && is_today, |el| {
                                    el.bg(theme.colors().accent.opacity(0.18))
                                })
                                .when(!is_selected, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                                .child(Label::new(date.day().to_string()).centered().size(LabelSize::Small).color(
                                    if is_selected {
                                        Color::OnAccent
                                    } else if is_today || is_other_endpoint {
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
            draft.time.map_or(strings.time.into(), |t| crate::calendar::format_time(t, strings).into());
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
                    .child(
                        v_scroll(
                            "time-options-scroll",
                            v_flex()
                                .id("time-options")
                                .children(
                                    std::iter::once(None)
                                        .chain(due_picker::time_options().into_iter().map(Some))
                                        .map(|time| {
                                            let label: SharedString = time.map_or(strings.no_time.into(), |t| crate::calendar::format_time(t, strings).into());
                                            ListItem::new(SharedString::from(format!("time-{label}")))
                                                .height(px(28.))
                                                .child(Label::new(label).size(LabelSize::Small))
                                                .when(draft.time == time, |item| item.end_slot(check_mark()))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.update_draft(cx, |d| d.pick_time(time, Local::now().date_naive()))
                                                }))
                                        })
                                        .collect::<Vec<_>>(),
                                ),
                        )
                        .max_h(px(180.))
                        .compact(),
                    )
                    .into_any_element(),
            ),
            Some(DueMenu::TimeZone) => {
                let query = self.tz_input.read(cx).text().to_owned();
                let options = due_picker::timezone_options(&query);
                Some(
                    v_flex()
                        .gap_1()
                        .child(
                            ListItem::new("tz-local")
                                .height(px(28.))
                                .child(Label::new(strings.timezone_local).size(LabelSize::Small))
                                .when(draft.timezone.is_none() && !draft.is_floating, |item| item.end_slot(check_mark()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_draft(cx, |d| {
                                        d.timezone = None;
                                        d.is_floating = false;
                                        d.menu = None;
                                    })
                                })),
                        )
                        .child(
                            div()
                                .id("tz-floating-tip")
                                .tooltip(Tooltip::text(strings.timezone_floating_hint))
                                .child(
                                    ListItem::new("tz-floating")
                                        .height(px(28.))
                                        .child(Label::new(strings.timezone_floating).size(LabelSize::Small))
                                        .when(draft.is_floating, |item| item.end_slot(check_mark()))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.update_draft(cx, |d| {
                                                d.is_floating = true;
                                                d.menu = None;
                                            })
                                        })),
                                ),
                        )
                        .child(div().py_0p5().child(Divider::horizontal()))
                        .child(
                            h_flex()
                                .h(px(28.))
                                .px_2()
                                .rounded(px(6.))
                                .bg(theme.colors().background)
                                .text_size(px(12.))
                                .child(self.tz_input.clone()),
                        )
                        .child(
                            v_scroll(
                                "tz-options-scroll",
                                v_flex().id("tz-options").children(options.into_iter().map(|(name, offset)| {
                                    let selected = !draft.is_floating && draft.timezone.as_deref() == Some(name.as_str());
                                    let pick_name = name.clone();
                                    ListItem::new(SharedString::from(format!("tz-{name}")))
                                        .height(px(28.))
                                        .child(
                                            h_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .gap_1()
                                                .child(div().flex_1().min_w_0().child(Label::new(name).size(LabelSize::Small).truncate()))
                                                .child(Label::new(offset).size(LabelSize::XSmall).color(Color::Muted)),
                                        )
                                        .when(selected, |item| item.end_slot(check_mark()))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let pick_name = pick_name.clone();
                                            this.update_draft(cx, move |d| {
                                                d.timezone = Some(pick_name);
                                                d.is_floating = false;
                                                d.menu = None;
                                            })
                                        }))
                                })),
                            )
                            .max_h(px(160.))
                            .compact(),
                        )
                        .into_any_element(),
                )
            }
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
                        h_flex()
                            .id("reminder-constant")
                            .h(px(28.))
                            .px_2()
                            .gap_2()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.colors().element_hover))
                            .tooltip(Tooltip::text(strings.annoying_alert_hint))
                            .child(Checkbox::new("reminder-constant-box").size(px(14.)).toggle_state(draft.constant_reminder))
                            .child(Label::new(strings.annoying_alert).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let today = Local::now().date_naive();
                                this.update_draft(cx, |d| {
                                    d.constant_reminder = !d.constant_reminder;
                                    if d.constant_reminder && d.reminders.is_empty() {
                                        d.toggle_reminder(0, today);
                                    }
                                })
                            })),
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
                let extended = due_picker::extended_repeat_options(base, strings, locale);
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
                        .children(extended.into_iter().map(|(rule, label, hint)| {
                            ListItem::new(SharedString::from(format!("repeat-ext-{rule}")))
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
        let (time_menu, tz_menu, reminder_menu, repeat_menu) = match &draft.menu {
            Some(DueMenu::Time) => (menu, None, None, None),
            Some(DueMenu::TimeZone) => (None, menu, None, None),
            Some(DueMenu::Reminder | DueMenu::CustomReminder(_)) => (None, None, menu, None),
            Some(DueMenu::Repeat | DueMenu::Custom(_)) => (None, None, None, menu),
            None => (None, None, None, None),
        };
        let tz_label: SharedString = due_picker::short_timezone_label(draft.timezone.as_deref(), draft.is_floating, strings)
            .unwrap_or_else(|| strings.timezone_local.to_owned())
            .into();
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
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
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
        let content = v_flex()
            .flex_none()
            .gap_2()
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
                    .children(time_menu)
                    .when(draft.time.is_some(), |el| {
                        el.child(row(
                            "due-timezone-row",
                            IconName::Map,
                            tz_label,
                            draft.timezone.is_some() || draft.is_floating,
                            DueMenu::TimeZone,
                            cx,
                        ))
                        .children(tz_menu)
                    }),
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
            .children(repeat_menu);
        popover(cx)
            .w(px(260.).min(available.width.max(px(0.))))
            .max_h(available.height.max(px(0.)))
            .p_2()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.dismiss_popup(window, cx);
            }))
            .child(
                v_scroll(
                    "due-popup-content-scroll",
                    v_flex().id("due-popup-content").child(content),
                )
                .min_h_0()
                .compact(),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .pt_1()
                    .child(button("due-clear", strings.clear, false).on_click(cx.listener(|this, _, window, cx| this.clear_due(window, cx))))
                    .child(button("due-ok", strings.ok, true).debug_selector(|| "due-popup-ok".into())
                        .on_click(cx.listener(|this, _, window, cx| this.apply_due(window, cx)))),
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
                .replace("{time}", &crate::calendar::format_time(at, strings))
                .replace("{date}", &task_rows::format_day(strings.short_date[1], at.date(), strings))
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
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
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
                    crate::calendar::format_time(custom.at, strings),
                    small("reminder-earlier", IconName::Minus)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update_custom_reminder(cx, |c| c.at -= chrono::Duration::minutes(30))
                        }))
                        .into_any_element(),
                    small("reminder-later", IconName::Plus)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update_custom_reminder(cx, |c| c.at += chrono::Duration::minutes(30))
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
                            if !matches!(frequency, Frequency::Monthly | Frequency::Yearly) {
                                rule.lunar = false;
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
                .child(Label::new(strings.weekdays_narrow[i as usize]).size(LabelSize::XSmall).when(on, |l| l.color(Color::OnAccent)))
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
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
        };
        // The original's repeat modes: Due Dates, Completion Date, Specific Dates, Forgetting Curve.
        let mode = h_flex()
            .id("repeat-mode")
            .h(px(26.))
            .px_2()
            .justify_between()
            .rounded(px(5.))
            .border_1()
            .border_color(theme.colors().border)
            .cursor_pointer()
            .child(
                Label::new(if rule.forgetting_curve {
                    strings.repeat_forgetting_curve
                } else if rule.is_specific_dates() {
                    strings.repeat_by_specific
                } else if rule.from_completion {
                    strings.repeat_by_completion
                } else {
                    strings.repeat_by_due
                })
                .size(LabelSize::Small),
            )
            .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
            .on_click(cx.listener(|this, _, _, cx| {
                let today = Local::now().date_naive();
                this.update_draft(cx, |d| {
                    let start = d.date.unwrap_or(today);
                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                        if rule.forgetting_curve {
                            rule.forgetting_curve = false;
                            rule.cycle = 0;
                        } else if rule.is_specific_dates() {
                            rule.dates.clear();
                            rule.forgetting_curve = true;
                            rule.cycle = 0;
                            rule.lunar = false;
                        } else if rule.from_completion {
                            rule.from_completion = false;
                            rule.dates = vec![start];
                            rule.lunar = false;
                        } else {
                            rule.from_completion = true;
                            rule.weekdays = 0;
                            rule.month_days = 0;
                            rule.nth_weekday = None;
                            rule.nth_workday = None;
                            rule.skip_weekends = false;
                            rule.lunar = false;
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
        let skip_holidays = h_flex()
            .id("repeat-skip-holidays")
            .gap_2()
            .cursor_pointer()
            .child(Checkbox::new("repeat-skip-holidays-box").size(px(14.)).toggle_state(rule.skip_holidays))
            .child(Label::new(strings.repeat_skip_holidays).size(LabelSize::Small))
            .on_click(cx.listener(|this, _, _, cx| {
                this.update_draft(cx, |d| {
                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                        rule.skip_holidays = !rule.skip_holidays;
                    }
                })
            }));
        let lunar_toggle = h_flex()
            .id("repeat-lunar")
            .gap_2()
            .cursor_pointer()
            .child(Checkbox::new("repeat-lunar-box").size(px(14.)).toggle_state(rule.lunar))
            .child(Label::new(strings.repeat_lunar).size(LabelSize::Small))
            .on_click(cx.listener(|this, _, _, cx| {
                this.update_draft(cx, |d| {
                    if let Some(DueMenu::Custom(rule)) = d.menu.as_mut() {
                        rule.lunar = !rule.lunar;
                        if rule.lunar {
                            rule.month_days = 0;
                            rule.nth_weekday = None;
                            rule.nth_workday = None;
                        }
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
        if rule.forgetting_curve {
            return v_flex()
                .gap_2()
                .p_2()
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border)
                .child(mode)
                .child(Label::new(strings.repeat_forgetting_curve_hint).size(LabelSize::XSmall).color(Color::Muted))
                .child(
                    Label::new(strings.repeat_forgetting_step.replace("{n}", &(rule.cycle + 1).to_string()))
                        .size(LabelSize::XSmall)
                        .color(Color::Accent),
                )
                .child(skip_weekends)
                .child(skip_holidays)
                .child(actions)
                .into_any_element();
        }
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
            .when(by_due, |el| el.child(skip_holidays))
            .when(by_due && matches!(rule.frequency, Frequency::Monthly | Frequency::Yearly), |el| el.child(lunar_toggle))
            .when(by_due && rule.frequency == Frequency::Weekly, |el| el.child(weekdays))
            .when(by_due && rule.frequency == Frequency::Monthly && !rule.lunar, |el| el.child(self.render_monthly_rule(rule.clone(), strings, cx)))
            .child(actions)
            .into_any_element()
    }

    /// "By Specific Dates", as in the original: a month calendar where each click picks or drops
    /// a date; past days cannot be picked.
    fn render_specific_dates(&self, rule: &Recurrence, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let month = self.session.specific_month.unwrap_or_else(|| calendar::month_start(rule.dates.first().copied().unwrap_or(today)));
        let nav = |id: &'static str, icon: IconName, delta: i32| {
            IconButton::new(id, icon).size(px(22.)).icon_size(IconSize::XSmall).on_click(cx.listener(move |this, _, _, cx| {
                this.session.specific_month = Some(calendar::add_months(month, delta));
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
                        .child(Label::new(date.day().to_string()).centered().size(LabelSize::Small).color(
                            if picked { Color::OnAccent } else if in_month && !past { Color::Default } else { Color::Muted },
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
                .child(Label::new(label).centered().size(LabelSize::XSmall).when(on, |l| l.color(Color::OnAccent)))
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
        let current = self.session.task.as_ref().map(|t| t.priority).unwrap_or_default();
        let items = [
            (Priority::High, "priority-high", strings.priority_high),
            (Priority::Medium, "priority-medium", strings.priority_medium),
            (Priority::Low, "priority-low", strings.priority_low),
            (Priority::None, "priority-none", strings.priority_none),
        ];
        popover(cx)
            .min_w(px(190.))
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

    /// The bottom bar's list menu, as the original's (200 × 240, opening upwards): a search
    /// field, then the Inbox and the lists, the task's own in the accent color; a list with
    /// sections has a chevron, and hovering it shows its sections beside the menu, the task's
    /// section ticked. A click on a list or a section moves the task there.
    fn render_list_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = self.session.task.as_ref().map(|t| (t.list_id, t.section_id));
        let query = self.list_search.read(cx).text().to_owned();
        let submenu_bounds = Rc::new(Cell::new(None::<gpui::Bounds<Pixels>>));
        let outside_submenu = submenu_bounds.clone();
        let rows: Vec<(Option<Id>, SharedString, IconName)> = std::iter::once((None, SharedString::new_static(strings.inbox), IconName::Inbox))
            .chain(self.session.lists.iter().map(|l| (Some(l.id), SharedString::from(l.name.clone()), IconName::List)))
            .filter(|(_, name, _)| list_matches(name, &query))
            .collect();
        popover(cx)
            .debug_selector(|| "task-list-popup".into())
            .w(px(200.))
            .h(px(240.))
            .p_0()
            .on_mouse_down_out(cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                if !outside_submenu.get().is_some_and(|bounds| bounds.contains(&event.position)) {
                    this.dismiss_popup(window, cx);
                }
            }))
            .child(
                h_flex()
                    .h(px(39.))
                    .flex_none()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(Icon::new(IconName::Search).color(Color::Muted))
                    .child(div().flex_1().child(self.list_search.clone())),
            )
            .child(
                v_scroll(
                    "list-menu-rows-scroll",
                    v_flex().id("list-menu-rows").p_1().children(rows.into_iter().map(|(list, name, icon)| {
                let selected = current.is_some_and(|(l, _)| l == list);
                let color = if selected { Color::Accent } else { Color::Default };
                let sections = self
                    .session
                    .list_sections
                    .get(&list)
                    .cloned()
                    .unwrap_or_default();
                let open = self.list_submenu == Some(list) && !sections.is_empty();
                let has_sections = !sections.is_empty();
                let row_id = format!("move-to-{}", list.map_or("inbox".to_owned(), |id| id.to_string()));
                div()
                    .id(SharedString::from(row_id.clone()))
                    .debug_selector(move || row_id)
                    .relative()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.list_submenu != Some(list) {
                            this.list_submenu = Some(list);
                            cx.notify();
                        }
                    }))
                    .child(
                        ListItem::new("row")
                            .height(px(33.))
                            .toggle_state(open)
                            .start_slot(Icon::new(icon).color(color))
                            .child(Label::new(name).color(color).truncate())
                            .when(has_sections, |item| item.end_slot(Icon::new(if self.sticky_style.is_some() { IconName::ChevronLeft } else { IconName::ChevronRight }).size(IconSize::Small).color(Color::Muted)))
                            .on_click(cx.listener(move |this, _, window, cx| this.set_list(list, window, cx))),
                    )
                    .when(open, |el| {
                        let current_section = current.and_then(|(l, s)| if l == list { s } else { None });
                        let submenu_bounds = submenu_bounds.clone();
                        el.child(PopoverAttachment::new(
                            if self.sticky_style.is_some() { Placement::LeftSide } else { Placement::RightSide },
                            popover(cx).debug_selector(|| "task-list-sections".into()).relative().min_w(px(162.))
                            .child(gpui::canvas(move |area, _, _| submenu_bounds.set(Some(area)), |_, _, _, _| {})
                                .absolute().inset_0())
                            .children(sections.into_iter().map(|section| {
                                let section_id = section.id;
                                let ticked = current_section == Some(section_id);
                                let color = if ticked { Color::Accent } else { Color::Default };
                                ListItem::new(SharedString::from(format!("move-to-section-{section_id}")))
                                    .height(px(34.))
                                    .start_slot(Icon::new(IconName::PanelBottom).color(color))
                                    .child(Label::new(section.name).color(color).truncate())
                                    .when(ticked, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if !this.edit_draft(cx, |task, draft| {
                                            task.list_id = list; task.section_id = Some(section_id); draft.list_changed = true;
                                        }) { this.save(cx, |store, task| {
                                            store.set_list(task, list)?;
                                            store.set_section(task, Some(section_id))
                                        }); }
                                        this.dismiss_popup(window, cx);
                                    }))
                            })),
                        ))
                    })
            })),
                )
                .flex_1()
                .min_h_0()
                .compact(),
            )
            .into_any_element()
    }

    fn render_tags_popup(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let strings = i18n::t(cx);
        let Some(draft) = &self.session.tag_draft else { return div().into_any_element() };
        let query = self.tag_query(cx).to_owned();
        let folded = query.to_lowercase();
        let mut names: Vec<&str> = self.session.all_tags.iter().map(|tag| tag.name.as_str()).collect();
        for name in &draft.picked {
            if !names.iter().any(|known| same_tag(known, name)) {
                names.push(name);
            }
        }
        let offer_new = !query.is_empty() && !names.iter().any(|name| same_tag(name, &query));
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div().id(id).flex_1().h(px(28.)).flex().items_center().justify_center()
                .rounded(px(6.)).cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).hover(|s| s.opacity(0.9)))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).hover(|s| s.bg(theme.colors().ghost_element_hover)))
                .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
        };
        popover(cx)
            .debug_selector(|| "task-tag-picker".to_owned())
            .w(px(240.))
            .gap_1()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.dismiss_popup(window, cx);
            }))
            .child(
                h_flex()
                    .h(px(30.))
                    .px_2()
                    .gap_2()
                    .rounded(px(4.))
                    .bg(theme.colors().background)
                    .child(Icon::new(IconName::Search).size(IconSize::Small).color(Color::Muted))
                    .child(div().flex_1().min_w_0().child(self.tag_input.clone())),
            )
            .child(
                v_scroll(
                    "task-tag-options-scroll",
                    v_flex()
                        .id("task-tag-options")
                        .children(names.into_iter().filter(|name| name.to_lowercase().contains(&folded)).map(|name| {
                            let name = name.to_owned();
                            let checked = draft.picked.iter().any(|picked| same_tag(picked, &name));
                            ListItem::new(SharedString::from(format!("tag-option-{name}")))
                                .start_slot(Icon::new(IconName::Tag).size(IconSize::Small).color(self.tag_color(&name, cx)))
                                .child(Label::new(name.clone()).truncate())
                                .when(checked, |item| item.end_slot(check_mark()))
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle_picked_tag(name.clone(), cx)))
                        }))
                        .when(offer_new, |el| el.child(ListItem::new("task-tag-create")
                            .start_slot(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Accent))
                            .child(Label::new(format!("{}: {query}", strings.new_tag)).truncate())
                            .on_click(cx.listener(|this, _, window, cx| this.pick_typed_tag(window, cx))))),
                )
                .max_h(px(200.))
                .compact(),
            )
            .when(draft.save_failed, |el| el.child(Label::new(strings.task_save_failed).color(Color::Error)))
            .child(h_flex().gap_2().p_1()
                .child(button("task-tags-cancel", strings.cancel, false).on_click(cx.listener(|this, _, window, cx| {
                    this.dismiss_popup(window, cx);
                })))
                .child(button("task-tags-ok", strings.ok, true).debug_selector(|| "task-tags-ok".to_owned())
                    .on_click(cx.listener(|this, _, window, cx| this.apply_tags(window, cx)))))
            .into_any_element()
    }

    fn render_tags_row(&self, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .flex_wrap()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // The Estimation (Start Focus → Estimation), which opens it again.
            .children(self.session.task.as_ref().and_then(|t| t.estimate).map(|estimate| {
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
                    .hover(|s| s.bg(theme.colors().accent.opacity(0.3)))
                    .tooltip(Tooltip::text(strings.estimation))
                    .child(Icon::new(IconName::CircleDot).size(IconSize::XSmall).color(Color::Accent))
                    .child(Label::new(text).size(LabelSize::XSmall).color(Color::Accent))
                    .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Estimate, cx)))
            }))
            // As the original's, a tag is a plain chip; a click opens the tag picker, where it is
            // unticked.
            .children(self.task_tags().into_iter().map(|tag| {
                div()
                    .id(SharedString::from(format!("tag-chip-{tag}")))
                    .cursor_pointer()
                    .child(match self.sticky_style {
                        Some(style) => div().h(px(18.)).px(px(6.)).rounded_full()
                            .bg(gpui::rgb(style.foreground()).opacity(0.12))
                            .text_color(gpui::rgb(style.foreground())).text_size(px(11.))
                            .child(tag).into_any_element(),
                        None => {
                            let color = self.tag_color(&tag, cx);
                            Chip::new(tag, color).into_any_element()
                        }
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_tags(Popup::InlineTags, window, cx);
                    }))
            }))
            .child(
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
                    .border_color(theme.colors().accent)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.colors().element_hover))
                    .child(Icon::new(IconName::Plus).size(IconSize::Custom(px(11.))).color(Color::Accent))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_tags(Popup::InlineTags, window, cx);
                    }))
                    .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::InlineTags), |el| {
                        el.child(PopoverAttachment::new(Placement::BelowLeft, self.render_tags_popup(cx)))
                    }),
            )
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

impl TaskDetail {
    pub fn set_sticky_style(&mut self, settings: crate::sticky_settings::StickySettings, cx: &mut Context<Self>) {
        self.sticky_style = Some(settings);
        self.editor_window.inline_popup = None;
        let muted = gpui::rgb(if settings.color >= 9 { 0xbcc5d6 } else { 0x525566 }).into();
        self.notes.update(cx, |input, cx| input.set_placeholder_color(muted, cx));
        cx.notify();
    }
}

impl Render for TaskDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let sticky_foreground: Option<Hsla> = self.sticky_style.map(|style| gpui::rgb(style.foreground()).into());
        let button_hover = sticky_foreground.map_or(theme.colors().element_hover, |color| color.opacity(0.12));
        let muted = sticky_foreground.map_or(Color::Muted, |color| Color::Custom(color.opacity(0.8)));
        let sticky = self.sticky_style.is_some();
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
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .size(px(52.))
                                .rounded_full()
                                .bg(theme.colors().surface_background)
                                .border_1()
                                .border_color(theme.colors().border_variant)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(IconName::SquareCheck).size(IconSize::Large).color(Color::Muted)),
                        )
                        .child(Label::new(strings.select_task).size(LabelSize::Small).color(Color::Muted)),
                );
        };
        let done = task.is_completed();
        let notes_focus = self.notes.focus_handle(cx);
        let notes_editing = notes_focus.is_focused(window);
        let notes_focus_click = notes_focus.clone();
        let today = Local::now().date_naive();
        let preview = self.session.draft.as_ref().map(|_| self.due_preview(task, today));
        let (date, time, repeats, tz_name, is_floating) = preview.as_ref().map_or(
            (task.due_date, task.due_time, task.repeat.is_some(), task.timezone.as_deref(), task.is_floating),
            |due| (due.date, due.time, due.repeat.is_some(), due.timezone.as_deref(), due.is_floating),
        );
        let tz_suffix = if time.is_some() {
            due_picker::short_timezone_label(tz_name, is_floating, strings)
        } else {
            None
        };
        let kind = self.kind_preview(task);
        let (due_text, due_color): (SharedString, Color) = match date {
            Some(date) => {
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
            // A note, as the original's, offers a reminder rather than a due date.
            None => ((if kind == Kind::Note { strings.set_reminder } else { strings.due_date }).into(), muted),
        };
        let in_progress = task.is_in_progress();
        let show_location = self.location_open || task.location.is_some();
        let priority_color = task_rows::priority_color(task.priority, &theme).map_or(Color::Muted, Color::Custom);
        let list_id = self.list_preview(task);
        let list_name: SharedString = list_id
            .and_then(|id| self.session.lists.iter().find(|l| l.id == id))
            .map(|l| l.name.clone().into())
            .unwrap_or_else(|| strings.inbox.into());
        let list_icon = if list_id.is_some() { IconName::List } else { IconName::Inbox };
        let task_id = task.id;
        let drop_color = theme.colors().accent.opacity(0.08);

        panel
            .when(!task.is_deleted(), |el| {
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
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_completed(cx))),
                        )
                        .child(div().w(px(1.)).h(px(14.)).bg(theme.colors().border))
                    })
                    .child(
                        label_button("due-date", IconName::CalendarDays, due_text.clone(), due_color, button_hover)
                            .debug_selector(|| "task-detail-due-date".into())
                            .when(repeats, |el| el.child(Icon::new(IconName::Repeat).size(IconSize::Small).color(due_color)))
                            .on_click(cx.listener(|this, _, window, cx| this.open_popup_from_ui(Popup::Due, window, cx)))
                            .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::Due), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowLeft,
                                    self.render_due_popup(strings, window, cx),
                                ))
                            }),
                    )
                    .when(in_progress, |el| {
                        el.child(
                            h_flex()
                                .id("detail-in-progress-badge")
                                .px_1p5()
                                .py_0p5()
                                .gap_1()
                                .rounded(px(5.))
                                .bg(theme.colors().accent.opacity(0.14))
                                .cursor_pointer()
                                .tooltip(Tooltip::text(strings.clear_in_progress))
                                .child(Icon::new(IconName::Hourglass).size(IconSize::XSmall).color(Color::Accent))
                                .child(Label::new(strings.in_progress).size(LabelSize::XSmall).color(Color::Accent))
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_in_progress(cx))),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .debug_selector(|| "task-detail-priority".to_owned())
                            .relative()
                            .child(
                                // A note's priority shows as the original's "!" in a circle.
                                IconButton::new("priority", if kind == Kind::Note { IconName::CircleAlert } else { IconName::Flag })
                                    .icon_color(priority_color)
                                    .tip(strings.summary_priority, TipSide::Below)
                                    .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Priority, cx))),
                            )
                            .when(self.sticky_style.is_none() && self.session.popup == Some(Popup::Priority), |el| {
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
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            window.focus(&notes_focus, cx);
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
                                        .child(Label::new(parent_title).color(Color::Muted).truncate())
                                        .child(Icon::new(IconName::ChevronRight).size(IconSize::Small).color(Color::Muted)),
                                )
                                .when(!task.is_deleted() && !sticky, |el| {
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
                                    .when(done, |el| el.text_color(theme.colors().text_muted))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .child(self.title.clone()),
                            )
                            .when(!sticky, |title| title.child(
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
                    .child(if !notes_editing && !task.notes.trim().is_empty() {
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
                    .children(self.session.attachment_errors.iter().map(|error| Label::new(error.clone()).color(Color::Error)))
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
                                .child(
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
                                ),
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
                        el.child(div().pt_2().child(Divider::horizontal())).child(self.render_subtasks(strings, cx))
                    })
                    .when(!self.session.comments.is_empty(), |el| el.child(self.render_comments(strings, cx))),
                )
                .track_scroll(&self.body_scroll)
                .flex_1()
                .min_h_0()
                .when_some(self.sticky_style, |el, style| el.color(gpui::rgb(style.foreground()).into())),
            )
            .when(self.session.draft.as_ref().is_some_and(|draft| draft.save_failed), |el| {
                el.child(div().px_5().py_1().child(Label::new(strings.task_save_failed).color(Color::Error)))
            })
            .when(self.session.draft.as_ref().is_some_and(|draft| draft.pending_attachments > 0), |el| {
                el.child(div().px_5().py_1().child(Label::new(strings.attachment_preparing).color(Color::Muted)))
            })
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
            .when_some(self.session.activity.clone(), |el, activity| el.child(self.render_activity(&activity, strings, cx)))
            // Bottom bar: list ……… formatting, comments, more; for a trashed task, as in the
            // original: Restore ……… delete forever.
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
                                label_button("restore-task", IconName::ArchiveRestore, strings.restore.into(), Color::Default, button_hover)
                                    .on_click(cx.listener(|this, _, _, cx| this.restore(cx))),
                            )
                            .child(div().flex_1())
                            .child(
                                IconButton::new("delete-forever", IconName::Trash)
                                    .tooltip(Tooltip::text(strings.delete_forever))
                                    .on_click(cx.listener(|this, _, _, cx| this.delete_forever(cx))),
                            )
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
                                    .text_color(sticky_foreground.unwrap_or(theme.colors().text).opacity(0.68))
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
                                bar.child(
                                    h_flex()
                                        .gap_1()
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .bg(theme.colors().accent.opacity(0.14))
                                        .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent))
                                        .child(Label::new(strings.summary_copied).size(LabelSize::XSmall).color(Color::Accent)),
                                )
                            })
                            .when(!sticky, |bar| {
                                let words = fern_core::markdown::count_words(self.notes.read(cx).text());
                                bar.when(words > 0, |bar| {
                                    bar.child(
                                        Label::new(strings.word_count.replace("{n}", &words.to_string()))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                })
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
    let line_height = px(sticky.map_or(22., |style| style.font_px() + 8.));
    let accent = sticky.map_or(theme.colors().text_accent, |s| gpui::rgb(if s.color >= 9 { 0x95b7ff } else { 0x254c9e }).into());
    let muted = sticky.map_or(theme.colors().text_muted, |s| gpui::rgb(if s.color >= 9 { 0xbcc5d6 } else { 0x525566 }).into());
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
                    h.background_color = Some(theme.colors().element_hover);
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
                    let cb = Checkbox::new(("md-check", ix)).size(px(14.)).toggle_state(done);
                    let cb = if let Some(toggle) = on_toggle_check.clone() {
                        cb.on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            toggle(ix, window, cx);
                        })
                    } else {
                        cb
                    };
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(div().pt(px(4.)).child(cb))
                        .child(body.when(done, |el| el.line_through().text_color(muted)))
                        .into_any_element()
                }
                BlockKind::Quote => div().pl_3().border_l_2().border_color(accent).text_color(muted).child(body).into_any_element(),
                BlockKind::Divider => div().my_1().h(px(1.)).bg(theme.colors().border).into_any_element(),
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
