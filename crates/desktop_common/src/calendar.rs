//! Date math for the due-date calendar.

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveTime, Timelike, Weekday};
use fern_i18n::Strings;
use gpui::App;

use crate::active_preferences::ActivePreferences;

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

pub fn explicit_date_format(cx: &App) -> Option<fern_core::DateFormat> {
    ActivePreferences::get(cx).date_format
}

pub fn week_numbers(cx: &App) -> bool {
    ActivePreferences::get(cx).week_numbers
}

pub fn horizons_enabled(cx: &App) -> bool {
    ActivePreferences::get(cx).horizons_enabled
}

/// The ISO week number shown for a calendar row: the week of its Thursday-most day, which is
/// the ISO rule for weeks starting on Monday.
pub fn week_number(week: &[NaiveDate; 7]) -> u32 {
    week.iter().find(|d| d.weekday() == Weekday::Thu).unwrap_or(&week[3]).iso_week().week()
}

/// A numeric date in the chosen format, such as "2026-12-31", or in the locale's default.
pub fn format_date(date: NaiveDate, strings: &Strings, cx: &App) -> String {
    format_date_with(date, explicit_date_format(cx), strings)
}

pub fn format_date_with(
    date: NaiveDate,
    explicit: Option<fern_core::DateFormat>,
    strings: &Strings,
) -> String {
    format_date_as(date, selected_date_format(explicit, strings), strings.dotted_dates, true)
}

/// A numeric date with the year omitted.
pub fn format_day_month(date: NaiveDate, strings: &Strings, cx: &App) -> String {
    format_day_month_with(date, explicit_date_format(cx), strings)
}

pub fn format_day_month_with(
    date: NaiveDate,
    explicit: Option<fern_core::DateFormat>,
    strings: &Strings,
) -> String {
    format_date_as(date, selected_date_format(explicit, strings), strings.dotted_dates, false)
}

/// Localized label for a filter's date rule chip.
pub fn filter_date_label(rule: fern_core::DateRule, strings: &Strings) -> &'static str {
    use fern_core::DateRule::*;
    match rule {
        All => strings.filter_all,
        Overdue => strings.filter_date_overdue,
        Today => strings.filter_date_today,
        Tomorrow => strings.filter_date_tomorrow,
        ThisWeek => strings.filter_date_this_week,
        NoDate => strings.filter_date_no_date,
        NextWeek => strings.filter_date_next_week,
        ThisMonth => strings.filter_date_this_month,
        NextMonth => strings.filter_date_next_month,
        NextDays(_) => strings.filter_date_next_days,
        Recurring => strings.filter_date_recurring,
        Span { .. } => strings.filter_date_range,
    }
}

fn selected_date_format(
    explicit: Option<fern_core::DateFormat>,
    strings: &Strings,
) -> Option<fern_core::DateFormat> {
    explicit.or(strings.year_first_dates.then_some(fern_core::DateFormat::YearMonthDay))
}

pub fn effective_date_format_with(
    explicit: Option<fern_core::DateFormat>,
    strings: &Strings,
) -> fern_core::DateFormat {
    use fern_core::DateFormat::{DayMonthYear, MonthDayYear};
    selected_date_format(explicit, strings)
        .unwrap_or(if strings.dotted_dates { DayMonthYear } else { MonthDayYear })
}

pub fn effective_date_format(strings: &Strings, cx: &App) -> fern_core::DateFormat {
    effective_date_format_with(explicit_date_format(cx), strings)
}

/// Writes `date` by a template of the language: `{weekday}` and `{wd}` the full and short
/// weekday, `{month}` and `{mon}` the month written out (as after a day) and short, `{d}` the
/// day, and `{dd}`, `{mm}` and `{yyyy}` the numeric date.
pub fn format_day(template: &str, date: NaiveDate, strings: &Strings) -> String {
    let weekday = date.weekday().num_days_from_monday() as usize;
    template
        .replace("{weekday}", strings.weekdays_full[weekday])
        .replace("{wd}", strings.weekdays_short[weekday])
        .replace("{month}", strings.months_genitive[date.month0() as usize])
        .replace("{mon}", strings.months_short[date.month0() as usize])
        .replace("{dd}", &format!("{:02}", date.day()))
        .replace("{d}", &date.day().to_string())
        .replace("{mm}", &format!("{:02}", date.month()))
        .replace("{yyyy}", &date.year().to_string())
}

