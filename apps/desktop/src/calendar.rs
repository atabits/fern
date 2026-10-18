//! Date math for the due-date calendar.

use std::cell::Cell;

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveTime, Timelike, Weekday};

use crate::i18n::Strings;

/// The first day of the month that contains `date`.
pub fn month_start(date: NaiveDate) -> NaiveDate {
    date.with_day(1).expect("every month has a first day")
}

pub fn add_months(month: NaiveDate, delta: i32) -> NaiveDate {
    let month = month_start(month);
    if delta >= 0 {
        month.checked_add_months(Months::new(delta as u32)).unwrap_or(month)
    } else {
        month.checked_sub_months(Months::new(delta.unsigned_abs())).unwrap_or(month)
    }
}

thread_local! {
    /// The first day of the week from the Settings ("Start Week on"). The UI runs on one thread,
    /// so every calendar reads it without threading it through each view.
    static WEEK_START: Cell<Weekday> = const { Cell::new(Weekday::Mon) };
}

thread_local! {
    /// Settings → Time Format: "1:00 PM" instead of "13:00".
    static HOUR12: Cell<bool> = const { Cell::new(false) };
}

thread_local! {
    /// Settings → Date Format; `None` follows the UI language.
    static DATE_FORMAT: Cell<Option<fern_core::DateFormat>> = const { Cell::new(None) };
    /// Settings → Show Week Numbers.
    static WEEK_NUMBERS: Cell<bool> = const { Cell::new(false) };
}

pub fn set_date_format(format: Option<fern_core::DateFormat>) {
    DATE_FORMAT.with(|f| f.set(format));
}

pub fn set_week_numbers(on: bool) {
    WEEK_NUMBERS.with(|w| w.set(on));
}

pub fn week_numbers() -> bool {
    WEEK_NUMBERS.with(Cell::get)
}

/// The ISO week number shown for a calendar row: the week of its Thursday-most day, which is
/// the ISO rule for weeks starting on Monday.
pub fn week_number(week: &[NaiveDate; 7]) -> u32 {
    week.iter().find(|d| d.weekday() == Weekday::Thu).unwrap_or(&week[3]).iso_week().week()
}

/// A numeric date in the chosen format: "12/31/2026", "31/12/2026" or "2026/12/31"; without a
/// choice, the language's own ("31.12.2026" in Russian).
pub fn format_date(date: NaiveDate, strings: &Strings) -> String {
    format_date_as(date, DATE_FORMAT.with(Cell::get), strings.dotted_dates, true)
}

/// A numeric day and month: "12/31", "31/12" or "31.12".
pub fn format_day_month(date: NaiveDate, strings: &Strings) -> String {
    format_date_as(date, DATE_FORMAT.with(Cell::get), strings.dotted_dates, false)
}

/// A typed date: ISO ("2026-10-18"), or the numeric format dates are shown in, with or without
/// the year (the current year then).
pub fn parse_date(text: &str, today: NaiveDate, strings: &Strings) -> Option<NaiveDate> {
    let text = text.trim();
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Some(date);
    }
    let parts: Vec<u32> = text.split(['.', '/', '-']).map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    use fern_core::DateFormat::*;
    let format = DATE_FORMAT.with(Cell::get).unwrap_or(if strings.dotted_dates { DayMonthYear } else { MonthDayYear });
    let (y, m, d) = match (format, parts.as_slice()) {
        (YearMonthDay, &[y, m, d]) => (y as i32, m, d),
        (DayMonthYear, &[d, m, y]) => (y as i32, m, d),
        (MonthDayYear, &[m, d, y]) => (y as i32, m, d),
        (DayMonthYear, &[d, m]) => (today.year(), m, d),
        (_, &[m, d]) => (today.year(), m, d),
        _ => return None,
    };
    NaiveDate::from_ymd_opt(y, m, d)
}

pub fn format_date_as(date: NaiveDate, format: Option<fern_core::DateFormat>, dotted: bool, year: bool) -> String {
    use fern_core::DateFormat::*;
    let pattern = match (format, year) {
        (Some(MonthDayYear), true) => "%m/%d/%Y",
        (Some(DayMonthYear), true) => "%d/%m/%Y",
        (Some(YearMonthDay), true) => "%Y/%m/%d",
        (Some(DayMonthYear), false) => "%d/%m",
        (Some(_), false) => "%m/%d",
        (None, true) if dotted => "%d.%m.%Y",
        (None, true) => "%m/%d/%Y",
        (None, false) if dotted => "%d.%m",
        (None, false) => "%m/%d",
    };
    date.format(pattern).to_string()
}

