//! TimeStripe-inspired panoramic Horizons planning view embedded inside Fern's Tasks section.
//!
//! Displays tasks across Inbox, Today, ISO Week, Month, Quarter, Year, Decade, and Life,
//! plus zoomed Time Scale strips (`Days`..`Decades`), drag-and-drop scheduling, smart rollover
//! for past incomplete items, slide-over `TaskDetail`, and filters.

mod column;
mod drawer;
mod prompts;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use chrono::{Datelike, Duration, Local, NaiveDate};
use desktop_common::actions::{DismissTransient, Redo, Search, Undo};
use desktop_common::calendar;
use desktop_common::colors;
use desktop_common::data_version::{DataVersion, changed_elsewhere, data_changed};
use desktop_common::layout;
use desktop_common::task_ops;
use desktop_common::text_input::{Cancelled, Mode, Submitted, TextInput};
use desktop_common::undo::UndoAction;
use fern_core::horizon::{
    horizons_bucket_partition, horizons_day_partition, horizons_week_dated_tasks,
    is_horizons_inbox,
};
use fern_core::{Horizon, Id, List, Priority, Store, Tag, Task, TaskDraft};
use fern_i18n::{self as i18n, CurrentLocale, Strings};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla,
    KeyDownEvent, MouseButton, Render, ScrollHandle, SharedString, Subscription, Window, div, px,
};
use task_ui::task_detail::{
    OpenTask, StartFocus, TaskDetail, TaskTextEdited, TaskTextField, TaskUpdated, Undoable,
};
use ui::prelude::*;
use ui::{
    HorizontalScrollbar, Icon, IconName, IconSize, Label, LabelSize, Placement, PopoverAttachment,
    Tooltip,
};

use crate::column::{
    ColumnSpec, ColumnTarget, HorizonSlot, PANORAMIC_MIN_WIDTH, STRIP_MIN_WIDTH, capitalize_first,
    format_day_subtitle,
};
use crate::drawer::ActivePopover;

const SETTING_COLLAPSED_COLS: &str = "horizons_collapsed_cols";
const SETTING_SHOW_COMPLETED: &str = "horizons_show_completed";
const SETTING_SHOW_DATED_IN_WEEK: &str = "horizons_show_dated_in_week";
const SETTING_SHOW_PROMPT: &str = "horizons_show_prompt";

/// Events emitted by [`HorizonsView`] to its host (`TodoView`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HorizonsEvent {
    StartFocus {
        task_id: Id,
        title: String,
        stopwatch: bool,
    },
    OpenSearch,
}

impl EventEmitter<HorizonsEvent> for HorizonsView {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HorizonsMode {
    Inbox,
    Horizons,
    Days,
    Weeks,
    Months,
    Quarters,
    Years,
    Decades,
}

impl HorizonsMode {
    fn is_scale_strip(self) -> bool {
        matches!(
            self,
            Self::Days
                | Self::Weeks
                | Self::Months
                | Self::Quarters
                | Self::Years
                | Self::Decades
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) enum UndoEntry {
    Single(UndoAction),
    Batch(Vec<UndoAction>),
}

impl UndoEntry {
    fn apply(&self, store: &Store) -> bool {
        match self {
            Self::Single(action) => action.apply(store).is_ok(),
            Self::Batch(actions) => {
                for action in actions.iter().rev() {
                    if action.apply(store).is_err() {
                        return false;
                    }
                }
                true
            }
        }
    }

    fn redo(&self, store: &Store) -> Option<Self> {
        match self {
            Self::Single(action) => action.redo(store).ok().flatten().map(Self::Single),
            Self::Batch(actions) => {
                let mut redone = Vec::with_capacity(actions.len());
                for action in actions {
                    if let Ok(Some(next)) = action.redo(store) {
                        redone.push(next);
                    }
                }
                (!redone.is_empty()).then_some(Self::Batch(redone))
            }
        }
    }
}

/// Root GPUI view for Fern's Horizons planning screen inside `Tasks`.
pub struct HorizonsView {
    pub(crate) store: Rc<Store>,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) mode: HorizonsMode,
    period_offset: i32,

    // Cached SQLite data (zero DB reads in `render()`).
    pub(crate) tasks: Vec<Task>,
    pub(crate) lists: Vec<List>,
    pub(crate) list_meta: HashMap<Id, (SharedString, Option<Hsla>)>,
    pub(crate) tags: Vec<Tag>,

    // Persisted view options.
    pub(crate) collapsed_cols: HashSet<HorizonSlot>,
    pub(crate) show_completed: bool,
    pub(crate) show_dated_in_week: bool,
    pub(crate) show_prompt: bool,
    pub(crate) prompt_index: usize,

    // Transient UI state.
    pub(crate) collapsed_rollovers: HashSet<String>,
    pub(crate) inline_add: Option<(ColumnTarget, Entity<TextInput>, Vec<Subscription>)>,
    pub(crate) active_popover: Option<ActivePopover>,

    // Filters.
    pub(crate) filter_board: Option<Id>,
    pub(crate) filter_tag: Option<String>,
    pub(crate) filter_priority: Option<Priority>,

    // Slide-over TaskDetail drawer.
    pub(crate) detail_task: Option<Id>,
    pub(crate) detail: Entity<TaskDetail>,
    pub(crate) text_edit_sync_task: Option<gpui::Task<()>>,

    // Scroll handle & undo history.
    h_scroll: ScrollHandle,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    active: bool,
    dirty: bool,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for HorizonsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl HorizonsView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let detail = cx.new(|cx| {
            let mut panel = TaskDetail::new(store.clone(), window, cx);
            panel.set_top_inset(false, cx);
            panel
        });

        let subscriptions = vec![
            cx.subscribe(&detail, |this, _, _: &TaskUpdated, cx| {
                this.text_edit_sync_task = None;
                data_changed(cx.entity_id(), cx);
                this.reload(cx);
            }),
            cx.subscribe(&detail, |this, _, edit: &TaskTextEdited, cx| {
                this.apply_task_text_edit(edit, cx);
            }),
            cx.subscribe(&detail, |_this, _, ev: &StartFocus, cx| {
                cx.emit(HorizonsEvent::StartFocus {
                    task_id: ev.0,
                    title: ev.1.clone(),
                    stopwatch: ev.2,
                });
            }),
            cx.subscribe_in(&detail, window, |this, _, ev: &OpenTask, window, cx| {
                this.open_task(ev.0, window, cx);
            }),
            cx.subscribe(&detail, |this, _, ev: &Undoable, cx| {
                this.push_undo(UndoEntry::Single(ev.0.clone()));
                if let UndoAction::Restore(deleted_id) = ev.0
                    && this.detail_task == Some(deleted_id)
                {
                    this.detail_task = None;
                    this.detail.update(cx, |d, cx| d.show(None, cx));
                }
                this.reload(cx);
            }),
            cx.observe_global::<DataVersion>(|this, cx| {
                if changed_elsewhere(cx.entity_id(), cx) {
                    this.sync_external(cx);
                }
            }),
            cx.observe_global::<CurrentLocale>(|this, cx| {
                let strings = i18n::t(cx);
                if let Some((_, input, _)) = &this.inline_add {
                    input.update(cx, |i, cx| i.set_placeholder(strings.horizons_add_placeholder, cx));
                }
                cx.notify();
            }),
        ];

        let mut this = Self {
            store,
            focus_handle: cx.focus_handle(),
            mode: HorizonsMode::Horizons,
            period_offset: 0,
            tasks: Vec::new(),
            lists: Vec::new(),
            list_meta: HashMap::new(),
            tags: Vec::new(),
            collapsed_cols: HorizonSlot::DEFAULT_COLLAPSED.into_iter().collect(),
            show_completed: false,
            show_dated_in_week: false,
            show_prompt: true,
            prompt_index: 0,
            collapsed_rollovers: HashSet::new(),
            inline_add: None,
            active_popover: None,
            filter_board: None,
            filter_tag: None,
            filter_priority: None,
            detail_task: None,
            detail,
            text_edit_sync_task: None,
            h_scroll: ScrollHandle::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            active: true,
            dirty: false,
            _subscriptions: subscriptions,
        };
        this.load_settings();
        this.reload(cx);
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
        if self.active {
            self.reload(cx);
        } else {
            self.dirty = true;
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        self.text_edit_sync_task = None;
        self.tasks = self.store.tasks().unwrap_or_default();
        self.lists = self.store.lists().unwrap_or_default();
        self.list_meta = self
            .lists
            .iter()
            .map(|l| (l.id, (SharedString::from(l.name.clone()), colors::parse_color(l.color.as_deref()))))
            .collect();
        self.tags = self.store.tags().unwrap_or_default();
        self.load_settings();

        let lists_for_detail = self.lists.clone();
        self.detail.update(cx, |d, _| d.set_lists(lists_for_detail));
        if let Some(id) = self.detail_task {
            let task = self.tasks.iter().find(|t| t.id == id && !t.is_deleted()).cloned();
            if task.is_none() {
                self.detail_task = None;
            }
            self.detail.update(cx, |d, cx| d.refresh(task, cx));
        }

        if let Some(board_id) = self.filter_board
            && !self
                .lists
                .iter()
                .any(|l| l.id == board_id && !l.archived && l.completed_at.is_none())
        {
            self.filter_board = None;
        }

        cx.notify();
    }

    pub fn open_task(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|t| t.id == id && !t.is_deleted()).cloned() else {
            return;
        };
        self.active_popover = None;
        self.inline_add = None;
        self.detail_task = Some(id);
        let lists_for_detail = self.lists.clone();
        self.detail.update(cx, |d, cx| {
            d.set_lists(lists_for_detail);
            d.show(Some(task), cx);
        });
        let _ = window;
        cx.notify();
    }

