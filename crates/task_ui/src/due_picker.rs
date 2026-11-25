//! State and labels of the due-date popover, matched to TickTick's: the user edits a draft
//! (date, time, repeat) and nothing is saved until "OK".

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use fern_core::reminder::{ALL_DAY_OFFSETS, ALL_DAY_REMINDER_HOUR, TIMED_OFFSETS};
use fern_core::{Frequency, Horizon, Recurrence, Task};

use crate::calendar;
use crate::i18n::{Locale, Strings};

/// A sub-menu expanded inside the popover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DueMenu {
    Time,
    TimeZone,
    Reminder,
    /// The custom reminder editor.
    CustomReminder(CustomReminder),
    Repeat,
    /// The custom repeat editor with the rule being edited.
    Custom(Recurrence),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DueDraft {
    pub date: Option<NaiveDate>,
    pub time: Option<NaiveTime>,
    pub repeat: Option<Recurrence>,
    /// Minutes before the due time, see `fern_core::reminder`.
    pub reminders: Vec<i64>,
    /// Whether the user chose reminders in this picker, including switching the last one off.
    pub reminders_changed: bool,
    /// The month shown in the calendar.
    pub month: NaiveDate,
    pub menu: Option<DueMenu>,
    pub tab: DueTab,
    /// The end of a duration, on the Duration tab.
    pub end_date: Option<NaiveDate>,
    pub end_time: Option<NaiveTime>,
    /// The Duration tab's field being picked with the calendar or the time list.
    pub range_field: Option<RangeField>,
    pub timezone: Option<String>,
    pub is_floating: bool,
    pub constant_reminder: bool,
    pub horizon: Option<Horizon>,
}

/// The tabs of the due-date popover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DueTab {
    #[default]
    Date,
    Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeField {
    StartDate,
    StartTime,
    EndDate,
    EndTime,
}

impl DueDraft {
    pub fn from_task(task: &Task, today: NaiveDate) -> Self {
        Self {
            date: task.due_date,
            time: task.due_time,
            repeat: task.repeat.clone(),
            reminders: task.reminders.clone(),
            reminders_changed: false,
            tab: if task.end_date.is_some() { DueTab::Duration } else { DueTab::Date },
            end_date: task.end_date,
            end_time: task.end_time,
            range_field: None,
            month: calendar::month_start(task.due_date.unwrap_or(today)),
            menu: None,
            timezone: task.timezone.clone(),
            is_floating: task.is_floating,
            constant_reminder: task.constant_reminder,
            horizon: task.horizon,
        }
    }

    /// Assigns a time horizon bucket and clears concrete date/duration fields for mutual exclusion.
    pub fn set_horizon(&mut self, horizon: Option<Horizon>) {
        self.horizon = horizon;
        self.date = None;
        self.time = None;
        self.end_date = None;
        self.end_time = None;
        self.reminders.clear();
        self.tab = DueTab::Date;
        self.menu = None;
    }

    /// Switches to the Duration tab. Like the original, an empty draft starts at the next full
    /// hour and lasts the default duration (Settings → Task Default), an hour unless changed.
    pub fn start_duration(&mut self, now: NaiveDateTime, minutes: i64) {
        self.horizon = None;
        self.tab = DueTab::Duration;
        self.menu = None;
        let had_date = self.date.is_some();
        let mut date = *self.date.get_or_insert(now.date());
        if self.end_date.is_none() {
            if self.time.is_none() {
                if now.hour() == 23 {
                    if !had_date {
                        date += Duration::days(1);
                        self.date = Some(date);
                    }
                    self.time = NaiveTime::from_hms_opt(0, 0, 0);
                } else {
                    self.time = NaiveTime::from_hms_opt(now.hour() + 1, 0, 0);
                }
            }
            if let Some(start_time) = self.time {
                let (end_time, overflow_secs) = start_time.overflowing_add_signed(Duration::minutes(minutes));
                let extra_days = overflow_secs.div_euclid(86_400);
                self.end_date = Some(date + Duration::days(extra_days));
                self.end_time = Some(end_time);
            } else {
                self.end_date = Some(date);
                self.end_time = None;
            }
        }
    }

