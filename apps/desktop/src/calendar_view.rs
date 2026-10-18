//! The calendar: tasks placed on their due dates in a month grid or a week with a time axis,
//! matched to TickTick's Calendar section.

use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Id, Store, Task};
use gpui::{EventEmitter, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, SubmenuItem, popover};

use crate::actions::{AgendaView, DayView, MonthView, NewTask, ToggleSidebar, WeekView, YearView};
use crate::calendar::{self, week_axis};
use crate::drag::DraggedTask;
use crate::i18n;
use crate::task_rows;

/// How many tasks a day cell shows before collapsing the rest into "+N".
const TASKS_PER_DAY: usize = 5;

/// The user clicked a task in the calendar.

/// The week-number column of the month grid.
const WEEK_NUMBER_WIDTH: f32 = 32.;
pub struct OpenTask(pub Id);

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

/// The Multi-Week counts the menu's stepper goes through.
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
    mode_menu_open: bool,
    focus_handle: gpui::FocusHandle,
    /// The mode menu's Multi-Day side menu, while hovered open.
    multi_day_menu: bool,
    /// The weeks Multi-Week shows, set with the menu's stepper; and whether its row is hovered,
    /// which shows the stepper, as in the original.
    multi_weeks: u8,
    multi_week_hovered: bool,
    tasks: Vec<Task>,
    /// The Agenda's days, from today (or the shown month's first day, when later) a year on.
    agenda: Vec<(NaiveDate, Vec<fern_core::agenda::AgendaItem>)>,
}

/// How far ahead the Agenda reaches.
const AGENDA_DAYS: i64 = 366;

impl EventEmitter<OpenTask> for CalendarView {}