    pub fn dismiss_transient(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.active_popover.take().is_some() {
            cx.notify();
            return true;
        }
        if self.inline_add.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
            return true;
        }
        if self.detail_task.is_some() {
            if self.detail.update(cx, |d, cx| d.dismiss_popup(window, cx)) {
                return true;
            }
            self.close_drawer(window, cx);
            return true;
        }
        false
    }

    #[cfg(debug_assertions)]
    pub fn dev_select_mode(&mut self, spec: &str, cx: &mut Context<Self>) {
        self.reload(cx);
        let mode = match spec.trim().to_ascii_lowercase().as_str() {
            "inbox" => HorizonsMode::Inbox,
            "days" => HorizonsMode::Days,
            "weeks" => HorizonsMode::Weeks,
            "months" => HorizonsMode::Months,
            "quarters" => HorizonsMode::Quarters,
            "years" => HorizonsMode::Years,
            "decades" => HorizonsMode::Decades,
            _ => HorizonsMode::Horizons,
        };
        self.select_mode(mode, cx);
    }

    #[cfg(debug_assertions)]
    pub fn dev_open_task(&mut self, needle: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.reload(cx);
        let needle_lower = needle.trim().to_lowercase();
        if needle_lower.is_empty() {
            return;
        }
        if let Some(task) = self
            .tasks
            .iter()
            .find(|t| !t.is_deleted() && t.title.to_lowercase().contains(&needle_lower))
        {
            let id = task.id;
            self.open_task(id, window, cx);
        }
    }

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    fn load_settings(&mut self) {
        self.collapsed_cols = match self.store.setting(SETTING_COLLAPSED_COLS).ok().flatten() {
            Some(raw) => raw.split(',').filter_map(HorizonSlot::from_key).collect(),
            None => HorizonSlot::DEFAULT_COLLAPSED.into_iter().collect(),
        };
        self.show_completed = self
            .store
            .setting(SETTING_SHOW_COMPLETED)
            .ok()
            .flatten()
            .is_some_and(|v| v == "1" || v == "true");
        self.show_dated_in_week = self
            .store
            .setting(SETTING_SHOW_DATED_IN_WEEK)
            .ok()
            .flatten()
            .is_some_and(|v| v == "1" || v == "true");
        self.show_prompt = self
            .store
            .setting(SETTING_SHOW_PROMPT)
            .ok()
            .flatten()
            .is_none_or(|v| v != "0" && v != "false");
    }

    fn persist_collapsed_cols(&self) {
        let mut keys: Vec<&'static str> = HorizonSlot::ALL
            .into_iter()
            .filter(|slot| self.collapsed_cols.contains(slot))
            .map(HorizonSlot::key)
            .collect();
        keys.dedup();
        let value = keys.join(",");
        let _ = self.store.set_setting(SETTING_COLLAPSED_COLS, &value);
    }

