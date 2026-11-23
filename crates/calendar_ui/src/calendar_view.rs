//! The calendar: tasks placed on their due dates in a month grid or a week with a time axis,
//! matched to TickTick's Calendar section.

use std::{collections::{BTreeMap, HashSet}, rc::Rc, time::Duration};

use chrono::{Datelike, Local, NaiveDate, NaiveTime};
use fern_core::{CalendarOptions, ColorBy, Id, Store, Task};
use gpui::{EventEmitter, Focusable, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment};

use crate::actions::{AgendaView, CalendarToday, DayView, DismissTransient, MonthView, MultiDayMode, NewTask, NextPeriod, NextYear, PreviousPeriod, PreviousYear, PrintYear, WeekView, YearView};
use crate::calendar::{self, week_axis};
use crate::drag::{DraggedEvent, DraggedTask, ResizeEdge, ResizedTask};
use crate::i18n;
use crate::calendar_options_view::CalendarOptionsDialog;
#[cfg(debug_assertions)]
use crate::calendar_options_view::Page;
use crate::task_detail::{StartFocus, TaskDetail, TaskTextEdited, TaskTextField, TaskUpdated};
use crate::task_rows;

#[path = "calendar_task_editor.rs"]
mod calendar_task_editor;
use calendar_task_editor::{CalendarTaskEditor, EditorEvent};

#[path = "calendar_year.rs"]
mod calendar_year;

mod agenda;
mod arrange;
mod event_editor;
mod menus;
mod month_grid;
mod sidebar;
mod week_grid;

use event_editor::EventEditor;
use menus::{ContextMenuState, PendingDeleteEvent};
use month_grid::{WEEK_NUMBER_WIDTH, WEEKDAY_HEADER_HEIGHT};
use sidebar::CalendarFilter;
use week_grid::{TimedResize, task_timed_resize_edges};

const CALENDAR_HEADER_HEIGHT: f32 = 50.;

/// The user clicked a task in the calendar.
pub struct OpenTask(pub Id);

/// A task's right-click menu asked for one of the details' pickers (`due`, `tags`,
/// `link-parent`): the task opens in the list with that picker.
pub struct OpenTaskPicker(pub Id, pub &'static str);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarMode {
    /// One day on the hour axis, as the original's Day view.
    Day,
    Week,
    Month,
    /// The original's Multi-Day: this many days from the anchor on the hour axis.
    MultiDay(u8),
    /// The original's Agenda: the coming days with something due, as a list.
    Agenda,
    /// The original's Year: the twelve months as small calendars.
    Year,
    /// The original's Multi-Week: this many weeks from the anchor's, laid out like the month.
    MultiWeek(u8),
}

impl CalendarMode {
    /// Whether the days are drawn on the hour axis (all but Month and Agenda).
    pub fn is_timed(self) -> bool {
        !matches!(self, CalendarMode::Month | CalendarMode::Agenda | CalendarMode::Year | CalendarMode::MultiWeek(_))
    }
}

pub struct CalendarView {
    store: Rc<Store>,
    mode: CalendarMode,
    /// The shown month (first day) in month mode.
    month: NaiveDate,
    /// A day of the shown week in week mode.
    anchor: NaiveDate,
    selected_day: Option<NaiveDate>,
    sidebar_open: bool,
    sidebar_month: Option<NaiveDate>,
    /// Calendar sidebar filter: all tasks, or any combination of lists and tags.
    filter: CalendarFilter,
    /// Open month/year picker popover on the header title, storing the displayed year.
    month_picker_open: Option<i32>,
    task_editor: Option<TaskEditor>,
    saved_hint: Option<(String, gpui::Task<()>)>,
    mode_menu_open: bool,
    focus_handle: gpui::FocusHandle,
    /// The counts last chosen in the mode menu.
    multi_days: u8,
    multi_weeks: u8,
    tasks: Vec<Task>,
    /// The Agenda's days, from today (or the shown month's first day, when later) a year on.
    agenda: Vec<(NaiveDate, Vec<fern_core::agenda::AgendaItem>)>,
    /// The countdowns the Agenda lists, and the one whose card is open (its date and id).
    countdowns: Vec<fern_core::Countdown>,
    open_countdown: Option<(NaiveDate, Id)>,
    /// A task's right-click menu: the task snapshot, nesting state, position, and open side menu.
    context_menu: Option<ContextMenuState>,
    context_side: Option<u8>,
    /// The Year view's day card, opened by clicking a day.
    year_day: Option<(NaiveDate, NaiveDate)>,
    year_size: Option<gpui::Size<Pixels>>,
    year_scroll: calendar_year::YearScroll,
    year_heatmap: BTreeMap<i32, BTreeMap<NaiveDate, usize>>,
    year_heatmap_day: NaiveDate,
    year_jump: Option<calendar_year::YearJump>,
    /// The task card a click on a task opens beside it, as in the original: the task, the day
    /// it was clicked on, and the details it shows.
    card: Option<(Id, NaiveDate)>,
    /// A subscribed calendar's event whose card is open: its UID and the day it starts.
    open_event: Option<(String, NaiveDate)>,
    open_habit: Option<(Id, NaiveDate)>,
    detail: gpui::Entity<TaskDetail>,
    /// View Options, the header's "…" menu and the dialog it opens.
    options: CalendarOptions,
    more_menu: bool,
    options_dialog: Option<gpui::Entity<CalendarOptionsDialog>>,
    /// The "Arrange tasks" panel: the open tasks without a date, grouped by the tab's choice
    /// (lists, tags or priority), to drag onto a day.
    arrange: Option<usize>,
    arrange_collapsed: HashSet<(usize, String)>,
    undated: Vec<Task>,
    /// Open non-deleted horizon tasks in the active calendar filter scope.
    horizon_tasks: Vec<Task>,
    /// Cached lists and preferences (refreshed on `reload()`, never queried in `render()`).
    lists: Vec<fern_core::List>,
    preferences: fern_core::Preferences,
    /// The colors tasks take with Color By List or Tag.
    list_colors: std::collections::HashMap<Id, gpui::Hsla>,
    tag_colors: std::collections::HashMap<String, gpui::Hsla>,
    list_names: Vec<(Id, String)>,
    tag_names: Vec<String>,
    sidebar_busy_days: HashSet<NaiveDate>,
    /// With Show Habit, all habits and their check-ins; with Show Focus Records, the focus sessions.
    habits: Vec<fern_core::Habit>,
    habit_records: std::collections::HashMap<Id, Vec<fern_core::Checkin>>,
    focus_sessions: Vec<fern_core::FocusSession>,
    timed_resize: Option<TimedResize>,
    event_editor: Option<EventEditor>,
    pending_delete_event: Option<PendingDeleteEvent>,
    pending_complete: HashSet<Id>,
    text_edit_sync_task: Option<gpui::Task<()>>,
    active: bool,
    dirty: bool,
}

fn unfold_ics_lines(ics: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in ics.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t'))
            && let Some(prev) = out.last_mut()
        {
            prev.push_str(rest);
            continue;
        }
        out.push(line.to_owned());
    }
    out
}

/// Returns `true` if any top-level `VEVENT` property in `ics` defines recurrence state
/// (`RRULE`, `RDATE`, `EXDATE`, or `RECURRENCE-ID`) that single-event `EventDraft` serialization
/// would otherwise clobber.
fn ics_has_recurrence(ics: &str) -> bool {
    let mut in_vevent = false;
    let mut nested_depth = 0usize;
    for line in unfold_ics_lines(ics) {
        let upper = line.to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" {
            in_vevent = true;
            nested_depth = 0;
            continue;
        }
        if upper == "END:VEVENT" {
            in_vevent = false;
            continue;
        }
        if !in_vevent {
            continue;
        }
        if upper.starts_with("BEGIN:") {
            nested_depth += 1;
            continue;
        }
        if upper.starts_with("END:") {
            nested_depth = nested_depth.saturating_sub(1);
            continue;
        }
        if nested_depth > 0 {
            continue;
        }
        let prop = upper
            .split_once(':')
            .map_or(upper.as_str(), |(lhs, _)| lhs)
            .split_once(';')
            .map_or_else(
                || upper.split_once(':').map_or(upper.as_str(), |(lhs, _)| lhs),
                |(name, _)| name,
            );
        if matches!(prop, "RRULE" | "RDATE" | "EXDATE" | "RECURRENCE-ID") {
            return true;
        }
    }
    false
}

