//! Pure calendar and continuous-timeline math for the Life Clock view.

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveDateTime, Timelike};

/// Largest `f64` strictly less than `1.0` (`1.0 - 2^-53`), ensuring progress stays in `[0.0, 1.0)`.
const MAX_PROGRESS: f64 = 1.0 - f64::EPSILON / 2.0;

/// Calendar-accurate age split into years, months, days, and wall-clock time components.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgeBreakdown {
    pub years: u32,
    pub months: u32,
    pub days: u32,
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
}

/// Computes calendar-accurate age from `birth_date` to `now`.
///
/// Returns `None` if `now.date() < birth_date`.
pub fn compute_age(birth_date: NaiveDate, now: NaiveDateTime) -> Option<AgeBreakdown> {
    let today = now.date();
    if today < birth_date {
        return None;
    }

    let mut total_months = (today.year() - birth_date.year()) * 12
        + (today.month() as i32 - birth_date.month() as i32);
    let mut anchor = birth_date.checked_add_months(Months::new(total_months.max(0) as u32))?;
    if total_months > 0 && anchor > today {
        total_months -= 1;
        anchor = birth_date.checked_add_months(Months::new(total_months as u32))?;
    }

    let total_months = total_months.max(0) as u32;
    let days = (today - anchor).num_days().max(0) as u32;
    let time = now.time();

    Some(AgeBreakdown {
        years: total_months / 12,
        months: total_months % 12,
        days,
        hours: time.hour(),
        minutes: time.minute(),
        seconds: time.second(),
    })
}

/// Returns `true` if `year` is a Gregorian leap year.
pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// One of the six continuously ticking scales in the Life Clock timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimelineUnit {
    Second,
    Minute,
    Hour,
    Day,
    Month,
    Year,
}

impl TimelineUnit {
    pub const ALL: [Self; 6] = [
        Self::Second,
        Self::Minute,
        Self::Hour,
        Self::Day,
        Self::Month,
        Self::Year,
    ];
}

/// Instantaneous snapshot of wall-clock time for rendering the six Life Clock scales.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineSnapshot {
    pub now: NaiveDateTime,
    pub sub_sec: f64,
}

impl TimelineSnapshot {
    pub fn from_datetime(now: NaiveDateTime) -> Self {
        let sub_sec = f64::from(now.nanosecond().min(999_999_999)) / 1_000_000_000.0;
        Self { now, sub_sec }
    }

    /// Fractional progress through the current `unit`, guaranteed to lie in `[0.0, 1.0)`.
    pub fn progress(&self, unit: TimelineUnit) -> f64 {
        let raw = match unit {
            TimelineUnit::Second => self.sub_sec,
            TimelineUnit::Minute => (f64::from(self.now.second()) + self.sub_sec) / 60.0,
            TimelineUnit::Hour => {
                (f64::from(self.now.minute()) + self.progress(TimelineUnit::Minute)) / 60.0
            }
            TimelineUnit::Day => {
                (f64::from(self.now.hour()) + self.progress(TimelineUnit::Hour)) / 24.0
            }
            TimelineUnit::Month => {
                (f64::from(self.now.day() - 1) + self.progress(TimelineUnit::Day))
                    / f64::from(crate::countdown::days_in_month(self.now.year(), self.now.month()))
            }
            TimelineUnit::Year => {
                let days_in_year = if is_leap_year(self.now.year()) { 366.0 } else { 365.0 };
                (f64::from(self.now.ordinal0()) + self.progress(TimelineUnit::Day)) / days_in_year
            }
        };
        raw.clamp(0.0, MAX_PROGRESS)
    }

    /// Formatted label for the tick at `offset` units relative to `self.now`.
    pub fn token_label(&self, unit: TimelineUnit, offset: i64, months_short: &[&str; 12]) -> String {
        match unit {
            TimelineUnit::Second => {
                let val = (i64::from(self.now.second()) + offset.rem_euclid(60)).rem_euclid(60);
                format!("{val:02}")
            }
            TimelineUnit::Minute => {
                let val = (i64::from(self.now.minute()) + offset.rem_euclid(60)).rem_euclid(60);
                format!("{val:02}")
            }
            TimelineUnit::Hour => {
                let val = (i64::from(self.now.hour()) + offset.rem_euclid(24)).rem_euclid(24);
                format!("{val:02}")
            }
            TimelineUnit::Day => {
                let today = self.now.date();
                let date = Duration::try_days(offset)
                    .and_then(|delta| today.checked_add_signed(delta))
                    .unwrap_or(today);
                format!("{:02}", date.day())
            }
            TimelineUnit::Month => {
                let idx = (i64::from(self.now.month0()) + offset.rem_euclid(12)).rem_euclid(12) as usize;
                months_short[idx].to_owned()
            }
            TimelineUnit::Year => {
                let delta = offset.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
                self.now.year().saturating_add(delta).to_string()
            }
        }
    }
}