    pub fn is_all_day(&self) -> bool {
        self.time.is_none()
    }

    /// "All Day" drops the times; turning it off restores an hour from 09:00.
    pub fn set_all_day(&mut self, all_day: bool) {
        if all_day {
            self.time = None;
            self.end_time = None;
        } else {
            self.time = NaiveTime::from_hms_opt(9, 0, 0);
            self.end_time = NaiveTime::from_hms_opt(10, 0, 0);
        }
    }

    /// Picks a day for the start or the end; the end never goes before the start.
    pub fn pick_range_date(&mut self, date: NaiveDate) {
        self.horizon = None;
        match self.range_field {
            Some(RangeField::EndDate) => self.end_date = Some(date.max(self.date.unwrap_or(date))),
            _ => {
                self.date = Some(date);
                if self.end_date.is_none_or(|end| end < date) {
                    self.end_date = Some(date);
                }
            }
        }
        self.range_field = None;
    }

    pub fn pick_range_time(&mut self, time: NaiveTime) {
        let same_day = self.date.is_some() && self.date == self.end_date;
        match self.range_field {
            Some(RangeField::EndTime) => {
                if same_day && let Some(start) = self.time && time < start {
                    self.end_time = Some(start);
                } else {
                    self.end_time = Some(time);
                }
            }
            _ => {
                self.time = Some(time);
                if same_day && self.end_time.is_some_and(|end| end < time) {
                    let (next_end, overflow_secs) = time.overflowing_add_signed(Duration::minutes(60));
                    if overflow_secs > 0
                        && let Some(d) = self.date
                    {
                        self.end_date = Some(d + Duration::days(overflow_secs.div_euclid(86_400)));
                    }
                    self.end_time = Some(next_end);
                }
            }
        }
        self.range_field = None;
    }

    /// The end to save: only on the Duration tab and with a start date.
    pub fn end(&self) -> Option<(NaiveDate, Option<NaiveTime>)> {
        (self.tab == DueTab::Duration && self.date.is_some()).then_some(()).and(self.end_date).map(|d| (d, self.end_time))
    }

    pub fn pick_date(&mut self, date: NaiveDate) {
        self.horizon = None;
        self.date = Some(date);
        self.month = calendar::month_start(date);
    }

    /// Picking a time without a date schedules the draft for today, as in the original.
    pub fn pick_time(&mut self, time: Option<NaiveTime>, today: NaiveDate) {
        self.time = time;
        if time.is_some() && self.date.is_none() {
            self.horizon = None;
            self.date = Some(today);
        }
        self.menu = None;
    }

