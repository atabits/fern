use chrono::{Datelike, Local, NaiveDate, NaiveTime, Timelike};
use fern_core::{Id, Task};
use gpui::FontWeight;
use ui::prelude::*;

use crate::calendar::{self, week_axis};
use crate::drag::{DraggedEvent, DraggedTask, ResizeEdge, ResizedTask};
use crate::i18n;

use super::{
    CalendarView, EventBar,
};

pub(super) fn dual_tz_reference_day(
    week: &[NaiveDate],
    selected_day: Option<NaiveDate>,
    anchor: NaiveDate,
    today: NaiveDate,
) -> NaiveDate {
    selected_day
        .filter(|d| week.contains(d))
        .or_else(|| week.contains(&today).then_some(today))
        .or_else(|| week.contains(&anchor).then_some(anchor))
        .or_else(|| week.first().copied())
        .unwrap_or(today)
}

const MAX_DAY_MINUTES: i32 = 23 * 60 + 59;
const MIN_TIMED_RESIZE_MINUTES: i32 = 15;

pub(super) fn time_to_minutes(time: NaiveTime) -> i32 {
    (time.hour() * 60 + time.minute()) as i32
}

pub(super) fn minutes_to_time(minutes: i32) -> NaiveTime {
    let clamped = minutes.clamp(0, MAX_DAY_MINUTES) as u32;
    NaiveTime::from_hms_opt(clamped / 60, clamped % 60, 0).unwrap_or(NaiveTime::MIN)
}

