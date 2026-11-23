//! Reusable TimeStripe-inspired column renderer, collapsed horizon strips, rollover strip,
//! task rows, and inline task creator.

use std::collections::HashMap;

use chrono::{Datelike, NaiveDate};
use desktop_common::{calendar, completion_mark};
use fern_core::{Horizon, Id, Task};
use fern_i18n as i18n;
use fern_i18n::Strings;
use gpui::{AnyElement, Context, FontWeight, Hsla, MouseButton, SharedString, div, px};
use task_ui::drag::DraggedTask;
use task_ui::task_rows;
use ui::prelude::*;
use ui::{Checkbox, Icon, IconName, IconSize, Label, LabelSize, Tooltip, v_scroll};

use crate::{HorizonsMode, HorizonsView};

/// Minimum width of an open column in panoramic `Horizons` mode:
/// `6 * 152 + 2 * 28 = 968 px`, fitting edge-to-edge on 1920 px @ 150% DPI inside `TodoView`.
pub(crate) const PANORAMIC_MIN_WIDTH: f32 = 152.;
/// Minimum width of an open column in zoomed Time Scale strip modes (`Days`..`Decades`).
pub(crate) const STRIP_MIN_WIDTH: f32 = 192.;
/// Width of a collapsed horizon vertical strip (`Q4`, `20-е`, etc.).
pub(crate) const COLLAPSED_STRIP_WIDTH: f32 = 28.;

/// The 8 slots of the panoramic `Horizons` mode, in chronological order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum HorizonSlot {
    Inbox,
    Today,
    Week,
    Month,
    Quarter,
    Year,
    Decade,
    Life,
}

impl HorizonSlot {
    pub(crate) const ALL: [Self; 8] = [
        Self::Inbox,
        Self::Today,
        Self::Week,
        Self::Month,
        Self::Quarter,
        Self::Year,
        Self::Decade,
        Self::Life,
    ];

    pub(crate) const DEFAULT_COLLAPSED: [Self; 2] = [Self::Quarter, Self::Decade];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Today => "today",
            Self::Week => "week",
            Self::Month => "month",
            Self::Quarter => "quarter",
            Self::Year => "year",
            Self::Decade => "decade",
            Self::Life => "life",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "inbox" => Some(Self::Inbox),
            "today" => Some(Self::Today),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            "quarter" => Some(Self::Quarter),
            "year" => Some(Self::Year),
            "decade" => Some(Self::Decade),
            "life" => Some(Self::Life),
            _ => None,
        }
    }

    pub(crate) fn full_label(self, strings: &Strings) -> &'static str {
        match self {
            Self::Inbox => strings.inbox,
            Self::Today => strings.today,
            Self::Week => strings.horizons_weeks,
            Self::Month => strings.horizons_months,
            Self::Quarter => strings.horizons_quarters,
            Self::Year => strings.horizons_years,
            Self::Decade => strings.horizons_decades,
            Self::Life => strings.horizons_life,
        }
    }

    pub(crate) fn target(self, today: NaiveDate) -> ColumnTarget {
        match self {
            Self::Inbox => ColumnTarget::Inbox,
            Self::Today => ColumnTarget::Day(today),
            Self::Week => ColumnTarget::Horizon(Horizon::current_week(today)),
            Self::Month => ColumnTarget::Horizon(Horizon::current_month(today)),
            Self::Quarter => ColumnTarget::Horizon(Horizon::current_quarter(today)),
            Self::Year => ColumnTarget::Horizon(Horizon::current_year(today)),
            Self::Decade => ColumnTarget::Horizon(Horizon::current_decade(today)),
            Self::Life => ColumnTarget::Horizon(Horizon::Life),
        }
    }

    /// Compact label sized to fit cleanly inside the 28 px collapsed vertical strip.
    pub(crate) fn strip_label(self, today: NaiveDate, strings: &Strings) -> String {
        match self {
            Self::Inbox => take_chars(strings.inbox, 3),
            Self::Today => take_chars(strings.today, 3),
            Self::Week => calendar::format_horizon_badge(Horizon::current_week(today), strings),
            Self::Month => calendar::format_horizon_badge(Horizon::current_month(today), strings),
            Self::Quarter => {
                let q = (today.month() - 1) / 3 + 1;
                format!("{}{q}", strings.horizons_quarter_prefix)
            }
            Self::Year => format!("'{:02}", today.year().rem_euclid(100)),
            Self::Decade => {
                let decade = today.year().div_euclid(10) * 10;
                let short_two = decade.rem_euclid(100);
                if strings.horizons_decade_suffix == "年代" {
                    format!("{short_two:02}代")
                } else {
                    format!("{short_two:02}{}", strings.horizons_decade_suffix)
                }
            }
            Self::Life => take_chars(strings.horizons_life, 3),
        }
    }

    pub(crate) fn zoom_mode(self) -> Option<HorizonsMode> {
        match self {
            Self::Inbox => Some(HorizonsMode::Inbox),
            Self::Today => Some(HorizonsMode::Days),
            Self::Week => Some(HorizonsMode::Weeks),
            Self::Month => Some(HorizonsMode::Months),
            Self::Quarter => Some(HorizonsMode::Quarters),
            Self::Year => Some(HorizonsMode::Years),
            Self::Decade => Some(HorizonsMode::Decades),
            Self::Life => None,
        }
    }
}

