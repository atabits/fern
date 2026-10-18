//! The search popover, matched to TickTick's: a field with filter chips, results grouped into
//! tasks, tags and lists, matches highlighted, and "View more" for the full result list.

use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::search::{DateRange, TagRule, TaskFilter};
use fern_core::{Id, List, Priority, Store, Tag, Task, View};
use gpui::{Entity, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle, StyledText};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::i18n;
use crate::lists_panel;
use crate::task_rows;
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};

/// How many tasks the popover lists before "View more".
const TASK_LIMIT: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    All,
    Tasks,
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

pub enum SearchEvent {
    OpenTask(Id),
    OpenView(View),
    Dismissed,
}

pub struct SearchModal {
    store: Rc<Store>,
    input: Entity<TextInput>,
    filter: Filter,
    tasks: Vec<Task>,
    tags: Vec<Tag>,
    lists: Vec<List>,
    all_lists: Vec<List>,
    /// The Task filter's choices, the open dropdown, and the choices being made in it (applied
    /// with OK, as the original's), with the Custom dates' fields.
    task_filter: TaskFilter,
    menu: Option<Menu>,
    draft: TaskFilter,
    custom_from: Entity<TextInput>,
    custom_until: Entity<TextInput>,
    custom_open: bool,
}

impl EventEmitter<SearchEvent> for SearchModal {}

