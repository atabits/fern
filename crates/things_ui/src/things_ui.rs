//! Native Things 3-style frontend for Fern tasks and projects.

pub mod bottom_bar;
pub mod inline_card;
pub mod magic_plus;
pub mod sidebar;
pub mod when_picker;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use chrono::{Datelike, Duration, Local, NaiveDate};
use desktop_common::actions::{DismissTransient, NewTask, Search};
use desktop_common::text_input::{Changed, Mode, Submitted, TextInput};
use desktop_common::{calendar, colors, completion_mark, data_version, layout, task_ops};
pub use fern_core::things::{ProjectProgress, ThingsCounts, ThingsViewKind};
use fern_core::{
    ChecklistItem, Id, List, ListFolder, ListSection, ScheduleBucket, Store, Tag, Task, TaskDraft,
};
use fern_i18n::{self as i18n, CurrentLocale, Strings};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla,
    KeyDownEvent, Render, ScrollHandle, SharedString, Subscription, Window, div, px,
};
use task_ui::drag::DraggedTask;
use task_ui::due_picker::DueDraft;
use task_ui::list_dialog::{DialogEvent, FolderDialog, ListDialog};
use task_ui::tag_dialog::{TagDialog, TagEvent};
use task_ui::task_rows;
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, EmptyState, Icon, IconButton, IconName, IconSize, Label,
    LabelSize, Tooltip, v_scroll,
};

use bottom_bar::{BottomBarAction, BottomBarProps};
use inline_card::{
    CardAction, CardPopover, ChecklistRowState, CollapsedRowProps, ExpandedCardProps,
    ExpandedCardState,
};
use magic_plus::{DraggedMagicPlus, MagicPlusDropTarget};
use sidebar::{DropDestination, SidebarAction, SidebarProps};
use when_picker::{DeadlineAction, DeadlinePickerState, DeadlineTarget, WhenAction, WhenPickerState};

/// Events emitted by [`ThingsView`] to the hosting workspace shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThingsEvent {
    StartFocus {
        task_id: Id,
        title: String,
        stopwatch: bool,
    },
    OpenSearch,
}

impl EventEmitter<ThingsEvent> for ThingsView {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PopoverAnchor {
    Card,
    BottomBar,
    ProjectHeader,
}

#[derive(Clone)]
enum GroupDrop {
    Schedule {
        start_date: Option<NaiveDate>,
        bucket: ScheduleBucket,
        list_id: Option<Id>,
    },
    Project {
        list_id: Id,
        section_id: Option<Id>,
    },
}

struct TaskGroup {
    id: SharedString,
    title: Option<String>,
    icon: Option<IconName>,
    icon_color: Option<Color>,
    donut: Option<(ProjectProgress, bool, Hsla)>,
    click_project: Option<Id>,
    drop: Option<GroupDrop>,
    tasks: Vec<Task>,
}

/// Root GPUI view for the Things 3 workspace panel.
pub struct ThingsView {
    store: Rc<Store>,
    focus_handle: FocusHandle,
    view: ThingsViewKind,
    tasks: Vec<Task>,
    counts: ThingsCounts,
    lists: Vec<List>,
    list_names: HashMap<Id, String>,
    folders: Vec<ListFolder>,
    tags: Vec<Tag>,
    tag_colors: HashMap<String, Hsla>,
    progress: HashMap<Id, ProjectProgress>,
    sections: Vec<ListSection>,
    visible_order: Vec<Id>,
    selected_id: Option<Id>,
    expanded: Option<ExpandedCardState>,
    collapsed_areas: HashSet<Id>,
    show_logged_in_project: bool,
    sidebar_scroll: ScrollHandle,
    list_scroll: ScrollHandle,
    sidebar_add_menu: bool,
    bottom_move_open: bool,
    when_picker: Option<(PopoverAnchor, WhenPickerState)>,
    deadline_picker: Option<(PopoverAnchor, DeadlinePickerState)>,
    project_notes: Option<(Id, Entity<TextInput>, Subscription)>,
    list_dialog: Option<Entity<ListDialog>>,
    folder_dialog: Option<Entity<FolderDialog>>,
    tag_dialog: Option<Entity<TagDialog>>,
}

impl Focusable for ThingsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ThingsView {
    pub fn new(store: Rc<Store>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            store,
            focus_handle: cx.focus_handle(),
            view: ThingsViewKind::Today,
            tasks: Vec::new(),
            counts: ThingsCounts::default(),
            lists: Vec::new(),
            list_names: HashMap::new(),
            folders: Vec::new(),
            tags: Vec::new(),
            tag_colors: HashMap::new(),
            progress: HashMap::new(),
            sections: Vec::new(),
            visible_order: Vec::new(),
            selected_id: None,
            expanded: None,
            collapsed_areas: HashSet::new(),
            show_logged_in_project: false,
            sidebar_scroll: ScrollHandle::new(),
            list_scroll: ScrollHandle::new(),
            sidebar_add_menu: false,
            bottom_move_open: false,
            when_picker: None,
            deadline_picker: None,
            project_notes: None,
            list_dialog: None,
            folder_dialog: None,
            tag_dialog: None,
        };
        this.reload(cx);
        cx.observe_global::<data_version::DataVersion>(|this, cx| {
            if data_version::changed_elsewhere(cx.entity_id(), cx) {
                this.reload(cx);
            }
        })
        .detach();
        cx.observe_global::<CurrentLocale>(|this, cx| {
            this.sync_locale_placeholders(cx);
            cx.notify();
        })
        .detach();
        this
    }

    pub fn view(&self) -> ThingsViewKind {
        self.view
    }

    pub fn selected_task(&self) -> Option<Id> {
        self.selected_id
    }

    pub fn expanded_task(&self) -> Option<Id> {
        self.expanded.as_ref().map(|e| e.task_id)
    }

    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    /// Switches the active Things smart view, area, or project.
    pub fn select_view(&mut self, view: ThingsViewKind, cx: &mut Context<Self>) {
        if self.view != view {
            self.view = view;
            self.expanded = None;
            self.selected_id = None;
            self.show_logged_in_project = false;
            self.close_all_popovers();
            self.reload(cx);
        }
    }

    /// Navigates to the task's appropriate view or project and expands its inline card.
    pub fn open_task(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(Some(task)) = self.store.task(id) else {
            return;
        };
        if task.is_deleted() {
            return;
        }
        let today = Self::today();
        let target_view = if task.is_closed() {
            ThingsViewKind::Logbook
        } else if let Some(list_id) = task.list_id {
            ThingsViewKind::Project(list_id)
        } else if task.schedule_bucket == ScheduleBucket::Someday {
            ThingsViewKind::Someday
        } else if task.start_date.is_some_and(|d| d <= today)
            || (task.start_date.is_none() && task.due_date.is_some_and(|d| d <= today))
        {
            ThingsViewKind::Today
        } else if task.start_date.is_some_and(|d| d > today)
            || (task.start_date.is_none() && task.due_date.is_some_and(|d| d > today))
        {
            ThingsViewKind::Upcoming
        } else {
            ThingsViewKind::Inbox
        };

        self.view = target_view;
        self.close_all_popovers();
        self.reload(cx);
        self.selected_id = Some(id);
        self.expand_task(id, window, cx);
    }

    /// Reloads cached queries from SQLite and synchronizes any open inline card or project notes.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let today = Self::today();
        self.lists = self.store.lists().unwrap_or_default();
        self.list_names = self
            .lists
            .iter()
            .map(|l| (l.id, l.name.clone()))
            .collect();
        self.folders = self.store.list_folders().unwrap_or_default();
        self.tags = self.store.tags().unwrap_or_default();
        self.tag_colors = self
            .tags
            .iter()
            .filter_map(|tag| {
                colors::parse_color(tag.color.as_deref()).map(|c| (tag.name.to_lowercase(), c))
            })
            .collect();
        self.counts = self.store.things_counts(today).unwrap_or_default();
        self.progress = self.store.all_project_progress().unwrap_or_default();
        self.tasks = self.store.things_tasks(&self.view, today).unwrap_or_default();
        self.sections = match self.view {
            ThingsViewKind::Project(list_id) => {
                self.store.list_sections(Some(list_id)).unwrap_or_default()
            }
            _ => Vec::new(),
        };

