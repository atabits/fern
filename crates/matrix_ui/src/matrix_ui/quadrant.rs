use std::collections::HashMap;

use chrono::NaiveDate;
use desktop_common::touch;
use fern_core::matrix::MatrixKind;
use fern_core::{Id, Kind, List, Task};
use fern_i18n::{self as i18n, Strings};
use gpui::{AnyElement, FontWeight};
use task_ui::drag::DraggedTask;
use task_ui::lists_panel;
use task_ui::task_rows::{self, GroupBy, SortBy};
use ui::prelude::*;
use ui::{EmptyState, Placement, PopoverAttachment, popover};

use crate::{MatrixView, OpenTask};

/// A quadrant's Group by, Sort by, and Order direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuadrantView {
    pub group: GroupBy,
    pub sort: SortBy,
    pub newest_first: bool,
}

impl Default for QuadrantView {
    /// Default matching the web reference: grouped by Date, sorted by Date ascending.
    fn default() -> Self {
        Self { group: GroupBy::Date, sort: SortBy::Date, newest_first: false }
    }
}

impl QuadrantView {
    pub(super) fn to_storage(self) -> String {
        format!(
            "{},{},{}",
            self.group.key(),
            self.sort.key(),
            if self.newest_first { "newest" } else { "oldest" }
        )
    }

    pub(super) fn from_storage(text: &str) -> Self {
        if text.trim().is_empty() {
            return Self::default();
        }
        let mut parts = text.split(',');
        let group = parts.next().map(GroupBy::from_key).unwrap_or(GroupBy::Date);
        let group = match group {
            GroupBy::Custom => GroupBy::List,
            other => other,
        };
        let sort = parts.next().map(SortBy::from_key).unwrap_or(SortBy::Date);
        let newest_first = parts.next() == Some("newest");
        Self { group, sort, newest_first }
    }
}

/// Cached grouped tasks for one quadrant so `render()` does not re-filter and re-sort every frame.
#[derive(Clone, Default)]
pub(super) struct QuadrantCache {
    pub(super) open_count: usize,
    /// Open task groups: `(group_label, tasks)`.
    pub(super) groups: Vec<(String, Vec<Task>)>,
    /// Completed tasks shown in a dedicated collapsible section at the bottom.
    pub(super) completed: Vec<Task>,
}

/// Payload for dragging a quadrant header to swap two quadrants' positions.
#[derive(Clone)]
struct DraggedQuadrant {
    quadrant: usize,
    title: SharedString,
}

impl Render for DraggedQuadrant {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_3()
            .py_1p5()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().accent)
            .bg(theme.colors().elevated_surface_background)
            .shadow_md()
            .child(Label::new(self.title.clone()).size(LabelSize::Small))
    }
}

/// Arranges open tasks of a quadrant into groups, applying manual order (when present) or the
/// chosen sort & direction.
pub fn arrange(
    tasks: Vec<Task>,
    view: QuadrantView,
    lists: &[List],
    today: NaiveDate,
    strings: &Strings,
) -> Vec<(String, Vec<Task>)> {
    arrange_with_horizons(tasks, view, lists, today, strings, &[], false)
}

pub(super) fn arrange_with_manual(
    tasks: Vec<Task>,
    view: QuadrantView,
    lists: &[List],
    today: NaiveDate,
    strings: &Strings,
    manual_order: &[Id],
    cx: &App,
) -> Vec<(String, Vec<Task>)> {
    arrange_with_horizons(
        tasks,
        view,
        lists,
        today,
        strings,
        manual_order,
        desktop_common::calendar::horizons_enabled(cx),
    )
}