fn take_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Identifies a single column's semantic destination for inline task creation, rollover, and drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ColumnTarget {
    Inbox,
    Day(NaiveDate),
    Horizon(Horizon),
}

impl ColumnTarget {
    pub(crate) fn debug_key(&self) -> String {
        match *self {
            Self::Inbox => "inbox".to_owned(),
            Self::Day(day) => format!("day-{}", day.format("%Y-%m-%d")),
            Self::Horizon(h) => format!("horizon-{}", h.as_code()),
        }
    }
}

/// All precomputed data needed to render one column without touching SQLite.
pub(crate) struct ColumnSpec {
    pub(crate) target: ColumnTarget,
    pub(crate) slot: Option<HorizonSlot>,
    pub(crate) title: String,
    pub(crate) subtitle: Option<String>,
    pub(crate) is_current: bool,
    pub(crate) tasks: Vec<Task>,
    pub(crate) rollover: Vec<Task>,
    pub(crate) dated_in_week: Vec<Task>,
    pub(crate) empty_hint: Option<&'static str>,
    pub(crate) show_triage_chips: bool,
    pub(crate) show_board_badge: bool,
}

/// Formats a day's compact TimeStripe-style subtitle (e.g. `"23 ноя"`, `"Nov 23"`, `"11月23日"`).
pub(crate) fn format_day_subtitle(day: NaiveDate, today: NaiveDate, strings: &Strings) -> String {
    let base = if strings.year_first_dates {
        format!("{}{}日", strings.months_short[day.month0() as usize], day.day())
    } else if strings.day_before_month {
        calendar::format_day("{d} {mon}", day, strings)
    } else {
        calendar::format_day("{mon} {d}", day, strings)
    };
    if day.year() != today.year() {
        format!("{base} {}", day.year())
    } else {
        base
    }
}

/// Capitalizes the first Unicode scalar of a localized weekday name (e.g. Russian `"среда"` -> `"Среда"`).
pub(crate) fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Formats a task's compact time range (e.g. `"11:00 – 11:30"` or `"11:00"`).
fn format_task_time_range(task: &Task, strings: &Strings) -> Option<String> {
    let start = task.due_time?;
    let start_str = calendar::format_time(start, strings);
    match task.end_time {
        Some(end) if end != start => Some(format!("{start_str} – {}", calendar::format_time(end, strings))),
        _ => Some(start_str),
    }
}

/// Formats the origin badge for a rollover task (either its past date or its past horizon).
fn rollover_origin_badge(task: &Task, strings: &Strings) -> Option<String> {
    if let Some(horizon) = task.horizon {
        return Some(calendar::format_horizon_badge(horizon, strings));
    }
    task.due_date
        .or(task.start_date)
        .map(|day| calendar::short_date(day, strings))
}

