use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use fern_core::{Id, Kind, List, Store, Tag, Task, View, ViewCounts};
use chrono::{Duration, Local, NaiveDate};
use gpui::{ElementId, Entity, FocusHandle, Focusable, FontWeight, MouseButton};
use ui::prelude::*;

use crate::drag::DraggedTask;
use ui::{Placement, PopoverAttachment, popover};

use crate::actions::{DeleteTask, SelectNext, SelectPrevious, ToggleComplete};
use crate::i18n::{self, CurrentLocale, Strings};
use crate::lists_panel::{self, LIST_COLORS};
use crate::task_detail::{OpenTask, TaskDetail, TaskUpdated};
use crate::task_rows::{self, Row, Section};
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};

const DETAIL_WIDTH: f32 = 400.;
const ROW_HEIGHT: f32 = 40.;
/// Space left of the checkbox for the subtask chevron.
const GUTTER: f32 = 20.;
const INDENT: f32 = 24.;

pub struct TodoView {
    store: Rc<Store>,
    view: View,
    tasks: Vec<Task>,
    counts: ViewCounts,
    lists: Vec<List>,
    tags: Vec<Tag>,
    show_lists: bool,
    collapsed_tasks: HashSet<Id>,
    collapsed_sections: HashSet<Section>,
    input: Entity<TextInput>,
    /// Focus of the task list itself, for keyboard navigation (the `TaskList` key context).
    list_focus: FocusHandle,
    new_list: Entity<TextInput>,
    creating_list: bool,
    rename_input: Entity<TextInput>,
    renaming_list: bool,
    search_input: Entity<TextInput>,
    header_menu_open: bool,
    detail: Entity<TaskDetail>,
    selected: Option<Id>,
    error: Option<String>,
}

