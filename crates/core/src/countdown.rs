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

/// The original's Show: when a countdown shows in the Today smart list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShowInList {
    #[default]
    OnTheDay,
    ThreeDaysAhead,
    SevenDaysAhead,
    Always,
    Never,
}

impl ShowInList {
    pub const ALL: [Self; 5] = [Self::OnTheDay, Self::ThreeDaysAhead, Self::SevenDaysAhead, Self::Always, Self::Never];

    pub fn key(self) -> &'static str {
        ["day", "3", "7", "always", "never"][self as usize]
    }

    pub fn parse(key: &str) -> Self {
        Self::ALL.into_iter().find(|s| s.key() == key).unwrap_or_default()
    }

    /// Whether a countdown counting to `date` shows in the Today list on `today`.
    pub fn shows(self, date: NaiveDate, today: NaiveDate) -> bool {
        let ahead = (date - today).num_days();
        match self {
            Self::OnTheDay => ahead == 0,
            Self::ThreeDaysAhead => (0..=3).contains(&ahead),
            Self::SevenDaysAhead => (0..=7).contains(&ahead),
            Self::Always => true,
            Self::Never => false,
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
    /// The original's "Show Age" for a birthday: the card says the age turned.
    pub show_age: bool,
    /// The original's Reminder: days before the date to be reminded, at 09:00 (0 is the day).
    pub reminders: Vec<u32>,
    /// The reminders stay on screen until answered.
    pub constant_reminder: bool,
    /// The original's Show: when it shows in the Today list.
    pub show_in_list: ShowInList,
    /// The original's Day Calculation Mode "Standard +1 day": the date itself is day 1 of the
    /// days since it, rather than the day after.
    pub plus_one_day: bool,
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
    /// The distance to the date it counts to, with "Standard +1 day" counting the date itself
    /// as day 1 of the days since it.
    pub fn span(&self, today: NaiveDate) -> Span {
        let mut span = span(today, self.next_date(today));
        if self.plus_one_day && span.past {
            span.total_days += 1;
            span.days += 1;
        }
        span
    }

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

/// The Reminder choices the original offers, in days before the date: on the day, 1, 2 and 3
/// days early and a week early.
pub const REMINDER_DAYS: [u32; 5] = [0, 1, 2, 3, 7];

/// The original's default for a new countdown: on the day and 3 days early.
pub const DEFAULT_REMINDERS: [u32; 2] = [0, 3];

/// The time of day countdown reminders fire, as the original's "(09:00)".
pub fn reminder_time() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(9, 0, 0).expect("09:00 is a time")
}

/// Countdown reminders firing in `(after, until]`: each reminder day before the date the
/// countdown counts to (the next time a repeating one comes back), at 09:00.
pub fn reminders_between(countdowns: &[Countdown], after: chrono::NaiveDateTime, until: chrono::NaiveDateTime) -> Vec<(&Countdown, chrono::NaiveDateTime)> {
    let mut fired = Vec::new();
    for countdown in countdowns {
        // The dates it counts to around the window: from its start, and a week later, which
        // covers "1 week early" for the next time it comes back.
        let mut targets = vec![countdown.next_date(after.date()), countdown.next_date(after.date() + chrono::Duration::days(8))];
        targets.dedup();
        for target in targets {
            for &days in &countdown.reminders {
                let at = (target - chrono::Duration::days(days as i64)).and_time(reminder_time());
                if after < at && at <= until && !fired.iter().any(|(c, t): &(&Countdown, _)| c.id == countdown.id && *t == at) {
                    fired.push((countdown, at));
                }
            }
        }
    }
    fired.sort_by_key(|(_, at)| *at);
    fired
}

/// A part of a date, as the original's month / day / year fields set it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatePart {
    Month(u32),
    Day(u32),
    Year(i32),
}

/// The date with one part changed; the day is kept within the month (Jan 31 → Feb 28).
pub fn with_date_part(date: NaiveDate, part: DatePart) -> NaiveDate {
    let (mut year, mut month, mut day) = (date.year(), date.month(), date.day());
    match part {
        DatePart::Month(m) => month = m.clamp(1, 12),
        DatePart::Day(d) => day = d.max(1),
        DatePart::Year(y) => year = y,
    }
    let last = days_in_month(year, month);
    NaiveDate::from_ymd_opt(year, month, day.min(last)).unwrap_or(date)
}

