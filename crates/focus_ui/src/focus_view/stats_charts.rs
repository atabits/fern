//! Chart primitives and focus-details breakdown for FocusView statistics.

use fern_core::stats::{self, Bucket};
use ui::Donut;

use super::stats::{CARD_HEIGHT, StatsPage};
use super::*;

/// Bars for the statistics charts, with a label under each; `highlight` marks one bar.
pub(super) fn bar_chart(
    id: &'static str,
    values: &[i64],
    labels: Vec<String>,
    tooltips: Vec<String>,
    highlight: Option<usize>,
    height: f32,
    cx: &App,
) -> Div {
    let colors = cx.theme().colors();
    let top = values.iter().copied().max().unwrap_or(0).max(1) as f32;
    v_flex()
        .gap_1()
        // As the original's: a thin bar over a grey track the chart's full height.
        .child(h_flex().h(px(height)).gap_1().children(values.iter().enumerate().map(|(i, v)| {
            div()
                .id(SharedString::from(format!("{id}-{i}")))
                .flex_1()
                .h_full()
                .flex()
                .justify_center()
                .rounded(px(4.))
                .hover(|s| s.bg(colors.element_hover.opacity(0.4)))
                .when_some(tooltips.get(i).cloned(), |el, tip| el.tooltip(Tooltip::text(tip)))
                .child(
                    div().w(px(6.)).h_full().flex().flex_col().justify_end().rounded(px(3.)).bg(colors.element_hover).child(
                        div()
                            .w_full()
                            .h(px((height * *v as f32 / top).max(if *v > 0 { 2. } else { 0. })))
                            .rounded(px(3.))
                            .bg(if highlight == Some(i) || highlight.is_none() { colors.accent } else { colors.accent.opacity(0.6) }),
                    ),
                )
        })))
        .child(h_flex().gap_1().children(labels.into_iter().map(|l| {
            div().flex_1().flex().justify_center().child(Label::new(l).size(LabelSize::XSmall).color(Color::Muted))
        })))
}

/// A smooth line with a shaded area under it, scaled between the values' minimum and maximum,
/// as in the original's "My Achievement Score".
/// `from_zero` starts the scale at zero (counts); otherwise at the lowest value (the score).
pub(super) fn line_chart(
    id: &'static str,
    values: &[i64],
    labels: Vec<String>,
    tooltips: Vec<String>,
    height: f32,
    from_zero: bool,
    cx: &App,
) -> Div {
    let colors = cx.theme().colors();
    let (accent, dot) = (colors.accent, colors.text);
    let low = if from_zero { 0 } else { values.iter().copied().min().unwrap_or(0) };
    // The original's axis: five whole steps from the lowest value over the highest.
    let (step, high) = fern_core::stats::chart_scale(low, values.iter().copied().max().unwrap_or(0));
    let values = values.to_vec();
    let axis = |v: i64| Label::new(v.to_string()).size(LabelSize::XSmall).color(Color::Muted);
    let plot = gpui::canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let n = values.len().max(1) as f32;
            let (left, top) = (bounds.origin.x, bounds.origin.y);
            let (width, chart) = (bounds.size.width, bounds.size.height - px(8.));
            let points: Vec<gpui::Point<Pixels>> = values
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let share = (*v - low) as f32 / (high - low) as f32;
                    gpui::point(left + width * ((i as f32 + 0.5) / n), top + px(4.) + chart * (1. - share))
                })
                .collect();
            let Some((first, last)) = points.first().zip(points.last()) else { return };
            let curve = |builder: &mut gpui::PathBuilder| {
                for pair in points.windows(2) {
                    let middle = (pair[0].x + pair[1].x) / 2.;
                    builder.cubic_bezier_to(pair[1], gpui::point(middle, pair[0].y), gpui::point(middle, pair[1].y));
                }
            };
            let bottom = top + bounds.size.height;
            let mut area = gpui::PathBuilder::fill();
            area.move_to(gpui::point(first.x, bottom));
            area.line_to(*first);
            curve(&mut area);
            area.line_to(gpui::point(last.x, bottom));
            area.close();
            if let Ok(path) = area.build() {
                window.paint_path(path, accent.opacity(0.3));
            }
            let mut line = gpui::PathBuilder::stroke(px(2.));
            line.move_to(*first);
            curve(&mut line);
            if let Ok(path) = line.build() {
                window.paint_path(path, accent);
            }
            for p in &points {
                let size = px(6.);
                window.paint_quad(
                    gpui::fill(gpui::Bounds::new(gpui::point(p.x - size / 2., p.y - size / 2.), gpui::size(size, size)), dot)
                        .corner_radii(size / 2.),
                );
            }
        },
    )
    .w_full()
    .h(px(height));
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .items_start()
                .gap_2()
                .child(v_flex().h(px(height)).justify_between().children((0..=5).rev().map(|i| axis(low + step * i))))
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .child(plot)
                        .child(h_flex().absolute().inset_0().children(tooltips.into_iter().enumerate().map(|(i, tip)| {
                            div()
                                .id(SharedString::from(format!("{id}-{i}")))
                                .flex_1()
                                .h_full()
                                .rounded(px(4.))
                                .hover(|s| s.bg(colors.element_hover.opacity(0.3)))
                                .tooltip(Tooltip::text(tip))
                        }))),
                ),
        )
        .child(h_flex().pl(px(36.)).children(labels.into_iter().map(|l| {
            div().flex_1().flex().justify_center().child(Label::new(l).size(LabelSize::XSmall).color(Color::Muted))
        })))
}

