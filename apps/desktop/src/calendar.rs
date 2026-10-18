//! Date math for the due-date calendar.

use chrono::{Datelike, Duration, Months, NaiveDate};

use crate::i18n::Strings;

/// The first day of the month that contains `date`.
pub fn month_start(date: NaiveDate) -> NaiveDate {
    date.with_day(1).expect("every month has a first day")
}

pub fn add_months(month: NaiveDate, delta: i32) -> NaiveDate {
    let month = month_start(month);
    if delta >= 0 {
        month.checked_add_months(Months::new(delta as u32)).unwrap_or(month)
    } else {
        month.checked_sub_months(Months::new(delta.unsigned_abs())).unwrap_or(month)
    }
}

/// 42 days (six weeks starting on Monday) covering `month`.
pub fn month_grid(month: NaiveDate) -> Vec<NaiveDate> {
    let first = month_start(month);
    let offset = first.weekday().num_days_from_monday() as i64;
    let start = first - Duration::days(offset);
    (0..42).map(|i| start + Duration::days(i)).collect()
}

/// The seven days (Monday to Sunday) of the week that contains `date`.
pub fn week_of(date: NaiveDate) -> [NaiveDate; 7] {
    let monday = date - Duration::days(date.weekday().num_days_from_monday() as i64);
    std::array::from_fn(|i| monday + Duration::days(i as i64))
}

pub fn month_title(month: NaiveDate, strings: &Strings) -> String {
    format!("{} {}", strings.months[month.month0() as usize], month.year())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{EN, RU};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn grid_starts_on_monday_and_covers_month() {
        // 1 October 2026 is a Thursday.
        let grid = month_grid(d(2026, 10, 18));
        assert_eq!(grid.len(), 42);
        assert_eq!(grid[0], d(2026, 9, 28));
        assert_eq!(grid[3], d(2026, 10, 1));
        assert!(grid.contains(&d(2026, 10, 31)));
    }

    #[test]
    fn week_starts_on_monday() {
        let week = week_of(d(2026, 10, 18)); // a Sunday
        assert_eq!(week[0], d(2026, 10, 12));
        assert_eq!(week[6], d(2026, 10, 18));
        assert_eq!(week_of(d(2026, 10, 12))[0], d(2026, 10, 12));
    }

    #[test]
    fn month_navigation_and_titles() {
        assert_eq!(add_months(d(2026, 1, 31), 1), d(2026, 2, 1));
        assert_eq!(add_months(d(2026, 1, 15), -1), d(2025, 12, 1));
        assert_eq!(month_title(d(2026, 10, 18), &EN), "October 2026");
        assert_eq!(month_title(d(2026, 10, 18), &RU), "Октябрь 2026");
    }
}
