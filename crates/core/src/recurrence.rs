//! Repeating tasks. Rules are stored as a subset of RFC 5545 RRULE (`FREQ` and `INTERVAL`),
//! the format TickTick uses too, so imports keep working and richer rules can come later.

use std::fmt;
use std::str::FromStr;

use chrono::{Duration, Months, NaiveDate};

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
}

impl Recurrence {
    pub fn new(frequency: Frequency, interval: u32) -> Self {
        Self { frequency, interval: interval.max(1) }
    }

    /// The next occurrence after `from`. Months clamp to the last day: Jan 31 → Feb 28.
    pub fn next_after(&self, from: NaiveDate) -> NaiveDate {
        let n = self.interval.max(1);
        match self.frequency {
            Frequency::Daily => from + Duration::days(n as i64),
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
        write!(f, "FREQ={frequency};INTERVAL={}", self.interval)
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
                // Other RRULE parts (BYDAY, BYMONTHDAY, …) are ignored for now.
                _ => {}
            }
        }
        Ok(Recurrence::new(frequency.ok_or_else(error)?, interval))
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
    fn next_occurrences() {
        assert_eq!(Recurrence::new(Frequency::Daily, 3).next_after(d(2026, 10, 30)), d(2026, 11, 2));
        assert_eq!(Recurrence::new(Frequency::Weekly, 1).next_after(d(2026, 10, 18)), d(2026, 10, 25));
        assert_eq!(Recurrence::new(Frequency::Monthly, 1).next_after(d(2026, 1, 31)), d(2026, 2, 28));
        assert_eq!(Recurrence::new(Frequency::Yearly, 1).next_after(d(2028, 2, 29)), d(2029, 2, 28));
    }
}