    pub(crate) fn persist_bool_setting(&self, key: &str, value: bool) {
        let _ = self.store.set_setting(key, if value { "1" } else { "0" });
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
            cx.notify();
        }
        self.text_edit_sync_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.text_edit_sync_task = None;
                data_changed(cx.entity_id(), cx);
            });
        }));
    }

    pub(crate) fn select_mode(&mut self, mode: HorizonsMode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            self.period_offset = 0;
            self.inline_add = None;
            self.active_popover = None;
            self.h_scroll.set_offset(gpui::point(px(0.), px(0.)));
            cx.notify();
        }
    }

    fn step_period(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.period_offset = self.period_offset.saturating_add(delta);
        self.inline_add = None;
        cx.notify();
    }

    fn jump_to_current(&mut self, cx: &mut Context<Self>) {
        if self.period_offset != 0 {
            self.period_offset = 0;
            self.inline_add = None;
            cx.notify();
        }
    }

    pub(crate) fn toggle_slot_collapsed(&mut self, slot: HorizonSlot, cx: &mut Context<Self>) {
        if !self.collapsed_cols.remove(&slot) {
            self.collapsed_cols.insert(slot);
        }
        self.persist_collapsed_cols();
        data_changed(cx.entity_id(), cx);
        cx.notify();
    }

    pub(crate) fn push_undo(&mut self, entry: UndoEntry) {
        self.undo_stack.push(entry);
        if self.undo_stack.len() > 64 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn perform_undo(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.undo_stack.pop() else {
            return;
        };
        if entry.apply(&self.store) {
            self.redo_stack.push(entry);
            data_changed(cx.entity_id(), cx);
            self.reload(cx);
        }
    }

    fn perform_redo(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.redo_stack.pop() else {
            return;
        };
        if let Some(next) = entry.redo(&self.store) {
            self.undo_stack.push(next);
            data_changed(cx.entity_id(), cx);
            self.reload(cx);
        }
    }

    pub(crate) fn toggle_task_done(&mut self, id: Id, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        let done = !task.is_completed();
        if let Ok(undo) = task_ops::toggle_task_completion(&self.store, &task, done, cx.entity_id(), cx) {
            if let Some(action) = undo {
                self.push_undo(UndoEntry::Single(action));
            }
            self.reload(cx);
        }
    }

    pub(crate) fn start_inline_add(
        &mut self,
        target: ColumnTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.horizons_add_placeholder, cx));
        let subs = vec![
            cx.subscribe_in(&input, window, move |this, input_entity, ev: &Submitted, window, cx| {
                this.submit_inline_add(target, &ev.0, input_entity, window, cx);
            }),
            cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
                this.inline_add = None;
                window.focus(&this.focus_handle, cx);
                cx.notify();
            }),
        ];
        window.focus(&input.focus_handle(cx), cx);
        self.inline_add = Some((target, input, subs));
        self.active_popover = None;
        cx.notify();
    }

    fn submit_inline_add(
        &mut self,
        target: ColumnTarget,
        raw: &str,
        input: &Entity<TextInput>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = raw.trim();
        if title.is_empty() {
            self.inline_add = None;
            window.focus(&self.focus_handle, cx);
            cx.notify();
            return;
        }

        let mut draft = TaskDraft {
            title: title.to_owned(),
            ..Default::default()
        };
        if let Some(tag) = &self.filter_tag {
            draft.tags.push(tag.clone());
        }
        if let Some(priority) = self.filter_priority {
            draft.priority = priority;
        }

        match target {
            ColumnTarget::Inbox => {}
            ColumnTarget::Day(day) => {
                draft.due_date = Some(day);
                draft.list_id = self.filter_board;
            }
            ColumnTarget::Horizon(horizon) => {
                draft.horizon = Some(horizon);
                draft.list_id = self.filter_board;
            }
        }

        if self.store.create_task(draft, false).is_ok() {
            input.update(cx, |i, cx| i.set_text("", cx));
            window.focus(&input.focus_handle(cx), cx);
            data_changed(cx.entity_id(), cx);
            self.reload(cx);
        }
    }

    pub(crate) fn handle_drop_on_column(
        &mut self,
        task_id: Id,
        target: ColumnTarget,
        cx: &mut Context<Self>,
    ) {
        let Some(task) = self.tasks.iter().find(|t| t.id == task_id).cloned() else {
            return;
        };
        let mut changed = false;
        match target {
            ColumnTarget::Inbox => {
                let before_horizon = task.horizon;
                let ok_due = self.store.set_due(task_id, None, None).is_ok();
                let ok_horizon = self.store.set_horizon(task_id, None).is_ok();
                let ok_list = self.store.set_list(task_id, None).is_ok();
                let ok_sec = self.store.set_section(task_id, None).is_ok();
                if ok_due && ok_horizon && ok_list && ok_sec {
                    if before_horizon.is_some() && task.due_date.is_none() && task.list_id.is_none() {
                        self.push_undo(UndoEntry::Single(UndoAction::RestoreHorizon {
                            id: task_id,
                            before: before_horizon,
                            after: None,
                        }));
                    }
                    changed = true;
                }
            }
            ColumnTarget::Day(day) => {
                if task.due_date != Some(day)
                    && self.store.set_due(task_id, Some(day), task.due_time).is_ok()
                {
                    changed = true;
                }
            }
            ColumnTarget::Horizon(horizon) => {
                if task.horizon != Some(horizon)
                    && self.store.set_horizon(task_id, Some(horizon)).is_ok()
                {
                    if task.due_date.is_none() {
                        self.push_undo(UndoEntry::Single(UndoAction::RestoreHorizon {
                            id: task_id,
                            before: task.horizon,
                            after: Some(horizon),
                        }));
                    }
                    changed = true;
                }
            }
        }
        if changed {
            data_changed(cx.entity_id(), cx);
            self.reload(cx);
        }
    }

    fn inbox_tasks(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| {
                is_horizons_inbox(t)
                    && (self.show_completed || !t.is_completed())
                    && self.matches_filter(t)
            })
            .collect()
    }

    fn open_inbox_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|t| is_horizons_inbox(t) && !t.is_completed())
            .count()
    }

    fn build_day_column(
        &self,
        day: NaiveDate,
        today: NaiveDate,
        slot: Option<HorizonSlot>,
        strings: &Strings,
    ) -> ColumnSpec<'_> {
        let (items, rollover) =
            horizons_day_partition(&self.tasks, &self.lists, day, today, self.show_completed);
        let diff = (day - today).num_days();
        let title = match diff {
            0 => strings.today.to_owned(),
            1 => strings.tomorrow.to_owned(),
            -1 => strings.yesterday.to_owned(),
            _ => capitalize_first(strings.weekdays_full[day.weekday().num_days_from_monday() as usize]),
        };
        let subtitle = Some(format_day_subtitle(day, today, strings));
        ColumnSpec {
            target: ColumnTarget::Day(day),
            slot,
            title,
            subtitle,
            is_current: day == today,
            tasks: self.filter_vec(items),
            rollover: self.filter_vec(rollover),
            dated_in_week: Vec::new(),
            empty_hint: None,
            show_triage_chips: false,
            show_board_badge: true,
        }
    }

    fn build_horizon_column(
        &self,
        horizon: Horizon,
        today: NaiveDate,
        slot: Option<HorizonSlot>,
        exclude_today_in_week: bool,
        strings: &Strings,
    ) -> ColumnSpec<'_> {
        let (items, rollover) =
            horizons_bucket_partition(&self.tasks, &self.lists, horizon, today, self.show_completed);
        let (title, subtitle) = calendar::format_horizon_heading(horizon, today, strings);
        let is_current = horizon != Horizon::Life
            && horizon == Horizon::current_for(horizon.granularity(), today);
        let dated_in_week = match horizon {
            Horizon::Week { year, week } if self.show_dated_in_week => {
                let exclude = exclude_today_in_week.then_some(today);
                self.filter_vec(horizons_week_dated_tasks(
                    &self.tasks,
                    &self.lists,
                    year,
                    week,
                    exclude,
                    self.show_completed,
                ))
            }
            _ => Vec::new(),
        };
        ColumnSpec {
            target: ColumnTarget::Horizon(horizon),
            slot,
            title,
            subtitle,
            is_current,
            tasks: self.filter_vec(items),
            rollover: self.filter_vec(rollover),
            dated_in_week,
            empty_hint: None,
            show_triage_chips: false,
            show_board_badge: true,
        }
    }

    fn slot_open_count(&self, slot: HorizonSlot, today: NaiveDate) -> usize {
        match slot {
            HorizonSlot::Inbox => self.open_inbox_count(),
            HorizonSlot::Today => {
                let (items, rollover) =
                    horizons_day_partition(&self.tasks, &self.lists, today, today, false);
                items.len() + rollover.len()
            }
            _ => match slot.target(today) {
                ColumnTarget::Horizon(h) => {
                    let (items, rollover) =
                        horizons_bucket_partition(&self.tasks, &self.lists, h, today, false);
                    items.len() + rollover.len()
                }
                _ => 0,
            },
        }
    }

    fn scale_range_hint(&self, today: NaiveDate, strings: &Strings) -> Option<String> {
        match self.mode {
            HorizonsMode::Days => {
                let first = today + Duration::days(i64::from(-1 + self.period_offset));
                let last = today + Duration::days(i64::from(4 + self.period_offset));
                Some(format!(
                    "{} – {}",
                    calendar::short_date(first, strings),
                    calendar::short_date(last, strings)
                ))
            }
            HorizonsMode::Weeks => {
                let base = Horizon::current_week(today);
                match (base.shift(-1 + self.period_offset), base.shift(4 + self.period_offset)) {
                    (Horizon::Week { year: y1, .. }, Horizon::Week { year: y2, .. }) => {
                        Some(if y1 == y2 { y1.to_string() } else { format!("{y1}–{y2}") })
                    }
                    _ => None,
                }
            }
            HorizonsMode::Months => {
                let base = Horizon::current_month(today);
                match (base.shift(self.period_offset), base.shift(5 + self.period_offset)) {
                    (Horizon::Month { year: y1, .. }, Horizon::Month { year: y2, .. }) => {
                        Some(if y1 == y2 { y1.to_string() } else { format!("{y1}–{y2}") })
                    }
                    _ => None,
                }
            }
            HorizonsMode::Quarters => {
                let base = Horizon::current_quarter(today);
                match (base.shift(self.period_offset), base.shift(3 + self.period_offset)) {
                    (Horizon::Quarter { year: y1, .. }, Horizon::Quarter { year: y2, .. }) => {
                        Some(if y1 == y2 { y1.to_string() } else { format!("{y1}–{y2}") })
                    }
                    _ => None,
                }
            }
            HorizonsMode::Years => {
                let base = Horizon::current_year(today);
                match (base.shift(self.period_offset), base.shift(4 + self.period_offset)) {
                    (Horizon::Year(y1), Horizon::Year(y2)) => Some(format!("{y1}–{y2}")),
                    _ => None,
                }
            }
            HorizonsMode::Decades => {
                let base = Horizon::current_decade(today);
                match (base.shift(self.period_offset), base.shift(3 + self.period_offset)) {
                    (Horizon::Decade(d1), Horizon::Decade(d2)) => Some(format!("{d1}–{}", d2 + 9)),
                    _ => None,
                }
            }
            HorizonsMode::Horizons | HorizonsMode::Inbox => None,
        }
    }

    /// Razor-clean top strip bar: scale tabs on the left, optional period stepper in the middle,
    /// and Filter + View Options buttons on the right (below `title_bar_height()`).
    fn render_top_strip(&self, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let strings = i18n::t(cx);
        let filter_active = self.has_active_filters() || self.show_completed;
        let filter_popover = (self.active_popover == Some(ActivePopover::Filter))
            .then(|| self.render_filter_popover(cx));
        let options_popover = (self.active_popover == Some(ActivePopover::Options))
            .then(|| self.render_options_popover(cx));

        let tabs = [
            ("horizons", strings.horizons_overview, HorizonsMode::Horizons),
            ("days", strings.horizons_days, HorizonsMode::Days),
            ("weeks", strings.horizons_weeks, HorizonsMode::Weeks),
            ("months", strings.horizons_months, HorizonsMode::Months),
            ("quarters", strings.horizons_quarters, HorizonsMode::Quarters),
            ("years", strings.horizons_years, HorizonsMode::Years),
            ("decades", strings.horizons_decades, HorizonsMode::Decades),
        ];

        let tab_bar = h_flex().items_center().gap(px(2.)).children(tabs.into_iter().map(
            |(key, label, mode)| {
                let active = self.mode == mode;
                div()
                    .id(SharedString::from(format!("horizons-tab-{key}")))
                    .debug_selector(move || format!("horizons-tab-{key}"))
                    .h(px(26.))
                    .px(px(9.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if active {
                        colors.border
                    } else {
                        gpui::transparent_black()
                    })
                    .when(active, |el| el.bg(colors.surface_background))
                    .when(!active, |el| el.hover(|s| s.bg(colors.ghost_element_hover)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_mode(mode, cx);
                    }))
                    .child(
                        Label::new(label)
                            .size(LabelSize::Small)
                            .weight(if active {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .color(if active { Color::Default } else { Color::Muted }),
                    )
            },
        ));

        let range_hint = self.scale_range_hint(today, strings);
        let at_current = self.period_offset == 0;

        let stepper = self.mode.is_scale_strip().then(|| {
            h_flex()
                .items_center()
                .gap(px(8.))
                .when_some(range_hint, |el, hint| {
                    el.child(Label::new(hint).size(LabelSize::XSmall).color(Color::Muted))
                })
                .child(
                    h_flex()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(colors.border_variant)
                        .bg(colors.surface_background.opacity(0.6))
                        .overflow_hidden()
                        .child(
                            div()
                                .id("horizons-pager-prev")
                                .debug_selector(|| "horizons-pager-prev".to_owned())
                                .h(px(24.))
                                .px(px(7.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(colors.ghost_element_hover))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.step_period(-1, cx);
                                }))
                                .child(Icon::new(IconName::ChevronLeft).size(IconSize::XSmall).color(Color::Icon)),
                        )
                        .child(div().w(px(1.)).h(px(14.)).bg(colors.border_variant))
                        .child(
                            div()
                                .id("horizons-pager-today")
                                .debug_selector(|| "horizons-pager-today".to_owned())
                                .h(px(24.))
                                .px(px(9.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(colors.ghost_element_hover))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.jump_to_current(cx);
                                }))
                                .child(
                                    Label::new(strings.horizons_jump_today)
                                        .size(LabelSize::XSmall)
                                        .weight(FontWeight::MEDIUM)
                                        .color(if at_current { Color::Muted } else { Color::Accent }),
                                ),
                        )
                        .child(div().w(px(1.)).h(px(14.)).bg(colors.border_variant))
                        .child(
                            div()
                                .id("horizons-pager-next")
                                .debug_selector(|| "horizons-pager-next".to_owned())
                                .h(px(24.))
                                .px(px(7.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(colors.ghost_element_hover))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.step_period(1, cx);
                                }))
                                .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(Color::Icon)),
                        ),
                )
        });

        let toolbar_btn = |id: &'static str,
                           icon: IconName,
                           selected: bool,
                           badge_dot: bool,
                           tooltip: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .relative()
                .size(px(26.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(selected, |el| el.bg(colors.element_selected))
                .when(!selected, |el| el.hover(|s| s.bg(colors.ghost_element_hover)))
                .tooltip(Tooltip::text(tooltip))
                .child(Icon::new(icon).size(IconSize::Small).color(if selected {
                    Color::Accent
                } else {
                    Color::Icon
                }))
                .when(badge_dot, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(3.))
                            .right(px(3.))
                            .size(px(5.5))
                            .rounded_full()
                            .bg(colors.accent),
                    )
                })
        };

        h_flex()
            .id("horizons-top-strip")
            .debug_selector(|| "horizons-top-strip".to_owned())
            .w_full()
            .flex_none()
            .pt(layout::title_bar_height() + px(8.))
            .pb(px(8.))
            .px(px(16.))
            .border_b_1()
            .border_color(colors.border_variant.opacity(0.6))
            .items_center()
            .justify_between()
            .gap(px(10.))
            .child(tab_bar)
            .child(
                h_flex()
                    .items_center()
                    .gap(px(6.))
                    .children(stepper)
                    // Filter button + BelowRight popover.
                    .child(
                        div()
                            .relative()
                            .child(
                                toolbar_btn(
                                    "horizons-btn-filter",
                                    IconName::Funnel,
                                    self.active_popover == Some(ActivePopover::Filter),
                                    filter_active,
                                    strings.horizons_filter_title,
                                )
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.active_popover =
                                        if this.active_popover == Some(ActivePopover::Filter) {
                                            None
                                        } else {
                                            Some(ActivePopover::Filter)
                                        };
                                    cx.notify();
                                })),
                            )
                            .when_some(filter_popover, |el, content| {
                                el.child(PopoverAttachment::new(Placement::BelowRight, content))
                            }),
                    )
                    // Columns / View Options button + BelowRight popover.
                    .child(
                        div()
                            .relative()
                            .child(
                                toolbar_btn(
                                    "horizons-btn-options",
                                    IconName::Settings2,
                                    self.active_popover == Some(ActivePopover::Options),
                                    false,
                                    strings.horizons_columns,
                                )
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.active_popover =
                                        if this.active_popover == Some(ActivePopover::Options) {
                                            None
                                        } else {
                                            Some(ActivePopover::Options)
                                        };
                                    cx.notify();
                                })),
                            )
                            .when_some(options_popover, |el, content| {
                                el.child(PopoverAttachment::new(Placement::BelowRight, content))
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_horizontal_strip(&self, children: Vec<AnyElement>) -> AnyElement {
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .child(
                div()
                    .id("horizons-columns-scroll")
                    .debug_selector(|| "horizons-columns-scroll".to_owned())
                    .size_full()
                    .flex()
                    .flex_row()
                    .items_stretch()
                    .overflow_x_scroll()
                    .track_scroll(&self.h_scroll)
                    .children(children),
            )
            .child(HorizontalScrollbar::for_handle(
                "horizons-h-scrollbar",
                self.h_scroll.clone(),
            ))
            .into_any_element()
    }

    fn render_main_canvas(&self, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);

        match self.mode {
            HorizonsMode::Horizons => {
                let mut cols: Vec<AnyElement> = Vec::with_capacity(HorizonSlot::ALL.len());
                let last_idx = HorizonSlot::ALL.len() - 1;

                for (idx, slot) in HorizonSlot::ALL.into_iter().enumerate() {
                    let is_last = idx == last_idx;
                    if self.collapsed_cols.contains(&slot) {
                        let count = self.slot_open_count(slot, today);
                        cols.push(self.render_collapsed_strip(slot, count, today, is_last, cx));
                    } else {
                        let spec = match slot {
                            HorizonSlot::Inbox => ColumnSpec {
                                target: ColumnTarget::Inbox,
                                slot: Some(slot),
                                title: strings.inbox.to_owned(),
                                subtitle: None,
                                is_current: false,
                                tasks: self.inbox_tasks(),
                                rollover: Vec::new(),
                                dated_in_week: Vec::new(),
                                empty_hint: None,
                                show_triage_chips: false,
                                show_board_badge: false,
                            },
                            HorizonSlot::Today => {
                                self.build_day_column(today, today, Some(slot), strings)
                            }
                            HorizonSlot::Week => self.build_horizon_column(
                                Horizon::current_week(today),
                                today,
                                Some(slot),
                                true,
                                strings,
                            ),
                            HorizonSlot::Month => self.build_horizon_column(
                                Horizon::current_month(today),
                                today,
                                Some(slot),
                                false,
                                strings,
                            ),
                            HorizonSlot::Quarter => self.build_horizon_column(
                                Horizon::current_quarter(today),
                                today,
                                Some(slot),
                                false,
                                strings,
                            ),
                            HorizonSlot::Year => self.build_horizon_column(
                                Horizon::current_year(today),
                                today,
                                Some(slot),
                                false,
                                strings,
                            ),
                            HorizonSlot::Decade => self.build_horizon_column(
                                Horizon::current_decade(today),
                                today,
                                Some(slot),
                                false,
                                strings,
                            ),
                            HorizonSlot::Life => self.build_horizon_column(
                                Horizon::Life,
                                today,
                                Some(slot),
                                false,
                                strings,
                            ),
                        };
                        cols.push(self.render_column(
                            spec,
                            today,
                            is_last,
                            PANORAMIC_MIN_WIDTH,
                            false,
                            cx,
                        ));
                    }
                }
                self.render_horizontal_strip(cols)
            }
            HorizonsMode::Inbox => {
                let spec = ColumnSpec {
                    target: ColumnTarget::Inbox,
                    slot: None,
                    title: strings.inbox.to_owned(),
                    subtitle: None,
                    is_current: false,
                    tasks: self.inbox_tasks(),
                    rollover: Vec::new(),
                    dated_in_week: Vec::new(),
                    empty_hint: Some(strings.horizons_empty_inbox),
                    show_triage_chips: true,
                    show_board_badge: false,
                };
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .h_full()
                    .flex()
                    .justify_center()
                    .child(self.render_column(
                        spec,
                        today,
                        true,
                        STRIP_MIN_WIDTH,
                        true,
                        cx,
                    ))
                    .into_any_element()
            }
            HorizonsMode::Days => {
                let days: Vec<NaiveDate> = (-1..=4)
                    .map(|i| today + Duration::days(i64::from(i + self.period_offset)))
                    .collect();
                let last_idx = days.len().saturating_sub(1);
                let cols = days
                    .into_iter()
                    .enumerate()
                    .map(|(idx, day)| {
                        let spec = self.build_day_column(day, today, None, strings);
                        self.render_column(
                            spec,
                            today,
                            idx == last_idx,
                            STRIP_MIN_WIDTH,
                            false,
                            cx,
                        )
                    })
                    .collect();
                self.render_horizontal_strip(cols)
            }
            HorizonsMode::Weeks => {
                let base = Horizon::current_week(today);
                let horizons: Vec<Horizon> = (-1..=4)
                    .map(|i| base.shift(i + self.period_offset))
                    .collect();
                let last_idx = horizons.len().saturating_sub(1);
                let cols = horizons
                    .into_iter()
                    .enumerate()
                    .map(|(idx, h)| {
                        let spec = self.build_horizon_column(h, today, None, false, strings);
                        self.render_column(
                            spec,
                            today,
                            idx == last_idx,
                            STRIP_MIN_WIDTH,
                            false,
                            cx,
                        )
                    })
                    .collect();
                self.render_horizontal_strip(cols)
            }
            HorizonsMode::Months => {
                let base = Horizon::current_month(today);
                let horizons: Vec<Horizon> = (0..=5)
                    .map(|i| base.shift(i + self.period_offset))
                    .collect();
                let last_idx = horizons.len().saturating_sub(1);
                let cols = horizons
                    .into_iter()
                    .enumerate()
                    .map(|(idx, h)| {
                        let spec = self.build_horizon_column(h, today, None, false, strings);
                        self.render_column(
                            spec,
                            today,
                            idx == last_idx,
                            STRIP_MIN_WIDTH,
                            false,
                            cx,
                        )
                    })
                    .collect();
                self.render_horizontal_strip(cols)
            }
            HorizonsMode::Quarters => {
                let base = Horizon::current_quarter(today);
                let horizons: Vec<Horizon> = (0..=3)
                    .map(|i| base.shift(i + self.period_offset))
                    .collect();
                let last_idx = horizons.len().saturating_sub(1);
                let cols = horizons
                    .into_iter()
                    .enumerate()
                    .map(|(idx, h)| {
                        let spec = self.build_horizon_column(h, today, None, false, strings);
                        self.render_column(
                            spec,
                            today,
                            idx == last_idx,
                            STRIP_MIN_WIDTH,
                            false,
                            cx,
                        )
                    })
                    .collect();
                self.render_horizontal_strip(cols)
            }
            HorizonsMode::Years => {
                let base = Horizon::current_year(today);
                let horizons: Vec<Horizon> = (0..=4)
                    .map(|i| base.shift(i + self.period_offset))
                    .collect();
                let last_idx = horizons.len().saturating_sub(1);
                let cols = horizons
                    .into_iter()
                    .enumerate()
                    .map(|(idx, h)| {
                        let spec = self.build_horizon_column(h, today, None, false, strings);
                        self.render_column(
                            spec,
                            today,
                            idx == last_idx,
                            STRIP_MIN_WIDTH,
                            false,
                            cx,
                        )
                    })
                    .collect();
                self.render_horizontal_strip(cols)
            }
            HorizonsMode::Decades => {
                let base = Horizon::current_decade(today);
                let horizons: Vec<Horizon> = (0..=3)
                    .map(|i| base.shift(i + self.period_offset))
                    .collect();
                let last_idx = horizons.len().saturating_sub(1);
                let cols = horizons
                    .into_iter()
                    .enumerate()
                    .map(|(idx, h)| {
                        let spec = self.build_horizon_column(h, today, None, false, strings);
                        self.render_column(
                            spec,
                            today,
                            idx == last_idx,
                            STRIP_MIN_WIDTH,
                            false,
                            cx,
                        )
                    })
                    .collect();
                self.render_horizontal_strip(cols)
            }
        }
    }
}

