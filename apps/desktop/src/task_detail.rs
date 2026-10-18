//! The task details panel on the right, modeled on TickTick's.

use std::rc::Rc;

use fern_core::{ChecklistItem, Frequency, Id, List, Priority, Recurrence, Store, Tag, Task};
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime};
use gpui::{Entity, EventEmitter, Focusable, FontWeight, MouseButton, Subscription};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::calendar;
use crate::i18n::{self, CurrentLocale, Strings};
use crate::lists_panel;
use crate::task_rows;
use crate::text_input::{Changed, Mode, Submitted, TextInput};

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
    /// Checklist items with an input each, in display order.
    checklist: Vec<ChecklistRow>,
    new_item: Entity<TextInput>,
    /// The checklist is shown when it has items or the user asked to start one.
    show_checklist: bool,
    /// An input to focus on the next render (focusing needs the window).
    pending_focus: Option<FocusTarget>,
    popup: Option<Popup>,
    /// The month shown in the due-date calendar.
    calendar_month: NaiveDate,
    _subscriptions: Vec<Subscription>,
}

struct ChecklistRow {
    item: ChecklistItem,
    input: Entity<TextInput>,
    _subscriptions: [Subscription; 2],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusTarget {
    Item(Id),
    NewItem,
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
        let new_item = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_item, cx));

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
            cx.subscribe_in(&new_item, window, |this, input, Submitted(text), window, cx| {
                if !text.trim().is_empty() {
                    let text = text.clone();
                    this.save(cx, |store, id| store.add_checklist_item(id, &text, None).map(|_| ()));
                    input.update(cx, |input, cx| input.clear(window, cx));
                    this.sync_checklist(cx);
                }
            }),
            cx.observe_global::<CurrentLocale>(|this, cx| {
                let strings = i18n::t(cx);
                this.title.update(cx, |input, cx| input.set_placeholder(strings.untitled, cx));
                this.notes.update(cx, |input, cx| input.set_placeholder(strings.notes_placeholder, cx));
                this.tag_input.update(cx, |input, cx| input.set_placeholder(strings.new_tag, cx));
                this.new_item.update(cx, |input, cx| input.set_placeholder(strings.add_item, cx));
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
            checklist: Vec::new(),
            new_item,
            show_checklist: false,
            pending_focus: None,
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
        self.checklist.clear();
        self.sync_checklist(cx);
        self.show_checklist = !self.checklist.is_empty();
        cx.notify();
    }

    /// Reloads checklist items, keeping the inputs of items that still exist so that the caret
    /// does not jump while the user types.
    fn sync_checklist(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.task_id() else {
            self.checklist.clear();
            return;
        };
        let items = self.store.checklist(task).unwrap_or_default();
        let mut old: Vec<ChecklistRow> = std::mem::take(&mut self.checklist);
        for item in items {
            let row = match old.iter().position(|row| row.item.id == item.id) {
                Some(index) => {
                    let mut row = old.swap_remove(index);
                    row.item = item;
                    row
                }
                None => self.checklist_row(item, cx),
            };
            self.checklist.push(row);
        }
        cx.notify();
    }

    fn checklist_row(&mut self, item: ChecklistItem, cx: &mut Context<Self>) -> ChecklistRow {
        let id = item.id;
        let title = item.title.clone();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::Wrapped, "", cx);
            input.set_text(title, cx);
            input
        });
        let subscriptions = [
            cx.subscribe(&input, move |this, input, _: &Changed, cx| {
                let text = input.read(cx).text().to_owned();
                this.save(cx, |store, _| store.set_checklist_item_title(id, &text));
            }),
            // Enter adds the next item right below, as in TickTick.
            cx.subscribe(&input, move |this, _, _: &Submitted, cx| {
                let created = this
                    .task_id()
                    .and_then(|task| this.store.add_checklist_item(task, "", Some(id)).ok());
                if let Some(created) = created {
                    this.pending_focus = Some(FocusTarget::Item(created.id));
                    cx.emit(TaskUpdated);
                    this.sync_checklist(cx);
                }
            }),
        ];
        ChecklistRow { item, input, _subscriptions: subscriptions }
    }

    fn toggle_checklist_item(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
        self.save(cx, |store, _| store.set_checklist_item_completed(id, completed));
        self.sync_checklist(cx);
    }

    fn delete_checklist_item(&mut self, id: Id, cx: &mut Context<Self>) {
        self.save(cx, |store, _| store.delete_checklist_item(id));
        self.sync_checklist(cx);
    }

    fn start_checklist(&mut self, cx: &mut Context<Self>) {
        self.show_checklist = true;
        self.pending_focus = Some(FocusTarget::NewItem);
        cx.notify();
    }

    /// Updates the task data without touching the inputs, so the caret stays where it is.
    pub fn refresh(&mut self, task: Option<Task>, cx: &mut Context<Self>) {
        match task {
            Some(task) if self.task_id() == Some(task.id) => {
                self.task = Some(task);
                self.sync_checklist(cx);
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
            "checklist" => {
                self.start_checklist(cx);
                return;
            }
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

    /// Sets the time of day; picking a time for a task without a date schedules it for today.
    fn set_time(&mut self, time: Option<NaiveTime>, cx: &mut Context<Self>) {
        let date = self.task.as_ref().and_then(|t| t.due_date).or_else(|| time.map(|_| Local::now().date_naive()));
        self.save(cx, |store, id| store.set_due(id, date, time));
    }

    fn set_repeat(&mut self, repeat: Option<Recurrence>, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        self.save(cx, |store, id| store.set_repeat(id, repeat, today));
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

    fn tag_color(&self, name: &str, cx: &App) -> Hsla {
        let color = self.all_tags.iter().find(|t| t.name == name).and_then(|t| t.color.as_deref());
        lists_panel::color(color, cx)
    }

    fn restore(&mut self, cx: &mut Context<Self>) {
        self.save(cx, |store, id| store.restore_task(id));
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

    fn render_checklist(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(self.checklist.iter().map(|row| {
                let id = row.item.id;
                let done = row.item.completed;
                h_flex()
                    .id(SharedString::from(format!("item-{id}")))
                    .group("checklist-item")
                    .min_h(px(30.))
                    .items_start()
                    .gap_2()
                    .py_1()
                    .child(
                        div().mt(px(3.)).child(
                            Checkbox::new("item-checkbox")
                                .size(px(14.))
                                .toggle_state(done)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_checklist_item(id, !done, cx)
                                })),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(done, |el| el.text_color(theme.colors().text_muted))
                            .child(row.input.clone()),
                    )
                    .child(
                        div()
                            .id("item-delete")
                            .flex_none()
                            .px_1()
                            .text_color(theme.colors().text_muted)
                            .opacity(0.)
                            .group_hover("checklist-item", |s| s.opacity(1.))
                            .hover(|s| s.text_color(theme.status().error))
                            .cursor_pointer()
                            .child("✕")
                            .on_click(cx.listener(move |this, _, _, cx| this.delete_checklist_item(id, cx))),
                    )
            }))
            .child(
                h_flex()
                    .min_h(px(30.))
                    .gap_2()
                    .child(Icon::new(IconName::Plus).size(IconSize::Small))
                    .child(div().flex_1().child(self.new_item.clone())),
            )
    }

    fn render_due_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
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
                .cursor_pointer()
                .bg(theme.colors().element_hover)
                .hover(|s| s.bg(theme.colors().element_selected))
                .child(Label::new(label).size(LabelSize::Small))
                .on_click(cx.listener(move |this, _, _, cx| this.set_due(date, cx)))
        };

        let days: Vec<_> = calendar::month_grid(month)
            .into_iter()
            .map(|date| {
                let in_month = date.month() == month.month();
                let is_selected = selected == Some(date);
                let color = if is_selected {
                    Color::Default
                } else if !in_month {
                    Color::Muted
                } else if date == today {
                    Color::Accent
                } else {
                    Color::Default
                };
                div()
                    .id(SharedString::from(format!("day-{date}")))
                    .size(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .when(is_selected, |el| el.bg(theme.colors().accent))
                    .when(!is_selected, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                    .child(Label::new(date.day().to_string()).size(LabelSize::Small).color(color))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_due(Some(date), cx)))
            })
            .collect();

        popover(cx)
            .w(px(240.))
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(
                h_flex()
                    .gap_1()
                    .child(quick("due-today", strings.today, Some(today), cx))
                    .child(quick("due-tomorrow", strings.tomorrow, Some(today + Duration::days(1)), cx))
                    .child(quick("due-week", strings.next_week, Some(today + Duration::days(7)), cx)),
            )
            .child(
                h_flex()
                    .pt_1()
                    .child(
                        div()
                            .flex_1()
                            .child(Label::new(calendar::month_title(month, strings)).weight(FontWeight::SEMIBOLD)),
                    )
                    .child(
                        IconButton::new("prev-month", IconName::ChevronLeft)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.shift_month(-1, cx))),
                    )
                    .child(
                        IconButton::new("next-month", IconName::ChevronRight)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.shift_month(1, cx))),
                    ),
            )
            .child(h_flex().children(strings.weekdays_short.iter().map(|day| {
                div()
                    .w(px(32.))
                    .flex()
                    .justify_center()
                    .child(Label::new(*day).size(LabelSize::Small).color(Color::Muted))
            })))
            .child(div().flex().flex_wrap().w(px(224.)).children(days))
            .child(Divider::horizontal())
            .child(self.render_time_row(strings, cx))
            .child(self.render_repeat_row(strings, cx))
            .when(selected.is_some(), |el| {
                el.child(
                    div()
                        .id("due-clear")
                        .py_1()
                        .flex()
                        .justify_center()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.colors().element_hover))
                        .child(Label::new(strings.clear_date).size(LabelSize::Small).color(Color::Muted))
                        .on_click(cx.listener(|this, _, _, cx| this.set_due(None, cx))),
                )
            })
            .into_any_element()
    }

    fn render_time_row(&self, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.task.as_ref().and_then(|t| t.due_time);
        let options: Vec<(SharedString, Option<NaiveTime>)> = std::iter::once((strings.no_time.into(), None))
            .chain([9, 12, 15, 18, 21].into_iter().map(|hour| {
                (format!("{hour:02}:00").into(), NaiveTime::from_hms_opt(hour, 0, 0))
            }))
            .collect();
        option_row(
            strings.time,
            options
                .into_iter()
                .map(|(label, time)| {
                    option_chip(SharedString::from(format!("time-{label}")), label, current == time, cx)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_time(time, cx)))
                })
                .collect::<Vec<_>>(),
        )
    }

    fn render_repeat_row(&self, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.task.as_ref().and_then(|t| t.repeat).map(|r| r.frequency);
        let options = [
            (strings.repeat_none, None),
            (strings.repeat_daily, Some(Frequency::Daily)),
            (strings.repeat_weekly, Some(Frequency::Weekly)),
            (strings.repeat_monthly, Some(Frequency::Monthly)),
            (strings.repeat_yearly, Some(Frequency::Yearly)),
        ];
        option_row(
            strings.repeat,
            options
                .into_iter()
                .map(|(label, frequency)| {
                    let id = SharedString::from(format!("repeat-{frequency:?}"));
                    option_chip(id, label.into(), current == frequency, cx).on_click(cx.listener(
                        move |this, _, _, cx| this.set_repeat(frequency.map(|f| Recurrence::new(f, 1)), cx),
                    ))
                })
                .collect::<Vec<_>>(),
        )
    }

    fn render_priority_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = self.task.as_ref().map(|t| t.priority).unwrap_or_default();
        let items = [
            (Priority::High, "priority-high", strings.priority_high),
            (Priority::Medium, "priority-medium", strings.priority_medium),
            (Priority::Low, "priority-low", strings.priority_low),
            (Priority::None, "priority-none", strings.priority_none),
        ];
        popover(cx)
            .w(px(190.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .children(items.into_iter().map(|(priority, id, label)| {
                let color = task_rows::priority_color(priority, &theme).unwrap_or(theme.colors().checkbox_border);
                ListItem::new(id)
                    .start_slot(Icon::new(IconName::Flag).size(IconSize::Large).color(color))
                    .child(Label::new(label))
                    .when(priority == current, |item| item.end_slot(check_mark()))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_priority(priority, cx)))
            }))
            .into_any_element()
    }

    fn render_list_popup(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let current = self.task.as_ref().and_then(|t| t.list_id);
        popover(cx)
            .w(px(200.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(
                ListItem::new("move-to-inbox")
                    .start_slot(Icon::new(IconName::Inbox))
                    .child(Label::new(strings.inbox))
                    .when(current.is_none(), |item| item.end_slot(check_mark()))
                    .on_click(cx.listener(|this, _, _, cx| this.set_list(None, cx))),
            )
            .children(self.lists.iter().map(|list| {
                let id = list.id;
                ListItem::new(SharedString::from(format!("move-to-{id}")))
                    .start_slot(Icon::new(IconName::List))
                    .child(Label::new(list.name.clone()).truncate())
                    .when(current == Some(id), |item| item.end_slot(check_mark()))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_list(Some(id), cx)))
            }))
            .into_any_element()
    }

    fn render_tags_popup(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = self.task_tags();
        popover(cx)
            .w(px(220.))
            .gap_1()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(
                h_flex()
                    .h(px(30.))
                    .px_2()
                    .rounded(px(4.))
                    .bg(theme.colors().background)
                    .child(self.tag_input.clone()),
            )
            .children(self.all_tags.iter().map(|tag| {
                let name = tag.name.clone();
                ListItem::new(SharedString::from(format!("tag-option-{}", tag.name)))
                    .start_slot(
                        Icon::new(IconName::Tag)
                            .size(IconSize::Large)
                            .color(lists_panel::color(tag.color.as_deref(), cx)),
                    )
                    .child(Label::new(tag.name.clone()))
                    .when(current.contains(&tag.name), |item| item.end_slot(check_mark()))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_tag(name.clone(), cx)))
            }))
            .into_any_element()
    }

    fn render_tags_row(&self, strings: &'static Strings, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .flex_wrap()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(self.task_tags().into_iter().map(|tag| {
                let name = tag.clone();
                Chip::new(tag.clone(), self.tag_color(&tag, cx)).end_slot(
                    div()
                        .id(SharedString::from(format!("remove-tag-{tag}")))
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
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.colors().element_hover))
                    .child(Label::new(strings.add_tag).size(LabelSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_popup(Popup::Tags, cx);
                        window.focus(&this.tag_input.focus_handle(cx), cx);
                    }))
                    .when(self.popup == Some(Popup::Tags), |el| {
                        el.child(PopoverAttachment::new(Placement::BelowLeft, self.render_tags_popup(cx)))
                    }),
            )
    }
}

