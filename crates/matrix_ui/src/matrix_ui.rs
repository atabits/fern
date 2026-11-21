//! The Eisenhower Matrix: four quadrants with independent rules on lists, tags, dates, priorities,
//! and item kind; cached grouping/sorting, manual per-quadrant ordering, quadrant swapping, full
//! cross-quadrant DnD, floating task detail card, context menu, and keyboard navigation.

mod edit_dialog;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use chrono::{Duration, Local, NaiveDate};
use fern_core::matrix::{
    DropOutcome, MatrixKind, QuadrantRule, build_tag_families, quadrant_of, quadrants_of,
};
use fern_core::{Id, Kind, List, Priority, Store, Task, TaskDraft};
use gpui::{Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Point};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use desktop_common::actions::{
    DeleteMatrixTask, DismissTransient, OpenMatrixTask, SelectNextMatrixTask, SelectPreviousMatrixTask,
    ToggleMatrixTask,
};
use desktop_common::text_input::{Mode, Submitted, TextInput};
use desktop_common::{layout, touch};
use fern_i18n::{self as i18n, Strings};
use task_ui::drag::DraggedTask;
use task_ui::lists_panel;
use task_ui::task_detail::{self, StartFocus, TaskDetail, TaskUpdated};
use task_ui::task_rows::{self, GroupBy, SortBy};

#[cfg(test)]
use desktop_common::keymap;

/// Maximum completed tasks loaded into the matrix when Show Completed is enabled.
const COMPLETED_HISTORY_LIMIT: usize = 100;

/// The user asked to jump to a task in the Tasks section (double-click or context menu).
pub struct OpenTask(pub Id);

/// A quadrant's Group by, Sort by, and Order direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuadrantView {
    pub group: GroupBy,
    pub sort: SortBy,
    pub newest_first: bool,
}

impl Default for QuadrantView {
    /// Default matching the web reference: grouped by Date, sorted by Date ascending.
    fn default() -> Self {
        Self { group: GroupBy::Date, sort: SortBy::Date, newest_first: false }
    }
}

impl QuadrantView {
    fn to_storage(self) -> String {
        format!(
            "{},{},{}",
            self.group.key(),
            self.sort.key(),
            if self.newest_first { "newest" } else { "oldest" }
        )
    }

    fn from_storage(text: &str) -> Self {
        if text.trim().is_empty() {
            return Self::default();
        }
        let mut parts = text.split(',');
        let group = parts.next().map(GroupBy::from_key).unwrap_or(GroupBy::Date);
        let group = match group {
            GroupBy::Custom => GroupBy::List,
            other => other,
        };
        let sort = parts.next().map(SortBy::from_key).unwrap_or(SortBy::Date);
        let newest_first = parts.next() == Some("newest");
        Self { group, sort, newest_first }
    }
}

/// Cached grouped tasks for one quadrant so `render()` does not re-filter and re-sort every frame.
#[derive(Clone, Default)]
struct QuadrantCache {
    open_count: usize,
    /// Open task groups: `(group_label, tasks)`.
    groups: Vec<(String, Vec<Task>)>,
    /// Completed tasks shown in a dedicated collapsible section at the bottom.
    completed: Vec<Task>,
}

/// Payload for dragging a quadrant header to swap two quadrants' positions.
#[derive(Clone)]
struct DraggedQuadrant {
    quadrant: usize,
    title: SharedString,
}

impl Render for DraggedQuadrant {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_3()
            .py_1p5()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().accent)
            .bg(theme.colors().elevated_surface_background)
            .shadow_md()
            .child(Label::new(self.title.clone()).size(LabelSize::Small))
    }
}

enum MatrixNotice {
    SavedElsewhere(SharedString),
    AlreadyInQuadrant,
    DropUnsupported,
    Failed,
}

/// Arranges open tasks of a quadrant into groups, applying manual order (when present) or the
/// chosen sort & direction.
pub fn arrange(
    tasks: Vec<Task>,
    view: QuadrantView,
    lists: &[List],
    today: NaiveDate,
    strings: &Strings,
) -> Vec<(String, Vec<Task>)> {
    arrange_with_manual(tasks, view, lists, today, strings, &[])
}

fn arrange_with_manual(
    mut tasks: Vec<Task>,
    view: QuadrantView,
    lists: &[List],
    today: NaiveDate,
    strings: &Strings,
    manual_order: &[Id],
) -> Vec<(String, Vec<Task>)> {
    if !manual_order.is_empty() {
        let pos: HashMap<Id, usize> = manual_order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        tasks.sort_by_key(|t| pos.get(&t.id).copied().unwrap_or(usize::MAX));
    } else {
        task_rows::sort_tasks(&mut tasks, view.sort);
        let natural_newest = matches!(view.sort, SortBy::Created | SortBy::Modified);
        if natural_newest != view.newest_first {
            tasks.reverse();
        }
    }

    match view.group {
        GroupBy::List | GroupBy::Custom => {
            let mut groups: Vec<(Option<Id>, Vec<Task>)> = Vec::new();
            let mut index: HashMap<Option<Id>, usize> = HashMap::new();
            for task in tasks {
                let at = *index.entry(task.list_id).or_insert_with(|| {
                    groups.push((task.list_id, Vec::new()));
                    groups.len() - 1
                });
                groups[at].1.push(task);
            }
            groups
                .into_iter()
                .map(|(list, tasks)| {
                    let name = list
                        .and_then(|id| lists.iter().find(|l| l.id == id))
                        .map_or_else(|| strings.inbox.to_owned(), |l| l.name.clone());
                    (name, tasks)
                })
                .collect()
        }
        GroupBy::None => {
            if tasks.is_empty() {
                Vec::new()
            } else {
                vec![(String::new(), tasks)]
            }
        }
        other => {
            let mut groups: Vec<(String, Vec<Task>)> = Vec::new();
            for row in task_rows::grouped_rows(&tasks, other, today, false, strings) {
                match row {
                    task_rows::Row::Day { label, .. } => groups.push((label, Vec::new())),
                    task_rows::Row::Task { task, .. } => match groups.last_mut() {
                        Some((_, tasks)) => tasks.push(task),
                        None => groups.push((String::new(), vec![task])),
                    },
                    task_rows::Row::Section { .. } => {}
                }
            }
            groups
        }
    }
}

pub struct MatrixView {
    store: Rc<Store>,
    focus: FocusHandle,
    tasks: Vec<Task>,
    lists: Vec<List>,
    tags: Vec<fern_core::Tag>,
    tag_families: HashMap<String, HashSet<String>>,
    rules: [QuadrantRule; 4],
    views: [QuadrantView; 4],
    /// Display slot order of quadrants 0..4 (supports dragging headers to swap).
    quadrant_order: [usize; 4],
    /// Optional per-quadrant manual task order.
    manual_order: [Vec<Id>; 4],
    /// Cached grouped rows per quadrant, rebuilt on reload/view changes.
    cache: [QuadrantCache; 4],
    /// Collapsed group keys `(quadrant, label)` (and `(quadrant, "__completed__")` for Completed).
    collapsed_groups: HashSet<(usize, String)>,
    expanded_parents: HashSet<(usize, Id)>,
    /// Currently selected task for keyboard navigation.
    selected_task: Option<Id>,
    /// Floating task detail card (lazily initialized on first task click so `new` needs no `Window`).
    detail: Option<Entity<TaskDetail>>,
    detail_task: Option<Id>,
    /// Right-click task context menu.
    task_menu: Option<(Id, Point<Pixels>)>,
    menu_row: Option<u8>,
    menu: Option<usize>,
    pub(crate) editing: Option<(usize, QuadrantRule)>,
    pub(crate) name_input: Entity<TextInput>,
    pub(crate) emoji_picker_open: bool,
    adding: Option<(usize, Entity<TextInput>)>,
    notice: Option<MatrixNotice>,
    header_menu: bool,
    show_completed: bool,
    show_subtasks: bool,
    profiles: Vec<fern_core::MatrixProfile>,
    active_profile_id: Option<Id>,
    profile_menu: bool,
    profile_editor: Option<(Option<Id>, Entity<TextInput>)>,
    confirm_delete_profile: Option<(Id, String)>,
}

impl EventEmitter<OpenTask> for MatrixView {}
impl EventEmitter<StartFocus> for MatrixView {}

