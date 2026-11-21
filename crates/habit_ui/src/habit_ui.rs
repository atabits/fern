//! The Habit section: a 7-day progress strip with single-date filtering, sectioned & reorderable
//! habit cards/rows with check-in modes, and a resizable details panel with stats, cycles,
//! monthly calendar, daily goals chart, and mood-tagged habit logs.

mod detail;
mod dialogs;
mod modal;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use chrono::{Datelike, Duration, Local, NaiveDate};
use fern_core::habit::{
    self, Checkin, CheckinMode, CustomSection, DayProgress, Habit, HabitSection, done_dates, format_amount,
    goal_cycles, stats_with,
};
use fern_core::{Id, Store};
use gpui::{Entity, Focusable as _, FontWeight, Point};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use desktop_common::data_version::{DataVersion, changed_elsewhere, data_changed};
use desktop_common::habit_helpers::progress_ring;
use desktop_common::text_input::{Cancelled, Mode, Submitted, TextInput};
use desktop_common::{actions, calendar, colors as lists_panel, export, layout, resize};
use fern_i18n as i18n;

pub use desktop_common::habit_helpers::{HABIT_EMOJI, IN_TODAY_SETTING, avatar, icon_avatar};

use dialogs::{AmountModal, CompletedCyclesModal, ConfirmAction, CycleCompletedModal, HabitLogModal};
use modal::HabitEdit;

const CARD_WIDTH: f32 = 340.;
const CARD_HEIGHT: f32 = 108.;
const COMPACT_CARD_HEIGHT: f32 = 60.;
const DEFAULT_DETAIL_WIDTH: f32 = 380.;
const MIN_DETAIL_WIDTH: f32 = 300.;
const MAX_DETAIL_WIDTH: f32 = 620.;

const LAYOUT_SETTING: &str = "habit_layout";
const SORT_SETTING: &str = "habits_sort_by_status";
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderMore {
    Menu,
    Settings,
}

#[derive(Clone, Copy)]
struct CardContextMenu {
    habit_id: Id,
    position: Point<Pixels>,
}

#[derive(Clone, Copy)]
struct DayContextMenu {
    habit_id: Id,
    date: NaiveDate,
    position: Point<Pixels>,
}

#[derive(Clone)]
struct DraggedHabit {
    id: Id,
    name: String,
}

impl Render for DraggedHabit {
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
            .child(Label::new(self.name.clone()).size(LabelSize::Small))
    }
}

#[derive(Clone)]
struct DraggedSection {
    id: Id,
    name: String,
}

impl Render for DraggedSection {
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
            .child(Label::new(self.name.clone()).size(LabelSize::Small))
    }
}

pub struct HabitView {
    store: Rc<Store>,
    focus_handle: gpui::FocusHandle,
    habits: Vec<Habit>,
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
}

