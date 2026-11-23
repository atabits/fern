use fern_core::{Id, Priority, Task};
use gpui::FontWeight;
use ui::prelude::*;

use crate::i18n;

use super::CalendarView;

impl CalendarView {
    /// The original's "Arrange tasks" panel at the calendar's right: the open tasks without a
    /// date under Lists, Tags or Priority tabs, dragged onto a day to plan them.
    pub(super) fn render_arrange(&self, tab: usize, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let mut groups: Vec<(String, Vec<&Task>)> = Vec::new();
        match tab {
            0 => {
                let mut by_list: std::collections::HashMap<Option<Id>, Vec<&Task>> = std::collections::HashMap::new();
                for task in &self.undated {
                    by_list.entry(task.list_id).or_default().push(task);
                }
                if let Some(tasks) = by_list.remove(&None) {
                    groups.push((strings.inbox.to_owned(), tasks));
                }
                for (id, name) in &self.list_names {
                    if let Some(tasks) = by_list.remove(&Some(*id)) {
                        groups.push((name.clone(), tasks));
                    }
                }
            }
            1 => {
                let mut by_tag: std::collections::BTreeMap<String, (String, Vec<&Task>)> = std::collections::BTreeMap::new();
                let mut untagged: Vec<&Task> = Vec::new();
                for task in &self.undated {
                    if task.tags.is_empty() {
                        untagged.push(task);
                    } else {
                        for tag in &task.tags {
                            let lower = tag.to_lowercase();
                            let entry = by_tag.entry(lower).or_insert_with(|| (tag.clone(), Vec::new()));
                            if entry.1.last().is_none_or(|t| t.id != task.id) {
                                entry.1.push(task);
                            }
                        }
                    }
                }
                for (_, (name, tasks)) in by_tag {
                    groups.push((format!("#{name}"), tasks));
                }
                if !untagged.is_empty() {
                    groups.push((strings.group_no_tags.to_owned(), untagged));
                }
            }
            _ => {
                let mut by_priority: [Vec<&Task>; 4] = Default::default();
                for task in &self.undated {
                    let idx = match task.priority {
                        Priority::High => 0,
                        Priority::Medium => 1,
                        Priority::Low => 2,
                        Priority::None => 3,
                    };
                    by_priority[idx].push(task);
                }
                for (tasks, label) in by_priority
                    .into_iter()
                    .zip([strings.priority_high, strings.priority_medium, strings.priority_low, strings.priority_none])
                {
                    if !tasks.is_empty() {
                        groups.push((label.to_owned(), tasks));
                    }
                }
            }
        }
        let tabs = SegmentedTabs::new().full_width().tabs(
            strings.arrange_tabs.iter().enumerate().map(|(i, label)| {
                SegmentedTab::new(SharedString::from(format!("arrange-tab-{i}")), *label)
                    .toggle_state(i == tab)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.arrange = Some(i);
                        cx.notify();
                    }))
            }),
        );
        let mut body: Vec<AnyElement> = Vec::new();
        for (index, (name, tasks)) in groups.into_iter().enumerate() {
            let key = (tab, name.clone());
            let collapsed = self.arrange_collapsed.contains(&key);
            body.push(
                h_flex()
                    .id(SharedString::from(format!("arrange-group-{tab}-{index}")))
                    .pt_2()
                    .pb_1()
                    .gap_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.colors().ghost_element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.arrange_collapsed.remove(&key) {
                            this.arrange_collapsed.insert(key.clone());
                        }
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(Label::new(name).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                    .child(Label::new(tasks.len().to_string()).size(LabelSize::XSmall).color(Color::Muted))
                    .into_any_element(),
            );
            if !collapsed {
                body.extend(tasks.into_iter().map(|t| self.task_bar(t, None, cx).h(px(20.)).into_any_element()));
            }
        }
        if body.is_empty() {
            body.push(
                div()
                    .pt_8()
                    .px_2()
                    .flex()
                    .justify_center()
                    .child(Label::new(strings.arrange_empty).size(LabelSize::Small).color(Color::Muted))
                    .into_any_element(),
            );
        }
        v_flex()
            .id("calendar-arrange")
            .w(px(240.))
            .flex_none()
            .h_full()
            .px_3()
            .pt(crate::title_bar::height() + px(4.))
            .gap_2()
            .border_l_1()
            .border_color(theme.colors().border_variant)
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.arrange_tasks_title).weight(FontWeight::SEMIBOLD)))
                    .child(
                        IconButton::new("arrange-close", IconName::X)
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .tooltip(ui::Tooltip::text(strings.close))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.arrange = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(tabs)
            .child(
                v_scroll(
                    ("calendar-arrange-tasks-scroll", tab),
                    v_flex().id(("arrange-tasks", tab)).gap(px(3.)).children(body),
                )
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }
}