impl Focusable for MatrixView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl MatrixView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|cx| TextInput::new(Mode::SingleLine, "", cx).without_cancel());
        cx.subscribe(&name_input, |this, _, _: &Submitted, cx| {
            let typed = this.name_input.read(cx).text().to_owned();
            if QuadrantRule::validate_name(&typed) {
                this.finish_edit(Some(true), cx);
            }
        })
        .detach();
        let mut this = Self {
            store,
            focus: cx.focus_handle(),
            tasks: Vec::new(),
            lists: Vec::new(),
            tags: Vec::new(),
            tag_families: HashMap::new(),
            rules: QuadrantRule::defaults(),
            views: [QuadrantView::default(); 4],
            quadrant_order: [0, 1, 2, 3],
            manual_order: std::array::from_fn(|_| Vec::new()),
            cache: std::array::from_fn(|_| QuadrantCache::default()),
            collapsed_groups: HashSet::new(),
            expanded_parents: HashSet::new(),
            selected_task: None,
            detail: None,
            detail_task: None,
            task_menu: None,
            menu_row: None,
            menu: None,
            editing: None,
            name_input,
            emoji_picker_open: false,
            adding: None,
            notice: None,
            header_menu: false,
            show_completed: false,
            show_subtasks: false,
            profiles: Vec::new(),
            active_profile_id: None,
            profile_menu: false,
            profile_editor: None,
            confirm_delete_profile: None,
        };
        this.reload(cx);
        cx.observe_global::<desktop_common::data_version::DataVersion>(|this, cx| {
            if desktop_common::data_version::changed_elsewhere(cx.entity_id(), cx) {
                this.reload(cx);
            }
        })
        .detach();
        this
    }

    fn profile_setting_key(&self, base: &str) -> String {
        let first_id = self.profiles.first().map(|p| p.id);
        match self.active_profile_id {
            Some(id) if Some(id) != first_id => format!("{base}_{id}"),
            _ => base.to_owned(),
        }
    }

    fn switch_profile(&mut self, id: Id, cx: &mut Context<Self>) {
        self.profile_menu = false;
        if let Err(err) = self.store.set_active_matrix_profile(id) {
            eprintln!("failed to switch matrix profile: {err:#}");
            return;
        }
        self.reload(cx);
    }

    fn open_profile_editor(&mut self, editing: Option<Id>, initial: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.profile_menu = false;
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, initial, cx).without_cancel());
        cx.subscribe(&input, |this, _, _: &Submitted, cx| {
            this.save_profile_editor(cx);
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.profile_editor = Some((editing, input));
        cx.notify();
    }

    fn save_profile_editor(&mut self, cx: &mut Context<Self>) {
        let Some((editing, input)) = self.profile_editor.take() else { return };
        let name = input.read(cx).text().trim().to_owned();
        if name.is_empty() {
            cx.notify();
            return;
        }
        match editing {
            Some(id) => {
                if let Err(err) = self.store.rename_matrix_profile(id, &name) {
                    eprintln!("failed to rename matrix profile: {err:#}");
                } else {
                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                }
            }
            None => match self.store.create_matrix_profile(&name, None) {
                Ok(profile) => {
                    let _ = self.store.set_active_matrix_profile(profile.id);
                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                }
                Err(err) => eprintln!("failed to create matrix profile: {err:#}"),
            },
        }
        self.reload(cx);
    }

    fn delete_profile(&mut self, id: Id, cx: &mut Context<Self>) {
        self.profile_menu = false;
        match self.store.delete_matrix_profile(id) {
            Ok(true) => desktop_common::data_version::data_changed(cx.entity_id(), cx),
            Ok(false) => {}
            Err(err) => eprintln!("failed to delete matrix profile: {err:#}"),
        }
        self.reload(cx);
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.show_completed = self
            .store
            .setting("matrix_show_completed")
            .ok()
            .flatten()
            .is_some_and(|v| v == "1");
        self.show_subtasks = self
            .store
            .setting("matrix_show_subtasks")
            .ok()
            .flatten()
            .is_some_and(|v| v == "1");
        self.profiles = self.store.matrix_profiles().unwrap_or_default();
        self.active_profile_id = self.store.active_matrix_profile_id().ok();
        self.rules = self
            .active_profile_id
            .and_then(|id| self.profiles.iter().find(|p| p.id == id))
            .or_else(|| self.profiles.first())
            .map(|p| p.rules.clone())
            .unwrap_or_else(QuadrantRule::defaults);
        self.tasks = self
            .store
            .matrix_tasks(&self.rules, self.show_completed, COMPLETED_HISTORY_LIMIT)
            .unwrap_or_default();
        self.lists = self.store.lists().unwrap_or_default();
        self.tags = self.store.tags().unwrap_or_default();
        self.tag_families = build_tag_families(&self.tags);

        for i in 0..4 {
            let key = self.profile_setting_key(&format!("matrix_view_{i}"));
            self.views[i] = self
                .store
                .setting(&key)
                .ok()
                .flatten()
                .map(|s| QuadrantView::from_storage(&s))
                .unwrap_or_default();
        }
        self.quadrant_order = [0, 1, 2, 3];
        if let Some(order_str) = self.store.setting(&self.profile_setting_key("matrix_quadrant_order")).ok().flatten() {
            let parsed: Vec<usize> = order_str.split(',').filter_map(|s| s.parse().ok()).filter(|&n| n < 4).collect();
            if parsed.len() == 4 && (0..4).all(|i| parsed.contains(&i)) {
                self.quadrant_order = [parsed[0], parsed[1], parsed[2], parsed[3]];
            }
        }
        for i in 0..4 {
            let key = self.profile_setting_key(&format!("matrix_manual_order_{i}"));
            self.manual_order[i] = self
                .store
                .setting(&key)
                .ok()
                .flatten()
                .map(|s| s.split(',').filter_map(|id| id.parse().ok()).collect())
                .unwrap_or_default();
        }

        self.rebuild_cache(cx);

        // Keep floating detail card synced if open.
        if let (Some(detail), Some(task_id)) = (self.detail.as_ref(), self.detail_task) {
            match self.store.task(task_id) {
                Ok(Some(task)) if !task.is_deleted() => {
                    detail.update(cx, |d, cx| d.refresh(Some(task), cx));
                }
                _ => {
                    self.detail_task = None;
                    detail.update(cx, |d, cx| d.show(None, cx));
                }
            }
        }
        cx.notify();
    }

    fn rebuild_cache(&mut self, cx: &App) {
        let strings = i18n::t(cx);
        let today = Local::now().date_naive();
        let all_ids: HashSet<Id> = self.tasks.iter().map(|t| t.id).collect();

        let mut per_quadrant_open: [Vec<Task>; 4] = std::array::from_fn(|_| Vec::new());
        let mut per_quadrant_done: [Vec<Task>; 4] = std::array::from_fn(|_| Vec::new());

        for task in &self.tasks {
            if !self.show_subtasks && task.parent_id.is_some_and(|p| all_ids.contains(&p)) {
                continue;
            }
            for q in quadrants_of(&self.rules, task, today, &self.tag_families) {
                if task.is_completed() {
                    per_quadrant_done[q].push(task.clone());
                } else {
                    per_quadrant_open[q].push(task.clone());
                }
            }
        }

        for q in 0..4 {
            let open_tasks = std::mem::take(&mut per_quadrant_open[q]);
            let open_count = open_tasks.len();
            let groups = arrange_with_manual(
                open_tasks,
                self.views[q],
                &self.lists,
                today,
                strings,
                &self.manual_order[q],
            );
            let mut completed = std::mem::take(&mut per_quadrant_done[q]);
            completed.sort_by_key(|t| std::cmp::Reverse(t.completed_at.unwrap_or(0)));
            self.cache[q] = QuadrantCache { open_count, groups, completed };
        }
    }

    fn open_detail_card(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(Some(task)) = self.store.task(id) else { return };
        if task.is_deleted() {
            return;
        }
        let detail = match self.detail.clone() {
            Some(existing) => existing,
            None => {
                let store = self.store.clone();
                let entity = cx.new(|cx| TaskDetail::new(store, window, cx));
                cx.subscribe(&entity, |this, _, _: &TaskUpdated, cx| {
                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                    this.reload(cx);
                })
                .detach();
                cx.subscribe_in(&entity, window, |this, _, task_detail::OpenTask(id), window, cx| {
                    this.open_detail_card(*id, window, cx);
                })
                .detach();
                cx.subscribe(&entity, |_, _, StartFocus(id, title, stopwatch), cx| {
                    cx.emit(StartFocus(*id, title.clone(), *stopwatch));
                })
                .detach();
                self.detail = Some(entity.clone());
                entity
            }
        };
        detail.update(cx, |d, cx| d.show(Some(task), cx));
        self.detail_task = Some(id);
        self.selected_task = Some(id);
        self.task_menu = None;
        cx.notify();
    }

    fn close_detail_card(&mut self, cx: &mut Context<Self>) {
        self.detail_task = None;
        if let Some(detail) = self.detail.as_ref() {
            detail.update(cx, |d, cx| d.show(None, cx));
        }
        cx.notify();
    }

    fn start_add(&mut self, quadrant: usize, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.matrix_add_placeholder, cx).without_cancel());
        cx.subscribe(&input, |this, input, _: &desktop_common::text_input::Changed, cx| {
            let text = input.read(cx).text().to_owned();
            let list_names: Vec<String> = this.lists.iter().map(|l| l.name.clone()).collect();
            let recognition = this.store.preferences().map(|p| p.recognition).unwrap_or_default();
            let parsed = fern_core::parse_quick_add_with(&text, Local::now().date_naive(), &list_names, recognition);
            input.update(cx, |input, cx| input.set_highlights(parsed.spans, cx));
        })
        .detach();
        cx.subscribe_in(&input, window, move |this, input, Submitted(title), window, cx| {
            if title.trim().is_empty() {
                return;
            }
            let today = Local::now().date_naive();
            let task = match this.add_quadrant_task(quadrant, title, today) {
                Ok(task) => task,
                Err(err) => {
                    eprintln!("failed to add the task: {err:#}");
                    this.notice = Some(MatrixNotice::Failed);
                    cx.notify();
                    return;
                }
            };
            input.update(cx, |input, cx| input.clear(window, cx));
            desktop_common::data_version::data_changed(cx.entity_id(), cx);
            this.reload(cx);
            this.notice = this.destination_notice(quadrant, &task, today, i18n::t(cx));
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.adding = Some((quadrant, input));
        self.notice = None;
        cx.notify();
    }

    fn destination_notice(&self, quadrant: usize, task: &Task, today: NaiveDate, strings: &Strings) -> Option<MatrixNotice> {
        let shown = self.tasks.iter().any(|visible| visible.id == task.id);
        if shown && self.rules[quadrant].matches_with_tags(task, today, &self.tag_families) {
            return None;
        }
        let actual = shown.then(|| quadrant_of(&self.rules, task, today)).flatten();
        let destination = match actual {
            Some(actual) => self.quadrant_title(actual, strings).1,
            None => task
                .list_id
                .and_then(|id| self.lists.iter().find(|list| list.id == id))
                .map_or_else(|| strings.inbox.into(), |list| list.name.clone().into()),
        };
        Some(MatrixNotice::SavedElsewhere(destination))
    }

    fn add_quadrant_task(&self, quadrant: usize, input: &str, today: NaiveDate) -> anyhow::Result<Task> {
        let preferences = self.store.preferences()?;
        let lists = self.store.lists()?;
        let available = |id: Id| lists.iter().any(|list| list.id == id && !list.archived);
        let list_names: Vec<_> = lists.iter().filter(|list| !list.archived).map(|list| list.name.clone()).collect();
        let mut parsed = fern_core::parse_quick_add_with(input, today, &list_names, preferences.recognition);
        let title = if parsed.title.is_empty() { input.trim() } else { &parsed.title };
        let rule = &self.rules[quadrant];
        let (priority, date) = rule.new_task_defaults(today);
        let preferred = preferences.default_list.filter(|id| available(*id));
        let default_list = if rule.lists.contains(&preferred) {
            preferred
        } else {
            rule.lists.iter().copied().find(|id| id.is_none_or(available)).unwrap_or(preferred)
        };
        let selected_list = parsed.list.and_then(|name| lists.iter().find(|list| list.name == name && !list.archived));
        let list_id = selected_list.map(|list| list.id).or(default_list);
        let list_kind = lists.iter().find(|list| Some(list.id) == list_id).map_or(Kind::Task, |list| list.kind);
        let kind = match rule.kind {
            MatrixKind::NoteOnly => Kind::Note,
            MatrixKind::TaskOnly => Kind::Task,
            MatrixKind::All => list_kind,
        };
        let defaults = &preferences.task_defaults;
        let default_tag = rule.tags.first().or(defaults.tag.as_ref());
        if let Some(tag) = default_tag
            && !rule
                .tags
                .iter()
                .any(|required| parsed.tags.iter().any(|tag| required.to_lowercase() == tag.to_lowercase()))
            && !parsed.tags.iter().any(|existing| existing.to_lowercase() == tag.to_lowercase())
        {
            parsed.tags.push(tag.clone());
        }
        let due_date = parsed
            .due_date
            .filter(|_| parsed.explicit_date)
            .or(date)
            .or_else(|| defaults.date.filter(|_| rule.dates.is_empty()).map(|date| date.date(today)))
            .or(parsed.due_date)
            .or_else(|| parsed.repeat.is_some().then_some(today));
        let reminder = if parsed.due_time.is_some() { defaults.timed_reminder } else { defaults.all_day_reminder };
        self.store.create_task(
            TaskDraft {
                title: title.to_owned(),
                list_id,
                kind,
                priority: parsed.priority.or(priority).unwrap_or(defaults.priority),
                due_date,
                due_time: parsed.due_time,
                repeat: parsed.repeat,
                reminders: reminder.filter(|_| due_date.is_some()).into_iter().collect(),
                tags: parsed.tags,
                ..Default::default()
            },
            !defaults.add_to_bottom,
        )
    }

    fn dismiss_transient(&mut self, _: &DismissTransient, window: &mut Window, cx: &mut Context<Self>) {
        if !self.try_dismiss_transient(window, cx) {
            cx.propagate();
        }
    }

    pub fn try_dismiss_transient(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.confirm_delete_profile.take().is_some()
            || self.profile_editor.take().is_some()
            || std::mem::take(&mut self.profile_menu)
        {
            cx.notify();
            return true;
        }
        if self.emoji_picker_open {
            self.emoji_picker_open = false;
            cx.notify();
            return true;
        }
        if self.editing.is_some() {
            self.finish_edit(None, cx);
        } else if let Some(detail) = self.detail.as_ref()
            && self.detail_task.is_some()
        {
            if detail.update(cx, |d, cx| d.dismiss_popup(window, cx)) {
                return true;
            }
            self.close_detail_card(cx);
        } else if self.task_menu.take().is_some() || self.menu_row.take().is_some() {
            cx.notify();
            return true;
        } else if self.menu.take().is_some()
            || std::mem::take(&mut self.header_menu)
            || self.adding.take().is_some()
            || self.notice.take().is_some()
        {
            cx.notify();
        } else {
            return false;
        }
        let focus = self
            .adding
            .as_ref()
            .map_or_else(|| self.focus.clone(), |(_, input)| input.focus_handle(cx));
        window.focus(&focus, cx);
        true
    }

    #[cfg(debug_assertions)]
    pub fn dev_header(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            "header-menu" => self.header_menu = true,
            "add" => self.start_add(0, window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn toggle_complete(&mut self, id: Id, cx: &mut Context<Self>) {
        let completed = self.tasks.iter().find(|t| t.id == id).is_some_and(Task::is_completed);
        if let Err(err) = self.store.set_completed(id, !completed) {
            eprintln!("failed to toggle completion: {err:#}");
        } else {
            if !completed {
                desktop_common::sound::play_complete(&self.store);
            }
            desktop_common::data_version::data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    fn delete_task(&mut self, id: Id, cx: &mut Context<Self>) {
        if let Err(err) = self.store.delete_task(id) {
            eprintln!("failed to delete task: {err:#}");
        } else {
            if self.selected_task == Some(id) {
                self.selected_task = None;
            }
            if self.detail_task == Some(id) {
                self.close_detail_card(cx);
            }
            desktop_common::data_version::data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    fn default_title(quadrant: usize, strings: &Strings) -> &'static str {
        [
            strings.matrix_urgent_important,
            strings.matrix_not_urgent_important,
            strings.matrix_urgent_unimportant,
            strings.matrix_not_urgent_unimportant,
        ][quadrant]
    }

    pub(crate) fn quadrant_title(&self, quadrant: usize, strings: &Strings) -> (&'static str, SharedString) {
        let name = &self.rules[quadrant].name;
        let title = if name.trim().is_empty() {
            Self::default_title(quadrant, strings).into()
        } else {
            name.clone().into()
        };
        (["I", "II", "III", "IV"][quadrant], title)
    }

    /// Exact quadrant accent colors matching the web reference:
    /// Q1 `#FF5F68`, Q2 `#FFB000`, Q3 `#4772FA`, Q4 `#0CCE9C`.
    pub(crate) fn quadrant_color(quadrant: usize) -> Hsla {
        match quadrant {
            0 => gpui::rgb(0xFF5F68).into(),
            1 => gpui::rgb(0xFFB000).into(),
            2 => gpui::rgb(0x4772FA).into(),
            _ => gpui::rgb(0x0CCE9C).into(),
        }
    }

    /// Applies full cross-quadrant drop adjustments (priority, date, list, tag, kind) and surfaces
    /// feedback toasts when already matching or unsupported.
    fn drop_on(&mut self, id: Id, quadrant: usize, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        let today = Local::now().date_naive();
        let available_lists: Vec<Id> = self.lists.iter().filter(|l| !l.archived).map(|l| l.id).collect();
        match self.rules[quadrant].drop_changes(&task, today, &available_lists, &self.tag_families) {
            DropOutcome::AlreadyMatches => {
                self.notice = Some(MatrixNotice::AlreadyInQuadrant);
                cx.notify();
            }
            DropOutcome::Unsupported => {
                self.notice = Some(MatrixNotice::DropUnsupported);
                cx.notify();
            }
            DropOutcome::Apply(patch) => {
                let mut ok = true;
                if let Some(kind) = patch.kind
                    && let Err(err) = self.store.set_kind(id, kind)
                {
                    eprintln!("failed to change task kind: {err:#}");
                    ok = false;
                }
                if let Some(priority) = patch.priority
                    && let Err(err) = self.store.set_priority(id, priority)
                {
                    eprintln!("failed to change priority: {err:#}");
                    ok = false;
                }
                if let Some(due_date) = patch.due_date {
                    let due_time = due_date.and(task.due_time);
                    if let Err(err) = self.store.set_due(id, due_date, due_time) {
                        eprintln!("failed to change due date: {err:#}");
                        ok = false;
                    }
                }
                if let Some(list_id) = patch.list_id
                    && let Err(err) = self.store.set_list(id, list_id)
                {
                    eprintln!("failed to move task list: {err:#}");
                    ok = false;
                }
                if let Some(new_tag) = patch.add_tag {
                    let mut tags = task.tags.clone();
                    if !tags.iter().any(|t| t.to_lowercase() == new_tag.to_lowercase()) {
                        tags.push(new_tag);
                        if let Err(err) = self.store.set_tags(id, &tags) {
                            eprintln!("failed to add tag: {err:#}");
                            ok = false;
                        }
                    }
                }
                if ok {
                    self.notice = None;
                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                } else {
                    self.notice = Some(MatrixNotice::Failed);
                }
                self.reload(cx);
            }
        }
    }

    /// Reorders `dragged_id` before `target_id` inside `quadrant`, persisting per-quadrant manual order.
    fn drop_task_on_task(&mut self, dragged_id: Id, target_id: Id, quadrant: usize, cx: &mut Context<Self>) {
        if dragged_id == target_id {
            return;
        }
        let in_quadrant = self.cache[quadrant]
            .groups
            .iter()
            .flat_map(|(_, tasks)| tasks.iter().map(|t| t.id))
            .any(|id| id == dragged_id);
        if !in_quadrant {
            self.drop_on(dragged_id, quadrant, cx);
            return;
        }
        let mut order: Vec<Id> = self.cache[quadrant]
            .groups
            .iter()
            .flat_map(|(_, tasks)| tasks.iter().map(|t| t.id))
            .collect();
        if let Some(from) = order.iter().position(|&id| id == dragged_id) {
            order.remove(from);
            let to = order.iter().position(|&id| id == target_id).unwrap_or(order.len());
            order.insert(to, dragged_id);
            let encoded = order.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
            self.manual_order[quadrant] = order;
            let _ = self.store.set_setting(&self.profile_setting_key(&format!("matrix_manual_order_{quadrant}")), &encoded);
            self.rebuild_cache(cx);
            cx.notify();
        }
    }

    fn reset_manual_order(&mut self, quadrant: usize, cx: &mut Context<Self>) {
        self.manual_order[quadrant].clear();
        let _ = self.store.delete_setting(&self.profile_setting_key(&format!("matrix_manual_order_{quadrant}")));
        self.menu = None;
        self.rebuild_cache(cx);
        cx.notify();
    }

    fn swap_quadrants(&mut self, from_q: usize, to_q: usize, cx: &mut Context<Self>) {
        if from_q == to_q {
            return;
        }
        if let (Some(a), Some(b)) = (
            self.quadrant_order.iter().position(|&q| q == from_q),
            self.quadrant_order.iter().position(|&q| q == to_q),
        ) {
            self.quadrant_order.swap(a, b);
            let encoded = self.quadrant_order.map(|q| q.to_string()).join(",");
            let _ = self.store.set_setting(&self.profile_setting_key("matrix_quadrant_order"), &encoded);
            cx.notify();
        }
    }

    fn start_edit(&mut self, quadrant: usize, cx: &mut Context<Self>) {
        self.menu = None;
        self.emoji_picker_open = false;
        let rule = self.rules[quadrant].clone();
        let (_, title) = self.quadrant_title(quadrant, i18n::t(cx));
        self.name_input.update(cx, |input, cx| input.set_text(title, cx));
        self.editing = Some((quadrant, rule));
        cx.notify();
    }

    pub(crate) fn finish_edit(&mut self, save: Option<bool>, cx: &mut Context<Self>) {
        self.emoji_picker_open = false;
        if let (Some(save), Some((quadrant, mut rule))) = (save, self.editing.take()) {
            let result = if save {
                let typed = self.name_input.read(cx).text().trim().to_owned();
                if !QuadrantRule::validate_name(&typed) {
                    self.editing = Some((quadrant, rule));
                    cx.notify();
                    return;
                }
                rule.name = if typed == Self::default_title(quadrant, i18n::t(cx)) {
                    String::new()
                } else {
                    typed
                };
                match self.active_profile_id {
                    Some(pid) => self.store.save_matrix_profile_rule(pid, quadrant, Some(&rule)),
                    None => self.store.save_matrix_rule(quadrant, Some(&rule)),
                }
            } else {
                match self.active_profile_id {
                    Some(pid) => self.store.save_matrix_profile_rule(pid, quadrant, None),
                    None => self.store.save_matrix_rule(quadrant, None),
                }
            };
            if let Err(err) = result {
                eprintln!("failed to save the quadrant: {err:#}");
            } else {
                desktop_common::data_version::data_changed(cx.entity_id(), cx);
            }
        }
        self.reload(cx);
    }

    fn visible_open_task_ids(&self) -> Vec<Id> {
        let mut ids = Vec::new();
        for &q in &self.quadrant_order {
            for (label, tasks) in &self.cache[q].groups {
                if !label.is_empty() && self.collapsed_groups.contains(&(q, label.clone())) {
                    continue;
                }
                ids.extend(tasks.iter().map(|t| t.id));
            }
        }
        ids
    }

    fn select_relative_task(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids = self.visible_open_task_ids();
        if ids.is_empty() {
            return;
        }
        let next = match self.selected_task.and_then(|sel| ids.iter().position(|&id| id == sel)) {
            Some(idx) => ids[(idx as isize + delta).clamp(0, ids.len() as isize - 1) as usize],
            None => ids[0],
        };
        self.selected_task = Some(next);
        cx.notify();
    }

    fn render_view_rows(&self, quadrant: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let view = self.views[quadrant];
        let has_manual = !self.manual_order[quadrant].is_empty();
        let note_only = self.rules[quadrant].kind == MatrixKind::NoteOnly;
        let is_time_sort = matches!(view.sort, SortBy::Date | SortBy::Created | SortBy::Modified);
        let order_labels = if is_time_sort {
            strings.order_options
        } else {
            [strings.matrix_order_asc, strings.matrix_order_desc]
        };

        type ViewRow = (u8, IconName, &'static str, &'static str, Vec<(&'static str, QuadrantView)>);
        let mut sort_choices: Vec<SortBy> = vec![SortBy::Date, SortBy::Created, SortBy::Modified, SortBy::Title, SortBy::Tag];
        if !note_only {
            sort_choices.push(SortBy::Priority);
        }

        let mut rows: Vec<ViewRow> = vec![
            (
                0,
                IconName::LayoutGrid,
                strings.group_by,
                view.group.label(strings),
                [GroupBy::Date, GroupBy::List, GroupBy::Created, GroupBy::Tag, GroupBy::Priority, GroupBy::None]
                    .map(|group| (group.label(strings), QuadrantView { group, ..view }))
                    .to_vec(),
            ),
            (
                1,
                IconName::ArrowDownUp,
                strings.sort_by,
                view.sort.label(strings),
                sort_choices.into_iter().map(|sort| (sort.label(strings), QuadrantView { sort, ..view })).collect(),
            ),
        ];
        if !has_manual {
            rows.push((
                2,
                IconName::ArrowDownUp,
                strings.order,
                order_labels[view.newest_first as usize],
                [false, true]
                    .map(|newest_first| (order_labels[newest_first as usize], QuadrantView { newest_first, ..view }))
                    .to_vec(),
            ));
        }

        let mut items = Vec::new();
        for (row, icon, label, value, choices) in rows {
            let open = self.menu_row == Some(row);
            items.push(
                ListItem::new(SharedString::from(format!("quadrant-view-{row}")))
                    .start_slot(Icon::new(icon).color(Color::Muted))
                    .child(Label::new(label))
                    .end_slot(
                        h_flex()
                            .gap_1()
                            .child(Label::new(value).size(LabelSize::Small).color(Color::Muted))
                            .child(
                                Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight })
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_row = if this.menu_row == Some(row) { None } else { Some(row) };
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            if open {
                for (i, (choice, next)) in choices.into_iter().enumerate() {
                    let selected = next == view;
                    items.push(
                        ListItem::new(SharedString::from(format!("quadrant-view-{row}-{i}")))
                            .child(
                                div().pl_6().child(
                                    Label::new(choice)
                                        .size(LabelSize::Small)
                                        .color(if selected { Color::Accent } else { Color::Default }),
                                ),
                            )
                            .when(selected, |item| {
                                item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.views[quadrant] = next;
                                let key = this.profile_setting_key(&format!("matrix_view_{quadrant}"));
                                if let Err(err) = this.store.set_setting(&key, &next.to_storage()) {
                                    eprintln!("failed to save the quadrant's view: {err:#}");
                                }
                                this.menu_row = None;
                                this.rebuild_cache(cx);
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
        if has_manual {
            items.push(
                ListItem::new("quadrant-reset-manual")
                    .start_slot(Icon::new(IconName::RotateCcw).color(Color::Muted))
                    .child(Label::new(strings.matrix_reset_manual_order))
                    .on_click(cx.listener(move |this, _, _, cx| this.reset_manual_order(quadrant, cx)))
                    .into_any_element(),
            );
        }
        items
    }

    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, quadrant: usize, edit: bool, cx: &mut Context<Self>) {
        if edit {
            self.start_edit(quadrant, cx);
        } else {
            self.menu = Some(quadrant);
            self.menu_row = Some(0);
            cx.notify();
        }
    }

    fn child_subtasks(&self, parent_id: Id) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| {
                t.parent_id == Some(parent_id)
                    && !t.is_deleted()
                    && !t.is_wont_do()
                    && (self.show_completed || !t.is_completed())
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn push_task_tree(
        &self,
        task: &Task,
        quadrant: usize,
        today: NaiveDate,
        list_name: &impl Fn(Option<Id>) -> SharedString,
        depth: usize,
        out: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        out.push(self.render_task(task, quadrant, today, &list_name(task.list_id), depth, cx));
        if !self.show_subtasks && depth < 4 && self.expanded_parents.contains(&(quadrant, task.id)) {
            for child in self.child_subtasks(task.id) {
                self.push_task_tree(child, quadrant, today, list_name, depth + 1, out, cx);
            }
        }
    }

    fn render_task(&self, task: &Task, quadrant: usize, today: NaiveDate, list_name: &SharedString, depth: usize, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = task.id;
        let completed = task.is_completed();
        let is_note = task.kind == Kind::Note;
        let selected = self.selected_task == Some(id);
        let tag_color = |name: &str| {
            lists_panel::color(self.tags.iter().find(|t| t.name == name).and_then(|t| t.color.as_deref()), cx)
        };
        let countdown_dates = self.store.preferences().map(|p| p.countdown_dates).unwrap_or(false);
        let due = task
            .due_date
            .map(|date| task_rows::row_due_label(date, task.due_time, today, strings, countdown_dates));
        let has_notes = !is_note && !task.notes.trim().is_empty();
        let has_location = task.location.as_deref().is_some_and(|l| !l.trim().is_empty());
        let has_children = !self.show_subtasks && !self.child_subtasks(id).is_empty();
        let expanded = has_children && self.expanded_parents.contains(&(quadrant, id));
        let all_subtasks: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|t| t.parent_id == Some(id) && !t.is_deleted() && !t.is_wont_do())
            .collect();
        let subtask_progress = (!all_subtasks.is_empty()).then(|| {
            let done_subs = all_subtasks.iter().filter(|t| t.is_completed()).count();
            (done_subs, all_subtasks.len())
        });
        let checklist_progress = task.checklist;
        let has_reminder = !task.reminders.is_empty();
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        let tags: Vec<AnyElement> =
            task.tags.iter().map(|tag| Chip::new(tag.clone(), tag_color(tag)).into_any_element()).collect();

        h_flex()
            .id(SharedString::from(format!("matrix-q{quadrant}-task-{id}")))
            .debug_selector(move || format!("matrix-task-{id}"))
            .h(px(40.))
            .flex_none()
            .gap(px(8.))
            .pl(px(8. + depth as f32 * 22.))
            .pr_2()
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme.colors().element_selected))
            .hover(|s| s.bg(theme.colors().ghost_element_hover))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .drag_over::<DraggedTask>(|s, _, _, cx| s.border_t_2().border_color(cx.theme().colors().accent))
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                cx.stop_propagation();
                this.drop_task_on_task(dragged.id, id, quadrant, cx);
            }))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                this.selected_task = Some(id);
                if event.click_count() >= 2 {
                    this.close_detail_card(cx);
                    cx.emit(OpenTask(id));
                } else {
                    this.open_detail_card(id, window, cx);
                }
            }))
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.selected_task = Some(id);
                    this.task_menu = Some((id, ev.position));
                    cx.notify();
                }),
            )
            .when(has_children, |el| {
                el.child(
                    div()
                        .id(SharedString::from(format!("matrix-expand-q{quadrant}-{id}")))
                        .w(px(16.))
                        .h(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .hover(|s| s.bg(theme.colors().element_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if !this.expanded_parents.remove(&(quadrant, id)) {
                                this.expanded_parents.insert((quadrant, id));
                            }
                            cx.notify();
                        }))
                        .child(
                            Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        ),
                )
            })
            .child(if is_note {
                div()
                    .w(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::FileText).size(IconSize::Small).color(Color::Muted))
                    .into_any_element()
            } else {
                Checkbox::new(SharedString::from(format!("matrix-cb-{quadrant}-{id}")))
                    .toggle_state(completed)
                    .in_progress(task.is_in_progress())
                    .tint(task_rows::priority_color(task.priority, &theme))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_complete(id, cx);
                    }))
                    .into_any_element()
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(completed, |el| el.line_through())
                            .child(
                                Label::new(task.title.clone())
                                    .color(if completed { Color::Muted } else { Color::Default })
                                    .truncate(),
                            ),
                    )
                    .children(tags)
                    .child(Label::new(list_name.clone()).size(LabelSize::Small).color(Color::Muted))
                    .when(has_notes, |el| {
                        el.child(Icon::new(IconName::FileText).size(IconSize::XSmall).color(Color::Muted))
                    })
                    .when(has_location, |el| {
                        el.child(Icon::new(IconName::MapPin).size(IconSize::XSmall).color(Color::Muted))
                    })
                    .when_some(checklist_progress, |el, (done_items, total)| {
                        el.child(
                            h_flex()
                                .gap_0p5()
                                .child(Icon::new(IconName::ListChecks).size(IconSize::XSmall).color(Color::Muted))
                                .child(Label::new(format!("{done_items}/{total}")).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                    })
                    .when_some(subtask_progress, |el, (done_subs, total_subs)| {
                        el.child(
                            h_flex()
                                .gap_0p5()
                                .child(Icon::new(IconName::ListTree).size(IconSize::XSmall).color(Color::Muted))
                                .child(Label::new(format!("{done_subs}/{total_subs}")).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                    })
                    .when(has_reminder, |el| {
                        el.child(
                            Icon::new(IconName::AlarmClock)
                                .size(IconSize::XSmall)
                                .color(if task.constant_reminder { Color::Accent } else { Color::Muted }),
                        )
                    })
                    .when(task.repeat.is_some(), |el| {
                        el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Accent))
                    })
                    .when_some(due, |el, (label, overdue)| {
                        let due_tip = match (countdown_dates, task.due_date) {
                            (true, Some(d)) => format!("{} · {}", desktop_common::calendar::format_date(d, strings), strings.show_countdown_mode),
                            _ => strings.show_countdown_mode.to_owned(),
                        };
                        el.child(
                            div()
                                .id(SharedString::from(format!("matrix-due-toggle-{quadrant}-{id}")))
                                .px_1()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.colors().element_hover))
                                .tooltip(Tooltip::text(due_tip))
                                .child(
                                    Label::new(label)
                                        .size(LabelSize::Small)
                                        .color(if completed {
                                            Color::Muted
                                        } else if overdue {
                                            Color::Error
                                        } else {
                                            Color::Accent
                                        }),
                                )
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    if let Err(err) = this.store.update_preferences(|p| p.countdown_dates = !p.countdown_dates) {
                                        eprintln!("failed to toggle countdown dates: {err:#}");
                                    } else {
                                        desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_task_context_menu(&self, id: Id, pos: Point<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned() else {
            return div().into_any_element();
        };
        let strings = i18n::t(cx);
        let today = Local::now().date_naive();
        let title_pomo = task.title.clone();
        let title_sw = task.title.clone();
        let is_note = task.kind == Kind::Note;

        div()
            .id("matrix-task-ctx")
            .absolute()
            .left(pos.x)
            .top(pos.y)
            .occlude()
            .child(
                popover(cx)
                    .min_w(px(190.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.task_menu = None;
                        cx.notify();
                    }))
                    .child(
                        ListItem::new("mctx-open")
                            .height(px(32.))
                            .start_slot(Icon::new(IconName::FernTasks).color(Color::Muted))
                            .child(Label::new(strings.matrix_open_in_tasks))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.task_menu = None;
                                this.close_detail_card(cx);
                                cx.emit(OpenTask(id));
                                cx.notify();
                            })),
                    )
                    .when(!is_note, |el| {
                        el.child(
                            h_flex().px_2().py_1().gap_1().children(
                                [
                                    (Priority::High, strings.priority_high),
                                    (Priority::Medium, strings.priority_medium),
                                    (Priority::Low, strings.priority_low),
                                    (Priority::None, strings.priority_none),
                                ]
                                .map(|(p, tip)| {
                                    let active = task.priority == p;
                                    IconButton::new(SharedString::from(format!("mctx-p-{}", p.to_i64())), IconName::FlagFilled)
                                        .size(px(26.))
                                        .icon_size(IconSize::Small)
                                        .icon_color(task_rows::priority_color(p, cx.theme()).map(Color::Custom).unwrap_or(Color::Muted))
                                        .toggle_state(active)
                                        .tip(tip, ui::TipSide::Above)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.task_menu = None;
                                            let _ = this.store.set_priority(id, p);
                                            desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                            this.reload(cx);
                                        }))
                                }),
                            ),
                        )
                    })
                    .child(
                        ListItem::new("mctx-today")
                            .height(px(30.))
                            .start_slot(Icon::new(IconName::CalendarDays).color(Color::Muted))
                            .child(Label::new(strings.today))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.task_menu = None;
                                let _ = this.store.set_due(id, Some(today), task.due_time);
                                desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                this.reload(cx);
                            })),
                    )
                    .child(
                        ListItem::new("mctx-tomorrow")
                            .height(px(30.))
                            .start_slot(Icon::new(IconName::Sunrise).color(Color::Muted))
                            .child(Label::new(strings.tomorrow))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.task_menu = None;
                                let _ = this.store.set_due(id, Some(today + Duration::days(1)), task.due_time);
                                desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                this.reload(cx);
                            })),
                    )
                    .child(
                        ListItem::new("mctx-clear-date")
                            .height(px(30.))
                            .start_slot(Icon::new(IconName::CalendarX).color(Color::Muted))
                            .child(Label::new(strings.clear))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.task_menu = None;
                                let _ = this.store.set_due(id, None, None);
                                desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                this.reload(cx);
                            })),
                    )
                    .child(
                        ListItem::new("mctx-pomo")
                            .height(px(30.))
                            .start_slot(Icon::new(IconName::Timer).color(Color::Muted))
                            .child(Label::new(strings.start_pomo))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.task_menu = None;
                                cx.emit(StartFocus(id, title_pomo.clone(), false));
                                cx.notify();
                            })),
                    )
                    .child(
                        ListItem::new("mctx-sw")
                            .height(px(30.))
                            .start_slot(Icon::new(IconName::Clock).color(Color::Muted))
                            .child(Label::new(strings.start_stopwatch))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.task_menu = None;
                                cx.emit(StartFocus(id, title_sw.clone(), true));
                                cx.notify();
                            })),
                    )
                    .child(
                        ListItem::new("mctx-delete")
                            .height(px(30.))
                            .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                            .child(Label::new(strings.delete).color(Color::Error))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.task_menu = None;
                                this.delete_task(id, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_quadrant(&self, quadrant: usize, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let color = Self::quadrant_color(quadrant);
        let (numeral, title) = self.quadrant_title(quadrant, strings);
        let cache = &self.cache[quadrant];

        let list_name = |list: Option<Id>| -> SharedString {
            list.and_then(|id| self.lists.iter().find(|l| l.id == id))
                .map_or(SharedString::new_static(strings.inbox), |l| l.name.clone().into())
        };

        let mut content: Vec<AnyElement> = Vec::new();
        for (label, tasks) in &cache.groups {
            let collapsed = !label.is_empty() && self.collapsed_groups.contains(&(quadrant, label.clone()));
            if !label.is_empty() {
                let key_label = label.clone();
                content.push(
                    h_flex()
                        .id(SharedString::from(format!("q{quadrant}-grp-{label}")))
                        .h(px(28.))
                        .gap_1()
                        .px_1()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let key = (quadrant, key_label.clone());
                            if !this.collapsed_groups.remove(&key) {
                                this.collapsed_groups.insert(key);
                            }
                            cx.notify();
                        }))
                        .child(
                            Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(Label::new(label.clone()).weight(FontWeight::SEMIBOLD))
                        .child(Label::new(tasks.len().to_string()).size(LabelSize::XSmall).color(Color::Muted))
                        .into_any_element(),
                );
            }
            if !collapsed {
                for task in tasks {
                    self.push_task_tree(task, quadrant, today, &list_name, 0, &mut content, cx);
                }
            }
        }

        // Dedicated collapsible Completed section at the bottom of the quadrant.
        if self.show_completed && !cache.completed.is_empty() {
            let done_key = "__completed__".to_owned();
            let collapsed = self.collapsed_groups.contains(&(quadrant, done_key.clone()));
            content.push(
                h_flex()
                    .id(SharedString::from(format!("q{quadrant}-completed-hdr")))
                    .h(px(28.))
                    .mt_1()
                    .gap_1()
                    .px_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let key = (quadrant, done_key.clone());
                        if !this.collapsed_groups.remove(&key) {
                            this.collapsed_groups.insert(key);
                        }
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(Label::new(strings.completed).weight(FontWeight::SEMIBOLD).color(Color::Muted))
                    .child(
                        Label::new(cache.completed.len().to_string())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            );
            if !collapsed {
                for task in &cache.completed {
                    self.push_task_tree(task, quadrant, today, &list_name, 0, &mut content, cx);
                }
            }
        }

        let empty = content.is_empty();
        let drag_title = title.clone();

        v_flex()
            .id(SharedString::from(format!("quadrant-{numeral}")))
            .flex_1()
            .min_w_0()
            .min_h(px(140.))
            .h_full()
            .px(px(14.))
            .py_2()
            .gap_1()
            .rounded(px(12.))
            .bg(theme.colors().matrix_background)
            .drag_over::<DraggedTask>(move |style, _, _, _| style.border_1().border_color(color))
            .drag_over::<DraggedQuadrant>(move |style, _, _, _| style.border_2().border_color(color))
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on(dragged.id, quadrant, cx)))
            .on_drop(cx.listener(move |this, dragged: &DraggedQuadrant, _, cx| {
                this.swap_quadrants(dragged.quadrant, quadrant, cx);
            }))
            .child({
                let header_group = SharedString::from(format!("quadrant-header-{quadrant}"));
                h_flex()
                    .id(SharedString::from(format!("quadrant-header-{numeral}")))
                    .group(header_group.clone())
                    .h(px(36.))
                    .gap_1p5()
                    .cursor_grab()
                    .on_drag(DraggedQuadrant { quadrant, title: drag_title }, |dragged, _, _, cx| {
                        cx.new(|_| dragged.clone())
                    })
                    .child(
                        div()
                            .size(px(16.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(color)
                            .text_color(gpui::white())
                            .text_size(px(8.))
                            .font_weight(FontWeight::BOLD)
                            .child(numeral),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(
                                    Label::new(title)
                                        .size(LabelSize::Small)
                                        .weight(FontWeight::SEMIBOLD)
                                        .color(Color::Custom(color))
                                        .truncate(),
                                )
                                .when(cache.open_count > 0, |el| {
                                    el.child(
                                        Label::new(cache.open_count.to_string())
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                }),
                        ),
                    )
                    .child(
                        div()
                            .opacity(if touch::active() { 1. } else { 0. })
                            .group_hover(header_group.clone(), |s| s.opacity(1.))
                            .child(
                                IconButton::new(SharedString::from(format!("quadrant-add-{quadrant}")), IconName::Plus)
                                    .size(px(22.))
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Muted)
                                    .hover_icon_color(Color::Icon)
                                    .tooltip(Tooltip::text(strings.add_task))
                                    .on_click(cx.listener(move |this, _, window, cx| this.start_add(quadrant, window, cx))),
                            ),
                    )
                    .child(
                        div()
                            .relative()
                            .opacity(if self.menu == Some(quadrant) || touch::active() { 1. } else { 0. })
                            .group_hover(header_group, |s| s.opacity(1.))
                            .child(
                                IconButton::new(SharedString::from(format!("quadrant-more-{quadrant}")), IconName::Ellipsis)
                                    .size(px(22.))
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Muted)
                                    .hover_icon_color(Color::Icon)
                                    .tooltip(Tooltip::text(strings.more))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.menu = if this.menu == Some(quadrant) { None } else { Some(quadrant) };
                                        this.menu_row = None;
                                        window.focus(&this.focus, cx);
                                        cx.notify();
                                    })),
                            )
                            .when(self.menu == Some(quadrant), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    popover(cx)
                                        .min_w(px(230.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.menu = None;
                                            this.menu_row = None;
                                            cx.notify();
                                        }))
                                        .child(
                                            ListItem::new("quadrant-edit")
                                                .start_slot(Icon::new(IconName::Pencil).color(Color::Icon))
                                                .child(Label::new(strings.edit))
                                                .on_click(cx.listener(move |this, _, _, cx| this.start_edit(quadrant, cx))),
                                        )
                                        .children(self.render_view_rows(quadrant, cx))
                                        .into_any_element(),
                                ))
                            }),
                    )
            })
            .child(
                v_scroll(
                    SharedString::from(format!("quadrant-tasks-scroll-{numeral}")),
                    v_flex()
                        .id(SharedString::from(format!("quadrant-tasks-{numeral}")))
                        .debug_selector(move || format!("matrix-quadrant-{numeral}"))
                        .children(self.adding.as_ref().filter(|(q, _)| *q == quadrant).map(|(_, input)| {
                            h_flex()
                                .h(px(34.))
                                .flex_none()
                                .mx_1()
                                .mb_1()
                                .px_2()
                                .gap_2()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(color)
                                .child(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Muted))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .debug_selector(|| format!("matrix-add-input-{quadrant}"))
                                        .child(input.clone()),
                                )
                        }))
                        .when(empty && self.adding.as_ref().is_none_or(|(q, _)| *q != quadrant), |el| {
                            el.pt_8()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .child(Label::new(strings.no_tasks).size(LabelSize::Small).color(Color::Muted))
                                .child(
                                    h_flex()
                                        .id(SharedString::from(format!("matrix-empty-add-{quadrant}")))
                                        .px_2p5()
                                        .py_1()
                                        .gap_1()
                                        .rounded(px(6.))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme.colors().element_hover))
                                        .child(Icon::new(IconName::Plus).size(IconSize::XSmall).color(Color::Accent))
                                        .child(Label::new(strings.add_task).size(LabelSize::XSmall).color(Color::Accent))
                                        .on_click(cx.listener(move |this, _, window, cx| this.start_add(quadrant, window, cx))),
                                )
                        })
                        .children(content),
                )
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }
}

