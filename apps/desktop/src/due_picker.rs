//! State and labels of the due-date popover, matched to TickTick's: the user edits a draft
//! (date, time, repeat) and nothing is saved until "OK".

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveTime};
use fern_core::reminder::{ALL_DAY_OFFSETS, ALL_DAY_REMINDER_HOUR, TIMED_OFFSETS};
use fern_core::{Frequency, Recurrence, Task};

use crate::calendar;
use crate::i18n::{Locale, Strings};

/// A sub-menu expanded inside the popover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DueMenu {
    Time,
    Reminder,
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
    let options = reminder_options(has_time, strings);
    let labels: Vec<String> = reminders
        .iter()
        .map(|m| {
            options.iter().find(|(o, _, _)| o == m).map_or_else(|| format!("-{m}m"), |(_, label, _)| (*label).to_owned())
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