impl gpui::Focusable for CalendarView {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl CalendarView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let today = Local::now().date_naive();
        let mut this = Self {
            store,
            mode: CalendarMode::Month,
            month: calendar::month_start(today),
            anchor: today,
            mode_menu_open: false,
            focus_handle: cx.focus_handle(),
            multi_day_menu: false,
            multi_weeks: 5,
            multi_week_hovered: false,
            tasks: Vec::new(),
            agenda: Vec::new(),
        };
        this.reload(cx);
        this
    }

    /// The days on the hour axis: the anchor's week, the anchor, or several days from it.
    fn shown_days(&self) -> Vec<NaiveDate> {
        match self.mode {
            CalendarMode::Day => vec![self.anchor],
            CalendarMode::MultiDay(n) => (0..n as i64).map(|i| self.anchor + chrono::Duration::days(i)).collect(),
            _ => calendar::week_of(self.anchor).to_vec(),
        }
    }

    /// Re-reads the tasks of the shown days.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.mode == CalendarMode::Agenda {
            let from = self.agenda_start();
            let countdowns = self.store.countdowns().unwrap_or_default();
            match self.store.tasks() {
                Ok(tasks) => {
                    self.agenda =
                        fern_core::agenda::agenda(&tasks, &countdowns, from, from + chrono::Duration::days(AGENDA_DAYS))
                }
                Err(err) => eprintln!("failed to load the agenda: {err:#}"),
            }
            cx.notify();
            return;
        }
        let (start, end) = match self.mode {
            CalendarMode::MultiWeek(n) => {
                let weeks = calendar::weeks_from(self.anchor, n as usize);
                (weeks[0][0], weeks[weeks.len() - 1][6])
            }
            // The original's Year shows no tasks.
            CalendarMode::Year => {
                self.tasks.clear();
                cx.notify();
                return;
            }
            CalendarMode::Month => {
                let weeks = calendar::month_weeks(self.month);
                (weeks[0][0], weeks[weeks.len() - 1][6])
            }
            _ => {
                let days = self.shown_days();
                (days[0], days[days.len() - 1])
            }
        };
        match self.store.tasks_due_between(start, end) {
            Ok(tasks) => self.tasks = tasks,
            Err(err) => eprintln!("failed to load calendar tasks: {err:#}"),
        }
        cx.notify();
    }

    /// The Agenda starts today in the current month, and on the first day of a later one.
    fn agenda_start(&self) -> NaiveDate {
        Local::now().date_naive().max(self.month)
    }

    fn shift(&mut self, delta: i32, cx: &mut Context<Self>) {
        match self.mode {
            CalendarMode::Month | CalendarMode::Agenda => self.month = calendar::add_months(self.month, delta),
            CalendarMode::Year => self.month = calendar::add_months(self.month, 12 * delta),
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
        self.reload(cx);
    }

    pub fn set_mode(&mut self, mode: CalendarMode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.mode_menu_open = false;
        if (mode.is_timed() || matches!(mode, CalendarMode::MultiWeek(_))) && calendar::month_start(self.anchor) != self.month {
            self.anchor = self.month;
        }
        self.reload(cx);
    }

    /// The mode menu with its Multi-Day side menu open, for the debug-build dev state.
    #[cfg(debug_assertions)]
    pub fn dev_open_menu(&mut self, cx: &mut Context<Self>) {
        self.mode_menu_open = true;
        self.multi_day_menu = true;
        cx.notify();
    }

    /// Moves a dropped task to `date`, keeping its time of day.
    fn drop_on_day(&mut self, id: Id, date: NaiveDate, cx: &mut Context<Self>) {
        let time = self.store.task(id).ok().flatten().and_then(|t| t.due_time);
        if let Err(err) = self.store.set_due(id, Some(date), time) {
            eprintln!("failed to move the task: {err:#}");
        }
        self.reload(cx);
    }

    fn go_to_today(&mut self, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        self.month = calendar::month_start(today);
        self.anchor = today;
        self.reload(cx);
    }

    fn render_day(&self, date: NaiveDate, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let tasks: Vec<&Task> = self.tasks.iter().filter(|t| t.occurs_on(date)).collect();
        let hidden = tasks.len().saturating_sub(TASKS_PER_DAY);
        // The first day of a month carries the month name, as in the original: "Oct 1".
        let number = if date.day() == 1 {
            let month = strings.months_short[date.month0() as usize];
            if strings.day_before_month { format!("1 {month}") } else { format!("{month} 1") }
        } else {
            date.day().to_string()
        };

        let accent = theme.colors().accent;
        v_flex()
            .id(SharedString::from(format!("calendar-day-{date}")))
            .flex_1()
            .min_w_0()
            .h_full()
            .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.12)))
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on_day(dragged.id, date, cx)))
            .gap(px(2.))
            .px(px(2.))
            .pt_1()
            .border_r_1()
            .border_b_1()
            .border_color(theme.colors().border_variant)
            .child(
                h_flex().h(px(26.)).px_1().child(
                    div()
                        .min_w(px(22.))
                        .h(px(22.))
                        .px_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .when(date == today, |el| el.bg(theme.colors().accent))
                        .child(Label::new(number).weight(FontWeight::SEMIBOLD)),
                ),
            )
            .children(tasks.iter().take(TASKS_PER_DAY).map(|task| {
                let id = task.id;
                let done = task.is_completed();
                let tint = task_rows::priority_color(task.priority, &theme).unwrap_or(theme.colors().accent);
                let dragged = DraggedTask { id, title: task.title.clone().into() };
                h_flex()
                    .id(SharedString::from(format!("calendar-task-{id}")))
                    .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                    .h(px(18.))
                    .flex_none()
                    .gap_1()
                    .px_1()
                    .rounded(px(3.))
                    .cursor_pointer()
                    .bg(tint.opacity(if done { 0.12 } else { 0.28 }))
                    .hover(|s| s.bg(tint.opacity(0.4)))
                    .child(
                        div()
                            .size(px(10.))
                            .flex_none()
                            .rounded(px(2.))
                            .border_1()
                            .border_color(theme.colors().text_muted)
                            .when(done, |el| el.bg(theme.colors().text_muted)),
                    )
                    .child(
                        Label::new(task.title.clone())
                            .size(LabelSize::XSmall)
                            .color(if done { Color::Muted } else { Color::Default })
                            .strikethrough(done)
                            .truncate(),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenTask(id))))
            }))
            .when(hidden > 0, |el| {
                el.child(div().px_1().child(Label::new(format!("+{hidden}")).size(LabelSize::XSmall).color(Color::Muted)))
            })
            .into_any_element()
    }

    fn task_bar(&self, task: &Task, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme().clone();
        let id = task.id;
        let done = task.is_completed();
        let tint = task_rows::priority_color(task.priority, &theme).unwrap_or(theme.colors().accent);
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        h_flex()
            .id(SharedString::from(format!("calendar-task-{id}")))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .h(px(18.))
            .flex_none()
            .gap_1()
            .px_1()
            .rounded(px(3.))
            .cursor_pointer()
            .bg(tint.opacity(if done { 0.12 } else { 0.28 }))
            .hover(|s| s.bg(tint.opacity(0.4)))
            .child(
                div()
                    .size(px(10.))
                    .flex_none()
                    .rounded(px(2.))
                    .border_1()
                    .border_color(theme.colors().text_muted)
                    .when(done, |el| el.bg(theme.colors().text_muted)),
            )
            .child(
                Label::new(task.title.clone())
                    .size(LabelSize::XSmall)
                    .color(if done { Color::Muted } else { Color::Default })
                    .strikethrough(done)
                    .truncate(),
            )
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenTask(id))))
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
                    .filter(|t| t.occurs_on(date) && t.due_time.is_none())
                    .map(|t| self.task_bar(t, cx).into_any_element())
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
                                .min_w(px(22.))
                                .h(px(22.))
                                .px_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .when(date == today, |el| el.bg(theme.colors().accent))
                                .child(Label::new(date.day().to_string()).weight(FontWeight::SEMIBOLD)),
                        ),
                    )
                    .children(tasks)
            }));

        // Time axis labels and hour lines.
        let hour_label = |hour: u32| -> SharedString {
            if hour == 12 { strings.noon.into() } else { calendar::format_hour(hour).into() }
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
                .child(Label::new(calendar::format_hour(0)).size(LabelSize::XSmall).color(Color::Muted))
                .child(Label::new(format!("– {}", calendar::format_hour(7))).size(LabelSize::XSmall).color(Color::Muted)),
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
                .child(Label::new(calendar::format_hour(21)).size(LabelSize::XSmall).color(Color::Muted))
                .child(Label::new(format!("– {}", calendar::format_hour(0))).size(LabelSize::XSmall).color(Color::Muted)),
        );

        let columns = week.iter().copied().map(|date| {
            let accent = theme.colors().accent;
            let timed: Vec<AnyElement> = self
                .tasks
                .iter()
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
                        .child(self.task_bar(t, cx).h(px(height)).items_start().pt(px(1.)))
                        .into_any_element()
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
                // The original's red line at the current time, with a dot where it starts.
                .when(date == today, |el| {
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
                                div().relative().child(axis).when(week.contains(&today), |el| {
                                    let now = Local::now().time();
                                    el.child(
                                        div()
                                            .absolute()
                                            .top(px(week_axis::offset(now) - 7.))
                                            .right(px(4.))
                                            .px_0p5()
                                            .bg(theme.colors().background)
                                            .child(
                                                Label::new(calendar::format_time(now))
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
            let day = h_flex()
                .w(px(120.))
                .flex_none()
                .gap_2()
                .items_baseline()
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(if is_today { theme.colors().accent } else { theme.colors().text })
                        .child(date.day().to_string()),
                )
                .child(
                    Label::new(strings.weekdays_short[date.weekday().num_days_from_monday() as usize])
                        .size(LabelSize::XSmall)
                        .color(if is_today { Color::Accent } else { Color::Muted }),
                );
            let entries: Vec<AnyElement> = if items.is_empty() {
                vec![
                    h_flex()
                        .gap_3()
                        .child(div().w(px(72.)).flex_none().child(Label::new(strings.range_all_day).size(LabelSize::Small).color(Color::Muted)))
                        .child(div().size(px(10.)).flex_none().rounded_full().bg(theme.colors().element_hover))
                        .child(
                            div()
                                .flex_1()
                                .h(px(52.))
                                .px_4()
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .bg(theme.colors().accent.opacity(0.18))
                                .child(Label::new(strings.no_tasks_today).size(LabelSize::Small)),
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
                        // the task's circle; they do not open a task.
                        let countdown = item.countdown;
                        let tint = if countdown {
                            theme.status().warning
                        } else {
                            task_rows::priority_color(item.priority, &theme).unwrap_or(theme.colors().accent)
                        };
                        let when = item.time.map_or(strings.range_all_day.to_owned(), calendar::format_time);
                        h_flex()
                            .gap_3()
                            .child(div().w(px(72.)).flex_none().child(Label::new(when).size(LabelSize::Small).color(Color::Muted)))
                            .child(
                                div()
                                    .size(px(10.))
                                    .flex_none()
                                    .rounded_full()
                                    .when(countdown, |el| el.bg(theme.colors().accent))
                                    .when(!countdown, |el| el.border_1().border_color(theme.colors().text_muted)),
                            )
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
                                    .when(!countdown, |el| el.hover(|s| s.bg(tint.opacity(0.2))).cursor_pointer())
                                    .child(div().flex_1().min_w_0().child(Label::new(item.title).size(LabelSize::Small).truncate()))
                                    .when(item.repeats, |el| {
                                        el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Muted))
                                    })
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        if !countdown {
                                            cx.emit(OpenTask(id))
                                        }
                                    })),
                            )
                            .into_any_element()
                    })
                    .collect()
            };
            rows.push(
                h_flex()
                    .py(px(20.))
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
            .px(px(60.))
            .pb_6()
            .child(v_flex().pt(px(14.)).max_w(px(760.)).children(rows))
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
            .w(px(170.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.mode_menu_open = false;
                cx.notify();
            }))
            .children(items.into_iter().map(|(mode, label, hint)| {
                let selected = self.mode == mode;
                ListItem::new(label)
                    .start_slot(div().w(px(14.)).when(selected, |el| el.child(Label::new("✓").color(Color::Accent))))
                    .child(Label::new(label).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default }))
                    .end_slot(Label::new(hint).size(LabelSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_mode(mode, cx)))
            }))
            .child(div().py_1().child(Divider::horizontal()))
            // The original's Multi-Day, with the number of days in a side menu.
            .child(
                SubmenuItem::new("multi-day", strings.multi_day)
                    .open(self.multi_day_menu)
                    .placement(Placement::LeftSide)
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered && !this.multi_day_menu {
                            this.multi_day_menu = true;
                            cx.notify();
                        }
                    }))
                    .items((2..=6u8).map(|n| {
                        let selected = self.mode == CalendarMode::MultiDay(n);
                        ListItem::new(SharedString::from(format!("multi-day-{n}")))
                            .child(Label::new(strings.n_days.replace("{n}", &n.to_string())).color(if selected { Color::Accent } else { Color::Default }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.multi_day_menu = false;
                                this.set_mode(CalendarMode::MultiDay(n), cx)
                            }))
                    }))

            )
            .child(self.render_multi_week_item(cx))
            .into_any_element()
    }

    /// The menu's Multi-Week row: the week count, and while hovered a "− n +" stepper, as in the
    /// original. Clicking the row shows that many weeks.
    fn render_multi_week_item(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let n = self.multi_weeks;
        let selected = matches!(self.mode, CalendarMode::MultiWeek(_));
        let step = |id: &'static str, icon: IconName, delta: i8| {
            IconButton::new(id, icon).size(px(20.)).icon_size(IconSize::XSmall).on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                let n = (this.multi_weeks as i8 + delta).clamp(*MULTI_WEEKS.start() as i8, *MULTI_WEEKS.end() as i8) as u8;
                this.multi_weeks = n;
                if matches!(this.mode, CalendarMode::MultiWeek(_)) {
                    this.mode = CalendarMode::MultiWeek(n);
                    this.reload(cx);
                }
                cx.notify();
            }))
        };
        h_flex()
            .id("multi-week")
            .h(px(32.))
            .mx_1()
            .px_2()
            .gap_2()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.colors().element_hover))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.multi_week_hovered = *hovered;
                cx.notify();
            }))
            .child(div().w(px(14.)).when(selected, |el| el.child(Label::new("✓").color(Color::Accent))))
            .child(
                div().flex_1().child(
                    Label::new(strings.multi_week).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default }),
                ),
            )
            .map(|el| {
                if self.multi_week_hovered {
                    el.child(step("multi-week-fewer", IconName::Minus, -1))
                        .child(Label::new(n.to_string()).size(LabelSize::Small))
                        .child(step("multi-week-more", IconName::Plus, 1))
                } else {
                    el.child(Label::new(strings.n_weeks.replace("{n}", &n.to_string())).size(LabelSize::XSmall).color(Color::Muted))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| this.set_mode(CalendarMode::MultiWeek(this.multi_weeks), cx)))
            .into_any_element()
    }

    /// The original's Year: the twelve months in three columns, today marked; a click on a day
    /// opens it in the Day view.
    fn render_year(&self, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let year = self.month.year();
        let months = (1..=12u32).map(|m| {
            let month = NaiveDate::from_ymd_opt(year, m, 1).expect("every month has a first day");
            let days = calendar::month_grid(month);
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().pb_3().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(strings.months[month.month0() as usize]))
                .child(h_flex().children(calendar::weekday_labels(&strings.weekdays_narrow).map(|day| {
                    div().flex_1().h(px(36.)).flex().items_center().justify_center().child(Label::new(day).size(LabelSize::Small).color(Color::Muted))
                })))
                .children(days.chunks(7).map(|week| {
                    h_flex().children(week.iter().map(|&date| {
                        let outside = date.month() != month.month();
                        div()
                            .id(SharedString::from(format!("year-{month}-{date}")))
                            .flex_1()
                            .h(px(41.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .child(
                                div()
                                    .size(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .when(date == today && !outside, |el| el.bg(theme.colors().accent))
                                    .hover(|s| s.bg(theme.colors().element_hover))
                                    .child(Label::new(date.day().to_string()).color(if outside { Color::Muted } else { Color::Default })),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.anchor = date;
                                this.month = calendar::month_start(date);
                                this.set_mode(CalendarMode::Day, cx);
                            }))
                    }))
                }))
        });
        let mut rows = Vec::new();
        let mut months = months.peekable();
        while months.peek().is_some() {
            rows.push(h_flex().gap(px(40.)).children(months.by_ref().take(3)));
        }
        v_flex()
            .id("year")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(60.))
            .pt_2()
            .pb_6()
            .gap(px(28.))
            .children(rows)
            .into_any_element()
    }
}