impl Render for MatrixView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let [q0, q1, q2, q3] = self.quadrant_order.map(|quadrant| self.render_quadrant(quadrant, today, cx));

        let edit_modal = self
            .editing
            .as_ref()
            .map(|(q, rule)| self.render_edit_modal(*q, rule, cx));
        let task_ctx = self.task_menu.map(|(id, pos)| self.render_task_context_menu(id, pos, cx));
        let floating_detail = self.detail_task.and(self.detail.clone()).map(|detail| {
            div()
                .id("matrix-detail-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.25))
                .occlude()
                .child(
                    div()
                        .id("matrix-detail-card")
                        .w(px(460.))
                        .h(px(560.))
                        .max_h(gpui::relative(0.85))
                        .rounded(px(12.))
                        .border_1()
                        .border_color(theme.colors().border)
                        .bg(theme.colors().elevated_surface_background)
                        .shadow_lg()
                        .overflow_hidden()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            if this.detail.as_ref().is_none_or(|d| !d.read(cx).has_popup()) {
                                this.close_detail_card(cx);
                            }
                        }))
                        .child(detail),
                )
                .into_any_element()
        });

        v_flex()
            .relative()
            .size_full()
            .key_context("Matrix")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::dismiss_transient))
            .on_action(cx.listener(|this, _: &SelectNextMatrixTask, _, cx| this.select_relative_task(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPreviousMatrixTask, _, cx| this.select_relative_task(-1, cx)))
            .on_action(cx.listener(|this, _: &ToggleMatrixTask, _, cx| {
                if let Some(id) = this.selected_task {
                    this.toggle_complete(id, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenMatrixTask, window, cx| {
                if let Some(id) = this.selected_task {
                    this.open_detail_card(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteMatrixTask, _, cx| {
                if let Some(id) = this.selected_task {
                    this.delete_task(id, cx);
                }
            }))
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(
                h_flex()
                    .mt(layout::header_inset())
                    .h(px(56.))
                    .flex_none()
                    .px(px(22.))
                    .gap_3()
                    .child(
                        Label::new(strings.section_matrix)
                            .size(LabelSize::Large)
                            .weight(FontWeight::SEMIBOLD),
                    )
                    .child({
                        let active_name = self
                            .active_profile_id
                            .and_then(|id| self.profiles.iter().find(|p| p.id == id))
                            .or_else(|| self.profiles.first())
                            .map(|p| if p.name == "Default" { strings.matrix_default_profile.to_owned() } else { p.name.clone() })
                            .unwrap_or_else(|| strings.matrix_default_profile.to_owned());
                        let can_delete = self.profiles.len() > 1;
                        let menu = self.profile_menu.then(|| {
                            PopoverAttachment::new(
                                Placement::BelowLeft,
                                popover(cx)
                                    .min_w(px(220.))
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        this.profile_menu = false;
                                        cx.notify();
                                    }))
                                    .children(self.profiles.iter().map(|profile| {
                                        let id = profile.id;
                                        let active = self.active_profile_id == Some(id);
                                        let display_name = if profile.name == "Default" {
                                            strings.matrix_default_profile.to_owned()
                                        } else {
                                            profile.name.clone()
                                        };
                                        let raw_name = profile.name.clone();
                                        ListItem::new(SharedString::from(format!("matrix-profile-{id}")))
                                            .height(px(34.))
                                            .start_slot(Icon::new(IconName::LayoutGrid).size(IconSize::XSmall).color(if active { Color::Accent } else { Color::Muted }))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .child(Label::new(display_name.clone()).color(if active { Color::Accent } else { Color::Default }).truncate()),
                                            )
                                            .end_slot(
                                                h_flex()
                                                    .gap_1()
                                                    .when(active, |el| el.child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                                                    .child(
                                                        IconButton::new(SharedString::from(format!("matrix-profile-rename-{id}")), IconName::Pencil)
                                                            .size(px(20.))
                                                            .icon_size(IconSize::XSmall)
                                                            .icon_color(Color::Muted)
                                                            .tooltip(ui::Tooltip::text(strings.edit))
                                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                                cx.stop_propagation();
                                                                this.open_profile_editor(Some(id), &raw_name, window, cx);
                                                            })),
                                                    )
                                                    .when(can_delete, |el| {
                                                        let del_name = display_name.clone();
                                                        el.child(
                                                            IconButton::new(SharedString::from(format!("matrix-profile-delete-{id}")), IconName::Trash)
                                                                .size(px(20.))
                                                                .icon_size(IconSize::XSmall)
                                                                .icon_color(Color::Muted)
                                                                .hover_icon_color(theme.status().error)
                                                                .tooltip(ui::Tooltip::text(strings.delete))
                                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                                    cx.stop_propagation();
                                                                    this.profile_menu = false;
                                                                    this.confirm_delete_profile = Some((id, del_name.clone()));
                                                                    cx.notify();
                                                                })),
                                                        )
                                                    }),
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| this.switch_profile(id, cx)))
                                    }))
                                    .child(div().my_1().child(Divider::horizontal()))
                                    .child(
                                        ListItem::new("matrix-profile-new")
                                            .height(px(34.))
                                            .start_slot(Icon::new(IconName::Plus).color(Color::Accent))
                                            .child(Label::new(strings.matrix_new_profile).color(Color::Accent))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_profile_editor(None, "", window, cx);
                                            })),
                                    ),
                            )
                        });
                        div()
                            .relative()
                            .child(
                                h_flex()
                                    .id("matrix-profile-switcher")
                                    .h(px(28.))
                                    .px_2p5()
                                    .gap_1()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(theme.colors().border)
                                    .bg(theme.colors().surface_background)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.colors().ghost_element_hover))
                                    .child(Icon::new(IconName::LayoutGrid).size(IconSize::XSmall).color(Color::Muted))
                                    .child(Label::new(active_name).size(LabelSize::Small))
                                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.profile_menu = !this.profile_menu;
                                        cx.notify();
                                    })),
                            )
                            .children(menu)
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .debug_selector(|| "matrix-header-menu-trigger".to_owned())
                            .relative()
                            .child(
                                IconButton::new("matrix-more", IconName::Ellipsis)
                                    .tooltip(ui::Tooltip::text(strings.more))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.header_menu = !this.header_menu;
                                        window.focus(&this.focus, cx);
                                        cx.notify();
                                    })),
                            )
                            .when(self.header_menu, |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    popover(cx)
                                        .min_w(px(230.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.header_menu = false;
                                            cx.notify();
                                        }))
                                        .child(
                                            ListItem::new("matrix-show-completed")
                                                .height(px(34.))
                                                .start_slot(Icon::new(IconName::SquareCheck).color(Color::Icon))
                                                .child(Label::new(if self.show_completed {
                                                    strings.hide_completed
                                                } else {
                                                    strings.show_completed
                                                }))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.header_menu = false;
                                                    let value = if this.show_completed { "0" } else { "1" };
                                                    if let Err(err) = this.store.set_setting("matrix_show_completed", value) {
                                                        eprintln!("failed to save the setting: {err:#}");
                                                    }
                                                    this.reload(cx);
                                                })),
                                        )
                                        .child(
                                            ListItem::new("matrix-show-subtasks")
                                                .height(px(34.))
                                                .start_slot(Icon::new(IconName::ListTree).color(Color::Icon))
                                                .child(Label::new(if self.show_subtasks {
                                                    strings.matrix_hide_subtasks
                                                } else {
                                                    strings.matrix_show_subtasks
                                                }))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.header_menu = false;
                                                    let value = if this.show_subtasks { "0" } else { "1" };
                                                    if let Err(err) = this.store.set_setting("matrix_show_subtasks", value) {
                                                        eprintln!("failed to save the setting: {err:#}");
                                                    }
                                                    this.reload(cx);
                                                })),
                                        )
                                        .child(
                                            ListItem::new("matrix-apply-template")
                                                .height(px(34.))
                                                .start_slot(Icon::new(IconName::LayoutTemplate).color(Color::Icon))
                                                .child(Label::new(strings.matrix_apply_date_template))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.header_menu = false;
                                                    let res = match this.active_profile_id {
                                                        Some(pid) => this.store.apply_matrix_profile_date_template(pid).map(|_| ()),
                                                        None => this.store.apply_matrix_date_template().map(|_| ()),
                                                    };
                                                    if let Err(err) = res {
                                                        eprintln!("failed to apply date template: {err:#}");
                                                    } else {
                                                        desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                                    }
                                                    this.reload(cx);
                                                })),
                                        ),
                                ))
                            }),
                    ),
            )
            .when_some(self.notice.as_ref(), |el, notice| {
                let (message, color) = match notice {
                    MatrixNotice::SavedElsewhere(destination) => (
                        strings.matrix_saved_elsewhere.replace("{destination}", destination.as_ref()),
                        Color::Default,
                    ),
                    MatrixNotice::AlreadyInQuadrant => (strings.matrix_already_in_quadrant.to_owned(), Color::Muted),
                    MatrixNotice::DropUnsupported => (strings.matrix_drop_unsupported.to_owned(), Color::Warning),
                    MatrixNotice::Failed => (strings.task_save_failed.to_owned(), Color::Error),
                };
                el.child(
                    h_flex()
                        .flex_none()
                        .mx(px(22.))
                        .px_2()
                        .py_1()
                        .gap_2()
                        .rounded(px(6.))
                        .bg(theme.colors().surface_background)
                        .child(div().flex_1().min_w_0().child(Label::new(message).color(color)))
                        .child(
                            IconButton::new("matrix-dismiss-add-notice", IconName::X)
                                .tip(strings.close, ui::TipSide::Above)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.notice = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px(px(22.))
                    .pt(px(8.))
                    .pb(px(16.))
                    .gap(px(16.))
                    .child(h_flex().flex_1().min_w_0().min_h_0().items_stretch().gap(px(16.)).child(q0).child(q1))
                    .child(h_flex().flex_1().min_w_0().min_h_0().items_stretch().gap(px(16.)).child(q2).child(q3)),
            )
            .children(task_ctx)
            .children(floating_detail)
            .children(edit_modal)
            .when_some(self.confirm_delete_profile.clone(), |el, (id, name)| {
                el.child(
                    div()
                        .id("matrix-delete-profile-backdrop")
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::black().opacity(0.35))
                        .occlude()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.confirm_delete_profile = None;
                            window.focus(&this.focus, cx);
                            cx.notify();
                        }))
                        .child(
                            v_flex()
                                .id("matrix-delete-profile-modal")
                                .w(px(340.))
                                .p_5()
                                .gap_3()
                                .rounded(px(10.))
                                .bg(theme.colors().elevated_surface_background)
                                .border_1()
                                .border_color(theme.colors().border)
                                .shadow_lg()
                                .on_click(|_, _, cx| cx.stop_propagation())
                                .child(
                                    Label::new(format!("{} \"{name}\"?", strings.delete))
                                        .size(LabelSize::Large)
                                        .weight(FontWeight::SEMIBOLD),
                                )
                                .child(
                                    h_flex()
                                        .pt_2()
                                        .gap_2()
                                        .justify_end()
                                        .child(
                                            div()
                                                .id("matrix-delete-profile-cancel")
                                                .px_3()
                                                .h(px(30.))
                                                .flex()
                                                .items_center()
                                                .rounded(px(6.))
                                                .border_1()
                                                .border_color(theme.colors().border)
                                                .cursor_pointer()
                                                .hover(|s| s.bg(theme.colors().element_hover))
                                                .child(Label::new(strings.cancel).size(LabelSize::Small))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.confirm_delete_profile = None;
                                                    window.focus(&this.focus, cx);
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id("matrix-delete-profile-confirm")
                                                .px_3()
                                                .h(px(30.))
                                                .flex()
                                                .items_center()
                                                .rounded(px(6.))
                                                .bg(theme.status().error)
                                                .cursor_pointer()
                                                .hover(|s| s.opacity(0.9))
                                                .child(Label::new(strings.delete).size(LabelSize::Small).color(Color::OnAccent))
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.confirm_delete_profile = None;
                                                    this.delete_profile(id, cx);
                                                    window.focus(&this.focus, cx);
                                                })),
                                        ),
                                ),
                        ),
                )
            })
            .when_some(self.profile_editor.as_ref(), |el, (editing, input)| {
                let heading = if editing.is_some() { strings.matrix_rename_profile } else { strings.matrix_new_profile };
                let can_save = !input.read(cx).text().trim().is_empty();
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::black().opacity(0.35))
                        .occlude()
                        .id("matrix-profile-editor-backdrop")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.profile_editor = None;
                            window.focus(&this.focus, cx);
                            cx.notify();
                        }))
                        .child(
                            v_flex()
                                .id("matrix-profile-editor")
                                .w(px(320.))
                                .p_4()
                                .gap_3()
                                .rounded(px(10.))
                                .bg(theme.colors().elevated_surface_background)
                                .border_1()
                                .border_color(theme.colors().border)
                                .shadow_lg()
                                .on_click(|_, _, cx| cx.stop_propagation())
                                .child(Label::new(heading).weight(FontWeight::SEMIBOLD))
                                .child(Label::new(strings.matrix_profile_name).size(LabelSize::XSmall).color(Color::Muted))
                                .child(
                                    div()
                                        .h(px(32.))
                                        .px_2p5()
                                        .flex()
                                        .items_center()
                                        .rounded(px(6.))
                                        .border_1()
                                        .border_color(theme.colors().border)
                                        .bg(theme.colors().background)
                                        .child(input.clone()),
                                )
                                .child(
                                    h_flex()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("matrix-profile-cancel")
                                                .h(px(30.))
                                                .px_3()
                                                .flex()
                                                .items_center()
                                                .rounded(px(6.))
                                                .border_1()
                                                .border_color(theme.colors().border)
                                                .cursor_pointer()
                                                .hover(|s| s.bg(theme.colors().element_hover))
                                                .child(Label::new(strings.cancel).size(LabelSize::Small))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.profile_editor = None;
                                                    window.focus(&this.focus, cx);
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id("matrix-profile-save")
                                                .h(px(30.))
                                                .px_3p5()
                                                .flex()
                                                .items_center()
                                                .rounded(px(6.))
                                                .bg(theme.colors().accent.opacity(if can_save { 1. } else { 0.5 }))
                                                .when(can_save, |el| el.cursor_pointer().hover(|s| s.opacity(0.9)))
                                                .child(Label::new(strings.save).size(LabelSize::Small).color(Color::OnAccent))
                                                .when(can_save, |el| el.on_click(cx.listener(|this, _, _, cx| this.save_profile_editor(cx)))),
                                        ),
                                ),
                        ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use fern_core::matrix::MatrixDate;
    use fern_core::Store;
    use gpui::EntityInputHandler;

    use super::*;

    #[gpui::test]
    fn long_quadrant_input_stays_visible_and_saves_in_full(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            keymap::load_default(&assets::Assets, cx).unwrap();
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = MatrixView::new(store.clone(), cx);
            view.start_add(0, window, cx);
            view
        });
        cx.simulate_resize(gpui::size(px(700.), px(500.)));
        let empty_field = cx.debug_bounds("matrix-add-input-0").unwrap();
        let title = format!("{}конец", "ДлинноеНазвание日本語🧑‍💻".repeat(100));
        cx.simulate_input(&title);
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("matrix-add-input-0").unwrap(), empty_field);
        for width in [700., 480.] {
            cx.simulate_resize(gpui::size(px(width), px(500.)));
            let field = cx.debug_bounds("matrix-add-input-0").unwrap();
            cx.update(|window, cx| {
                let input = view.read(cx).adding.as_ref().unwrap().1.clone();
                let end = title.encode_utf16().count();
                let caret = input.update(cx, |input, cx| {
                    input.bounds_for_range(end..end, gpui::Bounds::default(), window, cx).unwrap()
                });
                assert!(caret.left() >= field.left() && caret.right() <= field.right(), "{caret:?} outside {field:?}");
            });
        }
        cx.simulate_keystrokes("enter");
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, title);
        cx.update(|_, cx| assert!(view.read(cx).adding.as_ref().unwrap().1.read(cx).text().is_empty()));
    }

    #[gpui::test]
    fn quadrant_quick_add_saves_recognized_tags(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
            let add = |cx: &App, input| {
                let id = matrix.read(cx).add_quadrant_task(0, input, today).unwrap().id;
                store.task(id).unwrap().unwrap()
            };
            let task = add(cx, "Plan the sprint #work #team #Work");
            assert_eq!(task.title, "Plan the sprint");
            assert_eq!(task.tags, ["team", "work"]);
            assert_eq!(task.priority, Priority::High);

            let mut preferences = store.preferences().unwrap();
            preferences.recognition.remove_tag_text = false;
            store.save_preferences(&preferences).unwrap();
            let task = add(cx, "Plan the sprint #work");
            assert_eq!(task.title, "Plan the sprint #work");
            assert_eq!(task.tags, ["work"]);

            preferences.recognition.tags = false;
            store.save_preferences(&preferences).unwrap();
            let task = add(cx, "Plan the sprint #work");
            assert_eq!(task.title, "Plan the sprint #work");
            assert!(task.tags.is_empty());
        });
    }

    #[gpui::test]
    fn quadrant_quick_add_uses_an_available_list_after_deletion(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let removed = store.create_list("Remove me").unwrap();
        let available = store.create_list("Still available").unwrap();
        let mut preferences = store.preferences().unwrap();
        preferences.default_list = Some(removed.id);
        store.save_preferences(&preferences).unwrap();
        let mut rule = QuadrantRule::defaults()[0].clone();
        rule.lists = vec![Some(removed.id), Some(available.id)];
        store.save_matrix_rule(0, Some(&rule)).unwrap();
        let today = Local::now().date_naive();

        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
            store.delete_list(removed.id).unwrap();
            let id = matrix.read(cx).add_quadrant_task(0, "First #work", today).unwrap().id;
            let tasks = store.tasks().unwrap();
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].id, id);
            assert_eq!(tasks[0].list_id, Some(available.id));
            assert_eq!(tasks[0].tags, ["work"]);

            rule.lists = vec![Some(removed.id)];
            store.save_matrix_rule(0, Some(&rule)).unwrap();
            matrix.update(cx, |matrix, cx| matrix.reload(cx));
            let view = matrix.read(cx);
            assert_eq!(view.rules[0].lists, rule.lists);
            let unrelated = &tasks[0];
            assert!(!view.rules[0].matches(unrelated, today));

            matrix.update(cx, |matrix, cx| {
                matrix.start_edit(0, cx);
                matrix.name_input.update(cx, |input, cx| input.set_text("Scoped quadrant", cx));
                matrix.finish_edit(Some(true), cx);
            });
            assert_eq!(store.matrix_rules().unwrap()[0].lists, rule.lists);

            let id = matrix.read(cx).add_quadrant_task(0, "Second", today).unwrap().id;
            assert_eq!(store.task(id).unwrap().unwrap().list_id, None);
            assert_eq!(store.tasks().unwrap().len(), 2);
        });
    }

    #[gpui::test]
    fn archiving_the_last_list_keeps_the_quadrant_restricted_until_restore(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = Local::now().date_naive();
        let list = store.create_list("Restricted").unwrap();
        let fallback = store.create_list("Available").unwrap();
        let mut preferences = store.preferences().unwrap();
        preferences.default_list = Some(fallback.id);
        store.save_preferences(&preferences).unwrap();

        let task = |title: &str, list_id| {
            store
                .create_task(
                    TaskDraft {
                        title: title.into(),
                        list_id: Some(list_id),
                        priority: Priority::High,
                        due_date: Some(today),
                        ..TaskDraft::default()
                    },
                    true,
                )
                .unwrap()
        };
        let restricted = task("In the restricted list", list.id);
        let unrelated = task("In another list", fallback.id);
        let mut rule = QuadrantRule::defaults()[0].clone();
        rule.lists = vec![Some(list.id)];
        store.save_matrix_rule(0, Some(&rule)).unwrap();

        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
            store.set_list_archived(list.id, true).unwrap();
            matrix.update(cx, |matrix, cx| matrix.reload(cx));
            let view = matrix.read(cx);
            assert_eq!(view.rules[0].lists, rule.lists);
            assert!(view.cache[0].open_count == 0);
            let added = view.add_quadrant_task(0, "Saved in available list", today).unwrap();
            assert_eq!(added.list_id, Some(fallback.id));

            store.set_list_archived(list.id, false).unwrap();
            matrix.update(cx, |matrix, cx| matrix.reload(cx));
            let view = matrix.read(cx);
            assert!(view.tasks.iter().any(|task| task.id == restricted.id));
            assert_eq!(quadrant_of(&view.rules, &restricted, today), Some(0));
            assert_ne!(quadrant_of(&view.rules, &unrelated, today), Some(0));
            assert_ne!(quadrant_of(&view.rules, &added, today), Some(0));
        });
    }

    #[gpui::test]
    fn matrix_uses_rule_date_without_an_explicit_date_and_explains_other_destinations(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let mut rule = QuadrantRule::defaults()[0].clone();
        rule.dates = vec![MatrixDate::Tomorrow];
        store.save_matrix_rule(0, Some(&rule)).unwrap();

        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store, cx));
            let task = matrix.read(cx).add_quadrant_task(0, "Plan 9am", today).unwrap();
            assert_eq!(task.due_date, today.succ_opt());
            assert_eq!(task.due_time, chrono::NaiveTime::from_hms_opt(9, 0, 0));
            let other = matrix.read(cx).add_quadrant_task(0, "Plan today", today).unwrap();
            assert_eq!(other.due_date, Some(today));
            matrix.update(cx, |matrix, cx| {
                matrix.reload(cx);
                let strings = i18n::t(cx);
                assert!(matrix.destination_notice(0, &task, today, strings).is_none());
                assert!(matches!(
                    matrix.destination_notice(0, &other, today, strings),
                    Some(MatrixNotice::SavedElsewhere(ref list)) if list.as_ref() == strings.inbox,
                ));
            });
        });
    }

    #[gpui::test]
    fn escape_dismisses_the_matrix_header_menu(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let view = MatrixView::new(Rc::new(Store::open_in_memory().unwrap()), cx);
            window.focus(&view.focus_handle(cx), cx);
            view
        });
        let trigger = cx.debug_bounds("matrix-header-menu-trigger").unwrap().center();
        cx.simulate_click(trigger, gpui::Modifiers::default());
        cx.update(|_, cx| assert!(view.read(cx).header_menu));
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            assert!(!view.read(cx).header_menu);
            assert!(view.read(cx).focus_handle(cx).is_focused(window));
        });
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| assert!(view.read(cx).focus_handle(cx).is_focused(window)));
    }

    #[test]
    fn quadrant_views_roundtrip_including_list_and_date() {
        let store = Store::open_in_memory().unwrap();
        let work = store.create_list("Work").unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        for (title, days) in [("later", 5), ("soon", 1), ("now", 0)] {
            let task = store.add_task(title).unwrap();
            store.set_list(task.id, Some(work.id)).unwrap();
            store.set_due(task.id, Some(today + Duration::days(days)), None).unwrap();
        }
        let tasks = store.tasks().unwrap();
        let lists = store.lists().unwrap();
        let strings = &i18n::EN;
        let titles = |groups: &[(String, Vec<Task>)]| -> Vec<String> {
            groups.iter().flat_map(|(_, tasks)| tasks.iter().map(|t| t.title.clone())).collect()
        };

        // Regression test for bug 1: GroupBy::List round-trips cleanly and keeps real list names.
        let list_view = QuadrantView { group: GroupBy::List, sort: SortBy::Date, newest_first: false };
        assert_eq!(QuadrantView::from_storage(&list_view.to_storage()), list_view);
        let by_list = arrange(tasks.clone(), list_view, &lists, today, strings);
        assert_eq!(by_list.len(), 1);
        assert_eq!(by_list[0].0, "Work");
        assert_eq!(titles(&by_list), ["now", "soon", "later"]);

        let newest = arrange(tasks.clone(), QuadrantView { newest_first: true, ..list_view }, &lists, today, strings);
        assert_eq!(titles(&newest), ["later", "soon", "now"]);

        let by_date = arrange(tasks, QuadrantView::default(), &lists, today, strings);
        assert_eq!(by_date.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(), ["Today", "Tomorrow", "Next 7 Days"]);
        assert_eq!(QuadrantView::from_storage(&QuadrantView::default().to_storage()), QuadrantView::default());
        assert_eq!(QuadrantView::from_storage(""), QuadrantView::default());
    }

    #[gpui::test]
    fn show_completed_separates_and_toggles_completion(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Finish spec").unwrap();
        store.set_priority(task.id, Priority::High).unwrap();
        store.set_completed(task.id, true).unwrap();
        store.set_setting("matrix_show_completed", "1").unwrap();

        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
            assert_eq!(matrix.read(cx).cache[0].open_count, 0);
            assert_eq!(matrix.read(cx).cache[0].completed.len(), 1);

            // Toggling a completed task reopens it into the open list.
            matrix.update(cx, |m, cx| m.toggle_complete(task.id, cx));
            assert_eq!(matrix.read(cx).cache[0].open_count, 1);
            assert!(matrix.read(cx).cache[0].completed.is_empty());
        });
    }

    #[gpui::test]
    fn overflowing_quadrant_scrolls_when_clicking_scrollbar(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let mut ids = Vec::new();
        for i in 0..25 {
            ids.push(store.add_task(&format!("Quadrant item {i:02}")).unwrap().id);
        }

        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
            assert!(keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (_, cx) = cx.add_window_view(|_, cx| MatrixView::new(store, cx));
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(480.)));
        cx.run_until_parked();

        // Tasks without priority land in quadrant IV ("Not Urgent & Not Important").
        let quadrant = cx.debug_bounds("matrix-quadrant-IV").unwrap();
        let (bottom_sel, bottom_row) = ids
            .iter()
            .filter_map(|id| {
                let sel: &'static str = format!("matrix-task-{id}").leak();
                cx.debug_bounds(sel).map(|b| (sel, b))
            })
            .max_by(|a, b| a.1.bottom().partial_cmp(&b.1.bottom()).unwrap())
            .expect("quadrant renders task rows");
        let initial_top = bottom_row.top();
        assert!(bottom_row.bottom() > quadrant.bottom());

        let bar_bottom = gpui::point(quadrant.right() - gpui::px(6.), quadrant.bottom() - gpui::px(8.));
        let modifiers = gpui::Modifiers::default();
        cx.simulate_mouse_move(bar_bottom, None, modifiers);
        cx.simulate_click(bar_bottom, modifiers);
        let scrolled_row = cx.debug_bounds(bottom_sel).unwrap();
        assert!(
            scrolled_row.top() < initial_top,
            "clicking quadrant scrollbar should scroll rows upward: {initial_top:?} -> {:?}",
            scrolled_row.top()
        );
    }

    #[gpui::test]
    fn drop_on_applies_full_rule_patch_and_detects_already_in_quadrant(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let work = store.add_list(&fern_core::ListDraft { name: "Work".into(), ..Default::default() }).unwrap();
        let task = store.add_task("Draft quarterly plan").unwrap();

        // Configure Q1 with custom list, tag, date, and priority.
        let mut q1 = QuadrantRule::defaults()[0].clone();
        q1.lists = vec![Some(work.id)];
        q1.tags = vec!["focus".into()];
        q1.dates = vec![fern_core::matrix::MatrixDate::Today];
        q1.priorities = vec![Priority::High];
        store.save_matrix_rule(0, Some(&q1)).unwrap();

        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
            matrix.update(cx, |m, cx| m.drop_on(task.id, 0, cx));

            let updated = store.task(task.id).unwrap().unwrap();
            assert_eq!(updated.priority, Priority::High);
            assert_eq!(updated.list_id, Some(work.id));
            assert_eq!(updated.due_date, Some(Local::now().date_naive()));
            assert!(updated.tags.iter().any(|t| t.eq_ignore_ascii_case("focus")));
            assert!(matrix.read(cx).notice.is_none());

            // Dropping into Q1 again when the task already matches shows AlreadyInQuadrant.
            matrix.update(cx, |m, cx| m.drop_on(task.id, 0, cx));
            assert!(matches!(matrix.read(cx).notice, Some(MatrixNotice::AlreadyInQuadrant)));
        });
    }

    #[gpui::test]
    fn swap_quadrants_persists_custom_order(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
            assert_eq!(matrix.read(cx).quadrant_order, [0, 1, 2, 3]);

            matrix.update(cx, |m, cx| m.swap_quadrants(0, 3, cx));
            assert_eq!(matrix.read(cx).quadrant_order, [3, 1, 2, 0]);

            // Reloading from store preserves the swapped order.
            let reloaded = cx.new(|cx| MatrixView::new(store.clone(), cx));
            assert_eq!(reloaded.read(cx).quadrant_order, [3, 1, 2, 0]);
        });
    }

    #[gpui::test]
    fn manual_task_order_persists_and_resets(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let a = store.add_task("Alpha").unwrap();
        let b = store.add_task("Beta").unwrap();
        let c = store.add_task("Gamma").unwrap();
        for id in [a.id, b.id, c.id] {
            store.set_priority(id, Priority::High).unwrap();
        }

        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
            matrix.update(cx, |m, cx| {
                m.views[0] = QuadrantView { group: GroupBy::None, sort: SortBy::Title, newest_first: false };
                m.rebuild_cache(cx);
            });
            let before: Vec<Id> =
                matrix.read(cx).cache[0].groups[0].1.iter().map(|t| t.id).collect();
            assert_eq!(before, vec![a.id, b.id, c.id]);

            // Drag Gamma onto Alpha -> Gamma moves before Alpha.
            matrix.update(cx, |m, cx| m.drop_task_on_task(c.id, a.id, 0, cx));
            let after: Vec<Id> =
                matrix.read(cx).cache[0].groups[0].1.iter().map(|t| t.id).collect();
            assert_eq!(after, vec![c.id, a.id, b.id]);
            assert!(!matrix.read(cx).manual_order[0].is_empty());

            // Reset restores normal sort order.
            matrix.update(cx, |m, cx| m.reset_manual_order(0, cx));
            let reset: Vec<Id> =
                matrix.read(cx).cache[0].groups[0].1.iter().map(|t| t.id).collect();
            assert_eq!(reset, vec![a.id, b.id, c.id]);
            assert!(matrix.read(cx).manual_order[0].is_empty());
        });
    }

    #[gpui::test]
    fn multiple_matrix_profiles_keep_independent_rules_and_settings(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let p2 = store.create_matrix_profile("Focus Profile", None).unwrap();

        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
            assert_eq!(matrix.read(cx).profiles.len(), 2);

            // Rename Q1 on profile 1.
            matrix.update(cx, |m, cx| {
                m.start_edit(0, cx);
                m.name_input.update(cx, |i, cx| i.set_text("Profile 1 Q1", cx));
                m.finish_edit(Some(true), cx);
            });
            assert_eq!(matrix.read(cx).rules[0].name, "Profile 1 Q1");

            // Switch to profile 2 -> independent rules and view settings.
            matrix.update(cx, |m, cx| m.switch_profile(p2.id, cx));
            assert_eq!(matrix.read(cx).active_profile_id, Some(p2.id));
            assert_ne!(matrix.read(cx).rules[0].name, "Profile 1 Q1");

            matrix.update(cx, |m, cx| {
                m.start_edit(0, cx);
                m.name_input.update(cx, |i, cx| i.set_text("Profile 2 Q1", cx));
                m.finish_edit(Some(true), cx);
                m.swap_quadrants(0, 3, cx);
            });
            assert_eq!(matrix.read(cx).rules[0].name, "Profile 2 Q1");
            assert_eq!(matrix.read(cx).quadrant_order, [3, 1, 2, 0]);

            // Switch back -> profile 1 keeps its original order & rules.
            let p1_id = matrix.read(cx).profiles[0].id;
            matrix.update(cx, |m, cx| m.switch_profile(p1_id, cx));
            assert_eq!(matrix.read(cx).rules[0].name, "Profile 1 Q1");
            assert_eq!(matrix.read(cx).quadrant_order, [0, 1, 2, 3]);

            // Delete profile 2 -> only default remains.
            matrix.update(cx, |m, cx| m.delete_profile(p2.id, cx));
            assert_eq!(matrix.read(cx).profiles.len(), 1);
            assert_eq!(matrix.read(cx).active_profile_id, Some(p1_id));
        });
    }
}