        let strings = i18n::t(cx);
        let groups = self.build_groups(today, strings);
        self.visible_order = groups
            .iter()
            .flat_map(|g| g.tasks.iter().map(|t| t.id))
            .collect();

        if let Some(sel) = self.selected_id
            && !self.visible_order.contains(&sel)
        {
            self.selected_id = self.visible_order.first().copied();
        }

        if let Some(expanded_id) = self.expanded.as_ref().map(|e| e.task_id) {
            if let Some(task) = self.tasks.iter().find(|t| t.id == expanded_id).cloned() {
                let checklist = self.store.checklist(expanded_id).unwrap_or_default();
                self.sync_expanded_card(&task, &checklist, cx);
            } else {
                self.expanded = None;
            }
        }

        self.sync_project_notes(cx);
        cx.notify();
    }

    /// Dismisses the topmost transient popover/dialog or collapses an expanded inline card.
    pub fn dismiss_transient(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.list_dialog.take().is_some()
            || self.folder_dialog.take().is_some()
            || self.tag_dialog.take().is_some()
        {
            window.focus(&self.focus_handle, cx);
            cx.notify();
            return true;
        }
        if self.sidebar_add_menu
            || self.bottom_move_open
            || self.when_picker.is_some()
            || self.deadline_picker.is_some()
            || self
                .expanded
                .as_ref()
                .is_some_and(|card| card.active_popover.is_some())
        {
            self.close_all_popovers();
            cx.notify();
            return true;
        }
        if self.expanded.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
            return true;
        }
        false
    }

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    fn close_all_popovers(&mut self) {
        self.sidebar_add_menu = false;
        self.bottom_move_open = false;
        self.when_picker = None;
        self.deadline_picker = None;
        if let Some(card) = &mut self.expanded {
            card.active_popover = None;
        }
    }

    fn active_target_task_id(&self) -> Option<Id> {
        self.expanded
            .as_ref()
            .map(|e| e.task_id)
            .or(self.selected_id)
    }

    fn sync_locale_placeholders(&mut self, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        if let Some((_, input, _)) = &self.project_notes {
            input.update(cx, |i, cx| {
                i.set_placeholder(strings.things_project_notes_placeholder, cx)
            });
        }
        if let Some(card) = &self.expanded {
            card.title
                .update(cx, |i, cx| i.set_placeholder(strings.untitled, cx));
            card.notes.update(cx, |i, cx| {
                i.set_placeholder(strings.things_notes_placeholder, cx)
            });
            card.new_checklist_item
                .update(cx, |i, cx| i.set_placeholder(strings.things_checklist_add, cx));
            card.tag_input
                .update(cx, |i, cx| i.set_placeholder(strings.type_in_tag, cx));
        }
    }

    fn sync_project_notes(&mut self, cx: &mut Context<Self>) {
        let ThingsViewKind::Project(list_id) = self.view else {
            self.project_notes = None;
            return;
        };
        let Some(list) = self.lists.iter().find(|l| l.id == list_id).cloned() else {
            self.project_notes = None;
            return;
        };
        let needs_new = self
            .project_notes
            .as_ref()
            .is_none_or(|(id, _, _)| *id != list_id);
        if needs_new {
            let placeholder = i18n::t(cx).things_project_notes_placeholder;
            let notes_text = list.notes;
            let input = cx.new(|cx| {
                let mut ti = TextInput::new(Mode::MultiLine, placeholder, cx).without_cancel();
                if !notes_text.is_empty() {
                    ti.set_text(notes_text, cx);
                }
                ti
            });
            let sub = cx.subscribe(&input, move |this, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                if let Some(proj) = this.lists.iter_mut().find(|l| l.id == list_id) {
                    proj.notes = text.clone();
                    if this
                        .store
                        .set_list_project_fields(list_id, &text, proj.deadline, proj.completed_at)
                        .is_ok()
                    {
                        data_version::data_changed(cx.entity_id(), cx);
                    }
                }
            });
            self.project_notes = Some((list_id, input, sub));
        } else if let Some((_, input, _)) = &self.project_notes
            && input.read(cx).text() != list.notes
        {
            let text = list.notes;
            input.update(cx, |i, cx| i.set_text(text, cx));
        }
    }

    fn sync_expanded_card(
        &mut self,
        task: &Task,
        checklist: &[ChecklistItem],
        cx: &mut Context<Self>,
    ) {
        let Some(card) = &mut self.expanded else {
            return;
        };
        if card.title.read(cx).text() != task.title {
            let text = task.title.clone();
            card.title.update(cx, |i, cx| i.set_text(text, cx));
        }
        if card.notes.read(cx).text() != task.notes {
            let text = task.notes.clone();
            card.notes.update(cx, |i, cx| i.set_text(text, cx));
        }
        let ids_match = card.checklist_rows.len() == checklist.len()
            && card
                .checklist_rows
                .iter()
                .zip(checklist.iter())
                .all(|(r, item)| r.id == item.id);
        if ids_match {
            for (row, item) in card.checklist_rows.iter_mut().zip(checklist.iter()) {
                row.completed = item.completed;
                row.title_text = item.title.clone();
                if row.input.read(cx).text() != item.title {
                    let t = item.title.clone();
                    row.input.update(cx, |i, cx| i.set_text(t, cx));
                }
            }
        } else {
            let mut new_rows = Vec::with_capacity(checklist.len());
            for item in checklist {
                new_rows.push(Self::build_checklist_row(item, cx));
            }
            card.checklist_rows = new_rows;
        }
    }

    fn build_checklist_row(item: &ChecklistItem, cx: &mut Context<Self>) -> ChecklistRowState {
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
        ChecklistRowState {
            id: item_id,
            completed: item.completed,
            title_text: initial_title,
            input,
            _subscriptions: vec![sub],
        }
    }

    fn expand_task(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        if self.expanded.as_ref().is_some_and(|c| c.task_id == id) {
            if let Some(card) = &self.expanded {
                window.focus(&card.title.focus_handle(cx), cx);
            }
            return;
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
                    data_version::data_changed(cx.entity_id(), cx);
                    cx.notify();
                }
            }),
            cx.subscribe_in(&title, window, |this, _, _: &Submitted, window, cx| {
                this.close_all_popovers();
                this.expanded = None;
                window.focus(&this.focus_handle, cx);
                this.reload(cx);
            }),
            cx.subscribe(&notes, move |this, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                if this.store.set_notes(id, &text).is_ok() {
                    if let Some(t) = this.tasks.iter_mut().find(|t| t.id == id) {
                        t.notes = text;
                    }
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
            _subscriptions: subs,
        });
        cx.notify();
    }

    fn toggle_task_tag(&mut self, task_id: Id, tag_name: &str, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|t| t.id == task_id) else {
            return;
        };
        let mut next = task.tags.clone();
        if let Some(pos) = next.iter().position(|t| t.eq_ignore_ascii_case(tag_name)) {
            next.remove(pos);
        } else {
            next.push(tag_name.to_owned());
        }
        if self.store.set_tags(task_id, &next).is_ok() {
            data_version::data_changed(cx.entity_id(), cx);
            self.reload(cx);
        }
    }

    fn toggle_task_complete(&mut self, id: Id, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        let done = !task.is_closed();
        if done {
            completion_mark::record(id, cx);
        }
        if task_ops::toggle_task_completion(&self.store, &task, done, cx.entity_id(), cx).is_ok() {
            self.reload(cx);
        }
    }

    fn delete_task(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        if self.store.delete_task(id).is_ok() {
            if self.expanded.as_ref().is_some_and(|e| e.task_id == id) {
                self.expanded = None;
                window.focus(&self.focus_handle, cx);
            }
            data_version::data_changed(cx.entity_id(), cx);
            self.reload(cx);
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.visible_order.is_empty() {
            return;
        }
        let next = match self
            .selected_id
            .and_then(|id| self.visible_order.iter().position(|x| *x == id))
        {
            Some(idx) => {
                let len = self.visible_order.len() as isize;
                let clamped = (idx as isize + delta).clamp(0, len - 1) as usize;
                self.visible_order[clamped]
            }
            None => {
                if delta >= 0 {
                    self.visible_order[0]
                } else {
                    self.visible_order[self.visible_order.len() - 1]
                }
            }
        };
        self.selected_id = Some(next);
        cx.notify();
    }

    fn default_drop_target_for_view(&self) -> MagicPlusDropTarget {
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
                    start_date: if first_list.is_none() { Some(today) } else { None },
                    schedule_bucket: ScheduleBucket::None,
                    after_task: None,
                }
            }
        }
    }

    fn create_todo_at(
        &mut self,
        target: MagicPlusDropTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view == ThingsViewKind::Logbook {
            self.view = ThingsViewKind::Inbox;
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
        self.expand_task(created.id, window, cx);
    }

    fn create_new_todo_default(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

        let date_input = cx.new(|cx| {
            let mut ti = TextInput::new(Mode::SingleLine, "YYYY-MM-DD", cx).without_cancel();
            if !initial_text.is_empty() {
                ti.set_text(initial_text, cx);
            }
            ti
        });
        cx.subscribe_in(&date_input, window, move |this, _, Submitted(text), window, cx| {
            let strings = i18n::t(cx);
            if let Some(date) = calendar::parse_date(text, Self::today(), strings) {
                this.apply_when(task_id, Some(date), ScheduleBucket::None, window, cx);
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
        self.when_picker = Some((
            anchor,
            WhenPickerState {
                task_id,
                month: initial_month,
                start_date: task.start_date,
                bucket: task.schedule_bucket,
                date_input,
            },
        ));
        cx.notify();
    }

    fn apply_when(
        &mut self,
        task_id: Id,
        start_date: Option<NaiveDate>,
        bucket: ScheduleBucket,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.store.set_task_schedule(task_id, start_date, bucket).is_ok() {
            data_version::data_changed(cx.entity_id(), cx);
            self.close_all_popovers();
            self.reload(cx);
            if self.expanded.is_none() {
                window.focus(&self.focus_handle, cx);
            }
        }
    }

    fn open_deadline_picker(
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

    fn handle_when_action(
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
            WhenAction::ShiftMonth(delta) => {
                if let Some((_, state)) = &mut self.when_picker {
                    state.month = calendar::add_months(state.month, delta);
                    cx.notify();
                }
            }
            WhenAction::Close => {
                self.close_all_popovers();
                cx.notify();
            }
        }
    }

    fn handle_deadline_action(
        &mut self,
        action: DeadlineAction,
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
                            let time = self
                                .tasks
                                .iter()
                                .find(|t| t.id == id)
                                .and_then(|t| t.due_time)
                                .filter(|_| date.is_some());
                            self.store.set_due(id, date, time)
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
                    if res.is_ok() {
                        data_version::data_changed(cx.entity_id(), cx);
                        self.close_all_popovers();
                        self.reload(cx);
                    }
                }
            }
            DeadlineAction::Close => {
                self.close_all_popovers();
                cx.notify();
            }
        }
    }

    fn handle_sidebar_action(
        &mut self,
        action: SidebarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            SidebarAction::Select(view) => self.select_view(view, cx),
            SidebarAction::ToggleArea(id) => {
                if !self.collapsed_areas.remove(&id) {
                    self.collapsed_areas.insert(id);
                }
                cx.notify();
            }
            SidebarAction::DropTask { task_id, target } => {
                let today = Self::today();
                let res = match target {
                    DropDestination::Inbox => self
                        .store
                        .set_list(task_id, None)
                        .and_then(|_| self.store.set_task_schedule(task_id, None, ScheduleBucket::None)),
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
                    data_version::data_changed(cx.entity_id(), cx);
                    self.reload(cx);
                }
            }
            SidebarAction::ToggleAddMenu => {
                let open = !self.sidebar_add_menu;
                self.close_all_popovers();
                self.sidebar_add_menu = open;
                cx.notify();
            }
            SidebarAction::CloseAddMenu => {
                self.sidebar_add_menu = false;
                cx.notify();
            }
            SidebarAction::NewProject(folder) => {
                self.close_all_popovers();
                let dialog = cx.new(|cx| ListDialog::new(self.store.clone(), None, folder, window, cx));
                cx.subscribe_in(&dialog, window, |this, _, event: &DialogEvent, window, cx| {
                    if let DialogEvent::Saved(id) = event {
                        this.view = ThingsViewKind::Project(*id);
                        data_version::data_changed(cx.entity_id(), cx);
                        this.reload(cx);
                    }
                    this.list_dialog = None;
                    window.focus(&this.focus_handle, cx);
                    cx.notify();
                })
                .detach();
                self.list_dialog = Some(dialog);
                cx.notify();
            }
            SidebarAction::NewArea => {
                self.close_all_popovers();
                let dialog = cx.new(|cx| FolderDialog::new(self.store.clone(), None, window, cx));
                cx.subscribe_in(&dialog, window, |this, _, event: &DialogEvent, window, cx| {
                    if let DialogEvent::FolderSaved(_) = event {
                        data_version::data_changed(cx.entity_id(), cx);
                        this.reload(cx);
                    }
                    this.folder_dialog = None;
                    window.focus(&this.focus_handle, cx);
                    cx.notify();
                })
                .detach();
                self.folder_dialog = Some(dialog);
                cx.notify();
            }
        }
    }

    fn handle_card_action(
        &mut self,
        action: CardAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            CardAction::Select(id) => {
                self.selected_id = Some(id);
                if self.expanded.as_ref().is_some_and(|e| e.task_id != id) {
                    self.expanded = None;
                    self.close_all_popovers();
                }
                window.focus(&self.focus_handle, cx);
                cx.notify();
            }
            CardAction::Expand(id) => self.expand_task(id, window, cx),
            CardAction::Collapse => {
                self.close_all_popovers();
                self.expanded = None;
                window.focus(&self.focus_handle, cx);
                cx.notify();
            }
            CardAction::ToggleComplete(id) => self.toggle_task_complete(id, cx),
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
                        if let Some(task_id) = target_task {
                            this.toggle_task_tag(task_id, name, cx);
                        } else {
                            data_version::data_changed(cx.entity_id(), cx);
                            this.reload(cx);
                        }
                    }
                    this.tag_dialog = None;
                    window.focus(&this.focus_handle, cx);
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
                cx.notify();
            }
            CardAction::MoveTo(list_id) => {
                if let Some(task_id) = self.expanded.as_ref().map(|c| c.task_id)
                    && self.store.set_list(task_id, list_id).is_ok()
                {
                    data_version::data_changed(cx.entity_id(), cx);
                    self.close_all_popovers();
                    self.reload(cx);
                }
            }
            CardAction::MagicDropAfter(anchor_id) => {
                let Some(anchor) = self.tasks.iter().find(|t| t.id == anchor_id) else {
                    return;
                };
                let target = MagicPlusDropTarget {
                    list_id: anchor.list_id,
                    section_id: anchor.section_id,
                    start_date: anchor.start_date,
                    schedule_bucket: anchor.schedule_bucket,
                    after_task: Some((anchor_id, true)),
                };
                self.create_todo_at(target, window, cx);
            }
            CardAction::ReorderAfter { dragged_id, target_id } => {
                let target_task = self.tasks.iter().find(|t| t.id == target_id).cloned();
                let dragged_task = self.tasks.iter().find(|t| t.id == dragged_id).cloned();
                if let (Some(dragged), Some(target)) = (dragged_task, target_task) {
                    if dragged.list_id != target.list_id {
                        let _ = self.store.set_list(dragged_id, target.list_id);
                    }
                    if dragged.section_id != target.section_id {
                        let _ = self.store.set_section(dragged_id, target.section_id);
                    }
                    if dragged.start_date != target.start_date
                        || dragged.schedule_bucket != target.schedule_bucket
                    {
                        let _ = self.store.set_task_schedule(
                            dragged_id,
                            target.start_date,
                            target.schedule_bucket,
                        );
                    }
                    if self.store.move_task(dragged_id, target_id, true).is_ok() {
                        data_version::data_changed(cx.entity_id(), cx);
                        self.reload(cx);
                    }
                }
            }
        }
    }

    fn handle_bottom_bar_action(
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
                cx.notify();
            }
            BottomBarAction::QuickFind => {
                cx.emit(ThingsEvent::OpenSearch);
            }
            BottomBarAction::MoveTo(list_id) => {
                if let Some(task_id) = self.active_target_task_id()
                    && self.store.set_list(task_id, list_id).is_ok()
                {
                    data_version::data_changed(cx.entity_id(), cx);
                    self.close_all_popovers();
                    self.reload(cx);
                }
            }
            BottomBarAction::ClosePopover => {
                self.close_all_popovers();
                cx.notify();
            }
        }
    }

    fn apply_group_drop(
        &mut self,
        task_id: Id,
        drop: GroupDrop,
        cx: &mut Context<Self>,
    ) {
        let res = match drop {
            GroupDrop::Schedule {
                start_date,
                bucket,
                list_id,
            } => {
                let r1 = if let Some(lid) = list_id {
                    self.store.set_list(task_id, Some(lid))
                } else {
                    Ok(())
                };
                r1.and_then(|_| self.store.set_task_schedule(task_id, start_date, bucket))
            }
            GroupDrop::Project {
                list_id,
                section_id,
            } => self
                .store
                .set_list(task_id, Some(list_id))
                .and_then(|_| self.store.set_section(task_id, section_id)),
        };
        if res.is_ok() {
            data_version::data_changed(cx.entity_id(), cx);
            self.reload(cx);
        }
    }

    fn build_groups(&self, today: NaiveDate, strings: &'static Strings) -> Vec<TaskGroup> {
        match self.view {
            ThingsViewKind::Inbox => vec![TaskGroup {
                id: "group-inbox".into(),
                title: None,
                icon: None,
                icon_color: None,
                donut: None,
                click_project: None,
                drop: Some(GroupDrop::Schedule {
                    start_date: None,
                    bucket: ScheduleBucket::None,
                    list_id: None,
                }),
                tasks: self.tasks.clone(),
            }],
            ThingsViewKind::Today => {
                let (evening, daytime): (Vec<Task>, Vec<Task>) = self
                    .tasks
                    .iter()
                    .cloned()
                    .partition(|t| t.schedule_bucket == ScheduleBucket::Evening);
                let mut groups = vec![TaskGroup {
                    id: "group-today-day".into(),
                    title: None,
                    icon: None,
                    icon_color: None,
                    donut: None,
                    click_project: None,
                    drop: Some(GroupDrop::Schedule {
                        start_date: Some(today),
                        bucket: ScheduleBucket::None,
                        list_id: None,
                    }),
                    tasks: daytime,
                }];
                groups.push(TaskGroup {
                    id: "group-today-evening".into(),
                    title: Some(strings.things_today_evening.to_owned()),
                    icon: Some(IconName::Moon),
                    icon_color: Some(Color::Accent),
                    donut: None,
                    click_project: None,
                    drop: Some(GroupDrop::Schedule {
                        start_date: Some(today),
                        bucket: ScheduleBucket::Evening,
                        list_id: None,
                    }),
                    tasks: evening,
                });
                groups
            }
            ThingsViewKind::Upcoming => self.build_upcoming_groups(today, strings),
            ThingsViewKind::Anytime => self.build_project_grouped_tasks(
                &self.tasks,
                None,
                Some(today),
                ScheduleBucket::None,
            ),
            ThingsViewKind::Someday => {
                self.build_project_grouped_tasks(&self.tasks, None, None, ScheduleBucket::Someday)
            }
            ThingsViewKind::Logbook => Self::build_logbook_groups(&self.tasks, today, strings),
            ThingsViewKind::Area(folder_id) => self.build_area_groups(folder_id),
            ThingsViewKind::Project(list_id) => self.build_project_groups(list_id, strings),
        }
    }

    fn build_upcoming_groups(&self, today: NaiveDate, strings: &'static Strings) -> Vec<TaskGroup> {
        let horizon = today + Duration::days(7);
        let mut by_day: Vec<(NaiveDate, Vec<Task>)> = Vec::new();
        let mut by_month: Vec<((i32, u32), Vec<Task>)> = Vec::new();

        let mut sorted = self.tasks.clone();
        sorted.sort_by_key(|t| t.start_date.or(t.due_date).unwrap_or(today + Duration::days(1)));

        for task in sorted {
            let day = task.start_date.or(task.due_date).unwrap_or(today + Duration::days(1));
            if day <= horizon {
                if let Some((_, list)) = by_day.iter_mut().find(|(d, _)| *d == day) {
                    list.push(task);
                } else {
                    by_day.push((day, vec![task]));
                }
            } else {
                let key = (day.year(), day.month0());
                if let Some((_, list)) = by_month.iter_mut().find(|(k, _)| *k == key) {
                    list.push(task);
                } else {
                    by_month.push((key, vec![task]));
                }
            }
        }

        let mut groups = Vec::new();
        for (day, tasks) in by_day {
            let title = if day == today + Duration::days(1) {
                task_rows::relative_day(day, strings.tomorrow, strings)
            } else {
                calendar::day_header(day, today, strings)
            };
            groups.push(TaskGroup {
                id: SharedString::from(format!("group-up-day-{day}")),
                title: Some(title),
                icon: Some(IconName::CalendarDays),
                icon_color: Some(Color::Muted),
                donut: None,
                click_project: None,
                drop: Some(GroupDrop::Schedule {
                    start_date: Some(day),
                    bucket: ScheduleBucket::None,
                    list_id: None,
                }),
                tasks,
            });
        }
        for ((year, month0), tasks) in by_month {
            let title = strings
                .month_title
                .replace("{month}", strings.months[month0 as usize])
                .replace("{yyyy}", &year.to_string());
            let first = NaiveDate::from_ymd_opt(year, month0 + 1, 1).unwrap_or(today + Duration::days(8));
            groups.push(TaskGroup {
                id: SharedString::from(format!("group-up-month-{year}-{month0}")),
                title: Some(title),
                icon: Some(IconName::CalendarRange),
                icon_color: Some(Color::Muted),
                donut: None,
                click_project: None,
                drop: Some(GroupDrop::Schedule {
                    start_date: Some(first),
                    bucket: ScheduleBucket::None,
                    list_id: None,
                }),
                tasks,
            });
        }
        groups
    }

    fn build_project_grouped_tasks(
        &self,
        tasks: &[Task],
        default_title: Option<String>,
        default_start_date: Option<NaiveDate>,
        bucket: ScheduleBucket,
    ) -> Vec<TaskGroup> {
        let mut ungrouped = Vec::new();
        let mut by_list: HashMap<Id, Vec<Task>> = HashMap::new();
        for task in tasks {
            if let Some(lid) = task.list_id {
                by_list.entry(lid).or_default().push(task.clone());
            } else {
                ungrouped.push(task.clone());
            }
        }
        let mut groups = Vec::new();
        if !ungrouped.is_empty() || by_list.is_empty() {
            groups.push(TaskGroup {
                id: "group-ungrouped".into(),
                title: default_title,
                icon: None,
                icon_color: None,
                donut: None,
                click_project: None,
                drop: Some(GroupDrop::Schedule {
                    start_date: default_start_date,
                    bucket,
                    list_id: None,
                }),
                tasks: ungrouped,
            });
        }
        for list in &self.lists {
            if let Some(proj_tasks) = by_list.remove(&list.id) {
                let prog = self.progress.get(&list.id).copied().unwrap_or_default();
                let color = colors::parse_color(list.color.as_deref()).unwrap_or_default();
                groups.push(TaskGroup {
                    id: SharedString::from(format!("group-proj-{}", list.id)),
                    title: Some(list.name.clone()),
                    icon: None,
                    icon_color: None,
                    donut: Some((prog, list.completed_at.is_some(), color)),
                    click_project: Some(list.id),
                    drop: Some(GroupDrop::Schedule {
                        start_date: default_start_date,
                        bucket,
                        list_id: Some(list.id),
                    }),
                    tasks: proj_tasks,
                });
            }
        }
        groups
    }

    fn build_logbook_groups(
        tasks: &[Task],
        today: NaiveDate,
        strings: &'static Strings,
    ) -> Vec<TaskGroup> {
        let mut groups: Vec<(NaiveDate, Vec<Task>)> = Vec::new();
        for task in tasks {
            let day = task
                .completed_at
                .or(task.wont_do_at)
                .and_then(calendar::local_day)
                .unwrap_or(today);
            if let Some((_, list)) = groups.last_mut().filter(|(d, _)| *d == day) {
                list.push(task.clone());
            } else {
                groups.push((day, vec![task.clone()]));
            }
        }
        groups
            .into_iter()
            .map(|(day, tasks)| {
                let title = match (today - day).num_days() {
                    0 => task_rows::relative_day(day, strings.today, strings),
                    1 => task_rows::relative_day(day, strings.yesterday, strings),
                    _ => task_rows::closed_day(day, today, strings),
                };
                TaskGroup {
                    id: SharedString::from(format!("group-logbook-{day}")),
                    title: Some(title),
                    icon: Some(IconName::CircleCheck),
                    icon_color: Some(Color::Success),
                    donut: None,
                    click_project: None,
                    drop: None,
                    tasks,
                }
            })
            .collect()
    }

    fn build_area_groups(&self, folder_id: Id) -> Vec<TaskGroup> {
        let area_lists: Vec<&List> = self
            .lists
            .iter()
            .filter(|l| l.folder == Some(folder_id) && !l.archived && l.completed_at.is_none())
            .collect();
        area_lists
            .into_iter()
            .map(|list| {
                let tasks: Vec<Task> = self
                    .tasks
                    .iter()
                    .filter(|t| t.list_id == Some(list.id))
                    .cloned()
                    .collect();
                let prog = self.progress.get(&list.id).copied().unwrap_or_default();
                let color = colors::parse_color(list.color.as_deref()).unwrap_or_default();
                TaskGroup {
                    id: SharedString::from(format!("group-area-proj-{}", list.id)),
                    title: Some(list.name.clone()),
                    icon: None,
                    icon_color: None,
                    donut: Some((prog, list.completed_at.is_some(), color)),
                    click_project: Some(list.id),
                    drop: Some(GroupDrop::Project {
                        list_id: list.id,
                        section_id: None,
                    }),
                    tasks,
                }
            })
            .collect()
    }

    fn build_project_groups(&self, list_id: Id, strings: &'static Strings) -> Vec<TaskGroup> {
        let open: Vec<Task> = self
            .tasks
            .iter()
            .filter(|t| !t.is_closed())
            .cloned()
            .collect();

        if self.sections.is_empty() {
            return vec![TaskGroup {
                id: SharedString::from(format!("group-project-{list_id}")),
                title: None,
                icon: None,
                icon_color: None,
                donut: None,
                click_project: None,
                drop: Some(GroupDrop::Project {
                    list_id,
                    section_id: None,
                }),
                tasks: open,
            }];
        }

        let mut groups = Vec::new();
        let unsectioned: Vec<Task> = open
            .iter()
            .filter(|t| t.section_id.is_none())
            .cloned()
            .collect();
        if !unsectioned.is_empty() {
            groups.push(TaskGroup {
                id: SharedString::from(format!("group-sec-none-{list_id}")),
                title: Some(strings.not_sectioned.to_owned()),
                icon: None,
                icon_color: None,
                donut: None,
                click_project: None,
                drop: Some(GroupDrop::Project {
                    list_id,
                    section_id: None,
                }),
                tasks: unsectioned,
            });
        }
        for section in &self.sections {
            let sec_tasks: Vec<Task> = open
                .iter()
                .filter(|t| t.section_id == Some(section.id))
                .cloned()
                .collect();
            groups.push(TaskGroup {
                id: SharedString::from(format!("group-sec-{}", section.id)),
                title: Some(section.name.clone()),
                icon: None,
                icon_color: None,
                donut: None,
                click_project: None,
                drop: Some(GroupDrop::Project {
                    list_id,
                    section_id: Some(section.id),
                }),
                tasks: sec_tasks,
            });
        }
        groups
    }

    fn any_input_focused(&self, window: &Window, cx: &App) -> bool {
        if let Some((_, input, _)) = &self.project_notes
            && input.focus_handle(cx).is_focused(window)
        {
            return true;
        }
        if let Some(card) = &self.expanded
            && (card.title.focus_handle(cx).is_focused(window)
                || card.notes.focus_handle(cx).is_focused(window)
                || card.new_checklist_item.focus_handle(cx).is_focused(window)
                || card.tag_input.focus_handle(cx).is_focused(window)
                || card
                    .checklist_rows
                    .iter()
                    .any(|r| r.input.focus_handle(cx).is_focused(window)))
        {
            return true;
        }
        if let Some((_, wp)) = &self.when_picker
            && wp.date_input.focus_handle(cx).is_focused(window)
        {
            return true;
        }
        false
    }

    fn empty_state_spec(&self, strings: &'static Strings) -> (IconName, &'static str) {
        match self.view {
            ThingsViewKind::Inbox => (IconName::Inbox, strings.things_empty_inbox),
            ThingsViewKind::Today => (IconName::Sun, strings.things_empty_today),
            ThingsViewKind::Upcoming => (IconName::CalendarDays, strings.things_empty_upcoming),
            ThingsViewKind::Anytime => (IconName::Layers, strings.things_empty_anytime),
            ThingsViewKind::Someday => (IconName::Archive, strings.things_empty_someday),
            ThingsViewKind::Logbook => (IconName::CircleCheck, strings.things_empty_logbook),
            ThingsViewKind::Project(_) | ThingsViewKind::Area(_) => {
                (IconName::CircleDot, strings.things_empty_project)
            }
        }
    }

    fn render_header(&self, today: NaiveDate, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let theme_colors = theme.colors().clone();
        let status = theme.status().clone();

        let (icon_el, title, subtitle): (AnyElement, SharedString, Option<String>) = match self.view {
            ThingsViewKind::Inbox => (
                Icon::new(IconName::Inbox).size(IconSize::XLarge).color(Color::Custom(status.info)).into_any_element(),
                strings.things_inbox.into(),
                None,
            ),
            ThingsViewKind::Today => (
                Icon::new(IconName::Sun).size(IconSize::XLarge).color(Color::Custom(status.warning)).into_any_element(),
                strings.things_today.into(),
                Some(calendar::day_header(today, today, strings)),
            ),
            ThingsViewKind::Upcoming => (
                Icon::new(IconName::CalendarDays).size(IconSize::XLarge).color(Color::Custom(status.error)).into_any_element(),
                strings.things_upcoming.into(),
                None,
            ),
            ThingsViewKind::Anytime => (
                Icon::new(IconName::Layers).size(IconSize::XLarge).color(Color::Accent).into_any_element(),
                strings.things_anytime.into(),
                None,
            ),
            ThingsViewKind::Someday => (
                Icon::new(IconName::Archive).size(IconSize::XLarge).color(Color::IconMuted).into_any_element(),
                strings.things_someday.into(),
                None,
            ),
            ThingsViewKind::Logbook => (
                Icon::new(IconName::CircleCheck).size(IconSize::XLarge).color(Color::Custom(status.success)).into_any_element(),
                strings.things_logbook.into(),
                None,
            ),
            ThingsViewKind::Area(fid) => {
                let name = self
                    .folders
                    .iter()
                    .find(|f| f.id == fid)
                    .map(|f| f.name.clone())
                    .unwrap_or_else(|| strings.things_areas.to_owned());
                (
                    Icon::new(IconName::Folder).size(IconSize::XLarge).color(Color::Icon).into_any_element(),
                    name.into(),
                    None,
                )
            }
            ThingsViewKind::Project(lid) => {
                let proj = self.lists.iter().find(|l| l.id == lid);
                let name = proj
                    .map(|l| l.name.clone())
                    .unwrap_or_else(|| strings.things_projects.to_owned());
                let prog = self.progress.get(&lid).copied().unwrap_or_default();
                let ring_color = proj
                    .and_then(|l| colors::parse_color(l.color.as_deref()))
                    .unwrap_or(theme_colors.accent);
                let completed = proj.is_some_and(|l| l.completed_at.is_some());
                (
                    sidebar::project_donut(prog, completed, ring_color, px(22.), px(3.)).into_any_element(),
                    name.into(),
                    None,
                )
            }
        };

        let project_controls = if let ThingsViewKind::Project(list_id) = self.view {
            self.lists.iter().find(|l| l.id == list_id).map(|proj| {
                let is_completed = proj.completed_at.is_some();
                let deadline_label = proj
                    .deadline
                    .map(|d| task_rows::due_label(d, None, today, strings).0)
                    .unwrap_or_else(|| strings.things_deadline.to_owned());
                let deadline_open = self
                    .deadline_picker
                    .as_ref()
                    .is_some_and(|(a, _)| *a == PopoverAnchor::ProjectHeader);
                let deadline_pop = deadline_open.then(|| {
                    self.deadline_picker.as_ref().map(|(_, state)| {
                        when_picker::render_deadline_popover(state, today, strings, cx, |this, act, _, cx| {
                            this.handle_deadline_action(act, cx);
                        })
                    })
                }).flatten();

                v_flex()
                    .px_6()
                    .pb_2()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .relative()
                                    .child(
                                        Button::new("things-proj-deadline", deadline_label)
                                            .variant(ButtonVariant::Secondary)
                                            .size(ButtonSize::Sm)
                                            .icon(IconName::Flag)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if this
                                                    .deadline_picker
                                                    .as_ref()
                                                    .is_some_and(|(a, _)| *a == PopoverAnchor::ProjectHeader)
                                                {
                                                    this.close_all_popovers();
                                                    cx.notify();
                                                } else {
                                                    this.open_deadline_picker(
                                                        PopoverAnchor::ProjectHeader,
                                                        DeadlineTarget::Project(list_id),
                                                        cx,
                                                    );
                                                }
                                            })),
                                    )
                                    .when_some(deadline_pop, |el, pop| {
                                        el.child(ui::PopoverAttachment::new(ui::Placement::BelowLeft, pop))
                                    }),
                            )
                            .child(
                                Button::new(
                                    "things-proj-complete",
                                    if is_completed {
                                        strings.things_reopen_project
                                    } else {
                                        strings.things_complete_project
                                    },
                                )
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .icon(if is_completed {
                                    IconName::RotateCcw
                                } else {
                                    IconName::Check
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(p) = this.lists.iter().find(|l| l.id == list_id) {
                                        let next_at = if p.completed_at.is_some() {
                                            None
                                        } else {
                                            Some(chrono::Utc::now().timestamp_millis())
                                        };
                                        if this
                                            .store
                                            .set_list_project_fields(list_id, &p.notes, p.deadline, next_at)
                                            .is_ok()
                                        {
                                            data_version::data_changed(cx.entity_id(), cx);
                                            this.reload(cx);
                                        }
                                    }
                                })),
                            ),
                    )
                    .when_some(self.project_notes.as_ref().map(|(_, i, _)| i.clone()), |col, input| {
                        col.child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded(px(6.))
                                .bg(theme_colors.surface_background.opacity(0.6))
                                .text_size(px(13.))
                                .child(input),
                        )
                    })
                    .into_any_element()
            })
        } else {
            None
        };

        v_flex()
            .flex_none()
            .child(
                h_flex()
                    .mt(layout::header_inset())
                    .h(px(54.))
                    .px_6()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2p5()
                            .child(icon_el)
                            .child(
                                div()
                                    .when(cfg!(test), |el| el.debug_selector(|| "things-view-title".to_owned()))
                                    .child(
                                        Label::new(title)
                                            .size(LabelSize::Large)
                                            .weight(FontWeight::SEMIBOLD),
                                    ),
                            )
                            .when_some(subtitle, |el, sub| {
                                el.child(
                                    Label::new(sub)
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                            }),
                    )
                    .child(
                        IconButton::new("things-header-search", IconName::Search)
                            .size(px(28.))
                            .icon_size(IconSize::Small)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text(strings.section_search))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(ThingsEvent::OpenSearch);
                            })),
                    ),
            )
            .children(project_controls)
            .into_any_element()
    }

    fn render_task_item(&self, task: &Task, today: NaiveDate, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        if let Some(card) = &self.expanded
            && card.task_id == task.id
        {
            let when_pop = self
                .when_picker
                .as_ref()
                .filter(|(a, s)| *a == PopoverAnchor::Card && s.task_id == task.id)
                .map(|(_, state)| {
                    when_picker::render_when_popover(state, today, strings, cx, |this, act, window, cx| {
                        this.handle_when_action(act, window, cx);
                    })
                });
            let deadline_pop = self
                .deadline_picker
                .as_ref()
                .filter(|(a, s)| *a == PopoverAnchor::Card && s.target == DeadlineTarget::Task(task.id))
                .map(|(_, state)| {
                    when_picker::render_deadline_popover(state, today, strings, cx, |this, act, _, cx| {
                        this.handle_deadline_action(act, cx);
                    })
                });

            return inline_card::render_expanded_card(
                ExpandedCardProps {
                    task,
                    state: card,
                    today,
                    lists: &self.lists,
                    folders: &self.folders,
                    all_tags: &self.tags,
                    tag_colors: &self.tag_colors,
                    strings,
                    when_popover: when_pop,
                    deadline_popover: deadline_pop,
                },
                cx,
                |this, act, window, cx| this.handle_card_action(act, window, cx),
            );
        }

        inline_card::render_collapsed_row(
            CollapsedRowProps {
                task,
                view: self.view,
                selected: self.selected_id == Some(task.id),
                today,
                list_names: &self.list_names,
                tag_colors: &self.tag_colors,
                strings,
            },
            cx,
            |this, act, window, cx| this.handle_card_action(act, window, cx),
        )
    }

    fn render_task_pane(&self, today: NaiveDate, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors().clone();
        let accent = colors.accent;
        let groups = self.build_groups(today, strings);
        let active_drag = cx.has_active_drag();

        let logged_tasks: Vec<Task> = match self.view {
            ThingsViewKind::Project(_) => {
                self.tasks.iter().filter(|t| t.is_closed()).cloned().collect()
            }
            _ => Vec::new(),
        };
        let has_any_visible_tasks = groups.iter().any(|g| !g.tasks.is_empty());

        let mut group_elements: Vec<AnyElement> = Vec::new();
        for group in groups {
            let show_evening_when_dragging = group.id.as_ref() == "group-today-evening" && active_drag;
            if group.tasks.is_empty()
                && !show_evening_when_dragging
                && !matches!(self.view, ThingsViewKind::Area(_))
            {
                continue;
            }
            let drop_for_task = group.drop.clone();
            let drop_for_magic = group.drop.clone();

            let header = group.title.map(|title| {
                let click_proj = group.click_project;
                let task_count = group.tasks.len();
                h_flex()
                    .id(SharedString::from(format!("hdr-{}", group.id)))
                    .px_3()
                    .pt_3()
                    .pb_1()
                    .gap_2()
                    .when(click_proj.is_some(), |el| el.cursor_pointer())
                    .when_some(click_proj, |el, pid| {
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            this.select_view(ThingsViewKind::Project(pid), cx);
                        }))
                    })
                    .when_some(group.donut, |el, (prog, done, color)| {
                        let c = if color == Hsla::default() { accent } else { color };
                        el.child(sidebar::project_donut(prog, done, c, px(14.), px(2.2)))
                    })
                    .when_some(group.icon, |el, icon| {
                        el.child(
                            Icon::new(icon)
                                .size(IconSize::Small)
                                .color(group.icon_color.unwrap_or(Color::Muted)),
                        )
                    })
                    .child(
                        Label::new(title)
                            .size(LabelSize::Small)
                            .weight(FontWeight::SEMIBOLD)
                            .color(if group.id.as_ref() == "group-today-evening" {
                                Color::Accent
                            } else {
                                Color::Muted
                            }),
                    )
                    .when(task_count > 0, |el| {
                        el.child(
                            Label::new(task_count.to_string())
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
            });

            let rendered_tasks: Vec<AnyElement> = group
                .tasks
                .iter()
                .map(|t| self.render_task_item(t, today, strings, cx))
                .collect();

            let empty_drop_slot = (group.tasks.is_empty() && (show_evening_when_dragging || matches!(self.view, ThingsViewKind::Area(_)))).then(|| {
                div()
                    .mx_3()
                    .my_1()
                    .h(px(32.))
                    .px_3()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(colors.border_variant)
                    .flex()
                    .items_center()
                    .child(
                        Label::new(strings.things_empty_project)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
            });

            group_elements.push(
                v_flex()
                    .id(group.id)
                    .rounded(px(8.))
                    .when_some(drop_for_task, |el, d| {
                        el.drag_over::<DraggedTask>(move |style, _, _, _| {
                            style.bg(accent.opacity(0.08))
                        })
                        .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                            this.apply_group_drop(dragged.id, d.clone(), cx);
                        }))
                    })
                    .when_some(drop_for_magic, |el, d| {
                        el.drag_over::<DraggedMagicPlus>(move |style, _, _, _| {
                            style.bg(accent.opacity(0.08)).border_1().border_color(accent.opacity(0.4))
                        })
                        .on_drop(cx.listener(move |this, _: &DraggedMagicPlus, window, cx| {
                            let target = match &d {
                                GroupDrop::Schedule {
                                    start_date,
                                    bucket,
                                    list_id,
                                } => MagicPlusDropTarget {
                                    list_id: *list_id,
                                    section_id: None,
                                    start_date: *start_date,
                                    schedule_bucket: *bucket,
                                    after_task: None,
                                },
                                GroupDrop::Project {
                                    list_id,
                                    section_id,
                                } => MagicPlusDropTarget {
                                    list_id: Some(*list_id),
                                    section_id: *section_id,
                                    start_date: None,
                                    schedule_bucket: ScheduleBucket::None,
                                    after_task: None,
                                },
                            };
                            this.create_todo_at(target, window, cx);
                        }))
                    })
                    .children(header)
                    .children(rendered_tasks)
                    .children(empty_drop_slot)
                    .into_any_element(),
            );
        }

        let logged_section = (!logged_tasks.is_empty()).then(|| {
            let open = self.show_logged_in_project;
            let count = logged_tasks.len();
            v_flex()
                .mt_4()
                .child(
                    div()
                        .id("things-logged-toggle")
                        .px_3()
                        .py_1()
                        .cursor_pointer()
                        .rounded(px(6.))
                        .hover(|s| s.bg(colors.ghost_element_hover))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_logged_in_project = !this.show_logged_in_project;
                            cx.notify();
                        }))
                        .child(
                            h_flex()
                                .gap_1p5()
                                .child(
                                    Icon::new(if open {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                                )
                                .child(
                                    Label::new(format!("{} ({count})", strings.things_logged_items))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                        ),
                )
                .when(open, |col| {
                    col.children(
                        logged_tasks
                            .iter()
                            .map(|t| self.render_task_item(t, today, strings, cx)),
                    )
                })
        });

        let selected_task = self
            .active_target_task_id()
            .and_then(|id| self.tasks.iter().find(|t| t.id == id));

        let bottom_when_pop = self
            .when_picker
            .as_ref()
            .filter(|(a, _)| *a == PopoverAnchor::BottomBar)
            .map(|(_, state)| {
                when_picker::render_when_popover(state, today, strings, cx, |this, act, window, cx| {
                    this.handle_when_action(act, window, cx);
                })
            });

        let (empty_icon, empty_msg) = self.empty_state_spec(strings);
        let default_target = self.default_drop_target_for_view();
        let view_weak = cx.entity().downgrade();

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(colors.background)
            .child(self.render_header(today, strings, cx))
            .child(
                v_scroll(
                    "things-main-scroll",
                    v_flex()
                        .id("things-main-list")
                        .flex_1()
                        .px_4()
                        .pb_6()
                        .gap_1()
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.expanded.is_some() {
                                this.dismiss_transient(window, cx);
                            } else {
                                window.focus(&this.focus_handle, cx);
                            }
                        }))
                        .drag_over::<DraggedMagicPlus>(move |style, _, _, _| {
                            style.bg(accent.opacity(0.04))
                        })
                        .on_drop(cx.listener(move |this, _: &DraggedMagicPlus, window, cx| {
                            this.create_todo_at(default_target, window, cx);
                        }))
                        .when(!has_any_visible_tasks && !active_drag && logged_tasks.is_empty(), |col| {
                            col.child(
                                div().flex_1().min_h(px(220.)).child(
                                    EmptyState::new(empty_icon, empty_msg)
                                        .when(self.view != ThingsViewKind::Logbook, |es| {
                                            let view_weak = view_weak.clone();
                                            es.action(
                                                "things-empty-add",
                                                strings.things_new_todo,
                                                move |_, window, cx| {
                                                    let _ = view_weak.update(cx, |this, cx| {
                                                        this.create_new_todo_default(window, cx);
                                                    });
                                                },
                                            )
                                        }),
                                ),
                            )
                        })
                        .when(has_any_visible_tasks || active_drag || matches!(self.view, ThingsViewKind::Area(_)), |col| {
                            col.children(group_elements)
                        })
                        .children(logged_section),
                )
                .track_scroll(&self.list_scroll)
                .flex_1()
                .min_h_0(),
            )
            .child(bottom_bar::render_bottom_bar(
                BottomBarProps {
                    selected_task,
                    lists: &self.lists,
                    folders: &self.folders,
                    strings,
                    when_popover: bottom_when_pop,
                    move_open: self.bottom_move_open,
                },
                cx,
                |this, act, window, cx| this.handle_bottom_bar_action(act, window, cx),
            ))
            .into_any_element()
    }
}

