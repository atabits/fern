//! The task details panel on the right, modeled on TickTick's.

use std::rc::Rc;

use amin_core::{Id, List, Priority, Store, Tag, Task};
use chrono::{Datelike, Duration, Local, NaiveDate};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, Focusable, MouseButton, Rgba, SharedString,
    Subscription, Window, div, prelude::*, px, svg,
};

use crate::calendar;
use crate::i18n::{self, CurrentLocale, Strings};
use crate::lists_panel;
use crate::task_rows;
use crate::text_input::{Changed, Mode, Submitted, TextInput};
use crate::theme;
use crate::widgets::{self, Placement};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Popup {
    Due,
    Priority,
    Tags,
    List,
}

pub struct TaskDetail {
    store: Rc<Store>,
    task: Option<Task>,
    title: Entity<TextInput>,
    notes: Entity<TextInput>,
    tag_input: Entity<TextInput>,
    /// All tags, for the tag picker and chip colors.
    all_tags: Vec<Tag>,
    /// All lists, for the list picker and the list button label.
    lists: Vec<List>,
    popup: Option<Popup>,
    /// The month shown in the due-date calendar.
    calendar_month: NaiveDate,
    _subscriptions: Vec<Subscription>,
}

/// The task was changed in the panel; the list needs to reload.
pub struct TaskUpdated;

impl EventEmitter<TaskUpdated> for TaskDetail {}

