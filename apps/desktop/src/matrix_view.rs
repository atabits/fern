//! The Eisenhower Matrix, matched to TickTick's: four quadrants, each with a rule on lists, tags,
//! dates and priorities (by default the original's: urgent is overdue to tomorrow, important is
//! high or medium priority), tasks grouped by list inside each quadrant. Dragging a task to another
//! quadrant gives it that quadrant's priority.

use std::collections::HashMap;
use std::rc::Rc;

use chrono::Local;
use fern_core::matrix::{MatrixDate, QuadrantRule, quadrant_of};
use fern_core::{Id, List, Priority, Store, Task, TaskDraft, View};
use gpui::{Entity, EventEmitter, FocusHandle, Focusable, FontWeight};
use ui::{Placement, PopoverAttachment, popover};

use crate::actions::DismissTransient;
use crate::text_input::{Mode, Submitted, TextInput};
use ui::prelude::*;

use crate::drag::DraggedTask;

use crate::i18n::{self, Strings};
use crate::lists_panel;
use crate::task_rows::{self, GroupBy, SortBy};

/// The user clicked a task in the matrix.
pub struct OpenTask(pub Id);

/// A quadrant's Group by (`None` is by list, the original's default), Sort by and Order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuadrantView {
    pub group: Option<GroupBy>,
    pub sort: SortBy,
    pub newest_first: bool,
}

impl Default for QuadrantView {
    /// The original's: by list, by time, oldest first.
    fn default() -> Self {
        Self { group: None, sort: SortBy::Date, newest_first: false }
    }
}

impl QuadrantView {
    fn to_storage(self) -> String {
        format!("{},{},{}", self.group.map_or("list", GroupBy::key), self.sort.key(), if self.newest_first { "newest" } else { "oldest" })
    }

    fn from_storage(text: &str) -> Self {
        let mut parts = text.split(',');
        let group = parts.next().unwrap_or("list");
        let sort = parts.next().map(SortBy::from_key).unwrap_or(SortBy::Date);
        let newest_first = parts.next() == Some("newest");
        // Anything but a list grouping the list menu offers falls back to grouping by list.
        let group = match GroupBy::from_key(group) {
            GroupBy::Custom => None,
            other => Some(other),
        };
        Self { group, sort, newest_first }
    }
}

/// A quadrant's tasks as groups (a label, empty for no header, and its tasks), grouped and
/// sorted the quadrant's way. "Oldest first" is the earliest date or creation first.
pub fn arrange(mut tasks: Vec<Task>, view: QuadrantView, lists: &[List], today: chrono::NaiveDate, strings: &Strings) -> Vec<(String, Vec<Task>)> {
    task_rows::sort_tasks(&mut tasks, view.sort);
    // Created and Modified sort newest first, the others in their natural order.
    let natural_newest = matches!(view.sort, SortBy::Created | SortBy::Modified);
    if natural_newest != view.newest_first {
        tasks.reverse();
    }
    match view.group {
        None => {
            // By list, keeping the order in which lists first appear.
            let mut groups: Vec<(Option<Id>, Vec<Task>)> = Vec::new();
            let mut index: HashMap<Option<Id>, usize> = HashMap::new();
            for task in tasks {
                let at = *index.entry(task.list_id).or_insert_with(|| {
                    groups.push((task.list_id, Vec::new()));
                    groups.len() - 1
                });
                groups[at].1.push(task);
            }
            groups
                .into_iter()
                .map(|(list, tasks)| {
                    let name = list.and_then(|id| lists.iter().find(|l| l.id == id)).map_or(strings.inbox.to_owned(), |l| l.name.clone());
                    (name, tasks)
                })
                .collect()
        }
        Some(group) => {
            let mut groups: Vec<(String, Vec<Task>)> = Vec::new();
            for row in task_rows::grouped_rows(&tasks, group, today, false, strings) {
                match row {
                    task_rows::Row::Day { label, .. } => groups.push((label, Vec::new())),
                    task_rows::Row::Task { task, .. } => match groups.last_mut() {
                        Some((_, tasks)) => tasks.push(task),
                        None => groups.push((String::new(), vec![task])),
                    },
                    task_rows::Row::Section { .. } => {}
                }
            }
            groups
        }
    }
}

pub struct MatrixView {
    store: Rc<Store>,
    focus: FocusHandle,
    tasks: Vec<Task>,
    lists: Vec<List>,
    rules: [QuadrantRule; 4],
    /// Each quadrant's Group by, Sort by and Order, and the menu row whose choices are open.
    views: [QuadrantView; 4],
    menu_row: Option<u8>,
    /// The quadrant whose header "…" menu is open.
    menu: Option<usize>,
    /// "Edit Matrix": the quadrant and its rule being edited, with the name field.
    editing: Option<(usize, QuadrantRule)>,
    name_input: Entity<TextInput>,
    tags: Vec<fern_core::Tag>,
    /// A quadrant's "+": the quadrant and its field for the new task's title.
    adding: Option<(usize, Entity<TextInput>)>,
    add_notice: Option<AddNotice>,
    /// The header's "…" menu, and its Show Completed choice (kept in the settings).
    header_menu: bool,
    show_completed: bool,
}

enum AddNotice {
    SavedElsewhere(SharedString),
    Failed,
}

impl EventEmitter<OpenTask> for MatrixView {}

