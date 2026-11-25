//! Reusable month calendar header, weekday row, and 7-column day grid helpers shared across
//! due-date, range, habit, quick-add, and sidebar date pickers.

use chrono::{Datelike as _, NaiveDate};
use fern_i18n::Strings;
use gpui::{AnyElement, App, FontWeight, SharedString, Window, px};
use ui::prelude::*;

use crate::calendar;

/// Weekday label style rendered above the 7-column grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeekdayStyle {
    Narrow,
    Short,
}

/// Optional month header navigation bar (`<  Month YYYY  [Today]  >`).
pub struct MonthHeaderConfig<'a> {
    pub month: NaiveDate,
    pub strings: &'a Strings,
    pub prev_id: &'static str,
    pub next_id: &'static str,
    pub today_id: Option<&'static str>,
    pub today_tooltip: Option<&'a str>,
    pub small_title: bool,
    pub sidebar_palette: bool,
    pub arrow_size: Pixels,
    pub custom_title: Option<String>,
    pub title_weight: FontWeight,
    pub header_height: Option<Pixels>,
}

impl<'a> MonthHeaderConfig<'a> {
    pub fn surface(
        month: NaiveDate,
        strings: &'a Strings,
        prev_id: &'static str,
        next_id: &'static str,
    ) -> Self {
        Self {
            month,
            strings,
            prev_id,
            next_id,
            today_id: None,
            today_tooltip: None,
            small_title: false,
            sidebar_palette: false,
            arrow_size: px(22.),
            custom_title: None,
            title_weight: FontWeight::SEMIBOLD,
            header_height: None,
        }
    }
}

/// Configuration for [`render_month_grid`].
#[derive(Clone, Debug)]
pub struct MonthGridConfig {
    pub month: NaiveDate,
    pub today: NaiveDate,
    pub selected: Option<NaiveDate>,
    pub other_endpoint: Option<NaiveDate>,
    pub range: Option<(NaiveDate, NaiveDate)>,
    pub id_prefix: &'static str,
    pub debug_prefix: Option<&'static str>,
    pub cell_size: Pixels,
    pub cell_height: Option<Pixels>,
    pub rounded_full: bool,
    pub label_size: LabelSize,
    pub fade_outside_with_opacity: bool,
    pub highlight_today_bg: bool,
    pub accent_today_text: bool,
    pub wrap_in_span_background: bool,
}

impl MonthGridConfig {
    pub fn single(id_prefix: &'static str, month: NaiveDate, today: NaiveDate) -> Self {
        Self {
            month,
            today,
            selected: None,
            other_endpoint: None,
            range: None,
            id_prefix,
            debug_prefix: None,
            cell_size: px(26.),
            cell_height: None,
            rounded_full: true,
            label_size: LabelSize::Small,
            fade_outside_with_opacity: false,
            highlight_today_bg: true,
            accent_today_text: true,
            wrap_in_span_background: false,
        }
    }
}

/// Click handler invoked by the optional "Today" button in a month header.
pub type TodayClickHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;