/// A change against the period before, as the original's: the amount, then a green arrow up, or a
/// red one down when it fell.
pub(super) fn change_line(text: String, delta: i64) -> impl IntoElement {
    h_flex()
        .gap_0p5()
        .child(Label::new(text).size(LabelSize::XSmall).color(Color::Muted))
        .child(Icon::new(if delta < 0 { IconName::ArrowDown } else { IconName::ArrowUp }).size(IconSize::XSmall).color(if delta < 0 { Color::Error } else { Color::Success }))
}

impl FocusView {
    /// The Focus tab's "Details": focused time of a day, week or month per list, tag or task, as
    /// a ring chart with a legend.
    pub(super) fn render_focus_details(&self, page: StatsPage, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let status = theme.status().clone();
        let tasks = &self.stats_data.tasks;
        let week_start = crate::calendar::week_start();
        let period = page.details_period;
        let (first, last) = stats::buckets(period, page.details_day, week_start, 1)[0];
        let totals = fern_core::focus::focus_by(&self.sessions, tasks, page.details_by, first, last);
        let palette = [theme.colors().accent, status.success, status.warning, status.error, theme.colors().text_muted];
        let slices: Vec<(SharedString, i64, Hsla)> = totals
            .into_iter()
            .enumerate()
            .map(|(i, (key, minutes))| {
                let fallback = palette[i % palette.len()];
                let (name, color): (SharedString, Hsla) = match key {
                    fern_core::focus::FocusKey::List(None) => (strings.inbox.into(), fallback),
                    fern_core::focus::FocusKey::List(Some(id)) => {
                        let list = self.stats_data.list_lookup.get(&id);
                        (
                            list.map_or(SharedString::default(), |(name, _)| name.clone()),
                            list.and_then(|(_, color)| *color).unwrap_or(fallback),
                        )
                    }
                    fern_core::focus::FocusKey::Tag(None) => (strings.group_no_tags.into(), fallback),
                    fern_core::focus::FocusKey::Tag(Some(tag)) => {
                        let color = self.stats_data.tag_colors.get(tag.as_str()).copied().unwrap_or(fallback);
                        (format!("#{tag}").into(), color)
                    }
                    fern_core::focus::FocusKey::Task(title) => (title.into(), fallback),
                };
                (name, minutes, color)
            })
            .collect();
        // A session with several tags counts once in the middle.
        let total: i64 = stats::focus_in(&self.sessions, &[(first, last)])[0].1;
        let current = stats::buckets(period, today, week_start, 1)[0] == (first, last);
        let label = match period {
            Bucket::Day if current => strings.today.to_owned(),
            Bucket::Week if current => strings.stats_this_week.to_owned(),
            Bucket::Month if current => strings.stats_this_month.to_owned(),
            Bucket::Day => crate::task_rows::short_date(first, strings),
            Bucket::Week => format!("{} - {}", crate::task_rows::short_date(first, strings), crate::task_rows::short_date(last, strings)),
            Bucket::Month => format!("{} {}", strings.months_short[first.month0() as usize], first.year()),
        };
        let by_names = [strings.stats_by_list_option, strings.stats_by_tag_option, strings.stats_by_task_option];
        let period_names = [strings.stats_day, strings.stats_week, strings.stats_month];
        let period_index = Bucket::ALL.iter().position(|b| *b == period).unwrap_or(0);
        type StatsOption = (&'static str, bool, Box<dyn Fn(&mut StatsPage)>);
        let pill = |id: &'static str, menu: u8, text: &'static str, options: Vec<StatsOption>, cx: &mut Context<Self>| {
            let open = page.details_menu == Some(menu);
            let options = Rc::new(options);
            let dropdown = open.then(|| {
                PopoverAttachment::new(
                    Placement::BelowRight,
                    popover(cx)
                        .min_w(px(140.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.update_stats(cx, |p| p.details_menu = None)))
                        .children((0..options.len()).map(|i| {
                            let (name, selected, _) = &options[i];
                            let options = options.clone();
                            ListItem::new(SharedString::from(format!("{id}-{i}")))
                                .child(Label::new(*name).color(if *selected { Color::Accent } else { Color::Default }))
                                .when(*selected, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_stats(cx, |p| {
                                        (options[i].2)(p);
                                        p.details_menu = None;
                                    })
                                }))
                        })),
                )
            });
            h_flex()
                .id(id)
                .relative()
                .px_2()
                .h(px(27.))
                .gap_0p5()
                .rounded(px(14.))
                .border_1()
                .border_color(if open { theme.colors().accent } else { theme.colors().border })
                .hover(|s| s.bg(theme.colors().ghost_element_hover))
                .cursor_pointer()
                .child(Label::new(text).size(LabelSize::Small))
                .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                .on_click(cx.listener(move |this, _, _, cx| this.update_stats(cx, |p| p.details_menu = (!open).then_some(menu))))
                .children(dropdown)
        };
        let by_options = (0..3u8)
            .map(|by| {
                let set: Box<dyn Fn(&mut StatsPage)> = Box::new(move |p| p.details_by = by);
                (by_names[by as usize], by == page.details_by, set)
            })
            .collect();
        let period_options = Bucket::ALL
            .into_iter()
            .zip(period_names)
            .map(|(bucket, name)| {
                let set: Box<dyn Fn(&mut StatsPage)> = Box::new(move |p| p.details_period = bucket);
                (name, bucket == period, set)
            })
            .collect();
        let step = move |day: NaiveDate, forward: bool| {
            let (first, last) = stats::buckets(period, day, week_start, 1)[0];
            if forward { last + chrono::Duration::days(1) } else { first - chrono::Duration::days(1) }
        };
        v_flex()
            .min_h(px(CARD_HEIGHT))
            .p_4()
            .gap_2()
            .rounded(px(10.))
            .bg(theme.colors().surface_background)
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Label::new(strings.stats_details).weight(FontWeight::SEMIBOLD)))
                    .child(pill("details-by", 0, by_names[page.details_by as usize], by_options, cx))
                    .child(pill("details-period", 1, period_names[period_index], period_options, cx))
                    .child(
                        IconButton::new("details-back", IconName::ChevronLeft)
                            .size(px(22.))
                            .tooltip(Tooltip::text("←"))
                            .on_click(cx.listener(move |this, _, _, cx| this.update_stats(cx, |p| p.details_day = step(p.details_day, false)))),
                    )
                    .child(Label::new(label).size(LabelSize::Small))
                    .child(
                        IconButton::new("details-forward", IconName::ChevronRight)
                            .size(px(22.))
                            .tooltip(Tooltip::text("→"))
                            .on_click(cx.listener(move |this, _, _, cx| this.update_stats(cx, |p| p.details_day = step(p.details_day, true)))),
                    ),
            )
            .child(
                h_flex()
                    .py_2()
                    .gap_6()
                    .justify_center()
                    .child(
                        Donut::new(slices.iter().map(|(_, m, color)| (*m as f32, *color))).size(px(216.)).thickness(px(14.)).center(
                            v_flex()
                                .items_center()
                                .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(if total == 0 {
                                    strings.stats_no_data.to_owned()
                                } else {
                                    format_minutes(total, strings)
                                }))
                                .when(total > 0, |el| el.child(Label::new(strings.focus).size(LabelSize::XSmall).color(Color::Muted))),
                        ),
                    )
                    .child(v_flex().gap_1().children(slices.into_iter().map(|(name, minutes, color)| {
                        h_flex()
                            .gap_2()
                            .child(div().size(px(8.)).rounded_full().bg(color))
                            .child(Label::new(format_minutes(minutes, strings)).size(LabelSize::Small))
                            .child(Label::new(name).size(LabelSize::Small).color(Color::Muted).truncate())
                    }))),
            )
            .into_any_element()
    }
}
