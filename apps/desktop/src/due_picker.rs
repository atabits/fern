//! State and labels of the due-date popover, matched to TickTick's: the user edits a draft
//! (date, time, repeat) and nothing is saved until "OK".

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveTime, Timelike};
use fern_core::reminder::{ALL_DAY_OFFSETS, ALL_DAY_REMINDER_HOUR, TIMED_OFFSETS};
use fern_core::{Frequency, Recurrence, Task};

use crate::calendar;
use crate::i18n::{Locale, Strings};

/// A sub-menu expanded inside the popover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DueMenu {
    Time,
    Reminder,
    /// The custom reminder editor.
    CustomReminder(CustomReminder),
    Repeat,
    /// The custom repeat editor with the rule being edited.
    Custom(Recurrence),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DueDraft {
    pub date: Option<NaiveDate>,
    pub time: Option<NaiveTime>,
    pub repeat: Option<Recurrence>,
    /// Minutes before the due time, see `fern_core::reminder`.
    pub reminders: Vec<i64>,
    /// The month shown in the calendar.
    pub month: NaiveDate,
    pub menu: Option<DueMenu>,
}

impl DueDraft {
    pub fn from_task(task: &Task, today: NaiveDate) -> Self {
        Self {
            date: task.due_date,
            time: task.due_time,
            repeat: task.repeat,
            reminders: task.reminders.clone(),
            month: calendar::month_start(task.due_date.unwrap_or(today)),
            menu: None,
        }
    }

    pub fn pick_date(&mut self, date: NaiveDate) {
        self.date = Some(date);
        self.month = calendar::month_start(date);
    }

    /// Picking a time without a date schedules the draft for today, as in the original.
    pub fn pick_time(&mut self, time: Option<NaiveTime>, today: NaiveDate) {
        self.time = time;
        if time.is_some() && self.date.is_none() {
            self.date = Some(today);
        }
        self.menu = None;
    }

    /// Turns a reminder on or off; like a time, a reminder without a date schedules today.
    pub fn toggle_reminder(&mut self, minutes: i64, today: NaiveDate) {
        if let Some(at) = self.reminders.iter().position(|m| *m == minutes) {
            self.reminders.remove(at);
        } else {
            self.reminders.push(minutes);
            self.reminders.sort();
        }
        if self.date.is_none() {
            self.date = Some(today);
        }
    }

    /// Adds a custom reminder and goes back to the reminder list.
    pub fn add_reminder(&mut self, offset: i64, today: NaiveDate) {
        if !self.reminders.contains(&offset) {
            self.toggle_reminder(offset, today);
        }
        self.menu = Some(DueMenu::Reminder);
    }

    pub fn pick_repeat(&mut self, repeat: Option<Recurrence>, today: NaiveDate) {
        self.repeat = repeat;
        if repeat.is_some() && self.date.is_none() {
            self.date = Some(today);
        }
        self.menu = None;
    }
}

/// The rule the custom repeat editor starts from: the current one, or weekly on the date's day.
pub fn custom_start(repeat: Option<Recurrence>, date: NaiveDate) -> Recurrence {
    repeat.unwrap_or_else(|| {
        let mut rule = Recurrence::new(Frequency::Weekly, 1);
        rule.weekdays = 1 << date.weekday().num_days_from_monday();
        rule
    })
}

/// A time typed by hand: "7", "7:45", "745", "19.30", "7pm", "7:45 am".
pub fn parse_time(text: &str) -> Option<NaiveTime> {
    let text = text.trim().to_lowercase();
    let (text, meridiem) = match text.strip_suffix("am").or_else(|| text.strip_suffix("a")) {
        Some(rest) => (rest.trim().to_owned(), Some(false)),
        None => match text.strip_suffix("pm").or_else(|| text.strip_suffix("p")) {
            Some(rest) => (rest.trim().to_owned(), Some(true)),
            None => (text.clone(), None),
        },
    };
    let (hour, minute) = match text.split_once([':', '.', ' ']) {
        Some((h, m)) => (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?),
        None if text.len() >= 3 && text.chars().all(|c| c.is_ascii_digit()) => {
            let (h, m) = text.split_at(text.len() - 2);
            (h.parse().ok()?, m.parse().ok()?)
        }
        None => (text.parse().ok()?, 0),
    };
    let hour = match meridiem {
        Some(_) if !(1..=12).contains(&hour) => return None,
        Some(false) => hour % 12,
        Some(true) => hour % 12 + 12,
        None => hour,
    };
    NaiveTime::from_hms_opt(hour, minute, 0)
}