fn arrange_with_horizons(
    mut tasks: Vec<Task>,
    view: QuadrantView,
    lists: &[List],
    today: NaiveDate,
    strings: &Strings,
    manual_order: &[Id],
    horizons_enabled: bool,
) -> Vec<(String, Vec<Task>)> {
    if !manual_order.is_empty() {
        let pos: HashMap<Id, usize> = manual_order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        tasks.sort_by_key(|t| pos.get(&t.id).copied().unwrap_or(usize::MAX));
    } else {
        task_rows::sort_tasks(&mut tasks, view.sort);
        let natural_newest = matches!(view.sort, SortBy::Created | SortBy::Modified);
        if natural_newest != view.newest_first {
            tasks.reverse();
        }
    }

    match view.group {
        GroupBy::List | GroupBy::Custom => {
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
                    let name = list
                        .and_then(|id| lists.iter().find(|l| l.id == id))
                        .map_or_else(|| strings.inbox.to_owned(), |l| l.name.clone());
                    (name, tasks)
                })
                .collect()
        }
        GroupBy::None => {
            if tasks.is_empty() {
                Vec::new()
            } else {
                vec![(String::new(), tasks)]
            }
        }
        other => {
            let mut groups: Vec<(String, Vec<Task>)> = Vec::new();
            for row in
                task_rows::grouped_rows_with(&tasks, other, today, false, horizons_enabled, strings)
            {
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

impl MatrixView {
    pub(super) fn default_title(quadrant: usize, strings: &Strings) -> &'static str {
        [
            strings.matrix_urgent_important,
            strings.matrix_not_urgent_important,
            strings.matrix_urgent_unimportant,
            strings.matrix_not_urgent_unimportant,
        ][quadrant]
    }

    pub(crate) fn quadrant_title(&self, quadrant: usize, strings: &Strings) -> (&'static str, SharedString) {
        let name = &self.rules[quadrant].name;
        let title = if name.trim().is_empty() {
            Self::default_title(quadrant, strings).into()
        } else {
            name.clone().into()
        };
        (["I", "II", "III", "IV"][quadrant], title)
    }

    /// Exact quadrant accent colors matching the web reference:
    /// Q1 `#FF5F68`, Q2 `#FFB000`, Q3 `#4772FA`, Q4 `#0CCE9C`.
    pub(crate) fn quadrant_color(quadrant: usize) -> Hsla {
        match quadrant {
            0 => gpui::rgb(0xFF5F68).into(),
            1 => gpui::rgb(0xFFB000).into(),
            2 => gpui::rgb(0x4772FA).into(),
            _ => gpui::rgb(0x0CCE9C).into(),
        }
    }

    /// Contrast-safe quadrant header title color (vibrant on Dark, darkened WCAG AA on Light).
    pub(crate) fn quadrant_text_color(quadrant: usize, dark: bool) -> Hsla {
        if dark {
            Self::quadrant_color(quadrant)
        } else {
            match quadrant {
                0 => gpui::rgb(0xD93842).into(),
                1 => gpui::rgb(0xB45309).into(),
                2 => gpui::rgb(0x2F58D6).into(),
                _ => gpui::rgb(0x047857).into(),
            }
        }
    }

    fn render_view_rows(&self, quadrant: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let view = self.views[quadrant];
        let has_manual = !self.manual_order[quadrant].is_empty();
        let note_only = self.rules[quadrant].kind == MatrixKind::NoteOnly;
        let is_time_sort = matches!(view.sort, SortBy::Date | SortBy::Created | SortBy::Modified);
        let order_labels = if is_time_sort {
            strings.order_options
        } else {
            [strings.matrix_order_asc, strings.matrix_order_desc]
        };

        type ViewRow = (u8, IconName, &'static str, &'static str, Vec<(&'static str, QuadrantView)>);
        let mut sort_choices: Vec<SortBy> = vec![SortBy::Date, SortBy::Created, SortBy::Modified, SortBy::Title, SortBy::Tag];
        if !note_only {
            sort_choices.push(SortBy::Priority);
        }

        let mut rows: Vec<ViewRow> = vec![
            (
                0,
                IconName::LayoutGrid,
                strings.group_by,
                view.group.label(strings),
                [GroupBy::Date, GroupBy::List, GroupBy::Created, GroupBy::Tag, GroupBy::Priority, GroupBy::None]
                    .map(|group| (group.label(strings), QuadrantView { group, ..view }))
                    .to_vec(),
            ),
            (
                1,
                IconName::ArrowDownUp,
                strings.sort_by,
                view.sort.label(strings),
                sort_choices.into_iter().map(|sort| (sort.label(strings), QuadrantView { sort, ..view })).collect(),
            ),
        ];
        if !has_manual {
            rows.push((
                2,
                IconName::ArrowDownUp,
                strings.order,
                order_labels[view.newest_first as usize],
                [false, true]
                    .map(|newest_first| (order_labels[newest_first as usize], QuadrantView { newest_first, ..view }))
                    .to_vec(),
            ));
        }

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
                            .child(
                                Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight })
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            ),
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
                            .child(
                                div().pl_6().child(
                                    Label::new(choice)
                                        .size(LabelSize::Small)
                                        .color(if selected { Color::Accent } else { Color::Default }),
                                ),
                            )
                            .when(selected, |item| {
                                item.end_slot(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.views[quadrant] = next;
                                let key = this.profile_setting_key(&format!("matrix_view_{quadrant}"));
                                if let Err(err) = this.store.set_setting(&key, &next.to_storage()) {
                                    desktop_common::logging::log_error("matrix_ui", format!("failed to save the quadrant's view: {err:#}"));
                                }
                                this.menu_row = None;
                                this.rebuild_cache(cx);
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
        if has_manual {
            items.push(
                ListItem::new("quadrant-reset-manual")
                    .start_slot(Icon::new(IconName::RotateCcw).color(Color::Muted))
                    .child(Label::new(strings.matrix_reset_manual_order))
                    .on_click(cx.listener(move |this, _, _, cx| this.reset_manual_order(quadrant, cx)))
                    .into_any_element(),
            );
        }
        items
    }

    fn task_children(&self, parent_id: Id) -> impl Iterator<Item = &Task> {
        self.children_by_parent
            .get(&parent_id)
            .into_iter()
            .flat_map(|indices| indices.iter().map(|&idx| &self.tasks[idx]))
    }

    fn child_subtasks(&self, parent_id: Id) -> Vec<&Task> {
        self.task_children(parent_id)
            .filter(|t| {
                !t.is_deleted()
                    && !t.is_wont_do()
                    && (self.show_completed || !t.is_completed())
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn push_task_tree(
        &self,
        task: &Task,
        quadrant: usize,
        today: NaiveDate,
        list_name: &impl Fn(Option<Id>) -> SharedString,
        depth: usize,
        out: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        out.push(self.render_task(task, quadrant, today, &list_name(task.list_id), depth, cx));
        if !self.show_subtasks && depth < 4 && self.expanded_parents.contains(&(quadrant, task.id)) {
            for child in self.child_subtasks(task.id) {
                self.push_task_tree(child, quadrant, today, list_name, depth + 1, out, cx);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_task(
        &self,
        task: &Task,
        quadrant: usize,
        today: NaiveDate,
        list_name: &SharedString,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = task.id;
        let completed = task.is_completed();
        let is_note = task.kind == Kind::Note;
        let selected = self.selected_task == Some(id);
        let tag_color = |name: &str| {
            lists_panel::color(self.tag_colors.get(name).map(String::as_str), cx)
        };
        let countdown_dates = self.preferences.countdown_dates;
        let due = task
            .due_date
            .map(|date| {
                task_rows::row_due_label(date, task.due_time, today, strings, countdown_dates, cx)
            });
        let badge_group = if completed {
            GroupBy::None
        } else {
            self.views[quadrant].group
        };
        let horizon_badge = task_rows::horizon_badge_for_group(task, badge_group, today, strings, cx);
        let has_notes = !is_note && !task.notes.trim().is_empty();
        let has_location = task.location.as_deref().is_some_and(|l| !l.trim().is_empty());
        let mut has_visible_children = false;
        let (mut done_subs, mut total_subs) = (0usize, 0usize);
        for child in self.task_children(id) {
            if child.is_deleted() || child.is_wont_do() {
                continue;
            }
            total_subs += 1;
            if child.is_completed() {
                done_subs += 1;
                if self.show_completed {
                    has_visible_children = true;
                }
            } else {
                has_visible_children = true;
            }
        }
        let has_children = !self.show_subtasks && has_visible_children;
        let expanded = has_children && self.expanded_parents.contains(&(quadrant, id));
        let subtask_progress = (total_subs > 0).then_some((done_subs, total_subs));
        let checklist_progress = task.checklist;
        let has_reminder = !task.reminders.is_empty();
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        let tags: Vec<AnyElement> =
            task.tags.iter().map(|tag| Chip::new(tag.clone(), tag_color(tag)).into_any_element()).collect();

        h_flex()
            .id(SharedString::from(format!("matrix-q{quadrant}-task-{id}")))
            .debug_selector(move || format!("matrix-task-{id}"))
            .h(px(40.))
            .flex_none()
            .gap(px(8.))
            .pl(px(8. + depth as f32 * 22.))
            .pr_2()
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme.colors().element_selected))
            .hover(|s| s.bg(theme.colors().ghost_element_hover))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .drag_over::<DraggedTask>(|s, _, _, cx| s.border_t_2().border_color(cx.theme().colors().accent))
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                cx.stop_propagation();
                this.drop_task_on_task(dragged.id, id, quadrant, cx);
            }))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                this.selected_task = Some(id);
                if event.click_count() >= 2 {
                    this.close_detail_card(cx);
                    cx.emit(OpenTask(id));
                } else {
                    this.open_detail_card(id, window, cx);
                }
            }))
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.selected_task = Some(id);
                    this.task_menu = Some((id, ev.position));
                    cx.notify();
                }),
            )
            .when(has_children, |el| {
                el.child(
                    div()
                        .id(SharedString::from(format!("matrix-expand-q{quadrant}-{id}")))
                        .w(px(16.))
                        .h(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .hover(|s| s.bg(theme.colors().element_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if !this.expanded_parents.remove(&(quadrant, id)) {
                                this.expanded_parents.insert((quadrant, id));
                            }
                            cx.notify();
                        }))
                        .child(
                            Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        ),
                )
            })
            .child(if is_note {
                div()
                    .w(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::FileText).size(IconSize::Small).color(Color::Muted))
                    .into_any_element()
            } else {
                Checkbox::new(SharedString::from(format!("matrix-cb-{quadrant}-{id}")))
                    .toggle_state(completed)
                    .in_progress(task.is_in_progress())
                    .tint(task_rows::priority_color(task.priority, &theme))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_complete(id, cx);
                    }))
                    .into_any_element()
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(completed, |el| el.line_through())
                            .child(
                                Label::new(task.title.clone())
                                    .color(if completed { Color::Muted } else { Color::Default })
                                    .truncate(),
                            ),
                    )
                    .children(tags)
                    .child(Label::new(list_name.clone()).size(LabelSize::Small).color(Color::Muted))
                    .when(has_notes, |el| {
                        el.child(Icon::new(IconName::FileText).size(IconSize::XSmall).color(Color::Muted))
                    })
                    .when(has_location, |el| {
                        el.child(Icon::new(IconName::MapPin).size(IconSize::XSmall).color(Color::Muted))
                    })
                    .when_some(checklist_progress, |el, (done_items, total)| {
                        el.child(
                            h_flex()
                                .gap_0p5()
                                .child(Icon::new(IconName::ListChecks).size(IconSize::XSmall).color(Color::Muted))
                                .child(Label::new(format!("{done_items}/{total}")).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                    })
                    .when_some(subtask_progress, |el, (done_subs, total_subs)| {
                        el.child(
                            h_flex()
                                .gap_0p5()
                                .child(Icon::new(IconName::ListTree).size(IconSize::XSmall).color(Color::Muted))
                                .child(Label::new(format!("{done_subs}/{total_subs}")).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                    })
                    .when(has_reminder, |el| {
                        el.child(
                            Icon::new(IconName::AlarmClock)
                                .size(IconSize::XSmall)
                                .color(if task.constant_reminder { Color::Accent } else { Color::Muted }),
                        )
                    })
                    .when(task.repeat.is_some(), |el| {
                        el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Accent))
                    })
                    .when_some(due, |el, (label, overdue)| {
                        let due_tip = match (countdown_dates, task.due_date) {
                            (true, Some(d)) => format!("{} · {}", desktop_common::calendar::format_date(d, strings, cx), strings.show_countdown_mode),
                            _ => strings.show_countdown_mode.to_owned(),
                        };
                        el.child(
                            div()
                                .id(SharedString::from(format!("matrix-due-toggle-{quadrant}-{id}")))
                                .px_1()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.colors().element_hover))
                                .tooltip(Tooltip::text(due_tip))
                                .child(
                                    Label::new(label)
                                        .size(LabelSize::Small)
                                        .color(if completed {
                                            Color::Muted
                                        } else if overdue {
                                            Color::Error
                                        } else {
                                            Color::Accent
                                        }),
                                )
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    if let Err(err) = this.store.update_preferences(|p| p.countdown_dates = !p.countdown_dates) {
                                        desktop_common::logging::log_error("matrix_ui", format!("failed to toggle countdown dates: {err:#}"));
                                    } else {
                                        this.preferences.countdown_dates = !this.preferences.countdown_dates;
                                        desktop_common::data_version::data_changed_scoped(
                                            cx.entity_id(),
                                            desktop_common::data_version::DataScope::TASKS,
                                            cx,
                                        );
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    .when_some(horizon_badge, |el, (badge, past)| {
                        let (fg, bg, border) =
                            task_rows::horizon_badge_style(past, completed || task.is_wont_do(), &theme);
                        el.child(
                            div()
                                .flex_none()
                                .px(px(5.))
                                .py(px(0.5))
                                .rounded(px(4.))
                                .bg(bg)
                                .border_1()
                                .border_color(border)
                                .child(Label::new(badge).size(LabelSize::XSmall).color(Color::Custom(fg))),
                        )
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_quadrant(&self, quadrant: usize, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let color = Self::quadrant_color(quadrant);
        let (numeral, title) = self.quadrant_title(quadrant, strings);
        let cache = &self.cache[quadrant];

        let list_name = |list: Option<Id>| -> SharedString {
            list.and_then(|id| self.list_names.get(&id).cloned())
                .unwrap_or_else(|| SharedString::new_static(strings.inbox))
        };

        let mut content: Vec<AnyElement> = Vec::new();
        for (label, tasks) in &cache.groups {
            let collapsed = !label.is_empty() && self.collapsed_groups.contains(&(quadrant, label.clone()));
            if !label.is_empty() {
                let key_label = label.clone();
                content.push(
                    h_flex()
                        .id(SharedString::from(format!("q{quadrant}-grp-{label}")))
                        .h(px(28.))
                        .gap_1()
                        .px_1()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let key = (quadrant, key_label.clone());
                            if !this.collapsed_groups.remove(&key) {
                                this.collapsed_groups.insert(key);
                            }
                            cx.notify();
                        }))
                        .child(
                            Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(Label::new(label.clone()).weight(FontWeight::SEMIBOLD))
                        .child(Label::new(tasks.len().to_string()).size(LabelSize::XSmall).color(Color::Muted))
                        .into_any_element(),
                );
            }
            if !collapsed {
                for task in tasks {
                    self.push_task_tree(task, quadrant, today, &list_name, 0, &mut content, cx);
                }
            }
        }

        // Dedicated collapsible Completed section at the bottom of the quadrant.
        if self.show_completed && !cache.completed.is_empty() {
            let done_key = "__completed__".to_owned();
            let collapsed = self.collapsed_groups.contains(&(quadrant, done_key.clone()));
            content.push(
                h_flex()
                    .id(SharedString::from(format!("q{quadrant}-completed-hdr")))
                    .h(px(28.))
                    .mt_1()
                    .gap_1()
                    .px_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let key = (quadrant, done_key.clone());
                        if !this.collapsed_groups.remove(&key) {
                            this.collapsed_groups.insert(key);
                        }
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(Label::new(strings.completed).weight(FontWeight::SEMIBOLD).color(Color::Muted))
                    .child(
                        Label::new(cache.completed.len().to_string())
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            );
            if !collapsed {
                for task in &cache.completed {
                    self.push_task_tree(task, quadrant, today, &list_name, 0, &mut content, cx);
                }
            }
        }

        let empty = content.is_empty();
        let drag_title = title.clone();

        v_flex()
            .id(SharedString::from(format!("quadrant-{numeral}")))
            .flex_1()
            .min_w_0()
            .min_h(px(140.))
            .h_full()
            .px(px(14.))
            .py_2()
            .gap_1()
            .rounded(px(12.))
            .bg(theme.colors().matrix_background)
            .drag_over::<DraggedTask>(move |style, _, _, _| style.border_1().border_color(color))
            .drag_over::<DraggedQuadrant>(move |style, _, _, _| style.border_2().border_color(color))
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on(dragged.id, quadrant, cx)))
            .on_drop(cx.listener(move |this, dragged: &DraggedQuadrant, _, cx| {
                this.swap_quadrants(dragged.quadrant, quadrant, cx);
            }))
            .child({
                let header_group = SharedString::from(format!("quadrant-header-{quadrant}"));
                h_flex()
                    .id(SharedString::from(format!("quadrant-header-{numeral}")))
                    .group(header_group.clone())
                    .h(px(36.))
                    .gap_1p5()
                    .cursor_grab()
                    .on_drag(DraggedQuadrant { quadrant, title: drag_title }, |dragged, _, _, cx| {
                        cx.new(|_| dragged.clone())
                    })
                    .child(
                        div()
                            .size(px(16.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(color)
                            .text_color(theme::foreground_on(&[color], 3.))
                            .text_size(px(8.))
                            .font_weight(FontWeight::BOLD)
                            .child(numeral),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(
                                    Label::new(title)
                                        .size(LabelSize::Small)
                                        .weight(FontWeight::SEMIBOLD)
                                        .color(Color::Custom(Self::quadrant_text_color(quadrant, theme.is_dark())))
                                        .truncate(),
                                )
                                .when(cache.open_count > 0, |el| {
                                    el.child(
                                        Label::new(cache.open_count.to_string())
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                }),
                        ),
                    )
                    .child(
                        div()
                            .opacity(if touch::active() { 1. } else { 0. })
                            .group_hover(header_group.clone(), |s| s.opacity(1.))
                            .child(
                                IconButton::new(SharedString::from(format!("quadrant-add-{quadrant}")), IconName::Plus)
                                    .size(px(22.))
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Muted)
                                    .hover_icon_color(Color::Icon)
                                    .tooltip(Tooltip::text(strings.add_task))
                                    .on_click(cx.listener(move |this, _, window, cx| this.start_add(quadrant, window, cx))),
                            ),
                    )
                    .child(
                        div()
                            .relative()
                            .opacity(if self.menu == Some(quadrant) || touch::active() { 1. } else { 0. })
                            .group_hover(header_group, |s| s.opacity(1.))
                            .child(
                                IconButton::new(SharedString::from(format!("quadrant-more-{quadrant}")), IconName::Ellipsis)
                                    .size(px(22.))
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Muted)
                                    .hover_icon_color(Color::Icon)
                                    .tooltip(Tooltip::text(strings.more))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.menu = if this.menu == Some(quadrant) { None } else { Some(quadrant) };
                                        this.menu_row = None;
                                        window.focus(&this.focus, cx);
                                        cx.notify();
                                    })),
                            )
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
                                                .start_slot(Icon::new(IconName::Pencil).color(Color::Icon))
                                                .child(Label::new(strings.edit))
                                                .on_click(cx.listener(move |this, _, _, cx| this.start_edit(quadrant, cx))),
                                        )
                                        .children(self.render_view_rows(quadrant, cx))
                                        .into_any_element(),
                                ))
                            }),
                    )
            })
            .child(
                v_scroll(
                    SharedString::from(format!("quadrant-tasks-scroll-{numeral}")),
                    v_flex()
                        .id(SharedString::from(format!("quadrant-tasks-{numeral}")))
                        .debug_selector(move || format!("matrix-quadrant-{numeral}"))
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
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .debug_selector(|| format!("matrix-add-input-{quadrant}"))
                                        .child(input.clone()),
                                )
                        }))
                        .when(empty && self.adding.as_ref().is_none_or(|(q, _)| *q != quadrant), |el| {
                            el.child(
                                EmptyState::new(IconName::Inbox, strings.no_tasks).action(
                                    SharedString::from(format!("matrix-empty-add-{quadrant}")),
                                    strings.add_task,
                                    cx.listener(move |this, _, window, cx| this.start_add(quadrant, window, cx)),
                                ),
                            )
                        })
                        .children(content),
                )
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }
}
