//! State and labels of the due-date popover, matched to TickTick's: the user edits a draft
//! (date, time, repeat) and nothing is saved until "OK".

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveTime};
use fern_core::{Frequency, Recurrence, Task};

use crate::calendar;
use crate::i18n::{Locale, Strings};

/// A sub-menu expanded inside the popover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DueMenu {
    Time,
    Repeat,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DueDraft {
    pub date: Option<NaiveDate>,
    pub time: Option<NaiveTime>,
    pub repeat: Option<Recurrence>,
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

    pub fn pick_repeat(&mut self, repeat: Option<Recurrence>, today: NaiveDate) {
        self.repeat = repeat;
        if repeat.is_some() && self.date.is_none() {
            self.date = Some(today);
        }
        self.menu = None;
    }
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
    let base = match repeat.frequency {
        Frequency::Daily => strings.repeat_daily,
        Frequency::Weekly => strings.repeat_weekly,
        Frequency::Monthly => strings.repeat_monthly,
        Frequency::Yearly => strings.repeat_yearly,
    };
    if repeat.interval > 1 { format!("{base} ×{}", repeat.interval) } else { base.to_owned() }
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
    fn repeat_hints_follow_the_date() {
        let options = repeat_options(d(2026, 10, 18), &EN, Locale::En);
        let hints: Vec<&str> = options.iter().map(|(_, _, hint)| hint.as_str()).collect();
        assert_eq!(hints, ["", "Sunday", "18th", "Oct 18th", "Mon - Fri"]);
    }

    #[test]
    fn time_or_repeat_without_a_date_means_today() {
        let today = d(2026, 10, 18);
        let mut draft = DueDraft { date: None, time: None, repeat: None, month: today, menu: Some(DueMenu::Time) };
        draft.pick_time(NaiveTime::from_hms_opt(9, 30, 0), today);
        assert_eq!((draft.date, draft.menu), (Some(today), None));

        let mut draft = DueDraft { date: None, time: None, repeat: None, month: today, menu: None };
        draft.pick_repeat(Some(Recurrence::weekdays()), today);
        assert_eq!(draft.date, Some(today));
        assert_eq!(time_options().len(), 48);
        assert_eq!(quick_dates(d(2026, 1, 31))[3], d(2026, 2, 28));
    }
}