/// Units of a custom reminder; all-day tasks use days and weeks, timed ones minutes to days.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReminderUnit {
    Minute,
    Hour,
    Day,
    Week,
}

impl ReminderUnit {
    pub const TIMED: [Self; 3] = [Self::Minute, Self::Hour, Self::Day];
    pub const ALL_DAY: [Self; 2] = [Self::Day, Self::Week];

    fn minutes(self) -> i64 {
        match self {
            Self::Minute => 1,
            Self::Hour => 60,
            Self::Day => 24 * 60,
            Self::Week => 7 * 24 * 60,
        }
    }

    pub fn label(self, strings: &Strings) -> &'static str {
        strings.reminder_custom_units[self as usize]
    }
}

/// The custom reminder being edited: "1 day in advance at 09:00", or "30 minutes in advance".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CustomReminder {
    pub unit: ReminderUnit,
    pub amount: u32,
    /// The time of day, for all-day tasks.
    pub at: NaiveTime,
}

impl CustomReminder {
    pub fn new(has_time: bool) -> Self {
        Self {
            unit: if has_time { ReminderUnit::Minute } else { ReminderUnit::Day },
            amount: if has_time { 15 } else { 1 },
            at: NaiveTime::from_hms_opt(ALL_DAY_REMINDER_HOUR, 0, 0).unwrap_or_default(),
        }
    }

    /// Minutes before the due time (09:00 on the due day for all-day tasks), as stored.
    pub fn offset(&self, has_time: bool) -> i64 {
        let before = self.amount as i64 * self.unit.minutes();
        if has_time {
            before
        } else {
            let at = (self.at.num_seconds_from_midnight() / 60) as i64;
            before + ALL_DAY_REMINDER_HOUR as i64 * 60 - at
        }
    }
}

/// An all-day reminder offset as (days before, time of day): 1440 → (1, 09:00), -540 → (0, 18:00).
pub fn all_day_parts(offset: i64) -> (u32, NaiveTime) {
    let from_midnight = ALL_DAY_REMINDER_HOUR as i64 * 60 - offset;
    let days = if from_midnight >= 0 { 0 } else { (-from_midnight + 24 * 60 - 1) / (24 * 60) };
    let minutes = from_midnight + days * 24 * 60;
    let time = NaiveTime::from_hms_opt((minutes / 60) as u32, (minutes % 60) as u32, 0).unwrap_or_default();
    (days as u32, time)
}

/// The label of one reminder, with the time for all-day tasks: "1 day early (09:00)".
pub fn describe_reminder(offset: i64, has_time: bool, strings: &Strings) -> String {
    let fill = |template: &str, n: i64| template.replace("{n}", &n.to_string());
    let fill_days = |n: i64| if n == 1 { strings.reminders_all_day[1].to_owned() } else { fill(strings.reminder_days_early, n) };
    if has_time {
        if let Some(i) = TIMED_OFFSETS.iter().position(|o| *o == offset) {
            return strings.reminders_timed[i].to_owned();
        }
        return if offset % (24 * 60) == 0 {
            fill_days(offset / (24 * 60))
        } else if offset % 60 == 0 {
            fill(strings.reminder_hours_early, offset / 60)
        } else {
            fill(strings.reminder_minutes_early, offset)
        };
    }
    let (days, at) = all_day_parts(offset);
    let base = if days == 0 {
        strings.reminders_all_day[0].to_owned()
    } else if days == 7 {
        strings.reminders_all_day[4].to_owned()
    } else if days % 7 == 0 {
        fill(strings.reminder_weeks_early, days as i64 / 7)
    } else {
        fill_days(days as i64)
    };
    format!("{base} ({})", at.format("%H:%M"))
}

/// The reminder options for the draft, with a hint ("09:00" for all-day tasks).
pub fn reminder_options(has_time: bool, strings: &Strings) -> Vec<(i64, &'static str, String)> {
    if has_time {
        TIMED_OFFSETS.into_iter().zip(strings.reminders_timed).map(|(m, label)| (m, label, String::new())).collect()
    } else {
        let hint = format!("{ALL_DAY_REMINDER_HOUR:02}:00");
        ALL_DAY_OFFSETS.into_iter().zip(strings.reminders_all_day).map(|(m, label)| (m, label, hint.clone())).collect()
    }
}

/// The Reminder row's label: the chosen reminders, or `None` for the plain "Reminder".
pub fn reminder_label(reminders: &[i64], has_time: bool, strings: &Strings) -> Option<String> {
    if reminders.is_empty() {
        return None;
    }
    let labels: Vec<String> = reminders
        .iter()
        .map(|offset| {
            let label = describe_reminder(*offset, has_time, strings);
            // The row stays short: preset all-day reminders drop their usual "(09:00)".
            label.strip_suffix(" (09:00)").map(str::to_owned).unwrap_or(label)
        })
        .collect();
    Some(labels.join(", "))
}

