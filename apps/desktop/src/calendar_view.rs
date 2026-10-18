//! The month calendar: tasks placed on their due dates, matched to TickTick's Calendar section.

use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Id, Store, Task};
use gpui::{EventEmitter, FontWeight};
use ui::prelude::*;

use crate::actions::{NewTask, ToggleSidebar};
use crate::calendar;
use crate::drag::DraggedTask;
use crate::i18n;
use crate::task_rows;

/// How many tasks a day cell shows before collapsing the rest into "+N".
const TASKS_PER_DAY: usize = 5;

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

    /// Re-reads the tasks of the shown weeks.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let weeks = calendar::month_weeks(self.month);
        let (start, end) = (weeks[0][0], weeks[weeks.len() - 1][6]);
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

    /// Moves a dropped task to `date`, keeping its time of day.
    fn drop_on_day(&mut self, id: Id, date: NaiveDate, cx: &mut Context<Self>) {
        let time = self.store.task(id).ok().flatten().and_then(|t| t.due_time);
        if let Err(err) = self.store.set_due(id, Some(date), time) {
            eprintln!("failed to move the task: {err:#}");
        }
        self.reload(cx);
    }

    fn go_to_today(&mut self, cx: &mut Context<Self>) {
        self.month = calendar::month_start(Local::now().date_naive());
        self.reload(cx);
    }

    fn render_day(&self, date: NaiveDate, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let tasks: Vec<&Task> = self.tasks.iter().filter(|t| t.due_date == Some(date)).collect();
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
                            Label::new(calendar::month_title(self.month, strings))
                                .size(LabelSize::Large)
                                .weight(FontWeight::BOLD),
                        ),
                    )
                    .child(
                        bordered("calendar-add-frame").child(
                            IconButton::new("calendar-add", IconName::Plus)
                                .size(px(26.))
                                .icon_size(IconSize::Small)
                                .on_click(|_, window, cx| window.dispatch_action(Box::new(NewTask), cx)),
                        ),
                    )
                    .child(
                        bordered("calendar-mode")
                            .px_2()
                            .gap_1()
                            .child(Label::new(strings.month).size(LabelSize::Small))
                            .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall)),
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
                    .children(strings.weekdays_short.iter().map(|day| {
                        div()
                            .flex_1()
                            .flex()
                            .justify_center()
                            .child(Label::new(*day).size(LabelSize::Small).color(Color::Muted))
                    })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(theme.colors().border_variant)
                    .children(weeks),
            )
    }
}
