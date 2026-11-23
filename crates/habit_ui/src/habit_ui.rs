//! The Habit section: a 7-day progress strip with single-date filtering, sectioned & reorderable
//! habit cards/rows with check-in modes, and a resizable details panel with stats, cycles,
//! monthly calendar, daily goals chart, and mood-tagged habit logs.

mod cards;
mod detail;
mod dialogs;
mod modal;
#[cfg(all(test, debug_assertions))]
mod tests;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use chrono::{Datelike, Duration, Local, NaiveDate};
use fern_core::habit::{
    self, Checkin, CheckinMode, CustomSection, DayProgress, Habit, HabitSection, done_dates, goal_cycles,
};
use fern_core::{Id, Store};
use gpui::{Entity, Focusable as _, FontWeight, Point, ScrollHandle};
use ui::prelude::*;
use ui::{EmptyState, Placement, PopoverAttachment, popover};

use desktop_common::data_version::{DataVersion, changed_elsewhere, data_changed};
use desktop_common::habit_helpers::progress_ring;
use desktop_common::text_input::{Cancelled, Mode, Submitted, TextInput};
use desktop_common::{actions, calendar, export, layout, resize};
use fern_i18n as i18n;

pub use desktop_common::habit_helpers::{HABIT_EMOJI, IN_TODAY_SETTING, avatar, icon_avatar};

use cards::{CardContextMenu, DayContextMenu, HeaderMore};
use dialogs::{AmountModal, CompletedCyclesModal, ConfirmAction, CycleCompletedModal, HabitLogModal};
use modal::HabitEdit;

const DEFAULT_DETAIL_WIDTH: f32 = 380.;
const MIN_DETAIL_WIDTH: f32 = 300.;
const MAX_DETAIL_WIDTH: f32 = 620.;

const LAYOUT_SETTING: &str = "habit_layout";
pub(crate) const SORT_SETTING: &str = "habits_sort_by_status";
const DETAIL_WIDTH_SETTING: &str = "habit_detail_width";
const COLLAPSED_SECTIONS_SETTING: &str = "habit_collapsed_sections";
pub(crate) const STREAK_BEST_SETTING: &str = "habit_streak_best";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetailSubmenu {
    StartFocus,
    CheckinStyle,
}

/// "Start Focus" on a habit: a Pomodoro or a stopwatch linked to `habit_id`.
pub struct StartFocus {
    pub habit_id: Option<Id>,
    pub title: String,
    pub stopwatch: bool,
}

impl gpui::EventEmitter<StartFocus> for HabitView {}

enum HabitGroupKind {
    Section { section: HabitSection, grouped: bool },
    Completed,
}

struct HabitGroup<'a> {
    kind: HabitGroupKind,
    habits: Vec<&'a Habit>,
    collapsed: bool,
}

pub struct HabitView {
    store: Rc<Store>,
    focus_handle: gpui::FocusHandle,
    grid_scroll: ScrollHandle,
    card_bounds: Rc<RefCell<HashMap<Id, gpui::Bounds<Pixels>>>>,
    habits: Rc<Vec<Habit>>,
    /// Completed (`Done`) dates per habit.
    checkins: HashMap<Id, HashSet<NaiveDate>>,
    /// All daily records (Done, Undone, partial, log-only) per habit.
    records: HashMap<Id, Vec<Checkin>>,
    sections: Vec<CustomSection>,
    selected: Option<Id>,
    /// Single-date filter toggled by clicking a day in the 7-day week strip.
    selected_date: Option<NaiveDate>,
    /// Month currently displayed in the details calendar & chart.
    month: NaiveDate,
    show_archived: bool,
    list_layout: bool,
    in_today: bool,
    sort_by_status: bool,
    show_best_streak: bool,
    detail_width: f32,
    resizing_detail: bool,
    collapsed_sections: HashSet<String>,
    expanded_logs: HashSet<(Id, NaiveDate)>,
    // Menus & modals
    header_menu_open: bool,
    header_more: Option<HeaderMore>,
    detail_menu_open: bool,
    detail_submenu: Option<DetailSubmenu>,
    section_menu: Option<Id>,
    renaming_section: Option<(Id, Entity<TextInput>)>,
    card_menu: Option<CardContextMenu>,
    day_menu: Option<DayContextMenu>,
    editing: Option<HabitEdit>,
    log_modal: Option<HabitLogModal>,
    amount_modal: Option<AmountModal>,
    confirm: Option<ConfirmAction>,
    cycle_completed: Option<CycleCompletedModal>,
    cycles_modal: Option<CompletedCyclesModal>,
    export_message: Option<String>,
    detail_notice: Option<String>,
    active: bool,
    dirty: bool,
}

