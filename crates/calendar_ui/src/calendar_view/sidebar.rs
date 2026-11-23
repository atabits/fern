use std::collections::HashSet;

use chrono::NaiveDate;
use fern_core::{Id, Task};
use gpui::FontWeight;
use ui::prelude::*;

use crate::calendar;
use crate::i18n;

use super::CalendarView;
use super::calendar_task_editor::month_picker;

const CALENDAR_SIDEBAR_WIDTH: f32 = 216.;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CalendarFilter {
    pub(super) all: bool,
    pub(super) lists: HashSet<Option<Id>>,
    pub(super) tags: HashSet<String>,
}

impl Default for CalendarFilter {
    fn default() -> Self {
        Self {
            all: true,
            lists: HashSet::new(),
            tags: HashSet::new(),
        }
    }
}

impl CalendarFilter {
    pub(super) fn single_list(&self) -> Option<Option<Id>> {
        if !self.all && self.tags.is_empty() && self.lists.len() == 1 {
            self.lists.iter().next().copied()
        } else {
            None
        }
    }

    pub(super) fn matches(&self, task: &Task) -> bool {
        if self.all {
            return true;
        }
        (!self.lists.is_empty() && self.lists.contains(&task.list_id))
            || (!self.tags.is_empty() && task.tags.iter().any(|t| self.tags.contains(&t.to_lowercase())))
    }

    pub(super) fn parse(raw: &str, valid_lists: &[(Id, String)], valid_tags: &[String]) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "all" {
            return Self::default();
        }
        if trimmed == "inbox" {
            return Self {
                all: false,
                lists: HashSet::from([None]),
                tags: HashSet::new(),
            };
        }
        if let Ok(id) = trimmed.parse::<Id>() {
            if valid_lists.iter().any(|(list, _)| *list == id) {
                return Self {
                    all: false,
                    lists: HashSet::from([Some(id)]),
                    tags: HashSet::new(),
                };
            }
            return Self::default();
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            return Self::default();
        };
        let mut lists = HashSet::new();
        if let Some(arr) = value.get("lists").and_then(|v| v.as_array()) {
            for item in arr.iter().filter_map(|v| v.as_str()) {
                if item == "inbox" {
                    lists.insert(None);
                } else if let Ok(id) = item.parse::<Id>()
                    && valid_lists.iter().any(|(list, _)| *list == id)
                {
                    lists.insert(Some(id));
                }
            }
        }
        let mut tags = HashSet::new();
        if let Some(arr) = value.get("tags").and_then(|v| v.as_array()) {
            for item in arr.iter().filter_map(|v| v.as_str()) {
                let lower = item.to_lowercase();
                if valid_tags.iter().any(|t| t.to_lowercase() == lower) {
                    tags.insert(lower);
                }
            }
        }
        if lists.is_empty() && tags.is_empty() {
            Self::default()
        } else {
            Self {
                all: false,
                lists,
                tags,
            }
        }
    }

    pub(super) fn encode(&self) -> String {
        if self.all || (self.lists.is_empty() && self.tags.is_empty()) {
            return String::new();
        }
        if let Some(single) = self.single_list() {
            return match single {
                None => "inbox".to_owned(),
                Some(id) => id.to_string(),
            };
        }
        let lists: Vec<String> = self
            .lists
            .iter()
            .map(|l| match l {
                None => "inbox".to_owned(),
                Some(id) => id.to_string(),
            })
            .collect();
        let tags: Vec<&str> = self.tags.iter().map(String::as_str).collect();
        serde_json::json!({ "lists": lists, "tags": tags }).to_string()
    }
}

