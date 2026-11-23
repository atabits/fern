//! The calendar: tasks placed on their due dates in a month grid or a week with a time axis,
//! matched to TickTick's Calendar section.

use std::{collections::{BTreeMap, HashSet}, rc::Rc, time::Duration};

use chrono::{Datelike, Local, NaiveDate, NaiveTime};
use fern_core::{CalendarOptions, ColorBy, Id, Store, Task};
use gpui::{EventEmitter, Focusable, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::actions::{AgendaView, CalendarToday, DayView, DismissTransient, MonthView, NewTask, NextPeriod, NextYear, PreviousPeriod, PreviousYear, PrintYear, WeekView, YearView};
use crate::calendar::{self, week_axis};
use crate::drag::{DraggedEvent, DraggedTask, ResizeEdge, ResizedTask};
use crate::i18n;
use crate::calendar_options_view::{CalendarOptionsDialog, OptionsEvent, Page};
use crate::task_detail::{StartFocus, TaskDetail, TaskUpdated};
use crate::task_rows;

#[path = "calendar_task_editor.rs"]
mod calendar_task_editor;
use calendar_task_editor::{CalendarTaskEditor, EditorEvent, month_picker};

#[path = "calendar_year.rs"]
mod calendar_year;

mod agenda;
mod arrange;
mod event_editor;
mod week_grid;

use event_editor::EventEditor;
#[cfg(test)]
use week_grid::{LanePlacement, TimedInterval, assign_timed_lanes};
use week_grid::{TimedResize, task_timed_resize_edges};

/// How many tasks a day cell shows before collapsing the rest into "+N".
const TASKS_PER_DAY: usize = 5;
const CALENDAR_HEADER_HEIGHT: f32 = 50.;
const CALENDAR_SIDEBAR_WIDTH: f32 = 216.;
const WEEKDAY_HEADER_HEIGHT: f32 = 22.;
const HORIZON_BANNER_HEIGHT: f32 = 38.;
const DAY_HEADER_HEIGHT: f32 = 26.;
const DAY_ITEM_HEIGHT: f32 = 18.;
const DAY_ITEM_GAP: f32 = 2.;
const DAY_TOP_PADDING: f32 = 4.;

fn day_item_slots(height: Pixels) -> usize {
    let available = f32::from(height) - DAY_HEADER_HEIGHT - DAY_TOP_PADDING - 1.;
    (available / (DAY_ITEM_HEIGHT + DAY_ITEM_GAP)).floor().max(0.) as usize
}

/// The week-number column of the month grid.
const WEEK_NUMBER_WIDTH: f32 = 32.;

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

const MULTI_DAYS: std::ops::RangeInclusive<u8> = 1..=14;
const MULTI_WEEKS: std::ops::RangeInclusive<u8> = 2..=6;

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
    pending_complete: HashSet<Id>,
    active: bool,
    dirty: bool,
}

struct ContextMenuState {
    id: Id,
    position: gpui::Point<Pixels>,
    task: Task,
    nesting: crate::task_menu::TaskNesting,
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct CalendarFilter {
    all: bool,
    lists: HashSet<Option<Id>>,
    tags: HashSet<String>,
}

impl Default for CalendarFilter {
    fn default() -> Self {
        Self { all: true, lists: HashSet::new(), tags: HashSet::new() }
    }
}

impl CalendarFilter {
    fn single_list(&self) -> Option<Option<Id>> {
        if !self.all && self.tags.is_empty() && self.lists.len() == 1 {
            self.lists.iter().next().copied()
        } else {
            None
        }
    }

    fn matches(&self, task: &Task) -> bool {
        if self.all {
            return true;
        }
        (!self.lists.is_empty() && self.lists.contains(&task.list_id))
            || (!self.tags.is_empty() && task.tags.iter().any(|t| self.tags.contains(&t.to_lowercase())))
    }

    fn parse(raw: &str, valid_lists: &[(Id, String)], valid_tags: &[String]) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "all" {
            return Self::default();
        }
        if trimmed == "inbox" {
            return Self { all: false, lists: HashSet::from([None]), tags: HashSet::new() };
        }
        if let Ok(id) = trimmed.parse::<Id>() {
            if valid_lists.iter().any(|(list, _)| *list == id) {
                return Self { all: false, lists: HashSet::from([Some(id)]), tags: HashSet::new() };
            }
            return Self::default();
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            return Self::default();
        };
        let mut lists = HashSet::new();
        if let Some(arr) = value.get("lists").and_then(|v| v.as_array()) {
            for item in arr.iter().filter_map(|v| v.as_str()) {
                if item == "inbox" {
                    lists.insert(None);
                } else if let Ok(id) = item.parse::<Id>()
                    && valid_lists.iter().any(|(list, _)| *list == id)
                {
                    lists.insert(Some(id));
                }
            }
        }
        let mut tags = HashSet::new();
        if let Some(arr) = value.get("tags").and_then(|v| v.as_array()) {
            for item in arr.iter().filter_map(|v| v.as_str()) {
                let lower = item.to_lowercase();
                if valid_tags.iter().any(|t| t.to_lowercase() == lower) {
                    tags.insert(lower);
                }
            }
        }
        if lists.is_empty() && tags.is_empty() {
            Self::default()
        } else {
            Self { all: false, lists, tags }
        }
    }