/// The quick picks above the calendar: today, tomorrow, next week, next month.
pub fn quick_dates(today: NaiveDate) -> [NaiveDate; 4] {
    [
        today,
        today + Duration::days(1),
        today + Duration::days(7),
        today.checked_add_months(Months::new(1)).unwrap_or(today),
    ]
}

/// Times offered in the time list: every 30 minutes.
pub fn time_options() -> Vec<NaiveTime> {
    (0..48).filter_map(|i| NaiveTime::from_hms_opt(i / 2, (i % 2) * 30, 0)).collect()
}

/// A repeat option and its hint for the chosen date, e.g. ("Weekly", "Sunday").
pub fn repeat_options(date: NaiveDate, strings: &Strings, locale: Locale) -> Vec<(Recurrence, String, String)> {
    let weekday = strings.weekdays_full[date.weekday().num_days_from_monday() as usize];
    let day = ordinal(date.day(), locale);
    let month = strings.months_short[date.month0() as usize];
    let yearly = if strings.day_before_month { format!("{} {month}", date.day()) } else { format!("{month} {day}") };
    vec![
        (Recurrence::new(Frequency::Daily, 1), strings.repeat_daily.to_owned(), String::new()),
        (Recurrence::new(Frequency::Weekly, 1), strings.repeat_weekly.to_owned(), weekday.to_owned()),
        (Recurrence::new(Frequency::Monthly, 1), strings.repeat_monthly.to_owned(), day),
        (Recurrence::new(Frequency::Yearly, 1), strings.repeat_yearly.to_owned(), yearly),
        (Recurrence::weekdays(), strings.repeat_weekdays.to_owned(), strings.weekdays_range.to_owned()),
    ]
}

/// The label of the current repeat rule for the Repeat row.
pub fn repeat_label(repeat: &Recurrence, strings: &Strings) -> String {
    if repeat.is_weekdays() {
        return strings.repeat_weekdays.to_owned();
    }
    let unit = unit_index(repeat.frequency);
    let mut label = if repeat.interval > 1 {
        format!("{} {} {}", strings.repeat_every, repeat.interval, strings.repeat_units_plural[unit])
    } else {
        [strings.repeat_daily, strings.repeat_weekly, strings.repeat_monthly, strings.repeat_yearly][unit].to_owned()
    };
    if repeat.frequency == Frequency::Weekly && repeat.weekdays != 0 {
        let days: Vec<&str> =
            (0..7).filter(|i| repeat.weekdays & (1 << i) != 0).map(|i| strings.weekdays_min[i]).collect();
        label.push_str(strings.repeat_on);
        label.push_str(&days.join(", "));
    }
    label
}

pub const UNITS: [Frequency; 4] = [Frequency::Daily, Frequency::Weekly, Frequency::Monthly, Frequency::Yearly];

pub fn unit_index(frequency: Frequency) -> usize {
    UNITS.iter().position(|f| *f == frequency).unwrap_or(0)
}