impl gpui::Focusable for HabitView {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl HabitView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, true, cx)
    }

    pub fn new_inactive(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, false, cx)
    }

    fn new_with_active(store: Rc<Store>, initial_active: bool, cx: &mut Context<Self>) -> Self {
        let list_layout = store.setting(LAYOUT_SETTING).ok().flatten().as_deref() == Some("list");
        let detail_width = store
            .setting(DETAIL_WIDTH_SETTING)
            .ok()
            .flatten()
            .and_then(|v| v.parse::<f32>().ok())
            .map(|w| w.clamp(MIN_DETAIL_WIDTH, MAX_DETAIL_WIDTH))
            .unwrap_or(DEFAULT_DETAIL_WIDTH);
        let collapsed_sections: HashSet<String> = store
            .setting(COLLAPSED_SECTIONS_SETTING)
            .ok()
            .flatten()
            .map(|s| s.split(',').filter(|k| !k.is_empty()).map(str::to_owned).collect())
            .unwrap_or_default();

        cx.observe_global::<DataVersion>(|this, cx| {
            if changed_elsewhere(cx.entity_id(), cx) {
                this.sync_external(cx);
            }
        })
        .detach();

        let mut this = Self {
            store,
            focus_handle: cx.focus_handle(),
            grid_scroll: ScrollHandle::new(),
            card_bounds: Rc::new(RefCell::new(HashMap::new())),
            habits: Rc::new(Vec::new()),
            checkins: HashMap::new(),
            records: HashMap::new(),
            sections: Vec::new(),
            selected: None,
            selected_date: None,
            month: calendar::month_start(Local::now().date_naive()),
            show_archived: false,
            list_layout,
            in_today: true,
            sort_by_status: false,
            show_best_streak: false,
            detail_width,
            resizing_detail: false,
            collapsed_sections,
            expanded_logs: HashSet::new(),
            header_menu_open: false,
            header_more: None,
            detail_menu_open: false,
            detail_submenu: None,
            section_menu: None,
            renaming_section: None,
            card_menu: None,
            day_menu: None,
            editing: None,
            log_modal: None,
            amount_modal: None,
            confirm: None,
            cycle_completed: None,
            cycles_modal: None,
            export_message: None,
            detail_notice: None,
            active: initial_active,
            dirty: !initial_active,
        };
        if initial_active {
            this.reload(cx);
        }
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

    pub(crate) fn notify_data_changed(&self, cx: &mut Context<Self>) {
        data_changed(cx.entity_id(), cx);
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        self.habits = Rc::new(self.store.habits().unwrap_or_default());
        self.sections = self.store.habit_sections().unwrap_or_default();
        self.records = self.store.all_habit_checkins().unwrap_or_default();
        self.in_today = self
            .store
            .setting(IN_TODAY_SETTING)
            .ok()
            .flatten()
            .is_none_or(|v| v == "1");
        self.sort_by_status = self
            .store
            .setting(SORT_SETTING)
            .ok()
            .flatten()
            .is_some_and(|v| v == "1");
        self.show_best_streak = self
            .store
            .setting(STREAK_BEST_SETTING)
            .ok()
            .flatten()
            .is_some_and(|v| v == "1");
        self.checkins = self
            .habits
            .iter()
            .map(|h| {
                let done = self
                    .records
                    .get(&h.id)
                    .map(|recs| done_dates(recs))
                    .unwrap_or_default();
                (h.id, done)
            })
            .collect();
        if self.selected.is_some_and(|id| !self.habits.iter().any(|h| h.id == id)) {
            self.selected = None;
        }
        cx.notify();
    }

    /// "Record" in a reminder: selects the habit and opens today's Total amount modal (or Habit Log).
    pub fn record(&mut self, habit: Id, window: &mut Window, cx: &mut Context<Self>) {
        self.select(habit, cx);
        let today = Local::now().date_naive();
        if self.habits.iter().find(|h| h.id == habit).is_some_and(|h| h.goal.is_some()) {
            self.open_amount_modal(habit, today, false, window, cx);
        } else {
            self.open_log_modal(habit, today, false, window, cx);
        }
    }

    pub fn select(&mut self, habit: Id, cx: &mut Context<Self>) {
        if self.dirty {
            self.reload(cx);
        }
        if let Some(found) = self.habits.iter().find(|h| h.id == habit)
            && found.archived != self.show_archived
        {
            self.show_archived = found.archived;
            if self.show_archived {
                self.selected_date = None;
            }
        }
        if self.selected != Some(habit) {
            self.selected = Some(habit);
            self.month = calendar::month_start(Local::now().date_naive());
            self.detail_menu_open = false;
            self.detail_submenu = None;
            self.detail_notice = None;
        }
        cx.notify();
    }

    pub fn dismiss_transient(&mut self, cx: &mut Context<Self>) -> bool {
        if self.confirm.take().is_some()
            || self.cycle_completed.take().is_some()
            || self.cycles_modal.take().is_some()
            || self.amount_modal.take().is_some()
            || self.log_modal.take().is_some()
        {
            cx.notify();
            return true;
        }
        if let Some(edit) = self.editing.as_mut()
            && (edit.icon_picker.take().is_some()
                || edit.start_calendar.take().is_some()
                || edit.new_section.take().is_some()
                || edit.goal_days_input.take().is_some()
                || edit.reminder_picker.take().is_some()
                || edit.open.take().is_some())
        {
            cx.notify();
            return true;
        }
        if std::mem::take(&mut self.header_menu_open)
            || self.header_more.take().is_some()
            || std::mem::take(&mut self.detail_menu_open)
            || self.section_menu.take().is_some()
            || self.card_menu.take().is_some()
            || self.day_menu.take().is_some()
            || self.editing.take().is_some()
            || self.renaming_section.take().is_some()
        {
            self.detail_submenu = None;
            cx.notify();
            true
        } else {
            false
        }
    }

    pub(crate) fn flag(&self, key: &str, default: bool) -> bool {
        match key {
            IN_TODAY_SETTING => self.in_today,
            SORT_SETTING => self.sort_by_status,
            STREAK_BEST_SETTING => self.show_best_streak,
            _ => default,
        }
    }

    pub(crate) fn set_flag(&mut self, key: &str, on: bool, cx: &mut Context<Self>) {
        match key {
            IN_TODAY_SETTING => self.in_today = on,
            SORT_SETTING => self.sort_by_status = on,
            STREAK_BEST_SETTING => self.show_best_streak = on,
            _ => {}
        }
        if let Err(err) = self.store.set_setting(key, if on { "1" } else { "0" }) {
            eprintln!("failed to save setting {key}: {err:#}");
        }
        self.notify_data_changed(cx);
        cx.notify();
    }

    pub(crate) fn set_list_layout(&mut self, list: bool, cx: &mut Context<Self>) {
        self.list_layout = list;
        if let Err(err) = self.store.set_setting(LAYOUT_SETTING, if list { "list" } else { "cards" }) {
            eprintln!("failed to save habit layout: {err:#}");
        }
        cx.notify();
    }

    pub(crate) fn toggle_section_collapsed(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.collapsed_sections.remove(&key) {
            self.collapsed_sections.insert(key);
        }
        let mut keys: Vec<&str> = self.collapsed_sections.iter().map(String::as_str).collect();
        keys.sort_unstable();
        let _ = self.store.set_setting(COLLAPSED_SECTIONS_SETTING, &keys.join(","));
        cx.notify();
    }

    pub(crate) fn set_archived(&mut self, id: Id, archived: bool, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_habit_archived(id, archived) {
            eprintln!("failed to update archive status: {err:#}");
        }
        self.detail_menu_open = false;
        self.detail_submenu = None;
        self.card_menu = None;
        if self.selected == Some(id) && archived != self.show_archived {
            self.selected = None;
        }
        self.notify_data_changed(cx);
        self.reload(cx);
    }

    pub(crate) fn section_label(&self, section: HabitSection, cx: &App) -> String {
        let strings = i18n::t(cx);
        match section {
            HabitSection::Morning => strings.habit_sections[0].to_owned(),
            HabitSection::Afternoon => strings.habit_sections[1].to_owned(),
            HabitSection::Night => strings.habit_sections[2].to_owned(),
            HabitSection::Others => strings.habit_sections[3].to_owned(),
            HabitSection::Custom(id) => self
                .sections
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| strings.habit_sections[3].to_owned()),
        }
    }

    pub(crate) fn day_progress(&self, date: NaiveDate) -> DayProgress {
        habit::day_progress(&self.habits, &self.records, date, calendar::week_start())
    }

    pub(crate) fn open_day_menu(&mut self, habit_id: Id, date: NaiveDate, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.habits.iter().find(|h| h.id == habit_id).is_none_or(|h| h.archived) {
            return;
        }
        let today = Local::now().date_naive();
        if date > today {
            return;
        }
        if (today - date).num_days() > habit::MAX_EDITABLE_PAST_DAYS {
            self.day_menu = None;
            self.detail_notice = Some(i18n::t(cx).habit_past_limit.to_owned());
            cx.notify();
            return;
        }
        self.detail_notice = None;
        self.card_menu = None;
        self.day_menu = Some(DayContextMenu { habit_id, date, position });
        cx.notify();
    }

    pub(crate) fn open_card_menu(&mut self, habit_id: Id, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.day_menu = None;
        self.card_menu = Some(CardContextMenu { habit_id, position });
        cx.notify();
    }

    /// Primary left-click on a check-in dot (on card/list or boolean detail calendar).
    pub(crate) fn handle_dot_click(&mut self, habit_id: Id, date: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        if date > today {
            return;
        }
        let Some(habit) = self.habits.iter().find(|h| h.id == habit_id).cloned() else {
            return;
        };
        if habit.archived {
            return;
        }
        let is_done = self.checkins.get(&habit_id).is_some_and(|d| d.contains(&date));
        match habit.goal.as_ref().map(|g| g.mode) {
            None | Some(CheckinMode::All) => {
                let res = self.store.toggle_checkin(habit_id, date);
                match res {
                    Ok(now_done) => {
                        self.notify_data_changed(cx);
                        self.reload(cx);
                        if !is_done && now_done {
                            self.after_checkin_completed(habit_id, date, Some(window), cx);
                        }
                    }
                    Err(err) => eprintln!("failed to toggle check-in: {err:#}"),
                }
            }
            Some(CheckinMode::Auto { .. }) => {
                if is_done {
                    self.confirm = Some(ConfirmAction::ResetCheckin { habit_id, date });
                    cx.notify();
                } else {
                    if let Err(err) = self.store.step_checkin(habit_id, date) {
                        eprintln!("failed to step check-in: {err:#}");
                        return;
                    }
                    self.notify_data_changed(cx);
                    self.reload(cx);
                    let now_done = self.checkins.get(&habit_id).is_some_and(|d| d.contains(&date));
                    if now_done {
                        self.after_checkin_completed(habit_id, date, Some(window), cx);
                    }
                }
            }
            Some(CheckinMode::Manual) => {
                if is_done {
                    self.confirm = Some(ConfirmAction::ResetCheckin { habit_id, date });
                    cx.notify();
                } else {
                    self.open_amount_modal(habit_id, date, true, window, cx);
                }
            }
        }
    }

    pub(crate) fn after_checkin_completed(
        &mut self,
        habit_id: Id,
        date: NaiveDate,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let Some(habit) = self.habits.iter().find(|h| h.id == habit_id).cloned() else {
            return;
        };
        desktop_common::sound::play_complete(&self.store);
        // Check if this check-in just completed a Goal Days cycle.
        if let Some(target) = habit.goal_days
            && target > 0
            && habit.start_date.is_none_or(|start| date >= start)
            && let Some(done) = self.checkins.get(&habit_id)
            && let Some(summary) = goal_cycles(&habit, done)
            && summary.current_count == 0
            && !summary.completed.is_empty()
        {
            self.cycle_completed = Some(CycleCompletedModal { habit_id, target_days: target });
            cx.notify();
            return;
        }
        // Auto-pop Habit Log when enabled.
        if habit.record_enable
            && let Some(window) = window
        {
            self.open_log_modal(habit_id, date, true, window, cx);
        }
    }

    pub(crate) fn mark_undone(&mut self, habit_id: Id, date: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
        let Some(habit) = self.habits.iter().find(|h| h.id == habit_id).cloned() else {
            return;
        };
        if habit.archived {
            return;
        }
        if let Err(err) = self.store.mark_checkin_undone(habit_id, date) {
            eprintln!("failed to mark unachieved: {err:#}");
            return;
        }
        self.day_menu = None;
        self.notify_data_changed(cx);
        self.reload(cx);
        if habit.record_enable {
            self.open_log_modal(habit_id, date, true, window, cx);
        }
    }

    pub(crate) fn toggle_skip(&mut self, habit_id: Id, date: NaiveDate, cx: &mut Context<Self>) {
        let skipped = self
            .habits
            .iter()
            .find(|h| h.id == habit_id)
            .is_some_and(|h| h.is_skipped(date));
        let res = if skipped {
            self.store.unskip_habit_date(habit_id, date)
        } else {
            self.store.skip_habit_date(habit_id, date)
        };
        if let Err(err) = res {
            eprintln!("failed to toggle skip: {err:#}");
        }
        self.day_menu = None;
        self.notify_data_changed(cx);
        self.reload(cx);
    }

    pub(crate) fn export_csv(&mut self, cx: &mut Context<Self>) {
        let data: Vec<(Habit, Vec<Checkin>)> = self
            .habits
            .iter()
            .map(|h| (h.clone(), self.records.get(&h.id).cloned().unwrap_or_default()))
            .collect();
        let csv = habit::export_csv(&data);
        self.header_more = None;
        export::save_as("habits.csv".into(), csv.into_bytes(), cx, |this, result, cx| {
            this.export_message = export::outcome(result, i18n::t(cx));
            cx.notify();
        });
        cx.notify();
    }

    pub(crate) fn start_rename_section(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.sections.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_default();
        let strings = i18n::t(cx);
        let input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.habit_section_name, cx);
            input.set_text(current, cx);
            input
        });
        cx.subscribe(&input, move |this, _, Submitted(text), cx| {
            if let Err(err) = this.store.rename_habit_section(id, text) {
                eprintln!("failed to rename section: {err:#}");
            }
            this.renaming_section = None;
            this.notify_data_changed(cx);
            this.reload(cx);
        })
        .detach();
        cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
            this.renaming_section = None;
            cx.notify();
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.section_menu = None;
        self.renaming_section = Some((id, input));
        cx.notify();
    }

    pub(crate) fn reorder_habit_drop(&mut self, dragged_id: Id, target_section: HabitSection, before_id: Option<Id>, cx: &mut Context<Self>) {
        if self.show_archived {
            return;
        }
        let mut active: Vec<Habit> = self.habits.iter().filter(|h| !h.archived).cloned().collect();
        let Some(pos) = active.iter().position(|h| h.id == dragged_id) else {
            return;
        };
        let mut moved = active.remove(pos);
        moved.section = target_section;

        let insert_idx = match before_id {
            Some(target_id) => active.iter().position(|h| h.id == target_id).unwrap_or(active.len()),
            None => active
                .iter()
                .rposition(|h| h.section == target_section)
                .map_or(active.len(), |i| i + 1),
        };
        active.insert(insert_idx, moved);

        let updates: Vec<(Id, HabitSection, i64)> = active
            .iter()
            .enumerate()
            .map(|(idx, h)| (h.id, h.section, idx as i64))
            .collect();
        if let Err(err) = self.store.reorder_habits(&updates) {
            eprintln!("failed to reorder habits: {err:#}");
        }
        self.notify_data_changed(cx);
        self.reload(cx);
    }

    pub(crate) fn reorder_custom_section_drop(&mut self, dragged_id: Id, target_id: Id, cx: &mut Context<Self>) {
        if dragged_id == target_id {
            return;
        }
        let mut ids: Vec<Id> = self.sections.iter().map(|s| s.id).collect();
        let Some(from) = ids.iter().position(|id| *id == dragged_id) else {
            return;
        };
        let item = ids.remove(from);
        let to = ids.iter().position(|id| *id == target_id).unwrap_or(ids.len());
        ids.insert(to, item);
        if let Err(err) = self.store.reorder_habit_sections(&ids) {
            eprintln!("failed to reorder habit sections: {err:#}");
        }
        self.notify_data_changed(cx);
        self.reload(cx);
    }

    fn has_modal_open(&self) -> bool {
        self.editing.is_some()
            || self.log_modal.is_some()
            || self.amount_modal.is_some()
            || self.confirm.is_some()
            || self.cycle_completed.is_some()
            || self.cycles_modal.is_some()
    }

    fn has_popover_open(&self) -> bool {
        self.header_menu_open
            || self.header_more.is_some()
            || self.detail_menu_open
            || self.section_menu.is_some()
            || self.card_menu.is_some()
            || self.day_menu.is_some()
            || self.renaming_section.is_some()
    }

    fn is_handled_on(&self, habit: &Habit, day: NaiveDate) -> bool {
        self.records
            .get(&habit.id)
            .and_then(|recs| recs.iter().find(|r| r.date == day))
            .is_some_and(|r| r.is_handled())
    }

    fn visible_sections<'a>(&self, habits: &'a [Habit], today: NaiveDate) -> Vec<HabitGroup<'a>> {
        let week_start = calendar::week_start();
        let visible: Vec<&Habit> = habits
            .iter()
            .filter(|h| h.archived == self.show_archived)
            .filter(|h| match self.selected_date {
                Some(day) if !self.show_archived => {
                    let recs = self.records.get(&h.id).map(Vec::as_slice).unwrap_or(&[]);
                    h.is_active_on(day, recs, week_start)
                }
                _ => true,
            })
            .collect();

        let sort_by_status = self.flag(SORT_SETTING, false);
        let split_completed = self.selected_date.is_some() && !self.show_archived && sort_by_status;
        let (main_visible, completed_visible): (Vec<&Habit>, Vec<&Habit>) = if split_completed {
            let day = self.selected_date.unwrap_or(today);
            visible.into_iter().partition(|h| !self.is_handled_on(h, day))
        } else {
            (visible, Vec::new())
        };

        let grouped = main_visible.iter().any(|h| h.section != HabitSection::Others) || !self.sections.is_empty();
        let mut groups = Vec::new();
        for (i, section) in HabitSection::ordered(&self.sections).into_iter().enumerate() {
            let mut in_section: Vec<&Habit> = main_visible
                .iter()
                .copied()
                .filter(|h| !grouped || h.section == section)
                .collect();
            if sort_by_status && !split_completed {
                let day = self.selected_date.unwrap_or(today);
                in_section.sort_by_key(|h| self.is_handled_on(h, day));
            }
            if in_section.is_empty() || (!grouped && i > 0) {
                continue;
            }
            let collapsed = grouped && self.collapsed_sections.contains(&section.key());
            groups.push(HabitGroup {
                kind: HabitGroupKind::Section { section, grouped },
                habits: in_section,
                collapsed,
            });
        }

        if !completed_visible.is_empty() {
            let collapsed = self.collapsed_sections.contains("completed");
            groups.push(HabitGroup {
                kind: HabitGroupKind::Completed,
                habits: completed_visible,
                collapsed,
            });
        }
        groups
    }

    fn visible_habit_ids(&self, today: NaiveDate) -> Vec<Id> {
        self.visible_sections(&self.habits, today)
            .into_iter()
            .filter(|g| !g.collapsed)
            .flat_map(|g| g.habits.into_iter().map(|h| h.id))
            .collect()
    }

    fn scroll_habit_into_view(&self, habit_id: Id, is_first: bool) {
        let viewport = self.grid_scroll.bounds();
        if viewport.size.height <= px(0.) {
            return;
        }
        let Some(unscrolled) = self.card_bounds.borrow().get(&habit_id).copied() else {
            return;
        };
        let cur = self.grid_scroll.offset();
        let top = unscrolled.top() + cur.y;
        let bottom = unscrolled.bottom() + cur.y;
        let mut next_y = cur.y;
        if top < viewport.top() {
            next_y = if is_first {
                px(0.)
            } else {
                next_y + (viewport.top() - top)
            };
        } else if bottom > viewport.bottom() {
            next_y += viewport.bottom() - bottom;
        } else {
            return;
        }
        let max_y = self.grid_scroll.max_offset().y.max(px(0.));
        self.grid_scroll
            .set_offset(gpui::point(cur.x, next_y.clamp(-max_y, px(0.))));
    }

    fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        let ids = self.visible_habit_ids(today);
        if ids.is_empty() {
            return;
        }
        let next = match self.selected.and_then(|sel| ids.iter().position(|id| *id == sel)) {
            Some(idx) => {
                let target = (idx as isize + delta).clamp(0, ids.len() as isize - 1) as usize;
                ids[target]
            }
            None if delta < 0 => ids[ids.len() - 1],
            None => ids[0],
        };
        self.select(next, cx);
        self.scroll_habit_into_view(next, next == ids[0]);
    }

    #[cfg(debug_assertions)]
    pub fn dev_apply(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        use fern_core::{Goal, HabitFrequency};
        if self.dirty {
            self.reload(cx);
        }
        let custom = |name: &str| self.sections.iter().find(|s| s.name == name).map(|s| s.id);
        if let Some(id) = command.strip_prefix("section-menu=").and_then(custom) {
            self.section_menu = Some(id);
            cx.notify();
            return;
        }
        if let Some(id) = command.strip_prefix("section-rename=").and_then(custom) {
            self.start_rename_section(id, window, cx);
            return;
        }
        let Some(id) = self.selected else { return };
        if let Some(submenu) = command.strip_prefix("menu=") {
            self.detail_menu_open = true;
            self.detail_submenu = match submenu {
                "focus" => Some(DetailSubmenu::StartFocus),
                "style" => Some(DetailSubmenu::CheckinStyle),
                _ => None,
            };
            cx.notify();
            return;
        }
        if command == "icon-checkins" {
            self.store.set_habit_icon_checkins(id, true).ok();
            self.reload(cx);
            return;
        }
        let name = self.habits.iter().find(|h| h.id == id).map(|h| h.name.clone()).unwrap_or_default();
        if let Some(goal) = command.strip_prefix("goal=") {
            let (amount, unit) = goal.split_once(' ').unwrap_or((goal, ""));
            let goal = amount.parse::<f64>().ok().map(|amount| Goal::new(amount, unit));
            self.store.update_habit(id, &name, goal.as_ref()).ok();
        } else if let Some(note) = command.strip_prefix("note=") {
            let (days_ago, note) = note.split_once('>').unwrap_or(("0", note));
            let date = Local::now().date_naive() - Duration::days(days_ago.parse().unwrap_or(0));
            self.store.set_checkin_log(id, date, Some(note), 50).ok();
        } else if let Some(step) = command.strip_prefix("step=") {
            let (ago, times) = step.split_once('x').unwrap_or((step, "1"));
            let date = Local::now().date_naive() - Duration::days(ago.parse().unwrap_or(0));
            for _ in 0..times.parse().unwrap_or(1) {
                self.store.step_checkin(id, date).ok();
            }
        } else if let Some(times) = command.strip_prefix("remind=") {
            let times: Vec<chrono::NaiveTime> = times.split(',').filter_map(calendar::parse_time).collect();
            self.store.set_habit_reminders(id, &times).ok();
        } else if let Some(frequency) = command.strip_prefix("freq=") {
            self.store.set_habit_frequency(id, HabitFrequency::from_storage(frequency)).ok();
        } else if let Some(look) = command.strip_prefix("look=") {
            let (icon, color) = look.split_once('|').unwrap_or((look, ""));
            self.store.set_habit_look(id, Some(icon), (!color.is_empty()).then_some(color)).ok();
        } else if let Some(span) = command.strip_prefix("span=") {
            let (ago, days) = span.split_once(',').unwrap_or((span, "0"));
            let start = Local::now().date_naive() - Duration::days(ago.parse().unwrap_or(0));
            self.store.set_habit_span(id, Some(start), days.parse().ok()).ok();
        } else if let Some(section) = command.strip_prefix("section=") {
            let section = match section.strip_prefix("new:") {
                Some(name) => self.store.add_habit_section(name).map(|s| HabitSection::Custom(s.id)).unwrap_or_default(),
                None => HabitSection::from_key(section),
            };
            self.store.set_habit_section(id, section).ok();
        } else if command == "archive" {
            self.store.set_habit_archived(id, true).ok();
        } else if command == "settings" {
            self.header_more = Some(HeaderMore::Settings);
            cx.notify();
            return;
        } else if command == "more" {
            self.header_more = Some(HeaderMore::Menu);
            cx.notify();
            return;
        } else if command == "header" {
            self.header_menu_open = true;
            cx.notify();
            return;
        } else if command == "list" {
            self.set_list_layout(true, cx);
            return;
        } else if command == "show-archived" {
            self.show_archived = true;
        } else if let Some(which) = command.strip_prefix("row=") {
            let which = match which {
                "goal" => modal::EditRow::Goal,
                "section" => modal::EditRow::Section,
                _ => modal::EditRow::Frequency,
            };
            self.update_edit(cx, |e| e.open = Some(which));
            return;
        } else if command == "picker" {
            self.update_edit(cx, |e| e.icon_picker = Some(0));
            return;
        } else if command == "edit" {
            self.reload(cx);
            self.open_edit_modal(id, window, cx);
            return;
        }
        self.reload(cx);
    }

    #[cfg(debug_assertions)]
    pub fn dev_add(&mut self, name: &str, days_ago: &[i64], cx: &mut Context<Self>) {
        if let Ok(habit) = self.store.add_habit(name) {
            let today = Local::now().date_naive();
            for &ago in days_ago {
                let _ = self.store.toggle_checkin(habit.id, today - Duration::days(ago));
            }
            self.reload(cx);
        }
    }

    #[cfg(debug_assertions)]
    pub fn dev_select(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.dirty {
            self.reload(cx);
        }
        if let Some(id) = self.habits.iter().find(|h| h.name == name).map(|h| h.id) {
            self.select(id, cx);
        }
    }
}

