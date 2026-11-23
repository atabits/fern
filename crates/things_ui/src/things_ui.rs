//! Native Things 3-style frontend for Fern tasks and projects.

pub mod bottom_bar;
pub mod inline_card;
pub mod magic_plus;
pub mod sidebar;
pub mod when_picker;

#[path = "things_ui/mutations.rs"]
mod mutations;
#[path = "things_ui/render.rs"]
mod render;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use chrono::{Datelike, Duration, Local, NaiveDate};
use desktop_common::actions::{DismissTransient, NewTask, OpenWhen, Search};
use desktop_common::text_input::{Changed, Mode, TextInput};
use desktop_common::{calendar, colors, data_version};
pub use fern_core::things::{ProjectProgress, ThingsCounts, ThingsViewKind};
#[cfg(test)]
use fern_core::TaskDraft;
use fern_core::{
    ChecklistItem, Horizon, Id, List, ListFolder, ListSection, ScheduleBucket, Store, Tag, Task,
};
use fern_i18n::{self as i18n, CurrentLocale, Strings};
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, KeyDownEvent, Render,
    ScrollHandle, SharedString, Subscription, Window, div, px,
};
use task_ui::list_dialog::{FolderDialog, ListDialog};
use task_ui::tag_dialog::TagDialog;
use task_ui::task_rows;
use ui::IconName;
use ui::prelude::*;

#[cfg(test)]
use inline_card::{CardAction, CardPopover};
use inline_card::ExpandedCardState;
#[cfg(test)]
use sidebar::{DropDestination, SidebarAction};
use sidebar::SidebarProps;
#[cfg(test)]
use when_picker::{DeadlineAction, DeadlineTarget, WhenAction};
use when_picker::{DeadlinePickerState, WhenPickerState};

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
        /// `None` preserves the task's current project; `Some(None)` clears it; `Some(Some(id))` assigns it.
        list_id: Option<Option<Id>>,
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

#[derive(Clone, Copy, PartialEq, Eq)]
struct GroupsCacheKey {
    version: u64,
    today: NaiveDate,
    strings_ptr: usize,
    view: ThingsViewKind,
    week_start: chrono::Weekday,
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
    groups_version: u64,
    cached_groups: std::cell::RefCell<Option<(GroupsCacheKey, Rc<Vec<TaskGroup>>)>>,
    selected_id: Option<Id>,
    expanded: Option<ExpandedCardState>,
    collapsed_areas: HashSet<Id>,
    show_logged_in_project: bool,
    sidebar_scroll: ScrollHandle,
    list_scroll: ScrollHandle,
    row_bounds: Rc<std::cell::RefCell<HashMap<Id, gpui::Bounds<Pixels>>>>,
    sidebar_add_menu: bool,
    bottom_move_open: bool,
    when_picker: Option<(PopoverAnchor, WhenPickerState)>,
    when_date_error: bool,
    deadline_picker: Option<(PopoverAnchor, DeadlinePickerState)>,
    pending_checklist_focus: Option<Id>,
    project_notes: Option<(Id, Entity<TextInput>, Subscription)>,
    list_dialog: Option<Entity<ListDialog>>,
    folder_dialog: Option<Entity<FolderDialog>>,
    tag_dialog: Option<Entity<TagDialog>>,
    pending_complete: HashSet<Id>,
    active: bool,
    dirty: bool,
}

