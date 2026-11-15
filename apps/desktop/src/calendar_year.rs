//! The calendar's year overview and its day popovers.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use chrono::{Datelike, NaiveDate};
use gpui::{
    Bounds, ClickEvent, Entity, Focusable, FontWeight, ScrollWheelEvent, Size, TouchPhase, canvas,
};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use super::{CalendarMode, CalendarView, calendar, i18n};
use crate::drag::DraggedTask;
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};

pub(super) struct YearJump {
    input: Entity<TextInput>,
    invalid: bool,
}

pub(super) const YEAR_RANGE: std::ops::RangeInclusive<i32> = 1910..=2099;

pub(super) fn clamp_year_day(day: NaiveDate) -> NaiveDate {
    day.clamp(
        NaiveDate::from_ymd_opt(*YEAR_RANGE.start(), 1, 1).unwrap(),
        NaiveDate::from_ymd_opt(*YEAR_RANGE.end(), 12, 31).unwrap(),
    )
}

const YEAR_SCROLL_DURATION: Duration = Duration::from_millis(280);
const YEAR_SCROLL_SETTLE: Duration = Duration::from_millis(120);

#[derive(Default)]
pub(super) struct YearScroll {
    transition: Option<YearTransition>,
    precise_position: Option<f32>,
    settle: Option<gpui::Task<()>>,
}

struct YearTransition {
    // Positions are in year-page units, relative to YEAR_RANGE.start(). Retargeting samples
    // the current position, so a second wheel event never restarts at a row boundary.
    from: f32,
    to: f32,
    started: Instant,
}

impl YearTransition {
    fn position(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        let progress = (elapsed / YEAR_SCROLL_DURATION.as_secs_f32()).min(1.);
        let eased = 1. - (1. - progress).powi(3);
        self.from + (self.to - self.from) * eased
    }

    fn finished(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= YEAR_SCROLL_DURATION
    }
}

struct YearLayout {
    columns: usize,
    height: Pixels,
    year_heading_height: Pixels,
    row_stride: Pixels,
    padding: Pixels,
    column_gap: Pixels,
    row_gap: Pixels,
    heading_height: Pixels,
    heading_font: Pixels,
    day_font: Pixels,
    marker_size: Pixels,
}

impl YearLayout {
    fn new(size: Size<Pixels>, week_numbers: bool) -> Self {
        let width = f32::from(size.width);
        let height = f32::from(size.height);
        let columns = if width >= height { 4 } else { 3 };
        let rows = (12 / columns) as f32;
        let padding = (width * 0.035).clamp(8., 52.);
        let column_gap = (width * 0.03).clamp(8., 52.);
        let row_gap = (height / rows * 0.12).clamp(4., 24.);
        let month_height = (height - 16. - (rows - 1.) * row_gap) / rows;
        let heading_height = (month_height * 0.15).clamp(13., 29.);
        let month_width =
            (width - 2. * padding - (columns - 1) as f32 * column_gap) / columns as f32;
        let day_width = month_width / if week_numbers { 8. } else { 7. };
        let day_height = (month_height - heading_height) / 7.;
        let cell = day_width.min(day_height);
        Self {
            columns,
            height: size.height,
            year_heading_height: px((month_height * 0.2).clamp(20., 36.)),
            row_stride: px(month_height + row_gap),
            padding: px(padding),
            column_gap: px(column_gap),
            row_gap: px(row_gap),
            heading_height: px(heading_height),
            heading_font: px((heading_height * 0.7).clamp(10., 18.)),
            day_font: px((cell * 0.79).clamp(8., 14.)),
            marker_size: px(cell.clamp(8., 28.)),
        }
    }

    fn page_height(&self) -> Pixels {
        self.height + self.year_heading_height
    }

    fn position(&self, month: NaiveDate) -> f32 {
        let row = month.month0() as usize / self.columns;
        (month.year() - YEAR_RANGE.start()) as f32
            + f32::from(self.year_heading_height + self.row_stride * row as f32)
                / f32::from(self.page_height())
    }

    fn closest_month(&self, position: f32) -> NaiveDate {
        let center_year = position.floor() as i32 + YEAR_RANGE.start();
        let last_month = NaiveDate::from_ymd_opt(*YEAR_RANGE.end(), 1, 1).unwrap();
        ((center_year - 1)..=(center_year + 1))
            .filter(|year| YEAR_RANGE.contains(year))
            .flat_map(|year| {
                (1..=12)
                    .step_by(self.columns)
                    .map(move |month| NaiveDate::from_ymd_opt(year, month as u32, 1).unwrap())
            })
            .filter(|month| *month <= last_month)
            .min_by(|a, b| {
                (self.position(*a) - position)
                    .abs()
                    .total_cmp(&(self.position(*b) - position).abs())
            })
            .unwrap_or(last_month)
    }
}