impl TodoView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_task, cx));
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
        // Escape in the quick-add field hands the keyboard to the list.
        cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
            window.focus(&this.list_focus, cx);
        })
        .detach();

        let new_list = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.new_list, cx));
        cx.subscribe_in(&new_list, window, |this, input, Submitted(name), window, cx| {
            this.create_list(name, cx);
            input.update(cx, |input, cx| input.clear(window, cx));
        })
        .detach();
        cx.subscribe(&new_list, |this, _, _: &Cancelled, cx| {
            this.creating_list = false;
            cx.notify();
        })
        .detach();

        let rename_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.new_list, cx));
        cx.subscribe(&rename_input, |this, _, Submitted(name), cx| this.rename_current_list(name, cx))
            .detach();
        cx.subscribe(&rename_input, |this, _, _: &Cancelled, cx| {
            this.renaming_list = false;
            cx.notify();
        })
        .detach();

        let search_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.search_placeholder, cx));
        cx.subscribe(&search_input, |this, input, _: &Changed, cx| {
            let query = input.read(cx).text().to_owned();
            this.view = View::Search(query);
            this.reload(cx);
        })
        .detach();

        // Placeholders live inside the inputs, so they are updated when the language changes.
        cx.observe_global::<CurrentLocale>(|this, cx| {
            let strings = i18n::t(cx);
            this.input.update(cx, |input, cx| input.set_placeholder(strings.add_task, cx));
            this.new_list.update(cx, |input, cx| input.set_placeholder(strings.new_list, cx));
            this.search_input.update(cx, |input, cx| input.set_placeholder(strings.search_placeholder, cx));
        })
        .detach();

        let detail = cx.new(|cx| TaskDetail::new(store.clone(), window, cx));
        cx.subscribe(&detail, |this, _, _: &TaskUpdated, cx| this.reload(cx)).detach();
        cx.subscribe(&detail, |this, _, OpenTask(id), cx| this.select(*id, cx)).detach();

        let mut this = Self {
            store,
            view: View::Inbox,
            tasks: Vec::new(),
            counts: ViewCounts::default(),
            lists: Vec::new(),
            tags: Vec::new(),
            show_lists: true,
            collapsed_tasks: HashSet::new(),
            // As in TickTick, completed tasks sit collapsed at the bottom of the list.
            collapsed_sections: HashSet::from([Section::Completed]),
            input,
            list_focus: cx.focus_handle(),
            new_list,
            creating_list: false,
            rename_input,
            renaming_list: false,
            search_input,
            header_menu_open: false,
            detail,
            selected: None,
            error: None,
        };
        this.reload(cx);
        #[cfg(debug_assertions)]
        this.apply_dev_state(window, cx);
        this
    }

    /// Debug builds only: puts the UI into a given state without clicks, for screenshots.
    /// `FERN_DEV_STATE="add=<quick-add text>;list=<new list>;view=<list name>;select=<title part>;
    /// popup=due|priority|tags|list;menu=header;new-list=1"`
    #[cfg(debug_assertions)]
    fn apply_dev_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(state) = std::env::var("FERN_DEV_STATE") else { return };
        for part in state.split(';') {
            match part.split_once('=') {
                Some(("add", text)) => {
                    let result = self.add_task(text);
                    self.check(result);
                    self.reload(cx);
                }
                Some(("list", name)) => self.create_list(name, cx),
                Some(("view", "trash")) => self.set_view(View::Trash, cx),
                Some(("drop", spec)) => {
                    // `drop=<title part>><today|tomorrow|inbox|completed|trash|list:Name|tag:Name>`:
                    // the same path as dropping a task on the lists column.
                    let all = self.store.tasks().unwrap_or_default();
                    if let Some((needle, target)) = spec.split_once('>')
                        && let Some(id) = all.iter().find(|t| t.title.contains(needle)).map(|t| t.id)
                    {
                        let view = match target {
                            "today" => Some(View::Today),
                            "tomorrow" => Some(View::Tomorrow),
                            "inbox" => Some(View::Inbox),
                            "completed" => Some(View::Completed),
                            "trash" => Some(View::Trash),
                            other => other
                                .strip_prefix("tag:")
                                .map(|t| View::Tag(t.to_owned()))
                                .or_else(|| {
                                    let name = other.strip_prefix("list:")?;
                                    self.lists.iter().find(|l| l.name == name).map(|l| View::List(l.id))
                                }),
                        };
                        if let Some(view) = view {
                            self.drop_on_view(view, id, cx);
                        }
                    }
                }
                Some(("move", spec)) => {
                    // `move=<dragged title part>><target title part>`: same path as a mouse drop.
                    let find = |needle: &str| self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id);
                    if let Some((dragged, target)) = spec.split_once('>')
                        && let (Some(dragged), Some(target)) = (find(dragged), find(target))
                    {
                        self.drop_task(dragged, target, cx);
                    }
                }
                Some(("search", query)) => {
                    let query = query.to_owned();
                    self.search_input.update(cx, |input, cx| input.set_text(query, cx));
                    self.start_search(window, cx);
                }
                Some(("view", name)) => {
                    if let Some(id) = self.lists.iter().find(|l| l.name == name).map(|l| l.id) {
                        self.set_view(View::List(id), cx);
                    }
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
                Some(("menu", "header")) => self.header_menu_open = true,
                Some(("delete", needle)) => {
                    if let Some(id) = self.tasks.iter().find(|t| t.title.contains(needle)).map(|t| t.id) {
                        self.delete(id, cx);
                    }
                }
                Some(("rename", _)) => self.start_rename(window, cx),
                Some(("new-list", _)) => self.start_new_list(window, cx),
                _ => {}
            }
        }
    }

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let today = Self::today();
        if let Some(lists) = self.check(self.store.lists()) {
            self.lists = lists;
        }
        // The current list may have been deleted.
        if let View::List(id) = self.view
            && !self.lists.iter().any(|l| l.id == id)
        {
            self.view = View::Inbox;
        }
        if let Some(tasks) = self.check(self.store.tasks_in(&self.view, today)) {
            self.tasks = tasks;
        }
        if let Some(counts) = self.check(self.store.counts(today)) {
            self.counts = counts;
        }
        if let Some(tags) = self.check(self.store.tags()) {
            self.tags = tags;
        }
        // The details panel keeps showing a task as long as it exists, even when it has left
        // the current view (for example, after being completed in "Today").
        let selected = self.selected.and_then(|id| self.store.task(id).ok().flatten());
        self.selected = selected.as_ref().map(|t| t.id);
        let lists = self.lists.clone();
        self.detail.update(cx, |detail, cx| {
            detail.set_lists(lists);
            detail.refresh(selected, cx);
        });
        cx.notify();
    }

    /// Creates a task from the quick-add line: due date, time, tags, priority and list come from
    /// the text ("tomorrow 6pm #home !1 ^work"); anything missing comes from the current view —
    /// a due date in Today/Tomorrow, the tag in a tag view, the list in a list view.
    fn add_task(&self, input: &str) -> anyhow::Result<Task> {
        let today = Self::today();
        let list_names: Vec<String> = self.lists.iter().map(|l| l.name.clone()).collect();
        let parsed = fern_core::parse_quick_add(input, today, &list_names);
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
        if let Some(repeat) = parsed.repeat {
            self.store.set_repeat(task.id, Some(repeat), today)?;
        }
        if let Some(priority) = parsed.priority {
            self.store.set_priority(task.id, priority)?;
        }

        let parsed_list =
            parsed.list.and_then(|name| self.lists.iter().find(|l| l.name == name)).map(|l| l.id);
        let view_list = match self.view {
            View::List(id) => Some(id),
            _ => None,
        };
        if let Some(list) = parsed_list.or(view_list) {
            self.store.set_list(task.id, Some(list))?;
        }
        Ok(task)
    }

    /// Focuses the quick-add field.
    pub fn focus_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_read_only_view() {
            self.set_view(View::Inbox, cx);
        }
        window.focus(&self.input.focus_handle(cx), cx);
    }

    /// Task ids in the order they are shown, for keyboard navigation.
    fn visible_task_ids(&self) -> Vec<Id> {
        task_rows::build_rows(&self.tasks, &self.collapsed_tasks, &self.collapsed_sections, self.view != View::Trash)
            .into_iter()
            .filter_map(|row| match row {
                Row::Task { task, .. } => Some(task.id),
                Row::Section { .. } => None,
            })
            .collect()
    }

    fn move_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let ids = self.visible_task_ids();
        if ids.is_empty() {
            return;
        }
        let next = match self.selected.and_then(|id| ids.iter().position(|i| *i == id)) {
            Some(index) => (index as isize + step).clamp(0, ids.len() as isize - 1) as usize,
            None if step > 0 => 0,
            None => ids.len() - 1,
        };
        self.select(ids[next], cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    fn toggle_selected(&mut self, _: &ToggleComplete, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(task) = self.selected.and_then(|id| self.tasks.iter().find(|t| t.id == id)) {
            let (id, done) = (task.id, task.is_completed());
            self.toggle(id, !done, cx);
        }
    }

    fn delete_selected(&mut self, _: &DeleteTask, _: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        // Keep a selection so repeated presses keep working down the list.
        let ids = self.visible_task_ids();
        let neighbor = ids
            .iter()
            .position(|i| *i == id)
            .and_then(|index| ids.get(index + 1).or_else(|| index.checked_sub(1).and_then(|i| ids.get(i))))
            .copied();
        self.delete(id, cx);
        if let Some(neighbor) = neighbor {
            self.select(neighbor, cx);
        }
    }

    pub fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        self.renaming_list = false;
        if self.view != view {
            self.view = view;
            self.reload(cx);
        }
    }

    /// Switches the list to search results and focuses the search field.
    pub fn start_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).text().to_owned();
        self.set_view(View::Search(query), cx);
        window.focus(&self.search_input.focus_handle(cx), cx);
        cx.notify();
    }

    /// Shows the full results for `query` in the list (the popover's "View more").
    pub fn show_search_results(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_input.update(cx, |input, cx| input.set_text(query.clone(), cx));
        self.set_view(View::Search(query), cx);
    }

    /// Leaves search results for the Inbox.
    pub fn stop_search(&mut self, cx: &mut Context<Self>) {
        if matches!(self.view, View::Search(_)) {
            self.set_view(View::Inbox, cx);
        }
    }

    fn start_new_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.creating_list = true;
        self.show_lists = true;
        window.focus(&self.new_list.focus_handle(cx), cx);
        cx.notify();
    }

    fn create_list(&mut self, name: &str, cx: &mut Context<Self>) {
        self.creating_list = false;
        if name.trim().is_empty() {
            cx.notify();
            return;
        }
        let result = self.store.create_list(name);
        if let Some(list) = self.check(result) {
            self.view = View::List(list.id);
        }
        self.reload(cx);
    }

    fn current_list(&self) -> Option<&List> {
        match self.view {
            View::List(id) => self.lists.iter().find(|l| l.id == id),
            _ => None,
        }
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.current_list().map(|l| l.name.clone()) else { return };
        self.header_menu_open = false;
        self.renaming_list = true;
        self.rename_input.update(cx, |input, cx| input.set_text(name, cx));
        window.focus(&self.rename_input.focus_handle(cx), cx);
        cx.notify();
    }

    fn rename_current_list(&mut self, name: &str, cx: &mut Context<Self>) {
        self.renaming_list = false;
        if let View::List(id) = self.view
            && !name.trim().is_empty()
        {
            let result = self.store.rename_list(id, name);
            self.check(result);
        }
        self.reload(cx);
    }

    fn set_current_list_color(&mut self, color: Option<&str>, cx: &mut Context<Self>) {
        if let View::List(id) = self.view {
            let result = self.store.set_list_color(id, color);
            self.check(result);
        }
        self.header_menu_open = false;
        self.reload(cx);
    }

    fn delete_current_list(&mut self, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        if let View::List(id) = self.view {
            let result = self.store.delete_list(id);
            self.check(result);
            self.view = View::Inbox;
        }
        self.reload(cx);
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

    pub fn toggle_lists(&mut self, cx: &mut Context<Self>) {
        self.show_lists = !self.show_lists;
        cx.notify();
    }

    /// Database errors are shown in a line at the bottom of the window instead of crashing.
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

    pub fn select(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.selected == Some(id) {
            return;
        }
        self.selected = Some(id);
        // The task may be outside the current view, for example a subtask opened from the panel.
        let task = self
            .tasks
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .or_else(|| self.store.task(id).ok().flatten());
        self.detail.update(cx, |detail, cx| detail.show(task, cx));
        cx.notify();
    }

    fn toggle(&mut self, id: Id, completed: bool, cx: &mut Context<Self>) {
        let result = self.store.set_completed(id, completed);
        self.check(result);
        self.reload(cx);
    }

    /// Moves a task to the trash, or deletes it for good when it is already there.
    fn delete(&mut self, id: Id, cx: &mut Context<Self>) {
        let result = if self.view == View::Trash {
            self.store.purge_task(id)
        } else {
            self.store.delete_task(id)
        };
        self.check(result);
        self.reload(cx);
    }

    /// A task dropped on an entry of the lists column takes that entry's meaning: a list moves it,
    /// a tag tags it, Today/Tomorrow set its date, Completed completes it, Trash deletes it.
    fn drop_on_view(&mut self, view: View, id: Id, cx: &mut Context<Self>) {
        let today = Self::today();
        let time = self.store.task(id).ok().flatten().and_then(|t| t.due_time);
        let result = match &view {
            View::Inbox => self.store.set_list(id, None),
            View::List(list) => self.store.set_list(id, Some(*list)),
            View::Tag(tag) => match self.store.task(id) {
                Ok(Some(task)) if !task.tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) => {
                    let mut tags = task.tags;
                    tags.push(tag.clone());
                    self.store.set_tags(id, &tags)
                }
                other => other.map(|_| ()),
            },
            View::Today | View::Next7Days => self.store.set_due(id, Some(today), time),
            View::Tomorrow => self.store.set_due(id, Some(today + Duration::days(1)), time),
            View::Completed => self.store.set_completed(id, true),
            View::Trash => self.store.delete_task(id),
            View::All | View::Search(_) => Ok(()),
        };
        self.check(result);
        self.reload(cx);
    }

    /// Drops a dragged task right before `target`.
    fn drop_task(&mut self, dragged: Id, target: Id, cx: &mut Context<Self>) {
        let result = self.store.move_task(dragged, target, false);
        self.check(result);
        self.reload(cx);
    }

    fn restore(&mut self, id: Id, cx: &mut Context<Self>) {
        let result = self.store.restore_task(id);
        self.check(result);
        self.reload(cx);
    }

    fn empty_trash(&mut self, cx: &mut Context<Self>) {
        self.header_menu_open = false;
        let result = self.store.empty_trash();
        self.check(result);
        self.reload(cx);
    }

    /// Views where new tasks cannot be added.
    fn is_read_only_view(&self) -> bool {
        matches!(self.view, View::Completed | View::Trash | View::Search(_))
    }

    fn render_section(
        &self,
        section: Section,
        count: usize,
        collapsed: bool,
        strings: &Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .id(SharedString::from(format!("group-{section:?}")))
            .h(px(36.))
            .gap_1()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx)))
            .child(chevron(collapsed))
            .child(Label::new(section.title(strings)).weight(FontWeight::BOLD))
            .child(div().pl_1().child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted)))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_task(
        &self,
        task: &Task,
        depth: usize,
        has_children: bool,
        collapsed: bool,
        tag_colors: &HashMap<String, Hsla>,
        today: NaiveDate,
        strings: &Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let id = task.id;
        let done = task.is_completed();
        let selected = self.selected == Some(id);
        let group = format!("task-{id}");

        let marker = if task.kind == Kind::Note {
            Icon::new(IconName::FileText).into_any_element()
        } else {
            Checkbox::new("marker")
                .toggle_state(done)
                .tint(task_rows::priority_color(task.priority, &theme))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle(id, !done, cx);
                }))
                .into_any_element()
        };

        let due = task.due_date.map(|date| task_rows::due_label(date, task.due_time, today, strings));

        let draggable = self.view != View::Trash;
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        let accent = theme.colors().accent;

        // The separator starts at the checkbox; the gutter to its left holds the chevron.
        div()
            .id(ElementId::Uuid(id))
            .group(group.clone())
            .when(draggable, |el| {
                el.on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                    .drag_over::<DraggedTask>(move |style, _, _, _| style.border_t_2().border_color(accent))
                    .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                        this.drop_task(dragged.id, id, cx)
                    }))
            })
            .h(px(ROW_HEIGHT))
            .flex()
            .pl(px(depth as f32 * INDENT))
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme.colors().ghost_element_selected))
            .when(!selected, |el| el.hover(|s| s.bg(theme.colors().ghost_element_hover)))
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.list_focus, cx);
                this.select(id, cx);
            }))
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
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap(px(10.))
                    .pr_3()
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(marker)
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(if task.title.is_empty() {
                                SharedString::new_static(strings.untitled)
                            } else {
                                task.title.clone().into()
                            })
                            .color(if done || task.title.is_empty() { Color::Muted } else { Color::Default })
                            .strikethrough(done)
                            .truncate(),
                        ),
                    )
                    .children(task.tags.iter().map(|tag| {
                        let color = tag_colors.get(tag).copied().unwrap_or(theme.colors().text_muted);
                        Chip::new(tag.clone(), color)
                    }))
                    .when_some(task.checklist, |el, (done_items, total)| {
                        el.child(
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .child(Icon::new(IconName::ListChecks).size(IconSize::XSmall))
                                .child(
                                    Label::new(format!("{done_items}/{total}"))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                        )
                    })
                    .when_some(due, |el, (label, overdue)| {
                        el.when(task.repeat.is_some(), |el| {
                            el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Accent))
                        })
                        .child(
                            Label::new(label)
                                .size(LabelSize::Small)
                                .color(if overdue && !done { Color::Error } else { Color::Accent }),
                        )
                    })
                    .when(self.view == View::Trash, |el| {
                        el.child(
                            div()
                                .id("restore")
                                .flex_none()
                                .opacity(0.)
                                .group_hover(group.clone(), |s| s.opacity(1.))
                                .child(
                                    IconButton::new("restore-button", IconName::RotateCcw)
                                        .size(px(20.))
                                        .icon_size(IconSize::Small)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.restore(id, cx);
                                        })),
                                ),
                        )
                    })
                    .child(
                        div()
                            .id("delete")
                            .flex_none()
                            .px_1()
                            .text_color(theme.colors().text_muted)
                            .opacity(0.)
                            .group_hover(group, |s| s.opacity(1.))
                            .hover(|s| s.text_color(theme.status().error))
                            .child("✕")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.delete(id, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_header_menu(&self, strings: &Strings, cx: &mut Context<Self>) -> AnyElement {
        let in_list = matches!(self.view, View::List(_));
        let in_trash = self.view == View::Trash;
        popover(cx)
            .w(px(236.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.header_menu_open = false;
                cx.notify();
            }))
            .when(in_list, |el| {
                let current = self.current_list().and_then(|l| l.color.clone());
                el.child(
                    ListItem::new("rename-list")
                        .start_slot(Icon::new(IconName::Pencil))
                        .child(Label::new(strings.rename_list))
                        .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx))),
                )
                .child(div().px_2().pt_1().child(Label::new(strings.color).size(LabelSize::Small).color(Color::Muted)))
                .child(
                    h_flex()
                        .px_2()
                        .py_1()
                        .flex_wrap()
                        .gap_1p5()
                        .child(
                            color_swatch("color-none", None, current.is_none(), cx)
                                .on_click(cx.listener(|this, _, _, cx| this.set_current_list_color(None, cx))),
                        )
                        .children(LIST_COLORS.into_iter().map(|color| {
                            color_swatch(color, Some(color), current.as_deref() == Some(color), cx).on_click(
                                cx.listener(move |this, _, _, cx| this.set_current_list_color(Some(color), cx)),
                            )
                        })),
                )
                .child(div().my_1().child(Divider::horizontal()))
                .child(
                    ListItem::new("delete-list")
                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                        .child(Label::new(strings.delete_list).color(Color::Error))
                        .on_click(cx.listener(|this, _, _, cx| this.delete_current_list(cx))),
                )
            })
            .when(in_trash, |el| {
                el.child(
                    ListItem::new("empty-trash")
                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                        .child(Label::new(strings.empty_trash).color(Color::Error))
                        .on_click(cx.listener(|this, _, _, cx| this.empty_trash(cx))),
                )
            })
            .when(!in_list && !in_trash, |el| {
                el.child(div().px_2().py_1().child(Label::new(strings.no_actions).color(Color::Muted)))
            })
            .into_any_element()
    }
}