impl HorizonsView {
    pub(crate) fn render_collapsed_strip(
        &self,
        slot: HorizonSlot,
        count: usize,
        today: NaiveDate,
        is_last: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = slot.target(today);
        let colors = cx.theme().colors();
        let strings = i18n::t(cx);
        let strip_text = slot.strip_label(today, strings);
        let tooltip_text = match target {
            ColumnTarget::Horizon(h) => {
                let (title, sub) = calendar::format_horizon_heading(h, today, strings);
                match sub {
                    Some(s) => format!("{title} · {s}"),
                    None => title,
                }
            }
            ColumnTarget::Day(day) => format!("{} · {}", strings.today, format_day_subtitle(day, today, strings)),
            ColumnTarget::Inbox => strings.inbox.to_owned(),
        };
        let slot_key = slot.key();

        div()
            .id(SharedString::from(format!("horizons-strip-{slot_key}")))
            .debug_selector(move || format!("horizons-strip-{slot_key}"))
            .w(px(COLLAPSED_STRIP_WIDTH))
            .flex_none()
            .h_full()
            .bg(colors.panel_background.opacity(0.35))
            .when(!is_last, |el| el.border_r_1().border_color(colors.border_variant.opacity(0.65)))
            .border_t_2()
            .border_color(gpui::transparent_black())
            .flex()
            .flex_col()
            .items_center()
            .pt(px(14.))
            .gap(px(7.))
            .cursor_pointer()
            .hover(|s| s.bg(colors.ghost_element_hover))
            .tooltip(Tooltip::text(tooltip_text))
            .drag_over::<DraggedTask>(|style, _, _, cx| {
                let c = cx.theme().colors();
                style
                    .bg(c.accent.opacity(0.08))
                    .border_t_2()
                    .border_color(c.accent)
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                this.handle_drop_on_column(dragged.id, target, cx);
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_slot_collapsed(slot, cx);
            }))
            .child(
                div()
                    .text_size(px(9.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_muted)
                    .child(strip_text),
            )
            .when(count > 0, |el| {
                el.child(
                    div()
                        .min_w(px(16.))
                        .px(px(3.))
                        .py(px(1.))
                        .rounded_full()
                        .bg(colors.element_hover)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(9.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(colors.text_muted)
                                .child(count.to_string()),
                        ),
                )
            })
            .child(
                Icon::new(IconName::Plus)
                    .size(IconSize::Custom(px(10.)))
                    .color(colors.text_muted.opacity(0.45)),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_column(
        &self,
        spec: ColumnSpec,
        today: NaiveDate,
        list_meta: &HashMap<Id, (String, Option<Hsla>)>,
        is_last: bool,
        min_width: f32,
        full_width: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let strings = i18n::t(cx);
        let target = spec.target;
        let col_key = target.debug_key();
        let col_key_for_selector = col_key.clone();
        let header_group = SharedString::from(format!("col-hdr-{col_key}"));

        let zoom_mode = spec.slot.and_then(HorizonSlot::zoom_mode);
        let show_current_accent = spec.is_current && spec.slot.is_none();

        let title_el = div()
            .id(SharedString::from(format!("horizons-col-title-{col_key}")))
            .debug_selector({
                let key = col_key.clone();
                move || format!("horizons-col-title-{key}")
            })
            .flex_none()
            .whitespace_nowrap()
            .text_size(px(18.5))
            .line_height(px(24.))
            .font_weight(FontWeight::BOLD)
            .text_color(colors.text)
            .when_some(zoom_mode, |el, mode| {
                el.cursor_pointer()
                    .hover(|s| s.text_color(colors.accent))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_mode(mode, cx);
                    }))
            })
            .child(spec.title);