pub fn set_hour12(on: bool) {
    HOUR12.with(|hour12| hour12.set(on));
}

/// A time of day in the chosen format: "13:05" or "1:05 PM".
pub fn format_time(time: impl Timelike) -> String {
    format_time_as(time, HOUR12.with(Cell::get))
}

pub fn format_time_as(time: impl Timelike, hour12: bool) -> String {
    if hour12 {
        let (pm, hour) = time.hour12();
        format!("{hour}:{:02} {}", time.minute(), if pm { "PM" } else { "AM" })
    } else {
        format!("{:02}:{:02}", time.hour(), time.minute())
    }
}

/// A whole hour for time axes and hints: "09:00" or "9 AM".
pub fn format_hour(hour: u32) -> String {
    if HOUR12.with(Cell::get) {
        let (pm, hour) = NaiveTime::from_hms_opt(hour % 24, 0, 0).expect("a valid hour").hour12();
        format!("{hour} {}", if pm { "PM" } else { "AM" })
    } else {
        format!("{hour:02}:00")
    }
}

pub fn set_week_start(day: Weekday) {
    WEEK_START.with(|start| start.set(day));
}

pub fn week_start() -> Weekday {
    WEEK_START.with(Cell::get)
}

/// Weekday labels (given Monday first) in display order, starting with the first day of the week.
pub fn weekday_labels<T: Copy>(monday_first: &[T; 7]) -> [T; 7] {
    let offset = week_start().num_days_from_monday() as usize;
    std::array::from_fn(|i| monday_first[(i + offset) % 7])
}

/// The weeks that contain days of `month`: four to six of them.
pub fn month_weeks(month: NaiveDate) -> Vec<[NaiveDate; 7]> {
    let first = month_start(month);
    let next_month = add_months(first, 1);
    let mut weeks = Vec::new();
    let mut start = week_of(first)[0];
    while start < next_month {
        weeks.push(week_of(start));
        start += Duration::days(7);
    }
    weeks
}

/// `count` weeks from the one that contains `date`: the original's Multi-Week.
pub fn weeks_from(date: NaiveDate, count: usize) -> Vec<[NaiveDate; 7]> {
    let first = week_of(date)[0];
    (0..count).map(|i| week_of(first + Duration::weeks(i as i64))).collect()
}

/// 42 days (six weeks) covering `month`.
pub fn month_grid(month: NaiveDate) -> Vec<NaiveDate> {
    let start = week_of(month_start(month))[0];
    (0..42).map(|i| start + Duration::days(i)).collect()
}

/// The seven days of the week that contains `date`.
pub fn week_of(date: NaiveDate) -> [NaiveDate; 7] {
    week_from(date, week_start())
}

/// The seven days of the week that contains `date`, for weeks starting on `start`.
pub fn week_from(date: NaiveDate, start: Weekday) -> [NaiveDate; 7] {
    let offset = (date.weekday().num_days_from_monday() + 7 - start.num_days_from_monday()) % 7;
    let first = date - Duration::days(offset as i64);
    std::array::from_fn(|i| first + Duration::days(i as i64))
}

/// The week view's time axis, as in the original: the night is folded into two short blocks
/// (00:00–07:00 and 21:00–24:00) around one row per hour from 08:00 to 20:00.
pub mod week_axis {
    use super::*;

    pub const HOUR_HEIGHT: f32 = 60.;
    pub const FOLDED_HEIGHT: f32 = 40.;
    pub const FIRST_HOUR: u32 = 8;
    pub const LAST_HOUR: u32 = 21;

    pub fn total_height() -> f32 {
        2. * FOLDED_HEIGHT + (LAST_HOUR - FIRST_HOUR) as f32 * HOUR_HEIGHT
    }

    /// Whether `time` falls in the hour rows rather than a folded night block, where the original
    /// shows no current-time line.
    pub fn unfolded(time: NaiveTime) -> bool {
        (FIRST_HOUR..LAST_HOUR).contains(&time.hour())
    }