/// Renders the month title with prev/next arrows and an optional "Today" circle button.
pub fn render_month_header(
    cfg: MonthHeaderConfig<'_>,
    cx: &App,
    on_shift: impl Fn(i32, &mut Window, &mut App) + Clone + 'static,
    on_today: Option<TodayClickHandler>,
) -> AnyElement {
    let title = cfg
        .custom_title
        .unwrap_or_else(|| calendar::month_title(cfg.month, cfg.strings));
    let arrow = |id: &'static str, icon: IconName, delta: i32, tip: &'static str| {
        let on_shift = on_shift.clone();
        let mut btn = IconButton::new(id, icon)
            .size(cfg.arrow_size)
            .icon_size(IconSize::XSmall)
            .tooltip(Tooltip::text(tip));
        if cfg.sidebar_palette {
            let colors = cx.theme().colors();
            btn = btn
                .icon_color(colors.list_text_muted)
                .hover_icon_color(colors.list_text)
                .hover_color(colors.list_hover);
        }
        btn.on_click(move |_, window, cx| on_shift(delta, window, cx))
    };

    let mut title_label = Label::new(title).weight(cfg.title_weight);
    if cfg.small_title {
        title_label = title_label.size(LabelSize::Small);
    }
    if cfg.sidebar_palette {
        title_label = title_label.color(cx.theme().colors().list_text);
    }

    h_flex()
        .items_center()
        .when_some(cfg.header_height, |el, h| el.h(h))
        .child(div().flex_1().min_w_0().child(title_label))
        .child(arrow(cfg.prev_id, IconName::ChevronLeft, -1, "←"))
        .when_some(cfg.today_id.zip(on_today), |el, (today_id, handler)| {
            let mut btn = IconButton::new(today_id, IconName::Circle)
                .size(cfg.arrow_size)
                .icon_size(IconSize::Custom(px(8.)));
            if cfg.sidebar_palette {
                let colors = cx.theme().colors();
                btn = btn
                    .icon_color(colors.list_text_muted)
                    .hover_icon_color(colors.list_text)
                    .hover_color(colors.list_hover);
            }
            if let Some(tip) = cfg.today_tooltip {
                btn = btn.tooltip(Tooltip::text(tip));
            }
            el.child(btn.on_click(move |_, window, cx| handler(window, cx)))
        })
        .child(arrow(cfg.next_id, IconName::ChevronRight, 1, "→"))
        .into_any_element()
}

/// Renders the 7 localized weekday column headers aligned to the user's configured `week_start`.
pub fn render_weekday_row(strings: &Strings, style: WeekdayStyle, sidebar_palette: bool, cx: &App) -> AnyElement {
    let source = match style {
        WeekdayStyle::Narrow => &strings.weekdays_narrow,
        WeekdayStyle::Short => &strings.weekdays_short,
    };
    let color = if sidebar_palette {
        Color::Custom(cx.theme().colors().list_text_muted)
    } else {
        Color::Muted
    };
    h_flex()
        .children(calendar::weekday_labels(source, cx).map(|day| {
            div()
                .flex_1()
                .flex()
                .justify_center()
                .child(Label::new(day).size(LabelSize::XSmall).color(color))
        }))
        .into_any_element()
}

/// Renders a 7-column month grid (`calendar::month_grid`) passing each cell through `render_cell`.
pub fn render_month_weeks(
    month: NaiveDate,
    cx: &App,
    mut render_cell: impl FnMut(NaiveDate, bool) -> AnyElement,
) -> Vec<AnyElement> {
    calendar::month_grid(month, cx)
        .chunks(7)
        .map(|week| {
            h_flex()
                .children(week.iter().map(|&date| render_cell(date, date.month() == month.month())))
                .into_any_element()
        })
        .collect()
}