/// Extracts the IANA timezone (`TZID=...` or `UTC` for `Z`-suffixed instants) from a `VEVENT`'s
/// `DTSTART`, returning `None` for floating local times or all-day `DATE` values.
fn extract_ics_tzid(ics: &str) -> Option<String> {
    let mut in_vevent = false;
    let mut nested_depth = 0usize;
    for line in unfold_ics_lines(ics) {
        let upper = line.to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" {
            in_vevent = true;
            nested_depth = 0;
            continue;
        }
        if upper == "END:VEVENT" {
            in_vevent = false;
            continue;
        }
        if !in_vevent {
            continue;
        }
        if upper.starts_with("BEGIN:") {
            nested_depth += 1;
            continue;
        }
        if upper.starts_with("END:") {
            nested_depth = nested_depth.saturating_sub(1);
            continue;
        }
        if nested_depth > 0 {
            continue;
        }
        let Some((lhs, value)) = line.split_once(':') else { continue };
        let mut parts = lhs.split(';');
        let Some(name) = parts.next() else { continue };
        if !name.eq_ignore_ascii_case("DTSTART") {
            continue;
        }
        for param in parts {
            if let Some((k, v)) = param.split_once('=')
                && k.eq_ignore_ascii_case("TZID")
            {
                let trimmed = v.trim().trim_matches('"');
                if let Some(zone) = calendar::canonical_timezone_name(trimmed) {
                    return Some(zone.to_owned());
                }
            }
        }
        if value.trim().ends_with('Z') {
            return Some("UTC".to_owned());
        }
        return None;
    }
    None
}

fn dual_tz_reference_day(
    week: &[NaiveDate],
    selected_day: Option<NaiveDate>,
    anchor: NaiveDate,
    today: NaiveDate,
) -> NaiveDate {
    selected_day
        .filter(|d| week.contains(d))
        .or_else(|| week.contains(&today).then_some(today))
        .or_else(|| week.contains(&anchor).then_some(anchor))
        .or_else(|| week.first().copied())
        .unwrap_or(today)
}

/// A new task stays in a draft until it is saved. A day anchors the editor to a month cell;
/// without one it belongs to the toolbar's add button.
struct TaskEditor {
    view: gpui::Entity<CalendarTaskEditor>,
    day: Option<NaiveDate>,
}

/// The original's task card: 402 px wide, 301 px tall.
const CARD_WIDTH: f32 = 402.;
const CARD_HEIGHT: f32 = 301.;

/// How far ahead the Agenda reaches.
const AGENDA_DAYS: i64 = 366;

pub struct OpenHabit(pub Id);

impl EventEmitter<OpenTask> for CalendarView {}
impl EventEmitter<OpenTaskPicker> for CalendarView {}
impl EventEmitter<OpenHabit> for CalendarView {}
impl EventEmitter<crate::task_detail::StartFocus> for CalendarView {}

impl Focusable for CalendarView {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.task_editor.as_ref().map_or_else(|| self.focus_handle.clone(), |editor| editor.view.focus_handle(cx))
    }
}

