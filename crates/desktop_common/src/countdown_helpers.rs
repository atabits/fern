use chrono::{Local, NaiveDate};
use fern_core::{Countdown, CountdownKind};
use fern_i18n::{self as i18n, Strings};
use gpui::FontWeight;
use ui::prelude::*;

use crate::calendar;
use crate::colors::parse_color;

/// How far the unit letters sit above the numbers' line box, to share their baseline.
const UNIT_BASELINE: f32 = 9.;

pub fn kind_icon(kind: CountdownKind) -> IconName {
    match kind {
        CountdownKind::Countdown => IconName::Hourglass,
        CountdownKind::Anniversary => IconName::Heart,
        CountdownKind::Birthday => IconName::Cake,
        CountdownKind::Holiday => IconName::Gift,
    }
}

pub fn format_date(date: NaiveDate, strings: &Strings) -> String {
    match (calendar::explicit_date_format(), strings.countdown_date) {
        (None, Some(template)) => calendar::format_day(template, date, strings),
        _ => calendar::format_date(date, strings),
    }
}

/// The color a countdown's dot and icon take: its own, or the accent.
pub fn countdown_color(color: Option<&str>, cx: &App) -> gpui::Hsla {
    parse_color(color).unwrap_or(cx.theme().colors().accent)
}

/// A countdown's round icon: its emoji on its color, or the kind's icon.
pub fn badge(countdown: &Countdown, color: gpui::Hsla, cx: &App) -> gpui::Div {
    let fill = parse_color(countdown.color.as_deref());
    // The original's 24 px round icon beside the name.
    div()
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(fill.map_or(color.opacity(0.25), |c| c.opacity(0.6)))
        .map(|el| match &countdown.icon {
            Some(icon) => el.text_size(px(14.)).child(icon.clone()),
            None => el.child(
                Icon::new(kind_icon(countdown.kind))
                    .size(IconSize::Small)
                    .color(Color::Custom(if fill.is_some() { cx.theme().colors().text } else { color })),
            ),
        })
}

/// The parts of a countdown card: its color, the "1M 1D" numbers and "Days until …".
pub struct Face {
    pub color: gpui::Hsla,
    pub numbers: ui::HorizontalStack,
    pub subtitle: String,
}

pub fn face(countdown: &Countdown, today: NaiveDate, days_only: bool, cx: &App) -> Face {
    let strings = i18n::t(cx);
    let theme = cx.theme();
    // A repeating countdown counts to the next time its date comes back.
    // A birthday counting up shows the time since the birth.
    let date = if countdown.count_up { countdown.date } else { countdown.next_date(today) };
    let span = countdown.span(today);
    // On a card's picture the numbers are white, as in the original.
    let on_picture = countdown.background.is_some();
    let color = if on_picture {
        gpui::white()
    } else if span.past {
        theme.status().warning
    } else {
        theme.colors().accent
    };
    let number = |value: u32, unit: &str| {
        // As in the original: the numbers in a light serif, the units in the UI font, on the
        // numbers' baseline.
        h_flex()
            .items_end()
            .child(div().text_size(px(46.)).line_height(px(46.)).font_family("IBM Plex Serif").text_color(color).child(value.to_string()))
            .child(div().pl_1().pr_1().mb(px(UNIT_BASELINE)).text_size(px(18.)).line_height(px(18.)).font_weight(FontWeight::MEDIUM).text_color(color).child(unit.to_owned()))
    };
    let [y, m, d] = strings.unit_letters;
    let numbers = if days_only {
        h_flex().child(number(span.total_days, d))
    } else {
        h_flex()
            .when(span.years > 0, |el| el.child(number(span.years, y)))
            .when(span.months > 0, |el| el.child(number(span.months, m)))
            .when(span.days > 0 || (span.years == 0 && span.months == 0), |el| el.child(number(span.days, d)))
    };
    let mut subtitle = format!(
        "{} {}",
        if span.past { strings.countdown_days_since } else { strings.countdown_days_until },
        format_date(date, strings)
    );
    // A birthday with "Show Age" says the age turned on that day.
    if let Some(age) = countdown.age(today).filter(|_| countdown.show_age) {
        subtitle = format!("{} · {subtitle}", strings.countdown_turns.replace("{n}", &age.to_string()));
    }
    Face { color, numbers, subtitle }
}

/// A countdown as the original shows it when clicked in the calendar's Agenda: the icon and
/// name, the time left and "Days until …", counted to `date` (this year's date for yearly ones).
pub fn render_summary(countdown: &Countdown, date: NaiveDate, cx: &App) -> AnyElement {
    let today = Local::now().date_naive();
    let shown = Countdown { date, repeat: None, ..countdown.clone() };
    let Face { color, numbers, subtitle } = face(&shown, today, false, cx);
    v_flex()
        .items_center()
        .gap_1()
        .py_3()
        .px_6()
        .child(
            h_flex()
                .gap_1p5()
                .child(badge(countdown, color, cx))
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(countdown.name.clone())),
        )
        .child(numbers)
        .child(Label::new(subtitle).size(LabelSize::XSmall).color(Color::Muted))
        .into_any_element()
}