/// "18th" in English, "18-го" in Russian.
pub fn ordinal(day: u32, locale: Locale) -> String {
    match locale {
        Locale::Ru => format!("{day}-го"),
        Locale::En => {
            let suffix = match (day % 10, day % 100) {
                (1, 11) | (2, 12) | (3, 13) => "th",
                (1, _) => "st",
                (2, _) => "nd",
                (3, _) => "rd",
                _ => "th",
            };
            format!("{day}{suffix}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::EN;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn ordinals() {
        let en = |n| ordinal(n, Locale::En);
        assert_eq!([en(1), en(2), en(3), en(4), en(11), en(12), en(13), en(21), en(22), en(31)], [
            "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "31st"
        ]);
        assert_eq!(ordinal(18, Locale::Ru), "18-го");
    }

    #[test]
    fn reminders() {
        let today = d(2026, 10, 18);
        let mut draft = DueDraft {
            date: None,
            time: None,
            repeat: None,
            reminders: Vec::new(),
            month: today,
            menu: Some(DueMenu::Reminder),
        };
        draft.toggle_reminder(24 * 60, today);
        draft.toggle_reminder(0, today);
        assert_eq!((draft.date, draft.reminders.as_slice()), (Some(today), [0, 24 * 60].as_slice()));
        assert_eq!(reminder_label(&draft.reminders, false, &EN).as_deref(), Some("On the day, 1 day early"));
        assert_eq!(reminder_label(&[30], true, &EN).as_deref(), Some("30 minutes early"));
        draft.toggle_reminder(0, today);
        assert_eq!(draft.reminders, [24 * 60]);
        assert_eq!(reminder_options(false, &EN)[0].2, "09:00");
    }

    #[test]
    fn custom_reminders() {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        let all_day = |unit, amount, at| CustomReminder { unit, amount, at }.offset(false);
        assert_eq!(all_day(ReminderUnit::Day, 1, t(9, 0)), 24 * 60);
        assert_eq!(all_day(ReminderUnit::Day, 0, t(18, 0)), -9 * 60);
        assert_eq!(all_day(ReminderUnit::Week, 1, t(8, 30)), 7 * 24 * 60 + 30);
        assert_eq!(all_day_parts(24 * 60), (1, t(9, 0)));
        assert_eq!(all_day_parts(-9 * 60), (0, t(18, 0)));
        assert_eq!(all_day_parts(7 * 24 * 60 + 30), (7, t(8, 30)));
        assert_eq!(CustomReminder { unit: ReminderUnit::Hour, amount: 2, at: t(9, 0) }.offset(true), 120);

        assert_eq!(describe_reminder(120, true, &EN), "2 hours early");
        assert_eq!(describe_reminder(45, true, &EN), "45 minutes early");
        assert_eq!(describe_reminder(-9 * 60, false, &EN), "On the day (18:00)");
        assert_eq!(describe_reminder(7 * 24 * 60 + 30, false, &EN), "1 week early (08:30)");
        assert_eq!(reminder_label(&[24 * 60, 2 * 24 * 60 - 60], false, &EN).as_deref(), Some("1 day early, 2 days early (10:00)"));
    }

    #[test]
    fn typed_times() {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0);
        assert_eq!(parse_time("7"), t(7, 0));
        assert_eq!(parse_time("7:45"), t(7, 45));
        assert_eq!(parse_time("745"), t(7, 45));
        assert_eq!(parse_time("19.30"), t(19, 30));
        assert_eq!(parse_time("7pm"), t(19, 0));
        assert_eq!(parse_time("12 am"), t(0, 0));
        assert_eq!(parse_time("7:45 PM"), t(19, 45));
        assert_eq!(parse_time("25"), None);
        assert_eq!(parse_time("13pm"), None);
        assert_eq!(parse_time("soon"), None);
    }

    #[test]
    fn repeat_labels() {
        use crate::i18n::RU;
        let weekly = |interval, weekdays| Recurrence { frequency: Frequency::Weekly, interval, weekdays };
        assert_eq!(repeat_label(&Recurrence::new(Frequency::Daily, 1), &EN), "Daily");
        assert_eq!(repeat_label(&Recurrence::new(Frequency::Daily, 3), &EN), "Every 3 days");
        assert_eq!(repeat_label(&weekly(1, 0b100_0000), &EN), "Weekly on Su");
        assert_eq!(repeat_label(&weekly(2, 0b101), &EN), "Every 2 weeks on Mo, We");
        assert_eq!(repeat_label(&weekly(2, 0b101), &RU), "Каждые 2 нед.: пн, ср");
        assert_eq!(repeat_label(&Recurrence::weekdays(), &EN), "Every Weekday");
        assert_eq!(custom_start(None, d(2026, 10, 18)), weekly(1, 0b100_0000));
    }

    #[test]
    fn repeat_hints_follow_the_date() {
        let options = repeat_options(d(2026, 10, 18), &EN, Locale::En);
        let hints: Vec<&str> = options.iter().map(|(_, _, hint)| hint.as_str()).collect();
        assert_eq!(hints, ["", "Sunday", "18th", "Oct 18th", "Mon - Fri"]);
    }

    #[test]
    fn time_or_repeat_without_a_date_means_today() {
        let today = d(2026, 10, 18);
        let mut draft = DueDraft {
            date: None,
            time: None,
            repeat: None,
            reminders: Vec::new(),
            month: today,
            menu: Some(DueMenu::Time),
        };
        draft.pick_time(NaiveTime::from_hms_opt(9, 30, 0), today);
        assert_eq!((draft.date, draft.menu), (Some(today), None));

        let mut draft =
            DueDraft { date: None, time: None, repeat: None, reminders: Vec::new(), month: today, menu: None };
        draft.pick_repeat(Some(Recurrence::weekdays()), today);
        assert_eq!(draft.date, Some(today));
        assert_eq!(time_options().len(), 48);
        assert_eq!(quick_dates(d(2026, 1, 31))[3], d(2026, 2, 28));
    }
}
