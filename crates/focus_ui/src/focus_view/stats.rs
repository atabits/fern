//! Statistics page for FocusView: Overview, Tasks tab, Focus tab, charts, and year heatmap grid.

use fern_core::stats::{self, Bucket};
use ui::Donut;

use super::stats_charts::{bar_chart, change_line, line_chart};
use super::*;

/// All tasks, lists, tags and habits with their check-ins, for the Statistics page.
#[derive(Default)]
pub(super) struct StatsData {
    pub(super) tasks: Vec<fern_core::Task>,
    pub(super) lists: Vec<fern_core::List>,
    pub(super) list_lookup: HashMap<Id, (SharedString, Option<Hsla>)>,
    pub(super) tag_colors: HashMap<String, Hsla>,
    pub(super) all_habits: Vec<fern_core::Habit>,
    pub(super) all_habit_checkins: Vec<NaiveDate>,
    pub(super) habits: Vec<(fern_core::Habit, HashSet<NaiveDate>)>,
    pub(super) habit_records: HashMap<Id, Vec<fern_core::Checkin>>,
}

/// What the Statistics page shows: the Trends week, the Most Focused Time month and the year.
#[derive(Clone, Copy)]
pub(super) struct StatsPage {
    pub(super) tab: usize,
    pub(super) week: NaiveDate,
    pub(super) month: NaiveDate,
    pub(super) year: i32,
    /// The Task tab's day.
    pub(super) day: NaiveDate,
    /// The Overview's curves (completion, rate, pomos, focus): Day, Week or Month points.
    pub(super) curves: [Bucket; 4],
    /// The curve whose period menu is open.
    pub(super) curve_menu: Option<usize>,
    /// The Task tab's Daily / Weekly / Monthly report, and whether its menu is open.
    pub(super) task_period: Bucket,
    pub(super) task_menu: bool,
    /// Classified Completion Statistics by tag instead of by list, and whether its menu is open.
    pub(super) by_tag: bool,
    pub(super) classify_menu: bool,
    /// The Focus tab's "Details": split by list (0), tag (1) or task (2), its period and day,
    /// and which of its two menus is open.
    pub(super) details_by: u8,
    pub(super) details_period: Bucket,
    pub(super) details_day: NaiveDate,
    pub(super) details_menu: Option<u8>,
    /// The first day of the Timeline's week.
    pub(super) timeline_week: NaiveDate,
}

/// The Statistics page's cards and charts, as tall as the original's.
pub(super) const CARD_HEIGHT: f32 = 323.;
const CHART_HEIGHT: f32 = 240.;

/// Splits a template such as "{tasks} Tasks   {completed} Completed" into its pieces, each
/// marked whether it is one of the filled-in counts.
pub(super) fn summary_parts(template: &str, counts: &[(&str, String)]) -> Vec<(String, bool)> {
    let mut parts = Vec::new();
    let mut rest = template;
    while let Some((at, (key, value))) = counts.iter().filter_map(|c| rest.find(c.0).map(|at| (at, c))).min_by_key(|(at, _)| *at) {
        if at > 0 {
            parts.push((rest[..at].to_owned(), false));
        }
        parts.push((value.clone(), true));
        rest = &rest[at + key.len()..];
    }
    if !rest.is_empty() {
        parts.push((rest.to_owned(), false));
    }
    parts
}

impl FocusView {
    pub(super) fn open_stats(&mut self, cx: &mut Context<Self>) {
        if self.refresh.is_dirty() {
            self.reload(cx);
        }
        self.ensure_stats_data();
        let today = Local::now().date_naive();
        self.record_menu = None;
        self.stats_scroll = ScrollHandle::new();
        self.stats = Some(StatsPage {
            tab: 2,
            day: today,
            week: crate::calendar::week_of(today, cx)[0],
            month: crate::calendar::month_start(today),
            year: today.year(),
            curves: [Bucket::Day; 4],
            curve_menu: None,
            task_period: Bucket::Day,
            task_menu: false,
            by_tag: false,
            classify_menu: false,
            details_by: 0,
            details_period: Bucket::Day,
            details_day: today,
            details_menu: None,
            timeline_week: crate::calendar::week_of(today, cx)[0],
        });
        cx.notify();
    }

