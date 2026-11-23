//! The Eisenhower Matrix: four quadrants with independent rules on lists, tags, dates, priorities,
//! and item kind; cached grouping/sorting, manual per-quadrant ordering, quadrant swapping, full
//! cross-quadrant DnD, floating task detail card, context menu, and keyboard navigation.

mod edit_dialog;
#[path = "matrix_ui/dialogs.rs"]
mod dialogs;
#[path = "matrix_ui/quadrant.rs"]
mod quadrant;
#[cfg(test)]
#[path = "matrix_ui/tests.rs"]
mod tests;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use chrono::{Local, NaiveDate};
use desktop_common::actions::{
    DeleteMatrixTask, DismissTransient, OpenMatrixTask, SelectNextMatrixTask,
    SelectPreviousMatrixTask, ToggleMatrixTask,
};
use desktop_common::layout;
use desktop_common::text_input::{Mode, Submitted, TextInput};
use fern_core::matrix::{
    DropOutcome, MatrixKind, QuadrantRule, build_tag_families, quadrant_of, quadrants_of,
};
use fern_core::{Id, Kind, List, Preferences, Store, Task, TaskDraft};
use fern_i18n::{self as i18n, Strings};
use gpui::{Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Point};
use task_ui::task_detail::{self, StartFocus, TaskDetail, TaskTextEdited, TaskTextField, TaskUpdated};
use ui::prelude::*;

use dialogs::MatrixNotice;
use quadrant::{QuadrantCache, arrange_with_manual};
pub use quadrant::{QuadrantView, arrange};

/// Maximum completed tasks loaded into the matrix when Show Completed is enabled.
const COMPLETED_HISTORY_LIMIT: usize = 100;

/// The user asked to jump to a task in the Tasks section (double-click or context menu).
pub struct OpenTask(pub Id);

