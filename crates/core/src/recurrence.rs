//! Repeating tasks. Rules are stored as a subset of RFC 5545 RRULE (`FREQ`, `INTERVAL`, for
//! weekly rules `BYDAY`, and for monthly rules `BYMONTHDAY` or an ordinal `BYDAY` such as `1SU`),
//! the format TickTick uses too, so imports keep working and richer rules can come later.
//! The monthly "Workday" rule is `BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1` (or `-1`), as in RFC 5545;
//! "Skip Weekends" and repeating from the completion date have no RRULE part, so they are kept
//! as the extension parts `X-SKIP=WEEKEND` and `X-FROM=COMPLETION`.

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
    /// For monthly rules, the original's "Workday": the first (1) or last (-1) weekday, Monday to
    /// Friday, of the month.
    pub nth_workday: Option<i8>,
    /// For daily rules, the original's "Skip Weekends": a date falling on a weekend moves to the
    /// Monday after.
    pub skip_weekends: bool,
    /// The original's "By Completion Date": the next date counts from the day the task was
    /// completed rather than from its due date.
    pub from_completion: bool,
}

/// `month_days`' bit for the last day of the month.
pub const LAST_DAY: u32 = 1 << 31;

const BYDAY_CODES: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

/// Monday to Friday as `weekdays` bits.
const WORKDAYS: u8 = 0b0001_1111;

fn is_workday(day: NaiveDate) -> bool {
    !matches!(day.weekday(), Weekday::Sat | Weekday::Sun)
}

impl Recurrence {
    pub fn new(frequency: Frequency, interval: u32) -> Self {
        Self {
            frequency,
            interval: interval.max(1),
            weekdays: 0,
            month_days: 0,
            nth_weekday: None,
            nth_workday: None,
            skip_weekends: false,
            from_completion: false,
        }
    }

    /// Monday to Friday, every week.
    pub fn weekdays() -> Self {
        Self { weekdays: WORKDAYS, ..Self::new(Frequency::Weekly, 1) }
    }

    pub fn is_weekdays(&self) -> bool {
        self.frequency == Frequency::Weekly && self.interval == 1 && self.weekdays == WORKDAYS && !self.from_completion
    }

    /// The days of the month starting on `month` that a monthly "Each" or "On the" rule picks,
    /// in order.
    pub fn days_in(&self, month: NaiveDate) -> Vec<NaiveDate> {
        let first = month.with_day(1).expect("every month has a first day");
        let last = first.checked_add_months(Months::new(1)).unwrap_or(first) - Duration::days(1);
        if let Some(nth) = self.nth_workday {
            let workdays: Vec<NaiveDate> =
                first.iter_days().take_while(|d| *d <= last).filter(|d| is_workday(*d)).collect();
            let pick = if nth < 0 { workdays.last() } else { workdays.get(nth.max(1) as usize - 1) };
            return pick.copied().into_iter().collect();
        }
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

    /// The next date of a task due `due` and completed on `completed`: counted from the
    /// completion day for "By Completion Date" rules, from the due date otherwise.
    pub fn next_for(&self, due: NaiveDate, completed: NaiveDate) -> NaiveDate {
        self.next_after(if self.from_completion { completed } else { due })
    }

    /// The next occurrence after `from`. Months clamp to the last day: Jan 31 → Feb 28.
    pub fn next_after(&self, from: NaiveDate) -> NaiveDate {
        let n = self.interval.max(1);
        match self.frequency {
            Frequency::Daily if self.skip_weekends => {
                let next = from + Duration::days(n as i64);
                match next.weekday() {
                    Weekday::Sat => next + Duration::days(2),
                    Weekday::Sun => next + Duration::days(1),
                    _ => next,
                }
            }
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
            Frequency::Monthly if self.month_days != 0 || self.nth_weekday.is_some() || self.nth_workday.is_some() => {
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
        if let Some(nth) = self.nth_workday {
            write!(f, ";BYDAY=MO,TU,WE,TH,FR;BYSETPOS={nth}")?;
        }
        if self.skip_weekends {
            write!(f, ";X-SKIP=WEEKEND")?;
        }
        if self.from_completion {
            write!(f, ";X-FROM=COMPLETION")?;
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
        let mut setpos = None;
        let mut skip_weekends = false;
        let mut from_completion = false;
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
                "BYSETPOS" => setpos = Some(value.parse::<i8>().map_err(|_| error())?),
                "X-SKIP" => skip_weekends = value == "WEEKEND",
                "X-FROM" => from_completion = value == "COMPLETION",
                // Other RRULE parts are ignored for now.
                _ => {}
            }
        }
        let mut rule = Recurrence::new(frequency.ok_or_else(error)?, interval);
        rule.from_completion = from_completion;
        if rule.frequency == Frequency::Daily {
            rule.skip_weekends = skip_weekends;
        }
        if rule.frequency == Frequency::Weekly {
            rule.weekdays = weekdays;
        }
        if rule.frequency == Frequency::Monthly {
            match setpos {
                Some(nth) if weekdays == WORKDAYS => rule.nth_workday = Some(nth),
                _ => {
                    rule.month_days = month_days;
                    rule.nth_weekday = nth_weekday;
                }
            }
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

    #[test]
    fn workday_skip_weekends_and_by_completion() {
        // The first and last workday of the month.
        let first = Recurrence { nth_workday: Some(1), ..Recurrence::new(Frequency::Monthly, 1) };
        assert_eq!(first.to_string(), "FREQ=MONTHLY;INTERVAL=1;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1");
        assert_eq!(first.to_string().parse::<Recurrence>().unwrap(), first);
        // Nov 1 2026 is a Sunday: the first workday is Monday the 2nd.
        assert_eq!(first.next_after(d(2026, 10, 18)), d(2026, 11, 2));
        let last = Recurrence { nth_workday: Some(-1), ..first };
        // Oct 31 2026 is a Saturday.
        assert_eq!(last.next_after(d(2026, 10, 18)), d(2026, 10, 30));
        assert_eq!(last.next_after(d(2026, 10, 30)), d(2026, 11, 30));

        // Daily, skipping weekends: Friday → Monday, and every 2 days from Thursday → Monday.
        let daily = Recurrence { skip_weekends: true, ..Recurrence::new(Frequency::Daily, 1) };
        assert_eq!(daily.next_after(d(2026, 10, 16)), d(2026, 10, 19));
        assert_eq!(daily.next_after(d(2026, 10, 19)), d(2026, 10, 20));
        let every_two = Recurrence { interval: 2, ..daily };
        assert_eq!(every_two.next_after(d(2026, 10, 15)), d(2026, 10, 19));
        assert_eq!(daily.to_string().parse::<Recurrence>().unwrap(), daily);

        // By completion date: a weekly task due on the 10th, done on the 18th, comes back on the 25th.
        let weekly = Recurrence { from_completion: true, ..Recurrence::new(Frequency::Weekly, 1) };
        assert_eq!(weekly.next_for(d(2026, 10, 10), d(2026, 10, 18)), d(2026, 10, 25));
        assert_eq!(Recurrence::new(Frequency::Weekly, 1).next_for(d(2026, 10, 10), d(2026, 10, 18)), d(2026, 10, 17));
        assert_eq!(weekly.to_string(), "FREQ=WEEKLY;INTERVAL=1;X-FROM=COMPLETION");
        assert_eq!(weekly.to_string().parse::<Recurrence>().unwrap(), weekly);
    }
}
