//! Slide-over `TaskDetail` drawer, horizon quick-assign chips, filter/options popovers,
//! rollover actions, and reflection prompt bar for [`HorizonsView`].

use chrono::{Datelike, NaiveDate};
use desktop_common::calendar;
use desktop_common::colors;
use desktop_common::data_version::data_changed;
use desktop_common::layout;
use desktop_common::undo::UndoAction;
use fern_core::{Horizon, Id, Priority, Task};
use fern_i18n as i18n;
use gpui::{AnyElement, Context, FontWeight, Hsla, MouseButton, SharedString, Window, div, px};
use ui::prelude::*;
use ui::{
    Icon, IconButton, IconName, IconSize, Label, LabelSize, Switch, Tooltip, popover, v_scroll,
};

use crate::column::{ColumnTarget, HorizonSlot};
use crate::prompts;
use crate::{
    HorizonsView, SETTING_SHOW_COMPLETED, SETTING_SHOW_DATED_IN_WEEK, SETTING_SHOW_PROMPT,
    UndoEntry,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActivePopover {
    Filter,
    Options,
}

impl HorizonsView {
    pub(crate) fn toggle_rollover_collapsed(&mut self, col_key: &str, cx: &mut Context<Self>) {
        if !self.collapsed_rollovers.remove(col_key) {
            self.collapsed_rollovers.insert(col_key.to_owned());
        }
        cx.notify();
    }

    pub(super) fn close_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.text_edit_sync_task.take().is_some() {
            data_changed(cx.entity_id(), cx);
        }
        self.detail_task = None;
        self.detail.update(cx, |d, cx| d.show(None, cx));
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    pub(crate) fn rollover_tasks_to_target(
        &mut self,
        ids: &[Id],
        target: ColumnTarget,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        let changed = match target {
            ColumnTarget::Day(day) => self.store.rollover_overdue_to_day(ids, day).is_ok(),
            ColumnTarget::Horizon(horizon) => {
                let undo_actions: Vec<UndoAction> = ids
                    .iter()
                    .filter_map(|id| self.tasks.iter().find(|t| t.id == *id))
                    .map(|t| UndoAction::RestoreHorizon {
                        id: t.id,
                        before: t.horizon,
                        after: Some(horizon),
                    })
                    .collect();
                if self.store.rollover_horizon_tasks(ids, horizon).is_ok() {
                    if undo_actions.len() == 1 {
                        if let Some(action) = undo_actions.into_iter().next() {
                            self.push_undo(UndoEntry::Single(action));
                        }
                    } else if !undo_actions.is_empty() {
                        self.push_undo(UndoEntry::Batch(undo_actions));
                    }
                    true
                } else {
                    false
                }
            }
            ColumnTarget::Inbox => false,
        };
        if changed {
            data_changed(cx.entity_id(), cx);
            self.reload(cx);
        }
    }

    fn assign_drawer_destination(
        &mut self,
        task_id: Id,
        dest: Option<ColumnTarget>,
        cx: &mut Context<Self>,
    ) {
        let Some(task) = self.tasks.iter().find(|t| t.id == task_id).cloned() else {
            return;
        };
        let changed = match dest {
            Some(ColumnTarget::Day(day)) => self.store.set_due(task_id, Some(day), task.due_time).is_ok(),
            Some(ColumnTarget::Horizon(horizon)) => {
                if self.store.set_horizon(task_id, Some(horizon)).is_ok() {
                    if task.due_date.is_none() {
                        self.push_undo(UndoEntry::Single(UndoAction::RestoreHorizon {
                            id: task_id,
                            before: task.horizon,
                            after: Some(horizon),
                        }));
                    }
                    true
                } else {
                    false
                }
            }
            None => {
                let ok_due = self.store.set_due(task_id, None, None).is_ok();
                let ok_h = self.store.set_horizon(task_id, None).is_ok();
                if ok_due && ok_h {
                    if task.due_date.is_none() && task.horizon.is_some() {
                        self.push_undo(UndoEntry::Single(UndoAction::RestoreHorizon {
                            id: task_id,
                            before: task.horizon,
                            after: None,
                        }));
                    }
                    true
                } else {
                    false
                }
            }
            Some(ColumnTarget::Inbox) => false,
        };
        if changed {
            data_changed(cx.entity_id(), cx);
            self.reload(cx);
        }
    }

    pub(super) fn has_active_filters(&self) -> bool {
        self.filter_board.is_some()
            || self.filter_tag.is_some()
            || self.filter_priority.is_some()
    }

    pub(super) fn matches_filter(&self, task: &Task) -> bool {
        if let Some(board_id) = self.filter_board
            && task.list_id != Some(board_id)
        {
            return false;
        }
        if let Some(tag) = &self.filter_tag
            && !task.tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
        {
            return false;
        }
        if let Some(priority) = self.filter_priority
            && task.priority != priority
        {
            return false;
        }
        true
    }

    pub(super) fn filter_vec<'a>(&self, items: Vec<&'a Task>) -> Vec<&'a Task> {
        items
            .into_iter()
            .filter(|t| self.matches_filter(t))
            .collect()
    }

    pub(super) fn render_drawer(&self, task_id: Id, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let strings = i18n::t(cx);
        let task = self.tasks.iter().find(|t| t.id == task_id);
        let current_due = task.and_then(|t| t.due_date);
        let current_horizon = task.and_then(|t| t.horizon);

        let week = Horizon::current_week(today);
        let month = Horizon::current_month(today);
        let quarter = Horizon::current_quarter(today);
        let year = Horizon::current_year(today);

        let chips: Vec<(&'static str, String, Option<ColumnTarget>, bool)> = vec![
            (
                "today",
                strings.today.to_owned(),
                Some(ColumnTarget::Day(today)),
                current_due == Some(today),
            ),
            (
                "week",
                calendar::format_horizon_badge(week, strings),
                Some(ColumnTarget::Horizon(week)),
                current_horizon == Some(week),
            ),
            (
                "month",
                calendar::format_horizon_badge(month, strings),
                Some(ColumnTarget::Horizon(month)),
                current_horizon == Some(month),
            ),
            (
                "quarter",
                format!("{}{}", strings.horizons_quarter_prefix, (today.month() - 1) / 3 + 1),
                Some(ColumnTarget::Horizon(quarter)),
                current_horizon == Some(quarter),
            ),
            (
                "year",
                today.year().to_string(),
                Some(ColumnTarget::Horizon(year)),
                current_horizon == Some(year),
            ),
            (
                "life",
                strings.horizons_life.to_owned(),
                Some(ColumnTarget::Horizon(Horizon::Life)),
                current_horizon == Some(Horizon::Life),
            ),
            (
                "clear",
                strings.horizons_clear_horizon.to_owned(),
                None,
                current_due.is_none() && current_horizon.is_none(),
            ),
        ];

        let scrim_opacity = if cx.theme().is_dark() { 0.25 } else { 0.08 };

        div()
            .id("horizons-drawer-overlay")
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("horizons-drawer-scrim")
                    .debug_selector(|| "horizons-drawer-scrim".to_owned())
                    .absolute()
                    .inset_0()
                    .bg(gpui::black().opacity(scrim_opacity))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                        this.close_drawer(window, cx);
                    })),
            )
            .child(
                v_flex()
                    .id("horizons-drawer")
                    .debug_selector(|| "horizons-drawer".to_owned())
                    .occlude()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(360.))
                    .bg(colors.background)
                    .border_l_1()
                    .border_color(colors.border)
                    .shadow_xl()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        v_flex()
                            .px(px(16.))
                            .pt(layout::title_bar_height() + px(8.))
                            .pb(px(10.))
                            .border_b_1()
                            .border_color(colors.border_variant)
                            .gap(px(8.))
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        Label::new(strings.horizons_horizon_label)
                                            .size(LabelSize::XSmall)
                                            .weight(FontWeight::SEMIBOLD)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        div()
                                            .debug_selector(|| "horizons-drawer-close".to_owned())
                                            .child(
                                                IconButton::new("horizons-drawer-close", IconName::X)
                                                    .size(px(24.))
                                                    .icon_size(IconSize::Small)
                                                    .icon_color(Color::Muted)
                                                    .tooltip(Tooltip::text(strings.close))
                                                    .on_click(cx.listener(|this, _, window, cx| {
                                                        this.close_drawer(window, cx);
                                                    })),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .flex_wrap()
                                    .gap(px(5.))
                                    .children(chips.into_iter().map(|(key, label, dest, active)| {
                                        div()
                                            .id(SharedString::from(format!("horizons-chip-{key}")))
                                            .debug_selector(move || format!("horizons-chip-{key}"))
                                            .px(px(8.))
                                            .py(px(3.))
                                            .rounded(px(5.))
                                            .border_1()
                                            .cursor_pointer()
                                            .border_color(if active {
                                                colors.accent.opacity(0.6)
                                            } else {
                                                colors.border_variant
                                            })
                                            .bg(if active {
                                                colors.accent.opacity(0.12)
                                            } else {
                                                colors.panel_background.opacity(0.6)
                                            })
                                            .when(!active, |el| el.hover(|s| s.bg(colors.ghost_element_hover)))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.assign_drawer_destination(task_id, dest, cx);
                                            }))
                                            .child(
                                                Label::new(label)
                                                    .size(LabelSize::XSmall)
                                                    .weight(if active {
                                                        FontWeight::SEMIBOLD
                                                    } else {
                                                        FontWeight::NORMAL
                                                    })
                                                    .color(if active {
                                                        Color::Accent
                                                    } else {
                                                        Color::Muted
                                                    }),
                                            )
                                    })),
                            ),
                    )
                    .child(div().flex_1().min_h_0().child(self.detail.clone())),
            )
            .into_any_element()
    }

    pub(super) fn render_filter_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let strings = i18n::t(cx);
        let active_filters = self.has_active_filters();

        let boards: Vec<(Id, String, Option<Hsla>)> = self
            .lists
            .iter()
            .filter(|l| !l.archived && l.completed_at.is_none())
            .map(|l| (l.id, l.name.clone(), colors::parse_color(l.color.as_deref())))
            .collect();

        let priorities = [
            (None, strings.all),
            (Some(Priority::High), strings.priority_high),
            (Some(Priority::Medium), strings.priority_medium),
            (Some(Priority::Low), strings.priority_low),
        ];

        let content = v_flex()
            .w(px(252.))
            .gap(px(10.))
            .p(px(6.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Label::new(strings.horizons_filter_title)
                            .size(LabelSize::Small)
                            .weight(FontWeight::SEMIBOLD),
                    )
                    .when(active_filters, |el| {
                        el.child(
                            div()
                                .id("horizons-filter-reset")
                                .debug_selector(|| "horizons-filter-reset".to_owned())
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|s| s.bg(colors.ghost_element_hover))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.filter_board = None;
                                    this.filter_tag = None;
                                    this.filter_priority = None;
                                    cx.notify();
                                }))
                                .child(
                                    Label::new(strings.horizons_reset_filters)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Accent),
                                ),
                        )
                    }),
            )
            // Board / List filter.
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(
                        Label::new(strings.horizons_boards)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .child(self.render_filter_chip(
                                "board-all",
                                strings.horizons_all_boards.to_owned(),
                                None,
                                self.filter_board.is_none(),
                                |this, cx| {
                                    this.filter_board = None;
                                    cx.notify();
                                },
                                cx,
                            ))
                            .children(boards.into_iter().map(|(id, name, dot)| {
                                self.render_filter_chip(
                                    format!("board-{id}"),
                                    name,
                                    dot,
                                    self.filter_board == Some(id),
                                    move |this, cx| {
                                        this.filter_board =
                                            if this.filter_board == Some(id) { None } else { Some(id) };
                                        cx.notify();
                                    },
                                    cx,
                                )
                            })),
                    ),
            )
            // Tag filter.
            .when(!self.tags.is_empty(), |el| {
                let tags: Vec<(String, Option<Hsla>)> = self
                    .tags
                    .iter()
                    .map(|t| (t.name.clone(), colors::parse_color(t.color.as_deref())))
                    .collect();
                el.child(
                    v_flex()
                        .gap(px(4.))
                        .child(
                            Label::new(strings.tags)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap(px(4.))
                                .child(self.render_filter_chip(
                                    "tag-all",
                                    strings.horizons_all_tags.to_owned(),
                                    None,
                                    self.filter_tag.is_none(),
                                    |this, cx| {
                                        this.filter_tag = None;
                                        cx.notify();
                                    },
                                    cx,
                                ))
                                .children(tags.into_iter().map(|(name, dot)| {
                                    let active = self
                                        .filter_tag
                                        .as_deref()
                                        .is_some_and(|t| t.eq_ignore_ascii_case(&name));
                                    let id_key = format!("tag-{name}");
                                    let label = format!("#{name}");
                                    self.render_filter_chip(
                                        id_key,
                                        label,
                                        dot,
                                        active,
                                        move |this, cx| {
                                            this.filter_tag = if active { None } else { Some(name.clone()) };
                                            cx.notify();
                                        },
                                        cx,
                                    )
                                })),
                        ),
                )
            })
            // Priority filter.
            .child(
                v_flex()
                    .gap(px(4.))
                    .child(
                        Label::new(strings.filter_priority)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(h_flex().flex_wrap().gap(px(4.)).children(
                        priorities.into_iter().map(|(prio, label)| {
                            let active = self.filter_priority == prio;
                            let id_key = format!("prio-{prio:?}");
                            self.render_filter_chip(
                                id_key,
                                label.to_owned(),
                                None,
                                active,
                                move |this, cx| {
                                    this.filter_priority = prio;
                                    cx.notify();
                                },
                                cx,
                            )
                        }),
                    )),
            )
            // Show completed toggle.
            .child(div().w_full().h(px(1.)).bg(colors.border_variant))
            .child(
                h_flex()
                    .id("horizons-toggle-completed")
                    .debug_selector(|| "horizons-toggle-completed".to_owned())
                    .w_full()
                    .py(px(3.))
                    .px(px(4.))
                    .rounded(px(5.))
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.ghost_element_hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_completed = !this.show_completed;
                        this.persist_bool_setting(SETTING_SHOW_COMPLETED, this.show_completed);
                        cx.notify();
                    }))
                    .child(Label::new(strings.horizons_show_completed).size(LabelSize::Small))
                    .child(Switch::new().toggle_state(self.show_completed)),
            );

        popover(cx)
            .id("horizons-filter-popover")
            .debug_selector(|| "horizons-filter-popover".to_owned())
            .child(
                v_scroll("horizons-filter-scroll", div().id("horizons-filter-body").child(content))
                    .compact()
                    .max_h(px(420.)),
            )
            .into_any_element()
    }

    fn render_filter_chip(
        &self,
        id_suffix: impl Into<String>,
        label: String,
        dot: Option<Hsla>,
        active: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let suffix = id_suffix.into();
        let selector = format!("horizons-filter-{suffix}");

        h_flex()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .px(px(7.))
            .py(px(3.))
            .rounded(px(5.))
            .gap(px(4.))
            .border_1()
            .cursor_pointer()
            .border_color(if active {
                colors.accent.opacity(0.6)
            } else {
                colors.border_variant
            })
            .bg(if active {
                colors.accent.opacity(0.12)
            } else {
                colors.surface_background.opacity(0.6)
            })
            .when(!active, |el| el.hover(|s| s.bg(colors.ghost_element_hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                on_click(this, cx);
            }))
            .when_some(dot, |el, c| el.child(div().size(px(6.)).rounded_full().bg(c)))
            .child(
                Label::new(label)
                    .size(LabelSize::XSmall)
                    .color(if active { Color::Accent } else { Color::Muted }),
            )
            .into_any_element()
    }

    pub(super) fn render_options_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let strings = i18n::t(cx);

        let content = v_flex()
            .w(px(260.))
            .gap(px(8.))
            .p(px(6.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Label::new(strings.horizons_columns)
                    .size(LabelSize::XSmall)
                    .weight(FontWeight::SEMIBOLD)
                    .color(Color::Muted),
            )
            .child(
                v_flex().gap(px(2.)).children(HorizonSlot::ALL.into_iter().map(|slot| {
                    let visible = !self.collapsed_cols.contains(&slot);
                    let slot_key = slot.key();
                    h_flex()
                        .id(SharedString::from(format!("horizons-opt-col-{slot_key}")))
                        .debug_selector(move || format!("horizons-opt-col-{slot_key}"))
                        .w_full()
                        .px(px(6.))
                        .py(px(4.))
                        .rounded(px(5.))
                        .items_center()
                        .justify_between()
                        .cursor_pointer()
                        .hover(|s| s.bg(colors.ghost_element_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_slot_collapsed(slot, cx);
                        }))
                        .child(Label::new(slot.full_label(strings)).size(LabelSize::Small))
                        .child(
                            Icon::new(IconName::Check)
                                .size(IconSize::Small)
                                .color(if visible {
                                    colors.accent
                                } else {
                                    gpui::transparent_black()
                                }),
                        )
                })),
            )
            .child(div().w_full().h(px(1.)).bg(colors.border_variant))
            .child(
                h_flex()
                    .id("horizons-opt-dated-week")
                    .debug_selector(|| "horizons-opt-dated-week".to_owned())
                    .w_full()
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.ghost_element_hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_dated_in_week = !this.show_dated_in_week;
                        this.persist_bool_setting(
                            SETTING_SHOW_DATED_IN_WEEK,
                            this.show_dated_in_week,
                        );
                        cx.notify();
                    }))
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(strings.horizons_show_dated_in_week).size(LabelSize::Small),
                        ),
                    )
                    .child(Switch::new().toggle_state(self.show_dated_in_week)),
            )
            .child(
                h_flex()
                    .id("horizons-opt-prompt")
                    .debug_selector(|| "horizons-opt-prompt".to_owned())
                    .w_full()
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.ghost_element_hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_prompt = !this.show_prompt;
                        this.persist_bool_setting(SETTING_SHOW_PROMPT, this.show_prompt);
                        cx.notify();
                    }))
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(strings.horizons_show_prompt).size(LabelSize::Small),
                        ),
                    )
                    .child(Switch::new().toggle_state(self.show_prompt)),
            );

        popover(cx)
            .id("horizons-options-popover")
            .debug_selector(|| "horizons-options-popover".to_owned())
            .child(content)
            .into_any_element()
    }

    pub(super) fn render_reflection_prompt(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let strings = i18n::t(cx);
        let locale = i18n::locale(cx);
        let prompt_text = prompts::prompt_for(locale, self.prompt_index);

        h_flex()
            .flex_none()
            .w_full()
            .justify_end()
            .px_4()
            .py_1()
            .border_t_1()
            .border_color(colors.border_variant)
            .child(
                div()
                    .id("horizons-prompt")
                    .debug_selector(|| "horizons-prompt".to_owned())
                    .max_w_full()
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(6.))
                    .text_size(px(11.))
                    .line_height(px(15.5))
                    .text_color(colors.text_muted.opacity(0.72))
                    .cursor_pointer()
                    .hover(|s| s.text_color(colors.text).bg(colors.ghost_element_hover.opacity(0.75)))
                    .tooltip(Tooltip::text(strings.horizons_prompt_hint))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.prompt_index = (this.prompt_index + 1) % prompts::PROMPT_COUNT;
                        cx.notify();
                    }))
                    .child(prompt_text),
            )
            .into_any_element()
    }
}