    /// Turns a reminder on or off; like a time, a reminder without a date schedules today.
    pub fn toggle_reminder(&mut self, minutes: i64, today: NaiveDate) {
        self.reminders_changed = true;
        if let Some(at) = self.reminders.iter().position(|m| *m == minutes) {
            self.reminders.remove(at);
        } else {
            self.reminders.push(minutes);
            self.reminders.sort();
        }
        if self.date.is_none() {
            self.horizon = None;
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
        let repeats = repeat.is_some();
        self.repeat = repeat;
        if repeats && self.date.is_none() && self.horizon.is_none() {
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

/// A time typed by hand: "7", "7:45", "745", "19.30", "7pm", "7:45 am", "下午7:45".
pub use crate::calendar::parse_time;

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
pub fn describe_reminder(offset: i64, has_time: bool, strings: &Strings, cx: &gpui::App) -> String {
    describe_reminder_with(offset, has_time, strings, crate::calendar::hour12(cx))
}

/// Pure variant of [`describe_reminder`] for headless/unit-test callers.
pub fn describe_reminder_with(offset: i64, has_time: bool, strings: &Strings, hour12: bool) -> String {
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
    format!("{base} ({})", crate::calendar::format_time_with(at, hour12, strings))
}

/// The reminder options for the draft, with a hint ("09:00" for all-day tasks).
pub fn reminder_options(has_time: bool, strings: &Strings, cx: &gpui::App) -> Vec<(i64, &'static str, String)> {
    reminder_options_with(has_time, strings, crate::calendar::hour12(cx))
}

/// Pure variant of [`reminder_options`] for headless/unit-test callers.
pub fn reminder_options_with(has_time: bool, strings: &Strings, hour12: bool) -> Vec<(i64, &'static str, String)> {
    if has_time {
        TIMED_OFFSETS.into_iter().zip(strings.reminders_timed).map(|(m, label)| (m, label, String::new())).collect()
    } else {
        let hint = crate::calendar::format_hour_with(ALL_DAY_REMINDER_HOUR, hour12, strings);
        ALL_DAY_OFFSETS.into_iter().zip(strings.reminders_all_day).map(|(m, label)| (m, label, hint.clone())).collect()
    }
}

/// The Reminder row's label: the chosen reminders, or `None` for the plain "Reminder".
pub fn reminder_label(reminders: &[i64], has_time: bool, strings: &Strings, cx: &gpui::App) -> Option<String> {
    reminder_label_with(reminders, has_time, strings, crate::calendar::hour12(cx))
}

/// Pure variant of [`reminder_label`] for headless/unit-test callers.
pub fn reminder_label_with(reminders: &[i64], has_time: bool, strings: &Strings, hour12: bool) -> Option<String> {
    if reminders.is_empty() {
        return None;
    }
    let hint_suffix = format!(" ({})", crate::calendar::format_hour_with(ALL_DAY_REMINDER_HOUR, hour12, strings));
    let labels: Vec<String> = reminders
        .iter()
        .map(|offset| {
            let label = describe_reminder_with(*offset, has_time, strings, hour12);
            // The row stays short: preset all-day reminders drop their usual "(09:00)".
            label.strip_suffix(&hint_suffix).map(str::to_owned).unwrap_or(label)
        })
        .collect();
    Some(labels.join(", "))
}

/// A duration's label: "Oct 18, 06:00 - 07:00", "Oct 18 - Oct 20" or with times on both days.
pub fn range_label(
    start: NaiveDate,
    start_time: Option<NaiveTime>,
    end: NaiveDate,
    end_time: Option<NaiveTime>,
    strings: &Strings,
    cx: &gpui::App,
) -> String {
    range_label_with(start, start_time, end, end_time, strings, crate::calendar::hour12(cx))
}

/// Pure variant of [`range_label`] for headless/unit-test callers.
pub fn range_label_with(
    start: NaiveDate,
    start_time: Option<NaiveTime>,
    end: NaiveDate,
    end_time: Option<NaiveTime>,
    strings: &Strings,
    hour12: bool,
) -> String {
    let day = |date| crate::task_rows::short_date(date, strings);
    let time = |t: Option<NaiveTime>| t.map(|t| crate::calendar::format_time_with(t, hour12, strings));
    match (time(start_time), time(end_time)) {
        (Some(a), Some(b)) if start == end => format!("{}, {a} - {b}", day(start)),
        (Some(a), Some(b)) => format!("{}, {a} - {}, {b}", day(start), day(end)),
        _ if start == end => day(start),
        _ => format!("{} - {}", day(start), day(end)),
    }
}

/// The quick picks above the calendar: today, tomorrow, next week, next month.
pub fn quick_dates(today: NaiveDate, cx: &gpui::App) -> [NaiveDate; 4] {
    quick_dates_with(today, calendar::week_start(cx))
}

/// Pure variant of [`quick_dates`] for headless/unit-test callers.
pub fn quick_dates_with(today: NaiveDate, week_start: chrono::Weekday) -> [NaiveDate; 4] {
    [
        today,
        today + Duration::days(1),
        fern_core::next_week_date(today, week_start),
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
    let yearly = match locale {
        Locale::ZhCn => format!("{}月{day}", date.month()),
        _ if strings.day_before_month => format!("{} {month}", date.day()),
        _ => format!("{month} {day}"),
    };
    vec![
        (Recurrence::new(Frequency::Daily, 1), strings.repeat_daily.to_owned(), String::new()),
        (Recurrence::new(Frequency::Weekly, 1), strings.repeat_weekly.to_owned(), weekday.to_owned()),
        (Recurrence::new(Frequency::Monthly, 1), strings.repeat_monthly.to_owned(), day),
        (Recurrence::new(Frequency::Yearly, 1), strings.repeat_yearly.to_owned(), yearly),
        (Recurrence::weekdays(), strings.repeat_weekdays.to_owned(), strings.weekdays_range.to_owned()),
    ]
}

/// Additional repeat presets (Ebbinghaus forgetting curve and Lunar calendar).
pub fn extended_repeat_options(date: NaiveDate, strings: &Strings, locale: Locale) -> Vec<(Recurrence, String, String)> {
    let mut out = vec![(
        Recurrence::forgetting_curve(),
        strings.repeat_forgetting_curve.to_owned(),
        strings.repeat_forgetting_curve_hint.to_owned(),
    )];
    if let Some(lunar) = fern_core::lunar::gregorian_to_lunar(date) {
        let hint = fern_core::lunar::format_short(lunar, locale == Locale::ZhCn);
        out.push((Recurrence::lunar(Frequency::Yearly, 1), strings.repeat_lunar_yearly.to_owned(), hint.clone()));
        out.push((Recurrence::lunar(Frequency::Monthly, 1), strings.repeat_lunar_monthly.to_owned(), hint));
    }
    out
}

/// Lists `(iana_name, utc_offset)` pairs matching `query` (curated major zones when empty).
pub fn timezone_options(query: &str) -> Vec<(String, String)> {
    calendar::timezone_options(query)
}

/// Compact badge text for a task's time zone or floating status; `None` for default local time.
pub fn short_timezone_label(timezone: Option<&str>, is_floating: bool, strings: &Strings) -> Option<String> {
    if is_floating {
        return Some(strings.timezone_floating.to_owned());
    }
    let tz_name = timezone?;
    let city = tz_name.rsplit('/').next().unwrap_or(tz_name).replace('_', " ");
    match calendar::timezone_offset_label(tz_name) {
        Some(offset) => Some(format!("{city} ({offset})")),
        None => Some(city),
    }
}

/// The label of the current repeat rule for the Repeat row.
pub fn repeat_label(repeat: &Recurrence, strings: &Strings) -> String {
    if repeat.forgetting_curve {
        return strings.repeat_forgetting_curve.to_owned();
    }
    if repeat.is_specific_dates() {
        return strings.repeat_specific_dates.to_owned();
    }
    if repeat.is_weekdays() {
        return strings.repeat_weekdays.to_owned();
    }
    if repeat.lunar && repeat.interval == 1 {
        let base = match repeat.frequency {
            Frequency::Monthly => strings.repeat_lunar_monthly.to_owned(),
            Frequency::Yearly => strings.repeat_lunar_yearly.to_owned(),
            _ => format!("{} ({})", strings.repeat_daily, strings.repeat_lunar),
        };
        return if repeat.skip_holidays {
            format!("{base}, {}", strings.repeat_skip_holidays)
        } else {
            base
        };
    }
    let unit = unit_index(repeat.frequency);
    let mut label = if repeat.interval > 1 {
        format!("{} {} {}", strings.repeat_every, repeat.interval, strings.repeat_units_plural[unit])
    } else {
        [strings.repeat_daily, strings.repeat_weekly, strings.repeat_monthly, strings.repeat_yearly][unit].to_owned()
    };
    if repeat.lunar {
        label.push_str(&format!(" ({})", strings.repeat_lunar));
    }
    if repeat.frequency == Frequency::Weekly && repeat.weekdays != 0 {
        let days: Vec<&str> =
            (0..7).filter(|i| repeat.weekdays & (1 << i) != 0).map(|i| strings.weekdays_min[i]).collect();
        label.push_str(strings.repeat_on);
        label.push_str(&days.join(", "));
    }
    // Monthly "Each": the days, and "the last day".
    if repeat.frequency == Frequency::Monthly && repeat.month_days != 0 {
        let days: Vec<String> = (0..31)
            .filter(|i| repeat.month_days & (1 << i) != 0)
            .map(|i| (i + 1).to_string())
            .chain((repeat.month_days & fern_core::recurrence::LAST_DAY != 0).then(|| strings.repeat_last_day.to_owned()))
            .collect();
        label.push_str(strings.repeat_on);
        label.push_str(&days.join(", "));
    }
    // Monthly "On the": "the first Sunday", "the last Friday".
    if let Some((nth, weekday)) = repeat.nth_weekday.filter(|_| repeat.frequency == Frequency::Monthly) {
        let nth = if nth < 0 { strings.repeat_nth[4] } else { strings.repeat_nth[(nth as usize).clamp(1, 4) - 1] };
        label.push_str(strings.repeat_on);
        label.push_str(&format!("{nth} {}", strings.weekdays_full[weekday.num_days_from_monday() as usize]));
    }
    // Monthly "Workday": "the first workday", "the last workday".
    if let Some(nth) = repeat.nth_workday.filter(|_| repeat.frequency == Frequency::Monthly) {
        let nth = if nth < 0 { strings.repeat_nth[4] } else { strings.repeat_nth[0] };
        label.push_str(strings.repeat_on);
        label.push_str(&format!("{nth} {}", strings.repeat_workday_word));
    }
    if repeat.skip_holidays {
        label.push_str(&format!(", {}", strings.repeat_skip_holidays));
    }
    label
}

pub const UNITS: [Frequency; 4] = [Frequency::Daily, Frequency::Weekly, Frequency::Monthly, Frequency::Yearly];

pub fn unit_index(frequency: Frequency) -> usize {
    UNITS.iter().position(|f| *f == frequency).unwrap_or(0)
}

/// The day of the month as it appears in repeat options.
pub fn ordinal(day: u32, locale: Locale) -> String {
    match locale {
        Locale::Ru => format!("{day}-го"),
        Locale::ZhCn => format!("{day}日"),
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
        assert_eq!(ordinal(18, Locale::ZhCn), "18日");
    }

    #[test]
    fn reminders() {
        let today = d(2026, 10, 18);
        let mut draft = DueDraft {
            date: None,
            time: None,
            repeat: None,
            reminders: Vec::new(),
            reminders_changed: false,
            month: today,
            tab: DueTab::Date,
            end_date: None,
            end_time: None,
            range_field: None,
            menu: Some(DueMenu::Reminder),
            ..Default::default()
        };
        draft.toggle_reminder(24 * 60, today);
        draft.toggle_reminder(0, today);
        assert_eq!((draft.date, draft.reminders.as_slice()), (Some(today), [0, 24 * 60].as_slice()));
        assert_eq!(reminder_label_with(&draft.reminders, false, &EN, false).as_deref(), Some("On the day, 1 day early"));
        assert_eq!(reminder_label_with(&[30], true, &EN, false).as_deref(), Some("30 minutes early"));
        draft.toggle_reminder(0, today);
        assert_eq!(draft.reminders, [24 * 60]);
        assert_eq!(reminder_options_with(false, &EN, false)[0].2, "09:00");
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

        assert_eq!(describe_reminder_with(120, true, &EN, false), "2 hours early");
        assert_eq!(describe_reminder_with(45, true, &EN, false), "45 minutes early");
        assert_eq!(describe_reminder_with(-9 * 60, false, &EN, false), "On the day (18:00)");
        assert_eq!(describe_reminder_with(7 * 24 * 60 + 30, false, &EN, false), "1 week early (08:30)");
        assert_eq!(reminder_label_with(&[24 * 60, 2 * 24 * 60 - 60], false, &EN, false).as_deref(), Some("1 day early, 2 days early (10:00)"));
    }

    #[test]
    fn durations() {
        let today = d(2026, 10, 18);
        let t = |h| NaiveTime::from_hms_opt(h, 0, 0);
        let mut draft = DueDraft {
            date: None,
            time: None,
            repeat: None,
            reminders: Vec::new(),
            reminders_changed: false,
            month: today,
            tab: DueTab::Date,
            end_date: None,
            end_time: None,
            range_field: None,
            menu: None,
            ..Default::default()
        };
        assert_eq!(draft.end(), None);
        draft.start_duration(today.and_hms_opt(5, 30, 0).unwrap(), 60);
        assert_eq!((draft.date, draft.time, draft.end_date, draft.end_time), (Some(today), t(6), Some(today), t(7)));
        assert_eq!(draft.end(), Some((today, t(7))));

        draft.range_field = Some(RangeField::EndDate);
        draft.pick_range_date(d(2026, 10, 15));
        assert_eq!(draft.end_date, Some(today), "the end cannot go before the start");
        draft.range_field = Some(RangeField::StartDate);
        draft.pick_range_date(d(2026, 10, 20));
        assert_eq!((draft.date, draft.end_date), (Some(d(2026, 10, 20)), Some(d(2026, 10, 20))));
        draft.set_all_day(true);
        assert_eq!((draft.time, draft.end_time), (None, None));
        draft.tab = DueTab::Date;
        assert_eq!(draft.end(), None, "the Date tab saves no duration");

        assert_eq!(range_label_with(today, t(6), today, t(7), &EN, false), "Oct 18, 06:00 - 07:00");
        assert_eq!(range_label_with(today, None, d(2026, 10, 20), None, &EN, false), "Oct 18 - Oct 20");
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
        assert_eq!(parse_time("下午7:45"), t(19, 45));
        assert_eq!(parse_time("上午12:30"), t(0, 30));
        assert_eq!(parse_time("下午12:00"), t(12, 0));
        assert_eq!(parse_time("下午13:00"), None);
        assert_eq!(parse_time("25"), None);
        assert_eq!(parse_time("13pm"), None);
        assert_eq!(parse_time("soon"), None);
    }

    #[test]
    fn repeat_labels() {
        use crate::i18n::RU;
        let weekly = |interval, weekdays| Recurrence { weekdays, ..Recurrence::new(Frequency::Weekly, interval) };
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
            reminders_changed: false,
            month: today,
            tab: DueTab::Date,
            end_date: None,
            end_time: None,
            range_field: None,
            menu: Some(DueMenu::Time),
            ..Default::default()
        };
        draft.pick_time(NaiveTime::from_hms_opt(9, 30, 0), today);
        assert_eq!((draft.date, draft.menu), (Some(today), None));

        let mut draft = DueDraft {
            date: None,
            time: None,
            repeat: None,
            reminders: Vec::new(),
            reminders_changed: false,
            month: today,
            tab: DueTab::Date,
            end_date: None,
            end_time: None,
            range_field: None,
            menu: None,
            ..Default::default()
        };
        draft.pick_repeat(Some(Recurrence::weekdays()), today);
        assert_eq!(draft.date, Some(today));
        assert_eq!(time_options().len(), 48);
        assert_eq!(quick_dates_with(d(2026, 1, 31), chrono::Weekday::Mon)[3], d(2026, 2, 28));
    }

    #[test]
    fn monthly_rule_labels() {
        let mut rule = Recurrence::new(Frequency::Monthly, 1);
        rule.month_days = 1 | fern_core::recurrence::LAST_DAY;
        assert_eq!(repeat_label(&rule, &EN), "Monthly on 1, last day");
        let rule = Recurrence { nth_weekday: Some((1, chrono::Weekday::Sun)), ..Recurrence::new(Frequency::Monthly, 1) };
        assert_eq!(repeat_label(&rule, &EN), "Monthly on first Sunday");
    }

    #[test]
    fn extended_repeats_and_timezones() {
        assert_eq!(repeat_label(&Recurrence::forgetting_curve(), &EN), EN.repeat_forgetting_curve);
        assert_eq!(repeat_label(&Recurrence::lunar(Frequency::Yearly, 1), &EN), EN.repeat_lunar_yearly);
        let opts = extended_repeat_options(d(2026, 2, 17), &EN, Locale::ZhCn);
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[1].2, "正月初一");
        assert!(!timezone_options("").is_empty());
        assert!(timezone_options("tokyo").iter().any(|(name, _)| name == "Asia/Tokyo"));
        assert_eq!(short_timezone_label(None, true, &EN).as_deref(), Some(EN.timezone_floating));
        assert!(short_timezone_label(Some("America/New_York"), false, &EN).unwrap().starts_with("New York (UTC"));
    }

    #[test]
    fn duration_handles_midnight_overflow_and_keeps_end_after_start() {
        // Task at 23:30 with 60-min duration advances end_date to the next day.
        let today = d(2026, 10, 18);
        let mut draft = DueDraft {
            date: Some(today),
            time: NaiveTime::from_hms_opt(23, 30, 0),
            month: today,
            ..Default::default()
        };
        draft.start_duration(today.and_hms_opt(22, 0, 0).unwrap(), 60);
        assert_eq!(draft.end_date, Some(d(2026, 10, 19)));
        assert_eq!(draft.end_time, NaiveTime::from_hms_opt(0, 30, 0));

        // Empty draft at 23:15 starts at 00:00 the next day.
        let mut late = DueDraft { month: today, ..Default::default() };
        late.start_duration(today.and_hms_opt(23, 15, 0).unwrap(), 60);
        assert_eq!(late.date, Some(d(2026, 10, 19)));
        assert_eq!(late.time, NaiveTime::from_hms_opt(0, 0, 0));
        assert_eq!(late.end_date, Some(d(2026, 10, 19)));
        assert_eq!(late.end_time, NaiveTime::from_hms_opt(1, 0, 0));

        // Moving start time past end time on the same day pushes end time forward.
        let mut same = DueDraft {
            date: Some(today),
            time: NaiveTime::from_hms_opt(9, 0, 0),
            end_date: Some(today),
            end_time: NaiveTime::from_hms_opt(10, 0, 0),
            range_field: Some(RangeField::StartTime),
            ..Default::default()
        };
        same.pick_range_time(NaiveTime::from_hms_opt(14, 0, 0).unwrap());
        assert_eq!(same.end_time, NaiveTime::from_hms_opt(15, 0, 0));

        // Picking an end time earlier than start time on the same day clamps to start time.
        same.range_field = Some(RangeField::EndTime);
        same.pick_range_time(NaiveTime::from_hms_opt(12, 0, 0).unwrap());
        assert_eq!(same.end_time, NaiveTime::from_hms_opt(14, 0, 0));
    }

    #[test]
    fn quick_dates_next_week_aligns_with_core_next_week_date() {
        let sunday = d(2026, 10, 18);
        let monday = d(2026, 10, 19);
        for week_start in [chrono::Weekday::Mon, chrono::Weekday::Sun] {
            assert_eq!(
                quick_dates_with(sunday, week_start)[2],
                fern_core::next_week_date(sunday, week_start)
            );
            assert_eq!(
                quick_dates_with(monday, week_start)[2],
                fern_core::next_week_date(monday, week_start)
            );
        }
    }

    #[test]
    fn set_horizon_clears_concrete_dates_and_pick_date_clears_horizon() {
        let today = d(2026, 11, 23);
        let mut draft = DueDraft {
            date: Some(today),
            time: NaiveTime::from_hms_opt(11, 0, 0),
            end_date: Some(today),
            end_time: NaiveTime::from_hms_opt(11, 30, 0),
            reminders: vec![15],
            tab: DueTab::Duration,
            menu: Some(DueMenu::Time),
            ..Default::default()
        };

        let week = Horizon::current_week(today);
        draft.set_horizon(Some(week));
        assert_eq!(draft.horizon, Some(week));
        assert_eq!(draft.date, None);
        assert_eq!(draft.time, None);
        assert_eq!(draft.end_date, None);
        assert_eq!(draft.end_time, None);
        assert!(draft.reminders.is_empty());
        assert_eq!(draft.tab, DueTab::Date);
        assert_eq!(draft.menu, None);

        // Picking a concrete date clears horizon.
        draft.pick_date(today);
        assert_eq!(draft.date, Some(today));
        assert_eq!(draft.horizon, None);

        // Starting duration clears horizon too.
        draft.set_horizon(Some(Horizon::Life));
        draft.start_duration(today.and_hms_opt(10, 0, 0).unwrap(), 60);
        assert_eq!(draft.horizon, None);
        assert_eq!(draft.tab, DueTab::Duration);
    }
}