impl Focusable for MatrixView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl MatrixView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|cx| TextInput::new(Mode::SingleLine, "", cx).without_cancel());
        let mut this = Self {
            store,
            focus: cx.focus_handle(),
            tasks: Vec::new(),
            lists: Vec::new(),
            rules: QuadrantRule::defaults(),
            views: [QuadrantView::default(); 4],
            menu_row: None,
            menu: None,
            editing: None,
            name_input,
            tags: Vec::new(),
            adding: None,
            add_notice: None,
            header_menu: false,
            show_completed: false,
        };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        self.show_completed = self.store.setting("matrix_show_completed").ok().flatten().is_some_and(|v| v == "1");
        let show_completed = self.show_completed;
        self.tasks = self
            .store
            .tasks_in(&View::All, today)
            .unwrap_or_default()
            .into_iter()
            .filter(|t| show_completed || !t.is_completed())
            .collect();
        self.lists = self.store.lists().unwrap_or_default();
        // An empty list filter means all lists. Keep explicit selections even when a list
        // is unavailable; only quick add needs to pick a writable destination.
        self.rules = self.store.matrix_rules().unwrap_or_else(|_| QuadrantRule::defaults());
        for (i, view) in self.views.iter_mut().enumerate() {
            *view = self.store.setting(&format!("matrix_view_{i}")).ok().flatten().map(|s| QuadrantView::from_storage(&s)).unwrap_or_default();
        }
        self.tags = self.store.tags().unwrap_or_default();
        cx.notify();
    }

    /// A quadrant's "+": a field at the quadrant's top; the task it adds gets what the quadrant's
    /// rule asks for, so that it lands there.
    fn start_add(&mut self, quadrant: usize, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.matrix_add_placeholder, cx).without_cancel());
        cx.subscribe_in(&input, window, move |this, input, Submitted(title), window, cx| {
            if title.trim().is_empty() {
                return;
            }
            let today = Local::now().date_naive();
            let task = match this.add_quadrant_task(quadrant, title, today) {
                Ok(task) => task,
                Err(err) => {
                    eprintln!("failed to add the task: {err:#}");
                    this.add_notice = Some(AddNotice::Failed);
                    cx.notify();
                    return;
                }
            };
            input.update(cx, |input, cx| input.clear(window, cx));
            this.reload(cx);
            this.add_notice = this.destination_notice(quadrant, &task, today, i18n::t(cx));
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.adding = Some((quadrant, input));
        self.add_notice = None;
        cx.notify();
    }

    fn destination_notice(&self, quadrant: usize, task: &Task, today: chrono::NaiveDate, strings: &Strings) -> Option<AddNotice> {
        // An explicit date, priority or list can place the task elsewhere. Tell the user
        // where the matrix actually put it, including when no rule matches.
        let shown = self.tasks.iter().any(|visible| visible.id == task.id);
        let actual = shown.then(|| quadrant_of(&self.rules, task, today)).flatten();
        if actual == Some(quadrant) {
            return None;
        }
        let destination = match actual {
            Some(actual) => self.quadrant_title(actual, strings).1,
            None => task.list_id.and_then(|id| self.lists.iter().find(|list| list.id == id))
                .map_or_else(|| strings.inbox.into(), |list| list.name.clone().into()),
        };
        Some(AddNotice::SavedElsewhere(destination))
    }

    fn add_quadrant_task(&self, quadrant: usize, input: &str, today: chrono::NaiveDate) -> anyhow::Result<Task> {
        let preferences = self.store.preferences()?;
        let lists = self.store.lists()?;
        let available = |id: Id| lists.iter().any(|list| list.id == id && !list.archived);
        let list_names: Vec<_> = lists.iter().filter(|list| !list.archived).map(|list| list.name.clone()).collect();
        let mut parsed = fern_core::parse_quick_add_with(input, today, &list_names, preferences.recognition);
        let title = if parsed.title.is_empty() { input.trim() } else { &parsed.title };
        let rule = &self.rules[quadrant];
        let (priority, date) = rule.new_task_defaults(today);
        let preferred = preferences.default_list.filter(|id| available(*id));
        let default_list = if rule.lists.contains(&preferred) {
            preferred
        } else {
            rule.lists.iter().copied().find(|id| id.is_none_or(available)).unwrap_or(preferred)
        };
        let selected_list = parsed.list.and_then(|name| lists.iter().find(|list| list.name == name && !list.archived));
        let list_id = selected_list.map(|list| list.id).or(default_list);
        let kind = lists.iter().find(|list| Some(list.id) == list_id).map_or(fern_core::Kind::Task, |list| list.kind);
        let defaults = &preferences.task_defaults;
        let default_tag = rule.tags.first().or(defaults.tag.as_ref());
        if let Some(tag) = default_tag
            && !rule.tags.iter().any(|required| parsed.tags.iter().any(|tag| required.to_lowercase() == tag.to_lowercase()))
            && !parsed.tags.iter().any(|existing| existing.to_lowercase() == tag.to_lowercase())
        {
            parsed.tags.push(tag.clone());
        }
        let due_date = parsed.due_date.filter(|_| parsed.explicit_date).or(date)
            .or_else(|| defaults.date.filter(|_| rule.dates.is_empty()).map(|date| date.date(today)))
            .or(parsed.due_date)
            .or_else(|| parsed.repeat.is_some().then_some(today));
        let reminder = if parsed.due_time.is_some() { defaults.timed_reminder } else { defaults.all_day_reminder };
        self.store.create_task(TaskDraft {
            title: title.to_owned(),
            list_id,
            kind,
            priority: parsed.priority.or(priority).unwrap_or(defaults.priority),
            due_date,
            due_time: parsed.due_time,
            repeat: parsed.repeat,
            reminders: reminder.filter(|_| due_date.is_some()).into_iter().collect(),
            tags: parsed.tags,
            ..Default::default()
        }, !defaults.add_to_bottom)
    }

    fn dismiss_transient(&mut self, _: &DismissTransient, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.finish_edit(None, cx);
        } else if self.menu_row.take().is_some() {
            cx.notify();
            return;
        } else if self.menu.take().is_some() || std::mem::take(&mut self.header_menu) {
            cx.notify();
        } else if self.adding.take().is_some() || self.add_notice.take().is_some() {
            cx.notify();
        } else {
            cx.propagate();
            return;
        }
        let focus = self.adding.as_ref().map_or_else(
            || self.focus.clone(),
            |(_, input)| input.focus_handle(cx),
        );
        window.focus(&focus, cx);
    }

    #[cfg(debug_assertions)]
    pub fn dev_header(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            "header-menu" => self.header_menu = true,
            "add" => self.start_add(0, window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn set_priority(&mut self, id: Id, priority: Priority, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_priority(id, priority) {
            eprintln!("failed to change the priority: {err:#}");
        }
        self.reload(cx);
    }

    fn complete(&mut self, id: Id, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_completed(id, true) {
            eprintln!("failed to complete the task: {err:#}");
        }
        self.reload(cx);
    }

    /// The original's title of a quadrant.
    fn default_title(quadrant: usize, strings: &Strings) -> &'static str {
        [
            strings.matrix_urgent_important,
            strings.matrix_not_urgent_important,
            strings.matrix_urgent_unimportant,
            strings.matrix_not_urgent_unimportant,
        ][quadrant]
    }

    /// A quadrant's numeral and title: its own name, or the original's.
    fn quadrant_title(&self, quadrant: usize, strings: &Strings) -> (&'static str, SharedString) {
        let name = &self.rules[quadrant].name;
        let title = if name.trim().is_empty() { Self::default_title(quadrant, strings).into() } else { name.clone().into() };
        (["I", "II", "III", "IV"][quadrant], title)
    }

    fn quadrant_color(quadrant: usize, cx: &App) -> Hsla {
        let status = cx.theme().status();
        [status.error, status.warning, status.info, status.success][quadrant]
    }

    /// A task dropped on a quadrant takes its rule's first priority, unless it already has one
    /// of them.
    fn drop_on(&mut self, id: Id, quadrant: usize, cx: &mut Context<Self>) {
        let priorities = &self.rules[quadrant].priorities;
        let current = self.tasks.iter().find(|t| t.id == id).map(|t| t.priority);
        if let Some(first) = priorities.first().copied()
            && !current.is_some_and(|p| priorities.contains(&p))
        {
            self.set_priority(id, first, cx);
        }
    }

    fn start_edit(&mut self, quadrant: usize, cx: &mut Context<Self>) {
        self.menu = None;
        let rule = self.rules[quadrant].clone();
        let (_, title) = self.quadrant_title(quadrant, i18n::t(cx));
        self.name_input.update(cx, |input, cx| input.set_text(title, cx));
        self.editing = Some((quadrant, rule));
        cx.notify();
    }

    fn finish_edit(&mut self, save: Option<bool>, cx: &mut Context<Self>) {
        // `Some(true)` saves, `Some(false)` restores the default, `None` closes.
        if let (Some(save), Some((quadrant, mut rule))) = (save, self.editing.take()) {
            let result = if save {
                // The default title stays unnamed, so that it follows the language.
                let typed = self.name_input.read(cx).text().trim().to_owned();
                rule.name = if typed == Self::default_title(quadrant, i18n::t(cx)) { String::new() } else { typed };
                self.store.save_matrix_rule(quadrant, Some(&rule))
            } else {
                self.store.save_matrix_rule(quadrant, None)
            };
            if let Err(err) = result {
                eprintln!("failed to save the quadrant: {err:#}");
            }
        }
        self.editing = None;
        self.reload(cx);
    }

    /// The "…" menu's Group by, Sort by and Order, as in the original: each row shows its choice
    /// and a click lists the choices under it.
    fn render_view_rows(&self, quadrant: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let view = self.views[quadrant];
        let group_label = |group: Option<GroupBy>| group.map_or(strings.filter_lists, |g| g.label(strings));
        let rows: [(u8, IconName, &'static str, &'static str, Vec<(&'static str, QuadrantView)>); 3] = [
            (
                0,
                IconName::LayoutGrid,
                strings.group_by,
                group_label(view.group),
                std::iter::once(None)
                    .chain([GroupBy::Date, GroupBy::Created, GroupBy::Tag, GroupBy::Priority, GroupBy::None].map(Some))
                    .map(|group| (group_label(group), QuadrantView { group, ..view }))
                    .collect(),
            ),
            (
                1,
                IconName::ArrowDownUp,
                strings.sort_by,
                view.sort.label(strings),
                [SortBy::Date, SortBy::Created, SortBy::Modified, SortBy::Title, SortBy::Tag, SortBy::Priority]
                    .map(|sort| (sort.label(strings), QuadrantView { sort, ..view }))
                    .to_vec(),
            ),
            (
                2,
                IconName::ArrowDownUp,
                strings.order,
                strings.order_options[view.newest_first as usize],
                [false, true].map(|newest_first| (strings.order_options[newest_first as usize], QuadrantView { newest_first, ..view })).to_vec(),
            ),
        ];
        let mut items = Vec::new();
        for (row, icon, label, value, choices) in rows {
            let open = self.menu_row == Some(row);
            items.push(
                ListItem::new(SharedString::from(format!("quadrant-view-{row}")))
                    .start_slot(Icon::new(icon).color(Color::Muted))
                    .child(Label::new(label))
                    .end_slot(
                        h_flex()
                            .gap_1()
                            .child(Label::new(value).size(LabelSize::Small).color(Color::Muted))
                            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::XSmall).color(Color::Muted)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_row = if this.menu_row == Some(row) { None } else { Some(row) };
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            if open {
                for (i, (choice, next)) in choices.into_iter().enumerate() {
                    let selected = next == view;
                    items.push(
                        ListItem::new(SharedString::from(format!("quadrant-view-{row}-{i}")))
                            .child(div().pl_6().child(Label::new(choice).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default })))
                            .when(selected, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.views[quadrant] = next;
                                if let Err(err) = this.store.set_setting(&format!("matrix_view_{quadrant}"), &next.to_storage()) {
                                    eprintln!("failed to save the quadrant's view: {err:#}");
                                }
                                this.menu_row = None;
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
        items
    }

    /// Opens a quadrant's "…" menu (`menu`) or its "Edit Matrix" (`edit`), for the debug-build
    /// dev state.
    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, quadrant: usize, edit: bool, cx: &mut Context<Self>) {
        if edit {
            self.start_edit(quadrant, cx);
        } else {
            self.menu = Some(quadrant);
            self.menu_row = Some(0);
            cx.notify();
        }
    }

    /// The original's "Edit Matrix": the quadrant's name, and its Lists, Tags, Date and
    /// Priority; then Restore, Close or Save.
    fn render_edit(&self, quadrant: usize, rule: &QuadrantRule, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let color = Self::quadrant_color(quadrant, cx);
        let (numeral, _) = self.quadrant_title(quadrant, strings);
        let chip = |id: SharedString, label: SharedString, on: bool, change: Box<dyn Fn(&mut QuadrantRule)>| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded(px(4.))
                .cursor_pointer()
                .bg(if on { theme.colors().accent } else { theme.colors().element_hover })
                .child(Label::new(label).size(LabelSize::XSmall))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some((_, rule)) = this.editing.as_mut() {
                        change(rule);
                    }
                    cx.notify();
                }))
        };
        fn toggle<T: PartialEq + Clone>(items: &mut Vec<T>, item: &T) {
            match items.iter().position(|i| i == item) {
                Some(at) => {
                    items.remove(at);
                }
                None => items.push(item.clone()),
            }
        }
        let row = |label: &'static str, chips: Vec<Stateful<Div>>| {
            h_flex()
                .gap_2()
                .items_start()
                .child(div().w(px(64.)).pt_0p5().child(Label::new(label).size(LabelSize::Small).color(Color::Muted)))
                .child(div().flex_1().flex().flex_wrap().gap_1().children(chips))
        };
        let all = |id: &'static str, empty: bool, clear: Box<dyn Fn(&mut QuadrantRule)>| chip(id.into(), strings.filter_all.into(), empty, clear);

        let mut list_chips = vec![all("matrix-lists-all", rule.lists.is_empty(), Box::new(|r: &mut QuadrantRule| r.lists.clear()))];
        let unavailable = rule.lists.iter().flatten().copied()
            .filter(|id| !self.lists.iter().any(|list| list.id == *id))
            .map(|id| (Some(id), strings.matrix_unavailable_list.to_owned()));
        let list_choices = std::iter::once((None, strings.inbox.to_owned()))
            .chain(self.lists.iter().map(|list| (Some(list.id), list.name.clone())))
            .chain(unavailable);
        for (id, name) in list_choices {
            list_chips.push(chip(
                SharedString::from(format!("matrix-list-{}", id.map_or("inbox".to_owned(), |i| i.to_string()))),
                name.into(),
                rule.lists.contains(&id),
                Box::new(move |r: &mut QuadrantRule| toggle(&mut r.lists, &id)),
            ));
        }
        let mut tag_chips = vec![all("matrix-tags-all", rule.tags.is_empty(), Box::new(|r: &mut QuadrantRule| r.tags.clear()))];
        for tag in self.tags.iter().map(|t| &t.name) {
            let name = tag.clone();
            tag_chips.push(chip(
                SharedString::from(format!("matrix-tag-{tag}")),
                tag.clone().into(),
                rule.tags.contains(tag),
                Box::new(move |r: &mut QuadrantRule| toggle(&mut r.tags, &name)),
            ));
        }
        let mut date_chips = vec![all("matrix-dates-all", rule.dates.is_empty(), Box::new(|r: &mut QuadrantRule| r.dates.clear()))];
        for (i, date) in MatrixDate::ALL.into_iter().enumerate() {
            date_chips.push(chip(
                SharedString::from(format!("matrix-date-{i}")),
                strings.matrix_dates[i].into(),
                rule.dates.contains(&date),
                Box::new(move |r: &mut QuadrantRule| toggle(&mut r.dates, &date)),
            ));
        }
        let mut priority_chips =
            vec![all("matrix-priorities-all", rule.priorities.is_empty(), Box::new(|r: &mut QuadrantRule| r.priorities.clear()))];
        for (priority, label) in [
            (Priority::High, strings.priority_high),
            (Priority::Medium, strings.priority_medium),
            (Priority::Low, strings.priority_low),
            (Priority::None, strings.priority_none),
        ] {
            priority_chips.push(chip(
                SharedString::from(format!("matrix-priority-{}", priority.to_i64())),
                label.into(),
                rule.priorities.contains(&priority),
                Box::new(move |r: &mut QuadrantRule| toggle(&mut r.priorities, &priority)),
            ));
        }
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(28.))
                .px_4()
                .flex()
                .items_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small))
        };
        popover(cx)
            .w(px(420.))
            .p_3()
            .gap_3()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.finish_edit(None, cx)))
            .child(Label::new(strings.edit_matrix).weight(FontWeight::SEMIBOLD))
            .child(
                h_flex()
                    .h(px(30.))
                    .px_2()
                    .gap_2()
                    .rounded(px(6.))
                    .bg(theme.colors().background)
                    .child(
                        div()
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(color)
                            .text_color(theme.colors().background)
                            .text_size(px(8.))
                            .font_weight(FontWeight::BOLD)
                            .child(numeral),
                    )
                    .child(div().flex_1().child(self.name_input.clone())),
            )
            .child(row(strings.filter_lists, list_chips))
            .child(row(strings.tags, tag_chips))
            .child(row(strings.filter_date, date_chips))
            .child(row(strings.filter_priority, priority_chips))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .id("matrix-restore")
                            .cursor_pointer()
                            .child(Label::new(strings.restore).size(LabelSize::Small).color(Color::Accent))
                            .on_click(cx.listener(|this, _, _, cx| this.finish_edit(Some(false), cx))),
                    )
                    .child(div().flex_1())
                    .child(button("matrix-close", strings.close, false).on_click(cx.listener(|this, _, _, cx| this.finish_edit(None, cx))))
                    .child(button("matrix-save", strings.save, true).on_click(cx.listener(|this, _, _, cx| this.finish_edit(Some(true), cx)))),
            )
            .into_any_element()
    }

    fn render_task(&self, task: &Task, list_name: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = task.id;
        let today = Local::now().date_naive();
        let tag_color = |name: &str| {
            lists_panel::color(self.tags.iter().find(|t| t.name == name).and_then(|t| t.color.as_deref()), cx)
        };
        let due = task.due_date.map(|date| task_rows::due_label(date, task.due_time, today, strings));
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        let tags: Vec<AnyElement> =
            task.tags.iter().map(|tag| Chip::new(tag.clone(), tag_color(tag)).into_any_element()).collect();

        h_flex()
            .id(ElementId::Uuid(id))
            .h(px(40.))
            .flex_none()
            .gap(px(10.))
            .px_2()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.colors().ghost_element_hover))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenTask(id))))
            .child(
                Checkbox::new("matrix-checkbox")
                    .tint(task_rows::priority_color(task.priority, &theme))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.complete(id, cx);
                    })),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(div().flex_1().min_w_0().child(Label::new(task.title.clone()).truncate()))
                    .children(tags)
                    .child(Label::new(list_name.clone()).size(LabelSize::Small).color(Color::Muted))
                    .when(task.repeat.is_some(), |el| {
                        el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Accent))
                    })
                    .when_some(due, |el, (label, overdue)| {
                        el.child(
                            Label::new(label).size(LabelSize::Small).color(if overdue { Color::Error } else { Color::Accent }),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_quadrant(&self, quadrant: usize, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let color = Self::quadrant_color(quadrant, cx);
        let (numeral, title) = self.quadrant_title(quadrant, strings);
        let today = Local::now().date_naive();
        // Subtasks stay under their parent, as in the original; only top-level open tasks are listed.
        let open: std::collections::HashSet<Id> = self.tasks.iter().map(|t| t.id).collect();
        let tasks: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|t| {
                quadrant_of(&self.rules, t, today) == Some(quadrant) && !t.parent_id.is_some_and(|p| open.contains(&p))
            })
            .collect();

        let list_name = |list: Option<Id>| -> SharedString {
            list.and_then(|id| self.lists.iter().find(|l| l.id == id))
                .map_or(SharedString::new_static(strings.inbox), |l| l.name.clone().into())
        };
        let tasks: Vec<Task> = tasks.into_iter().cloned().collect();
        let groups = arrange(tasks, self.views[quadrant], &self.lists, today, strings);
        let content: Vec<AnyElement> = groups
            .iter()
            .flat_map(|(label, tasks)| {
                let header = (!label.is_empty()).then(|| {
                    h_flex()
                        .h(px(28.))
                        .gap_1()
                        .px_1()
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                        .child(Label::new(label.clone()).weight(FontWeight::SEMIBOLD))
                        .child(Label::new(tasks.len().to_string()).size(LabelSize::XSmall).color(Color::Muted))
                        .into_any_element()
                });
                header
                    .into_iter()
                    .chain(tasks.iter().map(|task| self.render_task(task, &list_name(task.list_id), cx)))
                    .collect::<Vec<_>>()
            })
            .collect();
        let empty = content.is_empty();

        v_flex()
            .id(SharedString::from(format!("quadrant-{numeral}")))
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(12.))
            .py_2()
            .gap_1()
            .rounded(px(8.))
            .bg(theme.colors().panel_background)
            .drag_over::<DraggedTask>(move |style, _, _, _| style.border_1().border_color(color))
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on(dragged.id, quadrant, cx)))
            .child(
                h_flex()
                    .id(SharedString::from(format!("quadrant-header-{numeral}")))
                    .group("quadrant-header")
                    .h(px(24.))
                    .gap_1p5()
                    .child(
                        div()
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(color)
                            .text_color(theme.colors().background)
                            .text_size(px(8.))
                            .font_weight(FontWeight::BOLD)
                            .child(numeral),
                    )
                    .child(div().flex_1().min_w_0().child(Label::new(title).weight(FontWeight::SEMIBOLD).color(Color::Custom(color)).truncate()))
                    // The original's "+" on a hovered header: a new task for this quadrant.
                    .child(
                        div()
                            .opacity(if crate::touch::active() { 1. } else { 0. })
                            .group_hover("quadrant-header", |s| s.opacity(1.))
                            .child(IconButton::new("quadrant-add", IconName::Plus).size(px(22.)).icon_size(IconSize::Small).on_click(
                                cx.listener(move |this, _, window, cx| this.start_add(quadrant, window, cx)),
                            )),
                    )
                    // The original's "…" on a hovered header; its Edit opens "Edit Matrix".
                    .child(
                        div()
                            .relative()
                            .opacity(if self.menu == Some(quadrant) || crate::touch::active() { 1. } else { 0. })
                            .group_hover("quadrant-header", |s| s.opacity(1.))
                            .child(IconButton::new("quadrant-more", IconName::Ellipsis).size(px(22.)).icon_size(IconSize::Small).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.menu = if this.menu == Some(quadrant) { None } else { Some(quadrant) };
                                    this.menu_row = None;
                                    window.focus(&this.focus, cx);
                                    cx.notify();
                                }),
                            ))
                            .when(self.menu == Some(quadrant), |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    popover(cx)
                                        .min_w(px(230.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.menu = None;
                                            this.menu_row = None;
                                            cx.notify();
                                        }))
                                        .child(
                                            ListItem::new("quadrant-edit")
                                                .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                                                .child(Label::new(strings.edit))
                                                .on_click(cx.listener(move |this, _, _, cx| this.start_edit(quadrant, cx))),
                                        )
                                        .children(self.render_view_rows(quadrant, cx))
                                        .into_any_element(),
                                ))
                            })
                            .when_some(self.editing.as_ref().filter(|(q, _)| *q == quadrant), |el, (_, rule)| {
                                el.child(PopoverAttachment::new(Placement::BelowRight, self.render_edit(quadrant, rule, cx)))
                            }),
                    ),
            )
            .child(
                v_flex()
                    .id(SharedString::from(format!("quadrant-tasks-{numeral}")))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(self.adding.as_ref().filter(|(q, _)| *q == quadrant).map(|(_, input)| {
                        h_flex()
                            .h(px(34.))
                            .flex_none()
                            .mx_1()
                            .mb_1()
                            .px_2()
                            .gap_2()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(color)
                            .child(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Muted))
                            .child(div().flex_1().min_w_0().debug_selector(|| format!("matrix-add-input-{quadrant}")).child(input.clone()))
                    }))
                    .when(empty && self.adding.as_ref().is_none_or(|(q, _)| *q != quadrant), |el| {
                        el.items_center()
                            .justify_center()
                            .child(Label::new(strings.no_tasks).color(Color::Muted))
                    })
                    .children(content),
            )
            .into_any_element()
    }
}