impl Focusable for ThingsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ThingsView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, window, true, cx)
    }

    pub fn new_inactive(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, window, false, cx)
    }

    fn new_with_active(
        store: Rc<Store>,
        _window: &mut Window,
        initial_active: bool,
        cx: &mut Context<Self>,
    ) -> Self {
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
            groups_version: 0,
            cached_groups: std::cell::RefCell::new(None),
            selected_id: None,
            expanded: None,
            collapsed_areas: HashSet::new(),
            show_logged_in_project: false,
            sidebar_scroll: ScrollHandle::new(),
            list_scroll: ScrollHandle::new(),
            row_bounds: Rc::new(std::cell::RefCell::new(HashMap::new())),
            sidebar_add_menu: false,
            bottom_move_open: false,
            when_picker: None,
            when_date_error: false,
            deadline_picker: None,
            pending_checklist_focus: None,
            project_notes: None,
            list_dialog: None,
            folder_dialog: None,
            tag_dialog: None,
            pending_complete: HashSet::new(),
            active: initial_active,
            dirty: !initial_active,
        };
        if initial_active {
            this.reload(cx);
        }
        cx.observe_global::<data_version::DataVersion>(|this, cx| {
            if data_version::changed_elsewhere(cx.entity_id(), cx) {
                this.sync_external(cx);
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
    pub fn select_view(
        &mut self,
        view: ThingsViewKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view != view {
            self.close_all_popovers();
            self.finish_expanded_card(cx);
            self.view = view;
            self.selected_id = None;
            self.show_logged_in_project = false;
            self.reload(cx);
            window.focus(&self.focus_handle, cx);
        } else if self.dirty {
            self.reload(cx);
        }
    }

    /// Navigates to the task's appropriate view or project and expands its inline card.
    pub fn open_task(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(Some(task)) = self.store.task(id) else {
            if self.dirty {
                self.reload(cx);
            }
            return;
        };
        if task.is_deleted() {
            if self.dirty {
                self.reload(cx);
            }
            return;
        }
        let today = Self::today();
        let target_view = if task.is_closed() {
            ThingsViewKind::Logbook
        } else if let Some(list_id) = task.list_id {
            ThingsViewKind::Project(list_id)
        } else if task.schedule_bucket == ScheduleBucket::Someday
            || matches!(
                task.horizon,
                Some(Horizon::Year(_) | Horizon::Decade(_) | Horizon::Life)
            )
        {
            ThingsViewKind::Someday
        } else if matches!(
            task.horizon,
            Some(Horizon::Week { .. } | Horizon::Month { .. } | Horizon::Quarter { .. })
        ) {
            ThingsViewKind::Anytime
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

        if self.expanded.as_ref().is_some_and(|e| e.task_id != id) {
            self.finish_expanded_card(cx);
        }
        self.view = target_view;
        self.close_all_popovers();
        self.reload(cx);
        self.selected_id = Some(id);
        self.expand_task(id, window, cx);
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if active && self.dirty {
            self.dirty = false;
            self.reload(cx);
        }
    }

    pub fn sync_external(&mut self, cx: &mut Context<Self>) {
        if self.active {
            self.reload(cx);
        } else {
            self.dirty = true;
        }
    }

    /// Reloads cached queries from SQLite and synchronizes any open inline card or project notes.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
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
        let all_tasks = self.store.tasks().unwrap_or_default();
        self.counts = self.store.things_counts_from(&all_tasks, &self.lists, today);
        self.progress = self.store.all_project_progress().unwrap_or_default();
        self.tasks = self
            .store
            .things_tasks_from(all_tasks, &self.lists, &self.view, today);
        self.sections = match self.view {
            ThingsViewKind::Project(list_id) => {
                self.store.list_sections(Some(list_id)).unwrap_or_default()
            }
            _ => Vec::new(),
        };

        self.invalidate_groups();
        let strings = i18n::t(cx);
        let groups = self.groups_rc(today, strings);
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
            let hide_logged = matches!(self.view, ThingsViewKind::Project(_))
                && !self.show_logged_in_project;
            if let Some(task) = self
                .tasks
                .iter()
                .find(|t| t.id == expanded_id && (!hide_logged || !t.is_closed()))
                .cloned()
            {
                let checklist = self.store.checklist(expanded_id).unwrap_or_default();
                self.sync_expanded_card(&task, &checklist, cx);
            } else {
                self.expanded = None;
                self.close_all_popovers();
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
            self.restore_focus(window, cx);
            cx.notify();
            return true;
        }
        if self.has_open_popover() {
            self.close_all_popovers();
            self.restore_focus(window, cx);
            cx.notify();
            return true;
        }
        if self.expanded.is_some() {
            self.finish_expanded_card(cx);
            window.focus(&self.focus_handle, cx);
            cx.notify();
            return true;
        }
        false
    }

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    fn has_open_modal(&self) -> bool {
        self.list_dialog.is_some() || self.folder_dialog.is_some() || self.tag_dialog.is_some()
    }

    fn has_open_popover(&self) -> bool {
        self.sidebar_add_menu
            || self.bottom_move_open
            || self.when_picker.is_some()
            || self.deadline_picker.is_some()
            || self
                .expanded
                .as_ref()
                .is_some_and(|card| card.active_popover.is_some())
    }

    fn restore_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(card) = &self.expanded {
            window.focus(&card.title.focus_handle(cx), cx);
        } else {
            window.focus(&self.focus_handle, cx);
        }
    }

    fn close_all_popovers(&mut self) {
        self.sidebar_add_menu = false;
        self.bottom_move_open = false;
        self.when_picker = None;
        self.when_date_error = false;
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
        if let Some((_, wp)) = &self.when_picker {
            wp.date_input
                .update(cx, |i, cx| i.set_placeholder(strings.things_date_placeholder, cx));
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

    fn scroll_task_into_view(&self, task_id: Id) {
        let viewport = self.list_scroll.bounds();
        if viewport.size.height <= px(0.) {
            return;
        }
        let Some(unscrolled) = self.row_bounds.borrow().get(&task_id).copied() else {
            return;
        };
        let cur = self.list_scroll.offset();
        let top = unscrolled.top() + cur.y;
        let bottom = unscrolled.bottom() + cur.y;
        let mut next_y = cur.y;
        if top < viewport.top() {
            next_y += viewport.top() - top;
        } else if bottom > viewport.bottom() {
            next_y += viewport.bottom() - bottom;
        } else {
            return;
        }
        let max_y = self.list_scroll.max_offset().y.max(px(0.));
        self.list_scroll
            .set_offset(gpui::point(cur.x, next_y.clamp(-max_y, px(0.))));
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
        self.scroll_task_into_view(next);
        cx.notify();
    }

    fn invalidate_groups(&mut self) {
        self.groups_version = self.groups_version.wrapping_add(1);
        self.cached_groups.borrow_mut().take();
    }

    fn groups_rc(&self, today: NaiveDate, strings: &'static Strings) -> Rc<Vec<TaskGroup>> {
        let key = GroupsCacheKey {
            version: self.groups_version,
            today,
            strings_ptr: strings as *const Strings as usize,
            view: self.view,
            week_start: calendar::week_start(),
        };
        if let Some((cached_key, groups)) = self.cached_groups.borrow().as_ref()
            && *cached_key == key
        {
            return Rc::clone(groups);
        }
        let built = Rc::new(self.build_groups(today, strings));
        *self.cached_groups.borrow_mut() = Some((key, Rc::clone(&built)));
        built
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
                    list_id: Some(None),
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
        let min_month_start = horizon + Duration::days(1);
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
            let first = NaiveDate::from_ymd_opt(year, month0 + 1, 1)
                .unwrap_or(min_month_start)
                .max(min_month_start);
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
                    list_id: Some(None),
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
                        start_date: None,
                        bucket,
                        list_id: Some(Some(list.id)),
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
}

impl Render for ThingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(focus_id) = self.pending_checklist_focus.take()
            && let Some(card) = &self.expanded
            && let Some(row) = card.checklist_rows.iter().find(|r| r.id == focus_id)
        {
            window.focus(&row.input.focus_handle(cx), cx);
        }
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
            .on_action(cx.listener(|this, _: &OpenWhen, window, cx| {
                if this.has_open_modal() {
                    return;
                }
                if !this.open_when_for_active_task(window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &NewTask, window, cx| {
                if this.has_open_modal() {
                    return;
                }
                this.create_new_todo_default(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Search, _, cx| {
                if this.has_open_modal() {
                    return;
                }
                cx.emit(ThingsEvent::OpenSearch);
            }))
            .on_action(cx.listener(|this, _: &DismissTransient, window, cx| {
                if !this.dismiss_transient(window, cx) {
                    cx.propagate();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.has_open_modal() {
                    return;
                }
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
                if this.any_input_focused(window, cx)
                    || this.has_open_popover()
                    || !this.focus_handle.is_focused(window)
                {
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
                    "j" if !m.control && !m.alt && !m.platform && !m.shift => {
                        this.move_selection(1, cx);
                        cx.stop_propagation();
                    }
                    "k" if !m.control && !m.alt && !m.platform && !m.shift => {
                        this.move_selection(-1, cx);
                        cx.stop_propagation();
                    }
                    "delete" | "backspace" if !m.control && !m.alt && !m.platform => {
                        if this.expanded.is_none()
                            && let Some(id) = this.selected_id
                        {
                            this.delete_task(id, window, cx);
                            cx.stop_propagation();
                        }
                    }
                    "enter" if !m.control && !m.alt && !m.platform => {
                        if let Some(id) = this.selected_id.or_else(|| this.visible_order.first().copied()) {
                            this.expand_task(id, window, cx);
                            cx.stop_propagation();
                        }
                    }
                    "space" if !m.control && !m.alt && !m.platform => {
                        if let Some(id) = this.selected_id {
                            this.toggle_task_complete(id, window, cx);
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
#[path = "things_ui/tests.rs"]
mod tests;