        let header = h_flex()
            .group(header_group.clone())
            .relative()
            .w_full()
            .px(px(13.))
            .pt(px(12.))
            .pb(px(10.))
            .items_center()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_baseline()
                    .gap_1p5()
                    .when(show_current_accent, |el| {
                        el.child(
                            div()
                                .self_center()
                                .size(px(5.5))
                                .flex_none()
                                .rounded_full()
                                .bg(colors.accent),
                        )
                    })
                    .child(title_el)
                    .when_some(spec.subtitle, |el, sub| {
                        el.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.))
                                .line_height(px(16.))
                                .text_color(if show_current_accent {
                                    colors.accent.opacity(0.9)
                                } else {
                                    colors.text_muted.opacity(0.75)
                                })
                                .child(sub),
                        )
                    }),
            )
            .when_some(spec.slot, |el, slot| {
                let slot_key = slot.key();
                el.child(
                    div()
                        .id(SharedString::from(format!("horizons-collapse-{slot_key}")))
                        .debug_selector(move || format!("horizons-collapse-{slot_key}"))
                        .absolute()
                        .right(px(8.))
                        .top(px(14.))
                        .size(px(20.))
                        .rounded(px(5.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .opacity(0.)
                        .group_hover(header_group.clone(), |s| {
                            s.opacity(0.85).bg(colors.background.opacity(0.92))
                        })
                        .hover(|s| s.opacity(1.).bg(colors.ghost_element_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_slot_collapsed(slot, cx);
                        }))
                        .child(Icon::new(IconName::Minus).size(IconSize::XSmall).color(Color::Muted)),
                )
            });

        let mut body_items = div()
            .id(SharedString::from(format!("horizons-col-body-{col_key}")))
            .px(px(9.))
            .pb(px(56.))
            .flex()
            .flex_col()
            .gap(px(2.));

        // 1. Smart Rollover Strip (past unfinished tasks in current Day / Horizon column).
        if !spec.rollover.is_empty() {
            body_items = body_items.child(self.render_rollover_strip(
                &col_key,
                target,
                &spec.rollover,
                cx,
            ));
        }

        // 2. Column tasks.
        for task in &spec.tasks {
            body_items = body_items.child(self.render_task_row(
                task,
                today,
                list_meta,
                spec.show_board_badge,
                spec.show_triage_chips,
                None,
                cx,
            ));
        }

        // 3. Optional dated tasks occurring inside this ISO week.
        if !spec.dated_in_week.is_empty() {
            body_items = body_items.child(
                v_flex()
                    .mt(px(8.))
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(colors.border_variant.opacity(0.6))
                    .gap(px(2.))
                    .child(
                        div()
                            .px(px(4.))
                            .pb(px(3.))
                            .child(
                                Label::new(strings.horizons_dated_in_week)
                                    .size(LabelSize::XSmall)
                                    .weight(FontWeight::MEDIUM)
                                    .color(colors.text_muted.opacity(0.75)),
                            ),
                    )
                    .children(spec.dated_in_week.iter().map(|task| {
                        let date_chip = task
                            .due_date
                            .or(task.start_date)
                            .map(|day| calendar::short_date(day, strings));
                        self.render_task_row(
                            task,
                            today,
                            list_meta,
                            spec.show_board_badge,
                            false,
                            date_chip,
                            cx,
                        )
                    })),
            );
        }

        // 4. Empty state hint (for focused Inbox triage mode).
        if spec.tasks.is_empty()
            && spec.rollover.is_empty()
            && spec.dated_in_week.is_empty()
            && let Some(hint) = spec.empty_hint
        {
            body_items = body_items.child(
                div()
                    .px(px(4.))
                    .py(px(8.))
                    .child(
                        Label::new(hint)
                            .size(LabelSize::Small)
                            .color(colors.text_muted.opacity(0.7)),
                    ),
            );
        }

        // 5. Inline "Добавить…" creator at bottom of column.
        body_items = body_items.child(self.render_inline_add(&col_key, target, cx));

        let mut col = div()
            .id(SharedString::from(format!("horizons-col-{col_key}")))
            .debug_selector(move || format!("horizons-col-{col_key_for_selector}"))
            .h_full()
            .flex()
            .flex_col()
            .border_t_2()
            .border_color(gpui::transparent_black())
            .when(!is_last, |el| el.border_r_1().border_color(colors.border_variant.opacity(0.65)))
            .drag_over::<DraggedTask>(|style, _, _, cx| {
                let c = cx.theme().colors();
                style
                    .bg(c.accent.opacity(0.06))
                    .border_t_2()
                    .border_color(c.accent)
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                this.handle_drop_on_column(dragged.id, target, cx);
            }))
            .child(header)
            .child(
                v_scroll(SharedString::from(format!("horizons-vscroll-{col_key}")), body_items)
                    .flex_1()
                    .min_h_0(),
            );

        if full_width {
            col = col.w_full().max_w(px(720.)).mx_auto();
        } else {
            col = col.flex_1().min_w(px(min_width));
        }

        col.into_any_element()
    }

    fn render_rollover_strip(
        &self,
        col_key: &str,
        target: ColumnTarget,
        rollover: &[Task],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let strings = i18n::t(cx);
        let collapsed = self.collapsed_rollovers.contains(col_key);
        let count = rollover.len();
        let ids: Vec<Id> = rollover.iter().map(|t| t.id).collect();
        let toggle_key = col_key.to_owned();
        let col_key_owned = col_key.to_owned();
        let label_text = strings.horizons_rollover_label.replace("{n}", &count.to_string());

        // Flex-wrap on the header guarantees neither "Прошлые (N)" nor "Перенести" ever clips with "…"
        // even inside the tightest 152 px panoramic column.
        let header = h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_x(px(4.))
            .gap_y(px(2.))
            .child(
                div()
                    .id(SharedString::from(format!("rollover-toggle-{col_key}")))
                    .debug_selector({
                        let k = col_key.to_owned();
                        move || format!("rollover-toggle-{k}")
                    })
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_rollover_collapsed(&toggle_key, cx);
                    }))
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .size(IconSize::Custom(px(11.)))
                        .color(Color::Muted),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.text_muted)
                            .whitespace_nowrap()
                            .child(label_text),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("rollover-move-all-{col_key}")))
                    .debug_selector(move || format!("rollover-move-all-{col_key_owned}"))
                    .flex_none()
                    .px(px(5.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.accent.opacity(0.12)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.rollover_tasks_to_target(&ids, target, cx);
                    }))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.accent)
                            .whitespace_nowrap()
                            .child(strings.horizons_move_all_here),
                    ),
            );

        v_flex()
            .mb(px(8.))
            .p(px(6.))
            .rounded(px(7.))
            .bg(colors.panel_background.opacity(0.7))
            .border_1()
            .border_color(colors.border_variant.opacity(0.8))
            .gap(px(4.))
            .child(header)
            .when(!collapsed, |el| {
                el.children(rollover.iter().map(|task| {
                    let task_id = task.id;
                    let group = SharedString::from(format!("rollover-row-{task_id}"));
                    let origin = rollover_origin_badge(task, strings);
                    let title = if task.title.is_empty() {
                        strings.untitled.to_owned()
                    } else {
                        task.title.clone()
                    };
                    let tint = task_rows::priority_color(task.priority, &theme);

                    h_flex()
                        .id(SharedString::from(format!("rollover-item-{task_id}")))
                        .debug_selector(move || format!("rollover-item-{task_id}"))
                        .group(group.clone())
                        .w_full()
                        .items_start()
                        .gap(px(5.))
                        .px(px(2.))
                        .py(px(2.5))
                        .rounded(px(5.))
                        .cursor_pointer()
                        .hover(|s| s.bg(colors.ghost_element_hover))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_task(task_id, window, cx);
                        }))
                        .child(
                            div().flex_none().pt(px(2.)).child(
                                Checkbox::new(SharedString::from(format!("rollover-cb-{task_id}")))
                                    .size(px(13.5))
                                    .toggle_state(task.is_completed())
                                    .animate_completion(completion_mark::started(task_id, cx))
                                    .tint(tint)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_task_done(task_id, cx);
                                    })),
                            ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .w_full()
                                        .min_w_0()
                                        .text_size(px(12.))
                                        .line_height(px(16.5))
                                        .text_color(colors.text)
                                        .child(title),
                                )
                                .when_some(origin, |col, badge| {
                                    col.child(
                                        h_flex().items_center().gap(px(4.)).child(
                                            div()
                                                .px(px(4.))
                                                .rounded(px(3.))
                                                .bg(theme.status().warning.opacity(0.12))
                                                .text_size(px(10.))
                                                .line_height(px(14.))
                                                .text_color(theme.status().warning)
                                                .child(badge),
                                        ),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("rollover-move-one-{task_id}")))
                                .debug_selector(move || format!("rollover-move-one-{task_id}"))
                                .flex_none()
                                .size(px(18.))
                                .rounded(px(4.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .opacity(0.45)
                                .group_hover(group, |s| s.opacity(1.))
                                .hover(|s| s.bg(colors.accent.opacity(0.14)))
                                .tooltip(Tooltip::text(strings.horizons_move_here))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.rollover_tasks_to_target(&[task_id], target, cx);
                                }))
                                .child(
                                    Icon::new(IconName::SquareArrowRight)
                                        .size(IconSize::Custom(px(11.)))
                                        .color(Color::Accent),
                                ),
                        )
                }))
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_task_row(
        &self,
        task: &Task,
        today: NaiveDate,
        list_meta: &HashMap<Id, (String, Option<Hsla>)>,
        show_board_badge: bool,
        show_triage_chips: bool,
        extra_chip: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let strings = i18n::t(cx);

        let task_id = task.id;
        let done = task.is_completed();
        let selected = self.detail_task == Some(task_id);
        let group = SharedString::from(format!("h-task-{task_id}"));
        let tint = task_rows::priority_color(task.priority, &theme);

        let title_text = if task.title.is_empty() {
            strings.untitled.to_owned()
        } else {
            task.title.clone()
        };
        let time_range = format_task_time_range(task, strings);
        let board_badge = show_board_badge
            .then(|| task.list_id.and_then(|lid| list_meta.get(&lid).cloned()))
            .flatten();
        let has_meta = time_range.is_some()
            || task.checklist.is_some()
            || board_badge.is_some()
            || extra_chip.is_some();

        let dragged = DraggedTask {
            id: task_id,
            title: title_text.clone().into(),
        };

        let mut content_col = v_flex().flex_1().min_w_0().gap(px(2.)).child(
            h_flex()
                .w_full()
                .items_start()
                .gap(px(4.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .line_height(px(18.))
                        .text_color(if done {
                            colors.text_muted
                        } else {
                            colors.text
                        })
                        .when(done, |el| el.line_through())
                        .child(title_text),
                )
                .when(task.repeat.is_some(), |el| {
                    el.child(
                        div().pt(px(3.5)).flex_none().child(
                            Icon::new(IconName::Repeat)
                                .size(IconSize::Custom(px(10.5)))
                                .color(colors.text_muted.opacity(0.75)),
                        ),
                    )
                }),
        );

        if has_meta {
            let mut meta_row = h_flex().flex_wrap().items_center().gap(px(6.));
            if let Some(range) = time_range {
                meta_row = meta_row.child(
                    div()
                        .text_size(px(10.5))
                        .line_height(px(14.))
                        .text_color(colors.text_muted.opacity(0.8))
                        .child(range),
                );
            }
            if let Some((done_items, total_items)) = task.checklist {
                meta_row = meta_row.child(
                    h_flex()
                        .items_center()
                        .gap(px(3.))
                        .child(
                            Icon::new(IconName::ListChecks)
                                .size(IconSize::Custom(px(10.5)))
                                .color(colors.text_muted.opacity(0.75)),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(colors.text_muted.opacity(0.8))
                                .child(format!("{done_items}/{total_items}")),
                        ),
                );
            }
            if let Some((board_name, board_color)) = board_badge {
                let dot = board_color.unwrap_or_else(|| colors.text_muted.opacity(0.45));
                meta_row = meta_row.child(
                    h_flex()
                        .items_center()
                        .gap(px(3.5))
                        .max_w(px(100.))
                        .child(div().size(px(5.)).flex_none().rounded_full().bg(dot))
                        .child(
                            Label::new(board_name)
                                .size(LabelSize::XSmall)
                                .color(colors.text_muted.opacity(0.75))
                                .truncate(),
                        ),
                );
            }
            if let Some(chip_text) = extra_chip {
                meta_row = meta_row.child(
                    div()
                        .px(px(4.5))
                        .py(px(0.5))
                        .rounded(px(4.))
                        .bg(colors.element_hover.opacity(0.6))
                        .child(Label::new(chip_text).size(LabelSize::XSmall).color(Color::Muted)),
                );
            }
            content_col = content_col.child(meta_row);
        }

        // Focused Inbox triage mode: 1-click chips on hover to route directly into any horizon.
        if show_triage_chips && !done {
            let week = Horizon::current_week(today);
            let month = Horizon::current_month(today);
            let year = Horizon::current_year(today);

            let chips = [
                ("today", strings.today.to_owned(), ColumnTarget::Day(today)),
                ("week", calendar::format_horizon_badge(week, strings), ColumnTarget::Horizon(week)),
                ("month", calendar::format_horizon_badge(month, strings), ColumnTarget::Horizon(month)),
                ("year", calendar::format_horizon_badge(year, strings), ColumnTarget::Horizon(year)),
                ("life", calendar::format_horizon_badge(Horizon::Life, strings), ColumnTarget::Horizon(Horizon::Life)),
            ];

            content_col = content_col.child(
                h_flex()
                    .mt(px(4.))
                    .flex_wrap()
                    .gap(px(5.))
                    .opacity(0.4)
                    .group_hover(group.clone(), |s| s.opacity(1.))
                    .children(chips.into_iter().map(|(key, label, dest)| {
                        div()
                            .id(SharedString::from(format!("triage-{task_id}-{key}")))
                            .debug_selector(move || format!("triage-{task_id}-{key}"))
                            .px(px(7.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(colors.panel_background)
                            .border_1()
                            .border_color(colors.border_variant)
                            .cursor_pointer()
                            .hover(|s| {
                                s.bg(colors.accent.opacity(0.12))
                                    .border_color(colors.accent.opacity(0.5))
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.handle_drop_on_column(task_id, dest, cx);
                            }))
                            .child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted))
                    })),
            );
        }

        div()
            .id(SharedString::from(format!("horizons-task-{task_id}")))
            .debug_selector(move || format!("horizons-task-{task_id}"))
            .group(group)
            .w_full()
            .px(px(4.))
            .py(px(4.5))
            .rounded(px(6.))
            .flex()
            .items_start()
            .gap(px(8.))
            .cursor_pointer()
            .when(selected, |el| el.bg(colors.ghost_element_selected))
            .when(!selected, |el| el.hover(|s| s.bg(colors.ghost_element_hover)))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .on_click(cx.listener(move |this, _, window, cx| {
                if cx.has_active_drag() {
                    return;
                }
                this.open_task(task_id, window, cx);
            }))
            .child(
                div().pt(px(2.)).flex_none().child(
                    Checkbox::new(SharedString::from(format!("horizons-cb-{task_id}")))
                        .size(px(14.5))
                        .toggle_state(done)
                        .animate_completion(completion_mark::started(task_id, cx))
                        .in_progress(task.is_in_progress())
                        .tint(tint)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if cx.has_active_drag() {
                                return;
                            }
                            cx.stop_propagation();
                            this.toggle_task_done(task_id, cx);
                        })),
                ),
            )
            .child(content_col)
            .into_any_element()
    }

    fn render_inline_add(
        &self,
        col_key: &str,
        target: ColumnTarget,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let strings = i18n::t(cx);

        if let Some((active_target, input, _)) = &self.inline_add
            && *active_target == target
        {
            return h_flex()
                .id(SharedString::from(format!("inline-add-active-{col_key}")))
                .debug_selector({
                    let k = col_key.to_owned();
                    move || format!("inline-add-active-{k}")
                })
                .mt(px(2.))
                .w_full()
                .items_start()
                .gap(px(8.))
                .px(px(4.))
                .py(px(4.5))
                .rounded(px(6.))
                .bg(colors.surface_background.opacity(0.85))
                .border_1()
                .border_color(colors.accent.opacity(0.45))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .mt(px(2.))
                        .size(px(14.5))
                        .flex_none()
                        .rounded(px(3.5))
                        .border_1()
                        .border_color(colors.checkbox_border.opacity(0.45)),
                )
                .child(div().flex_1().min_w_0().text_size(px(13.)).child(input.clone()))
                .into_any_element();
        }

        h_flex()
            .id(SharedString::from(format!("inline-add-{col_key}")))
            .debug_selector({
                let k = col_key.to_owned();
                move || format!("inline-add-{k}")
            })
            .mt(px(2.))
            .w_full()
            .items_center()
            .gap(px(8.))
            .px(px(4.))
            .py(px(4.5))
            .rounded(px(6.))
            .cursor(gpui::CursorStyle::IBeam)
            .hover(|s| s.bg(colors.ghost_element_hover.opacity(0.65)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.start_inline_add(target, window, cx);
            }))
            .child(
                div()
                    .size(px(14.5))
                    .flex_none()
                    .rounded(px(3.5))
                    .border_1()
                    .border_color(colors.checkbox_border.opacity(0.32)),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .text_color(colors.text_placeholder.opacity(0.85))
                    .child(strings.horizons_add_placeholder),
            )
            .into_any_element()
    }
}