impl CalendarView {
    pub(super) fn toggle_calendar_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        if let Err(error) = self
            .store
            .set_setting("calendar_sidebar", if self.sidebar_open { "1" } else { "0" })
        {
            eprintln!("failed to save the calendar sidebar setting: {error:#}");
        }
        cx.notify();
    }

    fn save_filter(&mut self, filter: CalendarFilter, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let encoded = filter.encode();
        if let Err(error) = self.store.set_setting("calendar_list_filter", &encoded) {
            eprintln!("failed to save the calendar filter: {error:#}");
            return;
        }
        self.close_card(cx);
        self.reload(cx);
    }

    #[cfg(test)]
    pub(super) fn set_list_filter(&mut self, filter: Option<Option<Id>>, cx: &mut Context<Self>) {
        let next = match filter {
            None => CalendarFilter::default(),
            Some(list) => CalendarFilter {
                all: false,
                lists: HashSet::from([list]),
                tags: HashSet::new(),
            },
        };
        self.save_filter(next, cx);
    }

    fn select_all_filter(&mut self, cx: &mut Context<Self>) {
        self.save_filter(CalendarFilter::default(), cx);
    }

    fn toggle_list_filter(&mut self, list: Option<Id>, cx: &mut Context<Self>) {
        let mut next = if self.filter.all {
            CalendarFilter {
                all: false,
                lists: HashSet::new(),
                tags: HashSet::new(),
            }
        } else {
            self.filter.clone()
        };
        if !next.lists.remove(&list) {
            next.lists.insert(list);
        }
        if next.lists.is_empty() && next.tags.is_empty() {
            next.all = true;
        }
        self.save_filter(next, cx);
    }

    fn toggle_all_lists_filter(&mut self, cx: &mut Context<Self>) {
        let all_lists: HashSet<Option<Id>> = std::iter::once(None)
            .chain(self.list_names.iter().map(|(id, _)| Some(*id)))
            .collect();
        let mut next = if self.filter.all {
            CalendarFilter {
                all: false,
                lists: HashSet::new(),
                tags: HashSet::new(),
            }
        } else {
            self.filter.clone()
        };
        if all_lists.iter().all(|l| next.lists.contains(l)) {
            next.lists.clear();
        } else {
            next.lists = all_lists;
        }
        if next.lists.is_empty() && next.tags.is_empty() {
            next.all = true;
        }
        self.save_filter(next, cx);
    }

    fn toggle_tag_filter(&mut self, tag: String, cx: &mut Context<Self>) {
        let key = tag.to_lowercase();
        let mut next = if self.filter.all {
            CalendarFilter {
                all: false,
                lists: HashSet::new(),
                tags: HashSet::new(),
            }
        } else {
            self.filter.clone()
        };
        if !next.tags.remove(&key) {
            next.tags.insert(key);
        }
        if next.lists.is_empty() && next.tags.is_empty() {
            next.all = true;
        }
        self.save_filter(next, cx);
    }

    fn toggle_all_tags_filter(&mut self, cx: &mut Context<Self>) {
        let all_tags: HashSet<String> = self.tag_names.iter().map(|t| t.to_lowercase()).collect();
        let mut next = if self.filter.all {
            CalendarFilter {
                all: false,
                lists: HashSet::new(),
                tags: HashSet::new(),
            }
        } else {
            self.filter.clone()
        };
        if !all_tags.is_empty() && all_tags.iter().all(|t| next.tags.contains(t)) {
            next.tags.clear();
        } else {
            next.tags = all_tags;
        }
        if next.lists.is_empty() && next.tags.is_empty() {
            next.all = true;
        }
        self.save_filter(next, cx);
    }

    pub(super) fn render_calendar_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let radio = |selected: bool| {
            div()
                .size(px(15.))
                .flex_none()
                .rounded_full()
                .border_1()
                .border_color(if selected { colors.accent } else { colors.checkbox_border })
                .flex()
                .items_center()
                .justify_center()
                .when(selected, |el| el.child(div().size(px(7.)).rounded_full().bg(colors.accent)))
        };
        let all_lists_checked = !self.filter.all
            && self.filter.lists.contains(&None)
            && self.list_names.iter().all(|(id, _)| self.filter.lists.contains(&Some(*id)));
        let all_tags_checked = !self.filter.all
            && !self.tag_names.is_empty()
            && self.tag_names.iter().all(|t| self.filter.tags.contains(&t.to_lowercase()));
        let check_row = |id: SharedString, label: String, bold: bool, icon: Option<(IconName, Hsla)>, checked: bool| {
            let box_id = SharedString::from(format!("{id}-box"));
            h_flex()
                .id(id.clone())
                .debug_selector(move || id.to_string())
                .h(px(32.))
                .px_2()
                .gap_2()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(colors.list_hover))
                .when_some(icon, |el, (name, color)| el.child(Icon::new(name).size(IconSize::Small).color(color)))
                .child(
                    div().flex_1().min_w_0().child(
                        Label::new(label)
                            .truncate()
                            .when(bold, |l| l.weight(FontWeight::SEMIBOLD))
                            .color(colors.list_text),
                    ),
                )
                .child(Checkbox::new(box_id).size(px(15.)).accent().toggle_state(checked))
        };
        v_scroll(
            "calendar-sidebar-scroll",
            v_flex()
                .id("calendar-sidebar")
                .debug_selector(|| "calendar-sidebar".to_owned())
                .pt(crate::title_bar::header_inset() + px(13.))
                .px_3()
                .pb_3()
                .gap_3()
                .child(month_picker(
                    self.sidebar_month.unwrap_or(self.month),
                    self.selected_day,
                    &self.sidebar_busy_days,
                    cx.listener(|this, delta: &i32, _, cx| {
                        this.sidebar_month = Some(calendar::add_months(this.sidebar_month.unwrap_or(this.month), *delta));
                        cx.notify();
                    }),
                    cx.listener(|this, _: &(), _, cx| {
                        this.sidebar_month = None;
                        this.go_to_today(cx);
                    }),
                    cx.listener(|this, day: &NaiveDate, _, cx| this.select_day(*day, cx)),
                    cx,
                ))
                .child(Divider::horizontal())
                .child(
                    v_flex()
                        .gap(px(2.))
                        .child(
                            h_flex()
                                .id("calendar-filter-all")
                                .debug_selector(|| "calendar-filter-all".to_owned())
                                .h(px(32.))
                                .px_2()
                                .gap_2()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .hover(|s| s.bg(colors.list_hover))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(Label::new(strings.all).weight(FontWeight::SEMIBOLD).color(colors.list_text)),
                                )
                                .child(radio(self.filter.all))
                                .on_click(cx.listener(|this, _, _, cx| this.select_all_filter(cx))),
                        )
                        .when(!self.tag_names.is_empty(), |el| {
                            el.child(
                                check_row("calendar-filter-tags".into(), strings.tags.to_owned(), true, None, all_tags_checked)
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_all_tags_filter(cx))),
                            )
                            .children(self.tag_names.iter().map(|tag| {
                                let lower = tag.to_lowercase();
                                let checked = !self.filter.all && self.filter.tags.contains(&lower);
                                let icon_color = self.tag_colors.get(&lower).copied().unwrap_or(colors.list_text_muted);
                                let tag_name = tag.clone();
                                check_row(
                                    SharedString::from(format!("calendar-filter-tag-{tag}")),
                                    tag.clone(),
                                    false,
                                    Some((IconName::Tag, icon_color)),
                                    checked,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle_tag_filter(tag_name.clone(), cx)))
                            }))
                        })
                        .child(
                            check_row("calendar-filter-lists".into(), strings.lists.to_owned(), true, None, all_lists_checked)
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_all_lists_filter(cx))),
                        )
                        .child(
                            check_row(
                                "calendar-filter-inbox".into(),
                                strings.inbox.to_owned(),
                                false,
                                Some((IconName::Inbox, colors.list_text_muted)),
                                !self.filter.all && self.filter.lists.contains(&None),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_list_filter(None, cx))),
                        )
                        .children(self.list_names.iter().map(|(id, name)| {
                            let list_id = *id;
                            let checked = !self.filter.all && self.filter.lists.contains(&Some(list_id));
                            let icon_color = self.list_colors.get(&list_id).copied().unwrap_or(colors.list_text_muted);
                            check_row(
                                SharedString::from(format!("calendar-filter-list-{list_id}")),
                                name.clone(),
                                false,
                                Some((IconName::List, icon_color)),
                                checked,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_list_filter(Some(list_id), cx)))
                        })),
                ),
        )
        .flex_none()
        .w(px(CALENDAR_SIDEBAR_WIDTH))
        .h_full()
        .border_r_1()
        .border_color(colors.lists_border)
        .bg(colors.lists_background)
        .color(colors.list_text)
    }
}
