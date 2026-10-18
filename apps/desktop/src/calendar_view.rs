//! The month calendar: tasks placed on their due dates, modeled on TickTick's Calendar section.

use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Id, Store, Task};
use gpui::{EventEmitter, FontWeight};
use ui::prelude::*;

use crate::calendar;
use crate::i18n;
use crate::task_rows;

/// How many tasks a day cell shows before collapsing the rest into "+N".
const TASKS_PER_DAY: usize = 4;

/// The user clicked a task in the calendar.
pub struct OpenTask(pub Id);

pub struct CalendarView {
    store: Rc<Store>,
    month: NaiveDate,
    tasks: Vec<Task>,
}

impl EventEmitter<OpenTask> for CalendarView {}

impl CalendarView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let mut this = Self { store, month: calendar::month_start(Local::now().date_naive()), tasks: Vec::new() };
        this.reload(cx);
        this
    }

    /// Re-reads the tasks of the shown month (the grid also shows the edges of nearby months).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let grid = calendar::month_grid(self.month);
        let (start, end) = (grid[0], grid[grid.len() - 1]);
        match self.store.tasks_due_between(start, end) {
            Ok(tasks) => self.tasks = tasks,
            Err(err) => eprintln!("failed to load calendar tasks: {err:#}"),
        }
        cx.notify();
    }

    fn shift(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.month = calendar::add_months(self.month, delta);
        self.reload(cx);
    }

    fn go_to_today(&mut self, cx: &mut Context<Self>) {
        self.month = calendar::month_start(Local::now().date_naive());
        self.reload(cx);
    }

    fn render_day(&self, date: NaiveDate, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let in_month = date.month() == self.month.month();
        let tasks: Vec<&Task> = self.tasks.iter().filter(|t| t.due_date == Some(date)).collect();
        let hidden = tasks.len().saturating_sub(TASKS_PER_DAY);

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .p_1()
            .gap_0p5()
            .border_r_1()
            .border_b_1()
            .border_color(theme.colors().border_variant)
            .child(
                h_flex().justify_end().child(
                    div()
                        .size(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .when(date == today, |el| el.bg(theme.colors().accent))
                        .child(Label::new(date.day().to_string()).size(LabelSize::Small).color(
                            if date == today || in_month { Color::Default } else { Color::Muted },
                        )),
                ),
            )
            .children(tasks.iter().take(TASKS_PER_DAY).map(|task| {
                let id = task.id;
                let done = task.is_completed();
                let tint = task_rows::priority_color(task.priority, &theme).unwrap_or(theme.colors().accent);
                h_flex()
                    .id(SharedString::from(format!("calendar-task-{id}")))
                    .gap_1()
                    .px_1()
                    .rounded(px(3.))
                    .cursor_pointer()
                    .bg(tint.opacity(if done { 0.08 } else { 0.18 }))
                    .hover(|s| s.bg(tint.opacity(0.3)))
                    .child(div().w(px(2.)).h(px(12.)).flex_none().rounded_full().bg(tint))
                    .child(
                        Label::new(task.title.clone())
                            .size(LabelSize::Small)
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
}

impl Render for CalendarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let grid = calendar::month_grid(self.month);
        let weeks: Vec<AnyElement> = grid
            .chunks(7)
            .map(|week| {
                h_flex()
                    .flex_1()
                    .items_stretch()
                    .min_h_0()
                    .children(week.iter().map(|date| self.render_day(*date, today, cx)).collect::<Vec<_>>())
                    .into_any_element()
            })
            .collect();

        v_flex()
            .size_full()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(
                h_flex()
                    .h(px(60.))
                    .flex_none()
                    .px(px(24.))
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Label::new(calendar::month_title(self.month, strings))
                                .size(LabelSize::Large)
                                .weight(FontWeight::BOLD),
                        ),
                    )
                    .child(
                        IconButton::new("calendar-prev", IconName::ChevronLeft)
                            .on_click(cx.listener(|this, _, _, cx| this.shift(-1, cx))),
                    )
                    .child(
                        div()
                            .id("calendar-today")
                            .px_2()
                            .py_0p5()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .border_1()
                            .border_color(theme.colors().border)
                            .hover(|s| s.bg(theme.colors().element_hover))
                            .child(Label::new(strings.today).size(LabelSize::Small))
                            .on_click(cx.listener(|this, _, _, cx| this.go_to_today(cx))),
                    )
                    .child(
                        IconButton::new("calendar-next", IconName::ChevronRight)
                            .on_click(cx.listener(|this, _, _, cx| this.shift(1, cx))),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .children(strings.weekdays_short.iter().map(|day| {
                        div()
                            .flex_1()
                            .py_1()
                            .flex()
                            .justify_center()
                            .child(Label::new(*day).size(LabelSize::Small).color(Color::Muted))
                    })),
            )
            .child(v_flex().flex_1().min_h_0().children(weeks))
    }
}