/// Renders a standard selectable single-date or date-range 7-column month grid.
pub fn render_month_grid(
    cfg: &MonthGridConfig,
    cx: &App,
    on_pick: impl Fn(NaiveDate, &mut Window, &mut App) + Clone + 'static,
) -> Vec<AnyElement> {
    let (accent, today_bg, endpoint_bg, range_bg, hover_bg) = {
        let c = cx.theme().colors();
        (
            c.accent,
            c.accent.opacity(0.16),
            c.accent.opacity(0.32),
            c.accent.opacity(0.14),
            c.element_hover,
        )
    };
    let id_prefix = cfg.id_prefix;
    let debug_prefix = cfg.debug_prefix;
    let selected = cfg.selected;
    let other_endpoint = cfg.other_endpoint;
    let range = cfg.range;
    let today = cfg.today;
    let cell_size = cfg.cell_size;
    let cell_height = cfg.cell_height;
    let rounded_full = cfg.rounded_full;
    let label_size = cfg.label_size;
    let fade_outside = cfg.fade_outside_with_opacity;
    let highlight_today_bg = cfg.highlight_today_bg;
    let accent_today_text = cfg.accent_today_text;
    let wrap_span = cfg.wrap_in_span_background;

    render_month_weeks(cfg.month, cx, |date, in_month| {
        let is_selected = selected == Some(date);
        let is_today = date == today;
        let is_other_endpoint = !is_selected && other_endpoint == Some(date);
        let in_range = matches!(range, Some((s, e)) if date > s && date < e);
        let on_pick = on_pick.clone();
        let elem_id = if id_prefix.is_empty() {
            SharedString::from(format!("day-{date}"))
        } else {
            SharedString::from(format!("{id_prefix}-day-{date}"))
        };

        let mut cell = div()
            .id(elem_id)
            .when_some(debug_prefix, |el, dbg_prefix| {
                el.debug_selector(move || format!("{dbg_prefix}-day-{date}"))
            });

        if let Some(h) = cell_height {
            cell = cell.flex_1().h(h);
        } else {
            cell = cell.size(cell_size);
        }

        let cell = cell
            .flex()
            .items_center()
            .justify_center()
            .when(rounded_full, |el| el.rounded_full())
            .when(!rounded_full, |el| el.rounded(px(6.)))
            .cursor_pointer()
            .when(fade_outside && !in_month, |el| el.opacity(0.4))
            .when(is_selected, |el| el.bg(accent))
            .when(is_other_endpoint, |el| el.bg(endpoint_bg))
            .when(!wrap_span && in_range, |el| el.bg(range_bg))
            .when(highlight_today_bg && !is_selected && !is_other_endpoint && !in_range && is_today, |el| {
                el.bg(today_bg)
            })
            .when(!is_selected, |el| el.hover(|s| s.bg(hover_bg)))
            .child(
                Label::new(date.day().to_string())
                    .centered()
                    .size(label_size)
                    .color(if is_selected {
                        Color::OnAccent
                    } else if (accent_today_text && is_today) || is_other_endpoint || (wrap_span && in_range) {
                        Color::Accent
                    } else if !fade_outside && !in_month {
                        Color::Muted
                    } else {
                        Color::Default
                    }),
            )
            .on_click(move |_, window, cx| on_pick(date, window, cx));

        div()
            .flex_1()
            .flex()
            .justify_center()
            .when(wrap_span && in_range, |el| el.bg(range_bg))
            .child(cell)
            .into_any_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use fern_i18n::{EN, Locale};
    use std::cell::Cell;
    use std::rc::Rc;

    #[gpui::test]
    fn month_calendar_respects_week_start_and_dispatches_day_clicks(cx: &mut gpui::TestAppContext) {
        let clicked = Rc::new(Cell::new(None));
        let month = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        let selected = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();

        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(crate::i18n::CurrentLocale(Locale::En));
            let prefs = fern_core::Preferences {
                week_start: chrono::Weekday::Mon,
                ..Default::default()
            };
            crate::ActivePreferences::sync_from_preferences(&prefs, false, cx);
        });

        let clicked_for_view = Rc::clone(&clicked);
        struct TestCalendar {
            month: NaiveDate,
            today: NaiveDate,
            selected: NaiveDate,
            clicked: Rc<Cell<Option<NaiveDate>>>,
        }
        impl Render for TestCalendar {
            fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let mut cfg = MonthGridConfig::single("test-cal", self.month, self.today);
                cfg.selected = Some(self.selected);
                cfg.debug_prefix = Some("test-cal");
                let on_pick = Rc::clone(&self.clicked);
                v_flex()
                    .child(render_month_header(
                        MonthHeaderConfig::surface(self.month, &EN, "test-prev", "test-next"),
                        cx,
                        |_, _, _| {},
                        None,
                    ))
                    .child(render_weekday_row(&EN, WeekdayStyle::Short, false, cx))
                    .children(render_month_grid(&cfg, cx, move |date, _, _| {
                        on_pick.set(Some(date));
                    }))
            }
        }

        let (_, cx) = cx.add_window_view(|_, _| TestCalendar {
            month,
            today,
            selected,
            clicked: clicked_for_view,
        });
        cx.simulate_resize(gpui::size(px(320.), px(320.)));
        cx.run_until_parked();

        let target = cx.debug_bounds("test-cal-day-2026-11-20").expect("selected day rendered");
        cx.simulate_click(target.center(), gpui::Modifiers::default());
        assert_eq!(clicked.get(), Some(selected));
    }
}