/// A round color option; `None` is "no color", drawn as an outlined circle.
fn color_swatch(id: &'static str, color: Option<&str>, selected: bool, cx: &App) -> Stateful<Div> {
    let colors = cx.theme().colors();
    let ring = if selected { colors.text } else { gpui::transparent_black() };
    div()
        .id(id)
        .size(px(18.))
        .rounded_full()
        .cursor_pointer()
        .map(|el| match lists_panel::parse_color(color) {
            Some(fill) => el.bg(fill).border_2().border_color(ring),
            None => el.border_1().border_color(if selected { colors.text } else { colors.text_muted }),
        })
}

fn chevron(collapsed: bool) -> Icon {
    Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(IconSize::XSmall)
}

impl Focusable for TodoView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for TodoView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let input_focus = self.input.focus_handle(cx);
        let today = Self::today();
        let tag_colors: HashMap<String, Hsla> = self
            .tags
            .iter()
            .map(|t| (t.name.clone(), lists_panel::color(t.color.as_deref(), cx)))
            .collect();
        let rows = task_rows::build_rows(&self.tasks, &self.collapsed_tasks, &self.collapsed_sections, self.view != View::Trash);
        let rows: Vec<_> = rows
            .iter()
            .map(|row| match row {
                Row::Section { section, count, collapsed } => {
                    self.render_section(*section, *count, *collapsed, strings, cx)
                }
                Row::Task { task, depth, has_children, collapsed } => self.render_task(
                    task,
                    *depth,
                    *has_children,
                    *collapsed,
                    &tag_colors,
                    today,
                    strings,
                    cx,
                ),
            })
            .collect();

        let list = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .h(px(60.))
                    .flex_none()
                    .gap_2()
                    .pl(px(24.))
                    .pr(px(16.))
                    .child(
                        IconButton::new("toggle-sidebar", IconName::PanelLeft)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_lists(cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(20.))
                            .font_weight(FontWeight::BOLD)
                            .map(|el| {
                                if matches!(self.view, View::Search(_)) {
                                    el.child(
                                        h_flex()
                                            .gap_2()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::NORMAL)
                                            .child(Icon::new(IconName::Search).size(IconSize::Large))
                                            .child(self.search_input.clone()),
                                    )
                                } else if self.renaming_list {
                                    el.child(self.rename_input.clone())
                                } else {
                                    el.truncate().child(lists_panel::title(&self.view, &self.lists, strings))
                                }
                            }),
                    )
                    .when(self.view == View::Trash, |el| {
                        el.child(
                            IconButton::new("empty-trash-button", IconName::BrushCleaning)
                                .on_click(cx.listener(|this, _, _, cx| this.empty_trash(cx))),
                        )
                    })
                    .when(self.view != View::Trash, |el| el.child(IconButton::new("sort", IconName::ArrowDownUp)))
                    .when(self.view != View::Trash, |el| el.child(
                        div()
                            .relative()
                            .child(IconButton::new("more", IconName::Ellipsis).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.header_menu_open = true;
                                    cx.notify();
                                },
                            )))
                            .when(self.header_menu_open, |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    self.render_header_menu(strings, cx),
                                ))
                            }),
                    )),
            )
            .when(!self.is_read_only_view(), |el| el.child(
                h_flex()
                    .mx_2()
                    .mb_2()
                    .h(px(38.))
                    .flex_none()
                    .gap_2()
                    .px_3()
                    .rounded(px(6.))
                    .bg(theme.colors().surface_background)
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&input_focus, cx);
                    })
                    .child(Icon::new(IconName::Plus))
                    .child(self.input.clone()),
            ))
            .child(
                div()
                    .id("tasks")
                    .track_focus(&self.list_focus)
                    .key_context("TaskList")
                    .on_action(cx.listener(Self::select_next))
                    .on_action(cx.listener(Self::select_previous))
                    .on_action(cx.listener(Self::toggle_selected))
                    .on_action(cx.listener(Self::delete_selected))
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_4()
                    .when(self.tasks.is_empty(), |el| {
                        el.child(div().px_3().pt_8().child(
                            Label::new(match &self.view {
                                View::Completed => strings.no_completed_tasks,
                                View::Trash => strings.trash_is_empty,
                                View::Search(query) if query.trim().is_empty() => strings.search_hint,
                                View::Search(_) => strings.nothing_found,
                                _ => strings.no_tasks,
                            })
                            .color(Color::Muted),
                        ))
                    })
                    .children(rows),
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .px_6()
                        .py_2()
                        .border_t_1()
                        .border_color(theme.colors().border)
                        .child(Label::new(error).color(Color::Error)),
                )
            });

        let this = cx.entity().downgrade();
        let on_select = move |view, _: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.set_view(view, cx)).ok();
        };
        let on_add_list = cx.listener(|this, _, window, cx| this.start_new_list(window, cx));
        let this = cx.entity().downgrade();
        let on_drop_task = move |view, id, _: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.drop_on_view(view, id, cx)).ok();
        };

        h_flex()
            .size_full()
            .items_start()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .text_size(px(14.))
            .when(self.show_lists, |el| {
                el.child(lists_panel::render(
                    lists_panel::Props {
                        active: &self.view,
                        counts: &self.counts,
                        lists: &self.lists,
                        tags: &self.tags,
                        strings,
                        new_list_input: self.creating_list.then(|| self.new_list.clone().into_any_element()),
                    },
                    on_select,
                    on_add_list,
                    on_drop_task,
                    cx,
                ))
            })
            .child(list)
            .child(div().flex_none().w(px(DETAIL_WIDTH)).h_full().child(self.detail.clone()))
    }
}
