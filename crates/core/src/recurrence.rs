//! Repeating tasks. Rules are stored as a subset of RFC 5545 RRULE (`FREQ`, `INTERVAL`, for
//! weekly rules `BYDAY`, and for monthly rules `BYMONTHDAY` or an ordinal `BYDAY` such as `1SU`),
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
    /// For monthly rules, the original's "Each": days of the month, bit 0 = the 1st … bit 30 =
    /// the 31st, bit 31 = the last day. Empty means "the same day".
    pub month_days: u32,
    /// For monthly rules, the original's "On the": the nth weekday (1–4, or -1 for the last).
    pub nth_weekday: Option<(i8, Weekday)>,
}

/// `month_days`' bit for the last day of the month.
pub const LAST_DAY: u32 = 1 << 31;

const BYDAY_CODES: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

impl Recurrence {
    pub fn new(frequency: Frequency, interval: u32) -> Self {
        Self { frequency, interval: interval.max(1), weekdays: 0, month_days: 0, nth_weekday: None }
    }

    /// Monday to Friday, every week.
    pub fn weekdays() -> Self {
        Self { weekdays: 0b0001_1111, ..Self::new(Frequency::Weekly, 1) }
    }

    pub fn is_weekdays(&self) -> bool {
        self.frequency == Frequency::Weekly && self.interval == 1 && self.weekdays == 0b0001_1111
    }

    /// The days of the month starting on `month` that a monthly "Each" or "On the" rule picks,
    /// in order.
    pub fn days_in(&self, month: NaiveDate) -> Vec<NaiveDate> {
        let first = month.with_day(1).expect("every month has a first day");
        let last = first.checked_add_months(Months::new(1)).unwrap_or(first) - Duration::days(1);
        if let Some((nth, weekday)) = self.nth_weekday {
            let matching: Vec<NaiveDate> =
                first.iter_days().take_while(|d| *d <= last).filter(|d| d.weekday() == weekday).collect();
            let pick = if nth < 0 { matching.last() } else { matching.get(nth as usize - 1) };
            return pick.copied().into_iter().collect();
        }
        first
            .iter_days()
            .take_while(|d| *d <= last)
            .filter(|d| self.month_days & (1 << (d.day() - 1)) != 0 || (self.month_days & LAST_DAY != 0 && *d == last))
            .collect()
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
            Frequency::Monthly if self.month_days != 0 || self.nth_weekday.is_some() => {
                // The next matching day later this month, otherwise in the month `interval` on.
                let month = from.with_day(1).expect("every month has a first day");
                let later = self.days_in(month).into_iter().find(|day| *day > from);
                later.unwrap_or_else(|| {
                    let next = month.checked_add_months(Months::new(n)).unwrap_or(month);
                    self.days_in(next).into_iter().next().unwrap_or(next)
                })
            }
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
        if self.month_days != 0 {
            let days: Vec<String> = (0..31)
                .filter(|i| self.month_days & (1 << i) != 0)
                .map(|i| (i + 1).to_string())
                .chain((self.month_days & LAST_DAY != 0).then(|| "-1".to_owned()))
                .collect();
            write!(f, ";BYMONTHDAY={}", days.join(","))?;
        }
        if let Some((nth, weekday)) = self.nth_weekday {
            write!(f, ";BYDAY={nth}{}", BYDAY_CODES[weekday.num_days_from_monday() as usize])?;
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
        let mut month_days = 0u32;
        let mut nth_weekday = None;
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
                        // An ordinal prefix ("1SU", "-1FR") is a monthly "On the" rule.
                        let day = code.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-' || c == '+');
                        let index = BYDAY_CODES.iter().position(|c| *c == day).ok_or_else(error)?;
                        let ordinal = &code[..code.len() - day.len()];
                        if ordinal.is_empty() {
                            weekdays |= 1 << index;
                        } else {
                            let nth: i8 = ordinal.trim_start_matches('+').parse().map_err(|_| error())?;
                            let weekday = Weekday::try_from(index as u8).map_err(|_| error())?;
                            nth_weekday = Some((nth, weekday));
                        }
                    }
                }
                "BYMONTHDAY" => {
                    for day in value.split(',') {
                        match day.parse::<i32>().map_err(|_| error())? {
                            -1 => month_days |= LAST_DAY,
                            d @ 1..=31 => month_days |= 1 << (d - 1),
                            _ => return Err(error()),
                        }
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
        if rule.frequency == Frequency::Monthly {
            rule.month_days = month_days;
            rule.nth_weekday = nth_weekday;
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
            Ok(Recurrence { month_days: 1 << 12, ..Recurrence::new(Frequency::Monthly, 1) })
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
        let rule = Recurrence { weekdays: 0b101, ..Recurrence::new(Frequency::Weekly, 2) };
        assert_eq!(rule.to_string(), "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE");
        assert_eq!(rule.next_after(d(2026, 10, 19)), d(2026, 10, 21));
        assert_eq!(rule.next_after(d(2026, 10, 21)), d(2026, 11, 2));
        assert_eq!(rule.next_after(d(2026, 10, 25)), d(2026, 11, 2));
    }

    #[test]
    fn monthly_each_and_on_the() {
        let d = |m, day| NaiveDate::from_ymd_opt(2026, m, day).unwrap();
        // "Each" 1st, 15th and the last day.
        let each = Recurrence { month_days: 1 | 1 << 14 | LAST_DAY, ..Recurrence::new(Frequency::Monthly, 1) };
        assert_eq!(each.next_after(d(10, 1)), d(10, 15));
        assert_eq!(each.next_after(d(10, 15)), d(10, 31));
        assert_eq!(each.next_after(d(10, 31)), d(11, 1));
        assert_eq!(each.to_string(), "FREQ=MONTHLY;INTERVAL=1;BYMONTHDAY=1,15,-1");
        assert_eq!(each.to_string().parse::<Recurrence>().unwrap(), each);

        // "On the" first Sunday, and the last Friday.
        let first_sunday = Recurrence { nth_weekday: Some((1, Weekday::Sun)), ..Recurrence::new(Frequency::Monthly, 1) };
        assert_eq!(first_sunday.next_after(d(10, 18)), d(11, 1));
        assert_eq!(first_sunday.to_string(), "FREQ=MONTHLY;INTERVAL=1;BYDAY=1SU");
        let last_friday: Recurrence = "FREQ=MONTHLY;INTERVAL=1;BYDAY=-1FR".parse().unwrap();
        assert_eq!(last_friday.nth_weekday, Some((-1, Weekday::Fri)));
        assert_eq!(last_friday.next_after(d(10, 18)), d(10, 30));
    }
}