pub struct MatrixView {
    store: Rc<Store>,
    focus: FocusHandle,
    tasks: Vec<Task>,
    /// Cached parent -> child indices into `self.tasks`, rebuilt once in `reload()`.
    children_by_parent: HashMap<Id, Vec<usize>>,
    lists: Vec<List>,
    list_names: HashMap<Id, SharedString>,
    tags: Vec<fern_core::Tag>,
    tag_colors: HashMap<String, String>,
    tag_families: HashMap<String, HashSet<String>>,
    preferences: Preferences,
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
    text_edit_sync_task: Option<gpui::Task<()>>,
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
    pending_complete: HashSet<Id>,
    submitting_quadrant_task: bool,
    active: bool,
    dirty: bool,
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
        Self::new_with_active(store, true, cx)
    }

    pub fn new_inactive(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, false, cx)
    }

    fn new_with_active(store: Rc<Store>, initial_active: bool, cx: &mut Context<Self>) -> Self {
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
            children_by_parent: HashMap::new(),
            lists: Vec::new(),
            list_names: HashMap::new(),
            tags: Vec::new(),
            tag_colors: HashMap::new(),
            tag_families: HashMap::new(),
            preferences: Preferences::default(),
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
            text_edit_sync_task: None,
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
            pending_complete: HashSet::new(),
            submitting_quadrant_task: false,
            active: initial_active,
            dirty: !initial_active,
        };
        if initial_active {
            this.reload(cx);
        }
        cx.observe_global::<desktop_common::data_version::DataVersion>(|this, cx| {
            if desktop_common::data_version::changed_elsewhere(cx.entity_id(), cx) {
                this.sync_external(cx);
            }
        })
        .detach();
        this
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if active && self.dirty {
            self.dirty = false;
            self.reload(cx);
        }
    }

    pub fn sync_external(&mut self, cx: &mut Context<Self>) {
        self.text_edit_sync_task = None;
        if self.active {
            self.reload(cx);
        } else {
            self.dirty = true;
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        self.preferences = self.store.preferences().unwrap_or_default();
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
        let task_ids: HashSet<Id> = self.tasks.iter().map(|t| t.id).collect();
        let mut children_by_parent: HashMap<Id, Vec<usize>> = HashMap::new();
        for (idx, task) in self.tasks.iter().enumerate() {
            if let Some(parent_id) = task.parent_id
                && parent_id != task.id
                && task_ids.contains(&parent_id)
            {
                children_by_parent.entry(parent_id).or_default().push(idx);
            }
        }
        self.children_by_parent = children_by_parent;
        self.lists = self.store.lists().unwrap_or_default();
        self.list_names = self
            .lists
            .iter()
            .map(|l| (l.id, SharedString::from(l.name.clone())))
            .collect();
        self.tags = self.store.tags().unwrap_or_default();
        self.tag_colors = self
            .tags
            .iter()
            .filter_map(|t| t.color.as_ref().map(|c| (t.name.clone(), c.clone())))
            .collect();
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

    fn apply_task_text_edit(&mut self, edit: &TaskTextEdited, cx: &mut Context<Self>) {
        if self.dirty {
            self.reload(cx);
        } else {
            for task in self.tasks.iter_mut().filter(|t| t.id == edit.id) {
                match &edit.field {
                    TaskTextField::Title(title) => task.title = title.clone(),
                    TaskTextField::Notes(notes) => task.notes = notes.clone(),
                    TaskTextField::Location(location) => task.location = location.clone(),
                }
            }
            self.rebuild_cache(cx);
            cx.notify();
        }
        self.text_edit_sync_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.text_edit_sync_task = None;
                desktop_common::data_version::data_changed(cx.entity_id(), cx);
            });
        }));
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
                    this.text_edit_sync_task = None;
                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                    this.reload(cx);
                })
                .detach();
                cx.subscribe(&entity, |this, _, edit: &TaskTextEdited, cx| {
                    this.apply_task_text_edit(edit, cx);
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
        if self.text_edit_sync_task.take().is_some() {
            desktop_common::data_version::data_changed(cx.entity_id(), cx);
        }
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
            let parsed = fern_core::parse_quick_add_with(&text, Local::now().date_naive(), &list_names, this.preferences.recognition);
            input.update(cx, |input, cx| input.set_highlights(parsed.spans, cx));
        })
        .detach();
        cx.subscribe_in(&input, window, move |this, input, Submitted(title), window, cx| {
            if title.trim().is_empty() {
                return;
            }
            let today = Local::now().date_naive();
            if let Some(hooks) = desktop_common::plugin_bus::PluginHooksGlobal::global(cx)
                .filter(desktop_common::plugin_bus::PluginHooksGlobal::is_active)
            {
                if this.submitting_quadrant_task {
                    return;
                }
                this.submitting_quadrant_task = true;
                let raw_title = title.clone();
                let rule = this.rules[quadrant].clone();
                let store = this.store.clone();
                let input_entity = input.clone();
                let window_handle = window.window_handle();
                cx.spawn(async move |this, cx| {
                    let hooks_t = hooks.clone();
                    let raw_for_t = raw_title.clone();
                    let transform = cx
                        .background_spawn(async move { hooks_t.run_quick_add_transform(raw_for_t) })
                        .await;
                    let transformed = match transform {
                        Ok(desktop_common::plugin_bus::PluginHookDecision::Cancel(reason)) => {
                            cx.update(|cx| {
                                let fallback = i18n::t(cx).plugins_task_create_cancelled;
                                desktop_common::plugin_bus::push_hook_cancel_toast(&reason, fallback, cx);
                            });
                            let _ = this.update(cx, |this, cx| {
                                this.submitting_quadrant_task = false;
                                cx.notify();
                            });
                            return;
                        }
                        Ok(desktop_common::plugin_bus::PluginHookDecision::Modify(val)) => {
                            desktop_common::plugin_bus::extract_transformed_quick_add(&val)
                                .unwrap_or(raw_title)
                        }
                        Ok(desktop_common::plugin_bus::PluginHookDecision::Allow) => raw_title,
                        Err(err) => {
                            eprintln!("plugin quickAddTransform hook failed: {err:#}");
                            raw_title
                        }
                    };

                    let built = Self::build_quadrant_draft(&store, &rule, &transformed, today);
                    let (base_draft, default_top) = match built {
                        Ok(pair) => pair,
                        Err(err) => {
                            eprintln!("failed to prepare matrix task: {err:#}");
                            let _ = this.update(cx, |this, cx| {
                                this.submitting_quadrant_task = false;
                                this.notice = Some(MatrixNotice::Failed);
                                cx.notify();
                            });
                            return;
                        }
                    };
                    let draft_json = desktop_common::plugin_bus::task_draft_to_hook_json(
                        &base_draft,
                        default_top,
                    );
                    let before_res = cx
                        .background_spawn(async move { hooks.run_before_task_create(draft_json) })
                        .await;
                    let (final_draft, top) = match before_res {
                        Ok(desktop_common::plugin_bus::PluginHookDecision::Cancel(reason)) => {
                            cx.update(|cx| {
                                let fallback = i18n::t(cx).plugins_task_create_cancelled;
                                desktop_common::plugin_bus::push_hook_cancel_toast(&reason, fallback, cx);
                            });
                            let _ = this.update(cx, |this, cx| {
                                this.submitting_quadrant_task = false;
                                cx.notify();
                            });
                            return;
                        }
                        Ok(desktop_common::plugin_bus::PluginHookDecision::Modify(modified_json)) => {
                            match desktop_common::plugin_bus::parse_task_draft_from_hook_json(
                                &modified_json,
                                default_top,
                            ) {
                                Ok(pair) => pair,
                                Err(err) => {
                                    eprintln!("plugin beforeTaskCreate returned invalid draft: {err:#}");
                                    (base_draft, default_top)
                                }
                            }
                        }
                        Ok(desktop_common::plugin_bus::PluginHookDecision::Allow) => {
                            (base_draft, default_top)
                        }
                        Err(err) => {
                            eprintln!("plugin beforeTaskCreate hook failed: {err:#}");
                            (base_draft, default_top)
                        }
                    };

                    let mut cleared = false;
                    let _ = this.update(cx, |this, cx| {
                        this.submitting_quadrant_task = false;
                        match this.store.create_task(final_draft, top) {
                            Ok(task) => {
                                cleared = true;
                                this.text_edit_sync_task = None;
                                desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                this.reload(cx);
                                this.notice = this.destination_notice(quadrant, &task, today, i18n::t(cx));
                            }
                            Err(err) => {
                                eprintln!("failed to add the task: {err:#}");
                                this.notice = Some(MatrixNotice::Failed);
                                cx.notify();
                            }
                        }
                    });
                    if cleared {
                        let _ = window_handle.update(cx, |_, window, cx| {
                            input_entity.update(cx, |input, cx| input.clear(window, cx));
                        });
                    }
                })
                .detach();
                return;
            }
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
            this.text_edit_sync_task = None;
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

    fn build_quadrant_draft(
        store: &Store,
        rule: &QuadrantRule,
        input: &str,
        today: NaiveDate,
    ) -> anyhow::Result<(TaskDraft, bool)> {
        let preferences = store.preferences()?;
        let lists = store.lists()?;
        let available = |id: Id| lists.iter().any(|list| list.id == id && !list.archived);
        let list_names: Vec<_> = lists.iter().filter(|list| !list.archived).map(|list| list.name.clone()).collect();
        let mut parsed = fern_core::parse_quick_add_with(input, today, &list_names, preferences.recognition);
        let title = if parsed.title.is_empty() { input.trim() } else { &parsed.title };
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
        Ok((
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
        ))
    }

    fn add_quadrant_task(&self, quadrant: usize, input: &str, today: NaiveDate) -> anyhow::Result<Task> {
        let (draft, top) = Self::build_quadrant_draft(&self.store, &self.rules[quadrant], input, today)?;
        self.store.create_task(draft, top)
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
        if self.dirty {
            self.reload(cx);
        }
        match what {
            "header-menu" => self.header_menu = true,
            "add" => self.start_add(0, window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn is_task_read_only(&self, id: Id) -> bool {
        self.store.is_task_read_only(id).unwrap_or(false)
    }

    fn finish_toggle_complete(&mut self, task: &Task, done: bool, cx: &mut Context<Self>) {
        self.text_edit_sync_task = None;
        if let Err(err) = desktop_common::task_ops::toggle_task_completion(
            &self.store,
            task,
            done,
            cx.entity_id(),
            cx,
        ) {
            eprintln!("failed to toggle completion: {err:#}");
        }
        self.reload(cx);
    }

    fn toggle_complete(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.is_task_read_only(id) {
            return;
        }
        let Some(task) = self
            .tasks
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .or_else(|| self.store.task(id).ok().flatten())
        else {
            return;
        };
        let done = !task.is_completed();
        if done
            && !task.is_wont_do()
            && desktop_common::plugin_bus::PluginHooksGlobal::global(cx)
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
                        if !allowed || this.is_task_read_only(id) {
                            cx.notify();
                            return;
                        }
                        if let Some(current) = this
                            .tasks
                            .iter()
                            .find(|t| t.id == id)
                            .cloned()
                            .or_else(|| this.store.task(id).ok().flatten())
                            && !current.is_completed()
                        {
                            this.finish_toggle_complete(&current, true, cx);
                        }
                    });
                },
            );
            return;
        }
        self.finish_toggle_complete(&task, done, cx);
    }

    fn delete_task(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.is_task_read_only(id) {
            return;
        }
        if let Err(err) = self.store.delete_task(id) {
            eprintln!("failed to delete task: {err:#}");
        } else {
            if self.selected_task == Some(id) {
                self.selected_task = None;
            }
            if self.detail_task == Some(id) {
                self.close_detail_card(cx);
            }
            self.text_edit_sync_task = None;
            desktop_common::data_version::data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    /// Applies full cross-quadrant drop adjustments (priority, date, list, tag, kind) and surfaces
    /// feedback toasts when already matching or unsupported.
    fn drop_on(&mut self, id: Id, quadrant: usize, cx: &mut Context<Self>) {
        if self.is_task_read_only(id) {
            return;
        }
        let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        let today = Local::now().date_naive();
        let available_lists: Vec<Id> = self.lists.iter().filter(|l| !l.archived).map(|l| l.id).collect();
        match self.rules[quadrant].drop_changes(&task, today, &available_lists, &self.tag_families) {
            DropOutcome::AlreadyMatches => {
                if !task.is_completed() {
                    self.notice = Some(MatrixNotice::AlreadyInQuadrant);
                    cx.notify();
                }
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
                    self.text_edit_sync_task = None;
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
        if self.cache[quadrant].completed.iter().any(|t| t.id == dragged_id) {
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
                self.text_edit_sync_task = None;
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

    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, quadrant: usize, edit: bool, cx: &mut Context<Self>) {
        if self.dirty {
            self.reload(cx);
        }
        if edit {
            self.start_edit(quadrant, cx);
        } else {
            self.menu = Some(quadrant);
            self.menu_row = Some(0);
            cx.notify();
        }
    }
}

impl Render for MatrixView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let [q0, q1, q2, q3] = self.quadrant_order.map(|quadrant| self.render_quadrant(quadrant, today, cx));

        let edit_modal = self
            .editing
            .as_ref()
            .map(|(q, rule)| self.render_edit_modal(*q, rule, cx));
        let task_ctx = self.task_menu.map(|(id, pos)| self.render_task_context_menu(id, pos, cx));
        let floating_detail = self.render_floating_detail(window, cx);

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
                    .child(self.render_profile_switcher(cx))
                    .child(div().flex_1())
                    .child(self.render_header_menu(cx)),
            )
            .when_some(self.notice.as_ref(), |el, notice| {
                el.child(self.render_notice_banner(notice, cx))
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
            .children(self.render_confirm_delete_profile(cx))
            .children(self.render_profile_editor_modal(cx))
    }
}