    pub(super) fn update_stats(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut StatsPage)) {
        if let Some(stats) = self.stats.as_mut() {
            let tab = stats.tab;
            change(stats);
            if tab != stats.tab {
                self.stats_scroll = ScrollHandle::new();
            }
        }
        cx.notify();
    }

    /// The original's Statistics page for focus: overview, Trends, Most Focused Time and the year grid.
    pub(super) fn render_stats(&self, page: StatsPage, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let stats = if self.session_summary.day == today {
            self.session_summary.stats
        } else {
            focus_stats(&self.sessions, today)
        };
        let (count_change, minutes_change) = fern_core::focus::change_from_yesterday(&self.sessions, today);
        let card = || v_flex().p_4().gap_2().rounded(px(10.)).bg(theme.colors().surface_background);
        let nav = |id: &'static str, label: String, back: fn(&mut StatsPage), forward: fn(&mut StatsPage), cx: &mut Context<Self>| {
            h_flex()
                .gap_1()
                .child(
                    IconButton::new(SharedString::from(format!("{id}-back")), IconName::ChevronLeft)
                        .tooltip(Tooltip::text("←"))
                        .on_click(cx.listener(move |this, _, _, cx| this.update_stats(cx, back))),
                )
                .child(Label::new(label).size(LabelSize::Small))
                .child(
                    IconButton::new(SharedString::from(format!("{id}-forward")), IconName::ChevronRight)
                        .tooltip(Tooltip::text("→"))
                        .on_click(cx.listener(move |this, _, _, cx| this.update_stats(cx, forward))),
                )
        };
        // As the original's: numbers large with a duration's unit letters small beside them.
        let overview_item = |parts: Vec<(i64, Option<&'static str>)>, label: &'static str, change: Option<(String, i64)>| {
            v_flex()
                .flex_1()
                .items_center()
                .gap_0p5()
                .child(h_flex().items_end().gap(px(6.)).text_color(theme.colors().text_accent).font_weight(FontWeight::SEMIBOLD).children(parts.into_iter().map(|(n, unit)| {
                    h_flex()
                        .items_end()
                        .child(div().text_size(px(26.)).line_height(px(32.)).child(n.to_string()))
                        .children(unit.map(|u| div().text_size(px(15.)).line_height(px(15.)).mb(px(5.)).ml_0p5().child(u)))
                })))
                .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
                .children(change.map(|(text, delta): (String, i64)| change_line(text, delta)))
        };
        let rule = || div().w(px(1.)).h(px(38.)).flex_none().bg(theme.colors().border);
        let full = |m: i64| localized_duration_parts(fern_core::focus::duration_parts_full(m), strings);
        let overview = card()
            .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(strings.focus_overview))
            .child(
                h_flex()
                    .child(overview_item(
                        vec![(stats.today_count as i64, None)],
                        strings.focus_today_pomo,
                        Some((strings.stats_from_yesterday.replace("{n}", &count_change.abs().to_string()), count_change)),
                    ))
                    .child(rule())
                    .child(overview_item(vec![(stats.total_count as i64, None)], strings.focus_total_pomo, None))
                    .child(rule())
                    .child(overview_item(
                        full(stats.today_minutes),
                        strings.focus_today_focus,
                        Some((
                            strings.stats_from_yesterday.replace(
                                "{n}",
                                &format!(
                                    "{}{} {}{}",
                                    minutes_change.abs() / 60,
                                    strings.duration_hour_short,
                                    minutes_change.abs() % 60,
                                    strings.duration_min_short,
                                ),
                            ),
                            minutes_change,
                        )),
                    ))
                    .child(rule())
                    .child(overview_item(full(stats.total_minutes), strings.focus_total_duration, None)),
            );

        // Trends: minutes per day of the chosen week.
        const CHART: f32 = 140.;
        let week = fern_core::focus::minutes_by_day(&self.sessions, page.week, 7);
        let average = week.iter().sum::<i64>() / 7;
        let week_label = format!(
            "{} - {}",
            crate::task_rows::short_date(page.week, strings),
            crate::task_rows::short_date(page.week + chrono::Duration::days(6), strings)
        );
        let bars = |values: Vec<i64>, top: f32, labels: Vec<String>, tooltips: Vec<String>, highlight: Option<usize>| {
            v_flex()
                .gap_1()
                .child(h_flex().h(px(CHART)).gap_1().children(values.iter().enumerate().map(|(i, v)| {
                    div()
                        .id(SharedString::from(format!("most-focused-bar-{i}")))
                        .flex_1()
                        .h_full()
                        .flex()
                        .flex_col()
                        .justify_end()
                        .rounded(px(3.))
                        .hover(|s| s.bg(theme.colors().element_hover.opacity(0.35)))
                        .when_some(tooltips.get(i).cloned(), |el, tip| el.tooltip(Tooltip::text(tip)))
                        .child(
                            div()
                                .w_full()
                                .h(px((CHART * *v as f32 / top).max(if *v > 0 { 2. } else { 0. })))
                                .rounded_t(px(3.))
                                .bg(if highlight == Some(i) { theme.colors().accent } else { theme.colors().accent.opacity(0.6) }),
                        )
                })))
                .child(h_flex().gap_1().children(labels.into_iter().map(|l| {
                    div().flex_1().flex().justify_center().child(Label::new(l).size(LabelSize::XSmall).color(Color::Muted))
                })))
        };
        let trends = card()
            .min_h(px(CARD_HEIGHT))
            .flex_1()
            .min_w_0()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.stats_trends).weight(FontWeight::SEMIBOLD)))
                    .child(nav(
                        "trends",
                        week_label,
                        |p| p.week -= chrono::Duration::days(7),
                        |p| p.week += chrono::Duration::days(7),
                        cx,
                    )),
            )
            .child(
                Label::new(strings.stats_daily_average.replace("{t}", &format_minutes(average, strings)))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            // As the original's, the week as a curve.
            .child({
                let trend_labels: Vec<String> = crate::calendar::weekday_labels(&strings.weekdays_short, cx).iter().map(|d| (*d).to_owned()).collect();
                let trend_tips: Vec<String> = trend_labels.iter().zip(&week).map(|(l, v)| format!("{l} · {}", format_minutes(*v, strings))).collect();
                line_chart("focus-trends", &week, trend_labels, trend_tips, CHART, true, cx)
            });

        // Timeline: the week's sessions as blocks on a day's hours, as in the original.
        const TIMELINE: f32 = 200.;
        let blocks = fern_core::focus::timeline(&self.sessions, page.timeline_week);
        let timeline_label = if page.timeline_week == crate::calendar::week_of(today, cx)[0] {
            strings.stats_this_week.to_owned()
        } else {
            format!(
                "{} - {}",
                crate::task_rows::short_date(page.timeline_week, strings),
                crate::task_rows::short_date(page.timeline_week + chrono::Duration::days(6), strings)
            )
        };
        let accent = theme.colors().accent;
        let grid_line = theme.colors().border_variant;
        let timeline = card()
            .min_h(px(CARD_HEIGHT))
            .flex_1()
            .min_w_0()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.stats_timeline).weight(FontWeight::SEMIBOLD)))
                    .child(nav(
                        "timeline",
                        timeline_label,
                        |p| p.timeline_week -= chrono::Duration::days(7),
                        |p| p.timeline_week += chrono::Duration::days(7),
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .child(v_flex().w(px(40.)).h(px(TIMELINE)).justify_between().children((0..=6).map(|i| {
                        Label::new(crate::calendar::format_hour((i * 4) % 24, strings, cx)).size(LabelSize::XSmall).color(Color::Muted)
                    })))
                    .child(
                        div()
                            .flex_1()
                            .h(px(TIMELINE))
                            .relative()
                            .children((0..=6).map(|i| {
                                div().absolute().left_0().right_0().top(px(TIMELINE * i as f32 / 6.)).h(px(1.)).bg(grid_line)
                            }))
                            .child(h_flex().size_full().children((0..7).map(|day| {
                                let weekday = crate::calendar::weekday_labels(&strings.weekdays_short, cx)[day];
                                div().flex_1().h_full().relative().children(
                                    blocks.iter().filter(|(d, _, _)| *d == day).enumerate().map(|(idx, (_, from, to))| {
                                        let top = TIMELINE * *from as f32 / 1440.;
                                        let height = (TIMELINE * (to - from) as f32 / 1440.).max(2.);
                                        let fmt_min = |m: i64| {
                                            let clamped = m.clamp(0, 1439) as u32;
                                            chrono::NaiveTime::from_hms_opt(clamped / 60, clamped % 60, 0)
                                                .map(|t| crate::calendar::format_time(t, strings, cx))
                                                .unwrap_or_default()
                                        };
                                        let tip = format!(
                                            "{weekday} · {}–{} ({})",
                                            fmt_min(i64::from(*from)),
                                            fmt_min(i64::from(*to)),
                                            format_minutes(i64::from(to.saturating_sub(*from)), strings)
                                        );
                                        div()
                                            .id(SharedString::from(format!("focus-timeline-{day}-{idx}")))
                                            .absolute()
                                            .left(px(4.))
                                            .right(px(4.))
                                            .top(px(top))
                                            .h(px(height))
                                            .rounded(px(2.))
                                            .bg(accent)
                                            .tooltip(Tooltip::text(tip))
                                    }),
                                )
                            }))),
                    ),
            )
            .child(h_flex().pl(px(48.)).children(crate::calendar::weekday_labels(&strings.weekdays_short, cx).iter().map(|d| {
                div().flex_1().flex().justify_center().child(Label::new(*d).size(LabelSize::XSmall).color(Color::Muted))
            })));

        // Most Focused Time: minutes per hour of the chosen month.
        let hours = fern_core::focus::minutes_by_hour(&self.sessions, page.month);
        let hours_top = hours.iter().copied().max().unwrap_or(0).max(1) as f32;
        let most = hours.iter().enumerate().max_by_key(|(_, m)| **m).filter(|(_, m)| **m > 0).map(|(h, _)| h);
        let most_focused = card()
            .min_h(px(CARD_HEIGHT))
            .flex_1()
            .min_w_0()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.stats_most_focused).weight(FontWeight::SEMIBOLD)))
                    .child(nav(
                        "most",
                        crate::calendar::month_title(page.month, strings),
                        |p| p.month = crate::calendar::add_months(p.month, -1),
                        |p| p.month = crate::calendar::add_months(p.month, 1),
                        cx,
                    )),
            )
            .child(bars(
                hours.to_vec(),
                hours_top,
                (0..24).map(|h| if h % 3 == 0 { format!("{h:02}") } else { String::new() }).collect(),
                hours
                    .iter()
                    .enumerate()
                    .map(|(h, m)| format!("{} · {}", crate::calendar::format_hour(h as u32, strings, cx), format_minutes(*m, strings)))
                    .collect(),
                most,
            ));

        // Year Grids: one cell per day of the year.
        let first = NaiveDate::from_ymd_opt(page.year, 1, 1).unwrap_or(today);
        let shade = |minutes: i64| match minutes {
            0 => theme.colors().element_hover,
            1..60 => theme.colors().accent.opacity(0.3),
            60..180 => theme.colors().accent.opacity(0.55),
            180..300 => theme.colors().accent.opacity(0.8),
            _ => theme.colors().accent,
        };
        // As the original's: the year's days in columns of 15, top to bottom, with the quarters'
        // months over them and the legend at the side.
        let year_days = fern_core::focus::minutes_by_day(&self.sessions, first, 366);
        let in_year = (0..366).take_while(|i| (first + chrono::Duration::days(*i as i64)).year() == page.year).count();
        const PER_COLUMN: usize = 15;
        const CELL: f32 = 13.;
        let columns = in_year.div_ceil(PER_COLUMN);
        let month_column = |month: u32| {
            NaiveDate::from_ymd_opt(page.year, month, 1).map_or(0, |d| (d - first).num_days() as usize / PER_COLUMN)
        };
        let h = strings.duration_hour_short;
        let m = strings.duration_min_short;
        let legend = [
            (0, format!("0{m}")),
            (1, format!("0-1{h}")),
            (60, format!("1{h}-3{h}")),
            (180, format!("3{h}-5{h}")),
            (300, format!(">5{h}")),
        ];
        let year_grid = card()
            .min_h(px(CARD_HEIGHT))
            .flex_1()
            .min_w_0()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.stats_year_grids).weight(FontWeight::SEMIBOLD)))
                    .child(nav("year", page.year.to_string(), |p| p.year -= 1, |p| p.year += 1, cx)),
            )
            .child(
                div().relative().h(px(14.)).children([1u32, 4, 7, 10].map(|month| {
                    div()
                        .absolute()
                        .left(px(month_column(month) as f32 * (CELL + 3.)))
                        .child(Label::new(strings.months_short[month as usize - 1]).size(LabelSize::XSmall).color(Color::Muted))
                })),
            )
            .child(
                h_flex()
                    .items_end()
                    .gap_3()
                    .child(h_flex().gap(px(3.)).children((0..columns).map(|column| {
                        v_flex().gap(px(3.)).children((0..PER_COLUMN).map(|row| {
                            let index = column * PER_COLUMN + row;
                            div()
                                .id(SharedString::from(format!("focus-year-cell-{index}")))
                                .size(px(CELL))
                                .rounded(px(2.))
                                .when(index < in_year, |el| {
                                    let day = first + chrono::Duration::days(index as i64);
                                    let mins = year_days[index];
                                    let tip = format!("{} · {}", crate::calendar::format_date(day, strings, cx), format_minutes(mins, strings));
                                    el.bg(shade(mins)).tooltip(Tooltip::text(tip))
                                })
                        }))
                    })))
                    .child(v_flex().gap_0p5().children(legend.map(|(minutes, label)| {
                        h_flex()
                            .gap_1()
                            .child(div().size(px(8.)).rounded(px(1.)).bg(shade(minutes)))
                            .child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted))
                    }))),
            );

        let header = h_flex()
            .debug_selector(|| "statistics-header".to_owned())
            .w_full()
            .max_w(px(1024.))
            .px_8()
            .pt(crate::title_bar::height(window, cx) + px(6.))
            .pb_4()
            .flex_none()
            .child(div().flex_1().child(div().text_size(px(28.)).font_weight(FontWeight::MEDIUM).child(strings.statistics)))
            .child(h_flex().p(px(2.)).rounded(px(18.)).bg(theme.colors().surface_background).children(
                strings.stats_tabs.iter().enumerate().map(|(i, label)| {
                    div()
                        .id(SharedString::from(format!("stats-tab-{i}")))
                        .debug_selector(|| format!("statistics-tab-{i}"))
                        .px_5()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .rounded(px(16.))
                        .cursor_pointer()
                        .when(i == page.tab, |el| el.bg(theme.colors().element_selected))
                        .when(i != page.tab, |el| el.hover(|s| s.bg(theme.colors().ghost_element_hover)))
                        .child(Label::new(*label))
                        .on_click(cx.listener(move |this, _, _, cx| this.update_stats(cx, |p| p.tab = i)))
                }),
            ))
            .child(div().flex_1())
            .child(
                div()
                    .id("stats-done")
                    .w(px(100.))
                    .h(px(40.))
                    .justify_center()
                    .flex()
                    .items_center()
                    .rounded(px(8.))
                    .bg(theme.colors().accent)
                    .hover(|s| s.opacity(0.9))
                    .cursor_pointer()
                    .child(Label::new(strings.done).color(Color::OnAccent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.stats = None;
                        cx.emit(StatsClosed);
                        cx.notify();
                    })),
            );

        let content = v_flex()
            .debug_selector(|| "statistics-content".to_owned())
            .w_full()
            .max_w(px(1024.))
            .px_8()
            .pb_5()
            .gap_4()
            .map(|el| match page.tab {
                0 => el.children(self.render_overview_tab(page, today, cx)),
                1 => el.children(self.render_task_tab(page, today, cx)),
                _ => el
                    .child(overview)
                    .child(
                        h_flex()
                            .items_stretch()
                            .gap_4()
                            .child(div().flex_1().min_w_0().child(self.render_focus_details(page, today, cx)))
                            // Focus Record beside Details, as in the original.
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .p_4()
                                    .gap_2()
                                    .rounded(px(10.))
                                    .bg(theme.colors().surface_background)
                                    .min_h(px(CARD_HEIGHT))
                                    .child(self.render_record_header(false, cx))
                                    .child(
                                        v_scroll(
                                            "stats-focus-record-scroll",
                                            v_flex()
                                                .id("stats-focus-record")
                                                .pr(px(14.))
                                                .children(self.render_record(strings, cx)),
                                        )
                                        .max_h(px(200.))
                                        .compact(),
                                    ),
                            ),
                    )
                    .child(h_flex().items_start().gap_4().child(trends).child(timeline))
                    // Most Focused Time and Year Grids side by side, as in the original.
                    .child(h_flex().items_start().gap_4().child(most_focused).child(year_grid)),
            });

        self.bind_focus_root(v_flex().relative().size_full(), cx)
            .items_center()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(header)
            .child(
                v_scroll(
                    ("focus-stats-bar", page.tab),
                    div()
                        .id(("focus-stats", page.tab))
                        .debug_selector(|| "statistics-scroll".to_owned())
                        .w_full()
                        .flex()
                        .items_start()
                        .justify_center()
                        .child(content),
                )
                .track_scroll(&self.stats_scroll)
                .w_full()
                .flex_1()
                .min_h_0(),
            )
            .children(self.render_record_dialog(cx))
            .when(self.confirm_delete_all, |el| el.child(self.render_confirm_delete_all(cx)))
            .when_some(self.confirm_delete_record.clone(), |el, (id, desc)| {
                el.child(self.render_confirm_delete_record(id, desc, cx))
            })
            .into_any_element()
    }

    /// The Statistics page's Overview tab: totals, recent curves and the week's habits.
    fn render_overview_tab(&self, page: StatsPage, today: NaiveDate, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let tasks = &self.stats_data.tasks;
        let lists = &self.stats_data.lists;
        let focus = if self.session_summary.day == today {
            self.session_summary.stats
        } else {
            focus_stats(&self.sessions, today)
        };
        let completed_total = tasks.iter().filter(|t| t.is_completed() && !t.is_deleted()).count();
        let open_total = tasks.iter().filter(|t| !t.is_closed() && !t.is_deleted()).count();
        let start = today - chrono::Duration::days(6);
        let completed = fern_core::stats::completed_by_day(tasks, start, 7);
        let card = || v_flex().min_w_0().p_4().gap_2().rounded(px(10.)).bg(theme.colors().surface_background);
        let day_labels: Vec<String> =
            (0..7).map(|i| (start + chrono::Duration::days(i)).day().to_string()).collect();
        // The original's Overview: 30 px values over muted words, lines between the columns.
        // A value as the original's: the numbers large, a duration's unit letters small beside them.
        let value = |parts: Vec<(i64, Option<&'static str>)>, label: &'static str| {
            v_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_1()
                .child(h_flex().items_end().gap(px(6.)).text_color(theme.colors().text_accent).font_weight(FontWeight::SEMIBOLD).children(parts.into_iter().map(|(n, unit)| {
                    h_flex()
                        .items_end()
                        .child(div().text_size(px(30.)).line_height(px(36.)).child(n.to_string()))
                        .children(unit.map(|u| div().text_size(px(16.)).line_height(px(16.)).mb(px(7.)).ml_0p5().child(u)))
                })))
                .child(Label::new(label).color(Color::Muted).truncate())
        };
        let count = |n: i64| vec![(n, None)];
        let duration = |m: i64| localized_duration_parts(fern_core::focus::duration_parts_full(m), strings);
        let rule = || div().w(px(1.)).h(px(38.)).flex_none().bg(theme.colors().border);
        // The counts are semibold, their words regular, as in the original.
        let counts = [
            ("{tasks}", open_total.to_string()),
            ("{completed}", completed_total.to_string()),
            ("{lists}", lists.len().to_string()),
            ("{days}", fern_core::stats::days_using(tasks, today).to_string()),
        ];
        let summary = h_flex().children(summary_parts(strings.stats_summary, &counts).into_iter().map(|(text, count)| {
            Label::new(text).when(count, |l| l.weight(FontWeight::SEMIBOLD))
        }));
        // Each curve has its own Day / Week / Month switch, as in the original.
        let week_start = crate::calendar::week_start(cx);
        let periods = |curve: usize| stats::buckets(page.curves[curve], today, week_start, 7);
        let labels = |curve: usize| -> Vec<String> {
            let bucket = page.curves[curve];
            let periods = periods(curve);
            let last = periods.len() - 1;
            periods
                .iter()
                .enumerate()
                .map(|(i, (first, _))| match bucket {
                    Bucket::Day if i == last => strings.today.to_owned(),
                    Bucket::Week if i == last => strings.stats_this_week.to_owned(),
                    Bucket::Month if i == last => strings.stats_this_month.to_owned(),
                    Bucket::Month => strings.months_short[first.month0() as usize].to_owned(),
                    _ => first.day().to_string(),
                })
                .collect()
        };
        let completed_curve: Vec<i64> = stats::completed_in(tasks, &periods(0)).into_iter().map(|n| n as i64).collect();
        let rates: Vec<i64> = stats::completion_rate_in(tasks, &periods(1)).into_iter().map(i64::from).collect();
        let pomos: Vec<i64> = stats::focus_in(&self.sessions, &periods(2)).into_iter().map(|(n, _)| n as i64).collect();
        let minutes: Vec<i64> = stats::focus_in(&self.sessions, &periods(3)).into_iter().map(|(_, m)| m).collect();
        let productive = stats::more_productive_than(tasks, today);
        let scores = fern_core::stats::achievement_scores(tasks, &self.sessions, &self.stats_data.all_habit_checkins, start, 7);
        let score = scores.last().copied().unwrap_or(0);
        let chart = |curve: usize, title: &'static str, values: Vec<i64>, line: bool, cx: &mut Context<Self>| {
            let bucket = page.curves[curve];
            let open = page.curve_menu == Some(curve);
            let names = [strings.stats_day, strings.stats_week, strings.stats_month];
            let index = Bucket::ALL.iter().position(|b| *b == bucket).unwrap_or(0);
            let menu = open.then(|| {
                PopoverAttachment::new(
                    Placement::BelowRight,
                    popover(cx)
                        .min_w(px(140.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.update_stats(cx, |p| p.curve_menu = None)))
                        .children(Bucket::ALL.into_iter().zip(names).enumerate().map(|(i, (choice, name))| {
                            ListItem::new(SharedString::from(format!("curve-{curve}-{i}")))
                                .child(Label::new(name).color(if choice == bucket { Color::Accent } else { Color::Default }))
                                .when(choice == bucket, |item| {
                                    item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_stats(cx, |p| {
                                        p.curves[curve] = choice;
                                        p.curve_menu = None;
                                    })
                                }))
                        })),
                )
            });
            let labels = labels(curve);
            let chart_id: &'static str = match curve {
                0 => "stats-curve-0",
                1 => "stats-curve-1",
                2 => "stats-curve-2",
                _ => "stats-curve-3",
            };
            let tooltips: Vec<String> = labels
                .iter()
                .zip(&values)
                .map(|(l, v)| {
                    let formatted = match curve {
                        1 => format!("{v}%"),
                        3 => format_minutes(*v, strings),
                        _ => v.to_string(),
                    };
                    format!("{l} · {formatted}")
                })
                .collect();
            card()
                .flex_1()
                .h(px(CARD_HEIGHT))
                .child(
                    h_flex()
                        .child(div().flex_1().child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title)))
                        .child(
                            h_flex()
                                .id(SharedString::from(format!("curve-period-{curve}")))
                                .relative()
                                .px_2()
                                .h(px(27.))
                                .gap_0p5()
                                .rounded(px(14.))
                                .border_1()
                                .border_color(if open { theme.colors().accent } else { theme.colors().border })
                                .cursor_pointer()
                                .child(Label::new(names[index]).size(LabelSize::Small))
                                .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_stats(cx, |p| p.curve_menu = (!open).then_some(curve))
                                }))
                                .children(menu),
                        ),
                )
                .map(|el| {
                    if line {
                        el.child(line_chart(chart_id, &values, labels, tooltips, CHART_HEIGHT, true, cx))
                    } else {
                        el.child(bar_chart(chart_id, &values, labels, tooltips, Some(6), CHART_HEIGHT, cx))
                    }
                })
        };

        // The week's habits: proportional daily progress respecting is_due and Undone.
        let week_start = crate::calendar::week_start(cx);
        let week = crate::calendar::week_of(today, cx);
        let habit_week = h_flex().justify_between().children(week.iter().map(|date| {
            let progress = fern_core::habit::day_progress(&self.stats_data.all_habits, &self.stats_data.habit_records, *date, week_start);
            let ring: AnyElement = match progress {
                fern_core::DayProgress::Needless => div()
                    .size(px(36.))
                    .rounded_full()
                    .border_2()
                    .border_color(theme.colors().element_hover)
                    .into_any_element(),
                fern_core::DayProgress::AllUndone => div()
                    .size(px(36.))
                    .rounded_full()
                    .border_2()
                    .border_color(theme.status().error.opacity(0.6))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::X).size(IconSize::Small).color(Color::Error))
                    .into_any_element(),
                fern_core::DayProgress::Fraction(frac) if frac >= 0.999 => div()
                    .size(px(36.))
                    .rounded_full()
                    .bg(theme.colors().accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Check).size(IconSize::Small).color(Color::IconOnAccent))
                    .into_any_element(),
                fern_core::DayProgress::Fraction(frac) => desktop_common::habit_helpers::progress_ring(
                    px(36.),
                    frac,
                    theme.colors().accent,
                    theme.colors().element_hover,
                    px(3.),
                )
                .into_any_element(),
            };
            let tip = match progress {
                fern_core::DayProgress::Needless => crate::calendar::format_date(*date, strings, cx),
                fern_core::DayProgress::AllUndone => format!("{} · {}", crate::calendar::format_date(*date, strings, cx), strings.habit_unachieved),
                fern_core::DayProgress::Fraction(frac) => format!("{} · {}%", crate::calendar::format_date(*date, strings, cx), (frac * 100.0).round() as u32),
            };
            v_flex()
                .id(SharedString::from(format!("stats-habit-week-{date}")))
                .items_center()
                .gap_1()
                .tooltip(Tooltip::text(tip))
                .child(ring)
                .child(Label::new(strings.weekdays_short[date.weekday().num_days_from_monday() as usize]).size(LabelSize::XSmall).color(Color::Muted))
        }));

        vec![
            div()
                .h(px(60.))
                .flex_none()
                .px_4()
                .items_center()
                .rounded(px(10.))
                .bg(theme.colors().surface_background)
                .flex()
                .child(div().flex_1().child(summary))
                .child(
                    h_flex().gap_1().children({
                        let (before, after) = strings.stats_more_productive.split_once("{n}").unwrap_or((strings.stats_more_productive, ""));
                        [
                            Label::new(before.trim_end()),
                            Label::new(format!("{productive}%")).weight(FontWeight::SEMIBOLD).color(Color::Warning),
                            Label::new(after.trim_start()),
                        ]
                    }),
                )
                .into_any_element(),
            h_flex()
                .items_stretch()
                .gap_4()
                .child(
                    card()
                        .flex_1()
                        .h(px(CARD_HEIGHT))
                        .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(strings.focus_overview))
                        .child(
                            v_flex()
                                .flex_1()
                                .justify_center()
                                .gap_8()
                                .child(
                                    h_flex()
                                        .child(value(count(completed[6] as i64), strings.stats_today_completed))
                                        .child(rule())
                                        .child(value(count(focus.today_count as i64), strings.focus_today_pomo))
                                        .child(rule())
                                        .child(value(duration(focus.today_minutes), strings.focus_today_focus)),
                                )
                                .child(
                                    h_flex()
                                        .child(value(count(completed_total as i64), strings.stats_total_completed))
                                        .child(rule())
                                        .child(value(count(focus.total_count as i64), strings.focus_total_pomo))
                                        .child(rule())
                                        .child(value(duration(focus.total_minutes), strings.focus_total_duration)),
                                ),
                        ),
                )
                .child(
                    card()
                        .flex_1()
                        .h(px(CARD_HEIGHT))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().flex_1().child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(strings.stats_achievement_score)))
                                .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.status().success).child(score.to_string()))
                                .child(
                                    div()
                                        .px_1p5()
                                        .h(px(20.))
                                        .flex()
                                        .items_center()
                                        .rounded(px(6.))
                                        .bg(theme.colors().accent.opacity(0.25))
                                        .child(
                                            Label::new(strings.stats_level.replace("{level}", &fern_core::stats::achievement_level(score).to_string()))
                                                .size(LabelSize::XSmall)
                                                .color(Color::Accent),
                                        ),
                                ),
                        )
                        .child({
                            let score_tips: Vec<String> = day_labels.iter().zip(&scores).map(|(l, v)| format!("{l} · {v}")).collect();
                            line_chart("stats-achievement-score", &scores, day_labels, score_tips, CHART_HEIGHT, false, cx)
                        }),
                )
                .into_any_element(),
            h_flex()
                .items_start()
                .gap_4()
                .child(chart(0, strings.stats_completion_curve, completed_curve, true, cx))
                .child(chart(1, strings.stats_rate_curve, rates, false, cx))
                .into_any_element(),
            h_flex()
                .items_start()
                .gap_4()
                .child(chart(2, strings.stats_pomo_curve, pomos, true, cx))
                .child(chart(3, strings.stats_focus_curve, minutes, false, cx))
                .into_any_element(),
            // As the original's, the week's habits take the left column's width.
            h_flex()
                .items_start()
                .gap_4()
                .child(card().flex_1().child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(strings.stats_weekly_habits)).child(habit_week))
                .child(div().flex_1())
                .into_any_element(),
        ]
    }

    /// The Statistics page's Task tab: a day's completed tasks and rate, and completion per list.
    /// The Statistics page's Task tab: the original's Daily / Weekly / Monthly report of a period:
    /// completed tasks and rate with the change from the previous period, the completion per day,
    /// how the tasks ended, and completion per list.
    fn render_task_tab(&self, page: StatsPage, today: NaiveDate, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let tasks = &self.stats_data.tasks;
        let week_start = crate::calendar::week_start(cx);
        let period = page.task_period;
        // The period containing a day, and the one before it.
        let range = |day: NaiveDate| stats::buckets(period, day, week_start, 1)[0];
        let (first, last) = range(page.day);
        let (prev_first, prev_last) = range(first - chrono::Duration::days(1));
        let count = |a: NaiveDate, b: NaiveDate| stats::completed_in(tasks, &[(a, b)])[0] as i64;
        let rate = |a: NaiveDate, b: NaiveDate| stats::completion_rate_in(tasks, &[(a, b)])[0] as i64;
        let from = match period {
            Bucket::Day => strings.stats_from_yesterday,
            Bucket::Week => strings.stats_from_last_week,
            Bucket::Month => strings.stats_from_last_month,
        };
        // As the original's, the Task tab's cards are as tall as the Overview's.
        let card = || v_flex().min_w_0().min_h(px(CARD_HEIGHT)).p_4().gap_2().rounded(px(10.)).bg(theme.colors().surface_background);
        let value = |v: String, label: &'static str, change: String, delta: i64| {
            v_flex()
                .flex_1()
                .items_center()
                .child(div().text_size(px(26.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.colors().text_accent).child(v))
                .child(Label::new(label).size(LabelSize::XSmall).color(Color::Muted))
                .child(change_line(from.replace("{n}", &change), delta))
        };
        let current = range(today) == (first, last);
        let period_label = match period {
            Bucket::Day if current => strings.today.to_owned(),
            Bucket::Week if current => strings.stats_this_week.to_owned(),
            Bucket::Month if current => strings.stats_this_month.to_owned(),
            Bucket::Day => crate::task_rows::short_date(first, strings),
            Bucket::Week => format!("{} - {}", crate::task_rows::short_date(first, strings), crate::task_rows::short_date(last, strings)),
            Bucket::Month => format!("{} {}", strings.months_short[first.month0() as usize], first.year()),
        };
        let names = [strings.stats_daily, strings.stats_weekly, strings.stats_monthly];
        let index = Bucket::ALL.iter().position(|b| *b == period).unwrap_or(0);
        let menu = page.task_menu.then(|| {
            PopoverAttachment::new(
                Placement::BelowLeft,
                popover(cx)
                    .min_w(px(140.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.update_stats(cx, |p| p.task_menu = false)))
                    .children(Bucket::ALL.into_iter().zip(names).enumerate().map(|(i, (choice, name))| {
                        ListItem::new(SharedString::from(format!("task-period-{i}")))
                            .child(Label::new(name).color(if choice == period { Color::Accent } else { Color::Default }))
                            .when(choice == period, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_stats(cx, |p| {
                                    p.task_period = choice;
                                    p.task_menu = false;
                                })
                            }))
                    })),
            )
        });
        let step = move |day: NaiveDate, forward: bool| -> NaiveDate {
            let (first, last) = stats::buckets(period, day, week_start, 1)[0];
            if forward { last + chrono::Duration::days(1) } else { first - chrono::Duration::days(1) }
        };

        let by_list = stats::completed_by_list_in(tasks, first, last);
        let kinds = stats::completion_kinds(tasks, first, last);
        let status = theme.status().clone();
        let kind_rows = [
            (status.error, kinds.overdue, strings.stats_overdue),
            (theme.colors().accent, kinds.on_time, strings.stats_on_time),
            (status.warning, kinds.undated, strings.stats_undated),
            (theme.colors().text_muted, kinds.uncompleted, strings.stats_uncompleted),
        ];
        let rate_card = card()
            .flex_1()
            .child(Label::new(strings.stats_rate_distribution).weight(FontWeight::SEMIBOLD))
            .child(
                h_flex()
                    .py_2()
                    .gap_6()
                    .justify_center()
                    .child(
                        Donut::new(kind_rows.iter().map(|(color, n, _)| (*n as f32, *color))).size(px(216.)).thickness(px(14.)).center(
                            v_flex()
                                .items_center()
                                .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(if kinds.total() == 0 {
                                    strings.stats_no_data.to_owned()
                                } else {
                                    format!("{}%", rate(first, last))
                                }))
                                .when(kinds.total() > 0, |el| {
                                    el.child(Label::new(strings.stats_completion_rate).size(LabelSize::XSmall).color(Color::Muted))
                                }),
                        ),
                    )
                    .when(kinds.total() > 0, |el| {
                        el.child(v_flex().gap_1().children(kind_rows.iter().map(|(color, n, label)| {
                            h_flex()
                                .gap_2()
                                .child(div().size(px(8.)).rounded_full().bg(*color))
                                .child(Label::new(n.to_string()).size(LabelSize::Small))
                                .child(Label::new(*label).size(LabelSize::Small).color(Color::Muted))
                        })))
                    }),
            );
        // Weekly and Monthly add the completion per day, as in the original.
        let distribution = (period != Bucket::Day).then(|| {
            let days = (last - first).num_days() as usize + 1;
            let values: Vec<i64> = stats::completed_by_day(tasks, first, days).into_iter().map(|n| n as i64).collect();
            let labels: Vec<String> = (0..days)
                .map(|i| {
                    let day = first + chrono::Duration::days(i as i64);
                    match period {
                        Bucket::Week => strings.weekdays_short[day.weekday().num_days_from_monday() as usize].to_owned(),
                        // A label every fifth day keeps a month readable.
                        _ if i % 5 == 0 => day.day().to_string(),
                        _ => String::new(),
                    }
                })
                .collect();
            let tooltips: Vec<String> = (0..days)
                .map(|i| {
                    let day = first + chrono::Duration::days(i as i64);
                    format!("{} · {}", crate::task_rows::short_date(day, strings), values[i])
                })
                .collect();
            card()
                .flex_1()
                .child(Label::new(strings.stats_completion_distribution).weight(FontWeight::SEMIBOLD))
                .child(bar_chart("stats-task-dist", &values, labels, tooltips, None, 120., cx))
        });

        // Daily shows how tasks ended beside the overview; Weekly and Monthly put the completion
        // per day there and move it below.
        let (top_right, lower_rate) = match distribution {
            Some(distribution) => (distribution, Some(rate_card)),
            None => (rate_card, None),
        };
        vec![
            // As the original's: the period and the day in 34 px boxes, 12 px under the tabs.
            h_flex()
                .gap_2()
                .pt(px(12.))
                .child(
                    h_flex()
                        .id("task-period")
                        .relative()
                        .px_3()
                        .h(px(34.))
                        .gap_1()
                        .rounded(px(8.))
                        .bg(theme.colors().surface_background)
                        .hover(|s| s.bg(theme.colors().element_hover))
                        .cursor_pointer()
                        .child(Label::new(names[index]).size(LabelSize::Small))
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(|this, _, _, cx| this.update_stats(cx, |p| p.task_menu = !p.task_menu)))
                        .children(menu),
                )
                .child(
                    h_flex()
                        .h(px(34.))
                        .px_1()
                        .gap_1()
                        .rounded(px(8.))
                        .bg(theme.colors().surface_background)
                        .child(
                            IconButton::new("task-day-back", IconName::ChevronLeft)
                                .tooltip(Tooltip::text("←"))
                                .on_click(cx.listener(move |this, _, _, cx| this.update_stats(cx, |p| p.day = step(p.day, false)))),
                        )
                        .child(div().min_w(px(90.)).flex().justify_center().child(Label::new(period_label).size(LabelSize::Small)))
                        .child(
                            IconButton::new("task-day-forward", IconName::ChevronRight)
                                .tooltip(Tooltip::text("→"))
                                .on_click(cx.listener(move |this, _, _, cx| this.update_stats(cx, |p| p.day = step(p.day, true)))),
                        ),
                )
                .into_any_element(),
            h_flex()
                .items_stretch()
                .gap_4()
                .child(
                    card()
                        .flex_1()
                        .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(strings.focus_overview))
                        // As the original's, the two values in the middle of the card with a line
                        // between them.
                        .child(
                            h_flex()
                                .flex_1()
                                .items_center()
                                .child({
                                    let delta = count(first, last) - count(prev_first, prev_last);
                                    value(count(first, last).to_string(), strings.stats_completed_tasks, delta.abs().to_string(), delta)
                                })
                                .child(div().w(px(1.)).h(px(38.)).flex_none().bg(theme.colors().border))
                                .child({
                                    let delta = rate(first, last) - rate(prev_first, prev_last);
                                    value(format!("{}%", rate(first, last)), strings.stats_completion_rate, format!("{}%", delta.abs()), delta)
                                }),
                        ),
                )
                .child(top_right)
                .into_any_element(),
            h_flex()
                .items_start()
                .gap_4()
                .children(lower_rate)
                .child(
                    card()
                        .flex_1()
                        .child(
                            h_flex()
                                .child(div().flex_1().child(Label::new(strings.stats_by_list).weight(FontWeight::SEMIBOLD)))
                                .child({
                                    let names = [strings.stats_by_list_option, strings.stats_by_tag_option];
                                    let menu = page.classify_menu.then(|| {
                                        PopoverAttachment::new(
                                            Placement::BelowRight,
                                            popover(cx)
                                                .min_w(px(140.))
                                                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.update_stats(cx, |p| p.classify_menu = false)))
                                                .children([false, true].into_iter().map(|tag| {
                                                    let selected = tag == page.by_tag;
                                                    ListItem::new(SharedString::from(format!("classify-{tag}")))
                                                        .child(Label::new(names[tag as usize]).color(if selected { Color::Accent } else { Color::Default }))
                                                        .when(selected, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                            this.update_stats(cx, |p| {
                                                                p.by_tag = tag;
                                                                p.classify_menu = false;
                                                            })
                                                        }))
                                                })),
                                        )
                                    });
                                    h_flex()
                                        .id("classify-by")
                                        .relative()
                                        .px_2()
                                        .h(px(27.))
                                        .gap_0p5()
                                        .rounded(px(14.))
                                        .border_1()
                                        .border_color(if page.classify_menu { theme.colors().accent } else { theme.colors().border })
                                        .cursor_pointer()
                                        .child(Label::new(names[page.by_tag as usize]).size(LabelSize::Small))
                                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                                        .on_click(cx.listener(|this, _, _, cx| this.update_stats(cx, |p| p.classify_menu = !p.classify_menu)))
                                        .children(menu)
                                }),
                        )
                        .child({
                            // Each list in its own color, or one from a palette.
                            let palette = [theme.colors().accent, status.success, status.warning, status.error, theme.colors().text_muted];
                            let slices: Vec<(SharedString, usize, Hsla)> = if page.by_tag {
                                stats::completed_by_tag_in(tasks, first, last)
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, (tag, n))| {
                                        let color = tag
                                            .as_deref()
                                            .and_then(|name| self.stats_data.tag_colors.get(name).copied())
                                            .unwrap_or(palette[i % palette.len()]);
                                        let name: SharedString = tag.map_or(strings.group_no_tags.into(), |t| format!("#{t}").into());
                                        (name, n, color)
                                    })
                                    .collect()
                            } else {
                                by_list
                                    .iter()
                                    .enumerate()
                                    .map(|(i, (list, n))| {
                                        let list = list.and_then(|id| self.stats_data.list_lookup.get(&id));
                                        let name: SharedString = list.map_or(strings.inbox.into(), |(name, _)| name.clone());
                                        let color = list
                                            .and_then(|(_, color)| *color)
                                            .unwrap_or(palette[i % palette.len()]);
                                        (name, *n, color)
                                    })
                                    .collect()
                            };
                            // A task with several tags counts once in the middle.
                            let unique: usize = by_list.iter().map(|(_, n)| n).sum();
                            let completed = unique;
                            h_flex()
                                .py_2()
                                .gap_6()
                                .justify_center()
                                .child(
                                    Donut::new(slices.iter().map(|(_, n, color)| (*n as f32, *color))).size(px(216.)).thickness(px(14.)).center(
                                        v_flex()
                                            .items_center()
                                            .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(if completed == 0 {
                                                strings.stats_no_data.to_owned()
                                            } else {
                                                completed.to_string()
                                            }))
                                            .when(completed > 0, |el| {
                                                el.child(Label::new(strings.stats_completed_tasks).size(LabelSize::XSmall).color(Color::Muted))
                                            }),
                                    ),
                                )
                                .child(v_flex().gap_1().children(slices.into_iter().map(|(name, n, color)| {
                                    h_flex()
                                        .gap_2()
                                        .child(div().size(px(8.)).rounded_full().bg(color))
                                        .child(Label::new(n.to_string()).size(LabelSize::Small))
                                        .child(Label::new(name).size(LabelSize::Small).color(Color::Muted).truncate())
                                })))
                        }),
                )
                // Daily keeps the list card at half width, as in the original.
                .when(period == Bucket::Day, |el| el.child(div().flex_1()))
                .into_any_element(),
        ]
    }
}