impl Focusable for SearchModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl SearchModal {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.search_placeholder, cx));
        cx.subscribe(&input, |this, _, _: &Changed, cx| this.refresh(cx)).detach();
        cx.subscribe(&input, |_, _, _: &Cancelled, cx| cx.emit(SearchEvent::Dismissed)).detach();
        cx.subscribe(&input, |this, _, _: &Submitted, cx| this.open_first(cx)).detach();
        let all_lists = store.lists().unwrap_or_default();
        let custom_from = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.range_from, cx));
        let custom_until = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.range_until, cx));
        Self {
            store,
            input,
            filter: Filter::All,
            tasks: Vec::new(),
            tags: Vec::new(),
            lists: Vec::new(),
            all_lists,
            task_filter: TaskFilter::default(),
            menu: None,
            draft: TaskFilter::default(),
            custom_from,
            custom_until,
            custom_open: false,
        }
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

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        let today = Local::now().date_naive();
        self.tasks = self.store.tasks_in(&View::Search(query.clone()), today).unwrap_or_default();
        if self.filter == Filter::Tasks {
            let week_start = crate::calendar::week_start();
            self.tasks.retain(|task| self.task_filter.matches(task, today, week_start));
        }
        let matches = |name: &str| !task_rows::match_ranges(name, &query).is_empty();
        self.tags = self.store.tags().unwrap_or_default().into_iter().filter(|t| matches(&t.name)).collect();
        self.lists = self.all_lists.iter().filter(|l| matches(&l.name)).cloned().collect();
        cx.notify();
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
        let short = |d: NaiveDate| {
            let month = strings.months_short[d.month0() as usize];
            if strings.day_before_month { format!("{} {month}", d.day()) } else { format!("{month} {}", d.day()) }
        };
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
                for tag in self.store.tags().unwrap_or_default() {
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
                items.push(
                    row("search-date-custom".into(), None, Self::range_label(current, strings).replace(strings.all_option, strings.custom_range).into(), custom)
                        .when(!custom, |item| item.child(div()))
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
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).child(Label::new(label).size(LabelSize::Small).color(Color::Custom(gpui::white()))))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).child(Label::new(label).size(LabelSize::Small)))
        };
        popover(cx)
            .w(px(if menu == Menu::Date { 250. } else { 210. }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .child(v_flex().id("search-menu-items").max_h(px(260.)).overflow_y_scroll().children(items))
            .when(buttons, |el| {
                el.child(
                    h_flex()
                        .pt_2()
                        .gap_2()
                        .child(button("search-menu-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            this.custom_open = false;
                            cx.notify();
                        })))
                        .child(button("search-menu-ok", strings.ok, true).on_click(cx.listener(|this, _, _, cx| {
                            if this.menu == Some(Menu::Date) {
                                let strings = i18n::t(cx);
                                let today = Local::now().date_naive();
                                let from = crate::calendar::parse_date(this.custom_from.read(cx).text(), today, strings);
                                let until = crate::calendar::parse_date(this.custom_until.read(cx).text(), today, strings);
                                if let (Some(from), Some(until)) = (from, until) {
                                    this.draft.date = DateRange::Custom(from, until);
                                }
                            }
                            this.apply(cx);
                        }))),
                )
            })
            .into_any_element()
    }

    fn shows(&self, filter: Filter) -> bool {
        self.filter == Filter::All || self.filter == filter
    }

    fn open_first(&mut self, cx: &mut Context<Self>) {
        if self.shows(Filter::Tasks)
            && let Some(task) = self.tasks.first()
        {
            cx.emit(SearchEvent::OpenTask(task.id));
        } else if self.shows(Filter::Tags)
            && let Some(tag) = self.tags.first()
        {
            cx.emit(SearchEvent::OpenView(View::Tag(tag.name.clone())));
        } else if self.shows(Filter::Lists)
            && let Some(list) = self.lists.first()
        {
            cx.emit(SearchEvent::OpenView(View::List(list.id)));
        }
    }

    fn highlighted(&self, text: &str, query: &str, cx: &App) -> StyledText {
        let highlight = HighlightStyle {
            background_color: Some(cx.theme().status().warning.opacity(0.45)),
            ..Default::default()
        };
        StyledText::new(text.to_owned())
            .with_highlights(task_rows::match_ranges(text, query).into_iter().map(|range| (range, highlight)))
    }

    fn group_heading(label: &'static str) -> impl IntoElement {
        div().px_2().pt_2().pb_1().child(Label::new(label).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
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

        let task_rows: Vec<AnyElement> = if self.shows(Filter::Tasks) {
            self.tasks
                .iter()
                .take(TASK_LIMIT)
                .map(|task| {
                    let id = task.id;
                    let list = task
                        .list_id
                        .and_then(|id| self.all_lists.iter().find(|l| l.id == id))
                        .map_or(SharedString::new_static(strings.inbox), |l| l.name.clone().into());
                    let title_matches = !task_rows::match_ranges(&task.title, &query).is_empty();
                    let note = (!title_matches).then(|| task_rows::snippet(&task.notes, &query, 30)).flatten();
                    v_flex()
                        .id(SharedString::from(format!("search-task-{id}")))
                        .px_2()
                        .py_1()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.colors().ghost_element_hover))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Checkbox::new("search-checkbox").size(px(14.)).toggle_state(task.is_completed()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(13.))
                                        .when(task.is_completed(), |el| el.text_color(theme.colors().text_muted))
                                        .child(self.highlighted(&task.title, &query, cx)),
                                )
                                .child(Label::new(list).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                        .when_some(note, |el, note| {
                            el.child(
                                div()
                                    .pl(px(22.))
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(theme.colors().text_muted)
                                    .child(self.highlighted(&note, &query, cx)),
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

        let named_rows = |items: Vec<(SharedString, IconName, Hsla, View)>, cx: &mut Context<Self>| -> Vec<AnyElement> {
            items
                .into_iter()
                .map(|(name, icon, color, view)| {
                    ListItem::new(SharedString::from(format!("search-{name}-{icon:?}")))
                        .height(px(30.))
                        .start_slot(Icon::new(icon).color(color))
                        .child(div().text_size(px(13.)).child(self.highlighted(&name, &query, cx)))
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(SearchEvent::OpenView(view.clone()))))
                        .into_any_element()
                })
                .collect()
        };
        let tag_rows = if self.shows(Filter::Tags) {
            let items = self
                .tags
                .iter()
                .map(|t| (t.name.clone().into(), IconName::Tag, lists_panel::color(t.color.as_deref(), cx), View::Tag(t.name.clone())))
                .collect();
            named_rows(items, cx)
        } else {
            Vec::new()
        };
        let list_rows = if self.shows(Filter::Lists) {
            let items = self
                .lists
                .iter()
                .map(|l| (l.name.clone().into(), IconName::List, lists_panel::color(l.color.as_deref(), cx), View::List(l.id)))
                .collect();
            named_rows(items, cx)
        } else {
            Vec::new()
        };
        let nothing = has_query && task_rows.is_empty() && tag_rows.is_empty() && list_rows.is_empty();

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
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Dismissed)))
            .child(
                h_flex()
                    .gap_2()
                    .px_1()
                    .h(px(32.))
                    .child(Icon::new(IconName::Search).size(IconSize::Large))
                    .child(div().flex_1().text_size(px(15.)).child(self.input.clone()))
                    .child(
                        IconButton::new("close-search", IconName::X)
                            .icon_size(IconSize::Medium)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Dismissed))),
                    ),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .when(self.filter == Filter::All || self.filter == Filter::Tasks, |el| el.child(chip(Filter::Tasks, strings.search_filter_task, cx)))
                    .children(task_dropdowns)
                    .when(self.filter == Filter::All || self.filter == Filter::Tags, |el| el.child(chip(Filter::Tags, strings.search_filter_tag, cx)))
                    .when(self.filter == Filter::All || self.filter == Filter::Lists, |el| el.child(chip(Filter::Lists, strings.search_filter_list, cx))),
            )
            .child(Divider::horizontal())
            .child(
                v_flex()
                    .id("search-results")
                    .flex_1()
                    .min_h(px(180.))
                    .overflow_y_scroll()
                    .when(!has_query, |el| {
                        // As in the original, the hint names what the chosen chip searches.
                        let hint = match self.filter {
                            Filter::All => strings.search_hint,
                            Filter::Tasks => strings.search_hint_task,
                            Filter::Tags => strings.search_hint_tag,
                            Filter::Lists => strings.search_hint_list,
                        };
                        let icon = match self.filter {
                            Filter::Tasks => IconName::SquareCheck,
                            Filter::Tags => IconName::Tag,
                            Filter::Lists => IconName::List,
                            Filter::All => IconName::Search,
                        };
                        el.items_center().justify_center().gap_2().child(Icon::new(icon).size(IconSize::XLarge).color(Color::Muted)).child(Label::new(hint).color(Color::Muted))
                    })
                    .when(nothing, |el| {
                        el.items_center().justify_center().child(Label::new(strings.nothing_found).color(Color::Muted))
                    })
                    .when(!task_rows.is_empty(), |el| el.child(Self::group_heading(strings.group_tasks)).children(task_rows))
                    .when(more, |el| {
                        el.child(
                            div()
                                .id("search-view-more")
                                .px_2()
                                .py_1()
                                .cursor_pointer()
                                .child(Label::new(strings.view_more).size(LabelSize::Small).color(Color::Accent))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let query = this.query(cx);
                                    cx.emit(SearchEvent::OpenView(View::Search(query)))
                                })),
                        )
                    })
                    .when(!tag_rows.is_empty(), |el| el.child(Self::group_heading(strings.tags)).children(tag_rows))
                    .when(!list_rows.is_empty(), |el| el.child(Self::group_heading(strings.lists)).children(list_rows)),
            )
    }
}