    /// The vertical offset of `time` from the top of the axis.
    pub fn offset(time: NaiveTime) -> f32 {
        let minutes = (time.hour() * 60 + time.minute()) as f32;
        let first = (FIRST_HOUR * 60) as f32;
        let last = (LAST_HOUR * 60) as f32;
        if minutes < first {
            minutes / first * FOLDED_HEIGHT
        } else if minutes < last {
            FOLDED_HEIGHT + (minutes - first) / 60. * HOUR_HEIGHT
        } else {
            FOLDED_HEIGHT + (LAST_HOUR - FIRST_HOUR) as f32 * HOUR_HEIGHT
                + (minutes - last) / (24. * 60. - last) * FOLDED_HEIGHT
        }
    }
}

/// The part of a timed task on `date` in the week view: from its start (or midnight) to its end
/// (or the end of the day). Tasks without a time, or not on that day, have none.
pub fn timed_segment(task: &fern_core::Task, date: NaiveDate) -> Option<(NaiveTime, NaiveTime)> {
    let (start_date, start_time) = (task.due_date?, task.due_time?);
    if !task.occurs_on(date) {
        return None;
    }
    let midnight = NaiveTime::MIN;
    let last = NaiveTime::from_hms_opt(23, 59, 0).unwrap_or(midnight);
    let end_date = task.end_date.unwrap_or(start_date);
    let from = if date == start_date { start_time } else { midnight };
    let to = match task.end_time {
        Some(end_time) if date == end_date => end_time,
        Some(_) => last,
        None if task.end_date.is_some() && date != end_date => last,
        // No end: a point in time, drawn as a short block.
        None => from,
    };
    Some((from, to.max(from)))
}