/// A labeled row of option chips in the due-date popover.
fn option_row(label: &'static str, chips: impl IntoIterator<Item = Stateful<Div>>) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
        .child(h_flex().flex_wrap().gap_1().children(chips))
}

fn option_chip(id: SharedString, label: SharedString, selected: bool, cx: &App) -> Stateful<Div> {
    let colors = cx.theme().colors();
    div()
        .id(id)
        .px_1p5()
        .py_0p5()
        .rounded(px(4.))
        .cursor_pointer()
        .bg(if selected { colors.accent } else { colors.element_hover })
        .hover(move |s| if selected { s } else { s.bg(colors.element_selected) })
        .child(Label::new(label).size(LabelSize::Small))
}

fn check_mark() -> impl IntoElement {
    Label::new("✓").color(Color::Accent)
}

/// A text button with an icon, used for the due date and the list.
fn label_button(id: &'static str, icon: IconName, label: SharedString, color: Color, cx: &App) -> Stateful<Div> {
    let hover = cx.theme().colors().element_hover;
    h_flex()
        .id(id)
        .relative()
        .gap_1p5()
        .px_1()
        .py_0p5()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(Icon::new(icon).size(IconSize::Large).color(color))
        .child(Label::new(label).color(color))
}

impl Render for TaskDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        if let Some(target) = self.pending_focus.take() {
            let input = match target {
                FocusTarget::NewItem => Some(self.new_item.clone()),
                FocusTarget::Item(id) => {
                    self.checklist.iter().find(|row| row.item.id == id).map(|row| row.input.clone())
                }
            };
            if let Some(input) = input {
                window.focus(&input.focus_handle(cx), cx);
            }
        }
        let panel = v_flex()
            .size_full()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .border_l_1()
            .border_color(theme.colors().border);

        let Some(task) = self.task.as_ref() else {
            return panel
                .items_center()
                .justify_center()
                .child(Label::new(strings.select_task).color(Color::Muted));
        };
        let done = task.is_completed();
        let notes_focus = self.notes.focus_handle(cx);
        let today = Local::now().date_naive();
        let repeats = task.repeat.is_some();
        let (due_text, due_color): (SharedString, Color) = match task.due_date {
            Some(date) => {
                let (label, overdue) = task_rows::due_label(date, task.due_time, today, strings);
                (label.into(), if overdue && !done { Color::Error } else { Color::Accent })
            }
            None => (strings.due_date.into(), Color::Muted),
        };
        let priority_color = task_rows::priority_color(task.priority, &theme).map_or(Color::Muted, Color::Custom);
        let list_name: SharedString = task
            .list_id
            .and_then(|id| self.lists.iter().find(|l| l.id == id))
            .map(|l| l.name.clone().into())
            .unwrap_or_else(|| strings.inbox.into());
        let list_icon = if task.list_id.is_some() { IconName::List } else { IconName::Inbox };

        panel
            // Top bar: checkbox | due date ……… priority
            .child(
                h_flex()
                    .gap_3()
                    .h(px(48.))
                    .px_5()
                    .border_b_1()
                    .border_color(theme.colors().border)
                    .child(
                        Checkbox::new("detail-checkbox")
                            .size(px(18.))
                            .toggle_state(done)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_completed(cx))),
                    )
                    .child(Divider::vertical())
                    .child(
                        label_button("due-date", IconName::CalendarDays, due_text, due_color, cx)
                            .when(repeats, |el| el.child(Icon::new(IconName::Repeat).size(IconSize::Small).color(due_color)))
                            .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Due, cx)))
                            .when(self.popup == Some(Popup::Due), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowLeft,
                                    self.render_due_popup(strings, cx),
                                ))
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .relative()
                            .child(
                                IconButton::new("priority", IconName::Flag)
                                    .icon_color(priority_color)
                                    .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Priority, cx))),
                            )
                            .when(self.popup == Some(Popup::Priority), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    self.render_priority_popup(strings, cx),
                                ))
                            }),
                    ),
            )
            .when(task.is_deleted(), |el| {
                el.child(
                    h_flex()
                        .gap_2()
                        .px_5()
                        .py_2()
                        .bg(theme.colors().surface_background)
                        .border_b_1()
                        .border_color(theme.colors().border)
                        .child(Icon::new(IconName::Trash).size(IconSize::Small))
                        .child(div().flex_1().child(Label::new(strings.trash).size(LabelSize::Small).color(Color::Muted)))
                        .child(
                            h_flex()
                                .id("restore-task")
                                .gap_1()
                                .px_2()
                                .py_0p5()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.colors().element_hover))
                                .child(Icon::new(IconName::RotateCcw).size(IconSize::Small).color(Color::Accent))
                                .child(Label::new(strings.restore).size(LabelSize::Small).color(Color::Accent))
                                .on_click(cx.listener(|this, _, _, cx| this.restore(cx))),
                        ),
                )
            })
            // Title, tags, checklist and notes; clicking empty space puts the caret into the notes.
            .child(
                v_flex()
                    .id("detail-body")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_5()
                    .pt_4()
                    .pb_6()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&notes_focus, cx);
                    })
                    .child(
                        h_flex()
                            .items_start()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(20.))
                                    .line_height(px(28.))
                                    .font_weight(FontWeight::BOLD)
                                    .when(done, |el| el.text_color(theme.colors().text_muted))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .child(self.title.clone()),
                            )
                            .child(
                                div()
                                    .pt_0p5()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .child(
                                        IconButton::new("checklist", IconName::ListChecks)
                                            .on_click(cx.listener(|this, _, _, cx| this.start_checklist(cx))),
                                    ),
                            ),
                    )
                    .child(self.render_tags_row(strings, cx))
                    .when(self.show_checklist, |el| el.child(self.render_checklist(cx)))
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
                h_flex()
                    .gap_2()
                    .h(px(44.))
                    .px_5()
                    .border_t_1()
                    .border_color(theme.colors().border)
                    .child(
                        label_button("list", list_icon, list_name, Color::Muted, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::List, cx)))
                            .when(self.popup == Some(Popup::List), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowLeft,
                                    self.render_list_popup(strings, cx),
                                ))
                            }),
                    )
                    .child(div().flex_1())
                    .child(IconButton::new("format", IconName::Baseline))
                    .child(IconButton::new("comments", IconName::MessageSquareText))
                    .child(IconButton::new("more", IconName::Ellipsis)),
            )
    }
}