pub(super) fn task_timed_resize_edges(task: &Task, date: NaiveDate) -> (bool, bool) {
    let Some(start_date) = task.due_date else {
        return (false, false);
    };
    let end_date = task.end_date.unwrap_or(start_date).max(start_date);
    (date == start_date, date == end_date)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TimedInterval {
    pub(super) start: f32,
    pub(super) end: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LanePlacement {
    pub(super) lane: usize,
    pub(super) lane_count: usize,
}

/// Greedy interval-graph lane assignment per connected overlap cluster.
/// Adjacent intervals touching at a boundary (`a.end <= b.start`) do not overlap.
pub(super) fn assign_timed_lanes(intervals: &[TimedInterval]) -> Vec<LanePlacement> {
    const EPS: f32 = 1e-3;
    let n = intervals.len();
    if n == 0 {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let ia = intervals[a];
        let ib = intervals[b];
        ia.start
            .total_cmp(&ib.start)
            .then_with(|| ib.end.total_cmp(&ia.end))
            .then_with(|| a.cmp(&b))
    });

    let mut result = vec![LanePlacement { lane: 0, lane_count: 1 }; n];
    let mut cluster: Vec<usize> = Vec::new();
    let mut cluster_end = f32::NEG_INFINITY;

    let finalize_cluster = |cluster: &[usize], result: &mut [LanePlacement]| {
        if cluster.is_empty() {
            return;
        }
        let mut lane_ends: Vec<f32> = Vec::new();
        let mut assigned: Vec<(usize, usize)> = Vec::with_capacity(cluster.len());
        for &idx in cluster {
            let item = intervals[idx];
            let item_end = item.end.max(item.start);
            let lane = match lane_ends.iter().position(|&end| end <= item.start + EPS) {
                Some(lane) => {
                    lane_ends[lane] = item_end;
                    lane
                }
                None => {
                    let lane = lane_ends.len();
                    lane_ends.push(item_end);
                    lane
                }
            };
            assigned.push((idx, lane));
        }
        let lane_count = lane_ends.len().max(1);
        for (idx, lane) in assigned {
            result[idx] = LanePlacement { lane, lane_count };
        }
    };

    for idx in order {
        let item = intervals[idx];
        let item_end = item.end.max(item.start);
        if !cluster.is_empty() && item.start >= cluster_end - EPS {
            finalize_cluster(&cluster, &mut result);
            cluster.clear();
            cluster_end = f32::NEG_INFINITY;
        }
        cluster.push(idx);
        cluster_end = cluster_end.max(item_end);
    }
    finalize_cluster(&cluster, &mut result);
    result
}

#[derive(Clone, Debug)]
pub(super) struct TimedResize {
    pub(super) id: Id,
    pub(super) date: NaiveDate,
    pub(super) edge: ResizeEdge,
    pub(super) orig_start: NaiveTime,
    pub(super) orig_end: NaiveTime,
    pub(super) end_date: Option<NaiveDate>,
    pub(super) start_y: Pixels,
    pub(super) current_time: NaiveTime,
}

impl TimedResize {
    pub(super) fn snapped_range(&self) -> (NaiveTime, NaiveTime) {
        let start_mins = time_to_minutes(self.orig_start);
        let end_mins = time_to_minutes(self.orig_end);
        let cur_mins = time_to_minutes(self.current_time);
        match self.edge {
            ResizeEdge::Start => {
                let limit = if self.end_date.is_some_and(|d| d > self.date) {
                    23 * 60 + 45
                } else {
                    (end_mins - MIN_TIMED_RESIZE_MINUTES).max(0)
                };
                let next_start = cur_mins.clamp(0, limit);
                (minutes_to_time(next_start), minutes_to_time(end_mins))
            }
            ResizeEdge::End => {
                let min_end = (start_mins + MIN_TIMED_RESIZE_MINUTES).min(MAX_DAY_MINUTES);
                let next_end = cur_mins.clamp(min_end, MAX_DAY_MINUTES);
                (minutes_to_time(start_mins), minutes_to_time(next_end))
            }
        }
    }
}

impl CalendarView {
    pub(super) fn render_week(&self, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let week = self.shown_days();
        let secondary_tz = self
            .options
            .secondary_timezone
            .as_deref()
            .and_then(calendar::canonical_timezone_name);
        let axis_ref_day = dual_tz_reference_day(&week, self.selected_day, self.anchor, today);
        let ref_tz_diff = secondary_tz.and_then(|tz| calendar::timezone_offset_diff_minutes(axis_ref_day, &Local, tz));
        let has_dst_transition = secondary_tz.is_some_and(|tz| {
            week.iter()
                .filter_map(|&d| calendar::timezone_offset_diff_minutes(d, &Local, tz))
                .any(|diff| Some(diff) != ref_tz_diff)
        });
        let gutter = if secondary_tz.is_some() { px(108.) } else { px(56.) };
        let line = theme.colors().border_variant;
        let buckets = week
            .first()
            .copied()
            .zip(week.last().copied())
            .map(|(first, last)| self.bucket_range(first, last, today, cx));

        // All-day strip: day numbers and tasks without a time.
        let all_day = h_flex()
            .flex_none()
            .items_start()
            .min_h(px(64.))
            .border_b_1()
            .border_color(line)
            .child(
                div().w(gutter).flex_none().when_some(secondary_tz, |el, tz| {
                    let tz_short = tz.rsplit('/').next().unwrap_or(tz).replace('_', " ");
                    el.flex().items_end().h(px(26.)).px_1().gap_1().child(
                        div().w(px(50.)).child(Label::new(tz_short).size(LabelSize::XSmall).color(Color::Muted).truncate()),
                    ).child(
                        div().flex_1().flex().justify_end().child(Label::new(strings.timezone_local).size(LabelSize::XSmall).color(Color::Muted).truncate()),
                    )
                }),
            )
            .children(week.iter().copied().map(|date| {
                let accent = theme.colors().accent;
                let day_tasks = buckets
                    .as_ref()
                    .and_then(|b| b.tasks.get(&date))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let day_events = buckets
                    .as_ref()
                    .and_then(|b| b.events.get(&date))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let day_extras = buckets
                    .as_ref()
                    .map(|b| self.day_extras_from_buckets(b, date, today, cx))
                    .unwrap_or_default();
                let mut tasks: Vec<AnyElement> = day_tasks
                    .iter()
                    .filter(|t| t.due_time.is_none())
                    .map(|t| self.task_bar_with_resize(t, Some(date), true, cx).into_any_element())
                    .collect();
                tasks.extend(
                    day_events
                        .iter()
                        .filter(|e| e.start_time.is_none())
                        .cloned()
                        .map(|e| self.event_bar(e, date, cx).into_any_element()),
                );
                tasks.extend(day_extras);
                v_flex()
                    .id(SharedString::from(format!("week-allday-{date}")))
                    .debug_selector(move || format!("week-allday-{date}"))
                    .flex_1()
                    .min_w_0()
                    .self_stretch()
                    .gap(px(2.))
                    .px(px(2.))
                    .pb_1()
                    .border_l_1()
                    .border_color(line)
                    .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.12)))
                    .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on_day(dragged.id, date, cx)))
                    .drag_over::<ResizedTask>(move |style, _, _, _| style.bg(accent.opacity(0.14)))
                    .on_drop(cx.listener(move |this, resized: &ResizedTask, _, cx| this.resize_task_to_day(resized, date, cx)))
                    .drag_over::<DraggedEvent>(move |style, _, _, _| style.bg(accent.opacity(0.12)))
                    .on_drop(cx.listener(move |this, event: &DraggedEvent, _, cx| this.move_event_to_day(event, date, cx)))
                    .child(
                        h_flex()
                            .h(px(26.))
                            .px_1()
                            .gap_1()
                            .child(
                                div()
                                    .id(SharedString::from(format!("calendar-select-day-{date}")))
                                    .min_w(px(22.))
                                    .h(px(22.))
                                    .px_1()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .when(date == today, |el| el.bg(theme.colors().accent))
                                    .when(date != today && self.selected_day == Some(date), |el| {
                                        el.bg(theme.colors().element_selected)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| this.select_day(date, cx)))
                                    .child(Label::new(date.day().to_string()).centered().weight(FontWeight::SEMIBOLD)
                                        .when(date == today, |label| label.color(Color::OnAccent))),
                            )
                            .when_some(
                                has_dst_transition.then_some(secondary_tz).flatten(),
                                |el, tz| {
                                    let shifted = calendar::timezone_offset_diff_minutes(date, &Local, tz) != ref_tz_diff;
                                    el.child(
                                        Label::new(calendar::timezone_offset_label_on(date, tz))
                                            .size(LabelSize::XSmall)
                                            .color(if shifted { Color::Accent } else { Color::Muted })
                                            .truncate(),
                                    )
                                },
                            ),
                    )
                    .children(tasks)
            }));

        // Time axis labels and hour lines.
        let hour_label = |hour: u32| -> SharedString {
            if hour == 12 { strings.noon.into() } else { calendar::format_hour(hour, strings).into() }
        };
        let tz_hour_label = |hour: u32, tz: &str| -> String {
            let (h, m, delta) = week_axis::hour_in_timezone(axis_ref_day, hour, tz);
            let mut label = if m == 0 {
                calendar::format_hour(h, strings)
            } else {
                calendar::format_time(NaiveTime::from_hms_opt(h, m, 0).unwrap_or_default(), strings)
            };
            if delta > 0 {
                label.push_str("+1");
            } else if delta < 0 {
                label.push_str("-1");
            }
            label
        };
        let now = Local::now().time();
        let now_color = theme.status().error;
        let now_label_y = (week.contains(&today) && week_axis::unfolded(now))
            .then(|| week_axis::offset(now) - 7.);
        let mut axis = v_flex().w(gutter).flex_none().child(
            div()
                .h(px(week_axis::FOLDED_HEIGHT))
                .pb_1()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.colors().panel_background)
                .border_b_1()
                .border_color(theme.colors().border_variant)
                .flex_col()
                .child(Label::new(calendar::format_hour(0, strings)).size(LabelSize::XSmall).color(Color::Muted))
                .child(Label::new(format!("– {}", calendar::format_hour(7, strings))).size(LabelSize::XSmall).color(Color::Muted)),
        );
        for hour in week_axis::FIRST_HOUR..week_axis::LAST_HOUR {
            let hour_y = week_axis::offset(NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or_default()) + 2.;
            let collides_with_now = now_label_y.is_some_and(|now_y| (hour_y - now_y).abs() < 16.);
            axis = axis.child(
                h_flex()
                    .h(px(week_axis::HOUR_HEIGHT))
                    .px_1()
                    .pt(px(2.))
                    .items_start()
                    .when_some(secondary_tz, |el, tz| {
                        el.child(
                            div()
                                .w(px(50.))
                                .flex()
                                .justify_end()
                                .pr_1()
                                .child(Label::new(tz_hour_label(hour, tz)).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                    })
                    .child(div().flex_1().flex().justify_end().when(!collides_with_now, |el| {
                        el.child(Label::new(hour_label(hour)).size(LabelSize::XSmall).color(Color::Muted))
                    })),
            );
        }
        axis = axis.child(
            div()
                .h(px(week_axis::FOLDED_HEIGHT))
                .pt_1()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.colors().panel_background)
                .border_t_1()
                .border_color(theme.colors().border_variant)
                .flex_col()
                .child(Label::new(calendar::format_hour(21, strings)).size(LabelSize::XSmall).color(Color::Muted))
                .child(Label::new(format!("– {}", calendar::format_hour(0, strings))).size(LabelSize::XSmall).color(Color::Muted)),
        );

        let columns = week.iter().copied().map(|date| {
            let accent = theme.colors().accent;
            enum TimedColumnItem<'a> {
                Task {
                    task: &'a Task,
                    orig_from: NaiveTime,
                    orig_to: NaiveTime,
                    top: f32,
                    height: f32,
                },
                Event {
                    event: EventBar,
                    top: f32,
                    height: f32,
                },
            }

            let mut column_items: Vec<TimedColumnItem<'_>> = Vec::new();
            let mut intervals: Vec<TimedInterval> = Vec::new();

            let day_tasks = buckets
                .as_ref()
                .and_then(|b| b.tasks.get(&date))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for &t in day_tasks {
                let Some((orig_from, orig_to)) = calendar::timed_segment(t, date) else { continue };
                let preview = self.timed_resize.as_ref().filter(|r| r.id == t.id && r.date == date);
                let (from, to) = preview.map_or((orig_from, orig_to), TimedResize::snapped_range);
                let top = week_axis::offset(from);
                // A span is drawn as a block from its start to its end; a point in time as a bar.
                let height = (week_axis::offset(to) - top).max(18.);
                intervals.push(TimedInterval { start: top, end: top + height });
                column_items.push(TimedColumnItem::Task { task: t, orig_from, orig_to, top, height });
            }

            let day_events = buckets
                .as_ref()
                .and_then(|b| b.events.get(&date))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for event in day_events {
                let Some(from) = (if event.start == date { event.start_time } else { Some(NaiveTime::MIN) }) else {
                    continue;
                };
                let to = match event.end_time {
                    Some(to) if event.end == date => to,
                    _ if event.end > date => minutes_to_time(MAX_DAY_MINUTES),
                    _ => minutes_to_time(time_to_minutes(from) + 30),
                };
                let top = week_axis::offset(from);
                let height = (week_axis::offset(to) - top).max(18.);
                intervals.push(TimedInterval { start: top, end: top + height });
                column_items.push(TimedColumnItem::Event { event: event.clone(), top, height });
            }

            let placements = assign_timed_lanes(&intervals);
            let timed_blocks: Vec<AnyElement> = column_items
                .into_iter()
                .zip(placements)
                .map(|(item, placement)| {
                    let left_pct = placement.lane as f32 / placement.lane_count as f32;
                    let width_pct = 1.0 / placement.lane_count as f32;
                    let pad_left = if placement.lane == 0 { px(2.) } else { px(1.) };
                    let pad_right = if placement.lane + 1 == placement.lane_count { px(2.) } else { px(1.) };
                    match item {
                        TimedColumnItem::Task { task: t, orig_from, orig_to, top, height } => {
                            let id = t.id;
                            let end_date = t.end_date;
                            let (can_resize_start, can_resize_end) = task_timed_resize_edges(t, date);
                            div()
                                .absolute()
                                .top(px(top))
                                .left(gpui::relative(left_pct))
                                .w(gpui::relative(width_pct))
                                .h(px(height))
                                .pl(pad_left)
                                .pr(pad_right)
                                .child(
                                    div()
                                        .relative()
                                        .size_full()
                                        .child(self.task_bar(t, Some(date), cx).size_full().items_start().pt(px(1.)))
                                        .when(can_resize_start, |el| {
                                            el.child(
                                                div()
                                                    .id(SharedString::from(format!("timed-resize-top-{id}-{date}")))
                                                    .absolute()
                                                    .top_0()
                                                    .left_0()
                                                    .right_0()
                                                    .h(px(5.))
                                                    .cursor_ns_resize()
                                                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                                        cx.stop_propagation();
                                                        this.start_timed_resize(id, date, ResizeEdge::Start, orig_from, orig_to, end_date, ev.position.y, cx);
                                                    })),
                                            )
                                        })
                                        .when(can_resize_end, |el| {
                                            el.child(
                                                div()
                                                    .id(SharedString::from(format!("timed-resize-bot-{id}-{date}")))
                                                    .absolute()
                                                    .bottom_0()
                                                    .left_0()
                                                    .right_0()
                                                    .h(px(5.))
                                                    .cursor_ns_resize()
                                                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                                        cx.stop_propagation();
                                                        this.start_timed_resize(id, date, ResizeEdge::End, orig_from, orig_to, end_date, ev.position.y, cx);
                                                    })),
                                            )
                                        }),
                                )
                                .into_any_element()
                        }
                        TimedColumnItem::Event { event, top, height } => div()
                            .absolute()
                            .top(px(top))
                            .left(gpui::relative(left_pct))
                            .w(gpui::relative(width_pct))
                            .h(px(height))
                            .pl(pad_left)
                            .pr(pad_right)
                            .child(self.event_bar(event, date, cx).size_full().items_start().pt(px(1.)))
                            .into_any_element(),
                    }
                })
                .collect();
            let mut hour_lines = Vec::new();
            for hour in week_axis::FIRST_HOUR..=week_axis::LAST_HOUR {
                let time = chrono::NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or_default();
                hour_lines.push(
                    div().absolute().top(px(week_axis::offset(time))).left_0().right_0().h(px(1.)).bg(line).into_any_element(),
                );
            }
            div()
                .id(SharedString::from(format!("week-day-{date}")))
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(week_axis::total_height()))
                .border_l_1()
                .border_color(line)
                .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.08)))
                .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| this.drop_on_day(dragged.id, date, cx)))
                .drag_over::<DraggedEvent>(move |style, _, _, _| style.bg(accent.opacity(0.08)))
                .on_drop(cx.listener(move |this, event: &DraggedEvent, _, cx| this.move_event_to_day(event, date, cx)))
                .children(hour_lines)
                .children(timed_blocks)
                // The original's red line at the current time, with a dot positioned cleanly inside
                // the day column so it is never clipped or bisected by the vertical border.
                .when(date == today && week_axis::unfolded(now), |el| {
                    let top = week_axis::offset(now);
                    el.child(div().absolute().top(px(top - 1.)).left(px(5.)).right_0().h(px(2.)).bg(now_color))
                        .child(div().absolute().top(px(top - 4.)).left(px(1.)).size(px(8.)).rounded_full().bg(now_color))
                })
        });

        v_flex()
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(line)
            .child(all_day)
            .child(
                v_scroll(
                    "week-scroll-bar",
                    div()
                        .id("week-scroll")
                        .child(
                            h_flex()
                                .items_start()
                                .child(
                                    // The current time in red on the axis, as in the original.
                                    div().relative().child(axis).when_some(now_label_y, |el, top| {
                                        el.child(
                                            div()
                                                .absolute()
                                                .top(px(top))
                                                .right(px(4.))
                                                .px_0p5()
                                                .bg(theme.colors().background)
                                                .child(
                                                    Label::new(calendar::format_time(now, strings))
                                                        .size(LabelSize::XSmall)
                                                        .color(Color::Custom(now_color)),
                                                ),
                                        )
                                    }),
                                )
                                .children(columns),
                        ),
                )
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }
}