impl Render for CalendarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let shown_weeks = match self.mode {
            CalendarMode::MultiWeek(n) => calendar::weeks_from(self.anchor, n as usize),
            _ => calendar::month_weeks(self.month),
        };
        let weeks: Vec<AnyElement> = shown_weeks
            .into_iter()
            .map(|week| {
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
                    .children(week.map(|date| self.render_day(date, today, cx)))
                    .into_any_element()
            })
            .collect();
        let bordered = |id: &'static str| {
            h_flex()
                .id(id)
                .h(px(28.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border)
                .bg(theme.colors().surface_background)
        };

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("Calendar")
            // The view menu's keys, as in the original: D/1, W/2, M/3.
            .on_action(cx.listener(|this, _: &DayView, _, cx| this.set_mode(CalendarMode::Day, cx)))
            .on_action(cx.listener(|this, _: &WeekView, _, cx| this.set_mode(CalendarMode::Week, cx)))
            .on_action(cx.listener(|this, _: &MonthView, _, cx| this.set_mode(CalendarMode::Month, cx)))
            .on_action(cx.listener(|this, _: &AgendaView, _, cx| this.set_mode(CalendarMode::Agenda, cx)))
            .on_action(cx.listener(|this, _: &YearView, _, cx| this.set_mode(CalendarMode::Year, cx)))
            .size_full()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(
                h_flex()
                    .h(px(50.))
                    .flex_none()
                    .pl(px(16.))
                    .pr(px(12.))
                    .gap_2()
                    .child(
                        IconButton::new("calendar-toggle-sidebar", IconName::PanelLeft)
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleSidebar), cx)),
                    )
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
                                .weight(FontWeight::BOLD),
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
                        bordered("calendar-add-frame").child(
                            IconButton::new("calendar-add", IconName::Plus)
                                .size(px(26.))
                                .icon_size(IconSize::Small)
                                .on_click(|_, window, cx| window.dispatch_action(Box::new(NewTask), cx)),
                        ),
                    )
                    .child(
                        div()
                            .relative()
                            .child(
                                bordered("calendar-mode")
                                    .px_2()
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
                                        })
                                        .size(LabelSize::Small),
                                    )
                                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.mode_menu_open = true;
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
                                IconButton::new("calendar-prev", IconName::ChevronLeft)
                                    .size(px(24.))
                                    .icon_size(IconSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| this.shift(-1, cx))),
                            )
                            .child(
                                div()
                                    .id("calendar-today")
                                    .px_1()
                                    .cursor_pointer()
                                    .child(Label::new(strings.today).size(LabelSize::Small))
                                    .on_click(cx.listener(|this, _, _, cx| this.go_to_today(cx))),
                            )
                            .child(
                                IconButton::new("calendar-next", IconName::ChevronRight)
                                    .size(px(24.))
                                    .icon_size(IconSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| this.shift(1, cx))),
                            ),
                    )
                    .child(IconButton::new("calendar-more", IconName::Ellipsis)),
            )
            .when(!matches!(self.mode, CalendarMode::Agenda | CalendarMode::Year), |el| el.child(
                h_flex()
                    .flex_none()
                    .h(px(22.))
                    .when(self.mode.is_timed(), |el| el.child(div().w(px(56.)).flex_none()))
                    .when(matches!(self.mode, CalendarMode::Month | CalendarMode::MultiWeek(_)) && calendar::week_numbers(), |el| {
                        el.child(div().w(px(WEEK_NUMBER_WIDTH)).flex_none())
                    })
                    // The days' weekday names: the week's, or those of the shown days.
                    .children(match self.mode {
                        CalendarMode::Month | CalendarMode::Week | CalendarMode::MultiWeek(_) => {
                            crate::calendar::weekday_labels(&strings.weekdays_short).to_vec()
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
                CalendarMode::Year => el.child(self.render_year(today, cx)),
                CalendarMode::Month | CalendarMode::MultiWeek(_) => el.child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .border_t_1()
                        .border_color(theme.colors().border_variant)
                        .children(weeks),
                ),
                _ => el.child(self.render_week(today, cx)),
            })
    }
}