impl Render for MatrixView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let [q1, q2, q3, q4] = [0, 1, 2, 3].map(|quadrant| self.render_quadrant(quadrant, cx));
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::dismiss_transient))
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(
                h_flex()
                    .h(px(52.))
                    .flex_none()
                    .px(px(22.))
                    .child(div().flex_1().child(Label::new(strings.section_matrix).size(LabelSize::Large).weight(FontWeight::SEMIBOLD)))
                    // The original's "…": Show / Hide Completed.
                    .child(
                        div()
                            .debug_selector(|| "matrix-header-menu-trigger".to_owned())
                            .relative()
                            .child(IconButton::new("matrix-more", IconName::Ellipsis).on_click(cx.listener(|this, _, window, cx| {
                                this.header_menu = !this.header_menu;
                                window.focus(&this.focus, cx);
                                cx.notify();
                            })))
                            .when(self.header_menu, |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    popover(cx)
                                        .min_w(px(200.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.header_menu = false;
                                            cx.notify();
                                        }))
                                        .child(
                                            ListItem::new("matrix-show-completed")
                                                .height(px(34.))
                                                .start_slot(Icon::new(IconName::SquareCheck).color(Color::Muted))
                                                .child(Label::new(if self.show_completed { strings.hide_completed } else { strings.show_completed }))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.header_menu = false;
                                                    let value = if this.show_completed { "0" } else { "1" };
                                                    if let Err(err) = this.store.set_setting("matrix_show_completed", value) {
                                                        eprintln!("failed to save the setting: {err:#}");
                                                    }
                                                    this.reload(cx);
                                                })),
                                        ),
                                ))
                            }),
                    ),
            )
            .when_some(self.add_notice.as_ref(), |el, notice| {
                let (message, color) = match notice {
                    AddNotice::SavedElsewhere(destination) => (
                        strings.matrix_saved_elsewhere.replace("{destination}", destination.as_ref()),
                        Color::Default,
                    ),
                    AddNotice::Failed => (strings.task_save_failed.to_owned(), Color::Error),
                };
                el.child(
                    h_flex().flex_none().mx(px(21.)).px_2().py_1().gap_2()
                        .rounded(px(6.)).bg(theme.colors().surface_background)
                        .child(div().flex_1().min_w_0().child(Label::new(message).color(color)))
                        .child(IconButton::new("matrix-dismiss-add-notice", IconName::X)
                            .tip(strings.close, ui::TipSide::Above)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.add_notice = None;
                                cx.notify();
                            }))),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px(px(21.))
                    // As the original's, the quadrants start 8 px under the header.
                    .pt(px(8.))
                    .pb(px(14.))
                    .gap(px(12.))
                    .child(h_flex().flex_1().min_h_0().items_stretch().gap(px(12.)).child(q1).child(q2))
                    .child(h_flex().flex_1().min_h_0().items_stretch().gap(px(12.)).child(q3).child(q4)),
            )
    }
}

