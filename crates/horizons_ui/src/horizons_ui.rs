//! TimeStripe-inspired panoramic Horizons planning view embedded inside Fern's Tasks section.
//!
//! Displays tasks across Inbox, Today, ISO Week, Month, Quarter, Year, Decade, and Life,
//! plus zoomed Time Scale strips (`Days`..`Decades`), drag-and-drop scheduling, smart rollover
//! for past incomplete items, slide-over `TaskDetail`, and filters.

mod column;
mod drawer;
mod mutations;
mod prompts;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;
use std::rc::Rc;

use chrono::{Datelike, Duration, Local, NaiveDate};
use desktop_common::actions::{DismissTransient, Redo, Undo};
use desktop_common::calendar;
use desktop_common::colors;
use desktop_common::data_version::{DataVersion, changed_elsewhere, data_changed};
use desktop_common::layout;
use desktop_common::text_input::TextInput;
use desktop_common::undo::UndoAction;
use fern_core::horizon::{
    horizons_bucket_partition, horizons_day_partition, horizons_week_dated_tasks,
    is_horizons_inbox,
};
use fern_core::{Horizon, Id, List, Priority, Store, Tag, Task};
use fern_i18n::{self as i18n, CurrentLocale, Strings};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla,
    KeyDownEvent, MouseButton, Render, ScrollHandle, SharedString, Subscription, Window, div, px,
};
use task_ui::task_detail::{OpenTask, StartFocus, TaskDetail, TaskTextEdited, TaskUpdated, Undoable};
use ui::prelude::*;
use ui::{
    HorizontalScrollbar, IconButton, IconName, IconSize, Label, LabelSize, Placement,
    PopoverAttachment, Tooltip,
};

use crate::column::{
    ColumnSpec, ColumnTarget, HorizonSlot, PANORAMIC_MIN_WIDTH, STRIP_MIN_WIDTH, capitalize_first,
    format_day_subtitle,
};
use crate::drawer::ActivePopover;
pub(crate) use crate::mutations::UndoEntry;

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