impl CalendarView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, window, true, cx)
    }

    pub fn new_inactive(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_active(store, window, false, cx)
    }

    fn new_with_active(
        store: Rc<Store>,
        window: &mut Window,
        initial_active: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let today = Local::now().date_naive();
        let detail = cx.new(|cx| {
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.set_card(true);
            detail
        });
        // A subscribed calendar read anew shows its events.
        if let Some(feeds) = crate::subscriptions::feeds(cx) {
            cx.observe(&feeds, |this, _, cx| {
                this.year_heatmap.clear();
                cx.notify();
            }).detach();
        }
        cx.subscribe(&detail, |this, _, _: &TaskUpdated, cx| {
            this.text_edit_sync_task = None;
            crate::data_version::data_changed(cx.entity_id(), cx);
            this.card_updated(cx);
        })
        .detach();
        cx.subscribe(&detail, |this, _, edit: &TaskTextEdited, cx| {
            this.apply_task_text_edit(edit, cx);
        })
        .detach();
        cx.observe_global::<crate::data_version::DataVersion>(|this, cx| {
            if crate::data_version::changed_elsewhere(cx.entity_id(), cx) {
                this.sync_external(cx);
            }
        })
        .detach();
        cx.subscribe(&detail, |_, _, StartFocus(id, title, stopwatch), cx| cx.emit(StartFocus(*id, title.clone(), *stopwatch))).detach();
        // A subtask clicked in the card opens in the card.
        cx.subscribe(&detail, |this, _, crate::task_detail::OpenTask(id), cx| {
            if let Some((_, day)) = this.card {
                this.open_card(*id, day, cx);
            }
        })
        .detach();
        let mut this = Self {
            sidebar_open: store.setting("calendar_sidebar").ok().flatten().as_deref() == Some("1"),
            store,
            mode: CalendarMode::Month,
            month: calendar::month_start(today),
            anchor: today,
            selected_day: None,
            sidebar_month: None,
            filter: CalendarFilter::default(),
            month_picker_open: None,
            task_editor: None,
            saved_hint: None,
            mode_menu_open: false,
            focus_handle: cx.focus_handle(),
            multi_days: 3,
            multi_weeks: 5,
            tasks: Vec::new(),
            agenda: Vec::new(),
            countdowns: Vec::new(),
            open_countdown: None,
            context_menu: None,
            year_day: None,
            year_size: None,
            year_scroll: calendar_year::YearScroll::default(),
            year_heatmap: BTreeMap::new(),
            year_heatmap_day: today,
            year_jump: None,
            context_side: None,
            card: None,
            open_event: None,
            open_habit: None,
            detail,
            options: CalendarOptions::default(),
            more_menu: false,
            options_dialog: None,
            arrange: None,
            arrange_collapsed: HashSet::new(),
            undated: Vec::new(),
            horizon_tasks: Vec::new(),
            lists: Vec::new(),
            preferences: fern_core::Preferences::default(),
            list_colors: Default::default(),
            tag_colors: Default::default(),
            list_names: Vec::new(),
            tag_names: Vec::new(),
            sidebar_busy_days: HashSet::new(),
            habits: Vec::new(),
            habit_records: std::collections::HashMap::new(),
            focus_sessions: Vec::new(),
            timed_resize: None,
            event_editor: None,
            pending_delete_event: None,
            pending_complete: HashSet::new(),
            text_edit_sync_task: None,
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
            self.card_updated(cx);
        }
    }

    pub fn sync_external(&mut self, cx: &mut Context<Self>) {
        if self.active {
            self.card_updated(cx);
        } else {
            self.dirty = true;
        }
    }

    /// The days on the hour axis: the anchor's week, the anchor, or several days from it.
    fn shown_days(&self) -> Vec<NaiveDate> {
        match self.mode {
            CalendarMode::Day => vec![self.anchor],
            CalendarMode::MultiDay(n) => (0..n as i64).map(|i| self.anchor + chrono::Duration::days(i)).collect(),
            _ => calendar::week_of(self.anchor).into_iter().filter(|d| self.options.shows_day(*d)).collect(),
        }
    }

    /// The tasks a day shows, as View Options say.
    fn tasks_on(&self, date: NaiveDate) -> Vec<&Task> {
        self.tasks.iter().filter(|t| self.options.shows_task(t, date)).collect()
    }

    /// A task's color, by its list, first tag or priority as View Options → Color By says.
    fn task_tint(&self, task: &Task, theme: &theme::Theme) -> gpui::Hsla {
        let accent = theme.colors().accent;
        match self.options.color_by {
            ColorBy::List => task.list_id.and_then(|id| self.list_colors.get(&id).copied()).unwrap_or(accent),
            ColorBy::Tag => task.tags.first().and_then(|t| self.tag_colors.get(&t.to_lowercase()).copied()).unwrap_or(accent),
            ColorBy::Priority => task_rows::priority_color(task.priority, theme).unwrap_or(accent),
        }
    }

    /// The extra items of a day, as View Options say: today's habits (as the original's calendar,
    /// on today only), the day's focus records, and its countdowns.
    /// The subscribed calendars' events on `date`, all-day ones first, then by time.
    fn events_on(&self, date: NaiveDate, cx: &App) -> Vec<EventBar> {
        self.events_between(date, date, cx)
    }

    /// The subscribed calendars' events covering any day from `from` to `to`.
    fn events_between(&self, from: NaiveDate, to: NaiveDate, cx: &App) -> Vec<EventBar> {
        let Some(feeds) = crate::subscriptions::feeds(cx) else { return Vec::new() };
        let accent = cx.theme().colors().accent;
        let mut events: Vec<EventBar> = feeds
            .read(cx)
            .occurrences(from, to)
            .into_iter()
            .map(|occurrence| EventBar {
                key: occurrence.event.uid.clone(),
                subscription_id: occurrence.calendar.id,
                writable: occurrence.calendar.is_caldav() && occurrence.calendar.writable,
                recurring: occurrence.event.repeat.is_some(),
                title: occurrence.event.title.clone(),
                start: occurrence.times.start,
                end: occurrence.times.end,
                start_time: occurrence.times.start_time,
                end_time: occurrence.times.end_time,
                location: occurrence.event.location.clone(),
                notes: occurrence.event.notes.clone(),
                calendar: occurrence.calendar.title().to_owned(),
                color: crate::lists_panel::parse_color(occurrence.calendar.color.as_deref()).unwrap_or(accent),
            })
            .collect();
        events.sort_by_key(|event| (event.start_time.is_some(), event.start_time));
        events
    }

    fn dismiss_transient(&mut self, _: &DismissTransient, window: &mut Window, cx: &mut Context<Self>) {
        if !self.try_dismiss_transient(window, cx) {
            cx.propagate();
        }
    }

    pub fn try_dismiss_transient(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.timed_resize.take().is_some() {
            cx.notify();
            true
        } else if self.pending_delete_event.take().is_some() || self.event_editor.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
            true
        } else if self.context_menu.take().is_some() {
            self.context_side = None;
            cx.notify();
            true
        } else if std::mem::take(&mut self.mode_menu_open)
            || std::mem::take(&mut self.more_menu)
            || self.month_picker_open.take().is_some()
        {
            cx.notify();
            true
        } else if self.year_jump.is_some() {
            self.close_year_jump(window, cx);
            true
        } else if let Some(editor) = &self.task_editor {
            editor.view.update(cx, |editor, cx| editor.dismiss(window, cx));
            true
        } else if self.options_dialog.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
            true
        } else if self.card.is_some() {
            if self.detail.update(cx, |detail, cx| detail.dismiss_popup(window, cx)) {
                return true;
            }
            self.close_card(cx);
            window.focus(&self.focus_handle, cx);
            true
        } else if self.open_event.take().is_some()
            || self.open_habit.take().is_some()
            || self.open_countdown.take().is_some()
            || self.year_day.take().is_some()
        {
            cx.notify();
            true
        } else {
            false
        }
    }

    pub(super) fn is_task_read_only(&self, id: Id) -> bool {
        self.store.is_task_read_only(id).unwrap_or(false)
    }

    fn finish_toggle_task_completed(
        &mut self,
        task: &Task,
        completed: bool,
        cx: &mut Context<Self>,
    ) {
        let id = task.id;
        let result = if completed && !task.is_wont_do() {
            crate::undo::complete(&self.store, task).map(|_| ())
        } else if task.is_wont_do() {
            self.store.set_wont_do(id, false)
        } else {
            self.store.set_completed(id, completed)
        };
        if let Err(err) = result {
            eprintln!("failed to toggle calendar task: {err:#}");
            return;
        }
        if completed && !task.is_wont_do() {
            desktop_common::sound::play_complete(&self.store);
        }
        crate::data_version::data_changed(cx.entity_id(), cx);
        self.card_updated(cx);
    }

    fn toggle_task_completed(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
        if self.is_task_read_only(id) {
            return;
        }
        let Some(task) = self.store.task(id).ok().flatten() else { return };
        if completed
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
                        if let Some(current) = this.store.task(id).ok().flatten()
                            && !current.is_closed()
                        {
                            this.finish_toggle_task_completed(&current, true, cx);
                        }
                    });
                },
            );
            return;
        }
        self.finish_toggle_task_completed(&task, completed, cx);
    }

    /// The card's task changed: the calendar shows the change, and the card closes when the task
    /// is gone.
    fn card_updated(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
        let Some((id, _)) = self.card else { return };
        match self.store.task(id) {
            Ok(Some(task)) if !task.is_deleted() => self.detail.update(cx, |detail, cx| detail.refresh(Some(task), cx)),
            _ => self.close_card(cx),
        }
    }

    fn apply_task_text_edit(&mut self, edit: &TaskTextEdited, cx: &mut Context<Self>) {
        if self.mode == CalendarMode::Agenda || self.dirty {
            self.reload(cx);
        } else {
            for list in [&mut self.tasks, &mut self.undated, &mut self.horizon_tasks] {
                for task in list.iter_mut().filter(|t| t.id == edit.id) {
                    match &edit.field {
                        TaskTextField::Title(title) => task.title = title.clone(),
                        TaskTextField::Notes(notes) => task.notes = notes.clone(),
                        TaskTextField::Location(location) => task.location = location.clone(),
                    }
                }
            }
            cx.notify();
        }
        self.text_edit_sync_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let _ = this.update(cx, |this, cx| {
                this.text_edit_sync_task = None;
                crate::data_version::data_changed(cx.entity_id(), cx);
            });
        }));
    }

    /// The header's "…" menu and what it opens, for the debug-build dev state
    /// (`calendar-more=menu|options|color|color-by|style|arrange|arrange-tags|extras`).
    #[cfg(debug_assertions)]
    pub fn dev_more(&mut self, what: &str, cx: &mut Context<Self>) {
        match what {
            "menu" => self.more_menu = true,
            "options" => self.open_options(Page::Main, cx),
            "color" => self.open_options(Page::Color, cx),
            "color-by" => {
                self.open_options(Page::Color, cx);
                if let Some(dialog) = &self.options_dialog {
                    dialog.update(cx, |d, _| d.dev_color_menu());
                }
            }
            "style" => self.open_options(Page::Style, cx),
            "color-habit" | "color-countdown" => {
                self.open_options(Page::Color, cx);
                let tab = if what == "color-habit" { 1 } else { 2 };
                if let Some(dialog) = &self.options_dialog {
                    dialog.update(cx, |d, _| d.dev_color_tab(tab));
                }
            }
            "arrange" => self.arrange = Some(0),
            "arrange-tags" => self.arrange = Some(1),
            // Show Habit and Show Focus Records turned on.
            "extras" => {
                let options = CalendarOptions { show_habits: true, show_focus: true, ..self.store.calendar_options().unwrap_or_default() };
                self.store.save_calendar_options(&options).ok();
            }
            _ => {}
        }
        self.reload(cx);
    }

    /// Opens a task's card from the debug-build dev state (`calendar-card=<title part>`).
    #[cfg(debug_assertions)]
    pub fn dev_card(&mut self, needle: &str, cx: &mut Context<Self>) {
        if let Some(task) = self.tasks.iter().find(|t| t.title.contains(needle)) {
            let day = task.due_date.unwrap_or_else(|| Local::now().date_naive());
            self.open_card(task.id, day, cx);
        }
    }

    /// Opens a task's right-click menu from the debug-build dev state (`calendar-context=<title part>`).
    #[cfg(debug_assertions)]
    pub fn dev_context(&mut self, needle: &str, cx: &mut Context<Self>) {
        if let Some(task) = self.tasks.iter().find(|t| t.title.contains(needle)) {
            self.open_context(task.id, gpui::point(px(600.), px(300.)), cx);
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        self.year_heatmap.clear();
        self.options = self.store.calendar_options().unwrap_or_default();
        self.preferences = self.store.preferences().unwrap_or_default();
        let lists = self.store.lists().unwrap_or_default();
        self.list_colors = lists.iter().filter_map(|l| crate::lists_panel::parse_color(l.color.as_deref()).map(|c| (l.id, c))).collect();
        self.list_names = lists.iter().filter(|l| !l.archived).map(|l| (l.id, l.name.clone())).collect();
        self.lists = lists;
        if let Some(menu) = &mut self.context_menu {
            match self.store.task(menu.id) {
                Ok(Some(task)) if !task.is_deleted() => {
                    menu.nesting = crate::task_menu::TaskNesting::of(&self.store, task.id);
                    menu.task = task;
                }
                _ => {
                    self.context_menu = None;
                    self.context_side = None;
                }
            }
        }
        let all_tags = self.store.tags().unwrap_or_default();
        self.tag_names = all_tags.iter().map(|t| t.name.clone()).collect();
        self.tag_colors = all_tags
            .into_iter()
            .filter_map(|t| crate::lists_panel::parse_color(t.color.as_deref()).map(|c| (t.name.to_lowercase(), c)))
            .collect();
        self.filter = self
            .store
            .setting("calendar_list_filter")
            .ok()
            .flatten()
            .map(|raw| CalendarFilter::parse(&raw, &self.list_names, &self.tag_names))
            .unwrap_or_default();
        let filter = self.filter.clone();
        let in_scope = |task: &Task| filter.matches(task);
        if self.mode == CalendarMode::Agenda {
            let from = self.agenda_start();
            self.countdowns = self.store.countdowns().unwrap_or_default();
            match self.store.tasks() {
                Ok(mut tasks) => {
                    tasks.retain(in_scope);
                    self.sidebar_busy_days = tasks
                        .iter()
                        .filter(|t| !t.is_closed() && !t.is_deleted())
                        .filter_map(|t| t.due_date)
                        .collect();
                    let countdowns: &[fern_core::Countdown] = if self.options.show_countdowns { &self.countdowns } else { &[] };
                    let until = from + chrono::Duration::days(AGENDA_DAYS);
                    let holidays = self.store.holiday_dates(from, until).unwrap_or_default();
                    self.agenda = fern_core::agenda::agenda_with_holidays(&tasks, countdowns, &holidays, from, until)
                }
                Err(err) => eprintln!("failed to load the agenda: {err:#}"),
            }
            cx.notify();
            return;
        }
        let range = match self.mode {
            CalendarMode::Month | CalendarMode::MultiWeek(_) => {
                let weeks = self.month_grid_weeks();
                Some((weeks[0][0], weeks[weeks.len() - 1][6]))
            }
            // Year navigation only moves a viewport over the same data, including repeats
            // that started before the visible year.
            CalendarMode::Year => None,
            _ => {
                let days = self.shown_days();
                Some((days[0], days[days.len() - 1]))
            }
        };
        let Ok(mut all) = self.store.tasks().inspect_err(|err| eprintln!("failed to load calendar tasks: {err:#}")) else {
            cx.notify();
            return;
        };
        all.retain(in_scope);
        self.sidebar_busy_days = all
            .iter()
            .filter(|t| !t.is_closed() && !t.is_deleted())
            .filter_map(|t| t.due_date)
            .collect();
        self.tasks.clear();
        self.undated.clear();
        self.horizon_tasks.clear();
        let mut repeats_before_range = Vec::new();
        for task in all.into_iter().filter(|task| !task.is_deleted()) {
            if !task.is_closed() && task.horizon.is_some() {
                self.horizon_tasks.push(task.clone());
            }
            let Some(due) = task.due_date else {
                if !task.is_closed() && task.parent_id.is_none() {
                    self.undated.push(task);
                }
                continue;
            };
            if range.is_none_or(|(start, end)| due <= end && task.end_date.unwrap_or(due).max(due) >= start) {
                self.tasks.push(task);
            } else if range.is_some_and(|(start, _)| due < start)
                && self.options.show_repeats && task.repeat.is_some() && !task.is_closed()
            {
                // As before, recurring occurrences go after the sorted tasks due in range.
                repeats_before_range.push(task);
            }
        }
        self.tasks.sort_by_key(|task| (task.due_date, task.due_time.is_some(), task.due_time));
        self.tasks.extend(repeats_before_range);
        self.countdowns = self.store.countdowns().unwrap_or_default();
        if self.options.show_habits {
            self.habits = self.store.habits().unwrap_or_default();
            self.habit_records = self.store.all_habit_checkins().unwrap_or_default();
        } else {
            self.habits.clear();
            self.habit_records.clear();
        }
        self.focus_sessions = if self.options.show_focus { self.store.focus_sessions().unwrap_or_default() } else { Vec::new() };
        cx.notify();
    }

    /// The Agenda starts today in the current month, and on the first day of a later one.
    fn agenda_start(&self) -> NaiveDate {
        self.selected_day.unwrap_or_else(|| Local::now().date_naive().max(self.month))
    }

    fn can_shift(&self, delta: i32) -> bool {
        if self.mode != CalendarMode::Year {
            return true;
        }
        calendar_year::YEAR_RANGE.contains(&(self.month.year() + delta))
    }

    fn shift(&mut self, delta: i32, cx: &mut Context<Self>) {
        if !self.can_shift(delta) || !self.finish_task_before_navigation(cx) {
            return;
        }
        if self.mode == CalendarMode::Year {
            let january = NaiveDate::from_ymd_opt(self.month.year() + delta, 1, 1).unwrap_or(self.month);
            self.navigate_year(january, cx);
            return;
        }
        self.selected_day = None;
        self.sidebar_month = None;
        self.year_day = None;
        self.open_event = None;
        match self.mode {
            CalendarMode::Month => self.month = calendar::add_months(self.month, delta),
            CalendarMode::Agenda => {
                self.month = calendar::add_months(calendar::month_start(self.agenda_start()), delta);
            }
            mode => {
                let step = match mode {
                    CalendarMode::Day => 1,
                    CalendarMode::MultiDay(n) => n as i64,
                    CalendarMode::MultiWeek(n) => 7 * n as i64,
                    _ => 7,
                };
                self.anchor += chrono::Duration::days(step * delta as i64);
                self.month = calendar::month_start(self.anchor);
            }
        }
        self.card = None;
        self.reload(cx);
    }

    pub fn set_mode(&mut self, mode: CalendarMode, cx: &mut Context<Self>) {
        self.mode_menu_open = false;
        self.apply_mode(mode, cx);
    }

    fn apply_mode(&mut self, mode: CalendarMode, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let prev_mode = self.mode;
        self.mode = mode;
        if prev_mode == CalendarMode::Year
            && mode != CalendarMode::Year
            && self.anchor.year() == self.month.year()
        {
            self.month = calendar::month_start(self.anchor);
        }
        self.year_day = None;
        self.year_scroll = calendar_year::YearScroll::default();
        self.year_jump = None;
        match mode {
            CalendarMode::Year => {
                let day = calendar_year::clamp_year_day(self.month);
                if day != self.month {
                    self.anchor = day;
                    self.selected_day = None;
                    self.sidebar_month = None;
                }
                self.month = NaiveDate::from_ymd_opt(day.year(), 1, 1).unwrap_or(self.month);
            }
            CalendarMode::MultiDay(days) => self.multi_days = days,
            CalendarMode::MultiWeek(weeks) => self.multi_weeks = weeks,
            _ => {}
        }
        if (mode.is_timed() || matches!(mode, CalendarMode::MultiWeek(_))) && calendar::month_start(self.anchor) != self.month {
            self.anchor = self.month;
        }
        self.card = None;
        self.reload(cx);
    }

    /// Opens a countdown's card in the Agenda, for the debug-build dev state.
    #[cfg(debug_assertions)]
    pub fn dev_open_countdown(&mut self, name: &str, cx: &mut Context<Self>) {
        self.open_countdown = self
            .agenda
            .iter()
            .find_map(|(date, items)| items.iter().find(|i| i.countdown && i.title.contains(name)).map(|i| (*date, i.task)));
        cx.notify();
    }

    #[cfg(debug_assertions)]
    pub fn dev_year_day(&mut self, day: NaiveDate, cx: &mut Context<Self>) {
        self.year_day = Some((calendar::month_start(day), day));
        cx.notify();
    }

    #[cfg(debug_assertions)]
    pub fn dev_open_menu(&mut self, cx: &mut Context<Self>) {
        self.mode_menu_open = true;
        cx.notify();
    }

    /// Moves a dropped task to `date`, keeping its time of day.
    fn drop_on_day(&mut self, id: Id, date: NaiveDate, cx: &mut Context<Self>) {
        if self.is_task_read_only(id) {
            return;
        }
        if let Err(err) = self.store.move_dates(id, date) {
            eprintln!("failed to move the task: {err:#}");
        } else {
            crate::data_version::data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    /// Resizes a task's start or end date to `day`.
    fn resize_task_to_day(&mut self, resized: &ResizedTask, day: NaiveDate, cx: &mut Context<Self>) {
        if self.is_task_read_only(resized.id) {
            return;
        }
        let (start, end) = match resized.edge {
            ResizeEdge::Start => (day.min(resized.end), resized.end),
            ResizeEdge::End => (resized.start, day.max(resized.start)),
        };
        if let Err(err) = self.store.resize_dates(resized.id, start, end) {
            eprintln!("failed to resize task dates: {err:#}");
        } else {
            crate::data_version::data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn start_timed_resize(
        &mut self,
        id: Id,
        date: NaiveDate,
        edge: ResizeEdge,
        orig_start: NaiveTime,
        orig_end: NaiveTime,
        end_date: Option<NaiveDate>,
        start_y: Pixels,
        cx: &mut Context<Self>,
    ) {
        if self.is_task_read_only(id) {
            return;
        }
        if let Some(task) = self.tasks.iter().find(|t| t.id == id) {
            let (can_start, can_end) = task_timed_resize_edges(task, date);
            let allowed = match edge {
                ResizeEdge::Start => can_start,
                ResizeEdge::End => can_end,
            };
            if !allowed {
                return;
            }
        }
        let current_time = match edge {
            ResizeEdge::Start => orig_start,
            ResizeEdge::End => orig_end,
        };
        self.timed_resize = Some(TimedResize { id, date, edge, orig_start, orig_end, end_date, start_y, current_time });
        cx.notify();
    }

    fn update_timed_resize(&mut self, mouse_y: Pixels, cx: &mut Context<Self>) {
        let Some(resize) = &mut self.timed_resize else { return };
        let base_time = match resize.edge {
            ResizeEdge::Start => resize.orig_start,
            ResizeEdge::End => resize.orig_end,
        };
        let next = week_axis::time_at_offset(week_axis::offset(base_time) + f32::from(mouse_y - resize.start_y));
        if resize.current_time != next {
            resize.current_time = next;
            cx.notify();
        }
    }

    fn finish_timed_resize(&mut self, cx: &mut Context<Self>) {
        let Some(resize) = self.timed_resize.take() else { return };
        let (start, end) = resize.snapped_range();
        let existing_task = self.tasks.iter().find(|t| t.id == resize.id);
        let save_date = existing_task.and_then(|t| t.due_date).unwrap_or(resize.date);
        let save_start = if resize.edge == ResizeEdge::Start && save_date == resize.date {
            start
        } else {
            existing_task.and_then(|t| t.due_time).unwrap_or(start)
        };
        let save_end_date = existing_task.and_then(|t| t.end_date);
        let save_end = if resize.edge == ResizeEdge::End && save_end_date.unwrap_or(save_date) == resize.date {
            end
        } else {
            existing_task.and_then(|t| t.end_time).unwrap_or(end)
        };
        if let Err(err) = self.store.resize_timed_range(resize.id, save_date, save_start, save_end, save_end_date) {
            eprintln!("failed to resize timed task: {err:#}");
        } else {
            crate::data_version::data_changed(cx.entity_id(), cx);
        }
        self.reload(cx);
    }

    fn move_event_to_day(&mut self, dragged: &DraggedEvent, target_day: NaiveDate, cx: &mut Context<Self>) {
        let range = self.events_between(self.anchor - chrono::Duration::days(365), self.anchor + chrono::Duration::days(365), cx);
        let Some(existing) = range.into_iter().find(|e| e.subscription_id == dragged.subscription_id && e.key == dragged.uid) else {
            return;
        };
        let Some(feeds) = crate::subscriptions::feeds(cx) else {
            return;
        };
        let stored = feeds.read(cx).stored_event(dragged.subscription_id, &dragged.uid);
        // Never clobber a recurring master series via single-event drag-move.
        if existing.recurring || stored.as_ref().is_some_and(|s| ics_has_recurrence(&s.ics)) {
            return;
        }
        let span = (existing.end - existing.start).num_days().max(0);
        let target_end_day = target_day + chrono::Duration::days(span);
        let preserved_tz = stored
            .as_ref()
            .and_then(|s| extract_ics_tzid(&s.ics))
            .and_then(|z| calendar::canonical_timezone_name(&z));
        let (start_date, start_time, end_date, end_time, timezone) = match (existing.start_time, preserved_tz) {
            (Some(st), Some(tz)) => {
                let et = existing.end_time.unwrap_or_else(|| {
                    let end_dt = target_end_day.and_time(st) + chrono::Duration::hours(1);
                    end_dt.time()
                });
                let adjusted_end_day = if span == 0 && et < st {
                    target_day + chrono::Duration::days(1)
                } else {
                    target_end_day
                };
                let (z_sd, z_st) = calendar::local_to_timezone(target_day, st, tz);
                let (z_ed, z_et) = calendar::local_to_timezone(adjusted_end_day, et, tz);
                (z_sd, Some(z_st), z_ed.max(z_sd), Some(z_et), Some(tz.to_owned()))
            }
            _ => (target_day, existing.start_time, target_end_day, existing.end_time, None),
        };
        let draft = fern_core::ical::EventDraft {
            uid: existing.key,
            title: existing.title,
            location: existing.location,
            notes: existing.notes,
            start_date,
            start_time,
            end_date,
            end_time,
            all_day: existing.start_time.is_none(),
            timezone,
        };
        feeds.update(cx, |feeds, cx| {
            if let Err(err) = feeds.save_event(dragged.subscription_id, &draft, cx) {
                eprintln!("failed to move CalDAV event: {err:#}");
            }
        });
        cx.notify();
    }

    fn go_to_today(&mut self, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let today = Local::now().date_naive();
        if self.mode == CalendarMode::Year {
            self.anchor = calendar_year::clamp_year_day(today);
            self.month = NaiveDate::from_ymd_opt(self.anchor.year(), 1, 1).unwrap_or(self.month);
        } else {
            self.anchor = today;
            self.month = calendar::month_start(today);
        }
        self.selected_day = None;
        self.sidebar_month = None;
        self.year_day = None;
        self.open_event = None;
        self.year_scroll = calendar_year::YearScroll::default();
        self.reload(cx);
    }

    fn select_month(&mut self, year: i32, month: u32, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let today = Local::now().date_naive();
        let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(self.month);
        let target = if today.year() == year && today.month() == month { today } else { first };
        self.month = first;
        self.anchor = target;
        self.selected_day = None;
        self.sidebar_month = None;
        self.month_picker_open = None;
        self.year_day = None;
        self.open_event = None;
        self.close_card(cx);
        self.reload(cx);
    }

    fn select_day(&mut self, day: NaiveDate, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let day = if self.mode == CalendarMode::Year {
            let day = calendar_year::clamp_year_day(day);
            self.month = NaiveDate::from_ymd_opt(day.year(), 1, 1).unwrap_or(self.month);
            self.year_scroll = calendar_year::YearScroll::default();
            day
        } else {
            self.month = calendar::month_start(day);
            day
        };
        self.selected_day = Some(day);
        self.anchor = day;
        self.sidebar_month = None;
        self.year_day = None;
        self.open_event = None;
        self.close_card(cx);
        self.reload(cx);
    }

    fn new_task_day(&self, today: NaiveDate) -> NaiveDate {
        use chrono::Duration;
        let (start, end) = match self.mode {
            CalendarMode::Month => (self.month, calendar::add_months(self.month, 1).pred_opt().unwrap_or(self.month)),
            CalendarMode::Year => {
                (self.month, calendar::add_months(self.month, 12).pred_opt().unwrap_or(self.month))
            }
            CalendarMode::MultiWeek(count) => {
                let start = calendar::week_of(self.anchor)[0];
                (start, start + Duration::days(i64::from(count) * 7 - 1))
            }
            CalendarMode::Agenda => (self.agenda_start(), self.agenda_start() + Duration::days(AGENDA_DAYS)),
            _ => {
                let days = self.shown_days();
                (days[0], days[days.len() - 1])
            }
        };
        let visible = |day| (start..=end).contains(&day)
            && (!matches!(self.mode, CalendarMode::Month | CalendarMode::Week | CalendarMode::MultiWeek(_)) || self.options.shows_day(day));
        self.selected_day.filter(|day| visible(*day))
            .or_else(|| visible(today).then_some(today))
            .or_else(|| start.iter_days().take_while(|day| *day <= end).find(|day| visible(*day)))
            .unwrap_or(start)
    }

    /// Changing the view must never detach a cell's editor while its draft is still active.
    pub fn finish_task_before_navigation(&mut self, cx: &mut Context<Self>) -> bool {
        self.task_editor.as_ref().is_none_or(|editor| editor.view.update(cx, |editor, cx| editor.close(cx)))
    }

    fn open_task_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &self.task_editor {
            window.focus(&editor.view.focus_handle(cx), cx);
            return;
        }
        let date = self.new_task_day(Local::now().date_naive());
        self.create_task_editor(date, None, window, cx);
    }

    fn open_day_task_editor(&mut self, date: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &self.task_editor
            && (editor.day == Some(date) || !editor.view.update(cx, |editor, cx| editor.close(cx)))
        {
            window.focus(&editor.view.focus_handle(cx), cx);
            return;
        }
        self.selected_day = Some(date);
        self.anchor = date;
        // Leading and trailing days belong to their actual date, without changing the grid
        // underneath the pointer when a day from the adjacent month is clicked.
        self.create_task_editor(date, Some(date), window, cx);
    }

    fn create_task_editor(&mut self, date: NaiveDate, day: Option<NaiveDate>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_card(cx);
        self.open_event = None;
        self.open_countdown = None;
        self.year_day = None;
        let list = self.filter.single_list().unwrap_or_else(|| {
            self.preferences
                .default_list
                .filter(|id| self.list_names.iter().any(|(list, _)| list == id))
        });
        let editor = cx.new(|cx| CalendarTaskEditor::new(self.store.clone(), date, list, day.is_none(), window, cx));
        cx.subscribe_in(&editor, window, |this, editor, event, window, cx| {
            // An outside click can save this draft and open the next one in the same event.
            // Its delayed notification must not close or steal focus from the new editor.
            if this.task_editor.as_ref().is_some_and(|open| open.view == *editor) {
                this.task_editor = None;
                if editor.read(cx).owns_focus(window, cx) {
                    window.focus(&this.focus_handle, cx);
                }
            }
            if let EditorEvent::Saved(task) = event {
                this.hint_if_outside_filter(*task, cx);
                crate::data_version::data_changed(cx.entity_id(), cx);
                this.reload(cx);
            } else {
                cx.notify();
            }
        }).detach();
        window.focus(&editor.focus_handle(cx), cx);
        self.task_editor = Some(TaskEditor { view: editor, day });
        cx.notify();
    }

    fn hint_if_outside_filter(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.filter.all {
            return;
        }
        let Ok(Some(task)) = self.store.task(id) else {
            return;
        };
        if self.filter.matches(&task) {
            return;
        }
        let strings = i18n::t(cx);
        let list = self.list_names.iter().find(|(id, _)| Some(*id) == task.list_id)
            .map_or(strings.inbox, |(_, name)| name.as_str());
        let message = strings.calendar_saved_outside_filter.replace("{list}", list);
        let timeout = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            let _ = this.update(cx, |this, cx| {
                this.saved_hint = None;
                cx.notify();
            });
        });
        self.saved_hint = Some((message, timeout));
    }

    fn has_modal(&self) -> bool {
        self.event_editor.is_some()
            || self.options_dialog.is_some()
            || self.pending_delete_event.is_some()
    }

    /// ISO week horizon for the currently displayed week in Week view.
    fn displayed_week_horizon(&self) -> fern_core::Horizon {
        let week = calendar::week_of(self.anchor);
        let ref_day = week
            .iter()
            .find(|d| d.weekday() == chrono::Weekday::Thu)
            .copied()
            .unwrap_or(week[3]);
        fern_core::Horizon::current_week(ref_day)
    }

    /// Collects open non-deleted tasks for `target`, plus unfinished past tasks of the same
    /// granularity when `include_past_when_current` is true and `target` is the current period.
    /// Returns `(task, is_rollover)` pairs with exact matches first, followed by past rollovers.
    pub(super) fn horizon_goals_for(
        &self,
        target: fern_core::Horizon,
        today: NaiveDate,
        include_past_when_current: bool,
    ) -> Vec<(&Task, bool)> {
        if !calendar::horizons_enabled() {
            return Vec::new();
        }
        let lists: &[fern_core::List] = if self.filter.all { &self.lists } else { &[] };
        let (in_target, past_rollover) = fern_core::horizon::horizons_bucket_partition(
            &self.horizon_tasks,
            lists,
            target,
            today,
            false,
        );
        let mut out: Vec<(&Task, bool)> = in_target.into_iter().map(|t| (t, false)).collect();
        if include_past_when_current {
            out.extend(past_rollover.into_iter().map(|t| (t, true)));
        }
        out
    }

    /// Renders a sleek, compact horizontal banner of horizon goals with draggable task chips.
    pub(super) fn render_horizon_goals_banner(
        &self,
        banner_id: &'static str,
        heading: &'static str,
        badge: String,
        goals: Vec<(&Task, bool)>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if goals.is_empty() {
            return None;
        }
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();

        let chips: Vec<AnyElement> = goals
            .into_iter()
            .map(|(task, is_rollover)| {
                let id = task.id;
                let title = if task.title.trim().is_empty() {
                    strings.untitled.to_owned()
                } else {
                    task.title.clone()
                };
                let dragged = DraggedTask {
                    id,
                    title: title.clone().into(),
                };
                let origin_badge = is_rollover
                    .then(|| task.horizon.map(|h| calendar::format_horizon_badge(h, strings)))
                    .flatten();
                let marker = if task.kind == fern_core::Kind::Note {
                    Icon::new(IconName::FileText)
                        .size(IconSize::XSmall)
                        .color(Color::IconMuted)
                        .into_any_element()
                } else {
                    div()
                        .debug_selector(move || format!("calendar-horizon-check-{id}"))
                        .flex_none()
                        .flex()
                        .items_center()
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            Checkbox::new(SharedString::from(format!("calendar-horizon-cb-{id}")))
                                .size(px(12.))
                                .toggle_state(task.is_completed())
                                .in_progress(task.is_in_progress())
                                .lines(task.checklist.is_some())
                                .tint(task_rows::priority_color(task.priority, &theme))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_task_completed(id, true, cx);
                                })),
                        )
                        .into_any_element()
                };

                h_flex()
                    .id(SharedString::from(format!("calendar-horizon-chip-{id}")))
                    .debug_selector(move || format!("calendar-horizon-chip-{id}"))
                    .flex_none()
                    .max_w(px(220.))
                    .h(px(24.))
                    .px_2()
                    .gap_1p5()
                    .rounded(px(6.))
                    .bg(colors.surface_background)
                    .border_1()
                    .border_color(colors.border_variant)
                    .hover(|s| s.bg(colors.element_hover).border_color(colors.border))
                    .cursor_pointer()
                    .tooltip(ui::Tooltip::text(format!(
                        "{title} · {}",
                        strings.calendar_drop_to_schedule
                    )))
                    .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        cx.emit(OpenTask(id));
                    }))
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_context(id, event.position, cx);
                            window.focus(&this.focus_handle, cx);
                        }),
                    )
                    .child(marker)
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(title)
                                .size(LabelSize::XSmall)
                                .color(Color::Default)
                                .truncate(),
                        ),
                    )
                    .when_some(origin_badge, |el, origin| {
                        let (fg, bg, _) = task_rows::horizon_badge_style(true, task.is_completed(), &theme);
                        el.child(
                            div()
                                .flex_none()
                                .px_1()
                                .rounded(px(3.))
                                .bg(bg)
                                .child(
                                    Label::new(origin)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Custom(fg)),
                                ),
                        )
                    })
                    .into_any_element()
            })
            .collect();

        let scroll_id = SharedString::from(format!("{banner_id}-scroll"));
        let chips_id = SharedString::from(format!("{banner_id}-chips"));
        let label_id = SharedString::from(format!("{banner_id}-label"));

        Some(
            h_flex()
                .id(banner_id)
                .debug_selector(move || banner_id.to_owned())
                .flex_none()
                .items_start()
                .px_4()
                .py(px(6.))
                .gap_2p5()
                .bg(colors.panel_background.opacity(0.55))
                .border_y_1()
                .border_color(colors.border_variant.opacity(0.75))
                .child(
                    h_flex()
                        .id(label_id)
                        .flex_none()
                        .h(px(24.))
                        .gap_1p5()
                        .tooltip(ui::Tooltip::text(strings.calendar_drop_to_schedule))
                        .child(
                            Icon::new(IconName::FernHorizons)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(
                            Label::new(heading)
                                .size(LabelSize::XSmall)
                                .weight(FontWeight::MEDIUM)
                                .color(Color::Muted),
                        )
                        .child(
                            div()
                                .px_1p5()
                                .h(px(18.))
                                .flex()
                                .items_center()
                                .rounded(px(4.))
                                .bg(colors.accent.opacity(0.12))
                                .child(
                                    Label::new(badge)
                                        .size(LabelSize::XSmall)
                                        .weight(FontWeight::MEDIUM)
                                        .color(Color::Accent),
                                ),
                        ),
                )
                .child(
                    v_scroll(
                        scroll_id,
                        div()
                            .id(chips_id)
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_1p5()
                            .children(chips),
                    )
                    .flex_1()
                    .min_w_0()
                    .max_h(px(62.))
                    .compact(),
                )
                .into_any_element(),
        )
    }

    fn toolbar_month(&self) -> NaiveDate {
        match self.mode {
            CalendarMode::Month | CalendarMode::Year => self.month,
            CalendarMode::Agenda => calendar::month_start(self.agenda_start()),
            CalendarMode::Week | CalendarMode::MultiWeek(_) => calendar::week_of(self.anchor)[0],
            _ => self.anchor,
        }
    }
}