impl Render for HabitView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let week: [NaiveDate; 7] = std::array::from_fn(|i| today - Duration::days(6 - i as i64));

        self.card_bounds.borrow_mut().clear();
        let habits = Rc::clone(&self.habits);
        let groups = self.visible_sections(&habits, today);
        let mut sections_ui: Vec<AnyElement> = Vec::new();
        for group in groups {
            let allow_dnd = matches!(group.kind, HabitGroupKind::Section { .. });
            let card_ids: Vec<Id> = group.habits.iter().map(|h| h.id).collect();
            let section_cards: Vec<AnyElement> = if group.collapsed {
                Vec::new()
            } else {
                group
                    .habits
                    .iter()
                    .map(|h| self.render_card(h, week, today, allow_dnd, cx))
                    .collect()
            };
            let card_bounds = Rc::clone(&self.card_bounds);
            let grid_scroll = self.grid_scroll.clone();
            let cards_wrap = div()
                .flex()
                .flex_wrap()
                .gap(px(if self.list_layout { 10. } else { 12. }))
                .children(section_cards)
                .on_children_prepainted(move |children_bounds, _, _| {
                    let offset = grid_scroll.offset();
                    let mut map = card_bounds.borrow_mut();
                    for (idx, habit_id) in card_ids.iter().enumerate() {
                        if let Some(bounds) = children_bounds.get(idx) {
                            map.insert(*habit_id, gpui::Bounds::new(bounds.origin - offset, bounds.size));
                        }
                    }
                });

            let group_el = match group.kind {
                HabitGroupKind::Section { section, grouped } => {
                    let title = self.section_label(section, cx);
                    v_flex()
                        .w_full()
                        .gap_2()
                        .when(grouped, |el| {
                            el.child(self.render_section_header(section, title, group.habits.len(), true, cx))
                        })
                        .when(!group.collapsed, |el| el.child(cards_wrap))
                        .into_any_element()
                }
                HabitGroupKind::Completed => v_flex()
                    .w_full()
                    .gap_2()
                    .pt_2()
                    .child(
                        h_flex()
                            .id("habit-sec-completed")
                            .h(px(30.))
                            .gap_1()
                            .items_center()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_section_collapsed("completed".to_owned(), cx);
                            }))
                            .child(
                                Icon::new(if group.collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(
                                Label::new(strings.habit_completed_section)
                                    .size(LabelSize::Small)
                                    .weight(FontWeight::MEDIUM)
                                    .color(Color::Muted),
                            )
                            .child(Label::new(group.habits.len().to_string()).size(LabelSize::XSmall).color(Color::Muted)),
                    )
                    .when(!group.collapsed, |el| el.child(cards_wrap))
                    .into_any_element(),
            };
            sections_ui.push(group_el);
        }

        // Interactive 7-day progress strip (Active view only).
        let strip = (!self.show_archived).then(|| {
            h_flex()
                .debug_selector(|| "habit-week-progress".to_owned())
                .px(px(22.))
                .pt(px(14.))
                .pb(px(20.))
                .children(week.map(|date| {
                    let progress = self.day_progress(date);
                    let is_today = date == today;
                    let is_selected = self.selected_date == Some(date);
                    let ring_el: AnyElement = match progress {
                        DayProgress::Needless => div()
                            .size(px(24.))
                            .rounded_full()
                            .border_1()
                            .border_color(theme.colors().border.opacity(0.45))
                            .into_any_element(),
                        DayProgress::AllUndone => div()
                            .size(px(24.))
                            .rounded_full()
                            .border_1()
                            .border_color(theme.status().error.opacity(0.6))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::X).size(IconSize::XSmall).color(Color::Error))
                            .into_any_element(),
                        DayProgress::Fraction(frac) if frac >= 0.999 => div()
                            .size(px(24.))
                            .rounded_full()
                            .bg(theme.colors().accent)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::IconOnAccent))
                            .into_any_element(),
                        DayProgress::Fraction(frac) => progress_ring(
                            px(24.),
                            frac,
                            theme.colors().accent,
                            theme.colors().border.opacity(0.6),
                            px(2.5),
                        )
                        .into_any_element(),
                    };

                    let day_tip = match progress {
                        DayProgress::Needless => calendar::format_date(date, strings),
                        DayProgress::AllUndone => format!("{} · {}", calendar::format_date(date, strings), strings.habit_unachieved),
                        DayProgress::Fraction(frac) => format!("{} · {}%", calendar::format_date(date, strings), (frac * 100.0).round() as u32),
                    };

                    v_flex()
                        .id(SharedString::from(format!("week-day-{date}")))
                        .flex_1()
                        .py_1()
                        .rounded(px(8.))
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .when(is_selected, |el| el.bg(theme.colors().accent.opacity(0.14)))
                        .hover(|s| s.bg(theme.colors().element_hover))
                        .tooltip(Tooltip::text(day_tip))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_date = if this.selected_date == Some(date) { None } else { Some(date) };
                            cx.notify();
                        }))
                        .child(
                            Label::new(strings.weekdays_short[date.weekday().num_days_from_monday() as usize])
                                .size(LabelSize::XSmall)
                                .color(if is_today || is_selected { Color::Accent } else { Color::Muted }),
                        )
                        .child(
                            Label::new(date.day().to_string())
                                .size(LabelSize::Small)
                                .weight(FontWeight::SEMIBOLD)
                                .color(if is_today || is_selected { Color::Accent } else { Color::Default }),
                        )
                        .child(ring_el)
                }))
        });

        let detail = self
            .selected
            .and_then(|id| habits.iter().find(|h| h.id == id))
            .map(|habit| self.render_detail(habit, today, cx));

        let viewport_width = window.viewport_size().width;
        let clamped_detail = self.detail_width.clamp(MIN_DETAIL_WIDTH, MAX_DETAIL_WIDTH);

        let left_column = v_flex()
            .flex_1()
            .min_w(px(320.))
            .h_full()
            .border_r_1()
            .border_color(theme.colors().border)
            .child(
                h_flex()
                    .mt(layout::header_inset())
                    .h(px(52.))
                    .flex_none()
                    .px(px(22.))
                    .gap_2()
                    .child(
                        h_flex()
                            .id("habit-title")
                            .relative()
                            .gap_1()
                            .cursor_pointer()
                            .child(
                                Label::new(if self.show_archived { strings.habit_archived } else { strings.section_habits })
                                    .size(LabelSize::Large)
                                    .weight(FontWeight::MEDIUM),
                            )
                            .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.header_menu_open = true;
                                cx.notify();
                            }))
                            .when(self.header_menu_open, |el| {
                                let archived = self.show_archived;
                                el.child(PopoverAttachment::new(
                                    Placement::BelowLeft,
                                    popover(cx)
                                        .min_w(px(162.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.header_menu_open = false;
                                            cx.notify();
                                        }))
                                        .children([(false, strings.habit_active), (true, strings.habit_archived)].map(
                                            |(value, label)| {
                                                ListItem::new(if value { "habits-archived" } else { "habits-active" })
                                                    .height(px(34.))
                                                    .child(Label::new(label).color(if value == archived {
                                                        Color::Accent
                                                    } else {
                                                        Color::Default
                                                    }))
                                                    .when(value == archived, |item| {
                                                        item.end_slot(Label::new("✓").color(Color::Accent))
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.show_archived = value;
                                                        this.header_menu_open = false;
                                                        this.selected = None;
                                                        this.grid_scroll.set_offset(gpui::point(px(0.), px(0.)));
                                                        if value {
                                                            this.selected_date = None;
                                                        }
                                                        cx.notify();
                                                    }))
                                            },
                                        )),
                                ))
                            }),
                    )
                    // Dismissible single-date filter chip
                    .when_some(self.selected_date, |el, day| {
                        el.child(
                            h_flex()
                                .id("habit-date-chip")
                                .h(px(24.))
                                .px_2()
                                .gap_1()
                                .rounded_full()
                                .bg(theme.colors().accent.opacity(0.16))
                                .child(Label::new(calendar::short_date(day, strings)).size(LabelSize::XSmall).color(Color::Accent))
                                .child(
                                    div()
                                        .id("habit-date-chip-clear")
                                        .cursor_pointer()
                                        .tooltip(Tooltip::text(strings.clear))
                                        .child(Icon::new(IconName::X).size(IconSize::XSmall).color(Color::Accent))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.selected_date = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        IconButton::new("habit-list", if self.list_layout { IconName::LayoutGrid } else { IconName::List })
                            .on_click(cx.listener(|this, _, _, cx| this.set_list_layout(!this.list_layout, cx))),
                    )
                    .child(
                        IconButton::new("habit-add", IconName::Plus)
                            .tooltip(Tooltip::text(strings.habit_add_title))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_create_modal(HabitSection::Others, window, cx);
                            })),
                    )
                    .child(
                        div()
                            .relative()
                            .child(
                                IconButton::new("habit-more", IconName::Ellipsis)
                                    .tooltip(Tooltip::text(strings.more))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.header_more = Some(HeaderMore::Menu);
                                        cx.notify();
                                    })),
                            )
                            .when_some(self.header_more, |el, more| {
                                el.child(PopoverAttachment::new(Placement::BelowRight, self.render_header_more(more, cx)))
                            }),
                    ),
            )
            .when_some(self.export_message.clone(), |el, message| {
                el.child(div().px(px(22.)).pb_2().child(Label::new(message).size(LabelSize::XSmall).color(Color::Muted)))
            })
            .children(strip)
            .child(
                v_scroll(
                    "habit-grid-scroll",
                    div()
                        .id("habit-grid")
                        .px(px(22.))
                        .pb_6()
                        .when(sections_ui.is_empty(), |el| {
                            let message = if self.show_archived {
                                strings.habit_no_archived
                            } else {
                                strings.habit_empty
                            };
                            let mut empty = EmptyState::new(IconName::FernHabit, message);
                            if !self.show_archived && self.selected_date.is_none() {
                                empty = empty.action(
                                    "habit-empty-add",
                                    strings.habit_add_title,
                                    cx.listener(|this, _, window, cx| {
                                        this.open_create_modal(HabitSection::Others, window, cx);
                                    }),
                                );
                            }
                            el.child(div().w_full().py(px(32.)).child(empty))
                        })
                        .child(v_flex().gap_3().children(sections_ui)),
                )
                .track_scroll(&self.grid_scroll)
                .flex_1()
                .min_h_0(),
            );

        let reset_entity = cx.entity().downgrade();
        let right_panel = div()
            .relative()
            .w(px(clamped_detail))
            .flex_none()
            .h_full()
            .child(match detail {
                Some(detail) => detail,
                None => EmptyState::new(IconName::FernHabit, strings.habit_select).into_any_element(),
            })
            .child(resize::handle(
                resize::Panel::Detail,
                self.resizing_detail,
                move |_, cx| {
                    let _ = reset_entity.update(cx, |this, cx| {
                        this.detail_width = DEFAULT_DETAIL_WIDTH;
                        let _ = this.store.set_setting(DETAIL_WIDTH_SETTING, &DEFAULT_DETAIL_WIDTH.to_string());
                        cx.notify();
                    });
                },
                cx,
            ));

        let edit_modal = self.editing.as_ref().map(|edit| self.render_edit_modal(edit, cx));
        let log_modal = self.log_modal.as_ref().map(|m| self.render_log_modal(m, cx));
        let amount_modal = self.amount_modal.as_ref().map(|m| self.render_amount_modal(m, cx));
        let confirm_modal = self.confirm.as_ref().map(|c| self.render_confirm_modal(c, cx));
        let cycle_done_modal = self.cycle_completed.as_ref().map(|m| self.render_cycle_completed_modal(m, cx));
        let cycles_list_modal = self.cycles_modal.as_ref().map(|m| self.render_completed_cycles_modal(m, cx));
        let card_ctx = self.card_menu.map(|m| self.render_card_context_menu(m, cx));
        let day_ctx = self.day_menu.map(|m| self.render_day_context_menu(m, cx));

        h_flex()
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("Habits")
            .on_action(cx.listener(|this, _: &actions::DismissTransient, _, cx| {
                if !this.dismiss_transient(cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &actions::SelectNextHabit, _, cx| {
                if !this.has_modal_open() && !this.has_popover_open() {
                    this.select_relative(1, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &actions::SelectPreviousHabit, _, cx| {
                if !this.has_modal_open() && !this.has_popover_open() {
                    this.select_relative(-1, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &actions::CheckInSelectedHabit, window, cx| {
                if !this.has_modal_open()
                    && !this.has_popover_open()
                    && let Some(id) = this.selected
                {
                    let day = this.selected_date.unwrap_or_else(|| Local::now().date_naive());
                    this.handle_dot_click(id, day, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &actions::NewHabit, window, cx| {
                if !this.has_modal_open() && this.renaming_section.is_none() {
                    this.open_create_modal(HabitSection::Others, window, cx);
                }
            }))
            .on_drag_move::<resize::DraggedEdge>(cx.listener(move |this, event: &gpui::DragMoveEvent<resize::DraggedEdge>, _, cx| {
                if event.drag(cx).0 == resize::Panel::Detail {
                    let width: f32 = (viewport_width - event.event.position.x).into();
                    this.detail_width = width.clamp(MIN_DETAIL_WIDTH, MAX_DETAIL_WIDTH);
                    this.resizing_detail = true;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|this, dragged: &resize::DraggedEdge, _, cx| {
                if dragged.0 == resize::Panel::Detail {
                    this.resizing_detail = false;
                    let _ = this.store.set_setting(DETAIL_WIDTH_SETTING, &this.detail_width.round().to_string());
                    cx.notify();
                }
            }))
            .items_start()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(left_column)
            .child(right_panel)
            .children(card_ctx)
            .children(day_ctx)
            .children(edit_modal)
            .children(log_modal)
            .children(amount_modal)
            .children(cycles_list_modal)
            .children(cycle_done_modal)
            .children(confirm_modal)
    }
}
