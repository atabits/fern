//! Date math for the due-date calendar.

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveTime, Timelike};

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

/// The weeks (Monday to Sunday) that contain days of `month`: four to six of them.
pub fn month_weeks(month: NaiveDate) -> Vec<[NaiveDate; 7]> {
    let first = month_start(month);
    let next_month = add_months(first, 1);
    let mut weeks = Vec::new();
    let mut monday = week_of(first)[0];
    while monday < next_month {
        weeks.push(week_of(monday));
        monday += Duration::days(7);
    }
    weeks
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

/// The week view's time axis, as in the original: the night is folded into two short blocks
/// (00:00–07:00 and 21:00–24:00) around one row per hour from 08:00 to 20:00.
pub mod week_axis {
    use super::*;

    pub const HOUR_HEIGHT: f32 = 60.;
    pub const FOLDED_HEIGHT: f32 = 40.;
    pub const FIRST_HOUR: u32 = 8;
    pub const LAST_HOUR: u32 = 21;

    pub fn total_height() -> f32 {
        2. * FOLDED_HEIGHT + (LAST_HOUR - FIRST_HOUR) as f32 * HOUR_HEIGHT
    }

    /// The vertical offset of `time` from the top of the axis.
    pub fn offset(time: NaiveTime) -> f32 {
        let minutes = (time.hour() * 60 + time.minute()) as f32;
        let first = (FIRST_HOUR * 60) as f32;
        let last = (LAST_HOUR * 60) as f32;
        if minutes < first {
            minutes / first * FOLDED_HEIGHT
        } else if minutes < last {
            FOLDED_HEIGHT + (minutes - first) / 60. * HOUR_HEIGHT
        } else {
            FOLDED_HEIGHT + (LAST_HOUR - FIRST_HOUR) as f32 * HOUR_HEIGHT
                + (minutes - last) / (24. * 60. - last) * FOLDED_HEIGHT
        }
    }
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
    fn month_weeks_cover_only_the_month() {
        // October 2026 spans five weeks: Sep 28 – Nov 1.
        let weeks = month_weeks(d(2026, 10, 18));
        assert_eq!(weeks.len(), 5);
        assert_eq!(weeks[0][0], d(2026, 9, 28));
        assert_eq!(weeks[4][6], d(2026, 11, 1));
        // February 2027 starts on a Monday and fits in four weeks.
        assert_eq!(month_weeks(d(2027, 2, 1)).len(), 4);
    }

    #[test]
    fn week_axis_offsets() {
        use week_axis::*;
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        assert_eq!(offset(t(0, 0)), 0.);
        assert_eq!(offset(t(4, 0)), FOLDED_HEIGHT / 2.);
        assert_eq!(offset(t(8, 0)), FOLDED_HEIGHT);
        assert_eq!(offset(t(9, 30)), FOLDED_HEIGHT + 1.5 * HOUR_HEIGHT);
        assert_eq!(offset(t(21, 0)), FOLDED_HEIGHT + 13. * HOUR_HEIGHT);
        assert!(offset(t(23, 59)) < total_height());
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
