//! Countdowns and anniversaries: named dates shown as the time left until them or passed since.

use chrono::{Datelike, Months, NaiveDate};

use crate::model::Id;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CountdownKind {
    /// Counts down to a future date.
    #[default]
    Countdown,
    /// Counts the time since a date, such as a wedding.
    Anniversary,
    Holiday,
}

impl CountdownKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CountdownKind::Countdown => "countdown",
            CountdownKind::Anniversary => "anniversary",
            CountdownKind::Holiday => "holiday",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "anniversary" => CountdownKind::Anniversary,
            "holiday" => CountdownKind::Holiday,
            _ => CountdownKind::Countdown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Countdown {
    pub id: Id,
    pub name: String,
    pub date: NaiveDate,
    pub kind: CountdownKind,
    /// The original's icon, here an emoji or a short text; `None` shows the kind's icon.
    pub icon: Option<String>,
    /// The icon's background, a hex color, which the Agenda's dot takes too.
    pub color: Option<String>,
}

/// The distance between two dates as calendar years, months and days.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub years: u32,
    pub months: u32,
    pub days: u32,
    /// Total days, for the "days only" format.
    pub total_days: u32,
    /// `true` when the date is in the past.
    pub past: bool,
}

/// How far `date` is from `today`, counted in whole calendar months first, then days.
pub fn span(today: NaiveDate, date: NaiveDate) -> Span {
    let past = date < today;
    let (start, end) = if past { (date, today) } else { (today, date) };
    let total_days = (end - start).num_days() as u32;

    let mut months = (end.year() - start.year()) * 12 + end.month() as i32 - start.month() as i32;
    if months > 0 && add_months(start, months as u32) > end {
        months -= 1;
    }
    let months = months.max(0) as u32;
    let days = (end - add_months(start, months)).num_days() as u32;
    Span { years: months / 12, months: months % 12, days, total_days, past }
}

fn add_months(date: NaiveDate, months: u32) -> NaiveDate {
    date.checked_add_months(Months::new(months)).unwrap_or(date)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn ymd(span: Span) -> (u32, u32, u32, bool) {
        (span.years, span.months, span.days, span.past)
    }

    #[test]
    fn spans_until_and_since() {
        let today = d(2026, 10, 18);
        assert_eq!(ymd(span(today, d(2027, 1, 1))), (0, 2, 14, false));
        assert_eq!(ymd(span(today, d(2026, 11, 19))), (0, 1, 1, false));
        assert_eq!(ymd(span(today, d(2025, 10, 14))), (1, 0, 4, true));
        assert_eq!(ymd(span(today, d(2024, 8, 27))), (2, 1, 21, true));
        assert_eq!(ymd(span(today, today)), (0, 0, 0, false));
        assert_eq!(span(today, d(2027, 1, 1)).total_days, 75);
    }

    #[test]
    fn month_ends_do_not_overshoot() {
        // Jan 31 → Mar 1: one month (to Feb 28) and one day.
        assert_eq!(ymd(span(d(2026, 1, 31), d(2026, 3, 1))), (0, 1, 1, false));
    }
}