pub fn parse_date_with(
    text: &str,
    today: NaiveDate,
    explicit: Option<fern_core::DateFormat>,
    strings: &Strings,
) -> Option<NaiveDate> {
    let text = text.trim();
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Some(date);
    }
    let parts: Vec<u32> = text.split(['.', '/', '-']).map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    use fern_core::DateFormat::*;
    let format = effective_date_format_with(explicit, strings);
    let (y, m, d) = match (format, parts.as_slice()) {
        (YearMonthDay | IsoYearMonthDay, &[y, m, d]) => (y as i32, m, d),
        (DayMonthYear, &[d, m, y]) => (y as i32, m, d),
        (MonthDayYear, &[m, d, y]) => (y as i32, m, d),
        (DayMonthYear, &[d, m]) => (today.year(), m, d),
        (_, &[m, d]) => (today.year(), m, d),
        _ => return None,
    };
    NaiveDate::from_ymd_opt(y, m, d)
}

/// A typed date: ISO ("2026-10-18"), or the numeric format dates are shown in, with or without
/// the year (the current year then).
pub fn parse_date(text: &str, today: NaiveDate, strings: &Strings, cx: &App) -> Option<NaiveDate> {
    parse_date_with(text, today, explicit_date_format(cx), strings)
}

pub fn format_date_as(date: NaiveDate, format: Option<fern_core::DateFormat>, dotted: bool, year: bool) -> String {
    use fern_core::DateFormat::*;
    let pattern = match (format, year) {
        (Some(MonthDayYear), true) => "%m/%d/%Y",
        (Some(DayMonthYear), true) => "%d/%m/%Y",
        (Some(YearMonthDay), true) => "%Y/%m/%d",
        (Some(IsoYearMonthDay), true) => "%Y-%m-%d",
        (Some(DayMonthYear), false) => "%d/%m",
        (Some(IsoYearMonthDay), false) => "%m-%d",
        (Some(_), false) => "%m/%d",
        (None, true) if dotted => "%d.%m.%Y",
        (None, true) => "%m/%d/%Y",
        (None, false) if dotted => "%d.%m",
        (None, false) => "%m/%d",
    };
    date.format(pattern).to_string()
}

pub fn hour12(cx: &App) -> bool {
    ActivePreferences::get(cx).hour12
}

/// A time of day in the chosen format, with the locale's 12-hour labels.
pub fn format_time(time: impl Timelike, strings: &Strings, cx: &App) -> String {
    format_time_as(time, hour12(cx), strings)
}

pub fn format_time_with(time: impl Timelike, use_hour12: bool, strings: &Strings) -> String {
    format_time_as(time, use_hour12, strings)
}