impl Render for CalendarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let bordered = |id: &'static str| {
            // The original's header buttons: 30 px boxes with text at the body size.
            h_flex()
                .id(id)
                .h(px(30.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border)
                .bg(theme.colors().surface_background)
        };

        let arrange = self.arrange.map(|tab| self.render_arrange(tab, cx));
        let shown_month = self.toolbar_month();
        let title_text = match self.mode {
            // The original's Year shows just the year.
            CalendarMode::Year => self.month.year().to_string(),
            _ => calendar::month_title(shown_month, strings),
        };
        let can_pick_month = self.mode != CalendarMode::Year;
        let toolbar = h_flex()
            .debug_selector(|| "calendar-toolbar".to_owned())
            .mt(crate::title_bar::header_inset())
            .h(px(CALENDAR_HEADER_HEIGHT))
            .flex_none()
            .pl(px(16.))
            .pr(px(12.))
            .gap_2()
            .child(IconButton::new("calendar-toggle-sidebar", IconName::PanelLeft)
                .toggle_state(self.sidebar_open)
                .tip(strings.mini_calendar, ui::TipSide::Below)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_calendar_sidebar(cx))))
            .child(
                div().flex_1().min_w_0().flex().items_center().child(
                    div()
                        .relative()
                        .max_w_full()
                        .child(
                            h_flex()
                                .id("calendar-month-title")
                                .debug_selector(|| "calendar-month-title".to_owned())
                                .group("calendar-month-title")
                                .h(px(30.))
                                .px_1p5()
                                .gap_1()
                                .rounded(px(6.))
                                .when(can_pick_month, |el| {
                                    el.cursor_pointer()
                                        .hover(|s| s.bg(theme.colors().ghost_element_hover))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if !this.finish_task_before_navigation(cx) {
                                                return;
                                            }
                                            this.month_picker_open = if this.month_picker_open.is_some() {
                                                None
                                            } else {
                                                Some(shown_month.year())
                                            };
                                            window.focus(&this.focus_handle, cx);
                                            cx.notify();
                                        }))
                                })
                                .child(
                                    div().min_w_0().child(
                                        Label::new(title_text)
                                            .size(LabelSize::Large)
                                            .weight(FontWeight::SEMIBOLD)
                                            .truncate(),
                                    ),
                                )
                                .when(can_pick_month, |el| {
                                    el.child(
                                        div()
                                            .opacity(if self.month_picker_open.is_some() { 1. } else { 0. })
                                            .group_hover("calendar-month-title", |s| s.opacity(1.))
                                            .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted)),
                                    )
                                }),
                        )
                        .when_some(self.month_picker_open, |el, picker_year| {
                            el.child(PopoverAttachment::new(
                                Placement::BelowLeft,
                                self.render_month_picker_popover(picker_year, shown_month, today, cx),
                            ))
                        }),
                ),
            )
            .when(self.mode == CalendarMode::Week && calendar::week_numbers(), |el| {
                el.child(
                    Label::new(format!(
                        "{}{}{}",
                        strings.horizons_week_prefix,
                        calendar::week_number(&calendar::week_of(self.anchor)),
                        strings.horizons_week_suffix
                    ))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
            .child(
                bordered("calendar-add-frame").relative().debug_selector(|| "calendar-add-frame".to_owned()).child(
                    IconButton::new("calendar-add", IconName::Plus)
                        .size(px(28.))
                        .icon_size(IconSize::Small)
                        .when(self.task_editor.is_none(), |button| {
                            button.tip(strings.add_task, ui::TipSide::Below)
                        })
                        // An open editor handles this as an outside click. Do not also
                        // register a click that would reopen it on mouse-up.
                        .when(self.task_editor.is_none(), |button| {
                            button.on_click(cx.listener(|this, _, window, cx| this.open_task_editor(window, cx)))
                        }),
                ).when_some(self.task_editor.as_ref().filter(|editor| editor.day.is_none()), |el, editor| {
                    el.child(PopoverAttachment::new(Placement::BelowRight, editor.view.clone()))
                }),
            )
            .when(
                crate::subscriptions::feeds(cx).is_some_and(|f| !f.read(cx).writable_calendars().is_empty()),
                |el| {
                    el.child(
                        bordered("calendar-add-event-frame").child(
                            IconButton::new("calendar-add-event", IconName::CalendarPlus)
                                .size(px(28.))
                                .icon_size(IconSize::Small)
                                .tip(strings.cal_new_event, ui::TipSide::Below)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_new_event_editor(this.selected_day.unwrap_or(today), window, cx);
                                })),
                        ),
                    )
                },
            )
            .child(
                div()
                    .relative()
                    .child(
                        bordered("calendar-mode")
                            .debug_selector(|| "calendar-mode-button".to_owned())
                            .px_3()
                            .gap_1()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.colors().ghost_element_hover))
                            .child(
                                Label::new(match self.mode {
                                    CalendarMode::Week => strings.week,
                                    CalendarMode::Day => strings.day,
                                    CalendarMode::MultiDay(_) => strings.multi_day,
                                    CalendarMode::Month => strings.month,
                                    CalendarMode::Agenda => strings.agenda,
                                    CalendarMode::Year => strings.year,
                                    CalendarMode::MultiWeek(_) => strings.multi_week,
                                }),
                            )
                            .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                            .on_click(cx.listener(|this, _, window, cx| {
                                if !this.finish_task_before_navigation(cx) {
                                    return;
                                }
                                this.mode_menu_open = true;
                                window.focus(&this.focus_handle, cx);
                                cx.notify();
                            })),
                    )
                    .when(self.mode_menu_open, |el| {
                        el.child(PopoverAttachment::new(Placement::BelowRight, self.render_mode_menu(cx)))
                    }),
            )
            .child(
                bordered("calendar-nav")
                    .child(
                        div().debug_selector(|| "calendar-prev".to_owned()).child(
                            IconButton::new("calendar-prev", IconName::ChevronLeft)
                                .size(px(28.))
                                .icon_size(IconSize::Small)
                                .when(!self.can_shift(-1), |el| el.icon_color(Color::Placeholder))
                                .tooltip(ui::Tooltip::text("←"))
                                .on_click(cx.listener(|this, _, _, cx| this.shift(-1, cx))),
                        ),
                    )
                    .child(
                        div()
                            .id("calendar-today")
                            .h_full()
                            .px_3()
                            .flex()
                            .items_center()
                            .border_x_1()
                            .border_color(theme.colors().border)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.colors().ghost_element_hover))
                            .tooltip(ui::Tooltip::text(format!("{} (T)", strings.today)))
                            .child(Label::new(strings.today))
                            .on_click(cx.listener(|this, _, _, cx| this.go_to_today(cx))),
                    )
                    .child(
                        div().debug_selector(|| "calendar-next".to_owned()).child(
                            IconButton::new("calendar-next", IconName::ChevronRight)
                                .size(px(28.))
                                .icon_size(IconSize::Small)
                                .when(!self.can_shift(1), |el| el.icon_color(Color::Placeholder))
                                .tooltip(ui::Tooltip::text("→"))
                                .on_click(cx.listener(|this, _, _, cx| this.shift(1, cx))),
                        ),
                    ),
            )
            .child(
                div()
                    .relative()
                    .debug_selector(|| "calendar-more-button".to_owned())
                    .child(IconButton::new("calendar-more", IconName::Ellipsis).when(!self.more_menu, |b| b.tip(i18n::t(cx).more, ui::TipSide::Below)).on_click(cx.listener(|this, _, window, cx| {
                        if !this.finish_task_before_navigation(cx) {
                            return;
                        }
                        this.more_menu = !this.more_menu;
                        window.focus(&this.focus_handle, cx);
                        cx.notify();
                    })))
                    .when(self.more_menu, |el| el.child(PopoverAttachment::new(Placement::BelowRight, self.render_more_menu(cx))))
                    .when_some(self.render_year_jump(cx), |el, jump| el.child(PopoverAttachment::new(Placement::BelowRight, jump))),
            );
        let secondary_tz_active = self
            .options
            .secondary_timezone
            .as_deref()
            .is_some_and(calendar::is_valid_timezone);
        let horizon_banner = match self.mode {
            CalendarMode::Week => {
                let target = self.displayed_week_horizon();
                let goals = self.horizon_goals_for(target, today, true);
                self.render_horizon_goals_banner(
                    "calendar-week-goals-banner",
                    strings.calendar_week_goals,
                    calendar::format_horizon_badge(target, strings),
                    goals,
                    cx,
                )
            }
            CalendarMode::Month => {
                let target = fern_core::Horizon::Month {
                    year: self.month.year(),
                    month: self.month.month(),
                };
                let goals = self.horizon_goals_for(target, today, true);
                self.render_horizon_goals_banner(
                    "calendar-month-goals-banner",
                    strings.calendar_month_goals,
                    strings.months[self.month.month0() as usize].to_owned(),
                    goals,
                    cx,
                )
            }
            _ => None,
        };
        let has_month_banner = self.mode == CalendarMode::Month && horizon_banner.is_some();
        let calendar = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .children(horizon_banner)
            .when(!matches!(self.mode, CalendarMode::Agenda | CalendarMode::Year), |el| el.child(
                h_flex()
                    .flex_none()
                    .h(px(WEEKDAY_HEADER_HEIGHT))
                    .when(self.mode.is_timed(), |el| {
                        el.child(div().w(if secondary_tz_active { px(108.) } else { px(56.) }).flex_none())
                    })
                    .when(matches!(self.mode, CalendarMode::Month | CalendarMode::MultiWeek(_)) && calendar::week_numbers(), |el| {
                        el.child(div().w(px(WEEK_NUMBER_WIDTH)).flex_none())
                    })
                    // The days' weekday names: the week's, or those of the shown days.
                    .children(match self.mode {
                        CalendarMode::Month | CalendarMode::Week | CalendarMode::MultiWeek(_) => {
                            let shown = calendar::week_of(self.anchor).map(|d| self.options.shows_day(d));
                            crate::calendar::weekday_labels(&strings.weekdays_short).into_iter().zip(shown).filter(|(_, on)| *on).map(|(l, _)| l).collect::<Vec<_>>()
                        }
                        _ => self
                            .shown_days()
                            .iter()
                            .map(|d| strings.weekdays_short[d.weekday().num_days_from_monday() as usize])
                            .collect(),
                    }
                    .into_iter()
                    .map(|day| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .justify_center()
                            .child(Label::new(day).size(LabelSize::Small).color(Color::Muted))
                    })),
            ))
            .map(|el| match self.mode {
                CalendarMode::Agenda => el.child(self.render_agenda(today, cx)),
                CalendarMode::Year => el.child(self.render_year(today, window, cx)),
                CalendarMode::Month | CalendarMode::MultiWeek(_) => el.child(self.render_month_grid(today, has_month_banner, window, cx)),
                _ => el.child(self.render_week(today, cx)),
            });
        h_flex()
            .relative()
            .track_focus(&self.focus_handle)
            .key_context(match (self.task_editor.is_some(), self.mode) {
                (true, _) => "CalendarTaskEditor",
                (_, _) if self.has_modal() => "CalendarModal",
                (_, _) if self.year_jump.is_some() => "CalendarDateInput",
                (false, CalendarMode::Year) => "Calendar CalendarYear",
                _ => "Calendar",
            })
            .on_action(cx.listener(|this, _: &NewTask, window, cx| {
                if this.has_modal() {
                    return;
                }
                this.open_task_editor(window, cx);
            }))
            // The view menu's keys, as in the original: D/1, W/2, M/3.
            .on_action(cx.listener(|this, _: &DayView, _, cx| {
                if this.has_modal() {
                    return;
                }
                this.set_mode(CalendarMode::Day, cx);
            }))
            .on_action(cx.listener(|this, _: &WeekView, _, cx| {
                if this.has_modal() {
                    return;
                }
                this.set_mode(CalendarMode::Week, cx);
            }))
            .on_action(cx.listener(|this, _: &MonthView, _, cx| {
                if this.has_modal() {
                    return;
                }
                this.set_mode(CalendarMode::Month, cx);
            }))
            .on_action(cx.listener(|this, _: &AgendaView, _, cx| {
                if this.has_modal() {
                    return;
                }
                this.set_mode(CalendarMode::Agenda, cx);
            }))
            .on_action(cx.listener(|this, _: &YearView, _, cx| {
                if this.has_modal() {
                    return;
                }
                this.set_mode(CalendarMode::Year, cx);
            }))
            .on_action(cx.listener(|this, _: &MultiDayMode, _, cx| {
                if this.has_modal() {
                    return;
                }
                this.set_mode(CalendarMode::MultiDay(this.multi_days), cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousPeriod, _, cx| {
                if this.has_modal() {
                    return;
                }
                this.shift(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &NextPeriod, _, cx| {
                if this.has_modal() {
                    return;
                }
                this.shift(1, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousYear, _, cx| {
                if this.has_modal() {
                    return;
                }
                this.shift_year_using_keys(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &NextYear, _, cx| {
                if this.has_modal() {
                    return;
                }
                this.shift_year_using_keys(1, cx);
            }))
            .on_action(cx.listener(|this, _: &CalendarToday, _, cx| {
                if this.has_modal() {
                    return;
                }
                this.go_to_today(cx);
            }))
            .on_action(cx.listener(|this, _: &PrintYear, _, cx| {
                if this.has_modal() {
                    return;
                }
                this.print_year(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::PrintList, _, cx| {
                if this.has_modal() {
                    return;
                }
                if this.mode == CalendarMode::Year {
                    this.print_year(cx);
                }
            }))
            .on_action(cx.listener(Self::dismiss_transient))
            .when(self.timed_resize.is_some(), |el| {
                el.on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, _, cx| this.update_timed_resize(ev.position.y, cx)))
                    .on_mouse_up(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| this.finish_timed_resize(cx)))
            })
            .size_full()
            .bg(theme.colors().calendar_background)
            .text_color(theme.colors().text)
            .when(self.sidebar_open, |el| el.child(self.render_calendar_sidebar(cx)))
            .child(
                v_flex().flex_1().min_w_0().h_full()
                    .child(toolbar)
                    .child(calendar),
            )
            .children(arrange)
            .children(self.saved_hint.as_ref().map(|(message, _)| {
                div().absolute().bottom(px(16.)).left_0().right_0().flex().justify_center().px_4()
                    .child(
                        div().debug_selector(|| "calendar-saved-hint".to_owned())
                            .px_4().py_3().rounded(px(8.))
                            .bg(theme.colors().elevated_surface_background)
                            .border_1().border_color(theme.colors().border).shadow_md()
                            .child(Label::new(message.clone()).size(LabelSize::Small)),
                    )
            }))
            .children(self.options_dialog.clone())
            .children(self.render_context_menu(cx))
            .children(self.render_event_editor_modal(cx))
            .children(self.render_delete_event_dialog(cx))
    }
}

/// A subscribed calendar's event as the calendar draws it.
#[derive(Clone)]
struct EventBar {
    key: String,
    subscription_id: Id,
    writable: bool,
    recurring: bool,
    title: String,
    start: NaiveDate,
    end: NaiveDate,
    start_time: Option<chrono::NaiveTime>,
    end_time: Option<chrono::NaiveTime>,
    location: String,
    notes: String,
    calendar: String,
    color: gpui::Hsla,
}

#[cfg(all(test, debug_assertions))]
mod tests;