impl Render for ThingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let today = Self::today();
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();

        let sidebar_el = sidebar::render_sidebar(
            SidebarProps {
                active: &self.view,
                counts: self.counts,
                lists: &self.lists,
                folders: &self.folders,
                progress: &self.progress,
                collapsed_areas: &self.collapsed_areas,
                add_menu_open: self.sidebar_add_menu,
                strings,
                scroll_handle: &self.sidebar_scroll,
            },
            cx,
            |this, act, window, cx| this.handle_sidebar_action(act, window, cx),
        );

        let main_el = self.render_task_pane(today, strings, cx);

        div()
            .id("things-root")
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .bg(colors.background)
            .text_color(colors.text)
            .track_focus(&self.focus_handle)
            .key_context("ThingsView")
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let m = &event.keystroke.modifiers;
                if event.keystroke.key == "s" && m.secondary() && !m.shift && !m.alt
                    && let Some(task_id) = this.active_target_task_id()
                {
                    let anchor = if this.expanded.as_ref().is_some_and(|e| e.task_id == task_id) {
                        PopoverAnchor::Card
                    } else {
                        PopoverAnchor::BottomBar
                    };
                    this.open_when_picker(anchor, task_id, window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &NewTask, window, cx| {
                this.create_new_todo_default(window, cx);
            }))
            .on_action(cx.listener(|_, _: &Search, _, cx| {
                cx.emit(ThingsEvent::OpenSearch);
            }))
            .on_action(cx.listener(|this, _: &DismissTransient, window, cx| {
                if !this.dismiss_transient(window, cx) {
                    cx.propagate();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                let m = &event.keystroke.modifiers;
                if key == "escape" && !m.control && !m.alt && !m.platform {
                    if this.dismiss_transient(window, cx) {
                        cx.stop_propagation();
                    }
                    return;
                }
                if m.secondary() && !m.shift && !m.alt {
                    match key {
                        "n" => {
                            this.create_new_todo_default(window, cx);
                            cx.stop_propagation();
                            return;
                        }
                        "f" | "k" => {
                            cx.emit(ThingsEvent::OpenSearch);
                            cx.stop_propagation();
                            return;
                        }
                        _ => {}
                    }
                }
                if this.any_input_focused(window, cx) {
                    return;
                }
                match key {
                    "down" if !m.control && !m.alt && !m.platform => {
                        this.move_selection(1, cx);
                        cx.stop_propagation();
                    }
                    "up" if !m.control && !m.alt && !m.platform => {
                        this.move_selection(-1, cx);
                        cx.stop_propagation();
                    }
                    "enter" if !m.control && !m.alt && !m.platform => {
                        if let Some(id) = this.selected_id.or_else(|| this.visible_order.first().copied()) {
                            this.expand_task(id, window, cx);
                            cx.stop_propagation();
                        }
                    }
                    "space" if !m.control && !m.alt && !m.platform => {
                        if let Some(id) = this.selected_id {
                            this.toggle_task_complete(id, cx);
                            cx.stop_propagation();
                        }
                    }
                    _ => {}
                }
            }))
            .child(sidebar_el)
            .child(main_el)
            .when_some(self.list_dialog.clone(), |el, dialog| el.child(dialog))
            .when_some(self.folder_dialog.clone(), |el, dialog| el.child(dialog))
            .when_some(self.tag_dialog.clone(), |el, dialog| el.child(dialog))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fern_core::ListDraft;
    use gpui::{Modifiers, TestAppContext, size};
    use std::cell::Cell;

    fn setup_env(cx: &mut TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        });
    }

    #[gpui::test]
    fn things_view_construction_navigation_and_open_task(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = ThingsView::today();
        let proj = store
            .add_list(&ListDraft {
                name: "Ship v2".into(),
                ..Default::default()
            })
            .unwrap();
        let inbox_task = store.add_task("Review PR").unwrap();
        let proj_task = store
            .create_task(
                TaskDraft {
                    title: "Write docs".into(),
                    list_id: Some(proj.id),
                    start_date: Some(today),
                    ..Default::default()
                },
                false,
            )
            .unwrap();
        let done_task = store
            .create_task(
                TaskDraft {
                    title: "Released alpha".into(),
                    list_id: Some(proj.id),
                    ..Default::default()
                },
                false,
            )
            .unwrap();
        store.set_completed(done_task.id, true).unwrap();

        setup_env(cx);
        let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
        cx.simulate_resize(size(px(960.), px(640.)));
        cx.run_until_parked();

        cx.update(|_, cx| {
            let v = view.read(cx);
            assert_eq!(v.view(), ThingsViewKind::Today);
            assert_eq!(v.counts.inbox, 1);
            assert_eq!(v.counts.today, 1);
            assert_eq!(v.progress.get(&proj.id).map(|p| (p.completed, p.total)), Some((1, 2)));
        });

        // Navigate via open_task to project task -> opens Project view & expands inline card.
        cx.update(|window, cx| {
            view.update(cx, |v, cx| v.open_task(proj_task.id, window, cx));
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let v = view.read(cx);
            assert_eq!(v.view(), ThingsViewKind::Project(proj.id));
            assert_eq!(v.expanded_task(), Some(proj_task.id));
        });
        let expanded_sel: &'static str = format!("things-expanded-{}", proj_task.id).leak();
        assert!(cx.debug_bounds(expanded_sel).is_some());

        // Navigate via open_task to Inbox task -> switches to Inbox & expands it.
        cx.update(|window, cx| {
            view.update(cx, |v, cx| v.open_task(inbox_task.id, window, cx));
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let v = view.read(cx);
            assert_eq!(v.view(), ThingsViewKind::Inbox);
            assert_eq!(v.expanded_task(), Some(inbox_task.id));
        });
    }

    #[gpui::test]
    fn inline_card_keyboard_expand_schedule_mutation_and_collapse(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = ThingsView::today();
        let task = store
            .create_task(
                TaskDraft {
                    title: "Draft spec".into(),
                    start_date: Some(today),
                    ..Default::default()
                },
                false,
            )
            .unwrap();

        setup_env(cx);
        let (view, cx) = cx.add_window_view(|window, cx| ThingsView::new(store.clone(), window, cx));
        cx.simulate_resize(size(px(960.), px(640.)));
        cx.run_until_parked();

        // Focus root, Down selects task, Enter expands inline card.
        cx.update(|window, cx| {
            let fh = view.read(cx).focus_handle(cx);
            window.focus(&fh, cx);
        });
        cx.simulate_keystrokes("down");
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(view.read(cx).selected_task(), Some(task.id)));

        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(view.read(cx).expanded_task(), Some(task.id)));

        // Typing updates the task title directly in SQLite.
        cx.simulate_input(" v2");
        cx.run_until_parked();
        assert_eq!(store.task(task.id).unwrap().unwrap().title, "Draft spec v2");

        // Ctrl/Cmd+S opens the When picker; clicking "This Evening" mutates schedule_bucket.
        #[cfg(target_os = "macos")]
        cx.simulate_keystrokes("cmd-s");
        #[cfg(not(target_os = "macos"))]
        cx.simulate_keystrokes("ctrl-s");
        cx.run_until_parked();
        cx.update(|_, cx| assert!(view.read(cx).when_picker.is_some()));

        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.handle_when_action(
                    WhenAction::Schedule {
                        start_date: Some(today),
                        bucket: ScheduleBucket::Evening,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();
        let updated = store.task(task.id).unwrap().unwrap();
        assert_eq!(updated.schedule_bucket, ScheduleBucket::Evening);
        assert_eq!(updated.start_date, Some(today));

        // Esc collapses the expanded inline card.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(view.read(cx).expanded_task(), None));
    }

    struct DualThingsView {
        a: Entity<ThingsView>,
        b: Entity<ThingsView>,
    }

    impl Render for DualThingsView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.a.clone())
        }
    }

    #[gpui::test]
    fn data_version_syncs_across_views_and_magic_plus_creates_task(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = ThingsView::today();

        setup_env(cx);
        let (dual, cx) = cx.add_window_view(|window, cx| DualThingsView {
            a: cx.new(|cx| ThingsView::new(store.clone(), window, cx)),
            b: cx.new(|cx| ThingsView::new(store.clone(), window, cx)),
        });
        cx.simulate_resize(size(px(1400.), px(640.)));
        cx.run_until_parked();

        let (view_a, view_b) = cx.update(|_, cx| {
            let d = dual.read(cx);
            (d.a.clone(), d.b.clone())
        });

        let search_count = Rc::new(Cell::new(0usize));
        let search_count_sub = search_count.clone();
        cx.update(|_, cx| {
            cx.subscribe(&view_a, move |_, event, _| {
                if matches!(event, ThingsEvent::OpenSearch) {
                    search_count_sub.set(search_count_sub.get() + 1);
                }
            })
            .detach();
        });

        // Clicking Magic Plus in view_a creates & expands a Today task and notifies view_b via DataVersion.
        let plus_bounds = cx.debug_bounds("things-magic-plus").unwrap();
        cx.simulate_click(plus_bounds.center(), Modifiers::default());
        cx.run_until_parked();

        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].start_date, Some(today));
        cx.update(|_, cx| {
            assert_eq!(view_a.read(cx).expanded_task(), Some(tasks[0].id));
            assert_eq!(view_b.read(cx).tasks().len(), 1);
            assert_eq!(view_b.read(cx).tasks()[0].id, tasks[0].id);
        });

        // Ctrl/Cmd+F emits ThingsEvent::OpenSearch.
        cx.update(|window, cx| {
            let fh = view_a.read(cx).focus_handle(cx);
            window.focus(&fh, cx);
        });
        #[cfg(target_os = "macos")]
        cx.simulate_keystrokes("cmd-f");
        #[cfg(not(target_os = "macos"))]
        cx.simulate_keystrokes("ctrl-f");
        cx.run_until_parked();
        assert_eq!(search_count.get(), 1);
    }
}