/// Parses a typed time such as `"9"`, `"09:30"`, `"9.30pm"`, `"930"`, or `"下午3:15"`.
pub fn parse_time(text: &str) -> Option<NaiveTime> {
    let input = text.trim().to_ascii_lowercase();
    let (text, meridiem) = [
        (input.strip_prefix("上午"), false),
        (input.strip_prefix("下午"), true),
        (input.strip_suffix("am").or_else(|| input.strip_suffix("a")), false),
        (input.strip_suffix("pm").or_else(|| input.strip_suffix("p")), true),
    ]
    .into_iter()
    .find_map(|(text, pm)| text.map(|text| (text.trim(), Some(pm))))
    .unwrap_or((&input, None));
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

/// Short localized date label (e.g. `"Oct 18"`).
pub fn short_date(date: NaiveDate, strings: &Strings) -> String {
    format_day(strings.short_date[0], date, strings)
}

/// The local calendar day of a Unix timestamp in milliseconds.
pub fn local_day(ms: i64) -> Option<NaiveDate> {
    use chrono::{Local, TimeZone};
    Local.timestamp_millis_opt(ms).single().map(|time| time.date_naive())
}

/// Day header label including year when `day` is in a different year from `today`.
pub fn day_header(day: NaiveDate, today: NaiveDate, strings: &Strings) -> String {
    format_day(strings.day_header[usize::from(day.year() != today.year())], day, strings)
}

fn format_time_as(time: impl Timelike, hour12: bool, strings: &Strings) -> String {
    if hour12 {
        let (pm, hour) = time.hour12();
        strings.time_meridiem[usize::from(pm)].replace("{time}", &format!("{hour}:{:02}", time.minute()))
    } else {
        format!("{:02}:{:02}", time.hour(), time.minute())
    }
}

/// A whole hour for time axes and hints: "09:00" or "9 AM".
pub fn format_hour_with(hour: u32, use_hour12: bool, strings: &Strings) -> String {
    if use_hour12 {
        let (pm, hour) = NaiveTime::from_hms_opt(hour % 24, 0, 0).expect("a valid hour").hour12();
        strings.time_meridiem[usize::from(pm)].replace("{time}", &hour.to_string())
    } else {
        format!("{hour:02}:00")
    }
}

pub fn format_hour(hour: u32, strings: &Strings, cx: &App) -> String {
    format_hour_with(hour, hour12(cx), strings)
}

pub fn week_start(cx: &App) -> Weekday {
    ActivePreferences::get(cx).week_start
}

/// Weekday labels (given Monday first) in display order, starting with the first day of the week.
pub fn weekday_labels_with<T: Copy>(monday_first: &[T; 7], start: Weekday) -> [T; 7] {
    let offset = start.num_days_from_monday() as usize;
    std::array::from_fn(|i| monday_first[(i + offset) % 7])
}

pub fn weekday_labels<T: Copy>(monday_first: &[T; 7], cx: &App) -> [T; 7] {
    weekday_labels_with(monday_first, week_start(cx))
}

/// The weeks that contain days of `month`: four to six of them.
pub fn month_weeks_with(month: NaiveDate, start_day: Weekday) -> Vec<[NaiveDate; 7]> {
    let first = month_start(month);
    let next_month = add_months(first, 1);
    let mut weeks = Vec::new();
    let mut start = week_from(first, start_day)[0];
    while start < next_month {
        weeks.push(week_from(start, start_day));
        start += Duration::days(7);
    }
    weeks
}

pub fn month_weeks(month: NaiveDate, cx: &App) -> Vec<[NaiveDate; 7]> {
    month_weeks_with(month, week_start(cx))
}

/// `count` weeks from the one that contains `date`: the original's Multi-Week.
pub fn weeks_from_with(date: NaiveDate, count: usize, start_day: Weekday) -> Vec<[NaiveDate; 7]> {
    let first = week_from(date, start_day)[0];
    (0..count).map(|i| week_from(first + Duration::weeks(i as i64), start_day)).collect()
}

pub fn weeks_from(date: NaiveDate, count: usize, cx: &App) -> Vec<[NaiveDate; 7]> {
    weeks_from_with(date, count, week_start(cx))
}

/// 42 days (six weeks) covering `month`.
pub fn month_grid_with(month: NaiveDate, start_day: Weekday) -> Vec<NaiveDate> {
    let start = week_from(month_start(month), start_day)[0];
    (0..42).map(|i| start + Duration::days(i)).collect()
}

pub fn month_grid(month: NaiveDate, cx: &App) -> Vec<NaiveDate> {
    month_grid_with(month, week_start(cx))
}

/// The year overview's heatmap levels, based on the number of visible items.
pub fn heatmap_level(count: usize) -> u8 {
    match count {
        0 => 0,
        1..=3 => 1,
        4..=6 => 2,
        7..=9 => 3,
        _ => 4,
    }
}

/// The seven days of the week that contains `date`.
pub fn week_of(date: NaiveDate, cx: &App) -> [NaiveDate; 7] {
    week_from(date, week_start(cx))
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

    /// Converts a vertical pixel offset on the week grid back into a 15-minute snapped time.
    pub fn time_at_offset(y: f32) -> NaiveTime {
        let first = (FIRST_HOUR * 60) as f32;
        let last = (LAST_HOUR * 60) as f32;
        let unfolded_end = FOLDED_HEIGHT + (LAST_HOUR - FIRST_HOUR) as f32 * HOUR_HEIGHT;
        let minutes = if y <= 0. {
            0.
        } else if y < FOLDED_HEIGHT {
            (y / FOLDED_HEIGHT) * first
        } else if y <= unfolded_end {
            first + (y - FOLDED_HEIGHT) / HOUR_HEIGHT * 60.
        } else {
            last + ((y - unfolded_end) / FOLDED_HEIGHT).min(1.) * (24. * 60. - last)
        };
        let snapped = ((minutes / 15.).round() as i64 * 15).clamp(0, 23 * 60 + 45) as u32;
        NaiveTime::from_hms_opt(snapped / 60, snapped % 60, 0).unwrap_or(NaiveTime::MIN)
    }

    /// Maps `date` at `local_hour:00` (system `Local`) into `(hour, minute, day_delta)` in `tz_name`.
    pub fn hour_in_timezone(date: NaiveDate, local_hour: u32, tz_name: &str) -> (u32, u32, i8) {
        use chrono::TimeZone as _;
        let Ok(tz) = tz_name.parse::<chrono_tz::Tz>() else {
            return (local_hour.min(23), 0, 0);
        };
        let Some(naive) = date.and_hms_opt(local_hour.min(23), 0, 0) else {
            return (local_hour.min(23), 0, 0);
        };
        let Some(local_dt) = chrono::Local.from_local_datetime(&naive).earliest() else {
            return (local_hour.min(23), 0, 0);
        };
        let target = local_dt.with_timezone(&tz);
        let delta = (target.date_naive() - date).num_days().clamp(-2, 2) as i8;
        (target.hour(), target.minute(), delta)
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
    let index = month.month0() as usize;
    strings.month_title
        .replace("{month}", strings.months[index])
        .replace("{mon}", strings.months_short[index])
        .replace("{yyyy}", &month.year().to_string())
}

/// Compact localized badge label for a [`fern_core::Horizon`], used in classic task rows,
/// rollover origin chips, and quick-assign selectors.
pub fn format_horizon_badge(horizon: fern_core::Horizon, strings: &Strings) -> String {
    match horizon {
        fern_core::Horizon::Week { week, .. } => {
            format!("{}{week}{}", strings.horizons_week_prefix, strings.horizons_week_suffix)
        }
        fern_core::Horizon::Month { month, .. } => {
            strings.life_clock_months[(month.saturating_sub(1) as usize).min(11)].to_owned()
        }
        fern_core::Horizon::Quarter { year, quarter } => {
            format!("{}{quarter} {year}", strings.horizons_quarter_prefix)
        }
        fern_core::Horizon::Year(year) => year.to_string(),
        fern_core::Horizon::Decade(start_year) => {
            format!("{start_year}{}", strings.horizons_decade_suffix)
        }
        fern_core::Horizon::Life => strings.horizons_life.to_owned(),
    }
}

/// Formats a week's Monday..=Sunday calendar span cleanly for the active locale
/// (e.g. `"Nov 23–29"`, `"23–29 ноя"`, `"11月23–29日"`), appending the year when different from `today`.
fn format_week_range(mon: NaiveDate, sun: NaiveDate, iso_year: i32, today: NaiveDate, strings: &Strings) -> String {
    let base = if mon.month() == sun.month() {
        let mon_short = strings.months_short[mon.month0() as usize];
        if strings.year_first_dates {
            format!("{mon_short}{}–{}日", mon.day(), sun.day())
        } else if strings.day_before_month {
            format!("{}–{} {mon_short}", mon.day(), sun.day())
        } else {
            format!("{mon_short} {}–{}", mon.day(), sun.day())
        }
    } else {
        format!("{} – {}", short_date(mon, strings), short_date(sun, strings))
    };
    if iso_year != today.year() {
        format!("{base} {iso_year}")
    } else {
        base
    }
}

/// Column heading `(primary_title, optional_subtitle)` for a [`fern_core::Horizon`].
pub fn format_horizon_heading(
    horizon: fern_core::Horizon,
    today: NaiveDate,
    strings: &Strings,
) -> (String, Option<String>) {
    match horizon {
        fern_core::Horizon::Week { year, week } => {
            let title = format!("{}{week}{}", strings.horizons_week_prefix, strings.horizons_week_suffix);
            let subtitle = fern_core::horizon::week_bounds(year, week)
                .map(|(mon, sun)| format_week_range(mon, sun, year, today, strings));
            (title, subtitle)
        }
        fern_core::Horizon::Month { year, month } => {
            let title = strings.months[(month.saturating_sub(1) as usize).min(11)].to_owned();
            let subtitle = (year != today.year()).then(|| year.to_string());
            (title, subtitle)
        }
        fern_core::Horizon::Quarter { year, quarter } => {
            let title = format!("{}{quarter}", strings.horizons_quarter_prefix);
            let subtitle = (year != today.year()).then(|| year.to_string());
            (title, subtitle)
        }
        fern_core::Horizon::Year(year) => (year.to_string(), None),
        fern_core::Horizon::Decade(start_year) => {
            let title = format!("{start_year}{}", strings.horizons_decade_suffix);
            let subtitle = Some(format!("{start_year}–{}", start_year + 9));
            (title, subtitle)
        }
        fern_core::Horizon::Life => (strings.horizons_life.to_owned(), None),
    }
}

/// Curated major IANA time zones shown when a timezone search query is empty.
pub const MAJOR_TIMEZONES: [&str; 29] = [
    "UTC",
    "America/New_York",
    "America/Chicago",
    "America/Denver",
    "America/Los_Angeles",
    "America/Toronto",
    "America/Sao_Paulo",
    "Europe/London",
    "Europe/Paris",
    "Europe/Berlin",
    "Europe/Madrid",
    "Europe/Rome",
    "Europe/Amsterdam",
    "Europe/Warsaw",
    "Europe/Kyiv",
    "Europe/Istanbul",
    "Europe/Moscow",
    "Asia/Dubai",
    "Asia/Almaty",
    "Asia/Kolkata",
    "Asia/Bangkok",
    "Asia/Singapore",
    "Asia/Shanghai",
    "Asia/Hong_Kong",
    "Asia/Taipei",
    "Asia/Tokyo",
    "Asia/Seoul",
    "Australia/Sydney",
    "Pacific/Auckland",
];

/// Returns `true` if `name` is a valid IANA time zone identifier.
pub fn is_valid_timezone(name: &str) -> bool {
    name.parse::<chrono_tz::Tz>().is_ok()
}

/// Canonicalizes a valid IANA time zone name (returns `None` if unrecognized).
pub fn canonical_timezone_name(name: &str) -> Option<&'static str> {
    name.parse::<chrono_tz::Tz>().ok().map(|tz| tz.name())
}

fn format_offset_minutes(mins: i32) -> String {
    let sign = if mins >= 0 { '+' } else { '-' };
    let abs = mins.abs();
    let (h, m) = (abs / 60, abs % 60);
    if m == 0 {
        format!("UTC{sign}{h}")
    } else {
        format!("UTC{sign}{h}:{m:02}")
    }
}

fn format_tz_offset(tz: chrono_tz::Tz) -> String {
    use chrono::{Offset as _, TimeZone as _, Utc};
    let now = Utc::now().naive_utc();
    let offset_secs = tz.offset_from_utc_datetime(&now).fix().local_minus_utc();
    format_offset_minutes(offset_secs / 60)
}

/// Formats the current UTC offset for a valid IANA time zone name (e.g. `"UTC+3"`, `"UTC+5:30"`).
pub fn timezone_offset_label(tz_name: &str) -> Option<String> {
    tz_name.parse::<chrono_tz::Tz>().ok().map(format_tz_offset)
}

/// Formats the UTC offset of `tz_name` at `date` 12:00 local time (e.g. `"UTC-4"`, `"UTC+5:30"`).
pub fn timezone_offset_label_on(date: NaiveDate, tz_name: &str) -> String {
    use chrono::{Offset as _, TimeZone as _};
    let Ok(tz) = tz_name.parse::<chrono_tz::Tz>() else {
        return String::new();
    };
    let Some(noon) = date.and_hms_opt(12, 0, 0) else {
        return String::new();
    };
    let Some(dt) = tz
        .from_local_datetime(&noon)
        .earliest()
        .or_else(|| tz.from_local_datetime(&noon).latest())
    else {
        return String::new();
    };
    format_offset_minutes(dt.offset().fix().local_minus_utc() / 60)
}

/// Difference in minutes between `target_tz_name` and `local_tz` at `date` 12:00 in `local_tz`.
pub fn timezone_offset_diff_minutes<L: chrono::TimeZone>(
    date: NaiveDate,
    local_tz: &L,
    target_tz_name: &str,
) -> Option<i32> {
    use chrono::Offset as _;
    let target_tz: chrono_tz::Tz = target_tz_name.parse().ok()?;
    let noon = date.and_hms_opt(12, 0, 0)?;
    let local_dt = local_tz
        .from_local_datetime(&noon)
        .earliest()
        .or_else(|| local_tz.from_local_datetime(&noon).latest())?;
    let target_dt = local_dt.with_timezone(&target_tz);
    let local_secs = local_dt.offset().fix().local_minus_utc();
    let target_secs = target_dt.offset().fix().local_minus_utc();
    Some((target_secs - local_secs) / 60)
}

/// Converts a local `(date, time)` into the wall-clock `(date, time)` in `tz_name`.
/// Falls back to `(date, time)` if `tz_name` is unrecognized or local conversion fails.
pub fn local_to_timezone(date: NaiveDate, time: NaiveTime, tz_name: &str) -> (NaiveDate, NaiveTime) {
    use chrono::TimeZone as _;
    let Ok(tz) = tz_name.parse::<chrono_tz::Tz>() else {
        return (date, time);
    };
    let naive = date.and_time(time);
    let local_dt = chrono::Local
        .from_local_datetime(&naive)
        .earliest()
        .or_else(|| chrono::Local.from_local_datetime(&naive).latest());
    match local_dt {
        Some(dt) => {
            let zoned = dt.with_timezone(&tz);
            (zoned.date_naive(), zoned.time())
        }
        None => (date, time),
    }
}

/// Lists `(iana_name, utc_offset)` pairs matching `query` (curated major zones when empty).
pub fn timezone_options(query: &str) -> Vec<(String, String)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return MAJOR_TIMEZONES
            .iter()
            .filter_map(|name| name.parse::<chrono_tz::Tz>().ok().map(|tz| ((*name).to_owned(), format_tz_offset(tz))))
            .collect();
    }
    chrono_tz::TZ_VARIANTS
        .iter()
        .filter(|tz| tz.name().to_lowercase().contains(&q))
        .take(40)
        .map(|tz| (tz.name().to_owned(), format_tz_offset(*tz)))
        .collect()
}

