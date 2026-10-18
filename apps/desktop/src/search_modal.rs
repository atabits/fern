//! The search popover, matched to TickTick's: a field with filter chips, results grouped into
//! tasks, tags and lists, matches highlighted, and "View more" for the full result list.

use std::rc::Rc;

use chrono::Local;
use fern_core::{Id, List, Store, Tag, Task, View};
use gpui::{Entity, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle, StyledText};
use ui::prelude::*;

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
        Self { store, input, filter: Filter::All, tasks: Vec::new(), tags: Vec::new(), lists: Vec::new(), all_lists }
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
        let matches = |name: &str| !task_rows::match_ranges(name, &query).is_empty();
        self.tags = self.store.tags().unwrap_or_default().into_iter().filter(|t| matches(&t.name)).collect();
        self.lists = self.all_lists.iter().filter(|l| matches(&l.name)).cloned().collect();
        cx.notify();
    }

    fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        cx.notify();
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

        let chip = |filter: Filter, label: &'static str, cx: &mut Context<Self>| {
            let selected = self.filter == filter;
            div()
                .id(label)
                .px_2()
                .py_0p5()
                .rounded(px(4.))
                .cursor_pointer()
                .bg(if selected { theme.colors().element_selected } else { theme.colors().element_hover })
                .child(Label::new(label).size(LabelSize::Small))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let next = if this.filter == filter { Filter::All } else { filter };
                    this.set_filter(next, cx)
                }))
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
                    .gap_1()
                    .child(chip(Filter::Tasks, strings.search_filter_task, cx))
                    .child(chip(Filter::Tags, strings.search_filter_tag, cx))
                    .child(chip(Filter::Lists, strings.search_filter_list, cx)),
            )
            .child(Divider::horizontal())
            .child(
                v_flex()
                    .id("search-results")
                    .flex_1()
                    .min_h(px(180.))
                    .overflow_y_scroll()
                    .when(!has_query, |el| {
                        el.items_center().justify_center().gap_2().child(Icon::new(IconName::Search).size(IconSize::XLarge)).child(
                            Label::new(strings.search_hint).color(Color::Muted),
                        )
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
