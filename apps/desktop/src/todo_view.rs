use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use amin_core::{Id, Kind, Priority, Store, Tag, Task, View, ViewCounts};
use chrono::{Duration, Local, NaiveDate};
use gpui::{
    Context, Entity, FocusHandle, Focusable, MouseButton, Rgba, SharedString, Window, div,
    prelude::*, px, svg,
};

use crate::lists_panel;
use crate::task_detail::{TaskDetail, TaskUpdated};
use crate::task_rows::{self, Row, Section};
use crate::text_input::{Mode, Submitted, TextInput};
use crate::theme;
use crate::widgets;

const DETAIL_WIDTH: f32 = 400.;
const ROW_HEIGHT: f32 = 40.;

pub struct TodoView {
    store: Rc<Store>,
    view: View,
    tasks: Vec<Task>,
    counts: ViewCounts,
    tags: Vec<Tag>,
    show_lists: bool,
    collapsed_tasks: HashSet<Id>,
    collapsed_sections: HashSet<Section>,
    input: Entity<TextInput>,
    detail: Entity<TaskDetail>,
    selected: Option<Id>,
    error: Option<String>,
}

impl TodoView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, "Добавить задачу", cx));
        cx.subscribe_in(&input, window, |this, input, Submitted(text), window, cx| {
            if text.trim().is_empty() {
                return;
            }
            let result = this.add_task(text);
            if this.check(result).is_some() {
                input.update(cx, |input, cx| input.clear(window, cx));
                this.reload(cx);
            }
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);

        let detail = cx.new(|cx| TaskDetail::new(store.clone(), window, cx));
        cx.subscribe(&detail, |this, _, _: &TaskUpdated, cx| this.reload(cx)).detach();

        let mut this = Self {
            store,
            view: View::Inbox,
            tasks: Vec::new(),
            counts: ViewCounts::default(),
            tags: Vec::new(),
            show_lists: true,
            collapsed_tasks: HashSet::new(),
            // Как в TickTick: выполненные свёрнуты внизу списка.
            collapsed_sections: HashSet::from([Section::Completed]),
            input,
            detail,
            selected: None,
            error: None,
        };
        this.reload(cx);
        #[cfg(debug_assertions)]
        this.apply_dev_state(cx);
        this
    }

    /// Только для отладочной сборки:
    /// `AMIN_DEV_STATE="add=<текст быстрого ввода>;select=<часть названия>;popup=due|priority|tags"`
    /// открывает нужное состояние без кликов — так проверяются скриншоты интерфейса.
    #[cfg(debug_assertions)]
    fn apply_dev_state(&mut self, cx: &mut Context<Self>) {
        let Ok(state) = std::env::var("AMIN_DEV_STATE") else { return };
        for part in state.split(';') {
            match part.split_once('=') {
                Some(("add", text)) => {
                    let result = self.add_task(text);
                    self.check(result);
                    self.reload(cx);
                }
                Some(("select", needle)) => {
                    if let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id) {
                        self.select(id, cx);
                    }
                }
                Some(("popup", popup)) => {
                    let popup = popup.to_owned();
                    self.detail.update(cx, |detail, cx| detail.dev_open_popup(&popup, cx));
                }
                _ => {}
            }
        }
    }

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let today = Self::today();
        if let Some(tasks) = self.check(self.store.tasks_in(&self.view, today)) {
            self.tasks = tasks;
        }
        if let Some(counts) = self.check(self.store.counts(today)) {
            self.counts = counts;
        }
        if let Some(tags) = self.check(self.store.tags()) {
            self.tags = tags;
        }
        // Панель показывает задачу, пока она существует, даже если ушла из текущего списка.
        let selected = self.selected.and_then(|id| self.store.task(id).ok().flatten());
        self.selected = selected.as_ref().map(|t| t.id);
        self.detail.update(cx, |detail, cx| detail.refresh(selected, cx));
        cx.notify();
    }

    /// Новая задача: из текста разбираются срок, время, теги и приоритет («завтра 18:00 #дом !1»),
    /// а недостающее берётся из текущего списка — срок в «Сегодня»/«Завтра», тег в списке тега.
    fn add_task(&self, input: &str) -> anyhow::Result<Task> {
        let today = Self::today();
        let parsed = amin_core::parse_quick_add(input, today);
        let title = if parsed.title.is_empty() { input.trim() } else { parsed.title.as_str() };
        let task = self.store.add_task(title)?;

        let view_due = match &self.view {
            View::Today | View::Next7Days => Some(today),
            View::Tomorrow => Some(today + Duration::days(1)),
            _ => None,
        };
        if let Some(date) = parsed.due_date.or(view_due) {
            self.store.set_due(task.id, Some(date), parsed.due_time)?;
        }

        let mut tags = parsed.tags;
        if let View::Tag(tag) = &self.view
            && !tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase())
        {
            tags.push(tag.clone());
        }
        if !tags.is_empty() {
            self.store.set_tags(task.id, &tags)?;
        }
        if let Some(priority) = parsed.priority {
            self.store.set_priority(task.id, priority)?;
        }
        Ok(task)
    }

    fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        if self.view != view {
            self.view = view;
            self.reload(cx);
        }
    }

    fn toggle_task_collapsed(&mut self, id: Id, cx: &mut Context<Self>) {
        if !self.collapsed_tasks.remove(&id) {
            self.collapsed_tasks.insert(id);
        }
        cx.notify();
    }

    fn toggle_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if !self.collapsed_sections.remove(&section) {
            self.collapsed_sections.insert(section);
        }
        cx.notify();
    }

    fn toggle_lists(&mut self, cx: &mut Context<Self>) {
        self.show_lists = !self.show_lists;
        cx.notify();
    }

    /// Ошибки базы показываем строкой внизу окна, а не роняем приложение.
    fn check<T>(&mut self, result: anyhow::Result<T>) -> Option<T> {
        match result {
            Ok(value) => {
                self.error = None;
                Some(value)
            }
            Err(err) => {
                self.error = Some(format!("{err:#}"));
                None
            }
        }
    }

    fn select(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.selected == Some(id) {
            return;
        }
        self.selected = Some(id);
        let task = self.tasks.iter().find(|t| t.id == id).cloned();
        self.detail.update(cx, |detail, cx| detail.show(task, cx));
        cx.notify();
    }

    fn toggle(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
        let result = self.store.set_completed(id, completed);
        self.check(result);
        self.reload(cx);
    }

    fn delete(&mut self, id: Id, cx: &mut Context<Self>) {
        let result = self.store.delete_task(id);
        self.check(result);
        self.reload(cx);
    }

    fn render_section(
        &self,
        section: Section,
        count: usize,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .id(SharedString::from(format!("section-{}", section.title())))
            .h(px(36.))
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
            .child(chevron(collapsed))
            .child(div().font_weight(gpui::FontWeight::BOLD).text_size(px(13.)).child(section.title()))
            .child(div().pl_1().text_xs().text_color(theme::text_muted()).child(count.to_string()))
            .into_any_element()
    }

    fn render_task(
        &self,
        task: &Task,
        depth: usize,
        has_children: bool,
        collapsed: bool,
        tag_colors: &HashMap<String, Rgba>,
        today: NaiveDate,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = task.id;
        let done = task.is_completed();
        let selected = self.selected == Some(id);
        let group = format!("task-{id}");

        let marker = if task.kind == Kind::Note {
            div()
                .id("marker")
                .flex_none()
                .child(svg().path("icons/file-text.svg").size(px(16.)).text_color(theme::checkbox()))
        } else {
            let color = if done { theme::accent() } else { theme::priority(task.priority) };
            div()
                .id("marker")
                .size(px(16.))
                .flex_none()
                .rounded(px(3.))
                .border_1()
                .border_color(color)
                .when(done, |el| el.bg(theme::accent()))
                .when(!done && task.priority != Priority::None, |el| el.bg(color.opacity(0.15)))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(theme::background())
                .when(done, |el| el.child("✓"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle(id, !done, cx);
                }))
        };

        let due = task.due_date.map(|date| task_rows::due_label(date, task.due_time, today));

        // Слева от чекбокса — колонка под шеврон; разделитель начинается от чекбокса.
        div()
            .id(gpui::ElementId::Uuid(id))
            .group(group.clone())
            .h(px(ROW_HEIGHT))
            .flex()
            .pl(px(depth as f32 * INDENT))
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme::row_selected()))
            .when(!selected, |el| el.hover(|s| s.bg(theme::row_hover())))
            .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
            .child(
                div()
                    .id("toggle-children")
                    .w(px(GUTTER))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(has_children, |el| {
                        el.child(chevron(collapsed)).on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_task_collapsed(id, cx);
                        }))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pr_3()
                    .border_b_1()
                    .border_color(theme::row_separator())
                    .child(marker)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(done, |el| el.line_through().text_color(theme::text_muted()))
                            .when(task.title.is_empty(), |el| el.text_color(theme::text_muted()))
                            .child(if task.title.is_empty() {
                                "Без названия".to_owned()
                            } else {
                                task.title.clone()
                            }),
                    )
                    .children(task.tags.iter().map(|tag| {
                        let color = tag_colors.get(tag).copied().unwrap_or_else(theme::text_muted);
                        widgets::tag_chip(tag, color)
                    }))
                    .when_some(due, |el, (label, overdue)| {
                        el.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(if overdue && !done { theme::danger() } else { theme::accent() })
                                .child(label),
                        )
                    })
                    .child(
                        div()
                            .id("delete")
                            .flex_none()
                            .px_1()
                            .text_color(theme::text_muted())
                            .opacity(0.)
                            .group_hover(group, |s| s.opacity(1.))
                            .hover(|s| s.text_color(theme::danger()))
                            .child("✕")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.delete(id, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

const GUTTER: f32 = 20.;
const INDENT: f32 = 24.;

fn chevron(collapsed: bool) -> impl IntoElement {
    let path = if collapsed { "icons/chevron-right.svg" } else { "icons/chevron-down.svg" };
    svg().path(path).size(px(12.)).flex_none().text_color(theme::text_muted())
}

fn header_icon(id: &'static str, path: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .p_1()
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(theme::surface_hover()))
        .child(svg().path(path).size(px(18.)).text_color(theme::text_muted()))
}

impl Focusable for TodoView {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for TodoView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input_focus = self.input.focus_handle(cx);
        let today = Self::today();
        let tag_colors: HashMap<String, Rgba> = self
            .tags
            .iter()
            .map(|t| (t.name.clone(), lists_panel::tag_color(t.color.as_deref())))
            .collect();
        let rows = task_rows::build_rows(&self.tasks, &self.collapsed_tasks, &self.collapsed_sections);
        let rows: Vec<_> = rows
            .iter()
            .map(|row| match row {
                Row::Section { section, count, collapsed } => {
                    self.render_section(*section, *count, *collapsed, cx)
                }
                Row::Task { task, depth, has_children, collapsed } => self.render_task(
                    task,
                    *depth,
                    *has_children,
                    *collapsed,
                    &tag_colors,
                    today,
                    cx,
                ),
            })
            .collect();

        let list = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(60.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl(px(24.))
                    .pr(px(16.))
                    .child(
                        header_icon("toggle-sidebar", "icons/panel-left.svg")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_lists(cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(20.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(lists_panel::title(&self.view)),
                    )
                    .child(header_icon("sort", "icons/arrow-down-up.svg"))
                    .child(header_icon("more", "icons/ellipsis.svg")),
            )
            .child(
                div()
                    .mx_2()
                    .mb_2()
                    .h(px(38.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .rounded(px(6.))
                    .bg(theme::surface())
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&input_focus, cx);
                    })
                    .child(svg().path("icons/plus.svg").size(px(16.)).flex_none().text_color(theme::text_muted()))
                    .child(self.input.clone()),
            )
            .child(
                div()
                    .id("tasks")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_4()
                    .when(self.tasks.is_empty(), |el| {
                        el.child(
                            div()
                                .px_3()
                                .pt_8()
                                .text_color(theme::text_muted())
                                .child(if self.view == View::Completed { "Выполненных задач нет" } else { "Задач нет" }),
                        )
                    })
                    .children(rows),
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .px_6()
                        .py_2()
                        .border_t_1()
                        .border_color(theme::border())
                        .text_color(theme::danger())
                        .child(error),
                )
            });

        let this = cx.entity().downgrade();
        let on_select = move |view, _: &mut Window, cx: &mut gpui::App| {
            this.update(cx, |this, cx| this.set_view(view, cx)).ok();
        };

        div()
            .size_full()
            .flex()
            .bg(theme::background())
            .text_color(theme::text())
            .text_size(px(14.))
            .when(self.show_lists, |el| {
                el.child(lists_panel::render(&self.view, &self.counts, &self.tags, on_select))
            })
            .child(list)
            .child(div().flex_none().w(px(DETAIL_WIDTH)).h_full().child(self.detail.clone()))
    }
}