/// The number of days in a month.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    let first = NaiveDate::from_ymd_opt(year, month, 1).expect("a month has a first day");
    (add_months(first, 1) - first).num_days() as u32
}

/// Reads stored reminder days ("0,3").
pub fn parse_reminders(text: &str) -> Vec<u32> {
    text.split(',').filter_map(|d| d.trim().parse().ok()).collect()
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
    fn date_parts() {
        assert_eq!(with_date_part(d(2026, 1, 31), DatePart::Month(2)), d(2026, 2, 28));
        assert_eq!(with_date_part(d(2024, 2, 29), DatePart::Year(2025)), d(2025, 2, 28));
        assert_eq!(with_date_part(d(2026, 10, 18), DatePart::Day(31)), d(2026, 10, 31));
        assert_eq!(with_date_part(d(2026, 11, 18), DatePart::Day(31)), d(2026, 11, 30));
        assert_eq!(days_in_month(2028, 2), 29);
    }

    #[test]
    fn countdown_reminders() {
        let at = |date: NaiveDate, h: u32| date.and_hms_opt(h, 0, 0).unwrap();
        let trip = Countdown {
            id: uuid::Uuid::now_v7(),
            name: "Trip".into(),
            date: d(2026, 10, 21),
            kind: CountdownKind::Countdown,
            icon: None,
            color: None,
            repeat: None,
            pinned_at: None,
            show_age: false,
            reminders: DEFAULT_REMINDERS.to_vec(),
            constant_reminder: false,
            show_in_list: ShowInList::OnTheDay,
            plus_one_day: false,
        };
        let list = [trip];
        let fired = |from: NaiveDate, to: NaiveDate| -> Vec<chrono::NaiveDateTime> {
            reminders_between(&list, at(from, 0), at(to, 23)).into_iter().map(|(_, t)| t).collect()
        };
        // 3 days early is the 18th at 09:00, the day itself the 21st.
        assert_eq!(fired(d(2026, 10, 18), d(2026, 10, 18)), [at(d(2026, 10, 18), 9)]);
        assert_eq!(fired(d(2026, 10, 17), d(2026, 10, 22)), [at(d(2026, 10, 18), 9), at(d(2026, 10, 21), 9)]);
        assert!(fired(d(2026, 10, 22), d(2026, 10, 30)).is_empty(), "past dates do not remind");
        assert_eq!(parse_reminders("0, 3,x"), [0, 3]);

        // Show in the Today list.
        let date = d(2026, 10, 21);
        assert!(ShowInList::ThreeDaysAhead.shows(date, d(2026, 10, 18)) && !ShowInList::OnTheDay.shows(date, d(2026, 10, 18)));
        assert!(ShowInList::OnTheDay.shows(date, date) && !ShowInList::SevenDaysAhead.shows(date, d(2026, 10, 22)));
        assert!(ShowInList::Always.shows(date, d(2020, 1, 1)) && !ShowInList::Never.shows(date, date));
        assert_eq!(ShowInList::parse(ShowInList::SevenDaysAhead.key()), ShowInList::SevenDaysAhead);

        // "Standard +1 day" counts the date as day 1 of the days since.
        let mut wedding = list[0].clone();
        wedding.date = d(2026, 10, 8);
        assert_eq!(wedding.span(d(2026, 10, 18)).total_days, 10);
        wedding.plus_one_day = true;
        assert_eq!(wedding.span(d(2026, 10, 18)).total_days, 11);
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
            show_age: false,
            reminders: Vec::new(),
            constant_reminder: false,
            show_in_list: ShowInList::OnTheDay,
            plus_one_day: false,
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
        assert!(!mum.show_age, "the age shows only when asked, as in the original");
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
            show_age: false,
            reminders: Vec::new(),
            constant_reminder: false,
            show_in_list: ShowInList::OnTheDay,
            plus_one_day: false,
        };
        assert_eq!(once.next_date(d(2026, 10, 18)), d(2026, 9, 13), "without a repeat the date stays");
        let monthly = Countdown { repeat: Some(Recurrence::new(Frequency::Monthly, 1)), ..once.clone() };
        assert_eq!(monthly.next_date(d(2026, 10, 18)), d(2026, 11, 13));
        assert_eq!(monthly.next_date(d(2026, 10, 13)), d(2026, 10, 13), "today counts");
        assert_eq!(monthly.next_date(d(2026, 8, 1)), d(2026, 9, 13), "before the first date, the first date");
    }
}