fn dates_between(from: NaiveDate, through: NaiveDate) -> impl Iterator<Item = NaiveDate> {
    from.iter_days().take_while(move |date| *date <= through)
}

impl CalendarView {
    fn begin_year_transition(&mut self, target: NaiveDate, cx: &App) {
        let precise_position = self.year_scroll.precise_position.take();
        self.year_scroll.settle = None;
        let Some(size) = self.year_size.filter(|_| !cx.reduce_motion()) else {
            self.year_scroll.transition = None;
            return;
        };
        let layout = YearLayout::new(size, calendar::week_numbers());
        let now = cx.background_executor().now();
        let from = precise_position.unwrap_or_else(|| {
            self.year_scroll.transition.as_ref().map_or_else(
                || layout.position(self.month),
                |motion| motion.position(now),
            )
        });
        let to = layout.position(target);
        self.year_scroll.transition = (from != to).then_some(YearTransition {
            from,
            to,
            started: now,
        });
    }

    pub(super) fn navigate_year(&mut self, target: NaiveDate, cx: &mut Context<Self>) {
        let target = target.clamp(
            NaiveDate::from_ymd_opt(*YEAR_RANGE.start(), 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(*YEAR_RANGE.end(), 1, 1).unwrap(),
        );
        if target == self.month && self.year_scroll.precise_position.is_none() {
            return;
        }
        self.begin_year_transition(target, cx);
        self.month = target;
        self.year_day = None;
        self.card = None;
        self.open_event = None;
        cx.notify();
    }

    pub(super) fn shift_year_using_keys(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.mode == CalendarMode::Year
            && self.year_day.is_none()
            && self.options_dialog.is_none()
            && !self.mode_menu_open
            && !self.more_menu
        {
            self.shift(delta, cx);
        } else {
            cx.propagate();
        }
    }

    fn scroll_year(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(16.));
        if delta.y == px(0.)
            || delta.x.abs() > delta.y.abs()
            || matches!(event.touch_phase, TouchPhase::Ended | TouchPhase::Cancelled)
        {
            return;
        }
        cx.stop_propagation();
        let Some(size) = self.year_size else { return };
        let layout = YearLayout::new(size, calendar::week_numbers());
        if event.delta.precise() {
            let now = cx.background_executor().now();
            let current = self
                .year_scroll
                .precise_position
                .or_else(|| {
                    self.year_scroll
                        .transition
                        .as_ref()
                        .map(|motion| motion.position(now))
                })
                .unwrap_or_else(|| layout.position(self.month));
            let first = NaiveDate::from_ymd_opt(*YEAR_RANGE.start(), 1, 1).unwrap();
            let last = NaiveDate::from_ymd_opt(*YEAR_RANGE.end(), 1, 1).unwrap();
            let position = (current - delta.y / layout.page_height())
                .clamp(layout.position(first), layout.position(last));
            self.year_scroll.precise_position = Some(position);
            self.year_scroll.transition = None;
            self.month = layout.closest_month(position);
            self.year_day = None;
            self.open_event = None;
            self.close_card(cx);
            // Some platforms provide only Moved events. Wait briefly for any further input
            // (including momentum) before aligning to the nearest row.
            self.year_scroll.settle = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(YEAR_SCROLL_SETTLE).await;
                let _ = this.update(cx, |this, cx| this.navigate_year(this.month, cx));
            }));
            cx.notify();
        } else {
            let columns = layout.columns as i32;
            let delta = if delta.y < px(0.) { columns } else { -columns };
            self.navigate_year(calendar::add_months(self.month, delta), cx);
        }
    }

    fn jump_from_year(&mut self, date: NaiveDate, mode: CalendarMode, cx: &mut Context<Self>) {
        self.month = calendar::month_start(date);
        self.anchor = date;
        self.open_event = None;
        self.set_mode(mode, cx);
    }

    fn year_counts(&self, year: i32, today: NaiveDate, cx: &App) -> BTreeMap<NaiveDate, usize> {
        let first = NaiveDate::from_ymd_opt(year, 1, 1).unwrap();
        let last = NaiveDate::from_ymd_opt(year, 12, 31).unwrap();
        let mut counts: BTreeMap<_, _> = if self.options.show_countdowns {
            fern_core::agenda::agenda(&[], &self.countdowns, first, last)
                .into_iter()
                .map(|(date, items)| (date, items.len()))
                .collect()
        } else {
            BTreeMap::new()
        };
        for task in &self.tasks {
            let Some(start) = task
                .due_date
                .filter(|date| *date <= last && self.options.shows_task(task, *date))
            else {
                continue;
            };
            let end = task.end_date.unwrap_or(start).max(start);
            for date in dates_between(start.max(first), end.min(last)) {
                *counts.entry(date).or_default() += 1;
            }
            // Advance each recurring task once, and count dates within its initial span only once.
            if let Some(repeat) = task
                .repeat
                .as_ref()
                .filter(|_| self.options.show_repeats && !task.is_closed() && end < last)
            {
                use fern_core::Frequency;
                let interval = i64::from(repeat.interval.max(1));
                let step = match repeat.frequency {
                    Frequency::Daily if !repeat.skip_weekends => Some(interval),
                    Frequency::Weekly if repeat.weekdays == 0 => Some(7 * interval),
                    _ => None,
                };
                // Simple daily and weekly rules have a fixed stride. Advance directly to
                // just before the year instead of counting years of earlier occurrences.
                let mut date = if let Some(step) = step.filter(|_| !repeat.is_specific_dates()) {
                    let elapsed = (first.max(end + chrono::Duration::days(1)) - start)
                        .num_days()
                        .saturating_sub(1)
                        .max(0);
                    start + chrono::Duration::days(elapsed / step * step)
                } else {
                    start
                };
                loop {
                    let next = repeat.next_after(date);
                    if next <= date || next > last {
                        break;
                    }
                    if next >= first && next > end {
                        *counts.entry(next).or_default() += 1;
                    }
                    date = next;
                }
            }
        }
        for event in self.events_between(first, last, cx) {
            for date in dates_between(event.start.max(first), event.end.min(last)) {
                *counts.entry(date).or_default() += 1;
            }
        }
        for session in &self.focus_sessions {
            if let Some(start) = chrono::DateTime::from_timestamp_millis(session.started_at) {
                let date = start.with_timezone(&chrono::Local).date_naive();
                if (first..=last).contains(&date) {
                    *counts.entry(date).or_default() += 1;
                }
            }
        }
        if today.year() == year {
            *counts.entry(today).or_default() += self.habits_today.len();
        }
        counts
    }

    fn ensure_year_heatmap(&mut self, year: i32, today: NaiveDate, cx: &App) {
        if self.year_heatmap_day != today {
            self.year_heatmap.clear();
            self.year_heatmap_day = today;
        }
        if self.options.heatmap && !self.year_heatmap.contains_key(&year) {
            let counts = self.year_counts(year, today, cx);
            self.year_heatmap.insert(year, counts);
        }
    }

    pub(super) fn open_year_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.more_menu = false;
        self.year_day = None;
        self.open_event = None;
        self.close_card(cx);
        let hint = i18n::t(cx).calendar_year_date_hint;
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, hint, cx));
        cx.subscribe_in(&input, window, |this, _, Submitted(text), window, cx| {
            this.submit_year_jump(text, window, cx);
        })
        .detach();
        cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
            this.close_year_jump(window, cx);
        })
        .detach();
        cx.subscribe(&input, |this, _, _: &Changed, cx| {
            if let Some(jump) = &mut this.year_jump {
                jump.invalid = false;
                cx.notify();
            }
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.year_jump = Some(YearJump {
            input,
            invalid: false,
        });
        cx.notify();
    }

    pub(super) fn close_year_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(jump) = self.year_jump.take() {
            if jump.input.focus_handle(cx).is_focused(window) {
                window.focus(&self.focus_handle, cx);
            }
            cx.notify();
        }
    }

    fn submit_year_jump(&mut self, input: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = input.trim();
        let date = NaiveDate::parse_from_str(input, "%Y-%m-%d")
            .ok()
            .or_else(|| NaiveDate::from_ymd_opt(input.parse().ok()?, 1, 1))
            .filter(|date| YEAR_RANGE.contains(&date.year()));
        if let Some(date) = date {
            self.close_year_jump(window, cx);
            self.jump_from_year(date, CalendarMode::Year, cx);
        } else if let Some(jump) = &mut self.year_jump {
            jump.invalid = true;
            cx.notify();
        }
    }

    pub(super) fn render_year_jump(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let jump = self.year_jump.as_ref()?;
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();
        Some(
            popover(cx)
                .debug_selector(|| "calendar-year-jump".into())
                .w(px(240.))
                .p_3()
                .gap_2()
                .on_mouse_down_out(
                    cx.listener(|this, _, window, cx| this.close_year_jump(window, cx)),
                )
                .child(Label::new(strings.calendar_go_to_date).weight(FontWeight::SEMIBOLD))
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .border_1()
                        .border_color(colors.border)
                        .rounded(px(4.))
                        .child(jump.input.clone()),
                )
                .when(jump.invalid, |el| {
                    el.child(Label::new(strings.calendar_invalid_year_date).color(Color::Error))
                })
                .child(
                    ListItem::new("calendar-year-jump-confirm")
                        .child(Label::new(strings.ok))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(jump) = &this.year_jump {
                                let text = jump.input.read(cx).text().to_owned();
                                this.submit_year_jump(&text, window, cx);
                            }
                        })),
                )
                .into_any_element(),
        )
    }

    pub(super) fn print_year(&mut self, cx: &mut Context<Self>) {
        self.more_menu = false;
        let today = chrono::Local::now().date_naive();
        let year = self.month.year();
        self.ensure_year_heatmap(year, today, cx);
        let empty = BTreeMap::new();
        let counts = self.year_heatmap.get(&year).unwrap_or(&empty);
        let html =
            crate::print::year_html(year, today, counts, calendar::week_numbers(), i18n::t(cx));
        if let Err(error) = crate::print::open(&format!("year-{year}"), &html) {
            eprintln!("failed to print the calendar year: {error:#}");
        }
        cx.notify();
    }

    pub(super) fn render_year(
        &mut self,
        today: NaiveDate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let size = self.year_size.unwrap_or_else(|| window.viewport_size());
        let layout = YearLayout::new(size, calendar::week_numbers());
        let now = cx.background_executor().now();
        if self
            .year_scroll
            .transition
            .as_ref()
            .is_some_and(|motion| motion.finished(now))
            || cx.reduce_motion()
        {
            self.year_scroll.transition = None;
        }
        let position = if let Some(position) = self.year_scroll.precise_position {
            position
        } else if let Some(motion) = &self.year_scroll.transition {
            window.request_animation_frame();
            motion.position(now)
        } else {
            layout.position(self.month)
        };
        let previous_size = self.year_size;
        let view = cx.entity().downgrade();
        // Measure the actual calendar, including the space taken by either sidebar.
        let measure = canvas(
            move |bounds, _, cx| {
                if previous_size != Some(bounds.size) {
                    let view = view.clone();
                    cx.defer(move |cx| {
                        view.update(cx, |this, cx| {
                            if this.year_size != Some(bounds.size) {
                                this.year_size = Some(bounds.size);
                                this.year_scroll = YearScroll::default();
                                // An orientation change changes how many months a row holds.
                                let columns =
                                    YearLayout::new(bounds.size, calendar::week_numbers()).columns;
                                let month =
                                    (this.month.month0() as usize / columns * columns + 1) as u32;
                                this.month = this.month.with_month(month).unwrap();
                                cx.notify();
                            }
                        })
                        .ok();
                    });
                }
            },
            |_, _, _, _| {},
        );
        let first = position.floor() as i32 + YEAR_RANGE.start();
        let last = (position + f32::from(size.height) / f32::from(layout.page_height())).ceil()
            as i32
            + YEAR_RANGE.start()
            - 1;
        let visible_years = first.max(*YEAR_RANGE.start())..=last.min(*YEAR_RANGE.end());
        for year in visible_years.clone() {
            self.ensure_year_heatmap(year, today, cx);
        }
        v_flex()
            .id("calendar-year")
            .debug_selector(|| "calendar-year".into())
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(Self::scroll_year))
            .child(measure.absolute().inset_0())
            // Like the web's virtual month strip, only the pages intersecting the viewport
            // exist. Their size stays fixed while their common vertical offset changes.
            .children(visible_years.map(|year| {
                let top = layout.page_height() * ((year - YEAR_RANGE.start()) as f32 - position);
                v_flex()
                    .id(("calendar-year-page", year as usize))
                    .debug_selector(move || format!("year-page-{year}"))
                    .absolute()
                    .top(top)
                    .left_0()
                    .w_full()
                    .h(layout.page_height())
                    .child(
                        h_flex()
                            .h(layout.year_heading_height)
                            .flex_none()
                            .px(layout.padding)
                            .text_size(layout.heading_font * 1.2)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(year.to_string()),
                    )
                    .child(self.render_year_page(year, today, &layout, cx))
            }))
    }

    fn render_year_page(
        &self,
        year: i32,
        today: NaiveDate,
        layout: &YearLayout,
        cx: &mut Context<Self>,
    ) -> Div {
        let empty = BTreeMap::new();
        let counts = self.year_heatmap.get(&year).unwrap_or(&empty);
        let mut months = (1..=12).map(|month| NaiveDate::from_ymd_opt(year, month, 1).unwrap());
        let rows = (0..12 / layout.columns)
            .map(|_| {
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .gap(layout.column_gap)
                    .children(
                        months
                            .by_ref()
                            .take(layout.columns)
                            .map(|month| self.render_year_month(month, today, counts, layout, cx)),
                    )
            })
            .collect::<Vec<_>>();
        v_flex()
            .h(layout.height)
            .flex_none()
            .px(layout.padding)
            .py(px(8.))
            .gap(layout.row_gap)
            .children(rows)
    }

    fn render_year_month(
        &self,
        month: NaiveDate,
        today: NaiveDate,
        counts: &BTreeMap<NaiveDate, usize>,
        layout: &YearLayout,
        cx: &mut Context<Self>,
    ) -> Div {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();
        let week_numbers = calendar::week_numbers();
        let cell = |label: String, color| {
            h_flex()
                .flex_1()
                .min_w_0()
                .justify_center()
                .text_color(color)
                .child(label)
        };
        let secondary = colors.text.opacity(0.65);
        v_flex()
            .debug_selector(move || format!("year-month-{}-{}", month.year(), month.month()))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .text_size(layout.day_font)
            .line_height(layout.day_font * 1.15)
            .child(
                h_flex().h(layout.heading_height).flex_none().child(
                    h_flex()
                        .id(("year-month", month.month() as usize))
                        .flex_none()
                        .ml(px(-8.))
                        .px(px(8.))
                        .py(px(4.))
                        .text_size(layout.heading_font)
                        .line_height(layout.heading_font * 1.25)
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text)
                        .rounded(px(10.))
                        .cursor_pointer()
                        .hover(|style| style.bg(colors.text.opacity(0.06)))
                        .debug_selector(move || {
                            format!("year-heading-{}-{}", month.year(), month.month())
                        })
                        .child(strings.months[month.month0() as usize])
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.jump_from_year(month, CalendarMode::Month, cx)
                        })),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(week_numbers, |el| el.child(div().flex_1().min_w_0()))
                    .children(
                        calendar::weekday_labels(&strings.weekdays_year)
                            .map(|label| cell(label.to_owned(), secondary)),
                    ),
            )
            .children(calendar::month_grid(month).chunks_exact(7).map(|week| {
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .when(week_numbers, |el| {
                        el.child(cell(
                            calendar::week_number(week.try_into().unwrap()).to_string(),
                            secondary,
                        ))
                    })
                    .children(
                        week.iter().map(|&date| {
                            self.render_year_date(month, date, today, counts, layout, cx)
                        }),
                    )
            }))
    }

    fn render_year_date(
        &self,
        month: NaiveDate,
        date: NaiveDate,
        today: NaiveDate,
        counts: &BTreeMap<NaiveDate, usize>,
        layout: &YearLayout,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let colors = cx.theme().colors();
        let in_month = calendar::month_start(date) == month;
        let is_today = date == today && in_month;
        let is_open = self.year_day == Some((month, date));
        let accent = colors.accent;
        let level = if in_month {
            calendar::heatmap_level(counts.get(&date).copied().unwrap_or(0))
        } else {
            0
        };
        let anchor_bounds = Rc::new(Cell::new(None));
        let heatmap = self.options.heatmap;
        // The web's four heat levels use different opacities in light and dark themes.
        let heat_opacity = if cx.theme().is_light() {
            [0., 0.16, 0.3, 0.5, 0.7][usize::from(level)]
        } else {
            f32::from(level) * 0.2
        };
        let background = if is_open {
            colors.element_selected
        } else if is_today {
            accent
        } else if heatmap && level > 0 {
            colors.background.blend(accent.opacity(heat_opacity))
        } else {
            colors.background
        };
        // Hover is a light overlay: it must not erase a busy day's tint or today's marker.
        let hovered = background.blend(colors.text.opacity(if level > 0 { 0.03 } else { 0.05 }));
        h_flex()
            .id(SharedString::from(format!("year-{month}-{date}")))
            .debug_selector(|| format!("year-date-{month}-{date}"))
            .relative()
            .group("year-date")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .justify_center()
            .m(px(0.5))
            .cursor_pointer()
            .when(heatmap, |el| {
                el.when(is_open || is_today || level > 0, |el| el.bg(background))
                    .hover(move |style| style.bg(hovered))
            })
            .drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.3)))
            .on_drop(cx.listener(move |this, dragged: &DraggedTask, window, cx| {
                let id = if window.modifiers().secondary() {
                    this.store.duplicate_task(dragged.id).map(|copy| copy.id)
                } else {
                    Ok(dragged.id)
                };
                if let Err(error) = id.and_then(|id| this.store.move_dates(id, date)) {
                    eprintln!("failed to schedule the dropped task: {error:#}");
                }
                this.reload(cx);
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    this.jump_from_year(date, CalendarMode::Week, cx);
                } else {
                    this.close_card(cx);
                    this.open_event = None;
                    this.year_day = (this.year_day != Some((month, date))).then_some((month, date));
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .justify_center()
                    .size(layout.marker_size)
                    .rounded_full()
                    .when(is_today, |el| el.text_color(gpui::white()))
                    .when(!heatmap, |el| {
                        el.when(is_today || is_open, |el| el.bg(background))
                            .group_hover("year-date", move |style| style.bg(hovered))
                    })
                    .when(!in_month, |el| el.text_color(colors.text.opacity(0.65)))
                    .child(date.day().to_string()),
            )
            .when(is_open, |el| {
                let bounds = anchor_bounds.clone();
                el.child(
                    canvas(move |area, _, _| bounds.set(Some(area)), |_, _, _, _| {})
                        .absolute()
                        .inset_0(),
                )
                .child(PopoverAttachment::new(
                    Placement::RightOf,
                    self.render_year_card(date, anchor_bounds, cx),
                ))
            })
    }

    fn render_year_card(
        &self,
        date: NaiveDate,
        anchor: Rc<Cell<Option<Bounds<Pixels>>>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let tasks = self.tasks_on(date);
        let events = self.events_on(date, cx);
        let extras = self.day_extras(date, cx);
        let empty = tasks.is_empty() && events.is_empty() && extras.is_empty();
        popover(cx)
            .id("year-day-popover")
            .debug_selector(|| "year-day-card".into())
            .w(px(328.))
            .p_3()
            .gap_1p5()
            .on_mouse_down_out(
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    if !anchor
                        .get()
                        .is_some_and(|bounds| bounds.contains(&event.position))
                        && this.card.is_none()
                        && this.open_event.is_none()
                    {
                        this.year_day = None;
                        cx.notify();
                    }
                }),
            )
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(Label::new(calendar::format_date(date, strings)).weight(FontWeight::SEMIBOLD))
            .when(empty, |el| {
                el.child(
                    Label::new(strings.no_tasks_scheduled)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
            .child(
                v_flex()
                    .id("year-day-items")
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .gap_0p5()
                    .children(
                        tasks
                            .into_iter()
                            .map(|task| self.task_bar(task, Some(date), cx).h(px(22.))),
                    )
                    .children(
                        events
                            .into_iter()
                            .map(|event| self.event_bar(event, date, cx)),
                    )
                    .children(extras),
            )
            .into_any_element()
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use fern_core::Store;
    use gpui::{Modifiers, ScrollDelta, TestAppContext, point, size};

    #[gpui::test]
    fn year_heatmap_counts_tasks_repeats_and_spans_with_the_current_filters(
        cx: &mut TestAppContext,
    ) {
        use fern_core::{Frequency, Recurrence};
        let store = Rc::new(Store::open_in_memory().unwrap());
        let day = |month, day| NaiveDate::from_ymd_opt(2032, month, day).unwrap();
        let single = store.add_task("One day").unwrap();
        store.set_due(single.id, Some(day(5, 13)), None).unwrap();
        let repeat = store.add_task("Monthly").unwrap();
        let start = NaiveDate::from_ymd_opt(2031, 12, 13).unwrap();
        store.set_due(repeat.id, Some(start), None).unwrap();
        store
            .set_repeat(
                repeat.id,
                Some(Recurrence::new(Frequency::Monthly, 1)),
                start,
            )
            .unwrap();
        let span = store.add_task("Across two months").unwrap();
        store.set_due(span.id, Some(day(2, 28)), None).unwrap();
        store.set_due_end(span.id, Some((day(3, 1), None))).unwrap();
        let completed = store.add_task("Completed").unwrap();
        store.set_due(completed.id, Some(day(5, 24)), None).unwrap();
        store.set_completed(completed.id, true).unwrap();
        cx.update(theme::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store.clone(), window, cx);
            view.month = day(1, 1);
            view.set_mode(CalendarMode::Year, cx);
            view
        });
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(
                view.options.heatmap,
                "scheduled days are marked without changing settings"
            );
            let counts = view.year_counts(2032, day(1, 1), cx);
            assert_eq!(counts.get(&day(5, 13)), Some(&2));
            assert_eq!(counts.get(&day(12, 13)), Some(&1));
            assert_eq!(counts.get(&day(2, 28)), Some(&1));
            assert_eq!(counts.get(&day(2, 29)), Some(&1));
            assert_eq!(counts.get(&day(3, 1)), Some(&1));
            assert_eq!(counts.get(&day(5, 24)), Some(&1));
            assert!(!counts.contains_key(&day(5, 25)));
        });
        store.set_due(single.id, Some(day(5, 25)), None).unwrap();
        let mut options = store.calendar_options().unwrap();
        options.show_completed = false;
        options.show_repeats = false;
        store.save_calendar_options(&options).unwrap();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.reload(cx);
                let counts = view.year_counts(2032, day(1, 1), cx);
                assert!(!counts.contains_key(&day(5, 13)));
                assert!(!counts.contains_key(&day(5, 24)));
                assert_eq!(counts.get(&day(5, 25)), Some(&1));
            })
        });
    }

    #[gpui::test]
    fn year_scroll_moves_both_years_and_can_reverse_without_jumping(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let day = NaiveDate::from_ymd_opt(2033, 1, 15).unwrap();
        let task = store.add_task("Incoming year").unwrap();
        store.set_due(task.id, Some(day), None).unwrap();
        cx.update(theme::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store, window, cx);
            view.set_mode(CalendarMode::Year, cx);
            view.month = NaiveDate::from_ymd_opt(2032, 9, 1).unwrap();
            view.reload(cx);
            view
        });
        cx.simulate_resize(size(px(1000.), px(650.)));
        cx.run_until_parked();
        let grid = cx.debug_bounds("calendar-year").unwrap();
        let origin = cx.debug_bounds("year-month-2032-9").unwrap().top();
        let wheel = |amount| ScrollWheelEvent {
            position: grid.center(),
            delta: ScrollDelta::Lines(point(0., amount)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        };
        cx.simulate_mouse_move(grid.center(), None, Modifiers::default());
        cx.simulate_event(wheel(-1.));
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.month, NaiveDate::from_ymd_opt(2033, 1, 1).unwrap());
            assert!(view.year_scroll.transition.is_some());
        });
        // A real intermediate frame, with both years in the clipped strip.
        cx.executor().advance_clock(Duration::from_millis(80));
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        assert!(cx.debug_bounds("year-month-2032-9").unwrap().top() < origin);
        let incoming = cx.debug_bounds("year-month-2033-1").unwrap().top();
        assert!(incoming > origin && incoming < grid.bottom());
        cx.simulate_event(wheel(1.));
        assert!((cx.debug_bounds("year-month-2033-1").unwrap().top() - incoming).abs() < px(1.));
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        assert!((cx.debug_bounds("year-month-2032-9").unwrap().top() - origin).abs() < px(0.1));
        cx.update(|_, cx| assert!(view.read(cx).year_scroll.transition.is_none()));

        cx.simulate_event(wheel(-1.));
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        assert!((cx.debug_bounds("year-month-2033-1").unwrap().top() - origin).abs() < px(0.1));
        let date = cx
            .debug_bounds("year-date-2033-01-01-2033-01-15")
            .unwrap()
            .center();
        cx.simulate_mouse_move(date, None, Modifiers::default());
        cx.simulate_click(date, Modifiers::default());
        assert!(cx.debug_bounds("year-day-card").is_some());
        assert!(
            cx.debug_bounds(format!("calendar-task-{}", task.id).leak())
                .is_some()
        );
    }

    #[gpui::test]
    fn year_touchpad_scroll_tracks_pixels_and_snaps_to_a_row(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let january = NaiveDate::from_ymd_opt(2032, 1, 1).unwrap();
        let may = NaiveDate::from_ymd_opt(2032, 5, 1).unwrap();
        cx.update(theme::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store, window, cx);
            view.month = january;
            view.set_mode(CalendarMode::Year, cx);
            view
        });
        cx.simulate_resize(size(px(1000.), px(650.)));
        let grid = cx.debug_bounds("calendar-year").unwrap();
        let origin = cx.debug_bounds("year-month-2032-1").unwrap().top();
        let modifiers = Modifiers::default();
        let scroll = |pixels, touch_phase| ScrollWheelEvent {
            position: grid.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(pixels))),
            modifiers,
            touch_phase,
        };
        cx.simulate_mouse_move(grid.center(), None, modifiers);
        cx.simulate_event(scroll(-20., TouchPhase::Started));
        let moved = cx.debug_bounds("year-month-2032-1").unwrap().top();
        assert!((moved - origin + px(20.)).abs() < px(0.1));
        cx.simulate_event(scroll(-125., TouchPhase::Moved));
        cx.simulate_event(scroll(0., TouchPhase::Ended));
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.month, may);
            assert!(view.year_scroll.precise_position.is_some());
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(YEAR_SCROLL_SETTLE + Duration::from_millis(1));
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(view.year_scroll.precise_position.is_none());
            assert!(view.year_scroll.transition.is_some());
        });
        cx.executor()
            .advance_clock(YEAR_SCROLL_DURATION + Duration::from_millis(1));
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        let aligned = cx.debug_bounds("year-month-2032-5").unwrap().top();
        assert!(
            (aligned - origin).abs() < px(1.),
            "{aligned:?} should align with {origin:?}"
        );
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(view.year_scroll.precise_position.is_none());
            assert!(view.year_scroll.transition.is_none());
        });
    }

    #[gpui::test]
    fn year_view_keeps_rendering_when_dates_cross_its_limits(cx: &mut TestAppContext) {
        let day = |year, month, day| NaiveDate::from_ymd_opt(year, month, day).unwrap();
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(theme::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store, window, cx);
            view.month = day(2099, 12, 1);
            view.shift(1, cx);
            assert_eq!(view.month, day(2100, 1, 1));
            view.set_mode(CalendarMode::Year, cx);
            assert_eq!(view.month, day(2099, 1, 1));
            assert!(!view.can_shift(1));
            view
        });
        cx.simulate_resize(size(px(1000.), px(650.)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("year-month-2099-1").is_some());
        assert!(cx.debug_bounds("year-month-2099-12").is_some());

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.month = day(1910, 1, 1);
                view.set_mode(CalendarMode::Month, cx);
                view.shift(-1, cx);
                assert_eq!(view.month, day(1909, 12, 1));
                view.set_mode(CalendarMode::Year, cx);
                assert_eq!(view.month, day(1910, 1, 1));
                assert!(!view.can_shift(-1));
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("year-month-1910-1").is_some());
        assert!(cx.debug_bounds("year-month-1910-12").is_some());
    }

    #[gpui::test]
    fn year_scroll_respects_motion_preferences_resize_and_limits(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            cx.set_reduce_motion(true);
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = CalendarView::new(store, window, cx);
            view.month = NaiveDate::from_ymd_opt(2032, 1, 1).unwrap();
            view.set_mode(CalendarMode::Year, cx);
            view
        });
        cx.simulate_resize(size(px(1000.), px(650.)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.navigate_year(NaiveDate::from_ymd_opt(2032, 5, 1).unwrap(), cx);
                assert!(view.year_scroll.transition.is_none());
                cx.set_reduce_motion(false);
                view.navigate_year(NaiveDate::from_ymd_opt(2032, 9, 1).unwrap(), cx);
                assert!(view.year_scroll.transition.is_some());
            })
        });
        cx.simulate_resize(size(px(600.), px(900.)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(view.year_scroll.transition.is_none());
            assert_eq!(view.month, NaiveDate::from_ymd_opt(2032, 7, 1).unwrap());
        });
        let grid = cx.debug_bounds("calendar-year").unwrap();
        assert!(
            (cx.debug_bounds("year-month-2032-7").unwrap().top() - grid.top() - px(8.)).abs()
                < px(0.1)
        );
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                cx.set_reduce_motion(true);
                view.navigate_year(NaiveDate::from_ymd_opt(2100, 4, 1).unwrap(), cx);
                assert_eq!(view.month, NaiveDate::from_ymd_opt(2099, 1, 1).unwrap());
                view.navigate_year(NaiveDate::from_ymd_opt(1909, 1, 1).unwrap(), cx);
                assert_eq!(view.month, NaiveDate::from_ymd_opt(1910, 1, 1).unwrap());
            })
        });
    }
}