impl Render for HorizonsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let today = Self::today();

        div()
            .id("horizons-view")
            .debug_selector(|| "horizons-view".to_owned())
            .key_context("Horizons")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.text)
            .on_action(cx.listener(|this, _: &DismissTransient, window, cx| {
                if !this.dismiss_transient(window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|_this, _: &Search, _, cx| {
                cx.emit(HorizonsEvent::OpenSearch);
            }))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.perform_undo(cx);
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.perform_redo(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.modifiers.secondary() {
                    let key = event.keystroke.key.as_str();
                    if key.eq_ignore_ascii_case("z") && !event.keystroke.modifiers.shift {
                        this.perform_undo(cx);
                    } else if (key.eq_ignore_ascii_case("z") && event.keystroke.modifiers.shift)
                        || key.eq_ignore_ascii_case("y")
                    {
                        this.perform_redo(cx);
                    }
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.active_popover.take().is_some() {
                    cx.notify();
                }
            }))
            .child(self.render_top_strip(today, cx))
            .child(self.render_main_canvas(today, cx))
            .when(self.show_prompt, |el| {
                el.child(self.render_reflection_prompt(cx))
            })
            .when_some(self.detail_task, |el, task_id| {
                el.child(self.render_drawer(task_id, today, cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fern_core::ListDraft;
    use gpui::{Modifiers, TestAppContext, size};

    fn setup_app(cx: &mut TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            assert!(
                desktop_common::keymap::load_default(&assets::Assets, cx)
                    .expect("load default keymap")
                    .is_empty()
            );
        });
    }

    fn bounds_of(cx: &mut gpui::VisualTestContext, selector: impl Into<String>) -> Option<gpui::Bounds<Pixels>> {
        let key: &'static str = Box::leak(selector.into().into_boxed_str());
        cx.debug_bounds(key)
    }

    #[gpui::test]
    fn horizons_fits_six_columns_edge_to_edge_at_1000px_and_persists_collapsed_strips(
        cx: &mut TestAppContext,
    ) {
        setup_app(cx);
        let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
        let today = HorizonsView::today();
        let week_code = Horizon::current_week(today).as_code();
        let month_code = Horizon::current_month(today).as_code();
        let quarter_code = Horizon::current_quarter(today).as_code();
        let year_code = Horizon::current_year(today).as_code();
        let decade_code = Horizon::current_decade(today).as_code();
        let today_key = format!("day-{}", today.format("%Y-%m-%d"));

        let (_view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
        // Simulates 1920px @ 150% DPI inside TodoView (1280 logical - 50 rail - 230 sidebar = 1000px).
        cx.simulate_resize(size(px(1000.), px(720.)));
        cx.run_until_parked();

        // No duplicate inner sidebar exists; top strip bar is rendered cleanly.
        assert!(cx.debug_bounds("horizons-top-strip").is_some());

        let cols = [
            "horizons-col-inbox".to_owned(),
            format!("horizons-col-{today_key}"),
            format!("horizons-col-horizon-{week_code}"),
            format!("horizons-col-horizon-{month_code}"),
            format!("horizons-col-horizon-{year_code}"),
            "horizons-col-horizon-LIFE".to_owned(),
        ];
        for col in &cols {
            let bounds = bounds_of(cx, col.clone()).unwrap_or_else(|| panic!("missing column {col}"));
            assert!(
                bounds.size.width >= px(PANORAMIC_MIN_WIDTH),
                "column {col} width {:?} < {PANORAMIC_MIN_WIDTH}px",
                bounds.size.width
            );
        }
        let life_bounds = bounds_of(cx, "horizons-col-horizon-LIFE").expect("life bounds");
        assert!(
            life_bounds.right() <= px(1001.),
            "all 6 open columns + 2x28px strips must fit edge-to-edge inside 1000px (life right = {:?})",
            life_bounds.right()
        );

        let q_strip = cx.debug_bounds("horizons-strip-quarter").expect("quarter strip");
        let d_strip = cx.debug_bounds("horizons-strip-decade").expect("decade strip");
        assert!((q_strip.size.width - px(28.)).abs() < px(1.));
        assert!((d_strip.size.width - px(28.)).abs() < px(1.));
        assert!(bounds_of(cx, format!("horizons-col-horizon-{quarter_code}")).is_none());
        assert!(bounds_of(cx, format!("horizons-col-horizon-{decade_code}")).is_none());

        // Expand Quarter and Decade -> persists "" so a fresh view keeps all 8 open.
        cx.simulate_click(q_strip.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(bounds_of(cx, format!("horizons-col-horizon-{quarter_code}")).is_some());
        assert_eq!(store.setting(SETTING_COLLAPSED_COLS).unwrap().as_deref(), Some("decade"));

        cx.simulate_resize(size(px(1400.), px(720.)));
        cx.run_until_parked();
        let d_center = cx.debug_bounds("horizons-strip-decade").expect("d strip").center();
        cx.simulate_click(d_center, Modifiers::default());
        cx.run_until_parked();
        assert!(bounds_of(cx, format!("horizons-col-horizon-{decade_code}")).is_some());
        assert_eq!(store.setting(SETTING_COLLAPSED_COLS).unwrap().as_deref(), Some(""));

        let persisted_open = cx.update(|window, cx| {
            let fresh = cx.new(|cx| HorizonsView::new(store.clone(), window, cx));
            fresh.read(cx).collapsed_cols.is_empty()
        });
        assert!(persisted_open, "empty collapsed setting must keep all 8 columns expanded");
    }

    #[gpui::test]
    fn inline_add_creates_tasks_in_each_column_with_mutual_exclusion(cx: &mut TestAppContext) {
        setup_app(cx);
        let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
        let today = HorizonsView::today();
        let week = Horizon::current_week(today);
        let month = Horizon::current_month(today);
        let year = Horizon::current_year(today);

        let (view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
        cx.simulate_resize(size(px(1200.), px(800.)));
        cx.run_until_parked();

        let cases = [
            ("inbox".to_owned(), "Inbox capture"),
            (format!("day-{}", today.format("%Y-%m-%d")), "Today standup"),
            (format!("horizon-{}", week.as_code()), "Weekly release"),
            (format!("horizon-{}", month.as_code()), "Monthly reading"),
            (format!("horizon-{}", year.as_code()), "Yearly marathon"),
            ("horizon-LIFE".to_owned(), "Life dream"),
        ];

        for (col_key, title) in cases {
            let btn = bounds_of(cx, format!("inline-add-{col_key}"))
                .unwrap_or_else(|| panic!("missing inline-add-{col_key}"));
            cx.simulate_click(btn.center(), Modifiers::default());
            cx.run_until_parked();
            cx.simulate_input(title);
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
        }

        assert!(view.read_with(cx, |v, _| v.inline_add.is_some()));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(view.read_with(cx, |v, _| v.inline_add.is_none()));

        let all = store.tasks().expect("tasks");
        let by_title = |name: &str| all.iter().find(|t| t.title == name).expect(name).clone();

        assert!(is_horizons_inbox(&by_title("Inbox capture")));
        assert_eq!(by_title("Today standup").due_date, Some(today));
        assert_eq!(by_title("Today standup").horizon, None);
        assert_eq!(by_title("Weekly release").horizon, Some(week));
        assert_eq!(by_title("Weekly release").due_date, None);
        assert_eq!(by_title("Monthly reading").horizon, Some(month));
        assert_eq!(by_title("Yearly marathon").horizon, Some(year));
        assert_eq!(by_title("Life dream").horizon, Some(Horizon::Life));
    }

    #[gpui::test]
    fn rollover_strip_moves_past_month_and_overdue_tasks_and_undoes(cx: &mut TestAppContext) {
        setup_app(cx);
        let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
        let today = HorizonsView::today();
        let cur_month = Horizon::current_month(today);
        let prev_month = cur_month.shift(-1);

        let past_month_task = store
            .create_task(
                TaskDraft {
                    title: "Carryover goal".into(),
                    horizon: Some(prev_month),
                    ..Default::default()
                },
                false,
            )
            .expect("past month task");
        let overdue_task = store
            .create_task(
                TaskDraft {
                    title: "Overdue invoice".into(),
                    due_date: Some(today - Duration::days(2)),
                    ..Default::default()
                },
                false,
            )
            .expect("overdue task");

        let (view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
        cx.simulate_resize(size(px(1000.), px(720.)));
        cx.run_until_parked();

        let month_btn_id = format!("rollover-move-all-horizon-{}", cur_month.as_code());
        let today_btn_id = format!("rollover-move-all-day-{}", today.format("%Y-%m-%d"));
        let month_btn = bounds_of(cx, month_btn_id.clone()).expect("month rollover move-all button");
        assert!(bounds_of(cx, today_btn_id.clone()).is_some());

        cx.simulate_click(month_btn.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            store.task(past_month_task.id).unwrap().unwrap().horizon,
            Some(cur_month)
        );
        assert!(bounds_of(cx, month_btn_id).is_none());

        cx.update(|_, cx| view.update(cx, |v, cx| v.perform_undo(cx)));
        cx.run_until_parked();
        assert_eq!(
            store.task(past_month_task.id).unwrap().unwrap().horizon,
            Some(prev_month)
        );
        cx.update(|_, cx| view.update(cx, |v, cx| v.perform_redo(cx)));
        cx.run_until_parked();
        assert_eq!(
            store.task(past_month_task.id).unwrap().unwrap().horizon,
            Some(cur_month)
        );

        let today_btn = bounds_of(cx, today_btn_id).expect("today move-all button");
        cx.simulate_click(today_btn.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            store.task(overdue_task.id).unwrap().unwrap().due_date,
            Some(today)
        );
    }

    #[gpui::test]
    fn top_strip_scale_tabs_pagers_and_header_zoom(cx: &mut TestAppContext) {
        setup_app(cx);
        let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
        let today = HorizonsView::today();
        let inbox_item = store.add_task("Triage me").expect("inbox task");

        let (view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
        cx.simulate_resize(size(px(1200.), px(760.)));
        cx.run_until_parked();

        // Click Week header in panoramic view -> zooms into Weeks scale.
        let week_title = bounds_of(
            cx,
            format!("horizons-col-title-horizon-{}", Horizon::current_week(today).as_code()),
        )
        .expect("week title");
        cx.simulate_click(week_title.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.mode), HorizonsMode::Weeks);

        // Switch through all 6 scale tabs via top strip bar and verify period stepper.
        let scale_checks = [
            ("horizons-tab-days", HorizonsMode::Days, format!("horizons-col-day-{}", (today + Duration::days(1)).format("%Y-%m-%d"))),
            ("horizons-tab-weeks", HorizonsMode::Weeks, format!("horizons-col-horizon-{}", Horizon::current_week(today).shift(1).as_code())),
            ("horizons-tab-months", HorizonsMode::Months, format!("horizons-col-horizon-{}", Horizon::current_month(today).shift(1).as_code())),
            ("horizons-tab-quarters", HorizonsMode::Quarters, format!("horizons-col-horizon-{}", Horizon::current_quarter(today).shift(1).as_code())),
            ("horizons-tab-years", HorizonsMode::Years, format!("horizons-col-horizon-{}", Horizon::current_year(today).shift(1).as_code())),
            ("horizons-tab-decades", HorizonsMode::Decades, format!("horizons-col-horizon-{}", Horizon::current_decade(today).shift(1).as_code())),
        ];
        for (tab_id, expected_mode, next_col_id) in scale_checks {
            let tab = cx.debug_bounds(tab_id).unwrap_or_else(|| panic!("missing {tab_id}"));
            cx.simulate_click(tab.center(), Modifiers::default());
            cx.run_until_parked();
            assert_eq!(view.read_with(cx, |v, _| v.mode), expected_mode);
            assert!(bounds_of(cx, next_col_id.clone()).is_some(), "missing {next_col_id}");

            let next_btn = cx.debug_bounds("horizons-pager-next").expect("next pager");
            cx.simulate_click(next_btn.center(), Modifiers::default());
            cx.run_until_parked();
            assert_eq!(view.read_with(cx, |v, _| v.period_offset), 1);

            let cur_btn = cx.debug_bounds("horizons-pager-today").expect("current pager");
            cx.simulate_click(cur_btn.center(), Modifiers::default());
            cx.run_until_parked();
            assert_eq!(view.read_with(cx, |v, _| v.period_offset), 0);
        }

        // Return to Horizons tab, click Inbox column header -> Inbox triage mode.
        let h_tab = cx.debug_bounds("horizons-tab-horizons").expect("horizons tab");
        cx.simulate_click(h_tab.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.mode), HorizonsMode::Horizons);

        let inbox_title = cx.debug_bounds("horizons-col-title-inbox").expect("inbox title");
        cx.simulate_click(inbox_title.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.mode), HorizonsMode::Inbox);

        let triage_week = bounds_of(cx, format!("triage-{}-week", inbox_item.id))
            .expect("triage week chip");
        cx.simulate_click(triage_week.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            store.task(inbox_item.id).unwrap().unwrap().horizon,
            Some(Horizon::current_week(today))
        );
    }

    #[gpui::test]
    fn drawer_quick_horizon_reassign_escape_cascade_and_filters(cx: &mut TestAppContext) {
        setup_app(cx);
        let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
        let today = HorizonsView::today();
        let week = Horizon::current_week(today);

        let work_list = store
            .add_list(&ListDraft {
                name: "Work".into(),
                ..Default::default()
            })
            .expect("work list");
        let task = store
            .create_task(
                TaskDraft {
                    title: "Plan architecture".into(),
                    list_id: Some(work_list.id),
                    horizon: Some(week),
                    tags: vec!["urgent".into()],
                    priority: Priority::High,
                    ..Default::default()
                },
                false,
            )
            .expect("task");
        let other = store
            .create_task(
                TaskDraft {
                    title: "Water plants".into(),
                    horizon: Some(week),
                    tags: vec!["home".into()],
                    ..Default::default()
                },
                false,
            )
            .expect("other task");
        let done_task = store
            .create_task(
                TaskDraft {
                    title: "Done goal".into(),
                    horizon: Some(week),
                    ..Default::default()
                },
                false,
            )
            .expect("done task");
        store.set_completed(done_task.id, true).expect("complete");

        let (view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
        cx.simulate_resize(size(px(1200.), px(800.)));
        cx.run_until_parked();

        // Filter popover opens from top strip bar and filters by tag & show-completed.
        let filter_btn = cx.debug_bounds("horizons-btn-filter").expect("filter btn");
        cx.simulate_click(filter_btn.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("horizons-filter-popover").is_some());

        let tag_chip = cx.debug_bounds("horizons-filter-tag-urgent").expect("urgent chip");
        cx.simulate_click(tag_chip.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(bounds_of(cx, format!("horizons-task-{}", task.id)).is_some());
        assert!(bounds_of(cx, format!("horizons-task-{}", other.id)).is_none());

        let reset = cx.debug_bounds("horizons-filter-reset").expect("reset");
        cx.simulate_click(reset.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(bounds_of(cx, format!("horizons-task-{}", other.id)).is_some());

        let toggle_done = cx.debug_bounds("horizons-toggle-completed").expect("completed toggle");
        cx.simulate_click(toggle_done.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(bounds_of(cx, format!("horizons-task-{}", done_task.id)).is_some());

        // Dismiss popover first.
        assert!(cx.update(|window, cx| view.update(cx, |v, cx| v.dismiss_transient(window, cx))));
        cx.run_until_parked();
        assert!(cx.debug_bounds("horizons-filter-popover").is_none());

        // Click task -> opens TaskDetail drawer, quick chips reassign cleanly.
        let task_row = bounds_of(cx, format!("horizons-task-{}", task.id)).expect("task row");
        cx.simulate_click(task_row.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("horizons-drawer").is_some());

        let chip_year = cx.debug_bounds("horizons-chip-year").expect("year chip");
        cx.simulate_click(chip_year.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(store.task(task.id).unwrap().unwrap().horizon, Some(Horizon::current_year(today)));

        let chip_today = cx.debug_bounds("horizons-chip-today").expect("today chip");
        cx.simulate_click(chip_today.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(store.task(task.id).unwrap().unwrap().due_date, Some(today));
        assert_eq!(store.task(task.id).unwrap().unwrap().horizon, None);

        // Dismiss drawer.
        assert!(cx.update(|window, cx| view.update(cx, |v, cx| v.dismiss_transient(window, cx))));
        cx.run_until_parked();
        assert!(cx.debug_bounds("horizons-drawer").is_none());
        assert!(!cx.update(|window, cx| view.update(cx, |v, cx| v.dismiss_transient(window, cx))));

        // Options popover hides reflection prompt and persists.
        let opt_btn = cx.debug_bounds("horizons-btn-options").expect("options btn");
        cx.simulate_click(opt_btn.center(), Modifiers::default());
        cx.run_until_parked();
        let prompt_toggle = cx.debug_bounds("horizons-opt-prompt").expect("prompt toggle");
        cx.simulate_click(prompt_toggle.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("horizons-prompt").is_none());
        assert_eq!(store.setting(SETTING_SHOW_PROMPT).unwrap().as_deref(), Some("0"));
    }

    #[gpui::test]
    fn typing_in_detail_drawer_updates_card_title_via_task_text_edited(cx: &mut TestAppContext) {
        setup_app(cx);
        let store = Rc::new(Store::open_in_memory().expect("in-memory store"));
        let today = HorizonsView::today();
        let task = store
            .create_task(
                TaskDraft {
                    title: "Initial horizon".into(),
                    due_date: Some(today),
                    ..Default::default()
                },
                false,
            )
            .expect("task");

        let (view, cx) = cx.add_window_view(|window, cx| HorizonsView::new(store.clone(), window, cx));
        cx.simulate_resize(size(px(1200.), px(800.)));
        cx.run_until_parked();

        let task_row = bounds_of(cx, format!("horizons-task-{}", task.id)).expect("task row");
        cx.simulate_click(task_row.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("horizons-drawer").is_some());

        let title_bounds = cx.debug_bounds("task-detail-title").expect("detail title visible");
        cx.simulate_click(title_bounds.center(), Modifiers::default());
        cx.simulate_keystrokes("end");
        cx.simulate_input("!");
        cx.run_until_parked();

        cx.update(|_, cx| {
            let v = view.read(cx);
            assert_eq!(v.tasks[0].title, "Initial horizon!");
            assert_eq!(v.detail.read(cx).task_title(), Some("Initial horizon!"));
            assert!(cx.try_global::<DataVersion>().is_none());
        });
        assert_eq!(store.task(task.id).unwrap().unwrap().title, "Initial horizon!");

        let observer = cx.update(|_, cx| cx.new(|_| ()));
        let close_btn = cx.debug_bounds("horizons-drawer-close").expect("drawer close button");
        cx.simulate_click(close_btn.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("horizons-drawer").is_none());
        cx.update(|_, cx| assert!(changed_elsewhere(observer.entity_id(), cx)));
    }
}
