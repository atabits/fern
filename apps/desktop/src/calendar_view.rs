//! The calendar: tasks placed on their due dates in a month grid or a week with a time axis,
//! matched to TickTick's Calendar section.

use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Id, Store, Task};
use gpui::{EventEmitter, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, SubmenuItem, popover};

use crate::actions::{NewTask, ToggleSidebar};
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
}

impl CalendarMode {
    /// Whether the days are drawn on the hour axis (all but Month).
    pub fn is_timed(self) -> bool {
        self != CalendarMode::Month
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
    /// The mode menu's Multi-Day side menu, while hovered open.
    multi_day_menu: bool,
    tasks: Vec<Task>,
}

impl EventEmitter<OpenTask> for CalendarView {}

impl CalendarView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let today = Local::now().date_naive();
        let mut this = Self {
            store,
            mode: CalendarMode::Month,
            month: calendar::month_start(today),
            anchor: today,
            mode_menu_open: false,
            multi_day_menu: false,
            tasks: Vec::new(),
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
        let (start, end) = match self.mode {
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

    fn shift(&mut self, delta: i32, cx: &mut Context<Self>) {
        match self.mode {
            CalendarMode::Month => self.month = calendar::add_months(self.month, delta),
            mode => {
                let step = match mode {
                    CalendarMode::Day => 1,
                    CalendarMode::MultiDay(n) => n as i64,
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
        if mode.is_timed() && calendar::month_start(self.anchor) != self.month {
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
                    .child(h_flex().items_start().child(axis).children(columns)),
            )
            .into_any_element()
    }

    fn render_mode_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let items = [
            (CalendarMode::Day, strings.day, "D/1"),
            (CalendarMode::Week, strings.week, "W/2"),
            (CalendarMode::Month, strings.month, "M/3"),
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
            .into_any_element()
    }
}

impl Render for CalendarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let weeks: Vec<AnyElement> = calendar::month_weeks(self.month)
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
                            Label::new(calendar::month_title(
                                match self.mode {
                                    CalendarMode::Month => self.month,
                                    CalendarMode::Week => calendar::week_of(self.anchor)[0],
                                    _ => self.anchor,
                                },
                                strings,
                            ))
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
            .child(
                h_flex()
                    .flex_none()
                    .h(px(22.))
                    .when(self.mode.is_timed(), |el| el.child(div().w(px(56.)).flex_none()))
                    .when(self.mode == CalendarMode::Month && calendar::week_numbers(), |el| {
                        el.child(div().w(px(WEEK_NUMBER_WIDTH)).flex_none())
                    })
                    // The days' weekday names: the week's, or those of the shown days.
                    .children(match self.mode {
                        CalendarMode::Month | CalendarMode::Week => {
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
            )
            .map(|el| match self.mode {
                CalendarMode::Month => el.child(
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