impl gpui::Focusable for HabitView {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl HabitView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
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
                this.reload(cx);
            }
        })
        .detach();

        let mut this = Self {
            store,
            focus_handle: cx.focus_handle(),
            habits: Vec::new(),
            checkins: HashMap::new(),
            records: HashMap::new(),
            sections: Vec::new(),
            selected: None,
            selected_date: None,
            month: calendar::month_start(Local::now().date_naive()),
            show_archived: false,
            list_layout,
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
        };
        this.reload(cx);
        this
    }

    pub(crate) fn notify_data_changed(&self, cx: &mut Context<Self>) {
        data_changed(cx.entity_id(), cx);
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.habits = self.store.habits().unwrap_or_default();
        self.sections = self.store.habit_sections().unwrap_or_default();
        self.records = self.store.all_habit_checkins().unwrap_or_default();
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
        self.store.setting(key).ok().flatten().map_or(default, |v| v == "1")
    }

    pub(crate) fn set_flag(&mut self, key: &str, on: bool, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_setting(key, if on { "1" } else { "0" }) {
            eprintln!("failed to save setting {key}: {err:#}");
        }
        self.notify_data_changed(cx);
        cx.notify();
    }

    fn set_list_layout(&mut self, list: bool, cx: &mut Context<Self>) {
        self.list_layout = list;
        if let Err(err) = self.store.set_setting(LAYOUT_SETTING, if list { "list" } else { "cards" }) {
            eprintln!("failed to save habit layout: {err:#}");
        }
        cx.notify();
    }

    fn toggle_section_collapsed(&mut self, key: String, cx: &mut Context<Self>) {
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
        if date > Local::now().date_naive() {
            return;
        }
        self.card_menu = None;
        self.day_menu = Some(DayContextMenu { habit_id, date, position });
        cx.notify();
    }

    fn open_card_menu(&mut self, habit_id: Id, position: Point<Pixels>, cx: &mut Context<Self>) {
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

    fn mark_undone(&mut self, habit_id: Id, date: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
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

    fn toggle_skip(&mut self, habit_id: Id, date: NaiveDate, cx: &mut Context<Self>) {
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

    fn export_csv(&mut self, cx: &mut Context<Self>) {
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

    fn start_rename_section(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
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

    fn reorder_habit_drop(&mut self, dragged_id: Id, target_section: HabitSection, before_id: Option<Id>, cx: &mut Context<Self>) {
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

    fn reorder_custom_section_drop(&mut self, dragged_id: Id, target_id: Id, cx: &mut Context<Self>) {
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

    fn visible_habit_ids(&self) -> Vec<Id> {
        let week_start = calendar::week_start();
        self.habits
            .iter()
            .filter(|h| h.archived == self.show_archived)
            .filter(|h| match self.selected_date {
                Some(day) if !self.show_archived => {
                    let recs = self.records.get(&h.id).map(Vec::as_slice).unwrap_or(&[]);
                    h.is_active_on(day, recs, week_start)
                }
                _ => true,
            })
            .map(|h| h.id)
            .collect()
    }

    fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids = self.visible_habit_ids();
        if ids.is_empty() {
            return;
        }
        let next = match self.selected.and_then(|sel| ids.iter().position(|id| *id == sel)) {
            Some(idx) => {
                let target = (idx as isize + delta).clamp(0, ids.len() as isize - 1) as usize;
                ids[target]
            }
            None => ids[0],
        };
        self.select(next, cx);
    }

    pub(crate) fn menu_entry(&self, id: &'static str, item: ListItem, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(id)
            .on_hover(cx.listener(|this, hovered, _, cx| {
                if *hovered && this.detail_submenu.take().is_some() {
                    cx.notify();
                }
            }))
            .child(item)
    }

    /// Renders a single day's check-in dot/ring (Done, Undone, Partial ring, Skipped, Rest, Unchecked).
    pub(crate) fn checkin_mark(&self, habit: &Habit, date: NaiveDate, size: Pixels, cx: &App) -> Div {
        let theme = cx.theme();
        let status_colors = theme.status();
        let accent = lists_panel::parse_color(habit.color.as_deref()).unwrap_or(theme.colors().accent);
        let rec = self.records.get(&habit.id).and_then(|r| r.iter().find(|c| c.date == date));
        let done_set = self.checkins.get(&habit.id);
        let empty = HashSet::new();
        let is_skipped = habit.is_skipped(date);
        let is_due = habit.is_due_on(date, done_set.unwrap_or(&empty), calendar::week_start());

        let base = div().size(size).rounded_full().flex().items_center().justify_center();

        if let Some(c) = rec {
            if c.is_done() {
                if habit.icon_checkins {
                    let glyph = habit
                        .icon
                        .clone()
                        .unwrap_or_else(|| habit.name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default());
                    return base
                        .bg(accent.opacity(0.24))
                        .border_1()
                        .border_color(accent)
                        .text_size(size * 0.55)
                        .child(glyph);
                }
                return base
                    .bg(accent)
                    .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Custom(gpui::white())));
            }
            if c.is_undone() {
                return base
                    .bg(status_colors.error.opacity(0.16))
                    .border_1()
                    .border_color(status_colors.error.opacity(0.65))
                    .child(Icon::new(IconName::X).size(IconSize::XSmall).color(Color::Error));
            }
            if c.is_partial() {
                return base.relative().child(progress_ring(
                    size,
                    c.progress(),
                    accent,
                    theme.colors().border.opacity(0.55),
                    px(2.5),
                ));
            }
        }

        if is_skipped {
            return base
                .border_1()
                .border_color(theme.colors().border)
                .bg(theme.colors().surface_background.opacity(0.6))
                .child(Icon::new(IconName::Minus).size(IconSize::XSmall).color(Color::Muted));
        }

        if !is_due {
            return base
                .border_1()
                .border_color(theme.colors().border.opacity(0.35))
                .bg(theme.colors().border.opacity(0.12));
        }

        base.border_2().border_color(theme.colors().text_muted.opacity(0.55))
    }

    fn render_header_more(&self, more: HeaderMore, cx: &mut Context<Self>) -> Div {
        let strings = i18n::t(cx);
        let in_today = self.flag(IN_TODAY_SETTING, true);
        let sort_status = self.flag(SORT_SETTING, false);
        match more {
            HeaderMore::Menu => popover(cx)
                .min_w(px(176.))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.header_more = None;
                    cx.notify();
                }))
                .child(
                    ListItem::new("habit-open-settings")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::Settings).color(Color::Muted))
                        .child(Label::new(strings.habit_settings))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.header_more = Some(HeaderMore::Settings);
                            cx.notify();
                        })),
                )
                .child(
                    ListItem::new("habit-export-csv")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::Share).color(Color::Muted))
                        .child(Label::new(strings.export))
                        .on_click(cx.listener(|this, _, _, cx| this.export_csv(cx))),
                ),
            HeaderMore::Settings => popover(cx)
                .w(px(270.))
                .p_3()
                .gap_3()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.header_more = None;
                    cx.notify();
                }))
                .child(
                    h_flex()
                        .id("habit-set-in-today")
                        .justify_between()
                        .items_center()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.set_flag(IN_TODAY_SETTING, !in_today, cx)))
                        .child(Label::new(strings.habits_in_today).size(LabelSize::Small))
                        .child(Checkbox::new("habit-in-today-box").toggle_state(in_today)),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .id("habit-set-sort")
                                .justify_between()
                                .items_center()
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_flag(SORT_SETTING, !sort_status, cx);
                                }))
                                .child(Label::new(strings.habits_sort_by_status).size(LabelSize::Small))
                                .child(Checkbox::new("habit-sort-box").toggle_state(sort_status)),
                        )
                        .child(Label::new(strings.habits_sort_hint).size(LabelSize::XSmall).color(Color::Muted)),
                ),
        }
    }

    fn render_section_header(
        &self,
        section: HabitSection,
        title: String,
        count: usize,
        allow_add: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let key = section.key();
        let collapsed = self.collapsed_sections.contains(&key);
        let custom_id = match section {
            HabitSection::Custom(id) => Some(id),
            _ => None,
        };
        let is_renaming = custom_id.is_some() && self.renaming_section.as_ref().is_some_and(|(id, _)| Some(*id) == custom_id);

        let key_toggle = key.clone();
        let title_for_drag = title.clone();
        h_flex()
            .id(SharedString::from(format!("habit-sec-hdr-{key}")))
            .h(px(30.))
            .gap_1p5()
            .items_center()
            .rounded(px(6.))
            .when_some(custom_id, |el, cid| {
                el.on_drag(DraggedSection { id: cid, name: title_for_drag.clone() }, |dragged, _, _, cx| {
                    cx.new(|_| dragged.clone())
                })
                .drag_over::<DraggedSection>(|s, _, _, cx| s.border_t_2().border_color(cx.theme().colors().accent))
                .on_drop(cx.listener(move |this, dragged: &DraggedSection, _, cx| {
                    this.reorder_custom_section_drop(dragged.id, cid, cx);
                }))
            })
            .drag_over::<DraggedHabit>(|s, _, _, cx| s.bg(cx.theme().colors().accent.opacity(0.12)))
            .on_drop(cx.listener(move |this, dragged: &DraggedHabit, _, cx| {
                this.reorder_habit_drop(dragged.id, section, None, cx);
            }))
            .child(
                h_flex()
                    .id(SharedString::from(format!("habit-sec-toggle-{key}")))
                    .gap_1()
                    .items_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_section_collapsed(key_toggle.clone(), cx);
                    }))
                    .child(
                        Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(if is_renaming {
                        match &self.renaming_section {
                            Some((_, input)) => h_flex()
                                .w(px(160.))
                                .h(px(24.))
                                .px_1p5()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(theme.colors().accent)
                                .bg(theme.colors().background)
                                .child(input.clone())
                                .into_any_element(),
                            None => div().into_any_element(),
                        }
                    } else {
                        Label::new(title).size(LabelSize::Small).weight(FontWeight::MEDIUM).color(Color::Muted).into_any_element()
                    })
                    .child(Label::new(count.to_string()).size(LabelSize::XSmall).color(Color::Muted)),
            )
            .child(div().flex_1())
            .when(allow_add && !self.show_archived, |el| {
                el.child(
                    IconButton::new(SharedString::from(format!("sec-add-{key}")), IconName::Plus)
                        .icon_size(IconSize::XSmall)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_create_modal(section, window, cx);
                        })),
                )
            })
            .when_some(custom_id, |el, cid| {
                el.child(
                    div()
                        .relative()
                        .child(
                            IconButton::new(SharedString::from(format!("sec-more-{cid}")), IconName::Ellipsis)
                                .icon_size(IconSize::XSmall)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.section_menu = Some(cid);
                                    cx.notify();
                                })),
                        )
                        .when(self.section_menu == Some(cid), |el| {
                            el.child(PopoverAttachment::new(
                                Placement::BelowRight,
                                popover(cx)
                                    .min_w(px(140.))
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        this.section_menu = None;
                                        cx.notify();
                                    }))
                                    .child(
                                        ListItem::new("sec-rename")
                                            .height(px(32.))
                                            .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                                            .child(Label::new(strings.rename_section))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.start_rename_section(cid, window, cx);
                                            })),
                                    )
                                    .child(
                                        ListItem::new("sec-delete")
                                            .height(px(32.))
                                            .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                                            .child(Label::new(strings.delete).color(Color::Error))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.section_menu = None;
                                                this.confirm = Some(ConfirmAction::DeleteSection { section_id: cid });
                                                cx.notify();
                                            })),
                                    ),
                            ))
                        }),
                )
            })
            .into_any_element()
    }

    fn render_card(
        &self,
        habit: &Habit,
        week: [NaiveDate; 7],
        today: NaiveDate,
        allow_dnd: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = habit.id;
        let empty = HashSet::new();
        let checked = self.checkins.get(&id).unwrap_or(&empty);
        let stats = stats_with(&habit.frequency, checked, today, calendar::week_start());
        let compact = self.selected_date.is_some() || habit.archived;
        let list = self.list_layout;
        let target_single_day = self.selected_date.unwrap_or(today);
        let single_day_rec = self
            .records
            .get(&id)
            .and_then(|r| r.iter().find(|c| c.date == target_single_day));
        let single_day_handled = single_day_rec.is_some_and(|c| c.is_handled());

        let subtitle_el: AnyElement = if let Some(date) = self.selected_date
            && let Some(goal) = habit.goal.as_ref()
        {
            let current = self
                .records
                .get(&id)
                .and_then(|r| r.iter().find(|c| c.date == date))
                .map_or(0.0, |c| c.value);
            Label::new(format!("{}/{} {}", format_amount(current), format_amount(goal.amount), goal.unit))
                .size(LabelSize::XSmall)
                .color(Color::Muted)
                .truncate()
                .into_any_element()
        } else {
            let total_text = if self.selected_date.is_none() && !habit.archived {
                match habit.goal_days {
                    Some(goal) => format!("{}/{goal} {}", stats.total_days, strings.habit_days),
                    None => format!("{} {}", stats.total_days, strings.habit_days),
                }
            } else {
                format!("{} {}", stats.total_days, strings.habit_days)
            };
            let (streak_count, streak_label) = if habit.archived && self.selected_date.is_none() {
                (stats.best_streak, strings.habit_best_streak)
            } else {
                (stats.streak, strings.habit_streak)
            };
            h_flex()
                .gap_2()
                .items_center()
                .overflow_hidden()
                .child(
                    h_flex()
                        .gap_0p5()
                        .items_center()
                        .child(
                            Icon::new(IconName::Zap)
                                .size(IconSize::XSmall)
                                .color(if stats.total_days > 0 { Color::Accent } else { Color::Muted }),
                        )
                        .child(Label::new(total_text).size(LabelSize::XSmall).color(Color::Muted)),
                )
                .child(
                    h_flex()
                        .gap_0p5()
                        .items_center()
                        .child(
                            Icon::new(IconName::Flame)
                                .size(IconSize::XSmall)
                                .color(if streak_count > 0 { Color::Warning } else { Color::Muted }),
                        )
                        .child(Label::new(format!("{streak_count} {streak_label}")).size(LabelSize::XSmall).color(Color::Muted)),
                )
                .into_any_element()
        };

        let habit_section = habit.section;
        let habit_name = habit.name.clone();

        let card_el = div()
            .id(SharedString::from(format!("habit-card-{id}")))
            .flex()
            .map(|el| {
                if list || compact {
                    el.flex_row()
                        .when(list, |el| el.w_full())
                        .when(!list, |el| el.w(px(CARD_WIDTH)))
                        .h(px(COMPACT_CARD_HEIGHT))
                        .px_3()
                        .gap_3()
                        .items_center()
                } else {
                    el.flex_col()
                        .w(px(CARD_WIDTH))
                        .h(px(CARD_HEIGHT))
                        .p_3()
                        .justify_between()
                }
            });

        let header_info = h_flex()
            .gap_2()
            .flex_1()
            .min_w_0()
            .items_center()
            .child(avatar(habit, px(30.), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        Label::new(habit.name.clone())
                            .weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .when(self.selected_date.is_some() && single_day_handled, |l| l.color(Color::Muted)),
                    )
                    .child(subtitle_el),
            );

        let dots_el: AnyElement = if let Some(single_day) = self.selected_date {
            self.checkin_mark(habit, single_day, px(24.), cx)
                .id(SharedString::from(format!("checkin-{id}-{single_day}")))
                .when(!habit.archived && single_day <= today, |el| {
                    el.cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.handle_dot_click(id, single_day, window, cx);
                        }))
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.open_day_menu(id, single_day, ev.position, cx);
                            }),
                        )
                })
                .into_any_element()
        } else if habit.archived {
            div().into_any_element()
        } else {
            h_flex()
                .justify_between()
                .px_1()
                .when(list, |el| el.w(px(210.)).flex_none())
                .children(week.map(|date| {
                    let blocked = habit.archived || date > today;
                    self.checkin_mark(habit, date, px(if list { 22. } else { 18. }), cx)
                        .id(SharedString::from(format!("checkin-{id}-{date}")))
                        .when(date > today, |el| el.opacity(0.4))
                        .when(!blocked, |el| {
                            el.cursor_pointer()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.handle_dot_click(id, date, window, cx);
                                }))
                                .on_mouse_down(
                                    gpui::MouseButton::Right,
                                    cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.open_day_menu(id, date, ev.position, cx);
                                    }),
                                )
                        })
                }))
                .into_any_element()
        };

        card_el
            .flex_none()
            .rounded(px(8.))
            .border_1()
            .border_color(if self.selected == Some(id) {
                theme.colors().accent
            } else {
                gpui::transparent_black()
            })
            .bg(theme.colors().surface_background)
            .hover(|s| s.bg(theme.colors().element_hover))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_card_menu(id, ev.position, cx);
                }),
            )
            .when(allow_dnd && !habit.archived, |el| {
                el.on_drag(DraggedHabit { id, name: habit_name.clone() }, |dragged, _, _, cx| {
                    cx.new(|_| dragged.clone())
                })
                .drag_over::<DraggedHabit>(|s, _, _, cx| s.border_color(cx.theme().colors().accent))
                .on_drop(cx.listener(move |this, dragged: &DraggedHabit, _, cx| {
                    if dragged.id != id {
                        this.reorder_habit_drop(dragged.id, habit_section, Some(id), cx);
                    }
                }))
            })
            .child(header_info)
            .child(dots_el)
            .into_any_element()
    }

    fn render_card_context_menu(&self, menu: CardContextMenu, cx: &mut Context<Self>) -> AnyElement {
        let Some(habit) = self.habits.iter().find(|h| h.id == menu.habit_id).cloned() else {
            return div().into_any_element();
        };
        let strings = i18n::t(cx);
        let id = habit.id;
        let name = habit.name.clone();
        let archived = habit.archived;

        div()
            .id("habit-card-ctx")
            .absolute()
            .left(menu.position.x)
            .top(menu.position.y)
            .occlude()
            .child(
                popover(cx)
                    .min_w(px(176.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.card_menu = None;
                        cx.notify();
                    }))
                    .child(
                        ListItem::new("ctx-habit-edit")
                            .height(px(32.))
                            .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                            .child(Label::new(strings.edit_habit))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_edit_modal(id, window, cx))),
                    )
                    .when(!archived, |el| {
                        let name_pomo = name.clone();
                        let name_sw = name.clone();
                        el.child(
                            ListItem::new("ctx-habit-pomo")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::Timer).color(Color::Muted))
                                .child(Label::new(strings.habit_start_pomo))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.card_menu = None;
                                    cx.emit(StartFocus {
                                        habit_id: Some(id),
                                        title: name_pomo.clone(),
                                        stopwatch: false,
                                    });
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("ctx-habit-sw")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::Clock).color(Color::Muted))
                                .child(Label::new(strings.habit_start_stopwatch))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.card_menu = None;
                                    cx.emit(StartFocus {
                                        habit_id: Some(id),
                                        title: name_sw.clone(),
                                        stopwatch: true,
                                    });
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        ListItem::new("ctx-habit-archive")
                            .height(px(32.))
                            .start_slot(
                                Icon::new(if archived { IconName::ArchiveRestore } else { IconName::Archive })
                                    .color(Color::Muted),
                            )
                            .child(Label::new(if archived { strings.habit_unarchive } else { strings.habit_archive }))
                            .on_click(cx.listener(move |this, _, _, cx| this.set_archived(id, !archived, cx))),
                    )
                    .child(
                        ListItem::new("ctx-habit-delete")
                            .height(px(32.))
                            .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                            .child(Label::new(strings.delete).color(Color::Error))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.card_menu = None;
                                this.confirm = Some(ConfirmAction::DeleteHabit { habit_id: id });
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_day_context_menu(&self, menu: DayContextMenu, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let id = menu.habit_id;
        let date = menu.date;
        let skipped = self.habits.iter().find(|h| h.id == id).is_some_and(|h| h.is_skipped(date));
        let rec = self.records.get(&id).and_then(|r| r.iter().find(|c| c.date == date));
        let is_handled_or_partial = rec.is_some_and(|c| c.is_handled() || c.is_partial());

        div()
            .id("habit-day-ctx")
            .absolute()
            .left(menu.position.x)
            .top(menu.position.y)
            .occlude()
            .child(
                popover(cx)
                    .min_w(px(164.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.day_menu = None;
                        cx.notify();
                    }))
                    .child(
                        ListItem::new("day-ctx-log")
                            .height(px(32.))
                            .start_slot(Icon::new(IconName::MessageSquareText).color(Color::Muted))
                            .child(Label::new(strings.habit_log_on.trim_end_matches([' ', ':', '-', '—'])))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_log_modal(id, date, false, window, cx);
                            })),
                    )
                    .child(
                        ListItem::new("day-ctx-skip")
                            .height(px(32.))
                            .start_slot(Icon::new(IconName::Minus).color(Color::Muted))
                            .child(Label::new(if skipped { strings.habit_unskip } else { strings.habit_skip }))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_skip(id, date, cx))),
                    )
                    .child(
                        ListItem::new("day-ctx-undone")
                            .height(px(32.))
                            .start_slot(Icon::new(IconName::Ban).color(Color::Muted))
                            .child(Label::new(strings.habit_unachieved))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.mark_undone(id, date, window, cx);
                            })),
                    )
                    .when(is_handled_or_partial || skipped, |el| {
                        el.child(
                            ListItem::new("day-ctx-reset")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::RotateCcw).color(Color::Muted))
                                .child(Label::new(strings.habit_reset))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.day_menu = None;
                                    if let Err(err) = this.store.reset_checkin(id, date, true) {
                                        eprintln!("failed to reset check-in: {err:#}");
                                    }
                                    this.notify_data_changed(cx);
                                    this.reload(cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    #[cfg(debug_assertions)]
    pub fn dev_apply(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        use fern_core::{Goal, HabitFrequency};
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
        let week_start = calendar::week_start();
        let week: [NaiveDate; 7] = std::array::from_fn(|i| today - Duration::days(6 - i as i64));

        let habits = self.habits.clone();
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
        let is_handled_on_selected = |h: &Habit| -> bool {
            let Some(day) = self.selected_date else { return false };
            self.records
                .get(&h.id)
                .and_then(|recs| recs.iter().find(|r| r.date == day))
                .is_some_and(|r| r.is_handled())
        };

        let (main_visible, completed_visible): (Vec<&Habit>, Vec<&Habit>) = if split_completed {
            visible.into_iter().partition(|h| !is_handled_on_selected(h))
        } else {
            (visible, Vec::new())
        };

        let grouped = main_visible.iter().any(|h| h.section != HabitSection::Others) || !self.sections.is_empty();
        let mut sections_ui: Vec<AnyElement> = Vec::new();
        for (i, section) in HabitSection::ordered(&self.sections).into_iter().enumerate() {
            let mut in_section: Vec<&Habit> = main_visible
                .iter()
                .copied()
                .filter(|h| !grouped || h.section == section)
                .collect();
            if sort_by_status && !split_completed {
                in_section.sort_by_key(|h| {
                    let day = self.selected_date.unwrap_or(today);
                    self.records
                        .get(&h.id)
                        .and_then(|recs| recs.iter().find(|r| r.date == day))
                        .is_some_and(|r| r.is_handled())
                });
            }
            if in_section.is_empty() || (!grouped && i > 0) {
                continue;
            }
            let title = self.section_label(section, cx);
            let collapsed = grouped && self.collapsed_sections.contains(&section.key());
            let section_cards: Vec<AnyElement> = if collapsed {
                Vec::new()
            } else {
                in_section
                    .iter()
                    .map(|h| self.render_card(h, week, today, true, cx))
                    .collect()
            };
            sections_ui.push(
                v_flex()
                    .w_full()
                    .gap_2()
                    .when(grouped, |el| el.child(self.render_section_header(section, title, in_section.len(), true, cx)))
                    .when(!collapsed, |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(if self.list_layout { 10. } else { 12. }))
                                .children(section_cards),
                        )
                    })
                    .into_any_element(),
            );
        }

        // Bottom "Completed" section when single-date filter + Sort by Check-in Status are active.
        if !completed_visible.is_empty() {
            let completed_key = "completed".to_owned();
            let collapsed = self.collapsed_sections.contains(&completed_key);
            let completed_cards: Vec<AnyElement> = if collapsed {
                Vec::new()
            } else {
                completed_visible
                    .iter()
                    .map(|h| self.render_card(h, week, today, false, cx))
                    .collect()
            };
            sections_ui.push(
                v_flex()
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
                                Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(
                                Label::new(strings.habit_completed_section)
                                    .size(LabelSize::Small)
                                    .weight(FontWeight::MEDIUM)
                                    .color(Color::Muted),
                            )
                            .child(Label::new(completed_visible.len().to_string()).size(LabelSize::XSmall).color(Color::Muted)),
                    )
                    .when(!collapsed, |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(if self.list_layout { 10. } else { 12. }))
                                .children(completed_cards),
                        )
                    })
                    .into_any_element(),
            );
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
                            .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Custom(gpui::white())))
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
                                .child(Label::new(day.format("%b %d").to_string()).size(LabelSize::XSmall).color(Color::Accent))
                                .child(
                                    div()
                                        .id("habit-date-chip-clear")
                                        .cursor_pointer()
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_create_modal(HabitSection::Others, window, cx);
                            })),
                    )
                    .child(
                        div()
                            .relative()
                            .child(IconButton::new("habit-more", IconName::Ellipsis).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.header_more = Some(HeaderMore::Menu);
                                    cx.notify();
                                },
                            )))
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
                            el.child(div().pt_8().child(
                                Label::new(if self.show_archived {
                                    strings.habit_no_archived
                                } else {
                                    strings.habit_empty
                                })
                                .color(Color::Muted),
                            ))
                        })
                        .child(v_flex().gap_3().children(sections_ui)),
                )
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
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Label::new(strings.habit_select).color(Color::Muted))
                    .into_any_element(),
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

        let edit_modal = self.editing.as_ref().map(|edit| {
            dialogs::modal_backdrop("habit-modal", self.render_edit_modal(edit, cx))
        });
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
            .on_action(cx.listener(|this, _: &actions::SelectNextHabit, _, cx| this.select_relative(1, cx)))
            .on_action(cx.listener(|this, _: &actions::SelectPreviousHabit, _, cx| this.select_relative(-1, cx)))
            .on_action(cx.listener(|this, _: &actions::CheckInSelectedHabit, window, cx| {
                if let Some(id) = this.selected {
                    let day = this.selected_date.unwrap_or_else(|| Local::now().date_naive());
                    this.handle_dot_click(id, day, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &actions::NewHabit, window, cx| {
                this.open_create_modal(HabitSection::Others, window, cx);
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

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use fern_core::habit::{Goal, HabitDraft};
    use gpui::TestAppContext;

    #[gpui::test]
    fn restoring_the_last_archived_habit_shows_its_weekly_progress_only_in_active(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = Local::now().date_naive();
        let habit = store.add_habit("Read").unwrap();
        assert!(store.toggle_checkin(habit.id, today).unwrap());
        store.set_habit_archived(habit.id, true).unwrap();

        cx.update(theme::init);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = HabitView::new(store.clone(), cx);
            view.show_archived = true;
            view.selected = Some(habit.id);
            view
        });
        assert!(cx.debug_bounds("habit-week-progress").is_none());

        // Archived habits are strictly read-only: clicking a dot does nothing.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.handle_dot_click(habit.id, today, window, cx));
        });
        assert!(store.checkins(habit.id).unwrap().contains(&today));

        cx.update(|_, cx| {
            view.update(cx, |view, cx| view.set_archived(habit.id, false, cx));
        });
        assert!(cx.debug_bounds("habit-week-progress").is_none());
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(view.show_archived);
            assert!(view.habits.iter().all(|habit| !habit.archived));
            assert!(view.selected.is_none());
        });

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.show_archived = false;
                cx.notify();
            });
        });
        assert!(cx.debug_bounds("habit-week-progress").is_some());
        cx.update(|_, cx| assert_eq!(view.read(cx).day_progress(today), DayProgress::Fraction(1.0)));
    }

    #[gpui::test]
    fn edit_modal_preserves_unset_start_date_and_navigates_calendar(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let habit = store.add_habit("Stretch").unwrap();
        assert_eq!(habit.start_date, None);

        cx.update(theme::init);
        let (view, cx) = cx.add_window_view(|_, cx| HabitView::new(store.clone(), cx));

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.open_edit_modal(habit.id, window, cx);
                assert_eq!(view.editing.as_ref().unwrap().start_date, None);
                view.request_save_edit(cx);
            });
        });
        // Saving without picking a start date must NOT silently overwrite start_date with Some(today).
        assert_eq!(store.habits().unwrap()[0].start_date, None);

        // Opening StartDate row initializes start_calendar so month shifting works.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.open_edit_modal(habit.id, window, cx);
                let today = Local::now().date_naive();
                view.update_edit(cx, |e| {
                    e.open = Some(modal::EditRow::StartDate);
                    e.start_calendar = Some(calendar::month_start(today));
                });
                let initial = view.editing.as_ref().unwrap().start_calendar.unwrap();
                view.update_edit(cx, |e| {
                    e.start_calendar = Some(calendar::add_months(initial, -1));
                });
                assert_ne!(view.editing.as_ref().unwrap().start_calendar.unwrap(), initial);
                assert!(view.dismiss_transient(cx));
            });
        });
    }

    #[gpui::test]
    fn checkin_modes_and_reset_confirmation(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = Local::now().date_naive();
        let water = store
            .create_habit(&HabitDraft {
                name: "Water".into(),
                goal: Some(Goal { amount: 2.0, unit: "L".into(), mode: CheckinMode::Auto { step: 1.0 } }),
                ..Default::default()
            })
            .unwrap();

        cx.update(theme::init);
        let (view, cx) = cx.add_window_view(|_, cx| HabitView::new(store.clone(), cx));

        // Step 1 -> 1.0 (partial), Step 2 -> 2.0 (Done), Step 3 -> opens Reset confirm modal!
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_dot_click(water.id, today, window, cx);
                assert!(!view.checkins.get(&water.id).unwrap().contains(&today));
                view.handle_dot_click(water.id, today, window, cx);
                assert!(view.checkins.get(&water.id).unwrap().contains(&today));
                view.handle_dot_click(water.id, today, window, cx);
                assert!(matches!(view.confirm, Some(ConfirmAction::ResetCheckin { .. })));
                view.execute_confirm(cx);
                assert!(!view.checkins.get(&water.id).unwrap().contains(&today));
            });
        });
    }
}
