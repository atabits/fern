//! The calendar: tasks placed on their due dates in a month grid or a week with a time axis,
//! matched to TickTick's Calendar section.

use std::{collections::BTreeMap, rc::Rc, time::Duration};

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{CalendarOptions, ColorBy, Id, Priority, Store, Task};
use gpui::{EventEmitter, Focusable, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::actions::{AgendaView, CalendarToday, DayView, DismissTransient, MonthView, NewTask, NextYear, PreviousYear, PrintYear, WeekView, YearView};
use crate::calendar::{self, week_axis};
use crate::drag::DraggedTask;
use crate::i18n;
use crate::calendar_options_view::{CalendarOptionsDialog, OptionsEvent, Page};
use crate::task_detail::{StartFocus, TaskDetail, TaskUpdated};
use crate::task_rows;

#[path = "calendar_task_editor.rs"]
mod calendar_task_editor;
use calendar_task_editor::{CalendarTaskEditor, EditorEvent, month_picker};

#[path = "calendar_year.rs"]
mod calendar_year;

/// How many tasks a day cell shows before collapsing the rest into "+N".
const TASKS_PER_DAY: usize = 5;
const CALENDAR_HEADER_HEIGHT: f32 = 50.;
const CALENDAR_SIDEBAR_WIDTH: f32 = 216.;
const WEEKDAY_HEADER_HEIGHT: f32 = 22.;
const DAY_HEADER_HEIGHT: f32 = 26.;
const DAY_ITEM_HEIGHT: f32 = 18.;
const DAY_ITEM_GAP: f32 = 2.;
const DAY_TOP_PADDING: f32 = 4.;

fn day_item_slots(height: Pixels) -> usize {
    let available = f32::from(height) - DAY_HEADER_HEIGHT - DAY_TOP_PADDING - 1.;
    (available / (DAY_ITEM_HEIGHT + DAY_ITEM_GAP)).floor().max(0.) as usize
}

/// The user clicked a task in the calendar.

/// The week-number column of the month grid.
const WEEK_NUMBER_WIDTH: f32 = 32.;
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
    /// No filter, Inbox, or one of the available lists.
    list_filter: Option<Option<Id>>,
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
    /// A task's right-click menu: the task and where the click was, and its open side menu.
    context_menu: Option<(Id, gpui::Point<Pixels>)>,
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
    detail: gpui::Entity<TaskDetail>,
    /// View Options, the header's "…" menu and the dialog it opens.
    options: CalendarOptions,
    more_menu: bool,
    options_dialog: Option<gpui::Entity<CalendarOptionsDialog>>,
    /// The "Arrange tasks" panel: the open tasks without a date, grouped by the tab's choice
    /// (lists, tags or priority), to drag onto a day.
    arrange: Option<usize>,
    undated: Vec<Task>,
    /// The colors tasks take with Color By List or Tag.
    list_colors: std::collections::HashMap<Id, gpui::Hsla>,
    tag_colors: std::collections::HashMap<String, gpui::Hsla>,
    list_names: Vec<(Id, String)>,
    /// With Show Habit, the habits due today; with Show Focus Records, the focus sessions.
    habits_today: Vec<fern_core::Habit>,
    focus_sessions: Vec<fern_core::FocusSession>,
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

impl EventEmitter<OpenTask> for CalendarView {}
impl EventEmitter<OpenTaskPicker> for CalendarView {}
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
        cx.subscribe(&detail, |this, _, _: &TaskUpdated, cx| this.card_updated(cx)).detach();
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
            list_filter: None,
            task_editor: None,
            saved_hint: None,
            mode_menu_open: false,
            focus_handle: cx.focus_handle(),
            multi_days: 1,
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
            detail,
            options: CalendarOptions::default(),
            more_menu: false,
            options_dialog: None,
            arrange: None,
            undated: Vec::new(),
            list_colors: Default::default(),
            tag_colors: Default::default(),
            list_names: Vec::new(),
            habits_today: Vec::new(),
            focus_sessions: Vec::new(),
        };
        this.reload(cx);
        this
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
    /// on a click. Events cannot be changed here: they belong to the calendar they come from.
    fn event_bar(&self, event: EventBar, date: NaiveDate, cx: &mut Context<Self>) -> Stateful<ui::HorizontalStack> {
        let strings = i18n::t(cx);
        let open = self.open_event.as_ref() == Some(&(event.key.clone(), event.start));
        let key = (event.key.clone(), event.start);
        let column = calendar::week_of(date).iter().position(|d| *d == date).unwrap_or(0);
        h_flex()
            .id(SharedString::from(format!("calendar-event-{}-{}-{date}", event.key, event.start)))
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
            .child(div().flex_1().min_w_0().child(Label::new(event.title.clone()).size(LabelSize::XSmall).truncate()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_event = if this.open_event.as_ref() == Some(&key) { None } else { Some(key.clone()) };
                window.focus(&this.focus_handle, cx);
                cx.notify();
            }))
            .when(open, |el| {
                let placement = if column < 3 { Placement::RightOf } else { Placement::LeftOf };
                el.relative().child(PopoverAttachment::new(placement, self.render_event_card(&event, cx)))
            })
    }

    /// An event's card: its title, when it is, its calendar, place and description.
    fn render_event_card(&self, event: &EventBar, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let day = |date: NaiveDate| task_rows::format_day(strings.closed_date[0], date, strings);
        let when = match (event.start_time, event.end_time) {
            (Some(from), Some(to)) if event.start == event.end => format!("{}, {} – {}", day(event.start), calendar::format_time(from, strings), calendar::format_time(to, strings)),
            (Some(from), Some(to)) => format!("{} {} – {} {}", day(event.start), calendar::format_time(from, strings), day(event.end), calendar::format_time(to, strings)),
            (Some(from), None) => format!("{}, {}", day(event.start), calendar::format_time(from, strings)),
            (None, _) if event.start == event.end => format!("{}, {}", day(event.start), strings.range_all_day),
            (None, _) => format!("{} – {}", day(event.start), day(event.end)),
        };
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
                    div()
                        .id("calendar-event-notes")
                        .max_h(px(160.))
                        .overflow_y_scroll()
                        .text_size(px(12.))
                        .child(event.notes.trim().to_owned()),
                )
            })
            .into_any_element()
    }

    /// An event's row in the Agenda, as a task's: its time, the calendar's dot and a card in the
    /// calendar's color, which opens the event's card.
    fn agenda_event_row(&self, event: EventBar, date: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let when = event.start_time.filter(|_| event.start == date).map_or(strings.range_all_day.to_owned(), |time| calendar::format_time(time, strings));
        let key = (event.key.clone(), event.start);
        let open = self.open_event.as_ref() == Some(&key);
        h_flex()
            .gap_3()
            .child(div().w(px(72.)).flex_none().child(Label::new(when).size(LabelSize::Small).color(Color::Muted)))
            .child(div().w(px(16.)).flex_none().flex().justify_center().child(div().size(px(10.)).rounded_full().bg(event.color)))
            .child(
                h_flex()
                    .id(SharedString::from(format!("agenda-event-{}-{}", event.key, event.start)))
                    .flex_1()
                    .min_w_0()
                    .h(px(40.))
                    .px_3()
                    .rounded(px(6.))
                    .border_l_3()
                    .border_color(event.color)
                    .bg(event.color.opacity(0.12))
                    .hover(|s| s.bg(event.color.opacity(0.2)))
                    .cursor_pointer()
                    .child(div().flex_1().min_w_0().child(Label::new(event.title.clone()).truncate()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_event = if this.open_event.as_ref() == Some(&key) { None } else { Some(key.clone()) };
                        window.focus(&this.focus_handle, cx);
                        cx.notify();
                    }))
                    .when(open, |el| el.relative().child(PopoverAttachment::new(Placement::BelowRight, self.render_event_card(&event, cx)))),
            )
            .into_any_element()
    }

    fn day_extras(&self, date: NaiveDate, cx: &App) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let theme = cx.theme();
        let today = Local::now().date_naive();
        let bar = |tint: gpui::Hsla| h_flex().h(px(18.)).flex_none().gap_1().px_1().rounded(px(3.)).bg(tint.opacity(0.28));
        let mut items: Vec<AnyElement> = Vec::new();
        if date == today {
            for habit in &self.habits_today {
                let tint = match self.options.habit_color {
                    fern_core::HabitColor::Theme => theme.colors().accent,
                    fern_core::HabitColor::Fixed => crate::calendar_options_view::palette(self.options.habit_fixed),
                    fern_core::HabitColor::Habit => crate::lists_panel::parse_color(habit.color.as_deref()).unwrap_or(theme.colors().accent),
                };
                items.push(
                    bar(tint)
                        .child(Icon::new(IconName::FernHabit).size(IconSize::XSmall).color(Color::Muted))
                        .child(Label::new(habit.name.clone()).size(LabelSize::XSmall).truncate())
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
        let dialog = cx.new(|_| CalendarOptionsDialog::new(store, page));
        cx.subscribe(&dialog, |this, _, event: &OptionsEvent, cx| {
            match event {
                OptionsEvent::Changed(options) => this.options = *options,
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

    /// The original's "Arrange tasks" panel at the calendar's right: the open tasks without a
    /// date under Lists, Tags or Priority tabs, dragged onto a day to plan them.
    fn render_arrange(&self, tab: usize, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let mut groups: Vec<(String, Vec<&Task>)> = Vec::new();
        match tab {
            0 => {
                let mut keys: Vec<(Option<Id>, String)> = vec![(None, strings.inbox.to_owned())];
                keys.extend(self.list_names.iter().map(|(id, name)| (Some(*id), name.clone())));
                for (id, name) in keys {
                    let tasks: Vec<&Task> = self.undated.iter().filter(|t| t.list_id == id).collect();
                    if !tasks.is_empty() {
                        groups.push((name, tasks));
                    }
                }
            }
            1 => {
                let mut names: Vec<String> = self.undated.iter().flat_map(|t| t.tags.iter().cloned()).collect();
                names.sort_by_key(|n| n.to_lowercase());
                names.dedup_by_key(|n| n.to_lowercase());
                for name in names {
                    let tasks: Vec<&Task> = self.undated.iter().filter(|t| t.tags.iter().any(|g| g.eq_ignore_ascii_case(&name))).collect();
                    groups.push((format!("#{name}"), tasks));
                }
                let untagged: Vec<&Task> = self.undated.iter().filter(|t| t.tags.is_empty()).collect();
                if !untagged.is_empty() {
                    groups.push((strings.group_no_tags.to_owned(), untagged));
                }
            }
            _ => {
                for (priority, label) in [Priority::High, Priority::Medium, Priority::Low, Priority::None]
                    .into_iter()
                    .zip([strings.priority_high, strings.priority_medium, strings.priority_low, strings.priority_none])
                {
                    let tasks: Vec<&Task> = self.undated.iter().filter(|t| t.priority == priority).collect();
                    if !tasks.is_empty() {
                        groups.push((label.to_owned(), tasks));
                    }
                }
            }
        }
        let tabs = h_flex().p(px(3.)).rounded(px(8.)).bg(theme.colors().surface_background).children(strings.arrange_tabs.iter().enumerate().map(|(i, label)| {
            div()
                .id(SharedString::from(format!("arrange-tab-{i}")))
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(i == tab, |el| el.bg(theme.colors().element_selected))
                .child(Label::new(*label).size(LabelSize::Small).color(if i == tab { Color::Default } else { Color::Muted }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.arrange = Some(i);
                    cx.notify();
                }))
        }));
        let mut body: Vec<AnyElement> = Vec::new();
        for (name, tasks) in groups {
            body.push(
                h_flex()
                    .pt_2()
                    .pb_1()
                    .gap_1()
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                    .child(Label::new(name).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                    .child(Label::new(tasks.len().to_string()).size(LabelSize::XSmall).color(Color::Muted))
                    .into_any_element(),
            );
            body.extend(tasks.into_iter().map(|t| self.task_bar(t, None, cx).h(px(20.)).into_any_element()));
        }
        if body.is_empty() {
            body.push(div().pt_4().child(Label::new(strings.arrange_empty).size(LabelSize::Small).color(Color::Muted)).into_any_element());
        }
        v_flex()
            .id("calendar-arrange")
            .w(px(240.))
            .flex_none()
            .h_full()
            .px_3()
            .pt(crate::title_bar::height() + px(4.))
            .gap_2()
            .border_l_1()
            .border_color(theme.colors().border_variant)
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.arrange_tasks_title).weight(FontWeight::SEMIBOLD)))
                    .child(IconButton::new("arrange-close", IconName::X).size(px(22.)).icon_size(IconSize::Small).on_click(cx.listener(|this, _, _, cx| {
                        this.arrange = None;
                        cx.notify();
                    }))),
            )
            .child(tabs)
            .child(v_flex().id("arrange-tasks").flex_1().min_h_0().overflow_y_scroll().gap(px(3.)).children(body))
            .into_any_element()
    }

    /// Opens a task's card beside it, the task as clicked on `day`.
    fn open_card(&mut self, id: Id, day: NaiveDate, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let Ok(Some(task)) = self.store.task(id) else { return };
        let lists = self.store.lists().unwrap_or_default();
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
        if self.context_menu.take().is_some() {
            self.context_side = None;
            cx.notify();
        } else if std::mem::take(&mut self.mode_menu_open) || std::mem::take(&mut self.more_menu) {
            cx.notify();
        } else if self.year_jump.is_some() {
            self.close_year_jump(window, cx);
        } else if let Some(editor) = &self.task_editor {
            editor.view.update(cx, |editor, cx| editor.dismiss(window, cx));
        } else if self.options_dialog.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        } else if self.card.is_some() {
            if self.detail.update(cx, |detail, cx| detail.dismiss_popup(window, cx)) {
                return;
            }
            self.close_card(cx);
            window.focus(&self.focus_handle, cx);
        } else if self.open_event.take().is_some()
            || self.open_countdown.take().is_some()
            || self.year_day.take().is_some()
        {
            cx.notify();
        } else {
            cx.propagate();
        }
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

    /// Re-reads the tasks of the shown days.
    fn open_context(&mut self, id: Id, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        self.context_menu = Some((id, position));
        self.context_side = None;
        cx.notify();
    }

    /// A choice in a task's right-click menu (see `task_menu`): edits apply here; the pickers
    /// open the task in the list.
    fn context_action(&mut self, action: crate::task_menu::MenuAction, cx: &mut Context<Self>) {
        use crate::task_menu::MenuAction;
        let Some((id, _)) = self.context_menu else { return };
        if let MenuAction::Side(side) = action {
            self.context_side = side;
            cx.notify();
            return;
        }
        self.context_menu = None;
        self.context_side = None;
        let Some(task) = self.store.task(id).ok().flatten() else {
            cx.notify();
            return;
        };
        if let Some(result) = crate::task_menu::apply(&self.store, &task, action, Local::now().date_naive()) {
            if let Err(err) = result {
                eprintln!("failed to change the task: {err:#}");
            }
            self.reload(cx);
            return;
        }
        match action {
            MenuAction::Due(None) => cx.emit(OpenTaskPicker(id, "due")),
            MenuAction::LinkParent => cx.emit(OpenTaskPicker(id, "link-parent")),
            MenuAction::Tags => cx.emit(OpenTaskPicker(id, "tags")),
            MenuAction::AddSubtask => cx.emit(OpenTaskPicker(id, "subtask")),
            MenuAction::Focus(stopwatch) => cx.emit(crate::task_detail::StartFocus(id, task.title.clone(), stopwatch)),
            MenuAction::CopyLink => cx.write_to_clipboard(gpui::ClipboardItem::new_string(fern_core::link::task_link(id))),
            MenuAction::StickyNote => crate::sticky_note::open(self.store.clone(), id, cx),
            MenuAction::Delete => {
                if let Err(err) = self.store.delete_task(id) {
                    eprintln!("failed to delete the task: {err:#}");
                }
                self.reload(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// The task's right-click menu, as the original's calendar has it (no Add Subtask or Pin).
    fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, position) = self.context_menu?;
        let task = self.store.task(id).ok().flatten()?;
        let lists = self.store.lists().unwrap_or_default();
        let menu = crate::task_menu::render(
            &task,
            &lists,
            crate::task_menu::MenuKind::Calendar,
            self.context_side,
            i18n::t(cx),
            cx,
            Rc::new(|this: &mut Self, action, _, cx| this.context_action(action, cx)),
        );
        Some(crate::task_menu::place(position, menu))
    }

    /// Opens a task's right-click menu from the debug-build dev state (`calendar-context=<title part>`).
    #[cfg(debug_assertions)]
    pub fn dev_context(&mut self, needle: &str, cx: &mut Context<Self>) {
        if let Some(task) = self.tasks.iter().find(|t| t.title.contains(needle)) {
            self.open_context(task.id, gpui::point(px(600.), px(300.)), cx);
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.year_heatmap.clear();
        self.options = self.store.calendar_options().unwrap_or_default();
        let lists = self.store.lists().unwrap_or_default();
        self.list_colors = lists.iter().filter_map(|l| crate::lists_panel::parse_color(l.color.as_deref()).map(|c| (l.id, c))).collect();
        self.list_names = lists.iter().filter(|l| !l.archived).map(|l| (l.id, l.name.clone())).collect();
        self.list_filter = self.store.setting("calendar_list_filter").ok().flatten().and_then(|filter| {
            if filter == "inbox" {
                Some(None)
            } else {
                filter.parse::<Id>().ok().filter(|id| self.list_names.iter().any(|(list, _)| list == id)).map(Some)
            }
        });
        let list_filter = self.list_filter;
        let in_scope = |task: &Task| list_filter.is_none_or(|list| list == task.list_id);
        self.tag_colors = self
            .store
            .tags()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| crate::lists_panel::parse_color(t.color.as_deref()).map(|c| (t.name.to_lowercase(), c)))
            .collect();
        if self.mode == CalendarMode::Agenda {
            let from = self.agenda_start();
            self.countdowns = self.store.countdowns().unwrap_or_default();
            match self.store.tasks() {
                Ok(mut tasks) => {
                    tasks.retain(in_scope);
                    let countdowns: &[fern_core::Countdown] = if self.options.show_countdowns { &self.countdowns } else { &[] };
                    self.agenda = fern_core::agenda::agenda(&tasks, countdowns, from, from + chrono::Duration::days(AGENDA_DAYS))
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
        self.tasks.clear();
        self.undated.clear();
        let mut repeats_before_range = Vec::new();
        for task in all.into_iter().filter(|task| !task.is_deleted()) {
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
        let today = Local::now().date_naive();
        self.habits_today = if self.options.show_habits {
            self.store
                .habits()
                .unwrap_or_default()
                .into_iter()
                .filter(|h| !h.archived && h.is_due(today, &self.store.checkins(h.id).unwrap_or_default()))
                .collect()
        } else {
            Vec::new()
        };
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
            let january = NaiveDate::from_ymd_opt(self.month.year() + delta, 1, 1).unwrap();
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
                self.month = NaiveDate::from_ymd_opt(day.year(), 1, 1).unwrap();
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
        if let Err(err) = self.store.move_dates(id, date) {
            eprintln!("failed to move the task: {err:#}");
        }
        self.reload(cx);
    }

    fn go_to_today(&mut self, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let today = Local::now().date_naive();
        if self.mode == CalendarMode::Year {
            self.anchor = calendar_year::clamp_year_day(today);
            self.month = NaiveDate::from_ymd_opt(self.anchor.year(), 1, 1).unwrap();
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

    fn set_list_filter(&mut self, filter: Option<Option<Id>>, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let value = match filter {
            None => String::new(),
            Some(None) => "inbox".to_owned(),
            Some(Some(id)) => id.to_string(),
        };
        if let Err(error) = self.store.set_setting("calendar_list_filter", &value) {
            eprintln!("failed to save the calendar filter: {error:#}");
            return;
        }
        self.close_card(cx);
        self.reload(cx);
    }

    fn select_day(&mut self, day: NaiveDate, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let day = if self.mode == CalendarMode::Year {
            let day = calendar_year::clamp_year_day(day);
            self.month = NaiveDate::from_ymd_opt(day.year(), 1, 1).unwrap();
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
        if let Some(editor) = &self.task_editor {
            if editor.day == Some(date) || !editor.view.update(cx, |editor, cx| editor.close(cx)) {
                window.focus(&editor.view.focus_handle(cx), cx);
                return;
            }
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
        let list = self.list_filter.unwrap_or_else(|| {
            self.store.preferences().ok().and_then(|preferences| preferences.default_list)
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
        let (Some(filter), Ok(Some(task))) = (self.list_filter, self.store.task(id)) else {
            return;
        };
        if filter == task.list_id {
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
        let row = |filter, name: String, icon| {
            ListItem::new(SharedString::from(format!("calendar-filter-{filter:?}")))
                .height(px(30.))
                .hover_color(theme.colors().list_hover)
                .selected_color(theme.colors().list_selected)
                .start_slot(Icon::new(icon).size(IconSize::Small).color(theme.colors().list_text))
                .child(Label::new(name).truncate().color(theme.colors().list_text))
                .toggle_state(self.list_filter == filter)
                .on_click(cx.listener(move |this, _, _, cx| this.set_list_filter(filter, cx)))
        };
        v_flex().id("calendar-sidebar").debug_selector(|| "calendar-sidebar".to_owned())
            .flex_none().w(px(CALENDAR_SIDEBAR_WIDTH)).h_full().overflow_y_scroll()
            .border_r_1().border_color(theme.colors().lists_border)
            .bg(theme.colors().lists_background)
            .p_3().gap_2()
            .child(month_picker(
                self.sidebar_month.unwrap_or(self.month), self.selected_day.unwrap_or(self.anchor),
                cx.listener(|this, delta: &i32, _, cx| {
                    this.sidebar_month = Some(calendar::add_months(this.sidebar_month.unwrap_or(this.month), *delta));
                    cx.notify();
                }),
                cx.listener(|this, day: &NaiveDate, _, cx| this.select_day(*day, cx)),
                cx,
            ))
            .child(Divider::horizontal())
            .child(Label::new(strings.lists).size(LabelSize::Small).weight(FontWeight::SEMIBOLD).color(theme.colors().list_text))
            .child(row(None, strings.all_lists.to_owned(), IconName::Layers))
            .child(row(Some(None), strings.inbox.to_owned(), IconName::Inbox))
            .children(self.list_names.iter().map(|(id, name)| row(Some(Some(*id)), name.clone(), IconName::List)))
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.select_day(date, cx);
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
                    .children(tasks.iter().take(visible).map(|task| self.task_bar(task, Some(date), cx)))
                    .children(events.into_iter().take(event_room).map(|event| self.event_bar(event, date, cx)))
                    .children(extras.into_iter().take(room))
                    .when(hidden > 0 && slots > 0, |el| {
                        el.child(h_flex().h(px(DAY_ITEM_HEIGHT)).flex_none().px_1()
                            .child(Label::new(format!("+{hidden}")).size(LabelSize::XSmall).color(Color::Muted)))
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
        let theme = cx.theme().clone();
        let id = task.id;
        // Keep closed titles muted; the checkbox distinguishes completed and dismissed tasks.
        let done = task.is_closed();
        let tint = self.task_tint(task, &theme);
        let modern = self.options.modern;
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        h_flex()
            .id(SharedString::from(format!("calendar-task-{id}")))
            .debug_selector(|| format!("calendar-task-{id}"))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .h(px(18.))
            .flex_none()
            .gap_1()
            .px_1()
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
            .child(
                Checkbox::new("calendar-task-checkbox")
                    .size(px(12.))
                    .toggle_state(task.is_completed())
                    .dismissed(task.is_wont_do()),
            )
            .child(
                Label::new(if task.title.trim().is_empty() { i18n::t(cx).untitled.to_owned() } else { task.title.clone() })
                    .size(LabelSize::XSmall)
                    .color(if done { Color::Muted } else { Color::Default })
                    .truncate(),
            )
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

    fn render_week(&self, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let week = self.shown_days();
        let gutter = px(56.);
        let line = theme.colors().border_variant;

        // All-day strip: day numbers and tasks without a time.
        let all_day = h_flex()
            .flex_none()
            .items_start()
            .min_h(px(64.))
            .border_b_1()
            .border_color(line)
            .child(div().w(gutter).flex_none())
            .children(week.iter().copied().map(|date| {
                let accent = theme.colors().accent;
                let tasks: Vec<AnyElement> = self
                    .tasks
                    .iter()
                    .filter(|t| self.options.shows_task(t, date) && t.due_time.is_none())
                    .map(|t| self.task_bar(t, Some(date), cx).into_any_element())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .chain(self.events_on(date, cx).into_iter().filter(|e| e.start_time.is_none()).map(|e| self.event_bar(e, date, cx).into_any_element()).collect::<Vec<_>>())
                    .chain(self.day_extras(date, cx))
                    .collect();
                v_flex()
                    .id(SharedString::from(format!("week-allday-{date}")))
                    .flex_1()
                    .min_w_0()
                    .self_stretch()
                    .gap(px(2.))
                    .px(px(2.))
                    .pb_1()
                    .border_l_1()
                    .border_color(line)
                    .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.12)))
                    .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on_day(dragged.id, date, cx)))
                    .child(
                        h_flex().h(px(26.)).px_1().child(
                            div()
                                .id(SharedString::from(format!("calendar-select-day-{date}")))
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
                                .on_click(cx.listener(move |this, _, _, cx| this.select_day(date, cx)))
                                .child(Label::new(date.day().to_string()).centered().weight(FontWeight::SEMIBOLD)
                                    .when(date == today, |label| label.color(Color::OnAccent))),
                        ),
                    )
                    .children(tasks)
            }));

        // Time axis labels and hour lines.
        let hour_label = |hour: u32| -> SharedString {
            if hour == 12 { strings.noon.into() } else { calendar::format_hour(hour, strings).into() }
        };
        let now_color = theme.status().error;
        let mut axis = v_flex().w(gutter).flex_none().child(
            div()
                .h(px(week_axis::FOLDED_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.colors().panel_background)
                .flex_col()
                .child(Label::new(calendar::format_hour(0, strings)).size(LabelSize::XSmall).color(Color::Muted))
                .child(Label::new(format!("– {}", calendar::format_hour(7, strings))).size(LabelSize::XSmall).color(Color::Muted)),
        );
        for hour in week_axis::FIRST_HOUR..week_axis::LAST_HOUR {
            axis = axis.child(
                div()
                    .h(px(week_axis::HOUR_HEIGHT))
                    .px_1()
                    .flex()
                    .justify_end()
                    .child(Label::new(hour_label(hour)).size(LabelSize::XSmall).color(Color::Muted)),
            );
        }
        axis = axis.child(
            div()
                .h(px(week_axis::FOLDED_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.colors().panel_background)
                .flex_col()
                .child(Label::new(calendar::format_hour(21, strings)).size(LabelSize::XSmall).color(Color::Muted))
                .child(Label::new(format!("– {}", calendar::format_hour(0, strings))).size(LabelSize::XSmall).color(Color::Muted)),
        );

        let columns = week.iter().copied().map(|date| {
            let accent = theme.colors().accent;
            let timed: Vec<AnyElement> = self
                .tasks
                .iter()
                .filter(|t| self.options.show_completed || !t.is_completed())
                .filter_map(|t| calendar::timed_segment(t, date).map(|segment| (t, segment)))
                .map(|(t, (from, to))| {
                    // A span is drawn as a block from its start to its end; a point in time as a bar.
                    let height = (week_axis::offset(to) - week_axis::offset(from)).max(18.);
                    div()
                        .absolute()
                        .top(px(week_axis::offset(from)))
                        .left(px(2.))
                        .right(px(2.))
                        .h(px(height))
                        .child(self.task_bar(t, Some(date), cx).h(px(height)).items_start().pt(px(1.)))
                        .into_any_element()
                })
                .collect();
            // The subscribed calendars' timed events, as blocks from their start to their end.
            let timed_events: Vec<AnyElement> = self
                .events_on(date, cx)
                .into_iter()
                .filter_map(|event| {
                    let from = if event.start == date { event.start_time? } else { chrono::NaiveTime::MIN };
                    let to = match event.end_time {
                        Some(to) if event.end == date => to,
                        _ if event.end > date => chrono::NaiveTime::from_hms_opt(23, 59, 0)?,
                        _ => from + chrono::Duration::minutes(30),
                    };
                    let height = (week_axis::offset(to) - week_axis::offset(from)).max(18.);
                    Some(
                        div()
                            .absolute()
                            .top(px(week_axis::offset(from)))
                            .left(px(2.))
                            .right(px(2.))
                            .h(px(height))
                            .child(self.event_bar(event, date, cx).h(px(height)).items_start().pt(px(1.)))
                            .into_any_element(),
                    )
                })
                .collect();
            let mut hour_lines = Vec::new();
            for hour in week_axis::FIRST_HOUR..=week_axis::LAST_HOUR {
                let time = chrono::NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or_default();
                hour_lines.push(
                    div().absolute().top(px(week_axis::offset(time))).left_0().right_0().h(px(1.)).bg(line).into_any_element(),
                );
            }
            div()
                .id(SharedString::from(format!("week-day-{date}")))
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(week_axis::total_height()))
                .border_l_1()
                .border_color(line)
                .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.08)))
                .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on_day(dragged.id, date, cx)))
                .children(hour_lines)
                .children(timed)
                .children(timed_events)
                // The original's red line at the current time, with a dot where it starts.
                .when(date == today && week_axis::unfolded(Local::now().time()), |el| {
                    let top = week_axis::offset(Local::now().time());
                    el.child(div().absolute().top(px(top - 1.)).left_0().right_0().h(px(2.)).bg(now_color)).child(
                        div().absolute().top(px(top - 4.)).left(px(-4.)).size(px(8.)).rounded_full().bg(now_color),
                    )
                })
        });

        v_flex()
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(line)
            .child(all_day)
            .child(
                div()
                    .id("week-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        h_flex()
                            .items_start()
                            .child(
                                // The current time in red on the axis, as in the original.
                                div().relative().child(axis).when(week.contains(&today) && week_axis::unfolded(Local::now().time()), |el| {
                                    let now = Local::now().time();
                                    el.child(
                                        div()
                                            .absolute()
                                            .top(px(week_axis::offset(now) - 7.))
                                            .right(px(4.))
                                            .px_0p5()
                                            .bg(theme.colors().background)
                                            .child(
                                                Label::new(calendar::format_time(now, strings))
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Custom(now_color)),
                                            ),
                                    )
                                }),
                            )
                            .children(columns),
                    ),
            )
            .into_any_element()
    }

    /// The original's Agenda: a row per day — the day, "All Day" or the time, a circle and a
    /// card per task — with a heading where a new month starts, and today always shown.
    fn render_agenda(&self, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let start = self.agenda_start();
        let mut days = self.agenda.clone();
        if start == today && days.first().is_none_or(|(date, _)| *date != today) {
            days.insert(0, (today, Vec::new()));
        }
        // The subscribed calendars' events on their first day (or the Agenda's first, for one
        // begun before), with a day of their own where there are no tasks.
        let mut events: std::collections::BTreeMap<NaiveDate, Vec<EventBar>> = Default::default();
        for event in self.events_between(start, start + chrono::Duration::days(AGENDA_DAYS), cx) {
            events.entry(event.start.max(start)).or_default().push(event);
        }
        for date in events.keys() {
            if !days.iter().any(|(day, _)| day == date) {
                days.push((*date, Vec::new()));
            }
        }
        days.sort_by_key(|(date, _)| *date);
        let mut month = calendar::month_start(start);
        let mut rows: Vec<AnyElement> = Vec::new();
        for (date, items) in days {
            if calendar::month_start(date) != month {
                month = calendar::month_start(date);
                rows.push(
                    div()
                        .pt(px(28.))
                        .pb(px(10.))
                        .text_size(px(18.))
                        .font_weight(FontWeight::BOLD)
                        .child(calendar::month_title(month, strings))
                        .into_any_element(),
                );
            }
            let is_today = date == today;
            // The original's day label: the number semibold, the weekday beside it on its baseline
            // (the boxes' bottoms, less the number's deeper descent).
            let day = h_flex()
                .w(px(120.))
                .flex_none()
                .gap_2()
                .items_end()
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if is_today { theme.colors().text_accent } else { theme.colors().text })
                        .child(date.day().to_string()),
                )
                .child(
                    div().pb(px(4.)).child(
                        Label::new(strings.weekdays_short[date.weekday().num_days_from_monday() as usize])
                            .size(LabelSize::XSmall)
                            .weight(FontWeight::MEDIUM)
                            .color(if is_today { Color::Accent } else { Color::Muted }),
                    ),
                );
            let day_events = events.remove(&date).unwrap_or_default();
            let entries: Vec<AnyElement> = if items.is_empty() && day_events.is_empty() {
                vec![
                    h_flex()
                        .gap_3()
                        .child(div().w(px(72.)).flex_none().child(Label::new(strings.range_all_day).size(LabelSize::Small).color(Color::Muted)))
                        .child(div().size(px(10.)).flex_none().rounded_full().bg(theme.colors().element_hover))
                        // The original's banner: a light sky with a sun, drawn for Fern rather than
                        // copied, the words dark on it.
                        .child(
                            h_flex()
                                .flex_1()
                                .h(px(64.))
                                .px_5()
                                .rounded(px(8.))
                                .bg(gpui::linear_gradient(
                                    90.,
                                    gpui::linear_color_stop(gpui::rgb(0xdbe9f7), 0.),
                                    gpui::linear_color_stop(gpui::rgb(0x9fc7ee), 1.),
                                ))
                                .child(div().flex_1().text_color(gpui::rgb(0x334155)).child(Label::new(strings.no_tasks_today).color(Color::Custom(gpui::rgb(0x334155).into()))))
                                .child(
                                    div()
                                        .size(px(44.))
                                        .mr(px(80.))
                                        .rounded_full()
                                        .bg(gpui::linear_gradient(
                                            180.,
                                            gpui::linear_color_stop(gpui::rgb(0xfde68a), 0.),
                                            gpui::linear_color_stop(gpui::rgb(0xf59e0b), 1.),
                                        )),
                                ),
                        )
                        .into_any_element(),
                ]
            } else {
                items
                    .into_iter()
                    .enumerate()
                    .map(|(i, item)| {
                        let id = item.task;
                        // Countdowns, as in the original: an orange card and a filled dot instead of
                        // the task's circle; a click shows the countdown's card.
                        let countdown = item.countdown;
                        let item_color = item.color.clone();
                        let tint = if countdown {
                            theme.status().warning
                        } else {
                            task_rows::priority_color(item.priority, &theme).unwrap_or(theme.colors().accent)
                        };
                        let when = item.time.map_or(strings.range_all_day.to_owned(), |time| calendar::format_time(time, strings));
                        h_flex()
                            .gap_3()
                            .child(div().w(px(72.)).flex_none().child(Label::new(when).size(LabelSize::Small).color(Color::Muted)))
                            // As in the original: a countdown's round icon, a repeating task's repeat
                            // mark, or a task's circle.
                            .child(div().w(px(16.)).flex_none().flex().justify_center().child(if countdown {
                                div()
                                    .size(px(16.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(crate::countdown_view::countdown_color(item_color.as_deref(), cx).opacity(0.5))
                                    .text_size(px(9.))
                                    .child(match item.icon.clone() {
                                        Some(icon) => icon.into_any_element(),
                                        None => Icon::new(crate::countdown_view::kind_icon(item.kind.unwrap_or_default()))
                                            .size(IconSize::XSmall)
                                            .color(Color::Default)
                                            .into_any_element(),
                                    })
                                    .into_any_element()
                            } else if item.repeats {
                                Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Muted).into_any_element()
                            } else {
                                div().size(px(10.)).rounded_full().border_1().border_color(theme.colors().text_muted).into_any_element()
                            }))
                            .child(
                                h_flex()
                                    .id(SharedString::from(format!("agenda-{date}-{i}")))
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(40.))
                                    .pl_3()
                                    .pr_3()
                                    .gap_2()
                                    .rounded(px(6.))
                                    .border_l_3()
                                    .border_color(tint)
                                    .bg(tint.opacity(0.12))
                                    .hover(|s| s.bg(tint.opacity(0.2)))
                                    .cursor_pointer()
                                    .child(div().flex_1().min_w_0().child(Label::new(if item.title.trim().is_empty() { strings.untitled.to_owned() } else { item.title }).truncate()))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if countdown {
                                            this.open_countdown = Some((date, id));
                                            window.focus(&this.focus_handle, cx);
                                            cx.notify();
                                        } else {
                                            cx.emit(OpenTask(id))
                                        }
                                    }))
                                    .when(!countdown, |el| {
                                        el.on_mouse_down(
                                            gpui::MouseButton::Right,
                                            cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                                                this.open_context(id, event.position, cx);
                                                window.focus(&this.focus_handle, cx);
                                            }),
                                        )
                                    })
                                    .when_some(
                                        self.open_countdown
                                            .filter(|open| *open == (date, id))
                                            .and_then(|_| self.countdowns.iter().find(|c| c.id == id)),
                                        |el, shown| {
                                            el.relative().child(PopoverAttachment::new(
                                                Placement::BelowRight,
                                                popover(cx)
                                                    .w(px(220.))
                                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                                        this.open_countdown = None;
                                                        cx.notify();
                                                    }))
                                                    .child(crate::countdown_view::render_summary(shown, date, cx))
                                                    .into_any_element(),
                                            ))
                                        },
                                    ),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .chain(day_events.into_iter().map(|event| self.agenda_event_row(event, date, cx)))
                    .collect()
            };
            rows.push(
                h_flex()
                    .py(px(30.))
                    .items_start()
                    .child(div().pt(px(6.)).child(day))
                    .child(v_flex().flex_1().gap_2().children(entries))
                    .into_any_element(),
            );
        }
        v_flex()
            .id("agenda")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pl(px(185.))
            .pr(px(60.))
            .pb_6()
            // The original's agenda: 185 px in from the left, up to 1030 px wide, each day 100 px
            // apart.
            .child(v_flex().pt(px(14.)).max_w(px(1030.)).children(rows))
            .into_any_element()
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

    fn render_month_grid(&self, today: NaiveDate, window: &Window, cx: &mut Context<Self>) -> Div {
        let colors = cx.theme().colors();
        let shown_weeks = self.month_grid_weeks();
        let grid_height = window.viewport_size().height - crate::title_bar::header_inset()
            - crate::title_bar::frame_vertical_inset(window)
            - px(CALENDAR_HEADER_HEIGHT + WEEKDAY_HEADER_HEIGHT + 1.);
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
        let toolbar = h_flex()
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
                div().flex_1().child(
                    Label::new(match self.mode {
                        // The original's Year shows just the year.
                        CalendarMode::Year => self.month.year().to_string(),
                        mode => calendar::month_title(
                            match mode {
                                CalendarMode::Month | CalendarMode::Agenda => self.month,
                                CalendarMode::Week | CalendarMode::MultiWeek(_) => calendar::week_of(self.anchor)[0],
                                _ => self.anchor,
                            },
                            strings,
                        ),
                    })
                        .size(LabelSize::Large)
                        .weight(FontWeight::SEMIBOLD),
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
                        // An open editor handles this as an outside click. Do not also
                        // register a click that would reopen it on mouse-up.
                        .when(self.task_editor.is_none(), |button| {
                            button.on_click(cx.listener(|this, _, window, cx| this.open_task_editor(window, cx)))
                        }),
                ).when_some(self.task_editor.as_ref().filter(|editor| editor.day.is_none()), |el, editor| {
                    el.child(PopoverAttachment::new(Placement::BelowRight, editor.view.clone()))
                }),
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
                            .child(Label::new(strings.today))
                            .on_click(cx.listener(|this, _, _, cx| this.go_to_today(cx))),
                    )
                    .child(
                        div().debug_selector(|| "calendar-next".to_owned()).child(
                            IconButton::new("calendar-next", IconName::ChevronRight)
                                .size(px(28.))
                                .icon_size(IconSize::Small)
                                .when(!self.can_shift(1), |el| el.icon_color(Color::Placeholder))
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
        let calendar = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .when(!matches!(self.mode, CalendarMode::Agenda | CalendarMode::Year), |el| el.child(
                h_flex()
                    .flex_none()
                    .h(px(WEEKDAY_HEADER_HEIGHT))
                    .when(self.mode.is_timed(), |el| el.child(div().w(px(56.)).flex_none()))
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
                            .flex()
                            .justify_center()
                            .child(Label::new(day).size(LabelSize::Small).color(Color::Muted))
                    })),
            ))
            .map(|el| match self.mode {
                CalendarMode::Agenda => el.child(self.render_agenda(today, cx)),
                CalendarMode::Year => el.child(self.render_year(today, window, cx)),
                CalendarMode::Month | CalendarMode::MultiWeek(_) => el.child(self.render_month_grid(today, window, cx)),
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
            .on_action(cx.listener(|this, _: &PreviousYear, _, cx| this.shift_year_using_keys(-1, cx)))
            .on_action(cx.listener(|this, _: &NextYear, _, cx| this.shift_year_using_keys(1, cx)))
            .on_action(cx.listener(|this, _: &CalendarToday, _, cx| this.go_to_today(cx)))
            .on_action(cx.listener(|this, _: &PrintYear, _, cx| this.print_year(cx)))
            .on_action(cx.listener(Self::dismiss_transient))
            .size_full()
            .bg(theme.colors().calendar_background)
            .text_color(theme.colors().text)
            .child(
                v_flex().flex_1().min_w_0().h_full()
                    .child(toolbar)
                    .child(
                        h_flex().flex_1().min_h_0()
                            .when(self.sidebar_open, |el| el.child(self.render_calendar_sidebar(cx)))
                            .child(calendar),
                    ),
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
    }
}

/// A subscribed calendar's event as the calendar draws it.
#[derive(Clone)]
struct EventBar {
    key: String,
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
}
