//! Repeating tasks. Rules are stored as a subset of RFC 5545 RRULE (`FREQ`, `INTERVAL` and, for
//! weekly rules, `BYDAY`),
//! the format TickTick uses too, so imports keep working and richer rules can come later.

use std::fmt;
use std::str::FromStr;

use chrono::{Datelike, Duration, Months, NaiveDate, Weekday};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Frequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Recurrence {
    pub frequency: Frequency,
    pub interval: u32,
    /// For weekly rules: the weekdays it falls on, bit 0 = Monday. Empty means "the same weekday".
    pub weekdays: u8,
}

const BYDAY_CODES: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

impl Recurrence {
    pub fn new(frequency: Frequency, interval: u32) -> Self {
        Self { frequency, interval: interval.max(1), weekdays: 0 }
    }

    /// Monday to Friday, every week.
    pub fn weekdays() -> Self {
        Self { frequency: Frequency::Weekly, interval: 1, weekdays: 0b0001_1111 }
    }

    pub fn is_weekdays(&self) -> bool {
        self.frequency == Frequency::Weekly && self.interval == 1 && self.weekdays == 0b0001_1111
    }

    pub fn falls_on(&self, day: Weekday) -> bool {
        self.weekdays & (1 << day.num_days_from_monday()) != 0
    }

    /// The next occurrence after `from`. Months clamp to the last day: Jan 31 → Feb 28.
    pub fn next_after(&self, from: NaiveDate) -> NaiveDate {
        let n = self.interval.max(1);
        match self.frequency {
            Frequency::Daily => from + Duration::days(n as i64),
            // With BYDAY: the next listed weekday later in the same week, otherwise the first
            // listed weekday `interval` weeks on (weeks start on Monday).
            Frequency::Weekly if self.weekdays != 0 => {
                let monday = from - Duration::days(from.weekday().num_days_from_monday() as i64);
                let later_this_week = (from.weekday().num_days_from_monday() as i64 + 1..7)
                    .map(|offset| monday + Duration::days(offset))
                    .find(|day| self.falls_on(day.weekday()));
                later_this_week.unwrap_or_else(|| {
                    let next = monday + Duration::weeks(n as i64);
                    (0..7)
                        .map(|offset| next + Duration::days(offset))
                        .find(|day| self.falls_on(day.weekday()))
                        .unwrap_or(next)
                })
            }
            Frequency::Weekly => from + Duration::weeks(n as i64),
            Frequency::Monthly => from.checked_add_months(Months::new(n)).unwrap_or(from),
            Frequency::Yearly => from.checked_add_months(Months::new(12 * n)).unwrap_or(from),
        }
    }
}

impl fmt::Display for Recurrence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let frequency = match self.frequency {
            Frequency::Daily => "DAILY",
            Frequency::Weekly => "WEEKLY",
            Frequency::Monthly => "MONTHLY",
            Frequency::Yearly => "YEARLY",
        };
        write!(f, "FREQ={frequency};INTERVAL={}", self.interval)?;
        if self.weekdays != 0 {
            let days: Vec<&str> =
                (0..7).filter(|i| self.weekdays & (1 << i) != 0).map(|i| BYDAY_CODES[i]).collect();
            write!(f, ";BYDAY={}", days.join(","))?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParseRecurrenceError(String);

impl fmt::Display for ParseRecurrenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsupported repeat rule: {}", self.0)
    }
}

impl std::error::Error for ParseRecurrenceError {}

impl FromStr for Recurrence {
    type Err = ParseRecurrenceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rule = s.trim().trim_start_matches("RRULE:");
        let error = || ParseRecurrenceError(rule.to_owned());
        let mut frequency = None;
        let mut interval = 1;
        let mut weekdays = 0u8;
        for part in rule.split(';').filter(|p| !p.is_empty()) {
            let (key, value) = part.split_once('=').ok_or_else(error)?;
            match key {
                "FREQ" => {
                    frequency = Some(match value {
                        "DAILY" => Frequency::Daily,
                        "WEEKLY" => Frequency::Weekly,
                        "MONTHLY" => Frequency::Monthly,
                        "YEARLY" => Frequency::Yearly,
                        _ => return Err(error()),
                    })
                }
                "INTERVAL" => interval = value.parse().map_err(|_| error())?,
                "BYDAY" => {
                    for code in value.split(',') {
                        // Drop ordinal prefixes such as "1MO"; they are not supported yet.
                        let code = code.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-' || c == '+');
                        let index = BYDAY_CODES.iter().position(|c| *c == code).ok_or_else(error)?;
                        weekdays |= 1 << index;
                    }
                }
                // Other RRULE parts (BYDAY, BYMONTHDAY, …) are ignored for now.
                _ => {}
            }
        }
        let mut rule = Recurrence::new(frequency.ok_or_else(error)?, interval);
        if rule.frequency == Frequency::Weekly {
            rule.weekdays = weekdays;
        }
        Ok(rule)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn roundtrip_and_ticktick_rules() {
        let rule = Recurrence::new(Frequency::Weekly, 2);
        assert_eq!(rule.to_string().parse::<Recurrence>(), Ok(rule));
        assert_eq!(
            "RRULE:FREQ=MONTHLY;INTERVAL=1;BYMONTHDAY=13".parse::<Recurrence>(),
            Ok(Recurrence::new(Frequency::Monthly, 1))
        );
        assert!("FREQ=HOURLY".parse::<Recurrence>().is_err());
        assert!("INTERVAL=2".parse::<Recurrence>().is_err());
    }

    #[test]
    fn every_weekday() {
        let rule = Recurrence::weekdays();
        assert_eq!(rule.to_string(), "FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,TU,WE,TH,FR");
        assert_eq!(rule.to_string().parse::<Recurrence>(), Ok(rule));
        assert!(rule.is_weekdays());
        // Friday → Monday, Saturday → Monday, Monday → Tuesday.
        assert_eq!(rule.next_after(d(2026, 10, 23)), d(2026, 10, 26));
        assert_eq!(rule.next_after(d(2026, 10, 24)), d(2026, 10, 26));
        assert_eq!(rule.next_after(d(2026, 10, 26)), d(2026, 10, 27));
    }

    #[test]
    fn next_occurrences() {
        assert_eq!(Recurrence::new(Frequency::Daily, 3).next_after(d(2026, 10, 30)), d(2026, 11, 2));
        assert_eq!(Recurrence::new(Frequency::Weekly, 1).next_after(d(2026, 10, 18)), d(2026, 10, 25));
        assert_eq!(Recurrence::new(Frequency::Monthly, 1).next_after(d(2026, 1, 31)), d(2026, 2, 28));
        assert_eq!(Recurrence::new(Frequency::Yearly, 1).next_after(d(2028, 2, 29)), d(2029, 2, 28));
    }

    #[test]
    fn every_other_week_on_some_days() {
        // Mondays and Wednesdays every 2 weeks; 2026-10-19 is a Monday.
        let rule = Recurrence { frequency: Frequency::Weekly, interval: 2, weekdays: 0b101 };
        assert_eq!(rule.to_string(), "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE");
        assert_eq!(rule.next_after(d(2026, 10, 19)), d(2026, 10, 21));
        assert_eq!(rule.next_after(d(2026, 10, 21)), d(2026, 11, 2));
        assert_eq!(rule.next_after(d(2026, 10, 25)), d(2026, 11, 2));
    }
}