/// Root GPUI view for Fern's Horizons planning screen inside `Tasks`.
pub struct HorizonsView {
    pub(crate) store: Rc<Store>,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) mode: HorizonsMode,
    pub(crate) period_offset: i32,

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
    pub(crate) h_scroll: ScrollHandle,
    pub(crate) undo_stack: Vec<UndoEntry>,
    pub(crate) redo_stack: Vec<UndoEntry>,
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
            cx.subscribe_in(&detail, window, |this, _, ev: &Undoable, window, cx| {
                this.push_undo(UndoEntry::Single(ev.0.clone()));
                if let UndoAction::Restore(deleted_id) = ev.0
                    && this.detail_task == Some(deleted_id)
                {
                    this.detail_task = None;
                    this.detail.update(cx, |d, cx| d.show(None, cx));
                    window.focus(&this.focus_handle, cx);
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
        let had_inline_add = self.inline_add.take().is_some();
        self.detail_task = Some(id);
        let lists_for_detail = self.lists.clone();
        self.detail.update(cx, |d, cx| {
            d.set_lists(lists_for_detail);
            d.show(Some(task), cx);
        });
        if had_inline_add || !self.focus_handle.contains_focused(window, cx) {
            window.focus(&self.focus_handle, cx);
        }
        cx.notify();
    }

    pub fn dismiss_transient(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.active_popover.take().is_some() {
            if self.detail_task.is_none() && !self.focus_handle.contains_focused(window, cx) {
                window.focus(&self.focus_handle, cx);
            }
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

    pub(crate) fn today() -> NaiveDate {
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

    pub(crate) fn persist_collapsed_cols(&self) {
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

    fn render_popover_button(
        &self,
        id: &'static str,
        icon: IconName,
        kind: ActivePopover,
        badge_dot: bool,
        tooltip: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.active_popover == Some(kind);
        let popover_el = if selected {
            Some(match kind {
                ActivePopover::Filter => self.render_filter_popover(cx),
                ActivePopover::Options => self.render_options_popover(cx),
            })
        } else {
            None
        };
        let accent = cx.theme().colors().accent;

        div()
            .relative()
            .child(
                div()
                    .relative()
                    .debug_selector(move || id.to_owned())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        IconButton::new(id, icon)
                            .size(px(26.))
                            .icon_size(IconSize::Small)
                            .toggle_state(selected)
                            .tooltip(Tooltip::text(tooltip))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.active_popover = if this.active_popover == Some(kind) {
                                    None
                                } else {
                                    Some(kind)
                                };
                                cx.notify();
                            })),
                    )
                    .when(badge_dot, |el| {
                        el.child(
                            div()
                                .absolute()
                                .top(px(3.))
                                .right(px(3.))
                                .size(px(5.5))
                                .rounded_full()
                                .bg(accent),
                        )
                    }),
            )
            .when_some(popover_el, |el, content| {
                el.child(PopoverAttachment::new(Placement::BelowRight, content))
            })
            .into_any_element()
    }

    /// Razor-clean top strip bar: scale tabs on the left, optional period stepper in the middle,
    /// and Filter + View Options buttons on the right (below `title_bar_height()`).
    fn render_top_strip(&self, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let strings = i18n::t(cx);
        let filter_active = self.has_active_filters() || self.show_completed;

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
                                .debug_selector(|| "horizons-pager-prev".to_owned())
                                .child(
                                    IconButton::new("horizons-pager-prev", IconName::ChevronLeft)
                                        .size(px(24.))
                                        .icon_size(IconSize::XSmall)
                                        .tooltip(Tooltip::text("←"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.step_period(-1, cx);
                                        })),
                                ),
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
                                .when(!at_current, |el| {
                                    el.cursor_pointer().hover(|s| s.bg(colors.ghost_element_hover))
                                })
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
                                .debug_selector(|| "horizons-pager-next".to_owned())
                                .child(
                                    IconButton::new("horizons-pager-next", IconName::ChevronRight)
                                        .size(px(24.))
                                        .icon_size(IconSize::XSmall)
                                        .tooltip(Tooltip::text("→"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.step_period(1, cx);
                                        })),
                                ),
                        ),
                )
        });

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
                    .child(self.render_popover_button(
                        "horizons-btn-filter",
                        IconName::Funnel,
                        ActivePopover::Filter,
                        filter_active,
                        strings.horizons_filter_title,
                        cx,
                    ))
                    .child(self.render_popover_button(
                        "horizons-btn-options",
                        IconName::Settings2,
                        ActivePopover::Options,
                        false,
                        strings.horizons_columns,
                        cx,
                    )),
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

    fn render_horizon_strip(
        &self,
        base: Horizon,
        offsets: RangeInclusive<i32>,
        today: NaiveDate,
        strings: &Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let horizons: Vec<Horizon> = offsets
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
                self.render_horizon_strip(Horizon::current_week(today), -1..=4, today, strings, cx)
            }
            HorizonsMode::Months => {
                self.render_horizon_strip(Horizon::current_month(today), 0..=5, today, strings, cx)
            }
            HorizonsMode::Quarters => {
                self.render_horizon_strip(Horizon::current_quarter(today), 0..=3, today, strings, cx)
            }
            HorizonsMode::Years => {
                self.render_horizon_strip(Horizon::current_year(today), 0..=4, today, strings, cx)
            }
            HorizonsMode::Decades => {
                self.render_horizon_strip(Horizon::current_decade(today), 0..=3, today, strings, cx)
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
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.perform_undo(cx);
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.perform_redo(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let editor_focused = this
                    .inline_add
                    .as_ref()
                    .is_some_and(|(_, input, _)| input.focus_handle(cx).is_focused(window))
                    || this.detail.read(cx).has_popup()
                    || window
                        .focused(cx)
                        .is_some_and(|focus| this.detail.read(cx).is_editor_focus(&focus, cx));
                if !editor_focused
                    && event.keystroke.modifiers.secondary()
                    && !event.keystroke.modifiers.alt
                {
                    let key = event.keystroke.key.as_str();
                    if key.eq_ignore_ascii_case("z") && !event.keystroke.modifiers.shift {
                        this.perform_undo(cx);
                        cx.stop_propagation();
                    } else if (key.eq_ignore_ascii_case("z") && event.keystroke.modifiers.shift)
                        || key.eq_ignore_ascii_case("y")
                    {
                        this.perform_redo(cx);
                        cx.stop_propagation();
                    }
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                let popover_closed = this.active_popover.take().is_some();
                let inline_closed = this.inline_add.take().is_some();
                window.focus(&this.focus_handle, cx);
                if popover_closed || inline_closed {
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