/// Strictly parses an ISO `YYYY-MM-DD` date string (ignoring surrounding whitespace).
pub fn parse_birth_date(value: &str) -> Option<NaiveDate> {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes[0..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").ok()
}

/// Formats a date as ISO `YYYY-MM-DD`.
pub fn format_birth_date(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONTHS_EN: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("valid test date")
    }

    fn dt(y: i32, m: u32, day: u32, h: u32, min: u32, s: u32) -> NaiveDateTime {
        d(y, m, day).and_hms_opt(h, min, s).expect("valid test datetime")
    }

    fn dt_nano(y: i32, m: u32, day: u32, h: u32, min: u32, s: u32, nano: u32) -> NaiveDateTime {
        d(y, m, day).and_hms_nano_opt(h, min, s, nano).expect("valid test datetime with nanos")
    }

    #[test]
    fn compute_age_exact_reference_case() {
        let birth = d(1998, 11, 20);
        let now = dt(2026, 11, 23, 3, 53, 16);
        assert_eq!(
            compute_age(birth, now),
            Some(AgeBreakdown {
                years: 28,
                months: 0,
                days: 3,
                hours: 3,
                minutes: 53,
                seconds: 16,
            })
        );
    }

    #[test]
    fn compute_age_same_day_and_future_birth_date() {
        let birth = d(2026, 11, 23);
        assert_eq!(
            compute_age(birth, dt(2026, 11, 23, 0, 0, 0)),
            Some(AgeBreakdown::default())
        );
        assert_eq!(
            compute_age(birth, dt(2026, 11, 23, 14, 25, 36)),
            Some(AgeBreakdown {
                years: 0,
                months: 0,
                days: 0,
                hours: 14,
                minutes: 25,
                seconds: 36,
            })
        );
        assert_eq!(compute_age(d(2026, 11, 24), dt(2026, 11, 23, 23, 59, 59)), None);
        assert_eq!(compute_age(d(2030, 1, 1), dt(2026, 11, 23, 0, 0, 0)), None);
    }

    #[test]
    fn compute_age_feb_29_leap_year_birthdays() {
        let birth = d(2000, 2, 29);

        // Leap year 2024: Feb 28 -> 23y 11m 30d; Feb 29 -> 24y 0m 0d; Mar 1 -> 24y 0m 1d.
        assert_eq!(
            compute_age(birth, dt(2024, 2, 28, 12, 0, 0)),
            Some(AgeBreakdown { years: 23, months: 11, days: 30, hours: 12, minutes: 0, seconds: 0 })
        );
        assert_eq!(
            compute_age(birth, dt(2024, 2, 29, 0, 0, 0)),
            Some(AgeBreakdown { years: 24, months: 0, days: 0, hours: 0, minutes: 0, seconds: 0 })
        );
        assert_eq!(
            compute_age(birth, dt(2024, 3, 1, 8, 30, 5)),
            Some(AgeBreakdown { years: 24, months: 0, days: 1, hours: 8, minutes: 30, seconds: 5 })
        );

        // Non-leap year 2025: Feb 27 -> 24y 11m 29d; Feb 28 -> 25y 0m 0d; Mar 1 -> 25y 0m 1d.
        assert_eq!(
            compute_age(birth, dt(2025, 2, 27, 6, 15, 0)),
            Some(AgeBreakdown { years: 24, months: 11, days: 29, hours: 6, minutes: 15, seconds: 0 })
        );
        assert_eq!(
            compute_age(birth, dt(2025, 2, 28, 6, 15, 0)),
            Some(AgeBreakdown { years: 25, months: 0, days: 0, hours: 6, minutes: 15, seconds: 0 })
        );
        assert_eq!(
            compute_age(birth, dt(2025, 3, 1, 6, 15, 0)),
            Some(AgeBreakdown { years: 25, months: 0, days: 1, hours: 6, minutes: 15, seconds: 0 })
        );
    }

    #[test]
    fn compute_age_jan_31_month_end_boundaries() {
        let birth = d(2026, 1, 31);
        assert_eq!(
            compute_age(birth, dt(2026, 2, 27, 10, 0, 0)),
            Some(AgeBreakdown { years: 0, months: 0, days: 27, hours: 10, minutes: 0, seconds: 0 })
        );
        assert_eq!(
            compute_age(birth, dt(2026, 2, 28, 10, 0, 0)),
            Some(AgeBreakdown { years: 0, months: 1, days: 0, hours: 10, minutes: 0, seconds: 0 })
        );
        assert_eq!(
            compute_age(birth, dt(2026, 3, 1, 10, 0, 0)),
            Some(AgeBreakdown { years: 0, months: 1, days: 1, hours: 10, minutes: 0, seconds: 0 })
        );

        // In a leap year (2024), Feb has 29 days: Feb 28 is 28 days, Feb 29 is 1 month, Mar 1 is 1m 1d.
        let leap_birth = d(2024, 1, 31);
        assert_eq!(
            compute_age(leap_birth, dt(2024, 2, 28, 0, 0, 0)),
            Some(AgeBreakdown { years: 0, months: 0, days: 28, hours: 0, minutes: 0, seconds: 0 })
        );
        assert_eq!(
            compute_age(leap_birth, dt(2024, 2, 29, 0, 0, 0)),
            Some(AgeBreakdown { years: 0, months: 1, days: 0, hours: 0, minutes: 0, seconds: 0 })
        );
        assert_eq!(
            compute_age(leap_birth, dt(2024, 3, 1, 0, 0, 0)),
            Some(AgeBreakdown { years: 0, months: 1, days: 1, hours: 0, minutes: 0, seconds: 0 })
        );
    }

    #[test]
    fn leap_year_rules() {
        assert!(is_leap_year(2000));
        assert!(is_leap_year(2024));
        assert!(is_leap_year(2028));
        assert!(!is_leap_year(1900));
        assert!(!is_leap_year(2100));
        assert!(!is_leap_year(2025));
        assert!(!is_leap_year(2026));
    }

    #[test]
    fn token_label_wrapping_across_boundaries() {
        // Nov 30 -> Dec 01 wrapping
        let nov30 = TimelineSnapshot::from_datetime(dt(2026, 11, 30, 23, 59, 59));
        assert_eq!(nov30.token_label(TimelineUnit::Day, 0, &MONTHS_EN), "30");
        assert_eq!(nov30.token_label(TimelineUnit::Day, 1, &MONTHS_EN), "01");
        assert_eq!(nov30.token_label(TimelineUnit::Day, -1, &MONTHS_EN), "29");
        assert_eq!(nov30.token_label(TimelineUnit::Hour, 0, &MONTHS_EN), "23");
        assert_eq!(nov30.token_label(TimelineUnit::Hour, 1, &MONTHS_EN), "00");
        assert_eq!(nov30.token_label(TimelineUnit::Minute, 1, &MONTHS_EN), "00");
        assert_eq!(nov30.token_label(TimelineUnit::Second, 1, &MONTHS_EN), "00");

        let dec01 = TimelineSnapshot::from_datetime(dt(2026, 12, 1, 0, 0, 0));
        assert_eq!(dec01.token_label(TimelineUnit::Day, -1, &MONTHS_EN), "30");
        assert_eq!(dec01.token_label(TimelineUnit::Hour, -1, &MONTHS_EN), "23");
        assert_eq!(dec01.token_label(TimelineUnit::Minute, -1, &MONTHS_EN), "59");
        assert_eq!(dec01.token_label(TimelineUnit::Second, -1, &MONTHS_EN), "59");

        // Feb 28 (non-leap) -> Mar 01 wrapping
        let feb28_non_leap = TimelineSnapshot::from_datetime(dt(2025, 2, 28, 12, 0, 0));
        assert_eq!(feb28_non_leap.token_label(TimelineUnit::Day, 0, &MONTHS_EN), "28");
        assert_eq!(feb28_non_leap.token_label(TimelineUnit::Day, 1, &MONTHS_EN), "01");

        // Feb 28 / Feb 29 (leap year) -> Mar 01 wrapping
        let feb28_leap = TimelineSnapshot::from_datetime(dt(2024, 2, 28, 12, 0, 0));
        assert_eq!(feb28_leap.token_label(TimelineUnit::Day, 1, &MONTHS_EN), "29");
        assert_eq!(feb28_leap.token_label(TimelineUnit::Day, 2, &MONTHS_EN), "01");

        let feb29_leap = TimelineSnapshot::from_datetime(dt(2024, 2, 29, 12, 0, 0));
        assert_eq!(feb29_leap.token_label(TimelineUnit::Day, 0, &MONTHS_EN), "29");
        assert_eq!(feb29_leap.token_label(TimelineUnit::Day, 1, &MONTHS_EN), "01");
        assert_eq!(feb29_leap.token_label(TimelineUnit::Day, -1, &MONTHS_EN), "28");

        // Dec -> Jan and Jan -> Dec month wrapping + year offset
        let dec = TimelineSnapshot::from_datetime(dt(2026, 12, 15, 0, 0, 0));
        assert_eq!(dec.token_label(TimelineUnit::Month, 0, &MONTHS_EN), "Dec");
        assert_eq!(dec.token_label(TimelineUnit::Month, 1, &MONTHS_EN), "Jan");
        assert_eq!(dec.token_label(TimelineUnit::Month, 2, &MONTHS_EN), "Feb");
        assert_eq!(dec.token_label(TimelineUnit::Year, 0, &MONTHS_EN), "2026");
        assert_eq!(dec.token_label(TimelineUnit::Year, 1, &MONTHS_EN), "2027");
        assert_eq!(dec.token_label(TimelineUnit::Year, -2, &MONTHS_EN), "2024");

        let jan = TimelineSnapshot::from_datetime(dt(2026, 1, 15, 0, 0, 0));
        assert_eq!(jan.token_label(TimelineUnit::Month, 0, &MONTHS_EN), "Jan");
        assert_eq!(jan.token_label(TimelineUnit::Month, -1, &MONTHS_EN), "Dec");
        assert_eq!(jan.token_label(TimelineUnit::Month, -13, &MONTHS_EN), "Dec");

        // Extreme offsets never panic.
        assert_eq!(jan.token_label(TimelineUnit::Day, i64::MAX, &MONTHS_EN), "15");
        assert_eq!(jan.token_label(TimelineUnit::Day, i64::MIN, &MONTHS_EN), "15");
        assert_eq!(jan.token_label(TimelineUnit::Year, i64::MAX, &MONTHS_EN), i32::MAX.to_string());
    }

    #[test]
    fn progress_bounds_and_values() {
        let samples = [
            dt_nano(2026, 1, 1, 0, 0, 0, 0),
            dt_nano(2026, 6, 15, 12, 30, 30, 500_000_000),
            dt_nano(2026, 12, 31, 23, 59, 59, 999_999_999),
            dt_nano(2024, 2, 29, 23, 59, 59, 999_999_999),
            dt_nano(2024, 12, 31, 23, 59, 59, 999_999_999),
            // Chrono represents leap seconds with nanos in 1_000_000_000..2_000_000_000.
            dt_nano(2026, 12, 31, 23, 59, 59, 1_500_000_000),
        ];

        for sample in samples {
            let snap = TimelineSnapshot::from_datetime(sample);
            assert!((0.0..1.0).contains(&snap.sub_sec));
            for unit in TimelineUnit::ALL {
                let p = snap.progress(unit);
                assert!(
                    (0.0..1.0).contains(&p),
                    "progress out of [0.0, 1.0) for {unit:?} at {sample}: {p}"
                );
            }
        }

        // Exact zero at Jan 1 00:00:00.000
        let start = TimelineSnapshot::from_datetime(dt_nano(2026, 1, 1, 0, 0, 0, 0));
        for unit in TimelineUnit::ALL {
            assert_eq!(start.progress(unit), 0.0);
        }

        // Exact midpoint check: 2026-01-01 12:00:00.500
        let noon = TimelineSnapshot::from_datetime(dt_nano(2026, 1, 1, 12, 0, 0, 500_000_000));
        assert!((noon.progress(TimelineUnit::Second) - 0.5).abs() < 1e-12);
        assert!((noon.progress(TimelineUnit::Minute) - (0.5 / 60.0)).abs() < 1e-12);
        assert!((noon.progress(TimelineUnit::Day) - (12.0 + 0.5 / 3600.0) / 24.0).abs() < 1e-12);
        assert!((noon.progress(TimelineUnit::Month) - noon.progress(TimelineUnit::Day) / 31.0).abs() < 1e-12);
        assert!((noon.progress(TimelineUnit::Year) - noon.progress(TimelineUnit::Day) / 365.0).abs() < 1e-12);

        // Leap year uses 366 days for Year progress and 29 days for February progress.
        let leap_mid_feb = TimelineSnapshot::from_datetime(dt_nano(2024, 2, 29, 0, 0, 0, 0));
        assert!((leap_mid_feb.progress(TimelineUnit::Month) - 28.0 / 29.0).abs() < 1e-12);
        assert!((leap_mid_feb.progress(TimelineUnit::Year) - 59.0 / 366.0).abs() < 1e-12);
    }

    #[test]
    fn birth_date_parse_and_format_roundtrip() {
        let date = d(1998, 11, 20);
        let formatted = format_birth_date(date);
        assert_eq!(formatted, "1998-11-20");
        assert_eq!(parse_birth_date(&formatted), Some(date));
        assert_eq!(parse_birth_date("  1998-11-20 \n"), Some(date));
        assert_eq!(parse_birth_date("2000-02-29"), Some(d(2000, 2, 29)));

        // Strict ISO YYYY-MM-DD rejects invalid calendar dates and non-ISO formats.
        assert_eq!(parse_birth_date("2025-02-29"), None);
        assert_eq!(parse_birth_date("1998-1-20"), None);
        assert_eq!(parse_birth_date("1998-11-2"), None);
        assert_eq!(parse_birth_date("98-11-20"), None);
        assert_eq!(parse_birth_date("1998/11/20"), None);
        assert_eq!(parse_birth_date("1998-11-20T00:00:00"), None);
        assert_eq!(parse_birth_date(""), None);
    }
}
