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
    /// A birthday: the date of birth, counted down to every year.
    Birthday,
    Holiday,
}

impl CountdownKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CountdownKind::Countdown => "countdown",
            CountdownKind::Anniversary => "anniversary",
            CountdownKind::Birthday => "birthday",
            CountdownKind::Holiday => "holiday",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "anniversary" => CountdownKind::Anniversary,
            "birthday" => CountdownKind::Birthday,
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
    /// The original's Repeat: the date comes back by this rule.
    pub repeat: Option<crate::recurrence::Recurrence>,
    /// When the countdown was pinned to the front of the grid.
    pub pinned_at: Option<i64>,
}

impl Countdown {
    /// The date the countdown counts to: its date, or for a repeating one the first time it
    /// comes back on or after `today`.
    pub fn next_date(&self, today: NaiveDate) -> NaiveDate {
        if self.kind == CountdownKind::Birthday && self.repeat.is_none() {
            return next_birthday(self.date, today);
        }
        let Some(repeat) = &self.repeat else { return self.date };
        let mut date = self.date;
        while date < today {
            let next = repeat.next_after(date);
            if next <= date {
                break;
            }
            date = next;
        }
        date
    }
}

impl Countdown {
    /// For a birthday, the age turned on its next date (0 before the day of birth).
    pub fn age(&self, today: NaiveDate) -> Option<u32> {
        (self.kind == CountdownKind::Birthday).then(|| (self.next_date(today).year() - self.date.year()).max(0) as u32)
    }
}

/// The first birthday on or after `today` of someone born on `born`; a 29 February birthday
/// falls on 28 February in other years.
fn next_birthday(born: NaiveDate, today: NaiveDate) -> NaiveDate {
    if born >= today {
        return born;
    }
    let on = |year: i32| {
        NaiveDate::from_ymd_opt(year, born.month(), born.day())
            .or_else(|| NaiveDate::from_ymd_opt(year, born.month(), born.day() - 1))
            .expect("a day before the 29th always exists")
    };
    let this_year = on(today.year());
    if this_year >= today { this_year } else { on(today.year() + 1) }
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

    #[test]
    fn birthdays_come_every_year() {
        let birthday = |born: NaiveDate| Countdown {
            id: uuid::Uuid::now_v7(),
            name: "Mum".into(),
            date: born,
            kind: CountdownKind::Birthday,
            icon: None,
            color: None,
            repeat: None,
            pinned_at: None,
        };
        let today = d(2026, 10, 18);
        let mum = birthday(d(1970, 12, 5));
        assert_eq!((mum.next_date(today), mum.age(today)), (d(2026, 12, 5), Some(56)));
        let past = birthday(d(1990, 3, 1));
        assert_eq!((past.next_date(today), past.age(today)), (d(2027, 3, 1), Some(37)));
        assert_eq!(birthday(d(1990, 10, 18)).next_date(today), today, "today is the birthday");
        assert_eq!(birthday(d(2000, 2, 29)).next_date(today), d(2027, 2, 28));
        assert_eq!(CountdownKind::parse(CountdownKind::Birthday.as_str()), CountdownKind::Birthday);
        assert_eq!(birthday(d(2030, 1, 1)).age(today), Some(0), "not born yet");
    }

    #[test]
    fn repeating_countdowns_come_back() {
        use crate::recurrence::{Frequency, Recurrence};
        let once = Countdown {
            id: uuid::Uuid::now_v7(),
            name: "Rent".into(),
            date: d(2026, 9, 13),
            kind: CountdownKind::Countdown,
            icon: None,
            color: None,
            repeat: None,
            pinned_at: None,
        };
        assert_eq!(once.next_date(d(2026, 10, 18)), d(2026, 9, 13), "without a repeat the date stays");
        let monthly = Countdown { repeat: Some(Recurrence::new(Frequency::Monthly, 1)), ..once.clone() };
        assert_eq!(monthly.next_date(d(2026, 10, 18)), d(2026, 11, 13));
        assert_eq!(monthly.next_date(d(2026, 10, 13)), d(2026, 10, 13), "today counts");
        assert_eq!(monthly.next_date(d(2026, 8, 1)), d(2026, 9, 13), "before the first date, the first date");
    }
}
