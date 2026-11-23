//! The search popover, matched to TickTick's: a field with filter chips, results grouped into
//! tasks, tags and lists, matches highlighted, and "View more" for the full result list.

use std::collections::HashSet;
use std::rc::Rc;

use chrono::Local;
use desktop_common::plugin_bus::{PluginBus, PluginCommandSpec, PluginSearchHit};
use fern_core::search::{DateRange, TagRule, TaskFilter};
use fern_core::{Habit, Id, Kind, List, Priority, Store, Tag, Task, View};
use gpui::{Entity, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle, ScrollHandle, StyledText};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::i18n;
use crate::lists_panel;
use crate::task_rows;
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};

/// How many tasks the popover lists before "View more".
const TASK_LIMIT: usize = 5;
/// Delay before filtering cached results and querying plugins while the user is actively typing.
const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(40);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    All,
    Tasks,
    Habits,
    Tags,
    Lists,
}

/// The Task filter's dropdowns, as the original's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Lists,
    Tags,
    Date,
    Priority,
}

#[derive(Clone)]
pub enum SearchEvent {
    OpenTask(Id),
    OpenHabit(Id),
    OpenView(View),
    CreateTask(String),
    RunPluginCommand {
        plugin_id: String,
        command_id: String,
        arg: Option<String>,
    },
    Dismissed,
}

pub struct SearchModal {
    store: Rc<Store>,
    input: Entity<TextInput>,
    filter: Filter,
    tasks: Vec<Task>,
    habits: Vec<Habit>,
    tags: Vec<Tag>,
    lists: Vec<List>,
    plugin_commands: Vec<PluginCommandSpec>,
    plugin_hits: Vec<PluginSearchHit>,
    last_plugin_query: String,
    pending_complete: HashSet<Id>,
    all_tasks: Vec<Task>,
    all_habits: Vec<Habit>,
    all_lists: Vec<List>,
    all_tags: Vec<Tag>,
    results_scroll: ScrollHandle,
    search_task: Option<gpui::Task<()>>,
    pending_search: bool,
    /// The Task filter's choices, the open dropdown, and the choices being made in it (applied
    /// with OK, as the original's), with the Custom dates' fields.
    task_filter: TaskFilter,
    menu: Option<Menu>,
    draft: TaskFilter,
    custom_from: Entity<TextInput>,
    custom_until: Entity<TextInput>,
    custom_open: bool,
    /// Index of the keyboard-highlighted row across visible results.
    selected: usize,
}

impl EventEmitter<SearchEvent> for SearchModal {}