impl TaskDetail {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let title = cx.new(|cx| TextInput::new(Mode::Wrapped, strings.untitled, cx));
        let notes = cx.new(|cx| TextInput::new(Mode::MultiLine, strings.notes_placeholder, cx));
        let tag_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.new_tag, cx));

        let subscriptions = vec![
            cx.subscribe(&title, |this, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                this.save(cx, |store, id| store.set_title(id, &text));
            }),
            // Enter in the title moves to the notes.
            cx.subscribe_in(&title, window, |this, _, _: &Submitted, window, cx| {
                window.focus(&this.notes.focus_handle(cx), cx);
            }),
            cx.subscribe(&notes, |this, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                this.save(cx, |store, id| store.set_notes(id, &text));
            }),
            cx.subscribe_in(&tag_input, window, |this, input, Submitted(text), window, cx| {
                let name = text.trim().trim_start_matches('#').to_owned();
                if !name.is_empty() {
                    this.add_tag(name, cx);
                }
                input.update(cx, |input, cx| input.clear(window, cx));
            }),
            cx.observe_global::<CurrentLocale>(|this, cx| {
                let strings = i18n::t(cx);
                this.title.update(cx, |input, cx| input.set_placeholder(strings.untitled, cx));
                this.notes.update(cx, |input, cx| input.set_placeholder(strings.notes_placeholder, cx));
                this.tag_input.update(cx, |input, cx| input.set_placeholder(strings.new_tag, cx));
            }),
        ];

        Self {
            store,
            task: None,
            title,
            notes,
            tag_input,
            all_tags: Vec::new(),
            lists: Vec::new(),
            popup: None,
            calendar_month: calendar::month_start(Local::now().date_naive()),
            _subscriptions: subscriptions,
        }
    }

    pub fn task_id(&self) -> Option<Id> {
        self.task.as_ref().map(|t| t.id)
    }

    pub fn set_lists(&mut self, lists: Vec<List>) {
        self.lists = lists;
    }

    /// Shows a task, or the empty panel for `None`.
    pub fn show(&mut self, task: Option<Task>, cx: &mut Context<Self>) {
        let (title, notes) = task
            .as_ref()
            .map_or((String::new(), String::new()), |t| (t.title.clone(), t.notes.clone()));
        self.title.update(cx, |input, cx| input.set_text(title, cx));
        self.notes.update(cx, |input, cx| input.set_text(notes, cx));
        self.task = task;
        self.popup = None;
        self.all_tags = self.store.tags().unwrap_or_default();
        cx.notify();
    }

    /// Updates the task data without touching the inputs, so the caret stays where it is.
    pub fn refresh(&mut self, task: Option<Task>, cx: &mut Context<Self>) {
        match task {
            Some(task) if self.task_id() == Some(task.id) => {
                self.task = Some(task);
                cx.notify();
            }
            other => self.show(other, cx),
        }
    }

    fn save(&mut self, cx: &mut Context<Self>, write: impl FnOnce(&Store, Id) -> anyhow::Result<()>) {
        let Some(id) = self.task_id() else { return };
        if let Err(err) = write(&self.store, id) {
            eprintln!("failed to save the task: {err:#}");
            return;
        }
        cx.emit(TaskUpdated);
    }

    fn open_popup(&mut self, popup: Popup, cx: &mut Context<Self>) {
        match popup {
            Popup::Due => {
                let shown = self
                    .task
                    .as_ref()
                    .and_then(|t| t.due_date)
                    .unwrap_or_else(|| Local::now().date_naive());
                self.calendar_month = calendar::month_start(shown);
            }
            Popup::Tags => self.all_tags = self.store.tags().unwrap_or_default(),
            Popup::Priority | Popup::List => {}
        }
        self.popup = Some(popup);
        cx.notify();
    }

    #[cfg(debug_assertions)]
    pub fn dev_open_popup(&mut self, popup: &str, cx: &mut Context<Self>) {
        let popup = match popup {
            "due" => Popup::Due,
            "priority" => Popup::Priority,
            "tags" => Popup::Tags,
            "list" => Popup::List,
            _ => return,
        };
        self.open_popup(popup, cx);
    }

    fn close_popup(&mut self, cx: &mut Context<Self>) {
        self.popup = None;
        cx.notify();
    }

    fn set_due(&mut self, date: Option<NaiveDate>, cx: &mut Context<Self>) {
        let time = self.task.as_ref().and_then(|t| t.due_time);
        self.save(cx, |store, id| store.set_due(id, date, time));
        self.close_popup(cx);
    }

    fn set_priority(&mut self, priority: Priority, cx: &mut Context<Self>) {
        self.save(cx, |store, id| store.set_priority(id, priority));
        self.close_popup(cx);
    }

    fn set_list(&mut self, list: Option<Id>, cx: &mut Context<Self>) {
        self.save(cx, |store, id| store.set_list(id, list));
        self.close_popup(cx);
    }

    fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.calendar_month = calendar::add_months(self.calendar_month, delta);
        cx.notify();
    }

    fn task_tags(&self) -> Vec<String> {
        self.task.as_ref().map(|t| t.tags.clone()).unwrap_or_default()
    }

    fn add_tag(&mut self, name: String, cx: &mut Context<Self>) {
        let mut tags = self.task_tags();
        if !tags.iter().any(|t| t.to_lowercase() == name.to_lowercase()) {
            tags.push(name);
            self.save(cx, |store, id| store.set_tags(id, &tags));
            self.all_tags = self.store.tags().unwrap_or_default();
        }
    }

    fn remove_tag(&mut self, name: &str, cx: &mut Context<Self>) {
        let tags: Vec<String> = self.task_tags().into_iter().filter(|t| t != name).collect();
        self.save(cx, |store, id| store.set_tags(id, &tags));
    }

    fn toggle_tag(&mut self, name: String, cx: &mut Context<Self>) {
        if self.task_tags().contains(&name) {
            self.remove_tag(&name, cx);
        } else {
            self.add_tag(name, cx);
        }
    }

    fn tag_color(&self, name: &str) -> Rgba {
        let color = self.all_tags.iter().find(|t| t.name == name).and_then(|t| t.color.as_deref());
        lists_panel::color(color)
    }

    fn render_due_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let today = Local::now().date_naive();
        let selected = self.task.as_ref().and_then(|t| t.due_date);
        let month = self.calendar_month;

        let quick = |id: &'static str, label: &'static str, date: Option<NaiveDate>, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex_1()
                .py_1()
                .flex()
                .justify_center()
                .rounded(px(4.))
                .text_xs()
                .cursor_pointer()
                .bg(theme::surface_hover())
                .hover(|s| s.bg(theme::row_selected()))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_due(date, cx)))
        };

        let days: Vec<_> = calendar::month_grid(month)
            .into_iter()
            .map(|date| {
                let in_month = date.month() == month.month();
                let is_selected = selected == Some(date);
                let is_today = date == today;
                div()
                    .id(SharedString::from(format!("day-{date}")))
                    .size(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(if !in_month {
                        theme::text_muted()
                    } else if is_today {
                        theme::accent()
                    } else {
                        theme::text()
                    })
                    .when(is_selected, |el| el.bg(theme::accent()).text_color(theme::text()))
                    .when(!is_selected, |el| el.hover(|s| s.bg(theme::surface_hover())))
                    .child(date.day().to_string())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_due(Some(date), cx)))
            })
            .collect();

        widgets::popover_frame()
            .w(px(240.))
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(quick("due-today", strings.today, Some(today), cx))
                    .child(quick("due-tomorrow", strings.tomorrow, Some(today + Duration::days(1)), cx))
                    .child(quick("due-week", strings.next_week, Some(today + Duration::days(7)), cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .pt_1()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(calendar::month_title(month, strings)),
                    )
                    .child(
                        small_icon_button("prev-month", "icons/chevron-left.svg")
                            .on_click(cx.listener(|this, _, _, cx| this.shift_month(-1, cx))),
                    )
                    .child(
                        small_icon_button("next-month", "icons/chevron-right.svg")
                            .on_click(cx.listener(|this, _, _, cx| this.shift_month(1, cx))),
                    ),
            )
            .child(div().flex().children(strings.weekdays_short.iter().map(|day| {
                div()
                    .w(px(32.))
                    .flex()
                    .justify_center()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(*day)
            })))
            .child(div().flex().flex_wrap().w(px(224.)).children(days))
            .when(selected.is_some(), |el| {
                el.child(
                    div()
                        .id("due-clear")
                        .py_1()
                        .flex()
                        .justify_center()
                        .rounded(px(4.))
                        .text_xs()
                        .text_color(theme::text_muted())
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::surface_hover()).text_color(theme::text()))
                        .child(strings.clear_date)
                        .on_click(cx.listener(|this, _, _, cx| this.set_due(None, cx))),
                )
            })
            .into_any_element()
    }

    fn render_priority_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let current = self.task.as_ref().map(|t| t.priority).unwrap_or_default();
        let items = [
            (Priority::High, "priority-high", strings.priority_high),
            (Priority::Medium, "priority-medium", strings.priority_medium),
            (Priority::Low, "priority-low", strings.priority_low),
            (Priority::None, "priority-none", strings.priority_none),
        ];
        widgets::popover_frame()
            .w(px(190.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .children(items.into_iter().map(|(priority, id, label)| {
                widgets::menu_item(id)
                    .child(colored_icon("icons/flag.svg", theme::priority(priority)))
                    .child(div().flex_1().child(label))
                    .when(priority == current, |el| el.child(check_mark()))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_priority(priority, cx)))
            }))
            .into_any_element()
    }

    fn render_list_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let current = self.task.as_ref().and_then(|t| t.list_id);
        let inbox = widgets::menu_item("move-to-inbox")
            .child(icon("icons/inbox.svg"))
            .child(div().flex_1().child(strings.inbox))
            .when(current.is_none(), |el| el.child(check_mark()))
            .on_click(cx.listener(|this, _, _, cx| this.set_list(None, cx)));
        widgets::popover_frame()
            .w(px(200.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(inbox)
            .children(self.lists.iter().map(|list| {
                let id = list.id;
                widgets::menu_item(SharedString::from(format!("move-to-{id}")))
                    .child(icon("icons/list.svg"))
                    .child(div().flex_1().min_w_0().truncate().child(list.name.clone()))
                    .when(current == Some(id), |el| el.child(check_mark()))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_list(Some(id), cx)))
            }))
            .into_any_element()
    }

    fn render_tags_popup(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.task_tags();
        widgets::popover_frame()
            .w(px(220.))
            .gap_1()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(
                div()
                    .h(px(30.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .bg(theme::background())
                    .child(self.tag_input.clone()),
            )
            .children(self.all_tags.iter().map(|tag| {
                let name = tag.name.clone();
                widgets::menu_item(SharedString::from(format!("tag-option-{}", tag.name)))
                    .child(colored_icon("icons/tag.svg", lists_panel::color(tag.color.as_deref())))
                    .child(div().flex_1().child(tag.name.clone()))
                    .when(current.contains(&tag.name), |el| el.child(check_mark()))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_tag(name.clone(), cx)))
            }))
            .into_any_element()
    }

    fn render_tags_row(&self, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(self.task_tags().into_iter().map(|tag| {
                let name = tag.clone();
                widgets::tag_chip(&tag, self.tag_color(&tag))
                    .id(SharedString::from(format!("detail-tag-{tag}")))
                    .child(
                        div()
                            .id("remove-tag")
                            .cursor_pointer()
                            .opacity(0.6)
                            .hover(|s| s.opacity(1.))
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_tag(&name, cx))),
                    )
            }))
            .child(
                div()
                    .id("add-tag")
                    .relative()
                    .h(px(18.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .text_size(px(11.))
                    .text_color(theme::text_muted())
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::surface_hover()).text_color(theme::text()))
                    .child(strings.add_tag)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_popup(Popup::Tags, cx);
                        window.focus(&this.tag_input.focus_handle(cx), cx);
                    }))
                    .when(self.popup == Some(Popup::Tags), |el| {
                        el.child(widgets::popover(Placement::BelowLeft, self.render_tags_popup(cx)))
                    }),
            )
    }

    fn toggle_completed(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.task.as_mut() else { return };
        let completed = !task.is_completed();
        if let Err(err) = self.store.set_completed(task.id, completed) {
            eprintln!("failed to update the task: {err:#}");
            return;
        }
        if let Ok(Some(fresh)) = self.store.task(task.id) {
            *task = fresh;
        }
        cx.emit(TaskUpdated);
        cx.notify();
    }
}