#[cfg(test)]
mod tests {
    use fern_core::Store;
    use gpui::EntityInputHandler;

    use super::*;

    #[gpui::test]
    fn long_quadrant_input_stays_visible_and_saves_in_full(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            crate::keymap::load_default(&assets::Assets, cx).unwrap();
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = MatrixView::new(store.clone(), cx);
            view.start_add(0, window, cx);
            view
        });
        cx.simulate_resize(gpui::size(px(700.), px(500.)));
        let empty_field = cx.debug_bounds("matrix-add-input-0").unwrap();
        let title = format!("{}конец", "ДлинноеНазвание日本語🧑‍💻".repeat(100));
        cx.simulate_input(&title);
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("matrix-add-input-0").unwrap(), empty_field);
        for width in [700., 480.] {
            cx.simulate_resize(gpui::size(px(width), px(500.)));
            let field = cx.debug_bounds("matrix-add-input-0").unwrap();
            cx.update(|window, cx| {
                let input = view.read(cx).adding.as_ref().unwrap().1.clone();
                let end = title.encode_utf16().count();
                let caret = input.update(cx, |input, cx| {
                    input.bounds_for_range(end..end, gpui::Bounds::default(), window, cx).unwrap()
                });
                assert!(caret.left() >= field.left() && caret.right() <= field.right(), "{caret:?} outside {field:?}");
            });
        }
        cx.simulate_keystrokes("enter");
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, title);
        cx.update(|_, cx| assert!(view.read(cx).adding.as_ref().unwrap().1.read(cx).text().is_empty()));
    }

    #[gpui::test]
    fn quadrant_quick_add_saves_recognized_tags(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
            let add = |cx: &App, input| {
                let id = matrix.read(cx).add_quadrant_task(0, input, today).unwrap().id;
                store.task(id).unwrap().unwrap()
            };
            let task = add(cx, "Plan the sprint #work #team #Work");
            assert_eq!(task.title, "Plan the sprint");
            assert_eq!(task.tags, ["team", "work"]);
            assert_eq!((task.priority, task.due_date), (Priority::High, Some(today)));

            let mut preferences = store.preferences().unwrap();
            preferences.recognition.remove_tag_text = false;
            store.save_preferences(&preferences).unwrap();
            let task = add(cx, "Plan the sprint #work");
            assert_eq!(task.title, "Plan the sprint #work");
            assert_eq!(task.tags, ["work"]);

            preferences.recognition.tags = false;
            store.save_preferences(&preferences).unwrap();
            let task = add(cx, "Plan the sprint #work");
            assert_eq!(task.title, "Plan the sprint #work");
            assert!(task.tags.is_empty());
        });
    }

    #[gpui::test]
    fn quadrant_quick_add_uses_an_available_list_after_deletion(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let removed = store.create_list("Remove me").unwrap();
        let available = store.create_list("Still available").unwrap();
        let mut preferences = store.preferences().unwrap();
        preferences.default_list = Some(removed.id);
        store.save_preferences(&preferences).unwrap();
        let mut rule = QuadrantRule::defaults()[0].clone();
        rule.lists = vec![Some(removed.id), Some(available.id)];
        store.save_matrix_rule(0, Some(&rule)).unwrap();
        let today = Local::now().date_naive();

        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
            store.delete_list(removed.id).unwrap();
            let id = matrix.read(cx).add_quadrant_task(0, "First #work", today).unwrap().id;
            let tasks = store.tasks().unwrap();
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].id, id);
            assert_eq!(tasks[0].list_id, Some(available.id));
            assert_eq!(tasks[0].tags, ["work"]);

            rule.lists = vec![Some(removed.id)];
            store.save_matrix_rule(0, Some(&rule)).unwrap();
            matrix.update(cx, |matrix, cx| matrix.reload(cx));
            let view = matrix.read(cx);
            assert_eq!(view.rules[0].lists, rule.lists);
            let unrelated = &tasks[0];
            assert!(!view.rules[0].matches(unrelated, today));

            // Editing another part of the rule must not silently clear the unavailable list.
            matrix.update(cx, |matrix, cx| {
                matrix.start_edit(0, cx);
                matrix.name_input.update(cx, |input, cx| input.set_text("Scoped quadrant", cx));
                matrix.finish_edit(Some(true), cx);
            });
            assert_eq!(store.matrix_rules().unwrap()[0].lists, rule.lists);

            let id = matrix.read(cx).add_quadrant_task(0, "Second", today).unwrap().id;
            assert_eq!(store.task(id).unwrap().unwrap().list_id, None);
            assert_eq!(store.tasks().unwrap().len(), 2);
        });
    }

    #[gpui::test]
    fn archiving_the_last_list_keeps_the_quadrant_restricted_until_restore(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = Local::now().date_naive();
        let list = store.create_list("Restricted").unwrap();
        let fallback = store.create_list("Available").unwrap();
        let mut preferences = store.preferences().unwrap();
        preferences.default_list = Some(fallback.id);
        store.save_preferences(&preferences).unwrap();

        let task = |title: &str, list_id| {
            store.create_task(TaskDraft {
                title: title.into(),
                list_id: Some(list_id),
                priority: Priority::High,
                due_date: Some(today),
                ..TaskDraft::default()
            }, true).unwrap()
        };
        let restricted = task("In the restricted list", list.id);
        let unrelated = task("In another list", fallback.id);
        let mut rule = QuadrantRule::defaults()[0].clone();
        rule.lists = vec![Some(list.id)];
        store.save_matrix_rule(0, Some(&rule)).unwrap();

        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store.clone(), cx));
            store.set_list_archived(list.id, true).unwrap();
            matrix.update(cx, |matrix, cx| matrix.reload(cx));
            let view = matrix.read(cx);
            assert_eq!(view.rules[0].lists, rule.lists);
            assert!(!view.tasks.iter().any(|task| quadrant_of(&view.rules, task, today) == Some(0)));
            let added = view.add_quadrant_task(0, "Saved in available list", today).unwrap();
            assert_eq!(added.list_id, Some(fallback.id));

            store.set_list_archived(list.id, false).unwrap();
            matrix.update(cx, |matrix, cx| matrix.reload(cx));
            let view = matrix.read(cx);
            assert!(view.tasks.iter().any(|task| task.id == restricted.id));
            assert_eq!(quadrant_of(&view.rules, &restricted, today), Some(0));
            assert_ne!(quadrant_of(&view.rules, &unrelated, today), Some(0));
            assert_ne!(quadrant_of(&view.rules, &added, today), Some(0));
        });
    }

    #[gpui::test]
    fn matrix_uses_rule_date_without_an_explicit_date_and_explains_other_destinations(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let mut rule = QuadrantRule::defaults()[0].clone();
        rule.dates = vec![MatrixDate::Tomorrow];
        store.save_matrix_rule(0, Some(&rule)).unwrap();

        cx.update(|cx| {
            let matrix = cx.new(|cx| MatrixView::new(store, cx));
            let task = matrix.read(cx).add_quadrant_task(0, "Plan 9am", today).unwrap();
            assert_eq!(task.due_date, today.succ_opt());
            assert_eq!(task.due_time, chrono::NaiveTime::from_hms_opt(9, 0, 0));
            let other = matrix.read(cx).add_quadrant_task(0, "Plan today", today).unwrap();
            assert_eq!(other.due_date, Some(today));
            matrix.update(cx, |matrix, cx| {
                matrix.reload(cx);
                let strings = i18n::t(cx);
                assert!(matrix.destination_notice(0, &task, today, strings).is_none());
                assert!(matches!(
                    matrix.destination_notice(0, &other, today, strings),
                    Some(AddNotice::SavedElsewhere(ref list)) if list.as_ref() == strings.inbox,
                ));
            });
        });
    }

    #[gpui::test]
    fn escape_dismisses_the_matrix_header_menu(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let view = MatrixView::new(Rc::new(Store::open_in_memory().unwrap()), cx);
            window.focus(&view.focus_handle(cx), cx);
            view
        });
        let trigger = cx.debug_bounds("matrix-header-menu-trigger").unwrap().center();
        cx.simulate_click(trigger, gpui::Modifiers::default());
        cx.update(|_, cx| assert!(view.read(cx).header_menu));
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            assert!(!view.read(cx).header_menu);
            assert!(view.read(cx).focus_handle(cx).is_focused(window));
        });
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| assert!(view.read(cx).focus_handle(cx).is_focused(window)));
    }

    #[test]
    fn quadrant_views() {
        let store = Store::open_in_memory().unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        for (title, days) in [("later", 5), ("soon", 1), ("now", 0)] {
            let task = store.add_task(title).unwrap();
            store.set_due(task.id, Some(today + chrono::Duration::days(days)), None).unwrap();
        }
        let tasks = store.tasks().unwrap();
        let strings = &crate::i18n::EN;
        let titles = |groups: &[(String, Vec<Task>)]| -> Vec<String> {
            groups.iter().flat_map(|(_, tasks)| tasks.iter().map(|t| t.title.clone())).collect()
        };
        let by_list = arrange(tasks.clone(), QuadrantView::default(), &[], today, strings);
        assert_eq!(by_list.len(), 1);
        assert_eq!(by_list[0].0, "Inbox");
        assert_eq!(titles(&by_list), ["now", "soon", "later"], "by time, oldest first");
        let newest = arrange(tasks.clone(), QuadrantView { newest_first: true, ..QuadrantView::default() }, &[], today, strings);
        assert_eq!(titles(&newest), ["later", "soon", "now"]);
        let by_date = arrange(tasks.clone(), QuadrantView { group: Some(GroupBy::Date), ..QuadrantView::default() }, &[], today, strings);
        assert_eq!(by_date.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(), ["Today", "Tomorrow", "Next 7 Days"]);
        let view = QuadrantView { group: Some(GroupBy::Tag), sort: SortBy::Title, newest_first: true };
        assert_eq!(QuadrantView::from_storage(&view.to_storage()), view);
        assert_eq!(QuadrantView::from_storage(""), QuadrantView::default());
    }
}