/// Lists up to `limit` IANA time zone names containing `query` (case-insensitive).
pub fn matching_timezone_names(query: &str, limit: usize) -> Vec<&'static str> {
    let q = query.trim().to_lowercase();
    chrono_tz::TZ_VARIANTS
        .iter()
        .map(|z| z.name())
        .filter(|name| name.to_lowercase().contains(&q))
        .take(limit)
        .collect()
}

/// Converts a local `DateTime` into the wall-clock `NaiveTime` of the named IANA time zone.
pub fn time_in_timezone(local: chrono::DateTime<chrono::Local>, tz_name: &str) -> Option<NaiveTime> {
    let tz: chrono_tz::Tz = tz_name.parse().ok()?;
    Some(local.with_timezone(&tz).time())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fern_i18n::{EN, RU, ZH_CN};

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
        let grid = month_grid_with(d(2026, 10, 18), Weekday::Mon);
        assert_eq!(grid.len(), 42);
        assert_eq!(grid[0], d(2026, 9, 28));
        assert_eq!(grid[3], d(2026, 10, 1));
        assert!(grid.contains(&d(2026, 10, 31)));
    }

    #[test]
    fn month_weeks_cover_only_the_month() {
        // October 2026 spans five weeks: Sep 28 – Nov 1.
        let weeks = month_weeks_with(d(2026, 10, 18), Weekday::Mon);
        assert_eq!(weeks.len(), 5);
        assert_eq!(weeks[0][0], d(2026, 9, 28));
        assert_eq!(weeks[4][6], d(2026, 11, 1));
        // February 2027 starts on a Monday and fits in four weeks.
        assert_eq!(month_weeks_with(d(2027, 2, 1), Weekday::Mon).len(), 4);
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
        let week = week_from(d(2026, 10, 18), Weekday::Mon); // a Sunday
        assert_eq!(week[0], d(2026, 10, 12));
        assert_eq!(week[6], d(2026, 10, 18));
        assert_eq!(week_from(d(2026, 10, 12), Weekday::Mon)[0], d(2026, 10, 12));
    }

    #[test]
    fn date_formats_and_week_numbers() {
        use fern_core::DateFormat::*;
        let date = d(2026, 12, 31);
        assert_eq!(format_date_as(date, Some(MonthDayYear), true, true), "12/31/2026");
        assert_eq!(format_date_as(date, Some(DayMonthYear), false, true), "31/12/2026");
        assert_eq!(format_date_as(date, Some(YearMonthDay), false, true), "2026/12/31");
        assert_eq!(format_date_as(date, Some(IsoYearMonthDay), true, true), "2026-12-31");
        assert_eq!(format_date_as(date, Some(IsoYearMonthDay), true, false), "12-31");
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
        assert_eq!(parse_date_with("2026-11-02", today, None, &fern_i18n::EN), Some(d(2026, 11, 2)));
        assert_eq!(parse_date_with("11/02/2026", today, None, &fern_i18n::EN), Some(d(2026, 11, 2)));
        assert_eq!(parse_date_with("11/2", today, None, &fern_i18n::EN), Some(d(2026, 11, 2)));
        assert_eq!(parse_date_with("2.11.2026", today, None, &fern_i18n::RU), Some(d(2026, 11, 2)));
        assert_eq!(parse_date_with("someday", today, None, &fern_i18n::EN), None);
    }

    #[gpui::test]
    fn chinese_dates_follow_the_locale_and_explicit_overrides(cx: &mut gpui::TestAppContext) {
        use fern_core::DateFormat::DayMonthYear;
        let date = d(2026, 11, 2);
        cx.update(|cx| {
            assert_eq!(format_date(date, &ZH_CN, cx), "2026/11/02");
            assert_eq!(format_day_month(date, &ZH_CN, cx), "11/02");
            assert_eq!(parse_date("2026/11/02", date, &ZH_CN, cx), Some(date));
            assert_eq!(parse_date("11/02", date, &ZH_CN, cx), Some(date));

            ActivePreferences::update(cx, |p| p.date_format = Some(DayMonthYear));
            assert_eq!(format_date(date, &ZH_CN, cx), "02/11/2026");
            assert_eq!(format_day_month(date, &ZH_CN, cx), "02/11");
            assert_eq!(parse_date("02/11/2026", date, &ZH_CN, cx), Some(date));
            assert_eq!(parse_date("02/11", date, &ZH_CN, cx), Some(date));

            ActivePreferences::update(cx, |p| p.date_format = Some(fern_core::DateFormat::IsoYearMonthDay));
            assert_eq!(format_date(date, &RU, cx), "2026-11-02");
            assert_eq!(parse_date("2026-11-02", date, &RU, cx), Some(date));
            assert_eq!(parse_date("11-02", date, &RU, cx), Some(date));
        });
    }

    #[gpui::test]
    fn twelve_and_twenty_four_hour_times(cx: &mut gpui::TestAppContext) {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        assert_eq!(format_time_as(t(13, 5), false, &EN), "13:05");
        assert_eq!(format_time_as(t(13, 5), true, &EN), "1:05 PM");
        assert_eq!(format_time_as(t(0, 30), true, &EN), "12:30 AM");
        assert_eq!(format_time_as(t(12, 0), true, &RU), "12:00 PM");
        assert_eq!(format_time_as(t(13, 5), false, &ZH_CN), "13:05");
        assert_eq!(format_time_as(t(0, 30), true, &ZH_CN), "上午12:30");
        assert_eq!(format_time_as(t(12, 0), true, &ZH_CN), "下午12:00");
        cx.update(|cx| {
            ActivePreferences::update(cx, |p| p.hour12 = true);
            assert_eq!(format_hour(9, &EN, cx), "9 AM");
            assert_eq!(format_hour(9, &ZH_CN, cx), "上午9");
            assert_eq!(format_time(t(21, 0), &EN, cx), "9:00 PM");
            assert_eq!(format_time(t(21, 0), &ZH_CN, cx), "下午9:00");
            ActivePreferences::update(cx, |p| p.hour12 = false);
            assert_eq!(format_hour(9, &EN, cx), "09:00");
        });
    }

    #[gpui::test]
    fn weeks_can_start_on_sunday_or_saturday(cx: &mut gpui::TestAppContext) {
        let sunday = d(2026, 10, 18);
        assert_eq!(week_from(sunday, Weekday::Sun)[0], sunday);
        assert_eq!(week_from(d(2026, 10, 24), Weekday::Sun)[6], d(2026, 10, 24));
        assert_eq!(week_from(sunday, Weekday::Sat)[0], d(2026, 10, 17));

        cx.update(|cx| {
            ActivePreferences::update(cx, |p| p.week_start = Weekday::Sun);
            assert_eq!(month_grid(d(2026, 10, 1), cx)[0], d(2026, 9, 27));
            assert_eq!(weekday_labels(&["M", "T", "W", "T", "F", "S", "Su"], cx)[0], "Su");
        });
    }

    #[test]
    fn month_navigation_and_titles() {
        assert_eq!(add_months(d(2026, 1, 31), 1), d(2026, 2, 1));
        assert_eq!(add_months(d(2026, 1, 15), -1), d(2025, 12, 1));
        assert_eq!(month_title(d(2026, 10, 18), &EN), "October 2026");
        assert_eq!(month_title(d(2026, 10, 18), &RU), "Октябрь 2026");
        assert_eq!(month_title(d(2026, 10, 18), &ZH_CN), "2026年10月");
    }

    #[test]
    fn multi_week_weeks() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let weeks = weeks_from_with(day, 5, Weekday::Mon);
        assert_eq!(weeks.len(), 5);
        assert_eq!(weeks[0], week_from(day, Weekday::Mon));
        assert_eq!(weeks[4][0], weeks[0][0] + Duration::weeks(4));
    }

    #[test]
    fn week_axis_offset_round_trips_and_formats_secondary_tz() {
        let t930 = NaiveTime::from_hms_opt(9, 30, 0).unwrap();
        assert_eq!(week_axis::time_at_offset(week_axis::offset(t930)), t930);
        let t2145 = NaiveTime::from_hms_opt(21, 45, 0).unwrap();
        assert_eq!(week_axis::time_at_offset(week_axis::offset(t2145)), t2145);

        let (h, m, _delta) = week_axis::hour_in_timezone(d(2026, 10, 18), 12, "UTC");
        assert!(h < 24 && m == 0);
    }

    #[test]
    fn horizon_badges_localize_across_locales() {
        use fern_core::Horizon;
        let week = Horizon::Week { year: 2026, week: 48 };
        let month = Horizon::Month { year: 2026, month: 11 };
        let quarter = Horizon::Quarter { year: 2026, quarter: 4 };
        let year = Horizon::Year(2026);
        let decade = Horizon::Decade(2020);

        assert_eq!(format_horizon_badge(week, &EN), "W48");
        assert_eq!(format_horizon_badge(week, &RU), "Н48");
        assert_eq!(format_horizon_badge(week, &ZH_CN), "第48周");

        assert_eq!(format_horizon_badge(month, &EN), "Nov");
        assert_eq!(format_horizon_badge(month, &RU), "Ноя");
        assert_eq!(format_horizon_badge(month, &ZH_CN), "11月");

        assert_eq!(format_horizon_badge(quarter, &EN), "Q4 2026");
        assert_eq!(format_horizon_badge(quarter, &RU), "Q4 2026");
        assert_eq!(format_horizon_badge(quarter, &ZH_CN), "Q4 2026");

        assert_eq!(format_horizon_badge(year, &EN), "2026");
        assert_eq!(format_horizon_badge(decade, &EN), "2020s");
        assert_eq!(format_horizon_badge(decade, &RU), "2020-е");
        assert_eq!(format_horizon_badge(decade, &ZH_CN), "2020年代");

        assert_eq!(format_horizon_badge(Horizon::Life, &EN), "Life");
        assert_eq!(format_horizon_badge(Horizon::Life, &RU), "Жизнь");
        assert_eq!(format_horizon_badge(Horizon::Life, &ZH_CN), "人生");
    }

    #[test]
    fn horizon_headings_format_titles_and_subtitles() {
        use fern_core::Horizon;
        let today = d(2026, 11, 23);

        // Same-month ISO week (2026-W48 = Nov 23..Nov 29)
        assert_eq!(
            format_horizon_heading(Horizon::Week { year: 2026, week: 48 }, today, &EN),
            ("W48".into(), Some("Nov 23–29".into()))
        );
        assert_eq!(
            format_horizon_heading(Horizon::Week { year: 2026, week: 48 }, today, &RU),
            ("Н48".into(), Some("23–29 ноя".into()))
        );
        assert_eq!(
            format_horizon_heading(Horizon::Week { year: 2026, week: 48 }, today, &ZH_CN),
            ("第48周".into(), Some("11月23–29日".into()))
        );

        // Cross-month ISO week (2026-W49 = Nov 30..Dec 6)
        assert_eq!(
            format_horizon_heading(Horizon::Week { year: 2026, week: 49 }, today, &EN),
            ("W49".into(), Some("Nov 30 – Dec 6".into()))
        );

        // Month & Quarter omit current year and show other years
        assert_eq!(
            format_horizon_heading(Horizon::Month { year: 2026, month: 11 }, today, &RU),
            ("Ноябрь".into(), None)
        );
        assert_eq!(
            format_horizon_heading(Horizon::Month { year: 2027, month: 1 }, today, &EN),
            ("January".into(), Some("2027".into()))
        );
        assert_eq!(
            format_horizon_heading(Horizon::Quarter { year: 2026, quarter: 4 }, today, &EN),
            ("Q4".into(), None)
        );
        assert_eq!(
            format_horizon_heading(Horizon::Quarter { year: 2027, quarter: 1 }, today, &EN),
            ("Q1".into(), Some("2027".into()))
        );

        // Year, Decade, Life
        assert_eq!(format_horizon_heading(Horizon::Year(2026), today, &EN), ("2026".into(), None));
        assert_eq!(
            format_horizon_heading(Horizon::Decade(2020), today, &RU),
            ("2020-е".into(), Some("2020–2029".into()))
        );
        assert_eq!(format_horizon_heading(Horizon::Life, today, &RU), ("Жизнь".into(), None));
    }

    #[gpui::test]
    fn horizons_enabled_flag_toggles_and_restores(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert!(horizons_enabled(cx));
            ActivePreferences::update(cx, |p| p.horizons_enabled = false);
            assert!(!horizons_enabled(cx));
            ActivePreferences::update(cx, |p| p.horizons_enabled = true);
            assert!(horizons_enabled(cx));
        });
    }
}