fn icon(path: &'static str) -> impl IntoElement {
    colored_icon(path, theme::text_muted())
}

fn colored_icon(path: &'static str, color: Rgba) -> impl IntoElement {
    svg().path(path).size(px(18.)).flex_none().text_color(color)
}

fn check_mark() -> impl IntoElement {
    div().text_color(theme::accent()).child("✓")
}

fn icon_button(id: &'static str, path: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .p_1()
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(theme::surface_hover()))
        .child(icon(path))
}

fn small_icon_button(id: &'static str, path: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .p_1()
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|s| s.bg(theme::surface_hover()))
        .child(svg().path(path).size(px(14.)).text_color(theme::text_muted()))
}

/// A text button with an icon, used for the due date and the list.
fn label_button(id: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .relative()
        .flex()
        .items_center()
        .gap_1p5()
        .px_1()
        .py_0p5()
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(theme::surface_hover()))
}

impl Render for TaskDetail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let panel = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::background())
            .text_color(theme::text())
            .border_l_1()
            .border_color(theme::border());

        let Some(task) = self.task.as_ref() else {
            return panel
                .items_center()
                .justify_center()
                .text_color(theme::text_muted())
                .child(strings.select_task);
        };
        let done = task.is_completed();
        let notes_focus = self.notes.focus_handle(cx);
        let today = Local::now().date_naive();
        let (due_text, due_color): (SharedString, Rgba) = match task.due_date {
            Some(date) => {
                let (label, overdue) = task_rows::due_label(date, task.due_time, today, strings);
                (label.into(), if overdue && !done { theme::danger() } else { theme::accent() })
            }
            None => (strings.due_date.into(), theme::text_muted()),
        };
        let priority_color = match task.priority {
            Priority::None => theme::text_muted(),
            other => theme::priority(other),
        };
        let list_name: SharedString = task
            .list_id
            .and_then(|id| self.lists.iter().find(|l| l.id == id))
            .map(|l| l.name.clone().into())
            .unwrap_or_else(|| strings.inbox.into());
        let list_icon = if task.list_id.is_some() { "icons/list.svg" } else { "icons/inbox.svg" };

        panel
            // Top bar: checkbox | due date ……… priority
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(48.))
                    .px_5()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .id("detail-checkbox")
                            .size(px(18.))
                            .flex_none()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if done { theme::accent() } else { theme::text_muted() })
                            .when(done, |el| el.bg(theme::accent()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .text_color(theme::background())
                            .when(done, |el| el.child("✓"))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_completed(cx))),
                    )
                    .child(div().w(px(1.)).h(px(16.)).bg(theme::border()))
                    .child(
                        label_button("due-date")
                            .text_color(due_color)
                            .child(colored_icon("icons/calendar-days.svg", due_color))
                            .child(due_text)
                            .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Due, cx)))
                            .when(self.popup == Some(Popup::Due), |el| {
                                el.child(widgets::popover(
                                    Placement::BelowLeft,
                                    self.render_due_popup(strings, cx),
                                ))
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("priority")
                            .relative()
                            .p_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::surface_hover()))
                            .child(colored_icon("icons/flag.svg", priority_color))
                            .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Priority, cx)))
                            .when(self.popup == Some(Popup::Priority), |el| {
                                el.child(widgets::popover(
                                    Placement::BelowRight,
                                    self.render_priority_popup(strings, cx),
                                ))
                            }),
                    ),
            )
            // Title, tags and notes; clicking empty space puts the caret into the notes.
            .child(
                div()
                    .id("detail-body")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_5()
                    .pt_4()
                    .pb_6()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&notes_focus, cx);
                    })
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(20.))
                                    .line_height(px(28.))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .when(done, |el| el.text_color(theme::text_muted()))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .child(self.title.clone()),
                            )
                            .child(div().pt_1().child(icon_button("checklist", "icons/list-checks.svg"))),
                    )
                    .child(self.render_tags_row(strings, cx))
                    .child(
                        div()
                            .text_size(px(15.))
                            .line_height(px(22.))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(self.notes.clone()),
                    ),
            )
            // Bottom bar: list ……… formatting, comments, more
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(44.))
                    .px_5()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        label_button("list")
                            .text_color(theme::text_muted())
                            .child(icon(list_icon))
                            .child(list_name)
                            .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::List, cx)))
                            .when(self.popup == Some(Popup::List), |el| {
                                // Opens upwards would be nicer; snapping keeps it inside the window.
                                el.child(widgets::popover(
                                    Placement::BelowLeft,
                                    self.render_list_popup(strings, cx),
                                ))
                            }),
                    )
                    .child(div().flex_1())
                    .child(icon_button("format", "icons/baseline.svg"))
                    .child(icon_button("comments", "icons/message-square-text.svg"))
                    .child(icon_button("more", "icons/ellipsis.svg")),
            )
    }
}