    fn encode(&self) -> String {
        if self.all || (self.lists.is_empty() && self.tags.is_empty()) {
            return String::new();
        }
        if let Some(single) = self.single_list() {
            return match single {
                None => "inbox".to_owned(),
                Some(id) => id.to_string(),
            };
        }
        let lists: Vec<String> = self
            .lists
            .iter()
            .map(|l| match l {
                None => "inbox".to_owned(),
                Some(id) => id.to_string(),
            })
            .collect();
        let tags: Vec<&str> = self.tags.iter().map(String::as_str).collect();
        serde_json::json!({ "lists": lists, "tags": tags }).to_string()
    }
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
            crate::data_version::data_changed(cx.entity_id(), cx);
            this.card_updated(cx);
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
            pending_complete: HashSet::new(),
            active: true,
            dirty: false,
        };
        this.reload(cx);
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

    /// An event's bar: the calendar's color, its time when it has one, and a card with the rest
    /// on a click. Writable CalDAV events can also be dragged to another day.
    fn event_bar(&self, event: EventBar, date: NaiveDate, cx: &mut Context<Self>) -> Stateful<ui::HorizontalStack> {
        let strings = i18n::t(cx);
        let open = self.open_event.as_ref() == Some(&(event.key.clone(), event.start));
        let key = (event.key.clone(), event.start);
        let column = calendar::week_of(date).iter().position(|d| *d == date).unwrap_or(0);
        let card = event.clone();
        let draggable = event.writable && !event.recurring;
        let dragged = DraggedEvent {
            subscription_id: event.subscription_id,
            uid: event.key.clone(),
            title: event.title.clone().into(),
        };
        h_flex()
            .id(SharedString::from(format!("calendar-event-{}-{}-{date}", event.key, event.start)))
            .when(draggable, |el| {
                el.on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
            })
            .h(px(18.))
            .flex_none()
            .gap_1()
            .px_1()
            .rounded(px(3.))
            .cursor_pointer()
            .bg(event.color.opacity(0.18))
            .border_l_2()
            .border_color(event.color)
            .when_some(event.start_time.filter(|_| event.start == date), |el, time| {
                el.child(Label::new(calendar::format_time(time, strings)).size(LabelSize::XSmall).color(Color::Muted))
            })
            .child(div().flex_1().min_w_0().child(Label::new(event.title).size(LabelSize::XSmall).truncate()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_event = if this.open_event.as_ref() == Some(&key) { None } else { Some(key.clone()) };
                window.focus(&this.focus_handle, cx);
                cx.notify();
            }))
            .when(open, |el| {
                let placement = if column < 3 { Placement::RightOf } else { Placement::LeftOf };
                el.relative().child(PopoverAttachment::new(placement, self.render_event_card(&card, cx)))
            })
    }

    /// An event's card: its title, when it is, its calendar, place and description.
    fn render_event_card(&self, event: &EventBar, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let day = |date: NaiveDate| task_rows::format_day(strings.closed_date[0], date, strings);
        let when = match (event.start_time, event.end_time) {
            (Some(from), Some(to)) if event.start == event.end => format!("{}, {} – {}", day(event.start), calendar::format_time(from, strings), calendar::format_time(to, strings)),
            (Some(from), Some(to)) => format!("{} {} – {} {}", day(event.start), calendar::format_time(from, strings), day(event.end), calendar::format_time(to, strings)),
            (Some(from), None) => format!("{}, {}", day(event.start), calendar::format_time(from, strings)),
            (None, _) if event.start == event.end => format!("{}, {}", day(event.start), strings.range_all_day),
            (None, _) => format!("{} – {}", day(event.start), day(event.end)),
        };
        let sub_id = event.subscription_id;
        let uid = event.key.clone();
        let edit_snapshot = event.clone();
        popover(cx)
            .w(px(300.))
            .p_3()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.open_event = None;
                cx.notify();
            }))
            .child(Label::new(event.title.clone()).weight(FontWeight::SEMIBOLD))
            .child(Label::new(when).size(LabelSize::Small).color(Color::Muted))
            .child(
                h_flex()
                    .gap_1p5()
                    .child(div().size(px(8.)).rounded_full().bg(event.color))
                    .child(Label::new(event.calendar.clone()).size(LabelSize::Small).color(Color::Muted)),
            )
            .when(!event.location.trim().is_empty(), |el| {
                el.child(
                    h_flex()
                        .gap_1p5()
                        .child(Icon::new(IconName::MapPin).size(IconSize::XSmall).color(Color::Muted))
                        .child(Label::new(event.location.clone()).size(LabelSize::Small)),
                )
            })
            .when(!event.notes.trim().is_empty(), |el| {
                el.child(
                    v_scroll(
                        "calendar-event-notes-scroll",
                        div()
                            .id("calendar-event-notes")
                            .text_size(px(12.))
                            .child(event.notes.trim().to_owned()),
                    )
                    .max_h(px(160.))
                    .compact(),
                )
            })
            .when(event.recurring, |el| {
                el.child(
                    h_flex()
                        .gap_1p5()
                        .child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Muted))
                        .child(Label::new(strings.filter_date_recurring).size(LabelSize::Small).color(Color::Muted)),
                )
            })
            .when(event.writable, |el| {
                el.child(div().h(px(1.)).bg(theme.colors().border_variant)).child(
                    h_flex()
                        .justify_end()
                        .gap_1p5()
                        .when(!event.recurring, |el| {
                            el.child(
                                IconButton::new("calendar-event-edit", IconName::Pencil)
                                    .tip(strings.cal_edit_event, ui::TipSide::Above)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_edit_event_editor(&edit_snapshot, window, cx);
                                    })),
                            )
                        })
                        .child(
                            IconButton::new("calendar-event-delete", IconName::Trash)
                                .icon_color(Color::Error)
                                .tip(strings.delete, ui::TipSide::Above)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.delete_caldav_event(sub_id, &uid, cx);
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_habit_popover(
        &self,
        habit: &fern_core::Habit,
        date: NaiveDate,
        cx: &mut Context<Self>,
    ) -> Div {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = habit.id;
        let rec = self
            .habit_records
            .get(&id)
            .and_then(|recs| recs.iter().find(|r| r.date == date));
        let done = rec.is_some_and(|r| r.is_done());
        let value = rec.map_or(0.0, |r| r.value);
        let goal_amt = habit.goal_amount();
        let unit = habit.goal.as_ref().map(|g| g.unit.clone()).unwrap_or_default();
        let pct = (value / goal_amt).clamp(0.0, 1.0) as f32;
        let has_goal = habit.goal.is_some();
        let step = habit.goal.as_ref().map_or(1.0, |g| g.step_amount());

        popover(cx)
            .w(px(236.))
            .p_3()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.open_habit = None;
                cx.notify();
            }))
            .child(
                h_flex()
                    .id(SharedString::from(format!("cal-habit-title-{id}")))
                    .gap_2()
                    .items_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_habit = None;
                        cx.emit(OpenHabit(id));
                        cx.notify();
                    }))
                    .child(desktop_common::habit_helpers::avatar(habit, px(24.), cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Label::new(habit.name.clone()).weight(FontWeight::SEMIBOLD).truncate()),
                    )
                    .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(Color::Muted)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(
                            format!(
                                "{} / {} {}",
                                fern_core::habit::format_amount(value),
                                fern_core::habit::format_amount(goal_amt),
                                unit
                            )
                            .trim()
                            .to_owned(),
                        )
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                    )
                    .child(
                        div()
                            .h(px(5.))
                            .w_full()
                            .rounded_full()
                            .bg(theme.colors().border.opacity(0.5))
                            .child(div().h_full().w(gpui::relative(pct)).rounded_full().bg(theme.colors().accent)),
                    ),
            )
            .when(!habit.archived, |el| {
                let btn_label = if done {
                    strings.habit_reset.to_owned()
                } else if has_goal {
                    format!("+{}", fern_core::habit::format_amount(step))
                } else {
                    strings.habit_i_did_it.to_owned()
                };
                el.child(
                    h_flex()
                        .gap_2()
                        .pt_1()
                        .child(
                            Button::new(SharedString::from(format!("cal-habit-act-{id}-{date}")), btn_label)
                                .variant(if done { ButtonVariant::Secondary } else { ButtonVariant::Primary })
                                .size(ButtonSize::Sm)
                                .full_width()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let res = if done {
                                        this.store.reset_checkin(id, date, true)
                                    } else if has_goal {
                                        this.store.step_checkin(id, date).map(|_| ())
                                    } else {
                                        this.store.toggle_checkin(id, date).map(|_| ())
                                    };
                                    if let Err(err) = res {
                                        eprintln!("failed to update habit from calendar: {err:#}");
                                    }
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload(cx);
                                })),
                        ),
                )
            })
    }

    fn day_extras(&self, date: NaiveDate, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let week_start = calendar::week_start();
        let bar = |tint: gpui::Hsla| h_flex().h(px(18.)).flex_none().gap_1().px_1().rounded(px(3.)).bg(tint.opacity(0.28));
        let mut items: Vec<AnyElement> = Vec::new();
        if self.options.show_habits && date <= today {
            for habit in &self.habits {
                let recs = self.habit_records.get(&habit.id).map(Vec::as_slice).unwrap_or(&[]);
                let day_rec = recs.iter().find(|r| r.date == date);
                let show = if date == today {
                    !habit.archived && habit.is_active_on(today, recs, week_start)
                } else {
                    day_rec.is_some_and(|r| r.is_handled() || r.is_partial())
                };
                if !show {
                    continue;
                }
                let done = day_rec.is_some_and(|r| r.is_done());
                let tint = match self.options.habit_color {
                    fern_core::HabitColor::Theme => theme.colors().accent,
                    fern_core::HabitColor::Fixed => crate::calendar_options_view::palette(self.options.habit_fixed),
                    fern_core::HabitColor::Habit => {
                        crate::lists_panel::parse_color(habit.color.as_deref()).unwrap_or(theme.colors().accent)
                    }
                };
                let id = habit.id;
                let key = (id, date);
                let open = self.open_habit == Some(key);
                let time_badge = (date == today)
                    .then(|| habit.reminders.first().copied())
                    .flatten()
                    .map(|t| calendar::format_time(t, strings));
                items.push(
                    div()
                        .relative()
                        .child(
                            bar(tint)
                                .id(SharedString::from(format!("cal-habit-{id}-{date}")))
                                .cursor_pointer()
                                .when(done, |el| el.opacity(0.65))
                                .child(
                                    Icon::new(if done { IconName::Check } else { IconName::FernHabit })
                                        .size(IconSize::XSmall)
                                        .color(if done { Color::Accent } else { Color::Muted }),
                                )
                                .child(div().flex_1().min_w_0().child(Label::new(habit.name.clone()).size(LabelSize::XSmall).truncate()))
                                .when_some(time_badge, |el, t| {
                                    el.child(Label::new(t).size(LabelSize::XSmall).color(Color::Muted))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_event = None;
                                    this.open_habit = if this.open_habit == Some(key) { None } else { Some(key) };
                                    cx.notify();
                                })),
                        )
                        .when(open, |el| {
                            el.child(PopoverAttachment::new(
                                Placement::BelowLeft,
                                self.render_habit_popover(habit, date, cx),
                            ))
                        })
                        .into_any_element(),
                );
            }
        }
        // A focus record: an outlined bar with a timer, the task and the time it started.
        for session in self.options.focus_on(&self.focus_sessions, date) {
            let start = chrono::DateTime::from_timestamp_millis(session.started_at).map(|t| t.with_timezone(&Local).time());
            items.push(
                h_flex()
                    .h(px(18.))
                    .flex_none()
                    .gap_1()
                    .px_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme.colors().accent.opacity(0.4))
                    .child(Icon::new(IconName::Timer).size(IconSize::XSmall).color(Color::Muted))
                    .child(div().flex_1().min_w_0().child(Label::new(session.title.clone()).size(LabelSize::XSmall).color(Color::Muted).truncate()))
                    .when_some(start, |el, start| el.child(Label::new(calendar::format_time(start, strings)).size(LabelSize::XSmall).color(Color::Muted)))
                    .into_any_element(),
            );
        }
        for countdown in self.countdowns.iter().filter(|c| !c.archived && self.options.shows_countdown(c, date)) {
            let tint = match self.options.countdown_color {
                fern_core::CountdownColor::Fixed => crate::calendar_options_view::palette(self.options.countdown_fixed),
                fern_core::CountdownColor::Countdown => crate::lists_panel::parse_color(countdown.color.as_deref()).unwrap_or(theme.status().warning),
            };
            items.push(
                bar(tint)
                    .child(div().size(px(6.)).flex_none().rounded_full().bg(tint))
                    .child(Label::new(countdown.name.clone()).size(LabelSize::XSmall).truncate())
                    .into_any_element(),
            );
        }
        items
    }

    fn open_options(&mut self, page: Page, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let dialog = cx.new(|cx| CalendarOptionsDialog::new(store, page, cx));
        cx.subscribe(&dialog, |this, _, event: &OptionsEvent, cx| {
            match event {
                OptionsEvent::Changed(options) => this.options = options.clone(),
                OptionsEvent::Closed => this.options_dialog = None,
            }
            this.reload(cx);
        })
        .detach();
        self.options_dialog = Some(dialog);
        self.more_menu = false;
        cx.notify();
    }

    /// The header's "…" menu, as the original's: View Options and Arrange Tasks.
    fn render_more_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let row = |id: &'static str, icon: IconName, label: &'static str| {
            ListItem::new(id).height(px(34.)).start_slot(Icon::new(icon).color(Color::Muted)).child(Label::new(label))
        };
        popover(cx)
            .min_w(px(184.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.more_menu = false;
                cx.notify();
            }))
            .child(row("calendar-view-options", IconName::Settings2, strings.view_options).on_click(cx.listener(|this, _, _, cx| this.open_options(Page::Main, cx))))
            .child(row("calendar-arrange", IconName::ListChecks, strings.arrange_tasks).on_click(cx.listener(|this, _, _, cx| {
                this.more_menu = false;
                this.arrange = Some(this.arrange.unwrap_or(0));
                this.reload(cx);
            })))
            .when(self.mode == CalendarMode::Year, |el| {
                el.child(row("calendar-go-to-date", IconName::CalendarDays, strings.calendar_go_to_date)
                    .on_click(cx.listener(|this, _, window, cx| this.open_year_jump(window, cx))))
                    .child(row("calendar-print-year", IconName::Printer, strings.print)
                        .on_click(cx.listener(|this, _, _, cx| this.print_year(cx))))
            })
            .into_any_element()
    }

    /// Opens a task's card beside it, the task as clicked on `day`.
    fn open_card(&mut self, id: Id, day: NaiveDate, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let Ok(Some(task)) = self.store.task(id) else { return };
        let lists = self.lists.clone();
        self.detail.update(cx, |detail, cx| {
            detail.set_lists(lists);
            detail.show(Some(task), cx);
        });
        self.card = Some((id, day));
        cx.notify();
    }

    fn close_card(&mut self, cx: &mut Context<Self>) {
        if self.card.take().is_some() {
            self.detail.update(cx, |detail, cx| detail.show(None, cx));
            cx.notify();
        }
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
        } else if self.event_editor.take().is_some() {
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

    /// The card a clicked task opens: its details, framed, closed by a click elsewhere or Escape.
    fn render_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        div()
            .id("calendar-task-card")
            .occlude()
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .rounded(px(10.))
            .overflow_hidden()
            .border_1()
            .border_color(theme.colors().border)
            .bg(theme.colors().background)
            .shadow_lg()
            // The details' own popovers lie outside the card; a click in them keeps it open.
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if !this.detail.read(cx).has_popup() {
                    this.close_card(cx);
                }
            }))
            .child(self.detail.clone())
            .into_any_element()
    }

    /// Opens a task's right-click menu, snapshotting task & nesting state outside `render()`.
    fn open_context(&mut self, id: Id, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) || self.is_task_read_only(id) {
            return;
        }
        let Some(task) = self.store.task(id).ok().flatten() else {
            return;
        };
        let nesting = crate::task_menu::TaskNesting::of(&self.store, task.id);
        self.context_menu = Some(ContextMenuState { id, position, task, nesting });
        self.context_side = None;
        cx.notify();
    }

    /// A choice in a task's right-click menu (see `task_menu`): edits apply here; the pickers
    /// open the task in the list.
    fn context_action(&mut self, action: crate::task_menu::MenuAction, cx: &mut Context<Self>) {
        use crate::task_menu::MenuAction;
        let Some(id) = self.context_menu.as_ref().map(|menu| menu.id) else { return };
        if let MenuAction::Side(side) = action {
            self.context_side = side;
            cx.notify();
            return;
        }
        self.context_menu = None;
        self.context_side = None;
        if self.is_task_read_only(id) {
            cx.notify();
            return;
        }
        let Some(task) = self.store.task(id).ok().flatten() else {
            cx.notify();
            return;
        };
        if let Some(result) = crate::task_menu::apply(&self.store, &task, action, Local::now().date_naive()) {
            if let Err(err) = result {
                eprintln!("failed to change the task: {err:#}");
            }
            crate::data_version::data_changed(cx.entity_id(), cx);
            self.card_updated(cx);
            return;
        }
        match action {
            MenuAction::Due(None) => cx.emit(OpenTaskPicker(id, "due")),
            MenuAction::LinkParent => cx.emit(OpenTaskPicker(id, "link-parent")),
            MenuAction::Tags => cx.emit(OpenTaskPicker(id, "tags")),
            MenuAction::AddSubtask => cx.emit(OpenTaskPicker(id, "subtask")),
            MenuAction::Focus(stopwatch) => cx.emit(crate::task_detail::StartFocus(id, task.title, stopwatch)),
            MenuAction::CopyLink => cx.write_to_clipboard(gpui::ClipboardItem::new_string(fern_core::link::task_link(id))),
            MenuAction::StickyNote => crate::sticky_opener::open_sticky_note(self.store.clone(), id, cx),
            MenuAction::Delete => {
                if let Err(err) = self.store.delete_task(id) {
                    eprintln!("failed to delete the task: {err:#}");
                }
                crate::data_version::data_changed(cx.entity_id(), cx);
                self.card_updated(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// The task's right-click menu, as the original's calendar has it (no Add Subtask or Pin).
    fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.context_menu.as_ref()?;
        let menu = crate::task_menu::render(
            &state.task,
            &self.lists,
            crate::task_menu::MenuKind::Calendar,
            state.nesting,
            self.context_side,
            i18n::t(cx),
            cx,
            Rc::new(|this: &mut Self, action, _, cx| this.context_action(action, cx)),
        );
        Some(crate::task_menu::place(state.position, menu))
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

    fn month_grid_weeks(&self) -> Vec<[NaiveDate; 7]> {
        match self.mode {
            CalendarMode::MultiWeek(n) => calendar::weeks_from(self.anchor, n as usize),
            _ => calendar::month_weeks(self.month),
        }
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
            CalendarMode::Month | CalendarMode::Agenda => self.month = calendar::add_months(self.month, delta),
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
        self.mode = mode;
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

    fn delete_caldav_event(&mut self, subscription_id: Id, uid: &str, cx: &mut Context<Self>) {
        self.open_event = None;
        if let Some(feeds) = crate::subscriptions::feeds(cx) {
            feeds.update(cx, |feeds, cx| {
                if let Err(err) = feeds.delete_event(subscription_id, uid, cx) {
                    eprintln!("failed to delete CalDAV event: {err:#}");
                }
            });
        }
        cx.notify();
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

    fn toggle_calendar_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        if let Err(error) = self.store.set_setting("calendar_sidebar", if self.sidebar_open { "1" } else { "0" }) {
            eprintln!("failed to save the calendar sidebar setting: {error:#}");
        }
        cx.notify();
    }

    fn save_filter(&mut self, filter: CalendarFilter, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let encoded = filter.encode();
        if let Err(error) = self.store.set_setting("calendar_list_filter", &encoded) {
            eprintln!("failed to save the calendar filter: {error:#}");
            return;
        }
        self.close_card(cx);
        self.reload(cx);
    }

    #[cfg(test)]
    fn set_list_filter(&mut self, filter: Option<Option<Id>>, cx: &mut Context<Self>) {
        let next = match filter {
            None => CalendarFilter::default(),
            Some(list) => CalendarFilter { all: false, lists: HashSet::from([list]), tags: HashSet::new() },
        };
        self.save_filter(next, cx);
    }

    fn select_all_filter(&mut self, cx: &mut Context<Self>) {
        self.save_filter(CalendarFilter::default(), cx);
    }

    fn toggle_list_filter(&mut self, list: Option<Id>, cx: &mut Context<Self>) {
        let mut next = if self.filter.all {
            CalendarFilter { all: false, lists: HashSet::new(), tags: HashSet::new() }
        } else {
            self.filter.clone()
        };
        if !next.lists.remove(&list) {
            next.lists.insert(list);
        }
        if next.lists.is_empty() && next.tags.is_empty() {
            next.all = true;
        }
        self.save_filter(next, cx);
    }

    fn toggle_all_lists_filter(&mut self, cx: &mut Context<Self>) {
        let all_lists: HashSet<Option<Id>> = std::iter::once(None)
            .chain(self.list_names.iter().map(|(id, _)| Some(*id)))
            .collect();
        let mut next = if self.filter.all {
            CalendarFilter { all: false, lists: HashSet::new(), tags: HashSet::new() }
        } else {
            self.filter.clone()
        };
        if all_lists.iter().all(|l| next.lists.contains(l)) {
            next.lists.clear();
        } else {
            next.lists = all_lists;
        }
        if next.lists.is_empty() && next.tags.is_empty() {
            next.all = true;
        }
        self.save_filter(next, cx);
    }

    fn toggle_tag_filter(&mut self, tag: String, cx: &mut Context<Self>) {
        let key = tag.to_lowercase();
        let mut next = if self.filter.all {
            CalendarFilter { all: false, lists: HashSet::new(), tags: HashSet::new() }
        } else {
            self.filter.clone()
        };
        if !next.tags.remove(&key) {
            next.tags.insert(key);
        }
        if next.lists.is_empty() && next.tags.is_empty() {
            next.all = true;
        }
        self.save_filter(next, cx);
    }

    fn toggle_all_tags_filter(&mut self, cx: &mut Context<Self>) {
        let all_tags: HashSet<String> = self.tag_names.iter().map(|t| t.to_lowercase()).collect();
        let mut next = if self.filter.all {
            CalendarFilter { all: false, lists: HashSet::new(), tags: HashSet::new() }
        } else {
            self.filter.clone()
        };
        if !all_tags.is_empty() && all_tags.iter().all(|t| next.tags.contains(t)) {
            next.tags.clear();
        } else {
            next.tags = all_tags;
        }
        if next.lists.is_empty() && next.tags.is_empty() {
            next.all = true;
        }
        self.save_filter(next, cx);
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

    fn render_calendar_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let radio = |selected: bool| {
            div()
                .size(px(15.))
                .flex_none()
                .rounded_full()
                .border_1()
                .border_color(if selected { colors.accent } else { colors.checkbox_border })
                .flex()
                .items_center()
                .justify_center()
                .when(selected, |el| el.child(div().size(px(7.)).rounded_full().bg(colors.accent)))
        };
        let all_lists_checked = !self.filter.all
            && self.filter.lists.contains(&None)
            && self.list_names.iter().all(|(id, _)| self.filter.lists.contains(&Some(*id)));
        let all_tags_checked = !self.filter.all
            && !self.tag_names.is_empty()
            && self.tag_names.iter().all(|t| self.filter.tags.contains(&t.to_lowercase()));
        let check_row = |id: SharedString, label: String, bold: bool, icon: Option<(IconName, Hsla)>, checked: bool| {
            let box_id = SharedString::from(format!("{id}-box"));
            h_flex()
                .id(id.clone())
                .debug_selector(move || id.to_string())
                .h(px(32.))
                .px_2()
                .gap_2()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(colors.list_hover))
                .when_some(icon, |el, (name, color)| el.child(Icon::new(name).size(IconSize::Small).color(color)))
                .child(
                    div().flex_1().min_w_0().child(
                        Label::new(label)
                            .truncate()
                            .when(bold, |l| l.weight(FontWeight::SEMIBOLD))
                            .color(colors.list_text),
                    ),
                )
                .child(Checkbox::new(box_id).size(px(15.)).accent().toggle_state(checked))
        };
        v_scroll(
            "calendar-sidebar-scroll",
            v_flex()
                .id("calendar-sidebar")
                .debug_selector(|| "calendar-sidebar".to_owned())
                .pt(crate::title_bar::header_inset() + px(13.))
                .px_3()
                .pb_3()
                .gap_3()
            .child(month_picker(
                self.sidebar_month.unwrap_or(self.month),
                self.selected_day,
                &self.sidebar_busy_days,
                cx.listener(|this, delta: &i32, _, cx| {
                    this.sidebar_month = Some(calendar::add_months(this.sidebar_month.unwrap_or(this.month), *delta));
                    cx.notify();
                }),
                cx.listener(|this, _: &(), _, cx| {
                    this.sidebar_month = None;
                    this.go_to_today(cx);
                }),
                cx.listener(|this, day: &NaiveDate, _, cx| this.select_day(*day, cx)),
                cx,
            ))
            .child(Divider::horizontal())
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .id("calendar-filter-all")
                            .debug_selector(|| "calendar-filter-all".to_owned())
                            .h(px(32.))
                            .px_2()
                            .gap_2()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|s| s.bg(colors.list_hover))
                            .child(div().flex_1().min_w_0().child(Label::new(strings.all).weight(FontWeight::SEMIBOLD).color(colors.list_text)))
                            .child(radio(self.filter.all))
                            .on_click(cx.listener(|this, _, _, cx| this.select_all_filter(cx))),
                    )
                    .when(!self.tag_names.is_empty(), |el| {
                        el.child(
                            check_row("calendar-filter-tags".into(), strings.tags.to_owned(), true, None, all_tags_checked)
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_all_tags_filter(cx))),
                        )
                        .children(self.tag_names.iter().map(|tag| {
                            let lower = tag.to_lowercase();
                            let checked = !self.filter.all && self.filter.tags.contains(&lower);
                            let icon_color = self.tag_colors.get(&lower).copied().unwrap_or(colors.list_text_muted);
                            let tag_name = tag.clone();
                            check_row(
                                SharedString::from(format!("calendar-filter-tag-{tag}")),
                                tag.clone(),
                                false,
                                Some((IconName::Tag, icon_color)),
                                checked,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_tag_filter(tag_name.clone(), cx)))
                        }))
                    })
                    .child(
                        check_row("calendar-filter-lists".into(), strings.lists.to_owned(), true, None, all_lists_checked)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_all_lists_filter(cx))),
                    )
                    .child(
                        check_row(
                            "calendar-filter-inbox".into(),
                            strings.inbox.to_owned(),
                            false,
                            Some((IconName::Inbox, colors.list_text_muted)),
                            !self.filter.all && self.filter.lists.contains(&None),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_list_filter(None, cx))),
                    )
                    .children(self.list_names.iter().map(|(id, name)| {
                        let list_id = *id;
                        let checked = !self.filter.all && self.filter.lists.contains(&Some(list_id));
                        let icon_color = self.list_colors.get(&list_id).copied().unwrap_or(colors.list_text_muted);
                        check_row(
                            SharedString::from(format!("calendar-filter-list-{list_id}")),
                            name.clone(),
                            false,
                            Some((IconName::List, icon_color)),
                            checked,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_list_filter(Some(list_id), cx)))
                    })),
            ),
        )
        .flex_none()
        .w(px(CALENDAR_SIDEBAR_WIDTH))
        .h_full()
        .border_r_1()
        .border_color(colors.lists_border)
        .bg(colors.lists_background)
        .color(colors.list_text)
    }

    fn render_month_picker_popover(
        &self,
        picker_year: i32,
        shown_month: NaiveDate,
        today: NaiveDate,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();
        popover(cx)
            .debug_selector(|| "calendar-month-picker".to_owned())
            .w(px(236.))
            .p_3()
            .gap_3()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.month_picker_open = None;
                cx.notify();
            }))
            .child(
                h_flex()
                    .px_1()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex_1()
                            .child(Label::new(picker_year.to_string()).weight(FontWeight::SEMIBOLD)),
                    )
                    .child(
                        IconButton::new("month-picker-prev-year", IconName::ChevronLeft)
                            .size(px(24.))
                            .icon_size(IconSize::Small)
                            .tooltip(ui::Tooltip::text((picker_year - 1).max(1910).to_string()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(year) = this.month_picker_open.as_mut() {
                                    *year = (*year - 1).max(1910);
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        IconButton::new("month-picker-this-year", IconName::Circle)
                            .size(px(24.))
                            .icon_size(IconSize::Custom(px(8.)))
                            .tooltip(ui::Tooltip::text(today.year().to_string()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.month_picker_open = Some(today.year());
                                cx.notify();
                            })),
                    )
                    .child(
                        IconButton::new("month-picker-next-year", IconName::ChevronRight)
                            .size(px(24.))
                            .icon_size(IconSize::Small)
                            .tooltip(ui::Tooltip::text((picker_year + 1).min(2099).to_string()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(year) = this.month_picker_open.as_mut() {
                                    *year = (*year + 1).min(2099);
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .children((0..3).map(|row| {
                h_flex().gap_1().children((0..4).map(|col| {
                    let month_index = row * 4 + col;
                    let month_num = (month_index + 1) as u32;
                    let active = shown_month.year() == picker_year && shown_month.month() == month_num;
                    let is_current = today.year() == picker_year && today.month() == month_num;
                    div()
                        .flex_1()
                        .h(px(40.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .id(SharedString::from(format!("calendar-month-pick-{picker_year}-{month_num}")))
                                .debug_selector(move || format!("calendar-month-pick-{picker_year}-{month_num}"))
                                .size(px(38.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .when(active, |el| el.bg(colors.accent))
                                .when(!active, |el| el.hover(|s| s.bg(colors.element_hover)))
                                .child(
                                    Label::new(strings.months_short[month_index])
                                        .centered()
                                        .when(active || is_current, |l| l.weight(FontWeight::SEMIBOLD))
                                        .color(if active {
                                            Color::OnAccent
                                        } else if is_current {
                                            Color::Accent
                                        } else {
                                            Color::Default
                                        }),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_month(picker_year, month_num, cx);
                                })),
                        )
                }))
            }))
            .into_any_element()
    }

    fn render_day(&self, date: NaiveDate, today: NaiveDate, slots: usize, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let tasks = self.tasks_on(date);
        let extras = self.day_extras(date, cx);
        let events = self.events_on(date, cx);
        let total = tasks.len() + events.len() + extras.len();
        let visible = if total > slots.min(TASKS_PER_DAY) {
            slots.saturating_sub(1).min(TASKS_PER_DAY)
        } else {
            total
        };
        let hidden = total - visible;
        // The subscribed calendars' events after the tasks, then the habits and the rest.
        let event_room = visible.saturating_sub(tasks.len());
        let room = event_room.saturating_sub(events.len());
        // The first day of a month carries the month name, as in the original: "Oct 1".
        let number = if date.day() == 1 {
            task_rows::short_date(date, strings)
        } else {
            date.day().to_string()
        };

        let accent = theme.colors().accent;
        let editor = self.task_editor.as_ref().filter(|editor| editor.day == Some(date));
        v_flex()
            .id(SharedString::from(format!("calendar-day-{date}")))
            .debug_selector(move || format!("calendar-day-{date}"))
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.12)))
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on_day(dragged.id, date, cx)))
            .drag_over::<ResizedTask>(move |style, _, _, _| style.bg(accent.opacity(0.14)))
            .on_drop(cx.listener(move |this, resized: &ResizedTask, _, cx| this.resize_task_to_day(resized, date, cx)))
            .drag_over::<DraggedEvent>(move |style, _, _, _| style.bg(accent.opacity(0.12)))
            .on_drop(cx.listener(move |this, event: &DraggedEvent, _, cx| this.move_event_to_day(event, date, cx)))
            .gap(px(DAY_ITEM_GAP))
            .px(px(2.))
            .pt(px(DAY_TOP_PADDING))
            .border_r_1()
            .border_b_1()
            .border_color(theme.colors().border_variant)
            .when(editor.is_some(), |el| el.bg(theme.colors().element_selected))
            .on_click(cx.listener(move |this, _, window, cx| this.open_day_task_editor(date, window, cx)))
            .child(
                h_flex().h(px(DAY_HEADER_HEIGHT)).flex_none().px_1().child(
                    div()
                        .id(SharedString::from(format!("calendar-select-day-{date}")))
                        .debug_selector(|| format!("calendar-select-day-{date}"))
                        .min_w(px(22.))
                        .h(px(22.))
                        .px_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .when(date == today, |el| el.bg(theme.colors().accent))
                        .when(date != today && self.selected_day == Some(date), |el| {
                            el.bg(theme.colors().element_selected)
                        })
                        .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                            cx.stop_propagation();
                            if event.click_count() >= 2 {
                                this.anchor = date;
                                this.month = calendar::month_start(date);
                                this.selected_day = Some(date);
                                this.set_mode(CalendarMode::Day, cx);
                            } else {
                                this.select_day(date, cx);
                            }
                        }))
                        // As in the original, the days of the months around are muted.
                        .child(Label::new(number).centered().weight(FontWeight::SEMIBOLD).color(
                            if date == today {
                                Color::OnAccent
                            } else if self.mode == CalendarMode::Month && date.month() != self.month.month() {
                                Color::Muted
                            } else {
                                Color::Default
                            },
                        )),
                ),
            )
            .child(
                v_flex().id("calendar-day-items").flex_none().gap(px(DAY_ITEM_GAP))
                    // Items and their popovers have their own actions. Only empty calendar
                    // space creates a task, including for non-task items and the more row.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .children(tasks.iter().take(visible).map(|task| self.task_bar_with_resize(task, Some(date), true, cx)))
                    .children(events.into_iter().take(event_room).map(|event| self.event_bar(event, date, cx)))
                    .children(extras.into_iter().take(room))
                    .when(hidden > 0 && slots > 0, |el| {
                        el.child(
                            h_flex()
                                .id(SharedString::from(format!("calendar-more-{date}")))
                                .h(px(DAY_ITEM_HEIGHT))
                                .flex_none()
                                .px_1()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.colors().element_hover))
                                .child(Label::new(format!("+{hidden}")).size(LabelSize::XSmall).color(Color::Accent))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.anchor = date;
                                    this.month = calendar::month_start(date);
                                    this.selected_day = Some(date);
                                    this.set_mode(CalendarMode::Day, cx);
                                })),
                        )
                    }),
            )
            .when_some(editor, |el, editor| {
                let days: Vec<_> = calendar::week_of(date).into_iter().filter(|day| self.options.shows_day(*day)).collect();
                let column = days.iter().position(|day| *day == date).unwrap_or(0);
                let placement = if column < days.len() / 2 { Placement::RightOf } else { Placement::LeftOf };
                el.child(PopoverAttachment::new(placement, editor.view.clone()))
            })
            .into_any_element()
    }

    /// A task's bar on `day`; `None` in the Year view's day card, where a click opens the task in
    /// the list.
    fn task_bar(&self, task: &Task, day: Option<NaiveDate>, cx: &mut Context<Self>) -> Stateful<ui::HorizontalStack> {
        self.task_bar_with_resize(task, day, false, cx)
    }

    fn task_bar_with_resize(&self, task: &Task, day: Option<NaiveDate>, horizontal_resize: bool, cx: &mut Context<Self>) -> Stateful<ui::HorizontalStack> {
        let theme = cx.theme().clone();
        let id = task.id;
        // Keep closed titles muted; the checkbox distinguishes completed and dismissed tasks, while notes show their page icon.
        let done = task.is_closed();
        let completed = task.is_completed();
        let wont_do = task.is_wont_do();
        let tint = self.task_tint(task, &theme);
        let modern = self.options.modern;
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        let marker = if task.kind == fern_core::Kind::Note {
            Icon::new(IconName::FileText)
                .size(IconSize::XSmall)
                .color(if done { Color::Muted } else { Color::IconMuted })
                .into_any_element()
        } else {
            div()
                .debug_selector(move || format!("calendar-task-checkbox-{id}"))
                .flex_none()
                .flex()
                .items_center()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Checkbox::new(SharedString::from(format!("calendar-task-checkbox-{id}")))
                        .size(px(12.))
                        .toggle_state(completed)
                        .in_progress(task.is_in_progress())
                        .dismissed(wont_do)
                        .lines(task.checklist.is_some())
                        .tint(task_rows::priority_color(task.priority, &theme))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_task_completed(id, !completed && !wont_do, cx);
                        })),
                )
                .into_any_element()
        };
        let title = if task.title.trim().is_empty() { i18n::t(cx).untitled.to_owned() } else { task.title.clone() };
        let handle_title: SharedString = title.clone().into();
        let span_start = task.due_date.unwrap_or_else(|| Local::now().date_naive());
        let span_end = task.end_date.unwrap_or(span_start).max(span_start);
        let resize_handle = |edge: ResizeEdge| {
            let tag = if edge == ResizeEdge::Start { "start" } else { "end" };
            let payload = ResizedTask {
                id,
                title: handle_title.clone(),
                start: span_start,
                end: span_end,
                edge,
            };
            div()
                .id(SharedString::from(format!("calendar-task-resize-{tag}-{id}")))
                .flex_none()
                .w(px(5.))
                .h_full()
                .cursor_ew_resize()
                .on_drag(payload, |d, _, _, cx| cx.new(|_| d.clone()))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, _, cx| cx.stop_propagation())
        };
        let strings = i18n::t(cx);
        let tip: SharedString = match task.due_time {
            Some(time) => format!("{} {}", calendar::format_time(time, strings), title).into(),
            None => title.clone().into(),
        };
        let body = h_flex()
            .id(SharedString::from(format!("calendar-task-body-{id}")))
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .gap_1()
            .tooltip(ui::Tooltip::text(tip))
            .when(horizontal_resize, |el| {
                el.on_drag(dragged.clone(), |d, _, _, cx| cx.new(|_| d.clone()))
            })
            .child(marker)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(done, |el| el.line_through())
                    .child(
                        Label::new(title)
                            .size(LabelSize::XSmall)
                            .color(if done { Color::Muted } else { Color::Default })
                            .truncate(),
                    ),
            );
        h_flex()
            .id(SharedString::from(format!("calendar-task-{id}")))
            .debug_selector(|| format!("calendar-task-{id}"))
            .when(!horizontal_resize, |el| {
                el.on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
            })
            .h(px(18.))
            .flex_none()
            .gap_0p5()
            .px(if horizontal_resize { px(0.) } else { px(4.) })
            .rounded(px(3.))
            .cursor_pointer()
            // Style → Item Style: Classic fills the bar, Modern tints it lighter with a colored edge.
            .bg(tint.opacity(match (done, modern) {
                (true, _) => 0.12,
                (false, true) => 0.16,
                (false, false) => 0.28,
            }))
            .when(modern, |el| el.border_l_2().border_color(tint))
            .hover(|s| s.bg(tint.opacity(0.4)))
            .when(horizontal_resize, |el| el.child(resize_handle(ResizeEdge::Start)))
            .child(body)
            .when(horizontal_resize, |el| el.child(resize_handle(ResizeEdge::End)))
            .on_click(cx.listener(move |this, _, _, cx| match day {
                Some(day) => this.open_card(id, day, cx),
                None => cx.emit(OpenTask(id)),
            }))
            .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                this.open_context(id, event.position, cx);
                window.focus(&this.focus_handle, cx);
            }))
            // As in the original, the card opens on the bar's left, or on its right in the first
            // days of the week, where there is no room.
            .when_some(day.filter(|d| self.card == Some((id, *d))), |el, day| {
                let column = calendar::week_of(day).iter().position(|d| *d == day).unwrap_or(0);
                let placement = if column < 3 { Placement::RightOf } else { Placement::LeftOf };
                el.relative().child(PopoverAttachment::new(placement, self.render_card(cx)))
            })
    }

    fn render_mode_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let items = [
            (CalendarMode::Day, strings.day, "D/1"),
            (CalendarMode::Week, strings.week, "W/2"),
            (CalendarMode::Month, strings.month, "M/3"),
            (CalendarMode::Year, strings.year, "Y/4"),
            (CalendarMode::Agenda, strings.agenda, "A/5"),
        ];

        popover(cx)
            .debug_selector(|| "calendar-mode-menu".to_owned())
            .min_w(px(192.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.mode_menu_open = false;
                cx.notify();
            }))
            .children(items.into_iter().map(|(mode, label, hint)| {
                self.mode_item(mode, label, cx)
                    .end_slot(Label::new(hint).size(LabelSize::XSmall).color(Color::Muted))
            }))
            .child(div().py_1().child(Divider::horizontal()))
            .child(self.render_multi_mode_item(CalendarMode::MultiDay(self.multi_days), cx))
            .child(self.render_multi_mode_item(CalendarMode::MultiWeek(self.multi_weeks), cx))
            .into_any_element()
    }

    fn mode_item(&self, mode: CalendarMode, label: &'static str, cx: &Context<Self>) -> ListItem {
        let selected = self.mode == mode;
        ListItem::new(label)
            .start_slot(div().w(px(14.)).when(selected, |el| {
                el.child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent))
            }))
            .child(Label::new(label).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default }))
            .on_click(cx.listener(move |this, _, _, cx| this.set_mode(mode, cx)))
    }

    fn render_multi_mode_item(&self, mode: CalendarMode, cx: &Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let (group, label, count, hint, limits, mode_with_count) = match mode {
            CalendarMode::MultiDay(days) => (
                "calendar-multi-day",
                strings.multi_day,
                days,
                if days == 1 {
                    strings.one_day.to_owned()
                } else {
                    strings.n_days.replace("{n}", &days.to_string())
                },
                MULTI_DAYS,
                CalendarMode::MultiDay as fn(u8) -> CalendarMode,
            ),
            CalendarMode::MultiWeek(weeks) => (
                "calendar-multi-week",
                strings.multi_week,
                weeks,
                strings.n_weeks.replace("{n}", &weeks.to_string()),
                MULTI_WEEKS,
                CalendarMode::MultiWeek as fn(u8) -> CalendarMode,
            ),
            _ => unreachable!("only multi-day and multi-week have count controls"),
        };
        let step = |id: &'static str, icon: IconName, delta: i8| {
            let next = count.checked_add_signed(delta).filter(|next| limits.contains(next));
            div().debug_selector(|| format!("{group}-{id}")).child(
                IconButton::new(id, icon)
                    .size(px(20.))
                    .icon_size(IconSize::XSmall)
                    .when(next.is_none(), |button| button.icon_color(Color::Placeholder))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(next) = next {
                            this.apply_mode(mode_with_count(next), cx);
                        }
                    })),
            )
        };
        let touch = crate::touch::active();
        let count_controls = div()
            .relative()
            .flex_none()
            .child(
                // Keep the controls in the layout so hover only changes visibility.
                h_flex()
                    .when(!touch, |el| el.invisible().group_hover(group, |style| style.visible()))
                    .child(step("fewer", IconName::Minus, -1))
                    .child(
                        div().w(px(22.)).flex().justify_center()
                            .child(Label::new(count.to_string()).size(LabelSize::Small)),
                    )
                    .child(step("more", IconName::Plus, 1)),
            )
            .when(!touch, |el| {
                el.child(
                    div().absolute().inset_0().flex().items_center().justify_end()
                        .group_hover(group, |style| style.invisible())
                        .child(Label::new(hint).size(LabelSize::XSmall).color(Color::Muted).truncate()),
                )
            });
        div()
            .id(group)
            .group(group)
            .debug_selector(|| group.to_owned())
            .child(self.mode_item(mode, label, cx).end_slot(count_controls))
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

    fn render_month_grid(&self, today: NaiveDate, has_horizon_banner: bool, window: &Window, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let shown_weeks = self.month_grid_weeks();
        let banner_height = if has_horizon_banner { HORIZON_BANNER_HEIGHT } else { 0. };
        let grid_height = window.viewport_size().height - crate::title_bar::header_inset()
            - crate::title_bar::frame_vertical_inset(window)
            - px(CALENDAR_HEADER_HEIGHT + WEEKDAY_HEADER_HEIGHT + banner_height + 1.);
        let slots = day_item_slots(grid_height / shown_weeks.len().max(1) as f32);
        v_flex()
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(colors.border_variant)
            .children(shown_weeks.into_iter().map(|week| {
                h_flex()
                    .flex_1()
                    .items_stretch()
                    .min_h_0()
                    // Settings → Show Week Numbers: "W42" before each week, as in the original.
                    .when(calendar::week_numbers(), |el| {
                        el.child(
                            div()
                                .w(px(WEEK_NUMBER_WIDTH))
                                .flex_none()
                                .pt_1()
                                .flex()
                                .justify_center()
                                .child(Label::new(format!("W{}", calendar::week_number(&week))).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                    })
                    .children(week.into_iter().filter(|d| self.options.shows_day(*d)).map(|date| self.render_day(date, today, slots, cx)))
            }))
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
        let shown_month = match self.mode {
            CalendarMode::Month | CalendarMode::Agenda | CalendarMode::Year => self.month,
            CalendarMode::Week | CalendarMode::MultiWeek(_) => calendar::week_of(self.anchor)[0],
            _ => self.anchor,
        };
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
                    Label::new(format!("W{}", calendar::week_number(&calendar::week_of(self.anchor))))
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
                (_, _) if self.year_jump.is_some() => "CalendarDateInput",
                (false, CalendarMode::Year) => "Calendar CalendarYear",
                _ => "Calendar",
            })
            .on_action(cx.listener(|this, _: &NewTask, window, cx| this.open_task_editor(window, cx)))
            // The view menu's keys, as in the original: D/1, W/2, M/3.
            .on_action(cx.listener(|this, _: &DayView, _, cx| this.set_mode(CalendarMode::Day, cx)))
            .on_action(cx.listener(|this, _: &WeekView, _, cx| this.set_mode(CalendarMode::Week, cx)))
            .on_action(cx.listener(|this, _: &MonthView, _, cx| this.set_mode(CalendarMode::Month, cx)))
            .on_action(cx.listener(|this, _: &AgendaView, _, cx| this.set_mode(CalendarMode::Agenda, cx)))
            .on_action(cx.listener(|this, _: &YearView, _, cx| this.set_mode(CalendarMode::Year, cx)))
            .on_action(cx.listener(|this, _: &PreviousPeriod, _, cx| this.shift(-1, cx)))
            .on_action(cx.listener(|this, _: &NextPeriod, _, cx| this.shift(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousYear, _, cx| this.shift_year_using_keys(-1, cx)))
            .on_action(cx.listener(|this, _: &NextYear, _, cx| this.shift_year_using_keys(1, cx)))
            .on_action(cx.listener(|this, _: &CalendarToday, _, cx| this.go_to_today(cx)))
            .on_action(cx.listener(|this, _: &PrintYear, _, cx| this.print_year(cx)))
            .on_action(cx.listener(|this, _: &crate::actions::PrintList, _, cx| {
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
mod tests {
    use super::*;
    use chrono::TimeZone as _;
    use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, point, size};

    #[gpui::test]
    fn subscribed_event_bars_use_each_occurrences_local_time(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        store.add_subscription_with_feed("Synthetic meetings", "https://example.com/test.ics", None, concat!(
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:weekly-ny\nSUMMARY:Weekly meeting\n",
            "DTSTART;TZID=America/New_York:20261025T090000\n",
            "DTEND;TZID=America/New_York:20261025T100000\n",
            "RRULE:FREQ=WEEKLY;BYDAY=SU;COUNT=3\nEND:VEVENT\nEND:VCALENDAR",
        )).unwrap();
        cx.update(|cx| {
            theme::init(cx);
            crate::subscriptions::init(store.clone(), cx);
        });
        let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store, window, cx));
        let expected = [(10, 25, 13), (11, 1, 14), (11, 8, 14)].map(|(month, day, hour)| {
            chrono::Utc.with_ymd_and_hms(2026, month, day, hour, 0, 0).unwrap().with_timezone(&Local)
        });
        cx.update(|_, cx| {
            let mut events = view.read(cx).events_between(expected[0].date_naive(), expected[2].date_naive(), cx);
            events.sort_by_key(|event| (event.start, event.start_time));
            assert_eq!(events.len(), expected.len());
            for (event, start) in events.iter().zip(expected) {
                let end = start + chrono::Duration::hours(1);
                assert_eq!((event.start, event.start_time), (start.date_naive(), Some(start.time())));
                assert_eq!((event.end, event.end_time), (end.date_naive(), Some(end.time())));
            }
        });
    }

    #[gpui::test]
    fn the_whole_calendar_year_fits_and_scrolls_by_month_rows(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            cx.set_reduce_motion(true);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store, window, cx);
            view.sidebar_open = true;
            view.arrange = Some(0);
            view.set_mode(CalendarMode::Year, cx);
            window.focus(&view.focus_handle, cx);
            view
        });
        cx.simulate_resize(size(px(850.), px(368.)));
        cx.run_until_parked();
        let grid = cx.debug_bounds("calendar-year").unwrap();
        let year = cx.update(|_, cx| view.read(cx).month.year());
        let months: Vec<_> = (1..=12).map(|month| {
            let selector = format!("year-month-{year}-{month}").leak();
            let bounds = cx.debug_bounds(selector).unwrap();
            assert!(bounds.left() >= grid.left() && bounds.right() <= grid.right());
            assert!(bounds.top() >= grid.top() && bounds.bottom() <= grid.bottom());
            bounds
        }).collect();
        assert_eq!(months[0].top(), months[3].top());
        assert!(months[4].top() > months[0].top());
        let position = grid.center();
        cx.simulate_mouse_move(position, None, Modifiers::default());
        cx.simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Lines(point(0., -1.)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
        cx.update(|_, cx| assert_eq!(view.read(cx).month, NaiveDate::from_ymd_opt(year, 5, 1).unwrap()));
        assert_eq!(cx.debug_bounds("calendar-year"), Some(grid));
        cx.simulate_keystrokes("right");
        cx.update(|_, cx| assert_eq!(view.read(cx).month, NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap()));
        cx.simulate_keystrokes("left");
        cx.update(|_, cx| assert_eq!(view.read(cx).month.year(), year));

        cx.simulate_resize(size(px(850.), px(900.)));
        cx.run_until_parked();
        let grid = cx.debug_bounds("calendar-year").unwrap();
        let january = cx.debug_bounds(format!("year-month-{year}-1").leak()).unwrap();
        let april = cx.debug_bounds(format!("year-month-{year}-4").leak()).unwrap();
        let december = cx.debug_bounds(format!("year-month-{year}-12").leak()).unwrap();
        assert_eq!(january.left(), april.left());
        assert!(april.top() > january.top());
        assert!(december.bottom() <= grid.bottom());
    }

    #[gpui::test]
    fn a_task_dropped_on_the_year_grid_keeps_its_duration(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let start = NaiveDate::from_ymd_opt(2032, 1, 12).unwrap();
        let target = NaiveDate::from_ymd_opt(2032, 12, 10).unwrap();
        let time = chrono::NaiveTime::from_hms_opt(9, 30, 0).unwrap();
        let task = store.add_task("A two-day trip").unwrap();
        store.set_due(task.id, Some(start), Some(time)).unwrap();
        store.set_due_end(task.id, Some((start.succ_opt().unwrap(), Some(time)))).unwrap();
        cx.update(theme::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store.clone(), window, cx);
            view.month = calendar::month_start(start);
            view.set_mode(CalendarMode::Year, cx);
            view
        });
        let start = format!("year-date-{}-{start}", calendar::month_start(start)).leak();
        let end = format!("year-date-{}-{target}", calendar::month_start(target)).leak();
        let date = cx.debug_bounds(start).unwrap().center();
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(date, None, modifiers);
        cx.simulate_click(date, modifiers);
        let selector = format!("calendar-task-{}", task.id).leak();
        let bar = cx.debug_bounds(selector).unwrap().center();
        let end = cx.debug_bounds(end).unwrap().center();
        cx.simulate_mouse_move(bar, None, modifiers);
        cx.simulate_mouse_down(bar, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(point(bar.x + px(5.), bar.y), Some(MouseButton::Left), modifiers);
        cx.simulate_mouse_move(end, Some(MouseButton::Left), modifiers);
        cx.simulate_mouse_up(end, MouseButton::Left, modifiers);
        let task = store.task(task.id).unwrap().unwrap();
        assert_eq!((task.due_date, task.due_time), (Some(target), Some(time)));
        assert_eq!((task.end_date, task.end_time), (target.succ_opt(), Some(time)));
    }

    #[gpui::test]
    fn calendar_year_day_card_and_jumps_use_the_clicked_date(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let day = NaiveDate::from_ymd_opt(2032, 4, 13).unwrap();
        let month = calendar::month_start(day);
        let task = store.add_task("Prepare the report").unwrap();
        store.set_due(task.id, Some(day), None).unwrap();
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store, window, cx);
            window.focus(&view.focus_handle, cx);
            view.set_mode(CalendarMode::Year, cx);
            view.open_year_jump(window, cx);
            view
        });
        cx.simulate_input("2032-04-13");
        cx.simulate_keystrokes("enter");
        assert!(cx.debug_bounds("calendar-year-jump").is_none());
        cx.update(|_, cx| assert_eq!(view.read(cx).new_task_day(Local::now().date_naive()), day));

        let selector = format!("year-date-{month}-{day}").leak();
        let modifiers = Modifiers::default();
        let date = cx.debug_bounds(selector).unwrap().center();
        cx.simulate_mouse_move(date, None, modifiers);
        cx.simulate_click(date, modifiers);
        assert!(cx.debug_bounds("year-day-card").is_some());
        let task_selector = format!("calendar-task-{}", task.id).leak();
        assert!(cx.debug_bounds(task_selector).is_some());
        let task_position = cx.debug_bounds(task_selector).unwrap().center();
        cx.simulate_mouse_move(task_position, None, modifiers);
        cx.simulate_click(task_position, modifiers);
        cx.update(|_, cx| assert_eq!(view.read(cx).card, Some((task.id, day))));
        cx.simulate_keystrokes("escape");
        assert!(cx.debug_bounds("year-day-card").is_some());
        cx.simulate_keystrokes("escape");
        assert!(cx.debug_bounds("year-day-card").is_none());

        cx.simulate_mouse_move(date, None, modifiers);
        cx.simulate_event(MouseDownEvent { position: date, button: MouseButton::Left, modifiers, click_count: 2, first_mouse: false });
        cx.simulate_event(MouseUpEvent { position: date, button: MouseButton::Left, modifiers, click_count: 2 });
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.mode, CalendarMode::Week);
            assert!(view.shown_days().contains(&day));
        });
        cx.simulate_keystrokes("y");
        let october = cx.debug_bounds("year-heading-2032-10").unwrap().center();
        cx.simulate_mouse_move(october, None, modifiers);
        cx.simulate_click(october, modifiers);
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.mode, CalendarMode::Month);
            assert_eq!(view.month, NaiveDate::from_ymd_opt(2032, 10, 1).unwrap());
        });
    }

    #[gpui::test]
    fn escape_dismisses_calendar_overlays_in_order(cx: &mut TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            let errors = crate::keymap::load_default(&assets::Assets, cx).unwrap();
            assert!(errors.is_empty(), "keymap errors: {errors:?}");
        });

        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Calendar task").unwrap();
        let today = Local::now().date_naive();
        store.set_due(task.id, Some(today), None).unwrap();
        let (calendar, cx) = cx.add_window_view(|window, cx| {
            let mut calendar = CalendarView::new(store, window, cx);
            window.focus(&calendar.focus_handle, cx);
            calendar.open_card(task.id, today, cx);
            calendar.detail.update(cx, |detail, cx| detail.open_due(cx));
            calendar.open_options(Page::Main, cx);
            calendar
        });

        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| {
            let calendar = calendar.read(cx);
            assert!(calendar.options_dialog.is_none());
            assert!(calendar.detail.read(cx).has_popup());
            assert!(calendar.card.is_some());
        });

        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| {
            let calendar = calendar.read(cx);
            assert!(!calendar.detail.read(cx).has_popup());
            assert!(calendar.card.is_some());
        });

        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            let calendar = calendar.read(cx);
            assert!(calendar.card.is_none());
            assert!(calendar.focus_handle.is_focused(window));
        });

        let modifiers = Modifiers::default();
        let more = cx.debug_bounds("calendar-more-button").unwrap().center();
        cx.simulate_click(more, modifiers);
        cx.update(|_, cx| assert!(calendar.read(cx).more_menu));
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| assert!(!calendar.read(cx).more_menu));

        let task_bar = cx.debug_bounds(format!("calendar-task-{}", task.id).leak()).unwrap().center();
        cx.simulate_mouse_move(task_bar, None, modifiers);
        cx.simulate_mouse_down(task_bar, MouseButton::Right, modifiers);
        cx.simulate_mouse_up(task_bar, MouseButton::Right, modifiers);
        cx.update(|_, cx| assert!(calendar.read(cx).context_menu.is_some()));
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| assert!(calendar.read(cx).context_menu.is_none()));
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| assert!(calendar.read(cx).focus_handle.is_focused(window)));
    }

    #[gpui::test]
    fn calendar_mode_menu_keeps_its_bounds_while_adjusting_days_and_weeks(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::Ru));
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store, window, cx);
            view.multi_days = 9;
            view
        });
        let modifiers = Modifiers::default();
        let button = cx.debug_bounds("calendar-mode-button").unwrap().center();
        cx.simulate_mouse_move(button, None, modifiers);
        cx.simulate_click(button, modifiers);
        let menu = cx.debug_bounds("calendar-mode-menu").unwrap();

        for (row, more, mode) in [
            ("calendar-multi-day", "calendar-multi-day-more", CalendarMode::MultiDay(10)),
            ("calendar-multi-week", "calendar-multi-week-more", CalendarMode::MultiWeek(6)),
        ] {
            let row = cx.debug_bounds(row).unwrap().center();
            cx.simulate_mouse_move(row, None, modifiers);
            assert_eq!(cx.debug_bounds("calendar-mode-menu"), Some(menu));

            let more = cx.debug_bounds(more).expect("stepper button appears on hover").center();
            cx.simulate_mouse_move(more, None, modifiers);
            cx.simulate_click(more, modifiers);
            cx.update(|_, cx| assert_eq!(view.read(cx).mode, mode));
            assert_eq!(cx.debug_bounds("calendar-mode-menu"), Some(menu));
        }

        let more = cx.debug_bounds("calendar-multi-week-more").unwrap().center();
        cx.simulate_click(more, modifiers);
        cx.update(|_, cx| assert_eq!(view.read(cx).mode, CalendarMode::MultiWeek(6)));
        assert_eq!(cx.debug_bounds("calendar-mode-menu"), Some(menu));

        cx.simulate_mouse_move(button, None, modifiers);
        assert_eq!(cx.debug_bounds("calendar-mode-menu"), Some(menu));
        cx.simulate_keystrokes("escape");
        assert!(cx.debug_bounds("calendar-mode-menu").is_none());
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.multi_days, 10);
            assert_eq!(view.multi_weeks, 6);
            assert!(view.focus_handle.is_focused(window));
        });
    }

    #[gpui::test]
    fn month_cell_creates_a_task_for_its_date_and_list_without_intercepting_task_clicks(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let list = store.create_list("Work").unwrap();
        let day = NaiveDate::from_ymd_opt(2028, 7, 6).unwrap();
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store.clone(), window, cx);
            view.month = calendar::month_start(day);
            view.set_list_filter(Some(Some(list.id)), cx);
            view
        });
        cx.simulate_resize(size(px(1200.), px(600.)));
        let cell = cx.debug_bounds("calendar-day-2028-07-06").unwrap();
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(cell.center(), None, modifiers);
        cx.simulate_click(cell.center(), modifiers);
        assert!(store.tasks().unwrap().is_empty());
        let editor = cx.debug_bounds("calendar-task-editor").unwrap();
        assert!(editor.right() < cell.left() || editor.left() > cell.right());
        assert!(editor.top() >= px(8.) && editor.bottom() <= px(592.));
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.mode, CalendarMode::Month);
            assert_eq!(view.task_editor.as_ref().unwrap().day, Some(day));
            assert!(view.focus_handle(cx).is_focused(window));
        });
        cx.simulate_input("Plan 9am #work");
        cx.simulate_keystrokes("enter");
        assert!(cx.debug_bounds("calendar-task-editor").is_none());
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        let task = &tasks[0];
        assert_eq!(task.title, "Plan");
        assert_eq!(task.due_date, Some(day));
        assert_eq!(task.list_id, Some(list.id));
        assert_eq!(task.due_time, chrono::NaiveTime::from_hms_opt(9, 0, 0));
        let bar = cx.debug_bounds(format!("calendar-task-{}", task.id).leak()).unwrap().center();
        cx.simulate_mouse_move(bar, None, modifiers);
        cx.simulate_click(bar, modifiers);
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.card, Some((task.id, day)));
            assert!(view.task_editor.is_none());
        });
        cx.simulate_keystrokes("escape");
        store.set_due_end(task.id, Some((day.succ_opt().unwrap(), None))).unwrap();
        cx.update(|_, cx| view.update(cx, |view, cx| view.reload(cx)));
        let target = cx.debug_bounds("calendar-day-2028-07-13").unwrap().center();
        cx.simulate_mouse_move(bar, None, modifiers);
        cx.simulate_mouse_down(bar, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(point(bar.x + px(5.), bar.y), Some(MouseButton::Left), modifiers);
        cx.simulate_mouse_move(target, Some(MouseButton::Left), modifiers);
        cx.simulate_mouse_up(target, MouseButton::Left, modifiers);
        assert_eq!(store.tasks().unwrap().len(), 1);
        let moved = store.task(task.id).unwrap().unwrap();
        assert_eq!(moved.due_date, NaiveDate::from_ymd_opt(2028, 7, 13));
        assert_eq!(moved.end_date, NaiveDate::from_ymd_opt(2028, 7, 14));
        assert_eq!(moved.due_time, task.due_time);
        cx.update(|_, cx| assert!(view.read(cx).task_editor.is_none()));
    }

    #[gpui::test]
    fn calendar_navigation_saves_cell_draft_with_a_date_picker_open(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let day = NaiveDate::from_ymd_opt(2028, 7, 6).unwrap();
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store.clone(), window, cx);
            view.month = calendar::month_start(day);
            view.reload(cx);
            view
        });
        cx.simulate_resize(size(px(1200.), px(650.)));
        let modifiers = Modifiers::default();
        let cell = cx.debug_bounds("calendar-day-2028-07-06").unwrap().center();
        cx.simulate_click(cell, modifiers);
        cx.simulate_input("Save before navigating");
        let due = cx.debug_bounds("task-detail-due-date").unwrap().center();
        cx.simulate_click(due, modifiers);
        assert!(cx.debug_bounds("due-popup-ok").is_some());
        let next = cx.debug_bounds("calendar-next").unwrap().center();
        cx.simulate_click(next, modifiers);
        assert!(cx.debug_bounds("calendar-task-editor").is_none());
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.month, NaiveDate::from_ymd_opt(2028, 8, 1).unwrap());
            assert!(view.task_editor.is_none());
            assert!(view.focus_handle.is_focused(window));
        });
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].due_date, Some(day));
        let add = cx.debug_bounds("calendar-add-frame").unwrap().center();
        cx.simulate_click(add, modifiers);
        assert!(cx.debug_bounds("calendar-task-editor").is_some());
    }

    #[gpui::test]
    fn month_draft_keeps_adjacent_month_dates_and_saves_once_when_another_day_is_clicked(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let month = NaiveDate::from_ymd_opt(2028, 7, 1).unwrap();
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store.clone(), window, cx);
            view.month = month;
            view.reload(cx);
            view
        });
        let modifiers = Modifiers::default();
        let first = cx.debug_bounds("calendar-day-2028-06-26").unwrap().center();
        cx.simulate_mouse_move(first, None, modifiers);
        cx.simulate_click(first, modifiers);
        cx.simulate_input("June draft");
        let second = cx.debug_bounds("calendar-day-2028-07-17").unwrap().center();
        cx.simulate_mouse_move(second, None, modifiers);
        cx.simulate_click(second, modifiers);
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].due_date, Some(NaiveDate::from_ymd_opt(2028, 6, 26).unwrap()));
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.month, month);
            assert_eq!(view.task_editor.as_ref().unwrap().day, Some(NaiveDate::from_ymd_opt(2028, 7, 17).unwrap()));
            assert!(view.focus_handle(cx).is_focused(window));
        });
        cx.simulate_input("Save when closed");
        cx.simulate_keystrokes("escape");
        assert_eq!(store.tasks().unwrap().len(), 2);
        assert!(cx.debug_bounds("calendar-task-editor").is_none());
        cx.update(|window, cx| assert!(view.read(cx).focus_handle.is_focused(window)));
    }

    #[gpui::test]
    fn month_draft_uses_the_shared_card_and_keeps_notes_without_a_title(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store.clone(), window, cx);
            view.month = NaiveDate::from_ymd_opt(2028, 7, 1).unwrap();
            view.reload(cx);
            view
        });
        cx.simulate_resize(size(px(850.), px(368.)));
        let modifiers = Modifiers::default();
        let cell = cx.debug_bounds("calendar-day-2028-07-30").unwrap().center();
        cx.simulate_mouse_move(cell, None, modifiers);
        cx.simulate_click(cell, modifiers);
        let original = cx.update(|_, cx| view.read(cx).task_editor.as_ref().unwrap().view.clone());
        let editor = cx.debug_bounds("calendar-task-editor").unwrap();
        assert!(editor.left() >= px(8.) && editor.right() <= px(842.));
        assert!(editor.top() >= px(8.) && editor.bottom() <= px(360.));
        assert!(cx.debug_bounds("task-detail-header").is_some());
        assert!(cx.debug_bounds("task-detail-priority").is_some());
        assert!(cx.debug_bounds("task-detail-checklist-toggle").is_some());
        assert!(cx.debug_bounds("task-detail-list").is_some());
        let date = cx.debug_bounds("task-detail-due-date").unwrap().center();
        cx.simulate_mouse_move(date, None, modifiers);
        cx.simulate_click(date, modifiers);
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| assert_eq!(view.read(cx).task_editor.as_ref().unwrap().view, original));
        let notes = cx.debug_bounds("task-notes-editor").unwrap().center();
        cx.simulate_mouse_move(notes, None, modifiers);
        cx.simulate_click(notes, modifiers);
        cx.simulate_input("Keep these notes");
        let outside = cx.debug_bounds("calendar-day-2028-07-03").unwrap().center();
        cx.simulate_mouse_move(outside, None, modifiers);
        cx.simulate_click(outside, modifiers);
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].title.is_empty());
        assert_eq!(tasks[0].notes, "Keep these notes");
        assert_eq!(tasks[0].due_date, Some(NaiveDate::from_ymd_opt(2028, 7, 30).unwrap()));
        cx.update(|_, cx| assert_ne!(view.read(cx).task_editor.as_ref().unwrap().view, original));
        cx.simulate_keystrokes("escape");
        assert_eq!(store.tasks().unwrap().len(), 1, "closing an empty draft creates no task");
    }

    #[gpui::test]
    fn calendar_plus_opens_an_editor_with_the_selected_day_and_list(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let list = store.create_list("Work").unwrap();
        let hidden_list = store.create_list("Personal").unwrap();
        let today = Local::now().date_naive();
        let day = today.succ_opt().unwrap();
        let inbox_task = store.add_task("Inbox").unwrap();
        store.set_due(inbox_task.id, Some(day), None).unwrap();
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut calendar = CalendarView::new(store.clone(), window, cx);
            calendar.set_list_filter(Some(Some(list.id)), cx);
            calendar.month = calendar::month_start(day);
            calendar.reload(cx);
            window.focus(&calendar.focus_handle, cx);
            calendar
        });
        let selector = format!("calendar-select-day-{day}").leak();
        let modifiers = Modifiers::default();
        let date = cx.debug_bounds(selector).unwrap().center();
        cx.simulate_mouse_move(date, None, modifiers);
        cx.simulate_click(date, modifiers);
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.selected_day, Some(day));
            assert!(view.tasks_on(day).is_empty(), "Inbox is filtered out");
        });
        let add = cx.debug_bounds("calendar-add-frame").unwrap().center();
        cx.simulate_mouse_move(add, None, modifiers);
        cx.simulate_click(add, modifiers);
        assert!(cx.debug_bounds("calendar-task-editor").is_some());
        cx.simulate_click(add, modifiers);
        assert!(cx.debug_bounds("calendar-task-editor").is_none(), "clicking the add button again closes the editor");
        cx.simulate_click(add, modifiers);
        cx.simulate_keystrokes("escape");
        assert!(cx.debug_bounds("calendar-task-editor").is_none());
        assert_eq!(store.tasks().unwrap().len(), 1);
        cx.update(|window, cx| assert!(view.read(cx).focus_handle.is_focused(window)));

        cx.simulate_mouse_move(add, None, modifiers);
        cx.simulate_click(add, modifiers);
        cx.simulate_input("Review 9am #work");
        cx.simulate_keystrokes("enter");
        assert!(cx.debug_bounds("calendar-task-editor").is_none());
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 2);
        let task = tasks.iter().find(|task| task.id != inbox_task.id).unwrap();
        assert_eq!(task.title, "Review");
        assert_eq!(task.list_id, Some(list.id));
        assert_eq!(task.due_date, Some(day));
        assert_eq!(task.due_time, chrono::NaiveTime::from_hms_opt(9, 0, 0));
        assert_eq!(task.tags, ["work"]);
        cx.update(|_, cx| assert_eq!(view.read(cx).tasks_on(day)[0].id, task.id));
        cx.simulate_click(add, modifiers);
        cx.simulate_input("Save elsewhere ^Personal");
        cx.simulate_keystrokes("enter");
        let elsewhere = store.tasks().unwrap().into_iter().find(|task| task.title == "Save elsewhere").unwrap();
        assert_eq!(elsewhere.list_id, Some(hidden_list.id));
        assert!(cx.debug_bounds("calendar-saved-hint").is_some());
    }

    #[gpui::test]
    fn unchecking_a_calendar_card_reopens_the_task(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Reopen in calendar").unwrap();
        let today = Local::now().date_naive();
        store.set_due(task.id, Some(today), None).unwrap();
        store.set_completed(task.id, true).unwrap();
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        });
        let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
        let selector = format!("calendar-task-{}", task.id).leak();
        let position = cx.debug_bounds(selector).expect("completed task is rendered in the calendar").center();
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(position, None, modifiers);
        cx.simulate_click(position, modifiers);
        cx.update(|_, cx| assert_eq!(view.read(cx).card, Some((task.id, today))));
        let header = cx.debug_bounds("task-detail-header").expect("calendar task card is rendered");
        let position = point(header.left() + px(27.), header.center().y);
        cx.simulate_mouse_move(position, None, modifiers);
        cx.simulate_click(position, modifiers);
        let reopened = store.task(task.id).unwrap().unwrap();
        assert!(!reopened.is_closed());
        assert_eq!(reopened.due_date, Some(today));
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.card, Some((task.id, today)));
            assert!(view.tasks_on(today).iter().any(|task| !task.is_closed()));
        });
    }

    #[gpui::test]
    #[cfg(debug_assertions)]
    fn calendar_row_checkbox_toggles_completion_without_opening_card_and_notes_skip_checkbox(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = Local::now().date_naive();
        let task = store.add_task("Bar task").unwrap();
        store.set_due(task.id, Some(today), None).unwrap();
        store.add_checklist_item(task.id, "Step 1", None).unwrap();
        let note = store.add_task("Calendar note").unwrap();
        store.set_due(note.id, Some(today), None).unwrap();
        store.set_kind(note.id, fern_core::Kind::Note).unwrap();
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        });
        let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
        let note_task: &'static str = format!("calendar-task-{}", note.id).leak();
        let note_checkbox: &'static str = format!("calendar-task-checkbox-{}", note.id).leak();
        assert!(cx.debug_bounds(note_task).is_some());
        assert!(cx.debug_bounds(note_checkbox).is_none(), "notes render a page icon rather than a checkbox");

        let box_selector: &'static str = format!("calendar-task-checkbox-{}", task.id).leak();
        let modifiers = Modifiers::default();
        let box_pos = cx.debug_bounds(box_selector).expect("task checkbox is rendered").center();
        cx.simulate_mouse_move(box_pos, None, modifiers);
        cx.simulate_click(box_pos, modifiers);
        assert!(store.task(task.id).unwrap().unwrap().is_completed());
        cx.update(|_, cx| assert_eq!(view.read(cx).card, None, "clicking the checkbox must not open the task card"));

        let box_pos = cx.debug_bounds(box_selector).expect("completed task checkbox remains rendered").center();
        cx.simulate_click(box_pos, modifiers);
        assert!(!store.task(task.id).unwrap().unwrap().is_closed());
    }

    #[gpui::test]
    #[cfg(debug_assertions)]
    fn calendar_sidebar_is_full_height_and_header_month_picker_navigates(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let day = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        let work = store.create_list("Work").unwrap();
        let t1 = store.add_task("Tagged task").unwrap();
        store.set_due(t1.id, Some(day), None).unwrap();
        store.set_tags(t1.id, &["health".to_owned()]).unwrap();
        let t2 = store.add_task("Work task").unwrap();
        store.set_due(t2.id, Some(day), None).unwrap();
        store.set_list(t2.id, Some(work.id)).unwrap();
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut calendar = CalendarView::new(store.clone(), window, cx);
            calendar.sidebar_open = true;
            calendar.month = calendar::month_start(day);
            calendar.anchor = day;
            calendar.reload(cx);
            calendar
        });
        let sidebar = cx.debug_bounds("calendar-sidebar").expect("calendar sidebar is rendered");
        let toolbar = cx.debug_bounds("calendar-toolbar").expect("calendar toolbar is rendered");
        assert_eq!(sidebar.top(), px(0.), "sidebar extends to the top edge of the calendar view");
        assert!(toolbar.left() >= sidebar.right() - px(1.), "header sits beside the full-height sidebar");

        // Tag filter checkbox narrows tasks to matching tags; All resets.
        let modifiers = Modifiers::default();
        let tag_row = cx.debug_bounds("calendar-filter-tag-health").expect("tag filter row").center();
        cx.simulate_click(tag_row, modifiers);
        cx.update(|_, cx| {
            let ids: Vec<_> = view.read(cx).tasks_on(day).into_iter().map(|t| t.id).collect();
            assert_eq!(ids, vec![t1.id]);
        });
        let all_row = cx.debug_bounds("calendar-filter-all").expect("all filter row").center();
        cx.simulate_click(all_row, modifiers);
        cx.update(|_, cx| assert_eq!(view.read(cx).tasks_on(day).len(), 2));

        // Clicking the header month title opens the 4x3 month picker and jumps to the picked month.
        let title = cx.debug_bounds("calendar-month-title").expect("clickable month title").center();
        cx.simulate_click(title, modifiers);
        assert!(cx.debug_bounds("calendar-month-picker").is_some());
        let march = cx.debug_bounds("calendar-month-pick-2026-3").expect("March cell").center();
        cx.simulate_click(march, modifiers);
        assert!(cx.debug_bounds("calendar-month-picker").is_none());
        cx.update(|_, cx| assert_eq!(view.read(cx).month, NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()));
    }

    #[gpui::test]
    fn drag_resize_span_and_timed_range_and_dual_tz_options(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let t_span = store.add_task("Multi-day sprint").unwrap();
        store.set_due(t_span.id, Some(day), None).unwrap();

        let t_timed = store.add_task("Deep block").unwrap();
        let t_start = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        let t_end = NaiveTime::from_hms_opt(11, 0, 0).unwrap();
        store.set_due(t_timed.id, Some(day), Some(t_start)).unwrap();
        store.set_due_end(t_timed.id, Some((day, Some(t_end)))).unwrap();

        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut calendar = CalendarView::new(store.clone(), window, cx);
            calendar.mode = CalendarMode::Week;
            calendar.anchor = day;
            calendar.options.secondary_timezone = Some("UTC".into());
            store.save_calendar_options(&calendar.options).unwrap();
            calendar.reload(cx);
            calendar
        });

        // Horizontal date-span resize from end edge and start edge.
        let end_day = day + chrono::Duration::days(2);
        let earlier_day = day - chrono::Duration::days(1);
        view.update(cx, |cal, cx| {
            let r_end = ResizedTask {
                id: t_span.id,
                title: "Multi-day sprint".into(),
                start: day,
                end: day,
                edge: ResizeEdge::End,
            };
            cal.resize_task_to_day(&r_end, end_day, cx);
            let after_end = store.task(t_span.id).unwrap().unwrap();
            assert_eq!((after_end.due_date, after_end.end_date), (Some(day), Some(end_day)));

            let r_start = ResizedTask {
                id: t_span.id,
                title: "Multi-day sprint".into(),
                start: day,
                end: end_day,
                edge: ResizeEdge::Start,
            };
            cal.resize_task_to_day(&r_start, earlier_day, cx);
            let after_start = store.task(t_span.id).unwrap().unwrap();
            assert_eq!((after_start.due_date, after_start.end_date), (Some(earlier_day), Some(end_day)));
        });

        // Vertical timed resize: drag bottom +60px (1h) and commit, then Escape cancels a second drag.
        view.update(cx, |cal, cx| {
            cal.start_timed_resize(t_timed.id, day, ResizeEdge::End, t_start, t_end, None, px(100.), cx);
            cal.update_timed_resize(px(100. + week_axis::HOUR_HEIGHT), cx);
            cal.finish_timed_resize(cx);
            let saved = store.task(t_timed.id).unwrap().unwrap();
            assert_eq!(saved.due_time, Some(NaiveTime::from_hms_opt(10, 0, 0).unwrap()));
            assert_eq!(saved.end_time, Some(NaiveTime::from_hms_opt(12, 0, 0).unwrap()));
        });
        cx.update(|window, cx| {
            view.update(cx, |cal, cx| {
                cal.start_timed_resize(t_timed.id, day, ResizeEdge::End, t_start, NaiveTime::from_hms_opt(12, 0, 0).unwrap(), None, px(100.), cx);
                cal.update_timed_resize(px(220.), cx);
                assert!(cal.try_dismiss_transient(window, cx));
                assert!(cal.timed_resize.is_none());
            });
            let unchanged = store.task(t_timed.id).unwrap().unwrap();
            assert_eq!(unchanged.end_time, Some(NaiveTime::from_hms_opt(12, 0, 0).unwrap()));
        });
    }

    #[gpui::test]
    fn writable_caldav_event_crud_and_drag_move(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let sub_id = store
            .add_caldav_subscription(
                "Team",
                "http://127.0.0.1:9/dav/cal/",
                Some("#3B82F6"),
                fern_core::ical::CalendarProvider::CalDav,
                "alice",
                "pw",
                true,
            )
            .unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:cal-ev-1\r\nSUMMARY:Design Sync\r\nDTSTART:20261118T100000\r\nDTEND:20261118T110000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        store.upsert_local_calendar_event(sub_id, "cal-ev-1", "http://127.0.0.1:9/dav/cal/cal-ev-1.ics", ics).unwrap();

        cx.update(|cx| {
            theme::init(cx);
            crate::subscriptions::init(store.clone(), cx);
        });
        let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
        cx.run_until_parked();
        let next_day = day + chrono::Duration::days(1);
        cx.update(|_, cx| {
            let bars = view.read(cx).events_on(day, cx);
            assert_eq!(bars.len(), 1);
            assert!(bars[0].writable);
            assert_eq!(bars[0].subscription_id, sub_id);

            view.update(cx, |cal, cx| {
                cal.move_event_to_day(&DraggedEvent { subscription_id: sub_id, uid: "cal-ev-1".into(), title: "Design Sync".into() }, next_day, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(view.read(cx).events_on(next_day, cx).len(), 1);
            assert!(view.read(cx).events_on(day, cx).is_empty());

            view.update(cx, |cal, cx| {
                cal.delete_caldav_event(sub_id, "cal-ev-1", cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(view.read(cx).events_on(next_day, cx).is_empty());
        });
    }

    #[test]
    fn timed_lane_layout_splits_overlapping_clusters_and_keeps_adjacent_full_width() {
        assert!(assign_timed_lanes(&[]).is_empty());

        // Back-to-back intervals (touching at end == next.start) stay full width.
        let back_to_back = assign_timed_lanes(&[
            TimedInterval { start: 0., end: 60. },
            TimedInterval { start: 60., end: 120. },
        ]);
        assert_eq!(
            back_to_back,
            vec![
                LanePlacement { lane: 0, lane_count: 1 },
                LanePlacement { lane: 0, lane_count: 1 },
            ]
        );

        // Connected staircase cluster: A=[0, 120], B=[30, 90], C=[90, 150] -> 2 lanes (C reuses B's lane).
        // Independent D=[200, 260] -> full width (1 lane).
        let mixed = assign_timed_lanes(&[
            TimedInterval { start: 0., end: 120. },
            TimedInterval { start: 30., end: 90. },
            TimedInterval { start: 90., end: 150. },
            TimedInterval { start: 200., end: 260. },
        ]);
        assert_eq!(mixed[0], LanePlacement { lane: 0, lane_count: 2 });
        assert_eq!(mixed[1], LanePlacement { lane: 1, lane_count: 2 });
        assert_eq!(mixed[2], LanePlacement { lane: 1, lane_count: 2 });
        assert_eq!(mixed[3], LanePlacement { lane: 0, lane_count: 1 });

        // Three mutually overlapping intervals get 3 distinct lanes.
        let triple = assign_timed_lanes(&[
            TimedInterval { start: 100., end: 160. },
            TimedInterval { start: 100., end: 220. },
            TimedInterval { start: 110., end: 170. },
        ]);
        assert_eq!(triple.iter().map(|p| p.lane_count).collect::<Vec<_>>(), vec![3, 3, 3]);
        let lanes: HashSet<usize> = triple.iter().map(|p| p.lane).collect();
        assert_eq!(lanes.len(), 3);
        // Longest starting at 100 gets lane 0.
        assert_eq!(triple[1].lane, 0);
    }

    #[test]
    fn timed_resize_clamps_near_midnight_without_wrapping() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();

        // Resizing Start when orig_end is 00:10 must clamp to 00:00, never wrap to 23:55.
        let near_midnight_start = TimedResize {
            id: Id::now_v7(),
            date: day,
            edge: ResizeEdge::Start,
            orig_start: t(0, 0),
            orig_end: t(0, 10),
            end_date: None,
            start_y: px(0.),
            current_time: t(23, 0),
        };
        let (s, e) = near_midnight_start.snapped_range();
        assert_eq!((s, e), (t(0, 0), t(0, 10)));
        assert!(s <= e);

        // Resizing End when orig_start is 23:50 must clamp to 23:59, never wrap to 00:05.
        let near_midnight_end = TimedResize {
            id: Id::now_v7(),
            date: day,
            edge: ResizeEdge::End,
            orig_start: t(23, 50),
            orig_end: t(23, 59),
            end_date: None,
            start_y: px(0.),
            current_time: t(0, 15),
        };
        let (s, e) = near_midnight_end.snapped_range();
        assert_eq!((s, e), (t(23, 50), t(23, 59)));
        assert!(s <= e);

        // Mid-day shrink enforces 15-minute minimum span.
        let shrink_end = TimedResize {
            id: Id::now_v7(),
            date: day,
            edge: ResizeEdge::End,
            orig_start: t(10, 0),
            orig_end: t(11, 0),
            end_date: None,
            start_y: px(0.),
            current_time: t(9, 0),
        };
        assert_eq!(shrink_end.snapped_range(), (t(10, 0), t(10, 15)));

        let shrink_start = TimedResize {
            id: Id::now_v7(),
            date: day,
            edge: ResizeEdge::Start,
            orig_start: t(10, 0),
            orig_end: t(11, 0),
            end_date: None,
            start_y: px(0.),
            current_time: t(12, 0),
        };
        assert_eq!(shrink_start.snapped_range(), (t(10, 45), t(11, 0)));
    }

    #[gpui::test]
    fn multi_day_timed_resize_only_allows_real_start_and_end_edges(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let mon = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
        let tue = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap();
        let wed = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();

        let task = store.add_task("Multi-day deploy").unwrap();
        store.set_due(task.id, Some(mon), Some(t(14, 0))).unwrap();
        store.set_due_end(task.id, Some((wed, Some(t(11, 0))))).unwrap();

        cx.update(theme::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut cal = CalendarView::new(store.clone(), window, cx);
            cal.mode = CalendarMode::Week;
            cal.anchor = mon;
            cal.reload(cx);
            cal
        });

        view.update(cx, |cal, cx| {
            // Invalid edges are rejected cleanly without starting a misleading preview.
            cal.start_timed_resize(task.id, mon, ResizeEdge::End, t(14, 0), t(23, 59), Some(wed), px(100.), cx);
            assert!(cal.timed_resize.is_none());
            cal.start_timed_resize(task.id, tue, ResizeEdge::Start, t(0, 0), t(23, 59), Some(wed), px(100.), cx);
            assert!(cal.timed_resize.is_none());
            cal.start_timed_resize(task.id, tue, ResizeEdge::End, t(0, 0), t(23, 59), Some(wed), px(100.), cx);
            assert!(cal.timed_resize.is_none());
            cal.start_timed_resize(task.id, wed, ResizeEdge::Start, t(0, 0), t(11, 0), Some(wed), px(100.), cx);
            assert!(cal.timed_resize.is_none());

            // Valid Start resize on start day (Mon) moves due_time +1h and keeps Wed 11:00.
            cal.start_timed_resize(task.id, mon, ResizeEdge::Start, t(14, 0), t(23, 59), Some(wed), px(100.), cx);
            assert!(cal.timed_resize.is_some());
            cal.update_timed_resize(px(100. + week_axis::HOUR_HEIGHT), cx);
            cal.finish_timed_resize(cx);
            let saved = store.task(task.id).unwrap().unwrap();
            assert_eq!((saved.due_date, saved.due_time), (Some(mon), Some(t(15, 0))));
            assert_eq!((saved.end_date, saved.end_time), (Some(wed), Some(t(11, 0))));

            // Valid End resize on end day (Wed) moves end_time +1h and keeps Mon 15:00.
            cal.start_timed_resize(task.id, wed, ResizeEdge::End, t(0, 0), t(11, 0), Some(wed), px(100.), cx);
            assert!(cal.timed_resize.is_some());
            cal.update_timed_resize(px(100. + week_axis::HOUR_HEIGHT), cx);
            cal.finish_timed_resize(cx);
            let saved = store.task(task.id).unwrap().unwrap();
            assert_eq!((saved.due_date, saved.due_time), (Some(mon), Some(t(15, 0))));
            assert_eq!((saved.end_date, saved.end_time), (Some(wed), Some(t(12, 0))));
        });
    }

    #[gpui::test]
    fn overlapping_timed_tasks_render_side_by_side_in_week_view(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();

        let first = store.add_task("Overlap A").unwrap();
        store.set_due(first.id, Some(day), Some(t(10, 0))).unwrap();
        store.set_due_end(first.id, Some((day, Some(t(11, 0))))).unwrap();

        let second = store.add_task("Overlap B").unwrap();
        store.set_due(second.id, Some(day), Some(t(10, 30))).unwrap();
        store.set_due_end(second.id, Some((day, Some(t(11, 30))))).unwrap();

        cx.update(theme::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let mut cal = CalendarView::new(store.clone(), window, cx);
            cal.mode = CalendarMode::Day;
            cal.anchor = day;
            cal.reload(cx);
            cal
        });
        cx.simulate_resize(size(px(800.), px(700.)));
        cx.run_until_parked();

        let a = cx.debug_bounds(format!("calendar-task-{}", first.id).leak()).expect("first task bounds");
        let b = cx.debug_bounds(format!("calendar-task-{}", second.id).leak()).expect("second task bounds");
        assert!(
            a.right() <= b.left() + px(1.) || b.right() <= a.left() + px(1.),
            "overlapping timed tasks should sit side-by-side: a={a:?}, b={b:?}"
        );
    }

    #[gpui::test]
    fn caldav_event_editor_preserves_draft_on_error_and_preserves_tzid(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let sub_id = store
            .add_caldav_subscription(
                "Team",
                "http://127.0.0.1:9/dav/cal/",
                Some("#3B82F6"),
                fern_core::ical::CalendarProvider::CalDav,
                "alice",
                "pw",
                true,
            )
            .unwrap();
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:tz-ev-1\r\nSUMMARY:NY Sync\r\nLOCATION:Room 1\r\nDTSTART;TZID=America/New_York:20261118T090000\r\nDTEND;TZID=America/New_York:20261118T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        store
            .upsert_local_calendar_event(sub_id, "tz-ev-1", "http://127.0.0.1:9/dav/cal/tz-ev-1.ics", ics)
            .unwrap();

        cx.update(|cx| {
            theme::init(cx);
            crate::subscriptions::init(store.clone(), cx);
        });
        let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
        cx.run_until_parked();

        let search_from = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap();
        let search_to = NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();

        cx.update(|window, cx| {
            let bars = view.read(cx).events_between(search_from, search_to, cx);
            assert_eq!(bars.len(), 1);
            let event = bars[0].clone();

            view.update(cx, |cal, cx| {
                cal.open_edit_event_editor(&event, window, cx);
                let ed = cal.event_editor.as_ref().expect("editor opened");
                assert_eq!(ed.title.read(cx).text(), "NY Sync");
                assert_eq!(ed.location.read(cx).text(), "Room 1");
                assert_eq!(ed.timezone.as_deref(), Some("America/New_York"));

                // Invalid date must keep the draft open with an error message.
                ed.title.update(cx, |input, cx| input.set_text("Updated NY Sync", cx));
                ed.date.update(cx, |input, cx| input.set_text("not-a-date", cx));
                cal.save_event_editor(window, cx);
                let ed = cal.event_editor.as_ref().expect("draft preserved on invalid date");
                assert!(ed.error.is_some());
                assert_eq!(ed.title.read(cx).text(), "Updated NY Sync");

                // Backend save failure (unknown subscription) must also preserve the draft.
                let valid_date = event.start.format("%Y-%m-%d").to_string();
                ed.date.update(cx, |input, cx| input.set_text(valid_date, cx));
                if let Some(ed_mut) = cal.event_editor.as_mut() {
                    ed_mut.subscription_id = Id::now_v7();
                }
                cal.save_event_editor(window, cx);
                let ed = cal.event_editor.as_ref().expect("draft preserved on save failure");
                assert!(ed.error.is_some());
                assert_eq!(ed.title.read(cx).text(), "Updated NY Sync");

                // Restore valid subscription and save -> closes editor and preserves TZID.
                if let Some(ed_mut) = cal.event_editor.as_mut() {
                    ed_mut.subscription_id = sub_id;
                }
                cal.save_event_editor(window, cx);
                assert!(cal.event_editor.is_none());
            });
        });

        let saved = store
            .calendar_events(sub_id)
            .unwrap()
            .into_iter()
            .find(|ev| ev.uid == "tz-ev-1")
            .expect("stored event");
        assert!(saved.ics.contains("SUMMARY:Updated NY Sync"), "ics: {}", saved.ics);
        assert!(saved.ics.contains("TZID=America/New_York"), "TZID must be preserved: {}", saved.ics);
    }

    #[gpui::test]
    fn caldav_recurring_event_is_protected_from_clobbering_rrule(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let sub_id = store
            .add_caldav_subscription(
                "Team",
                "http://127.0.0.1:9/dav/cal/",
                Some("#3B82F6"),
                fern_core::ical::CalendarProvider::CalDav,
                "alice",
                "pw",
                true,
            )
            .unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:recur-1\r\nSUMMARY:Weekly Standup\r\nDTSTART:20261118T090000\r\nDTEND:20261118T093000\r\nRRULE:FREQ=WEEKLY;BYDAY=WE;COUNT=4\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        store
            .upsert_local_calendar_event(sub_id, "recur-1", "http://127.0.0.1:9/dav/cal/recur-1.ics", ics)
            .unwrap();

        cx.update(|cx| {
            theme::init(cx);
            crate::subscriptions::init(store.clone(), cx);
        });
        let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
        cx.run_until_parked();

        cx.update(|window, cx| {
            let bars = view.read(cx).events_on(day, cx);
            assert_eq!(bars.len(), 1);
            assert!(bars[0].recurring);
            let event = bars[0].clone();

            view.update(cx, |cal, cx| {
                cal.open_edit_event_editor(&event, window, cx);
                assert!(cal.event_editor.is_none(), "recurring master must not open destructive editor");

                cal.move_event_to_day(
                    &DraggedEvent { subscription_id: sub_id, uid: "recur-1".into(), title: "Weekly Standup".into() },
                    day + chrono::Duration::days(1),
                    cx,
                );
            });
        });

        let stored = store
            .calendar_events(sub_id)
            .unwrap()
            .into_iter()
            .find(|ev| ev.uid == "recur-1")
            .expect("recurring event still stored");
        assert!(stored.ics.contains("RRULE:FREQ=WEEKLY;BYDAY=WE;COUNT=4"));
    }

    #[test]
    fn dual_tz_reference_day_and_dst_offset_shift_detection() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        let week = vec![
            d(2026, 10, 26),
            d(2026, 10, 27),
            d(2026, 10, 28),
            d(2026, 10, 29),
            d(2026, 10, 30),
            d(2026, 10, 31),
            d(2026, 11, 1),
        ];

        // Priority: selected_day > today > anchor > first visible day.
        assert_eq!(dual_tz_reference_day(&week, Some(d(2026, 11, 1)), d(2026, 10, 26), d(2026, 10, 28)), d(2026, 11, 1));
        assert_eq!(dual_tz_reference_day(&week, Some(d(2026, 12, 1)), d(2026, 10, 26), d(2026, 10, 28)), d(2026, 10, 28));
        assert_eq!(dual_tz_reference_day(&week, None, d(2026, 10, 30), d(2026, 12, 1)), d(2026, 10, 30));
        assert_eq!(dual_tz_reference_day(&week, None, d(2026, 12, 1), d(2026, 12, 2)), d(2026, 10, 26));

        // US DST fall-back on 2026-11-01 shifts America/New_York vs UTC from -240m (EDT) to -300m (EST).
        let ny = "America/New_York";
        assert_eq!(calendar::timezone_offset_diff_minutes(d(2026, 10, 31), &chrono::Utc, ny), Some(-240));
        assert_eq!(calendar::timezone_offset_diff_minutes(d(2026, 11, 1), &chrono::Utc, ny), Some(-300));
        assert_eq!(calendar::timezone_offset_label_on(d(2026, 10, 31), ny), "UTC-4");
        assert_eq!(calendar::timezone_offset_label_on(d(2026, 11, 1), ny), "UTC-5");
        assert_eq!(calendar::timezone_offset_label_on(d(2026, 11, 1), "Asia/Kolkata"), "UTC+5:30");
    }

    static HORIZONS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct RestoreHorizonsEnabled;

    impl Drop for RestoreHorizonsEnabled {
        fn drop(&mut self) {
            calendar::set_horizons_enabled(true);
        }
    }

    #[gpui::test]
    fn horizon_goal_banners_render_in_week_month_year_and_respect_horizons_enabled(cx: &mut TestAppContext) {
        let _lock = HORIZONS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = RestoreHorizonsEnabled;
        calendar::set_horizons_enabled(true);

        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = Local::now().date_naive();
        let cur_week = fern_core::Horizon::current_week(today);
        let past_week = cur_week.shift(-1);
        let next_week = cur_week.shift(1);
        let cur_month = fern_core::Horizon::current_month(today);
        let past_month = cur_month.shift(-1);
        let cur_year = fern_core::Horizon::current_year(today);

        let week_goal = store.add_task("Ship week milestone").unwrap();
        store.set_horizon(week_goal.id, Some(cur_week)).unwrap();

        let past_week_goal = store.add_task("Carryover week goal").unwrap();
        store.set_horizon(past_week_goal.id, Some(past_week)).unwrap();

        let next_week_goal = store.add_task("Future week goal").unwrap();
        store.set_horizon(next_week_goal.id, Some(next_week)).unwrap();

        let done_week_goal = store.add_task("Finished week goal").unwrap();
        store.set_horizon(done_week_goal.id, Some(cur_week)).unwrap();
        store.set_completed(done_week_goal.id, true).unwrap();

        let month_goal = store.add_task("Close month target").unwrap();
        store.set_horizon(month_goal.id, Some(cur_month)).unwrap();

        let past_month_goal = store.add_task("Carryover month goal").unwrap();
        store.set_horizon(past_month_goal.id, Some(past_month)).unwrap();

        let year_goal = store.add_task("Annual objective").unwrap();
        store.set_horizon(year_goal.id, Some(cur_year)).unwrap();

        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        });
        let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
        cx.simulate_resize(size(px(1200.), px(720.)));
        cx.run_until_parked();

        let opened: Rc<std::cell::RefCell<Vec<Id>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
        let opened_sub = opened.clone();
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, OpenTask(id), _| {
                opened_sub.borrow_mut().push(*id);
            })
            .detach();
        });

        let month_chip: &'static str = format!("calendar-horizon-chip-{}", month_goal.id).leak();
        let past_month_chip: &'static str = format!("calendar-horizon-chip-{}", past_month_goal.id).leak();
        let week_chip: &'static str = format!("calendar-horizon-chip-{}", week_goal.id).leak();
        let past_week_chip: &'static str = format!("calendar-horizon-chip-{}", past_week_goal.id).leak();
        let past_week_check: &'static str = format!("calendar-horizon-check-{}", past_week_goal.id).leak();
        let next_week_chip: &'static str = format!("calendar-horizon-chip-{}", next_week_goal.id).leak();
        let done_week_chip: &'static str = format!("calendar-horizon-chip-{}", done_week_goal.id).leak();
        let year_chip: &'static str = format!("calendar-horizon-chip-{}", year_goal.id).leak();

        // Month view: shows current month + past unfinished month goals only.
        let month_banner = cx
            .debug_bounds("calendar-month-goals-banner")
            .expect("month goals banner renders when horizons enabled");
        assert!(month_banner.size.height > px(0.));
        assert!(cx.debug_bounds(month_chip).is_some());
        assert!(cx.debug_bounds(past_month_chip).is_some());
        assert!(cx.debug_bounds(week_chip).is_none());

        // Navigating to next month hides past-month rollovers when no goals exist for that month.
        cx.update(|_, cx| view.update(cx, |cal, cx| cal.shift(1, cx)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("calendar-month-goals-banner").is_none());
        cx.update(|_, cx| view.update(cx, |cal, cx| cal.go_to_today(cx)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("calendar-month-goals-banner").is_some());

        // Week view: shows current week + past unfinished week goals, excludes future & completed.
        cx.update(|_, cx| view.update(cx, |cal, cx| cal.set_mode(CalendarMode::Week, cx)));
        cx.run_until_parked();
        let week_banner = cx
            .debug_bounds("calendar-week-goals-banner")
            .expect("week goals banner renders when horizons enabled");
        assert!(week_banner.size.height > px(0.));
        assert!(cx.debug_bounds(week_chip).is_some());
        assert!(cx.debug_bounds(past_week_chip).is_some());
        assert!(cx.debug_bounds(next_week_chip).is_none());
        assert!(cx.debug_bounds(done_week_chip).is_none());

        // Clicking mini checkbox completes task and removes it from the banner.
        let check_pos = cx.debug_bounds(past_week_check).expect("week goal checkbox").center();
        cx.simulate_click(check_pos, Modifiers::default());
        cx.run_until_parked();
        assert!(store.task(past_week_goal.id).unwrap().unwrap().is_completed());
        assert!(cx.debug_bounds(past_week_chip).is_none());

        // Clicking the goal chip emits OpenTask.
        let chip_pos = cx.debug_bounds(week_chip).expect("week goal chip").center();
        cx.simulate_click(chip_pos, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(*opened.borrow(), vec![week_goal.id]);

        // Year view: shows displayed year's goals cleanly in the Year header strip.
        cx.update(|_, cx| view.update(cx, |cal, cx| cal.set_mode(CalendarMode::Year, cx)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("calendar-year-goals-banner").is_some());
        assert!(cx.debug_bounds(year_chip).is_some());

        // Disabling Horizons hides banners completely across Year, Month, and Week views.
        calendar::set_horizons_enabled(false);
        cx.update(|_, cx| view.update(cx, |cal, cx| cal.reload(cx)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("calendar-year-goals-banner").is_none());

        cx.update(|_, cx| view.update(cx, |cal, cx| cal.set_mode(CalendarMode::Month, cx)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("calendar-month-goals-banner").is_none());

        cx.update(|_, cx| view.update(cx, |cal, cx| cal.set_mode(CalendarMode::Week, cx)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("calendar-week-goals-banner").is_none());

        calendar::set_horizons_enabled(true);
    }

    #[gpui::test]
    fn dragging_horizon_goal_chip_onto_calendar_day_schedules_and_clears_horizon(cx: &mut TestAppContext) {
        let _lock = HORIZONS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _restore = RestoreHorizonsEnabled;
        calendar::set_horizons_enabled(true);

        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = Local::now().date_naive();
        let cur_month = fern_core::Horizon::current_month(today);
        let cur_week = fern_core::Horizon::current_week(today);

        let month_goal = store.add_task("Drag month goal").unwrap();
        store.set_horizon(month_goal.id, Some(cur_month)).unwrap();

        let week_goal = store.add_task("Drag week goal").unwrap();
        store.set_horizon(week_goal.id, Some(cur_week)).unwrap();

        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        });
        let (view, cx) = cx.add_window_view(|window, cx| CalendarView::new(store.clone(), window, cx));
        cx.simulate_resize(size(px(1200.), px(720.)));
        cx.run_until_parked();

        // Drag month goal chip onto today's day cell in Month view.
        let modifiers = Modifiers::default();
        let month_chip_sel: &'static str = format!("calendar-horizon-chip-{}", month_goal.id).leak();
        let day_cell_sel: &'static str = format!("calendar-day-{today}").leak();
        let chip_pos = cx.debug_bounds(month_chip_sel).expect("month goal chip").center();
        let cell_pos = cx.debug_bounds(day_cell_sel).expect("today day cell").center();
        cx.simulate_mouse_move(chip_pos, None, modifiers);
        cx.simulate_mouse_down(chip_pos, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(point(chip_pos.x + px(6.), chip_pos.y), Some(MouseButton::Left), modifiers);
        cx.simulate_mouse_move(cell_pos, Some(MouseButton::Left), modifiers);
        cx.simulate_mouse_up(cell_pos, MouseButton::Left, modifiers);
        cx.run_until_parked();

        let scheduled_month = store.task(month_goal.id).unwrap().unwrap();
        assert_eq!(scheduled_month.due_date, Some(today));
        assert_eq!(scheduled_month.horizon, None);
        assert!(cx.debug_bounds("calendar-month-goals-banner").is_none());
        let scheduled_bar: &'static str = format!("calendar-task-{}", month_goal.id).leak();
        assert!(cx.debug_bounds(scheduled_bar).is_some());

        // Drag week goal chip onto a day column in Week view.
        cx.update(|_, cx| view.update(cx, |cal, cx| cal.set_mode(CalendarMode::Week, cx)));
        cx.run_until_parked();
        let week_chip_sel: &'static str = format!("calendar-horizon-chip-{}", week_goal.id).leak();
        let week_day_sel: &'static str = format!("week-allday-{today}").leak();
        let chip_pos = cx.debug_bounds(week_chip_sel).expect("week goal chip").center();
        let col_pos = cx.debug_bounds(week_day_sel).expect("week day column").center();
        cx.simulate_mouse_move(chip_pos, None, modifiers);
        cx.simulate_mouse_down(chip_pos, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(point(chip_pos.x + px(6.), chip_pos.y), Some(MouseButton::Left), modifiers);
        cx.simulate_mouse_move(col_pos, Some(MouseButton::Left), modifiers);
        cx.simulate_mouse_up(col_pos, MouseButton::Left, modifiers);
        cx.run_until_parked();

        let scheduled_week = store.task(week_goal.id).unwrap().unwrap();
        assert_eq!(scheduled_week.due_date, Some(today));
        assert_eq!(scheduled_week.horizon, None);
        assert!(cx.debug_bounds("calendar-week-goals-banner").is_none());
    }
}