pub fn month_title(month: NaiveDate, strings: &Strings) -> String {
    format!("{} {}", strings.months[month.month0() as usize], month.year())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{EN, RU};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn timed_segments_split_by_day() {
        let store = fern_core::Store::open_in_memory().unwrap();
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        let task = store.add_task("workshop").unwrap();
        store.set_due(task.id, Some(d(2026, 10, 20)), Some(t(14, 0))).unwrap();
        let point = store.task(task.id).unwrap().unwrap();
        assert_eq!(timed_segment(&point, d(2026, 10, 20)), Some((t(14, 0), t(14, 0))));

        store.set_due_end(task.id, Some((d(2026, 10, 20), Some(t(15, 30))))).unwrap();
        let hour = store.task(task.id).unwrap().unwrap();
        assert_eq!(timed_segment(&hour, d(2026, 10, 20)), Some((t(14, 0), t(15, 30))));
        assert_eq!(timed_segment(&hour, d(2026, 10, 21)), None);

        store.set_due_end(task.id, Some((d(2026, 10, 22), Some(t(11, 0))))).unwrap();
        let days = store.task(task.id).unwrap().unwrap();
        assert_eq!(timed_segment(&days, d(2026, 10, 20)), Some((t(14, 0), t(23, 59))));
        assert_eq!(timed_segment(&days, d(2026, 10, 21)), Some((NaiveTime::MIN, t(23, 59))));
        assert_eq!(timed_segment(&days, d(2026, 10, 22)), Some((NaiveTime::MIN, t(11, 0))));
    }

    #[test]
    fn grid_starts_on_monday_and_covers_month() {
        // 1 October 2026 is a Thursday.
        let grid = month_grid(d(2026, 10, 18));
        assert_eq!(grid.len(), 42);
        assert_eq!(grid[0], d(2026, 9, 28));
        assert_eq!(grid[3], d(2026, 10, 1));
        assert!(grid.contains(&d(2026, 10, 31)));
    }

    #[test]
    fn month_weeks_cover_only_the_month() {
        // October 2026 spans five weeks: Sep 28 – Nov 1.
        let weeks = month_weeks(d(2026, 10, 18));
        assert_eq!(weeks.len(), 5);
        assert_eq!(weeks[0][0], d(2026, 9, 28));
        assert_eq!(weeks[4][6], d(2026, 11, 1));
        // February 2027 starts on a Monday and fits in four weeks.
        assert_eq!(month_weeks(d(2027, 2, 1)).len(), 4);
    }

    #[test]
    fn week_axis_offsets() {
        use week_axis::*;
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        assert_eq!(offset(t(0, 0)), 0.);
        assert_eq!(offset(t(4, 0)), FOLDED_HEIGHT / 2.);
        assert_eq!(offset(t(8, 0)), FOLDED_HEIGHT);
        assert_eq!(offset(t(9, 30)), FOLDED_HEIGHT + 1.5 * HOUR_HEIGHT);
        assert_eq!(offset(t(21, 0)), FOLDED_HEIGHT + 13. * HOUR_HEIGHT);
        assert!(offset(t(23, 59)) < total_height());
        // As the original's, no current-time line in the folded night blocks.
        assert!(unfolded(t(8, 0)) && unfolded(t(20, 59)));
        assert!(!unfolded(t(21, 9)) && !unfolded(t(7, 59)));
    }

    #[test]
    fn week_starts_on_monday() {
        let week = week_of(d(2026, 10, 18)); // a Sunday
        assert_eq!(week[0], d(2026, 10, 12));
        assert_eq!(week[6], d(2026, 10, 18));
        assert_eq!(week_of(d(2026, 10, 12))[0], d(2026, 10, 12));
    }

    #[test]
    fn date_formats_and_week_numbers() {
        use fern_core::DateFormat::*;
        let date = d(2026, 12, 31);
        assert_eq!(format_date_as(date, Some(MonthDayYear), true, true), "12/31/2026");
        assert_eq!(format_date_as(date, Some(DayMonthYear), false, true), "31/12/2026");
        assert_eq!(format_date_as(date, Some(YearMonthDay), false, true), "2026/12/31");
        assert_eq!(format_date_as(date, None, true, true), "31.12.2026");
        assert_eq!(format_date_as(date, None, false, false), "12/31");
        assert_eq!(format_date_as(date, Some(DayMonthYear), false, false), "31/12");
        // October 12–18, 2026 is ISO week 42, whichever day the row starts on.
        assert_eq!(week_number(&week_from(d(2026, 10, 18), Weekday::Mon)), 42);
        assert_eq!(week_number(&week_from(d(2026, 10, 14), Weekday::Sun)), 42);
    }

    #[test]
    fn parses_typed_dates() {
        let today = d(2026, 10, 18);
        assert_eq!(parse_date("2026-11-02", today, &crate::i18n::EN), Some(d(2026, 11, 2)));
        assert_eq!(parse_date("11/02/2026", today, &crate::i18n::EN), Some(d(2026, 11, 2)));
        assert_eq!(parse_date("11/2", today, &crate::i18n::EN), Some(d(2026, 11, 2)));
        assert_eq!(parse_date("2.11.2026", today, &crate::i18n::RU), Some(d(2026, 11, 2)));
        assert_eq!(parse_date("someday", today, &crate::i18n::EN), None);
    }

    #[test]
    fn twelve_and_twenty_four_hour_times() {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        assert_eq!(format_time_as(t(13, 5), false), "13:05");
        assert_eq!(format_time_as(t(13, 5), true), "1:05 PM");
        assert_eq!(format_time_as(t(0, 30), true), "12:30 AM");
        assert_eq!(format_time_as(t(12, 0), true), "12:00 PM");
        set_hour12(true);
        assert_eq!(format_hour(9), "9 AM");
        assert_eq!(format_time(t(21, 0)), "9:00 PM");
        set_hour12(false);
        assert_eq!(format_hour(9), "09:00");
    }

    #[test]
    fn weeks_can_start_on_sunday_or_saturday() {
        let sunday = d(2026, 10, 18);
        assert_eq!(week_from(sunday, Weekday::Sun)[0], sunday);
        assert_eq!(week_from(d(2026, 10, 24), Weekday::Sun)[6], d(2026, 10, 24));
        assert_eq!(week_from(sunday, Weekday::Sat)[0], d(2026, 10, 17));

        set_week_start(Weekday::Sun);
        assert_eq!(month_grid(d(2026, 10, 1))[0], d(2026, 9, 27));
        assert_eq!(weekday_labels(&["M", "T", "W", "T", "F", "S", "Su"])[0], "Su");
        set_week_start(Weekday::Mon);
    }

    #[test]
    fn month_navigation_and_titles() {
        assert_eq!(add_months(d(2026, 1, 31), 1), d(2026, 2, 1));
        assert_eq!(add_months(d(2026, 1, 15), -1), d(2025, 12, 1));
        assert_eq!(month_title(d(2026, 10, 18), &EN), "October 2026");
        assert_eq!(month_title(d(2026, 10, 18), &RU), "Октябрь 2026");
    }

    #[test]
    fn multi_week_weeks() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let weeks = weeks_from(day, 5);
        assert_eq!(weeks.len(), 5);
        assert_eq!(weeks[0], week_of(day));
        assert_eq!(weeks[4][0], weeks[0][0] + Duration::weeks(4));
    }
}