impl Focusable for SearchModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl SearchModal {
    pub fn clear_plugin_search_hits(cx: &mut App) {
        if cx
            .try_global::<PluginBus>()
            .is_some_and(|bus| !bus.search_hits.is_empty())
        {
            PluginBus::update(cx, |bus| bus.search_hits.clear());
        }
    }

    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        Self::clear_plugin_search_hits(cx);
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.search_placeholder, cx));
        cx.subscribe(&input, |this, _, _: &Changed, cx| this.schedule_refresh(cx)).detach();
        cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
            this.emit_close(SearchEvent::Dismissed, cx);
        })
        .detach();
        cx.subscribe(&input, |this, _, _: &Submitted, cx| this.open_selected(cx)).detach();
        let all_tasks = store.tasks().unwrap_or_default();
        let all_habits = store.habits().unwrap_or_default();
        let all_lists = store.lists().unwrap_or_default();
        let all_tags = store.tags().unwrap_or_default();
        cx.observe_global::<desktop_common::data_version::DataVersion>(|this, cx| {
            if desktop_common::data_version::changed_elsewhere(cx.entity_id(), cx) {
                this.reload_cache();
                if !this.query(cx).is_empty() {
                    this.refresh(cx);
                } else {
                    cx.notify();
                }
            }
        })
        .detach();
        cx.observe_global::<PluginBus>(|this, cx| {
            this.sync_plugin_results(cx);
        })
        .detach();
        let custom_from = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.range_from, cx));
        let custom_until = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.range_until, cx));
        Self {
            store,
            input,
            filter: Filter::All,
            tasks: Vec::new(),
            habits: Vec::new(),
            tags: Vec::new(),
            lists: Vec::new(),
            plugin_commands: Vec::new(),
            plugin_hits: Vec::new(),
            last_plugin_query: String::new(),
            pending_complete: HashSet::new(),
            all_tasks,
            all_habits,
            all_lists,
            all_tags,
            results_scroll: ScrollHandle::new(),
            search_task: None,
            pending_search: false,
            task_filter: TaskFilter::default(),
            menu: None,
            draft: TaskFilter::default(),
            custom_from,
            custom_until,
            custom_open: false,
            selected: 0,
        }
    }

    fn emit_close(&mut self, event: SearchEvent, cx: &mut Context<Self>) {
        self.search_task = None;
        self.last_plugin_query.clear();
        self.plugin_hits.clear();
        Self::clear_plugin_search_hits(cx);
        cx.emit(event);
    }

    #[cfg(debug_assertions)]
    pub fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        let query = query.to_owned();
        self.input.update(cx, |input, cx| input.set_text(query, cx));
        self.refresh(cx);
    }

    fn query(&self, cx: &App) -> String {
        self.input.read(cx).text().trim().to_owned()
    }

    fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        if self.query(cx).is_empty() {
            self.pending_search = false;
            self.refresh(cx);
            return;
        }
        self.pending_search = true;
        self.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            this.update(cx, |this, cx| this.refresh(cx)).ok();
        }));
    }

    fn reload_cache(&mut self) {
        self.all_tasks = self.store.tasks().unwrap_or_default();
        self.all_habits = self.store.habits().unwrap_or_default();
        self.all_lists = self.store.lists().unwrap_or_default();
        self.all_tags = self.store.tags().unwrap_or_default();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.search_task = None;
        self.pending_search = false;
        let query = self.query(cx);
        if query.is_empty() {
            self.tasks.clear();
            self.habits.clear();
            self.tags.clear();
            self.lists.clear();
        } else {
            let today = Local::now().date_naive();
            self.tasks = self
                .store
                .tasks_in_from(self.all_tasks.clone(), &View::Search(query.clone()), today)
                .unwrap_or_default();
            if self.filter == Filter::Tasks {
                let week_start = crate::calendar::week_start();
                self.tasks
                    .retain(|task| self.task_filter.matches(task, today, week_start));
            }
            let matches = |name: &str| !task_rows::match_ranges(name, &query).is_empty();
            self.habits = self
                .all_habits
                .iter()
                .filter(|h| matches(&h.name))
                .cloned()
                .collect();
            self.tags = self
                .all_tags
                .iter()
                .filter(|t| matches(&t.name))
                .cloned()
                .collect();
            self.lists = self
                .all_lists
                .iter()
                .filter(|l| matches(&l.name))
                .cloned()
                .collect();
        }
        self.selected = 0;
        self.results_scroll.set_offset(gpui::point(px(0.), px(0.)));
        if self.filter != Filter::All || query.is_empty() {
            self.last_plugin_query.clear();
            self.plugin_hits.clear();
            Self::clear_plugin_search_hits(cx);
            self.sync_plugin_results(cx);
        } else if query != self.last_plugin_query {
            self.last_plugin_query = query.clone();
            self.plugin_hits.clear();
            Self::clear_plugin_search_hits(cx);
            self.sync_plugin_results(cx);
            plugin_ui::PluginHost::search_async(query, cx);
        } else {
            self.sync_plugin_results(cx);
        }
    }

    fn sync_plugin_results(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        if self.filter != Filter::All || query.is_empty() {
            self.plugin_commands.clear();
            self.plugin_hits.clear();
        } else if let Some(bus) = cx.try_global::<PluginBus>() {
            self.plugin_commands = bus
                .commands
                .iter()
                .filter(|cmd| {
                    !task_rows::match_ranges(&cmd.title, &query).is_empty()
                        || !task_rows::match_ranges(&cmd.plugin_name, &query).is_empty()
                })
                .cloned()
                .collect();
            self.plugin_hits = bus.search_hits.clone();
        } else {
            self.plugin_commands.clear();
            self.plugin_hits.clear();
        }
        let count = self.visible_events().len();
        if count == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(count - 1);
        }
        cx.notify();
    }

    fn plugin_hit_event(hit: &PluginSearchHit) -> Option<SearchEvent> {
        if let Some(task_id) = hit.task_id {
            Some(SearchEvent::OpenTask(task_id))
        } else {
            hit.command_id.as_ref().map(|command_id| SearchEvent::RunPluginCommand {
                plugin_id: hit.plugin_id.clone(),
                command_id: command_id.clone(),
                arg: hit.command_arg.clone(),
            })
        }
    }

    fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        self.menu = None;
        self.task_filter = TaskFilter::default();
        self.refresh(cx);
    }

    fn open_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(menu) { None } else { Some(menu) };
        self.draft = self.task_filter.clone();
        self.custom_open = false;
        cx.notify();
    }

    /// OK in a dropdown, or a Date choice: the choices apply.
    fn apply(&mut self, cx: &mut Context<Self>) {
        self.task_filter = self.draft.clone();
        self.menu = None;
        self.custom_open = false;
        self.refresh(cx);
    }

    /// For the dev state: `search-filter=task[>lists|tags|date|priority]`.
    #[cfg(debug_assertions)]
    pub fn dev_filter(&mut self, spec: &str, cx: &mut Context<Self>) {
        let (filter, menu) = spec.split_once('>').unwrap_or((spec, ""));
        if filter == "task" {
            self.set_filter(Filter::Tasks, cx);
        }
        match menu {
            "lists" => self.open_menu(Menu::Lists, cx),
            "tags" => self.open_menu(Menu::Tags, cx),
            "date" => self.open_menu(Menu::Date, cx),
            "priority" => self.open_menu(Menu::Priority, cx),
            _ => {}
        }
    }

    /// A Date choice's words: "This Week (Oct 12 - Oct 18)", as the original's.
    fn range_label(range: DateRange, strings: &'static crate::i18n::Strings) -> String {
        let today = Local::now().date_naive();
        let short = |date| task_rows::short_date(date, strings);
        let name = match range {
            DateRange::All => strings.all_option,
            DateRange::ThisWeek => strings.this_week,
            DateRange::NextWeek => strings.next_week_range,
            DateRange::LastWeek => strings.last_week,
            DateRange::ThisMonth => strings.this_month,
            DateRange::LastMonth => strings.last_month,
            DateRange::Custom(..) => strings.custom_range,
        };
        match (range, range.days(today, crate::calendar::week_start())) {
            (DateRange::ThisWeek | DateRange::NextWeek | DateRange::LastWeek | DateRange::Custom(..), Some((from, until))) => {
                format!("{name} ({} - {})", short(from), short(until))
            }
            _ => name.to_owned(),
        }
    }

    /// The Task filter's dropdown under its chip: choices with ticks, and Cancel and OK for the
    /// ones that take several (the original's Lists, Tag and Priority); Date applies on a click.
    fn render_menu(&self, menu: Menu, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let row = |id: SharedString, icon: Option<Icon>, label: SharedString, on: bool| {
            ListItem::new(id)
                .height(px(30.))
                .when_some(icon, |item, icon| item.start_slot(icon.size(IconSize::Small).color(if on { Color::Accent } else { Color::Muted })))
                .child(Label::new(label).size(LabelSize::Small).color(if on { Color::Accent } else { Color::Default }))
                .when(on, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
        };
        // A multi-choice: "All" alone, or any of the others.
        fn toggle<T: PartialEq + Clone>(choices: &mut Vec<T>, value: T) {
            match choices.iter().position(|c| *c == value) {
                Some(i) => {
                    choices.remove(i);
                }
                None => choices.push(value),
            }
        }
        let draft = &self.draft;
        let mut items: Vec<AnyElement> = Vec::new();
        let mut buttons = true;
        match menu {
            Menu::Lists => {
                items.push(
                    row("search-lists-all".into(), Some(Icon::new(IconName::Layers)), strings.all_lists.into(), draft.lists.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.draft.lists.clear();
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                let lists = std::iter::once((None, SharedString::new_static(strings.inbox), IconName::Inbox))
                    .chain(self.all_lists.iter().filter(|l| !l.archived).map(|l| (Some(l.id), l.name.clone().into(), IconName::List)));
                for (list, name, icon) in lists {
                    items.push(
                        row(SharedString::from(format!("search-list-{list:?}")), Some(Icon::new(icon)), name, draft.lists.contains(&list))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                toggle(&mut this.draft.lists, list);
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Tags => {
                for (rule, label) in [(TagRule::All, strings.all_option), (TagRule::WithTags, strings.with_tags), (TagRule::NoTags, strings.no_tags)] {
                    let on = draft.tags == rule;
                    items.push(
                        row(SharedString::from(format!("search-tags-{label}")), Some(Icon::new(IconName::Tag)), label.into(), on)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.draft.tags = rule.clone();
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
                for tag in &self.all_tags {
                    let name = tag.name.clone();
                    let on = matches!(&draft.tags, TagRule::Any(tags) if tags.contains(&name));
                    items.push(
                        row(SharedString::from(format!("search-tag-{name}")), Some(Icon::new(IconName::Tag)), name.clone().into(), on)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let mut tags = match &this.draft.tags {
                                    TagRule::Any(tags) => tags.clone(),
                                    _ => Vec::new(),
                                };
                                toggle(&mut tags, name.clone());
                                this.draft.tags = if tags.is_empty() { TagRule::All } else { TagRule::Any(tags) };
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Priority => {
                items.push(
                    row("search-priority-all".into(), None, strings.all_option.into(), draft.priorities.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.draft.priorities.clear();
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                for (priority, label) in [
                    (Priority::High, strings.priority_high),
                    (Priority::Medium, strings.priority_medium),
                    (Priority::Low, strings.priority_low),
                    (Priority::None, strings.none_option),
                ] {
                    items.push(
                        row(SharedString::from(format!("search-priority-{label}")), None, label.into(), draft.priorities.contains(&priority))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                toggle(&mut this.draft.priorities, priority);
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            Menu::Date => {
                buttons = self.custom_open;
                let current = self.task_filter.date;
                for range in [DateRange::All, DateRange::ThisWeek, DateRange::NextWeek, DateRange::LastWeek, DateRange::ThisMonth, DateRange::LastMonth] {
                    items.push(
                        row(SharedString::from(format!("search-date-{range:?}")), None, Self::range_label(range, strings).into(), current == range)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.draft.date = range;
                                this.apply(cx);
                            }))
                            .into_any_element(),
                    );
                }
                let custom = matches!(current, DateRange::Custom(..));
                let custom_label = if custom {
                    Self::range_label(current, strings)
                } else {
                    strings.custom_range.to_owned()
                };
                items.push(
                    row("search-date-custom".into(), None, custom_label.into(), custom)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.custom_open = true;
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                if self.custom_open {
                    let field = |input: Entity<TextInput>| h_flex().h(px(28.)).px_2().rounded(px(6.)).bg(theme.colors().element_hover).child(input);
                    items.push(
                        v_flex()
                            .px_2()
                            .gap_1()
                            .child(field(self.custom_from.clone()))
                            .child(field(self.custom_until.clone()))
                            .into_any_element(),
                    );
                }
            }
        }
        popover(cx)
            .w(px(if menu == Menu::Date { 250. } else { 210. }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .child(
                v_scroll(
                    ("search-menu-items-scroll", menu as usize),
                    v_flex().id(("search-menu-items", menu as usize)).children(items),
                )
                .max_h(px(260.))
                .compact(),
            )
            .when(buttons, |el| {
                el.child(
                    h_flex()
                        .pt_2()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .when(cfg!(test), |el| {
                                    el.debug_selector(|| "search-menu-cancel".to_owned())
                                })
                                .child(
                                    Button::new("search-menu-cancel", strings.cancel)
                                        .variant(ButtonVariant::Secondary)
                                        .size(ButtonSize::Sm)
                                        .full_width()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.menu = None;
                                            this.custom_open = false;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .when(cfg!(test), |el| {
                                    el.debug_selector(|| "search-menu-ok".to_owned())
                                })
                                .child(
                                    Button::new("search-menu-ok", strings.ok)
                                        .variant(ButtonVariant::Primary)
                                        .size(ButtonSize::Sm)
                                        .full_width()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if this.menu == Some(Menu::Date) {
                                                let strings = i18n::t(cx);
                                                let today = Local::now().date_naive();
                                                let from = crate::calendar::parse_date(
                                                    this.custom_from.read(cx).text(),
                                                    today,
                                                    strings,
                                                );
                                                let until = crate::calendar::parse_date(
                                                    this.custom_until.read(cx).text(),
                                                    today,
                                                    strings,
                                                );
                                                if let (Some(from), Some(until)) = (from, until) {
                                                    this.draft.date = DateRange::Custom(from, until);
                                                }
                                            }
                                            this.apply(cx);
                                        })),
                                ),
                        ),
                )
            })
            .into_any_element()
    }

    fn shows(&self, filter: Filter) -> bool {
        self.filter == Filter::All || self.filter == filter
    }

    fn visible_events(&self) -> Vec<SearchEvent> {
        let mut events = Vec::new();
        if self.shows(Filter::Tasks) {
            events.extend(self.tasks.iter().take(TASK_LIMIT).map(|t| SearchEvent::OpenTask(t.id)));
        }
        if self.shows(Filter::Habits) {
            events.extend(self.habits.iter().map(|h| SearchEvent::OpenHabit(h.id)));
        }
        if self.shows(Filter::Tags) {
            events.extend(self.tags.iter().map(|t| SearchEvent::OpenView(View::Tag(t.name.clone()))));
        }
        if self.shows(Filter::Lists) {
            events.extend(self.lists.iter().map(|l| SearchEvent::OpenView(View::List(l.id))));
        }
        if self.filter == Filter::All {
            events.extend(self.plugin_commands.iter().map(|cmd| SearchEvent::RunPluginCommand {
                plugin_id: cmd.plugin_id.clone(),
                command_id: cmd.command_id.clone(),
                arg: None,
            }));
            events.extend(self.plugin_hits.iter().filter_map(Self::plugin_hit_event));
        }
        events
    }

    fn selected_child_index(&self) -> Option<usize> {
        let mut child_idx = 0usize;
        let mut sel_idx = 0usize;

        let task_count = if self.shows(Filter::Tasks) {
            self.tasks.len().min(TASK_LIMIT)
        } else {
            0
        };
        if task_count > 0 {
            child_idx += 1;
            if self.selected < sel_idx + task_count {
                return Some(child_idx + (self.selected - sel_idx));
            }
            child_idx += task_count;
            sel_idx += task_count;
            if self.tasks.len() > TASK_LIMIT {
                child_idx += 1;
            }
        }

        let groups = [
            (self.shows(Filter::Habits), self.habits.len()),
            (self.shows(Filter::Tags), self.tags.len()),
            (self.shows(Filter::Lists), self.lists.len()),
            (self.filter == Filter::All, self.plugin_commands.len()),
        ];
        for (shown, len) in groups {
            if shown && len > 0 {
                child_idx += 1;
                if self.selected < sel_idx + len {
                    return Some(child_idx + (self.selected - sel_idx));
                }
                child_idx += len;
                sel_idx += len;
            }
        }

        if self.filter == Filter::All && !self.plugin_hits.is_empty() {
            child_idx += 1;
            for hit in &self.plugin_hits {
                if Self::plugin_hit_event(hit).is_some() {
                    if sel_idx == self.selected {
                        return Some(child_idx);
                    }
                    sel_idx += 1;
                }
                child_idx += 1;
            }
        }

        None
    }

    fn scroll_selected_into_view(&self) {
        let Some(child_ix) = self.selected_child_index() else {
            return;
        };
        let viewport = self.results_scroll.bounds();
        if viewport.size.height <= px(0.) {
            self.results_scroll
                .scroll_to_item(if self.selected == 0 { 0 } else { child_ix });
            return;
        }
        if self.selected == 0 {
            let cur = self.results_scroll.offset();
            if cur.y < px(0.) {
                self.results_scroll.set_offset(gpui::point(cur.x, px(0.)));
            }
            return;
        }
        let Some(unscrolled) = self.results_scroll.bounds_for_item(child_ix) else {
            self.results_scroll.scroll_to_item(child_ix);
            return;
        };
        let cur = self.results_scroll.offset();
        let top = unscrolled.top() + cur.y;
        let bottom = unscrolled.bottom() + cur.y;
        let mut next_y = cur.y;
        if top < viewport.top() {
            next_y += viewport.top() - top;
        } else if bottom > viewport.bottom() {
            next_y += viewport.bottom() - bottom;
        } else {
            return;
        }
        let max_y = self.results_scroll.max_offset().y.max(px(0.));
        self.results_scroll
            .set_offset(gpui::point(cur.x, next_y.clamp(-max_y, px(0.))));
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.visible_events().len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let next = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
        if self.selected != next {
            self.selected = next;
            self.scroll_selected_into_view();
            cx.notify();
        }
    }

    fn is_task_read_only(&self, task: &Task) -> bool {
        task.is_deleted()
            || task.list_id.is_some_and(|id| self.all_lists.iter().any(|l| l.id == id && l.archived))
    }

    fn finish_complete_task(
        store: &Store,
        task: &Task,
        entity_id: gpui::EntityId,
        cx: &mut App,
    ) -> bool {
        if let Err(err) = desktop_common::undo::complete(store, task) {
            eprintln!("failed to toggle task from search: {err:#}");
            return false;
        }
        desktop_common::completion_mark::record(task.id, cx);
        desktop_common::sound::play_complete(store);
        desktop_common::data_version::data_changed(entity_id, cx);
        true
    }

    fn toggle_task_completion(&mut self, task: &Task, cx: &mut Context<Self>) {
        if self.is_task_read_only(task) {
            return;
        }
        let done = task.is_completed();
        let wont_do = task.is_wont_do();
        let completing = !done && !wont_do;
        if completing {
            if !self.pending_complete.insert(task.id) {
                return;
            }
            if let Some(mgr) = crate::quick_add_window::active_hook_manager(cx) {
                let store = self.store.clone();
                let task_snapshot = task.clone();
                let task_id = task.id;
                let entity_id = cx.entity_id();
                let weak = cx.weak_entity();
                crate::quick_add_window::run_before_task_complete(mgr, task, cx, move |allowed, cx| {
                    let _ = weak.update(cx, |this, _| {
                        this.pending_complete.remove(&task_id);
                    });
                    if allowed && Self::finish_complete_task(&store, &task_snapshot, entity_id, cx) {
                        let _ = weak.update(cx, |this, cx| {
                            this.reload_cache();
                            this.refresh(cx);
                        });
                    }
                });
                return;
            }
            self.pending_complete.remove(&task.id);
            if Self::finish_complete_task(&self.store, task, cx.entity_id(), cx) {
                self.reload_cache();
                self.refresh(cx);
            }
            return;
        }
        let result = if wont_do {
            self.store.set_wont_do(task.id, false)
        } else {
            self.store.set_completed(task.id, false)
        };
        if let Err(err) = result {
            eprintln!("failed to toggle task from search: {err:#}");
            return;
        }
        desktop_common::data_version::data_changed(cx.entity_id(), cx);
        self.reload_cache();
        self.refresh(cx);
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let mut events = self.visible_events();
        if events.is_empty() {
            let query = self.query(cx);
            if !query.is_empty() {
                self.emit_close(SearchEvent::CreateTask(query), cx);
            }
            return;
        }
        let index = self.selected.min(events.len() - 1);
        self.emit_close(events.swap_remove(index), cx);
    }

    fn highlighted(&self, text: &str, query: &str, cx: &App) -> StyledText {
        let highlight = HighlightStyle {
            color: Some(cx.theme().colors().text_accent),
            font_weight: Some(FontWeight::SEMIBOLD),
            background_color: Some(cx.theme().colors().accent.opacity(0.18)),
            ..Default::default()
        };
        StyledText::new(text.to_owned())
            .with_highlights(task_rows::match_ranges(text, query).into_iter().map(|range| (range, highlight)))
    }

    fn strip_markdown_inline(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while !rest.is_empty() {
            if let Some(after) = rest
                .strip_prefix("**")
                .or_else(|| rest.strip_prefix("__"))
                .or_else(|| rest.strip_prefix("~~"))
                .or_else(|| rest.strip_prefix('`'))
            {
                rest = after;
            } else if let Some(ch) = rest.chars().next() {
                out.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
        out
    }

    fn group_heading(label: &'static str) -> impl IntoElement {
        div().px_2().pt_2().pb_1().child(Label::new(label).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
    }

    fn create_task_button_label(add_task: &str, query: &str) -> (String, bool) {
        // `ui::Button` renders a non-truncating `Label` with `whitespace_nowrap`, so keep the
        // displayed query bounded inside the 560px modal while emitting the full query on click.
        const MAX_QUERY_CHARS: usize = 28;
        let mut chars = query.chars();
        let prefix: String = chars.by_ref().take(MAX_QUERY_CHARS).collect();
        if chars.next().is_some() {
            (format!("{add_task}: \"{prefix}…\""), true)
        } else {
            (format!("{add_task}: \"{query}\""), false)
        }
    }
}

impl Render for SearchModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let query = self.query(cx);
        let has_query = !query.is_empty();

        // The original's chips: bordered pills; the chosen one tinted with an × that clears it.
        let chip = |filter: Filter, label: &'static str, cx: &mut Context<Self>| {
            let selected = self.filter == filter;
            h_flex()
                .id(label)
                .h(px(24.))
                .px_2()
                .gap_1()
                .rounded(px(6.))
                .cursor_pointer()
                .border_1()
                .when(selected, |el| el.bg(theme.colors().accent.opacity(0.15)).border_color(theme.colors().accent.opacity(0.3)))
                .when(!selected, |el| el.border_color(theme.colors().border))
                .when(selected && filter == Filter::Tasks, |el| el.child(Icon::new(IconName::SquareCheck).size(IconSize::XSmall).color(Color::Accent)))
                .child(Label::new(label).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default }))
                .when(selected, |el| el.child(Icon::new(IconName::X).size(IconSize::XSmall).color(Color::Accent)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let next = if this.filter == filter { Filter::All } else { filter };
                    this.set_filter(next, cx)
                }))
        };
        // With Task chosen, the original's Lists, Tag, Date and Priority dropdowns follow it.
        let dropdown = |menu: Menu, label: SharedString, active: bool, cx: &mut Context<Self>| {
            let open = self.menu == Some(menu);
            div()
                .relative()
                .child(
                    h_flex()
                        .id(SharedString::from(format!("search-dropdown-{menu:?}")))
                        .h(px(24.))
                        .px_2()
                        .gap_1()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .border_1()
                        .border_color(if open || active { theme.colors().accent.opacity(0.5) } else { theme.colors().border })
                        .child(Label::new(label).size(LabelSize::Small).color(if active { Color::Accent } else { Color::Default }))
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(move |this, _, _, cx| this.open_menu(menu, cx))),
                )
                .when(open, |el| el.child(PopoverAttachment::new(Placement::BelowLeft, self.render_menu(menu, cx))))
        };
        let task_dropdowns: Vec<AnyElement> = if self.filter == Filter::Tasks {
            let f = &self.task_filter;
            let date_label: SharedString = if f.date == DateRange::All { strings.filter_date.into() } else { Self::range_label(f.date, strings).into() };
            vec![
                dropdown(Menu::Lists, strings.lists.into(), !f.lists.is_empty(), cx).into_any_element(),
                dropdown(Menu::Tags, strings.search_filter_tag.into(), f.tags != TagRule::All, cx).into_any_element(),
                dropdown(Menu::Date, date_label, f.date != DateRange::All, cx).into_any_element(),
                dropdown(Menu::Priority, strings.filter_priority.into(), !f.priorities.is_empty(), cx).into_any_element(),
            ]
        } else {
            Vec::new()
        };

        let today = Local::now().date_naive();
        let mut row_index = 0usize;
        let selected_index = self.selected;

        let task_rows: Vec<AnyElement> = if self.shows(Filter::Tasks) {
            let list_names: std::collections::HashMap<_, _> = self
                .all_lists
                .iter()
                .map(|l| (l.id, l.name.as_str()))
                .collect();
            self.tasks
                .iter()
                .take(TASK_LIMIT)
                .map(|task| {
                    let is_selected = has_query && row_index == selected_index;
                    row_index += 1;
                    let id = task.id;
                    let list = task
                        .list_id
                        .and_then(|id| list_names.get(&id).copied())
                        .map_or(SharedString::new_static(strings.inbox), |name| SharedString::from(name.to_owned()));
                    let due = task.due_date.map(|date| task_rows::due_label(date, task.due_time, today, strings));
                    let horizon = crate::calendar::horizons_enabled()
                        .then(|| task_rows::horizon_badge(task, today, strings))
                        .flatten();
                    let title_matches = !task_rows::match_ranges(&task.title, &query).is_empty();
                    let note = (!title_matches)
                        .then(|| task_rows::snippet(&task.notes, &query, 30))
                        .flatten()
                        .map(|snippet| Self::strip_markdown_inline(&snippet));
                    let location_match = task
                        .location
                        .as_deref()
                        .map(str::trim)
                        .filter(|loc| !loc.is_empty() && !task_rows::match_ranges(loc, &query).is_empty())
                        .map(str::to_owned);
                    v_flex()
                        .id(SharedString::from(format!("search-task-{id}")))
                        .px_2()
                        .py_1()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(is_selected, |el| el.bg(theme.colors().ghost_element_selected))
                        .hover(|s| s.bg(theme.colors().ghost_element_hover))
                        .child(
                            h_flex()
                                .h(px(24.))
                                .gap_2()
                                .child(if task.kind == Kind::Note {
                                    Icon::new(IconName::FileText).size(IconSize::Small).color(Color::Muted).into_any_element()
                                } else {
                                    let done = task.is_completed();
                                    let wont_do = task.is_wont_do();
                                    let read_only = self.is_task_read_only(task);
                                    let task_snapshot = task.clone();
                                    Checkbox::new(SharedString::from(format!("search-checkbox-{id}")))
                                        .size(px(14.))
                                        .toggle_state(done)
                                        .in_progress(task.is_in_progress())
                                        .dismissed(wont_do)
                                        .tint(task_rows::priority_color(task.priority, &theme))
                                        .when(!read_only, |cb| {
                                            cb.on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.toggle_task_completion(&task_snapshot, cx);
                                            }))
                                        })
                                        .into_any_element()
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(13.))
                                        .line_height(px(17.))
                                        .when(task.is_completed(), |el| el.text_color(theme.colors().text_muted).line_through())
                                        .child(self.highlighted(&task.title, &query, cx)),
                                )
                                .when_some(due, |el, (label, overdue)| {
                                    el.child(
                                        Label::new(label)
                                            .size(LabelSize::XSmall)
                                            .color(if overdue && !task.is_completed() { Color::Error } else { Color::Accent }),
                                    )
                                })
                                .when_some(horizon, |el, (label, past)| {
                                    let (fg, bg, border) =
                                        task_rows::horizon_badge_style(past, task.is_closed(), &theme);
                                    el.child(
                                        div()
                                            .flex_none()
                                            .px(px(6.))
                                            .py(px(1.))
                                            .rounded(px(4.))
                                            .bg(bg)
                                            .border_1()
                                            .border_color(border)
                                            .child(
                                                Label::new(label)
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Custom(fg)),
                                            ),
                                    )
                                })
                                .child(Label::new(list).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                        .when_some(note, |el, note| {
                            el.child(
                                div()
                                    .pl(px(22.))
                                    .truncate()
                                    .text_size(px(12.))
                                    .line_height(px(15.))
                                    .text_color(theme.colors().text_muted)
                                    .child(self.highlighted(&note, &query, cx)),
                            )
                        })
                        .when_some(location_match, |el, loc| {
                            el.child(
                                h_flex()
                                    .pl(px(22.))
                                    .gap_1()
                                    .child(Icon::new(IconName::MapPin).size(IconSize::XSmall).color(Color::Muted))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(12.))
                                            .line_height(px(15.))
                                            .text_color(theme.colors().text_muted)
                                            .child(self.highlighted(&loc, &query, cx)),
                                    ),
                            )
                        })
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(SearchEvent::OpenTask(id))))
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        let more = self.shows(Filter::Tasks) && self.tasks.len() > TASK_LIMIT;

        let habit_rows: Vec<AnyElement> = if self.shows(Filter::Habits) {
            self.habits
                .iter()
                .map(|habit| {
                    let is_selected = has_query && row_index == selected_index;
                    row_index += 1;
                    let id = habit.id;
                    ListItem::new(SharedString::from(format!("search-habit-{id}")))
                        .height(px(30.))
                        .toggle_state(is_selected)
                        .start_slot(desktop_common::habit_helpers::avatar(habit, px(18.), cx))
                        .child(div().text_size(px(13.)).line_height(px(17.)).child(self.highlighted(&habit.name, &query, cx)))
                        .when(habit.archived, |item| {
                            item.end_slot(Label::new(strings.habit_archived).size(LabelSize::XSmall).color(Color::Muted))
                        })
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(SearchEvent::OpenHabit(id))))
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        let tag_rows = if self.shows(Filter::Tags) {
            self.tags
                .iter()
                .map(|t| {
                    let is_selected = has_query && row_index == selected_index;
                    row_index += 1;
                    let view = View::Tag(t.name.clone());
                    ListItem::new(SharedString::from(format!("search-tag-{}", t.name)))
                        .height(px(30.))
                        .toggle_state(is_selected)
                        .start_slot(Icon::new(IconName::Tag).color(lists_panel::color(t.color.as_deref(), cx)))
                        .child(div().text_size(px(13.)).line_height(px(17.)).child(self.highlighted(&t.name, &query, cx)))
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(SearchEvent::OpenView(view.clone()))))
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        let list_rows = if self.shows(Filter::Lists) {
            self.lists
                .iter()
                .map(|l| {
                    let is_selected = has_query && row_index == selected_index;
                    row_index += 1;
                    let view = View::List(l.id);
                    let start: AnyElement = match l.icon.as_deref().filter(|s| !s.is_empty()) {
                        Some(emoji) => div().w(px(16.)).flex().justify_center().text_size(px(13.)).child(emoji.to_owned()).into_any_element(),
                        None => {
                            let icon = if l.kind == Kind::Note { IconName::FileText } else { IconName::List };
                            Icon::new(icon).color(lists_panel::color(l.color.as_deref(), cx)).into_any_element()
                        }
                    };
                    ListItem::new(SharedString::from(format!("search-list-result-{}", l.id)))
                        .height(px(30.))
                        .toggle_state(is_selected)
                        .start_slot(start)
                        .child(div().text_size(px(13.)).line_height(px(17.)).child(self.highlighted(&l.name, &query, cx)))
                        .when(l.archived, |item| {
                            item.end_slot(Label::new(strings.habit_archived).size(LabelSize::XSmall).color(Color::Muted))
                        })
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(SearchEvent::OpenView(view.clone()))))
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        let command_rows = if self.filter == Filter::All {
            self.plugin_commands
                .iter()
                .map(|cmd| {
                    let is_selected = has_query && row_index == selected_index;
                    row_index += 1;
                    let plugin_id = cmd.plugin_id.clone();
                    let command_id = cmd.command_id.clone();
                    ListItem::new(SharedString::from(format!(
                        "search-plugin-cmd-{}-{}",
                        cmd.plugin_id, cmd.command_id
                    )))
                    .height(px(30.))
                    .toggle_state(is_selected)
                    .start_slot(Icon::new(IconName::Code).color(Color::Accent))
                    .child(div().text_size(px(13.)).line_height(px(17.)).child(self.highlighted(&cmd.title, &query, cx)))
                    .end_slot(Label::new(cmd.plugin_name.clone()).size(LabelSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SearchEvent::RunPluginCommand {
                            plugin_id: plugin_id.clone(),
                            command_id: command_id.clone(),
                            arg: None,
                        });
                    }))
                    .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        let plugin_hit_rows = if self.filter == Filter::All {
            self.plugin_hits
                .iter()
                .enumerate()
                .map(|(idx, hit)| {
                    let event = Self::plugin_hit_event(hit);
                    let is_selected = has_query && event.is_some() && row_index == selected_index;
                    if event.is_some() {
                        row_index += 1;
                    }
                    let mut item = ListItem::new(SharedString::from(format!(
                        "search-plugin-hit-{}-{idx}",
                        hit.plugin_id
                    )))
                    .height(px(30.))
                    .toggle_state(is_selected)
                    .start_slot(Icon::new(IconName::Zap).color(Color::Muted))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_size(px(13.)).line_height(px(17.)).child(self.highlighted(&hit.title, &query, cx)))
                            .when_some(hit.subtitle.clone(), |el, sub| {
                                el.child(Label::new(sub).size(LabelSize::XSmall).color(Color::Muted).truncate())
                            }),
                    );
                    if let Some(badge) = hit.badge.clone() {
                        item = item.end_slot(Label::new(badge).size(LabelSize::XSmall).color(Color::Muted));
                    }
                    if let Some(ev) = event {
                        item = item.on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ev.clone());
                        }));
                    }
                    item.into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        let nothing = has_query
            && !self.pending_search
            && task_rows.is_empty()
            && habit_rows.is_empty()
            && tag_rows.is_empty()
            && list_rows.is_empty()
            && command_rows.is_empty()
            && plugin_hit_rows.is_empty();

        v_flex()
            .w(px(560.))
            .max_h(px(520.))
            .p_3()
            .gap_2()
            .rounded(px(10.))
            .bg(theme.colors().elevated_surface_background)
            .border_1()
            .border_color(theme.colors().border)
            .shadow_lg()
            .text_color(theme.colors().text)
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if this.menu.is_some() {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "down" => {
                        this.move_selection(1, cx);
                        cx.stop_propagation();
                    }
                    "up" => {
                        this.move_selection(-1, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.emit_close(SearchEvent::Dismissed, cx);
            }))
            .child(
                h_flex()
                    .gap_2()
                    .px_1()
                    .h(px(36.))
                    .child(Icon::new(IconName::Search).size(IconSize::Large))
                    .child(div().flex_1().text_size(px(15.)).child(self.input.clone()))
                    // As the original's: a round clear button once something is typed.
                    .when(has_query, |el| {
                        el.child(
                            div()
                                .id("clear-search")
                                .size(px(16.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .bg(theme.colors().text_muted.opacity(0.5))
                                .hover(|s| s.bg(theme.colors().text_muted.opacity(0.75)))
                                .tooltip(Tooltip::text(strings.clear))
                                .child(Icon::new(IconName::X).size(IconSize::Custom(px(10.))).color(Color::Custom(theme.colors().elevated_surface_background)))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.input.update(cx, |input, cx| input.set_text("", cx));
                                    this.refresh(cx);
                                    window.focus(&this.input.focus_handle(cx), cx);
                                })),
                        )
                    })
                    .child(
                        IconButton::new("close-search", IconName::X)
                            .icon_size(IconSize::Medium)
                            .tooltip(Tooltip::text(strings.close))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.emit_close(SearchEvent::Dismissed, cx);
                            })),
                    ),
            )
            // As the original's: the line under the field, then the chips.
            .child(Divider::horizontal())
            .child(
                h_flex()
                    .gap_1p5()
                    .when(self.filter == Filter::All || self.filter == Filter::Tasks, |el| el.child(chip(Filter::Tasks, strings.search_filter_task, cx)))
                    .children(task_dropdowns)
                    .when(self.filter == Filter::All || self.filter == Filter::Habits, |el| el.child(chip(Filter::Habits, strings.section_habits, cx)))
                    .when(self.filter == Filter::All || self.filter == Filter::Tags, |el| el.child(chip(Filter::Tags, strings.search_filter_tag, cx)))
                    .when(self.filter == Filter::All || self.filter == Filter::Lists, |el| el.child(chip(Filter::Lists, strings.search_filter_list, cx))),
            )
            .child(
                v_scroll(
                    "search-results-scroll",
                    v_flex()
                        .id("search-results")
                        .when(!has_query, |el| {
                            // As in the original, the hint names what the chosen chip searches.
                            let hint = match self.filter {
                                Filter::All => strings.search_hint,
                                Filter::Tasks => strings.search_hint_task,
                                Filter::Habits => strings.section_habits,
                                Filter::Tags => strings.search_hint_tag,
                                Filter::Lists => strings.search_hint_list,
                            };
                            let icon = match self.filter {
                                Filter::Tasks => IconName::SquareCheck,
                                Filter::Habits => IconName::FernHabit,
                                Filter::Tags => IconName::Tag,
                                Filter::Lists => IconName::List,
                                Filter::All => IconName::Search,
                            };
                            el.items_center().justify_center().gap_2().child(Icon::new(icon).size(IconSize::XLarge).color(Color::Muted)).child(Label::new(hint).color(Color::Muted))
                        })
                        .when(nothing, |el| {
                            let create_query = query.clone();
                            let (create_label, truncated) =
                                Self::create_task_button_label(strings.add_task, &create_query);
                            let full_tooltip = truncated
                                .then(|| format!("{}: \"{}\"", strings.add_task, create_query));
                            el.items_center()
                                .justify_center()
                                .gap_3()
                                .child(Label::new(strings.nothing_found).color(Color::Muted))
                                .child(
                                    div()
                                        .max_w_full()
                                        .rounded(px(6.))
                                        .bg(theme.colors().accent.opacity(0.14))
                                        .when(cfg!(test), |el| {
                                            el.debug_selector(|| "search-create-task".to_owned())
                                        })
                                        .child(
                                            Button::new("search-create-task", create_label)
                                                .variant(ButtonVariant::Ghost)
                                                .size(ButtonSize::Md)
                                                .icon(IconName::Plus)
                                                .when_some(full_tooltip, |btn, tip| {
                                                    btn.tooltip(Tooltip::text(tip))
                                                })
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.emit(SearchEvent::CreateTask(
                                                        create_query.clone(),
                                                    ));
                                                })),
                                        ),
                                )
                        })
                        .when(!task_rows.is_empty(), |el| el.child(Self::group_heading(strings.group_tasks)).children(task_rows))
                        .when(more, |el| {
                            el.child(
                                div()
                                    .id("search-view-more")
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.))
                                    .hover(|s| s.bg(theme.colors().element_hover))
                                    .cursor_pointer()
                                    .child(Label::new(strings.view_more).size(LabelSize::Small).color(Color::Accent))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let query = this.query(cx);
                                        cx.emit(SearchEvent::OpenView(View::Search(query)))
                                    })),
                            )
                        })
                        .when(!habit_rows.is_empty(), |el| el.child(Self::group_heading(strings.section_habits)).children(habit_rows))
                        .when(!tag_rows.is_empty(), |el| el.child(Self::group_heading(strings.tags)).children(tag_rows))
                        .when(!list_rows.is_empty(), |el| el.child(Self::group_heading(strings.lists)).children(list_rows))
                        .when(!command_rows.is_empty(), |el| el.child(Self::group_heading(strings.plugins_commands)).children(command_rows))
                        .when(!plugin_hit_rows.is_empty(), |el| el.child(Self::group_heading(strings.plugins)).children(plugin_hit_rows)),
                )
                .track_scroll(&self.results_scroll)
                .flex_1()
                .min_h(px(180.)),
            )
            .child({
                let kbd = |text: &'static str| {
                    div()
                        .px_1p5()
                        .py(px(1.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme.colors().border)
                        .text_size(px(10.))
                        .text_color(theme.colors().text_muted)
                        .child(text)
                };
                v_flex()
                    .gap_2()
                    .child(Divider::horizontal())
                    .child(
                        h_flex()
                            .px_1()
                            .gap_3()
                            .items_center()
                            .child(h_flex().gap_1().child(kbd("↑↓")).child(kbd("Enter")))
                            .child(div().flex_1())
                            .child(kbd("Esc")),
                    )
            })
    }
}

#[cfg(test)]
mod tests;
