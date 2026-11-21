//! Parsing of the quick-add line: "Buy milk tomorrow at 6pm #home !1".
//!
//! Recognized tokens (English and Russian keywords work in any UI language):
//! - tags: `#home`;
//! - list: `^work` (only existing lists; otherwise the word stays in the title);
//! - priority: `!1`/`!2`/`!3` (high/medium/low), `!high`, `!medium`, `!low`, `!высокий`, ...;
//! - dates: today, tomorrow, weekdays, "in 3 days", "in 2 weeks", `13.11`, `13.11.2026`,
//!   `2026-11-13`, "13 nov", and Russian equivalents ("завтра", "в пятницу", "через неделю");
//! - time: `18:00`, `6pm`, `6:30am`, `18ч`;
//! - repeat: "every day", "every 2 weeks", "daily", "weekly", "каждый день", "ежемесячно".
//!
//! Recognized words, together with a preposition right before them ("at", "on", "в"), are
//! removed from the title.

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveTime, Weekday};

use crate::model::Priority;
use crate::recurrence::{Frequency, Recurrence};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedTask {
    pub title: String,
    pub due_date: Option<NaiveDate>,
    /// A date was typed, rather than inferred as today from a time alone.
    pub explicit_date: bool,
    pub due_time: Option<NaiveTime>,
    pub priority: Option<Priority>,
    pub tags: Vec<String>,
    pub repeat: Option<Recurrence>,
    /// The list name as the user spelled it when creating the list.
    pub list: Option<String>,
    /// Byte ranges of the recognized words in the input, neighbouring words joined, for
    /// highlighting them as the user types ("tomorrow 9am", "#work").
    pub spans: Vec<std::ops::Range<usize>>,
}

/// `lists` holds the names of existing lists; `^name` is recognized only for those.
/// Settings → More → Smart Recognition: what quick add recognizes, and whether the recognized
/// text is removed from the title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recognition {
    pub dates: bool,
    pub remove_date_text: bool,
    pub tags: bool,
    pub remove_tag_text: bool,
}

impl Default for Recognition {
    fn default() -> Self {
        Self { dates: true, remove_date_text: true, tags: true, remove_tag_text: true }
    }
}

pub fn parse_quick_add(input: &str, today: NaiveDate, lists: &[String]) -> ParsedTask {
    parse_quick_add_with(input, today, lists, Recognition::default())
}

pub fn parse_quick_add_with(input: &str, today: NaiveDate, lists: &[String], recognition: Recognition) -> ParsedTask {
    parse_quick_add_context(input, today, lists, recognition, false)
}

/// A calendar cell already supplies the date. Recognize time without replacing that date
/// or consuming date/repeat words from the title, as the calendar's time-only editor does.
pub fn parse_quick_add_on_day(input: &str, day: NaiveDate, lists: &[String], recognition: Recognition) -> ParsedTask {
    parse_quick_add_context(input, day, lists, recognition, true)
}

fn parse_quick_add_context(input: &str, today: NaiveDate, lists: &[String], recognition: Recognition, time_only: bool) -> ParsedTask {
    let words: Vec<&str> = input.split_whitespace().collect();
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut used = vec![false; words.len()];
    let mut parsed = ParsedTask::default();
    // With "Remove text in tasks" off, recognized date words are marked here and stay in the title.
    let mut dates_used = vec![false; words.len()];

    let mut i = 0;
    while i < words.len() {
        let word = lower[i].as_str();

        if recognition.tags
            && let Some(tag) = words[i].strip_prefix('#').filter(|t| !t.is_empty())
        {
            if !parsed.tags.iter().any(|t: &String| t.to_lowercase() == tag.to_lowercase()) {
                parsed.tags.push(tag.to_owned());
            }
            used[i] = recognition.remove_tag_text;
        } else if let Some(list) = word
            .strip_prefix('^')
            .or_else(|| word.strip_prefix('~'))
            .and_then(|name| lists.iter().find(|l| l.to_lowercase() == name))
        {
            parsed.list = Some(list.clone());
            used[i] = true;
        } else if let Some(priority) = word.strip_prefix('!').and_then(parse_priority) {
            parsed.priority = Some(priority);
            used[i] = true;
        } else if recognition.dates
            && !time_only
            && parsed.repeat.is_none()
            && let Some((repeat, implied_weekday, len)) = parse_repeat(&lower[i..])
        {
            parsed.repeat = Some(repeat);
            if let Some(wd) = implied_weekday
                && parsed.due_date.is_none()
            {
                let ahead = (wd.num_days_from_monday() as i64
                    - today.weekday().num_days_from_monday() as i64)
                    .rem_euclid(7);
                parsed.due_date = Some(today + Duration::days(ahead));
            }
            if recognition.remove_date_text {
                used[i..i + len].iter_mut().for_each(|u| *u = true);
            } else {
                dates_used[i..i + len].iter_mut().for_each(|u| *u = true);
            }
            i += len;
            continue;
        } else if recognition.dates
            && parsed.due_time.is_none()
            && let Some(time) = parse_time(word)
        {
            parsed.due_time = Some(time);
            mark_with_preposition(if recognition.remove_date_text { &mut used } else { &mut dates_used }, &lower, i, 1);
        } else if recognition.dates
            && !time_only
            && parsed.due_date.is_none()
            && let Some((date, len)) = parse_date(&lower[i..], today)
        {
            parsed.due_date = Some(date);
            parsed.explicit_date = true;
            mark_with_preposition(if recognition.remove_date_text { &mut used } else { &mut dates_used }, &lower, i, len);
            i += len;
            continue;
        }
        i += 1;
    }

    // A time without a date means today, as in TickTick.
    if parsed.due_time.is_some() && parsed.due_date.is_none() {
        parsed.due_date = Some(today);
    }

    // Recognized words are those marked used, and kept date words; tags kept in the title too.
    let starts: Vec<usize> = words.iter().map(|w| w.as_ptr() as usize - input.as_ptr() as usize).collect();
    let recognized = |i: usize| {
        used[i] || dates_used[i] || (recognition.tags && words[i].len() > 1 && words[i].starts_with('#'))
    };
    for i in 0..words.len() {
        if !recognized(i) {
            continue;
        }
        let (start, end) = (starts[i], starts[i] + words[i].len());
        match parsed.spans.last_mut() {
            Some(last) if i > 0 && recognized(i - 1) && !words[i].starts_with('#') && !words[i - 1].starts_with('#') => last.end = end,
            _ => parsed.spans.push(start..end),
        }
    }

    parsed.title = words
        .iter()
        .zip(&used)
        .filter(|(_, used)| !**used)
        .map(|(w, _)| *w)
        .collect::<Vec<_>>()
        .join(" ");
    parsed
}

/// Marks `len` words starting at `start`, plus a preposition right before them.
fn mark_with_preposition(used: &mut [bool], lower: &[String], start: usize, len: usize) {
    used[start..start + len].iter_mut().for_each(|u| *u = true);
    if start > 0
        && !used[start - 1]
        && matches!(lower[start - 1].as_str(), "в" | "во" | "на" | "к" | "до" | "at" | "on" | "by" | "next")
    {
        used[start - 1] = true;
    }
}

/// A repeat rule at the start of `words`, an optional implied weekday (e.g. "every monday"),
/// and how many words it took.
fn parse_repeat(words: &[String]) -> Option<(Recurrence, Option<Weekday>, usize)> {
    let single = |word: &str| {
        Some(match word {
            "daily" | "ежедневно" | "每天" => Frequency::Daily,
            "weekly" | "еженедельно" | "每周" => Frequency::Weekly,
            "monthly" | "ежемесячно" | "每月" => Frequency::Monthly,
            "yearly" | "annually" | "ежегодно" | "每年" => Frequency::Yearly,
            _ => return None,
        })
    };
    if let Some(frequency) = single(&words[0]) {
        return Some((Recurrence::new(frequency, 1), None, 1));
    }
    if matches!(words[0].as_str(), "biweekly" | "fortnightly" | "每两周") {
        return Some((Recurrence::new(Frequency::Weekly, 2), None, 1));
    }
    if words[0] == "每周末" {
        let mut r = Recurrence::new(Frequency::Weekly, 1);
        r.weekdays = 0b0110_0000;
        return Some((r, Some(Weekday::Sat), 1));
    }
    if let Some(rest) = words[0].strip_prefix("每")
        && let Some(weekday) = parse_weekday(rest)
    {
        let mut r = Recurrence::new(Frequency::Weekly, 1);
        r.weekdays = 1u8 << weekday.num_days_from_monday();
        return Some((r, Some(weekday), 1));
    }
    if matches!(words[0].as_str(), "weekdays" | "workdays" | "工作日") {
        let mut r = Recurrence::new(Frequency::Weekly, 1);
        r.weekdays = 0b0001_1111;
        return Some((r, None, 1));
    }
    if words[0] == "по" && words.len() >= 2 && matches!(words[1].as_str(), "будням" | "будни") {
        let mut r = Recurrence::new(Frequency::Weekly, 1);
        r.weekdays = 0b0001_1111;
        return Some((r, None, 2));
    }
    if words[0] == "по" && words.len() >= 2 && matches!(words[1].as_str(), "выходным" | "выходные") {
        let mut r = Recurrence::new(Frequency::Weekly, 1);
        r.weekdays = 0b0110_0000;
        return Some((r, Some(Weekday::Sat), 2));
    }
    if !matches!(words[0].as_str(), "every" | "каждый" | "каждую" | "каждое" | "каждые") || words.len() < 2 {
        return None;
    }
    if matches!(words[1].as_str(), "weekday" | "workday" | "будний" | "рабочий") {
        let mut r = Recurrence::new(Frequency::Weekly, 1);
        r.weekdays = 0b0001_1111;
        let len = if words.len() >= 3 && matches!(words[2].as_str(), "день" | "day") { 3 } else { 2 };
        return Some((r, None, len));
    }
    if matches!(words[1].as_str(), "weekend" | "выходные") {
        let mut r = Recurrence::new(Frequency::Weekly, 1);
        r.weekdays = 0b0110_0000;
        return Some((r, Some(Weekday::Sat), 2));
    }
    if let Some(weekday) = parse_weekday(&words[1]) {
        let mut r = Recurrence::new(Frequency::Weekly, 1);
        r.weekdays = 1u8 << weekday.num_days_from_monday();
        return Some((r, Some(weekday), 2));
    }
    let (interval, unit, len) = if words[1] == "other" && words.len() >= 3 {
        (2, words[2].as_str(), 3)
    } else {
        match words[1].parse::<u32>() {
            Ok(n) if words.len() >= 3 => (n, words[2].as_str(), 3),
            Ok(_) => return None,
            Err(_) => (1, words[1].as_str(), 2),
        }
    };
    let frequency = if unit.starts_with("day") || unit.starts_with("дн") || unit.starts_with("ден") {
        Frequency::Daily
    } else if unit.starts_with("week") || unit.starts_with("недел") {
        Frequency::Weekly
    } else if unit.starts_with("month") || unit.starts_with("месяц") {
        Frequency::Monthly
    } else if unit.starts_with("year") || unit.starts_with("год") || unit.starts_with("лет") {
        Frequency::Yearly
    } else {
        return None;
    };
    Some((Recurrence::new(frequency, interval), None, len))
}

fn parse_priority(value: &str) -> Option<Priority> {
    match value {
        "1" | "p1" | "h" | "high" | "!!" | "высокий" | "в" | "高" => Some(Priority::High),
        "2" | "p2" | "m" | "med" | "medium" | "!" | "средний" | "с" | "中" => Some(Priority::Medium),
        "3" | "p3" | "l" | "low" | "низкий" | "н" | "低" => Some(Priority::Low),
        _ => None,
    }
}

fn parse_time(word: &str) -> Option<NaiveTime> {
    match word {
        "morning" | "утром" | "早上" => return NaiveTime::from_hms_opt(9, 0, 0),
        "noon" | "днём" | "днем" | "中午" => return NaiveTime::from_hms_opt(12, 0, 0),
        "afternoon" | "下午" => return NaiveTime::from_hms_opt(14, 0, 0),
        "evening" | "tonight" | "вечером" | "晚上" => return NaiveTime::from_hms_opt(19, 0, 0),
        _ => {}
    }
    let (clock, pm) = match word.strip_suffix("pm") {
        Some(clock) => (clock, Some(true)),
        None => match word.strip_suffix("am") {
            Some(clock) => (clock, Some(false)),
            None => (word.strip_suffix("ч").unwrap_or(word), None),
        },
    };
    let has_suffix = clock.len() != word.len();
    let (hours, minutes) = match clock.split_once(':') {
        Some((h, m)) if m.len() == 2 => (h, m.parse::<u32>().ok()?),
        Some(_) => return None,
        // A bare number is a time only with a suffix: "6pm", "18ч".
        None if has_suffix => (clock, 0),
        None => return None,
    };
    if hours.is_empty() || hours.len() > 2 {
        return None;
    }
    let mut hours: u32 = hours.parse().ok()?;
    if let Some(pm) = pm {
        if !(1..=12).contains(&hours) {
            return None;
        }
        hours = match (hours, pm) {
            (12, false) => 0,
            (12, true) => 12,
            (h, true) => h + 12,
            (h, false) => h,
        };
    }
    NaiveTime::from_hms_opt(hours, minutes, 0)
}

/// A date at the start of `words` and how many words it took.
fn parse_date(words: &[String], today: NaiveDate) -> Option<(NaiveDate, usize)> {
    let first = words[0].as_str();
    match first {
        "сегодня" | "today" | "今天" => return Some((today, 1)),
        "завтра" | "tomorrow" | "明天" => return Some((today + Duration::days(1), 1)),
        "послезавтра" | "后天" => return Some((today + Duration::days(2), 1)),
        "下周" => return Some((today + Duration::days(7), 1)),
        "下个月" => return today.checked_add_months(Months::new(1)).map(|d| (d, 1)),
        "明年" => return today.checked_add_months(Months::new(12)).map(|d| (d, 1)),
        "weekend" | "выходные" | "выходных" | "周末" => {
            let ahead = (Weekday::Sat.num_days_from_monday() as i64
                - today.weekday().num_days_from_monday() as i64)
                .rem_euclid(7);
            return Some((today + Duration::days(ahead), 1));
        }
        _ => {}
    }
    if matches!(first, "this" | "эти" | "этих")
        && words.len() >= 2
        && matches!(words[1].as_str(), "weekend" | "выходные" | "выходных")
    {
        let ahead = (Weekday::Sat.num_days_from_monday() as i64
            - today.weekday().num_days_from_monday() as i64)
            .rem_euclid(7);
        return Some((today + Duration::days(ahead), 2));
    }
    if matches!(first, "next" | "следующая" | "следующую" | "следующей" | "следующий" | "следующем")
        && words.len() >= 2
    {
        if matches!(words[1].as_str(), "week" | "неделя" | "неделю" | "неделе") {
            return Some((today + Duration::days(7), 2));
        }
        if matches!(words[1].as_str(), "month" | "месяц" | "месяце") {
            return today.checked_add_months(Months::new(1)).map(|d| (d, 2));
        }
        if matches!(words[1].as_str(), "year" | "год" | "году") {
            return today.checked_add_months(Months::new(12)).map(|d| (d, 2));
        }
    }
    if let Some(weekday) = parse_weekday(first) {
        let ahead = (weekday.num_days_from_monday() as i64
            - today.weekday().num_days_from_monday() as i64)
            .rem_euclid(7);
        // The same weekday as today means next week.
        let ahead = if ahead == 0 { 7 } else { ahead };
        return Some((today + Duration::days(ahead), 1));
    }
    if matches!(first, "через" | "in") && words.len() >= 2 {
        let (count, unit, len) = match words[1].parse::<u32>() {
            Ok(n) if words.len() >= 3 => (n, words[2].as_str(), 3),
            Ok(_) => return None,
            Err(_) => (1, words[1].as_str(), 2),
        };
        return shift(today, count, unit).map(|date| (date, len));
    }
    if let Some(stem) = first.strip_suffix("天后")
        && let Ok(n) = stem.parse::<u32>()
    {
        return Some((today + Duration::days(n as i64), 1));
    }
    if let Some(stem) = first.strip_suffix("周后")
        && let Ok(n) = stem.parse::<u32>()
    {
        return Some((today + Duration::weeks(n as i64), 1));
    }
    if let Some(stem) = first.strip_suffix("个月后")
        && let Ok(n) = stem.parse::<u32>()
    {
        return today.checked_add_months(Months::new(n)).map(|d| (d, 1));
    }
    if let Some(date) = parse_numeric_date(first, today) {
        return Some((date, 1));
    }
    if words.len() >= 2
        && let Some(month) = parse_month(&words[1])
    {
        let raw_day = first
            .strip_suffix("st")
            .or_else(|| first.strip_suffix("nd"))
            .or_else(|| first.strip_suffix("rd"))
            .or_else(|| first.strip_suffix("th"))
            .unwrap_or(first);
        if let Ok(day) = raw_day.parse::<u32>() {
            let date = NaiveDate::from_ymd_opt(today.year(), month, day)?;
            // "13 March" said in November means March of next year.
            let date = if date < today { NaiveDate::from_ymd_opt(today.year() + 1, month, day)? } else { date };
            return Some((date, 2));
        }
    }
    if words.len() >= 2
        && let Some(month) = parse_month(first)
    {
        let raw_day = words[1]
            .strip_suffix("st")
            .or_else(|| words[1].strip_suffix("nd"))
            .or_else(|| words[1].strip_suffix("rd"))
            .or_else(|| words[1].strip_suffix("th"))
            .unwrap_or(&words[1]);
        if let Ok(day) = raw_day.parse::<u32>() {
            let date = NaiveDate::from_ymd_opt(today.year(), month, day)?;
            let date = if date < today { NaiveDate::from_ymd_opt(today.year() + 1, month, day)? } else { date };
            return Some((date, 2));
        }
    }
    None
}

fn shift(today: NaiveDate, count: u32, unit: &str) -> Option<NaiveDate> {
    let unit = unit.trim_end_matches(|c: char| !c.is_alphabetic());
    if unit.starts_with("дн") || unit.starts_with("ден") || unit.starts_with("day") {
        Some(today + Duration::days(count as i64))
    } else if unit.starts_with("недел") || unit.starts_with("week") {
        Some(today + Duration::weeks(count as i64))
    } else if unit.starts_with("месяц") || unit.starts_with("month") {
        today.checked_add_months(Months::new(count))
    } else if unit.starts_with("год") || unit.starts_with("лет") || unit.starts_with("year") {
        today.checked_add_months(Months::new(12 * count))
    } else {
        None
    }
}

fn parse_numeric_date(word: &str, today: NaiveDate) -> Option<NaiveDate> {
    if let Ok(date) = NaiveDate::parse_from_str(word, "%Y-%m-%d") {
        return Some(date);
    }
    if let Ok(date) = NaiveDate::parse_from_str(word, "%Y/%m/%d") {
        return Some(date);
    }
    let num = |s: &str| s.parse::<u32>().ok();
    let dot_parts: Vec<&str> = word.split('.').collect();
    match dot_parts.as_slice() {
        [d, m] if d.len() <= 2 && m.len() <= 2 => {
            let date = NaiveDate::from_ymd_opt(today.year(), num(m)?, num(d)?)?;
            return if date < today { NaiveDate::from_ymd_opt(today.year() + 1, num(m)?, num(d)?) } else { Some(date) };
        }
        [d, m, y] if d.len() <= 2 && m.len() <= 2 && y.len() == 4 => {
            return NaiveDate::from_ymd_opt(y.parse().ok()?, num(m)?, num(d)?);
        }
        _ => {}
    }
    let slash_parts: Vec<&str> = word.split('/').collect();
    match slash_parts.as_slice() {
        [m, d] if m.len() <= 2 && d.len() <= 2 => {
            let date = NaiveDate::from_ymd_opt(today.year(), num(m)?, num(d)?)?;
            if date < today { NaiveDate::from_ymd_opt(today.year() + 1, num(m)?, num(d)?) } else { Some(date) }
        }
        [m, d, y] if m.len() <= 2 && d.len() <= 2 && y.len() == 4 => {
            NaiveDate::from_ymd_opt(y.parse().ok()?, num(m)?, num(d)?)
        }
        _ => None,
    }
}

fn parse_weekday(word: &str) -> Option<Weekday> {
    Some(match word {
        "пн" | "понедельник" | "mon" | "monday" | "周一" | "星期一" => Weekday::Mon,
        "вт" | "вторник" | "tue" | "tuesday" | "周二" | "星期二" => Weekday::Tue,
        "ср" | "среда" | "среду" | "wed" | "wednesday" | "周三" | "星期三" => Weekday::Wed,
        "чт" | "четверг" | "thu" | "thursday" | "周四" | "星期四" => Weekday::Thu,
        "пт" | "пятница" | "пятницу" | "fri" | "friday" | "周五" | "星期五" => Weekday::Fri,
        "сб" | "суббота" | "субботу" | "sat" | "saturday" | "周六" | "星期六" => Weekday::Sat,
        "вс" | "воскресенье" | "sun" | "sunday" | "周日" | "星期日" | "星期天" => Weekday::Sun,
        _ => return None,
    })
}

/// Month by full name or common abbreviation (English, Russian nominative and genitive).
/// Exact forms only, so that "5 marbles" is not read as 5 March.
fn parse_month(word: &str) -> Option<u32> {
    const FORMS: [&[&str]; 12] = [
        &["jan", "january", "янв", "январь", "января"],
        &["feb", "february", "фев", "февр", "февраль", "февраля"],
        &["mar", "march", "мар", "март", "марта"],
        &["apr", "april", "апр", "апрель", "апреля"],
        &["may", "май", "мая"],
        &["jun", "june", "июн", "июнь", "июня"],
        &["jul", "july", "июл", "июль", "июля"],
        &["aug", "august", "авг", "август", "августа"],
        &["sep", "sept", "september", "сен", "сент", "сентябрь", "сентября"],
        &["oct", "october", "окт", "октябрь", "октября"],
        &["nov", "november", "ноя", "нояб", "ноябрь", "ноября"],
        &["dec", "december", "дек", "декабрь", "декабря"],
    ];
    let word = word.trim_end_matches('.');
    FORMS.iter().position(|forms| forms.contains(&word)).map(|i| i as u32 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_cell_recognizes_time_without_consuming_date_words() {
        let day = NaiveDate::from_ymd_opt(2028, 7, 6).unwrap();
        let parsed = parse_quick_add_on_day("Plan tomorrow 9am #work", day, &[], Recognition::default());
        assert_eq!(parsed.title, "Plan tomorrow");
        assert_eq!(parsed.due_date, Some(day));
        assert_eq!(parsed.due_time, NaiveTime::from_hms_opt(9, 0, 0));
        assert!(!parsed.explicit_date);
        assert_eq!(parsed.tags, ["work"]);
    }

    // 18 October 2026 is a Sunday.
    fn today() -> NaiveDate {
        d(2026, 10, 18)
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn t(h: u32, m: u32) -> Option<NaiveTime> {
        NaiveTime::from_hms_opt(h, m, 0)
    }

    fn date(input: &str) -> Option<NaiveDate> {
        parse_quick_add(input, today(), &[]).due_date
    }

    #[test]
    fn plain_title_is_untouched() {
        let p = parse_quick_add("  Buy   milk ", today(), &[]);
        assert_eq!(p, ParsedTask { title: "Buy milk".into(), ..Default::default() });
    }

    #[test]
    fn full_example() {
        let p = parse_quick_add("Buy milk tomorrow at 6pm #home !1", today(), &[]);
        assert_eq!(p.title, "Buy milk");
        assert_eq!(p.due_date, Some(d(2026, 10, 19)));
        assert_eq!(p.due_time, t(18, 0));
        assert_eq!(p.tags, ["home"]);
        assert_eq!(p.priority, Some(Priority::High));
    }

    #[test]
    fn relative_dates() {
        assert_eq!(date("x today"), Some(today()));
        assert_eq!(date("x in 3 days"), Some(d(2026, 10, 21)));
        assert_eq!(date("x in 2 weeks"), Some(d(2026, 11, 1)));
        assert_eq!(date("x in 1 month"), Some(d(2026, 11, 18)));
    }

    #[test]
    fn weekdays_point_forward() {
        assert_eq!(date("report on friday"), Some(d(2026, 10, 23)));
        assert_eq!(date("report mon"), Some(d(2026, 10, 19)));
        // Today is Sunday, so "sunday" means next week.
        assert_eq!(date("report next sunday"), Some(d(2026, 10, 25)));
        assert_eq!(parse_quick_add("report next sunday", today(), &[]).title, "report");
    }

    #[test]
    fn absolute_dates() {
        assert_eq!(date("x 13.11"), Some(d(2026, 11, 13)));
        assert_eq!(date("x 01.03"), Some(d(2027, 3, 1)));
        assert_eq!(date("x 13.11.2027"), Some(d(2027, 11, 13)));
        assert_eq!(date("x 2026-12-31"), Some(d(2026, 12, 31)));
        assert_eq!(date("x 13 nov"), Some(d(2026, 11, 13)));
        assert_eq!(date("x nov 13"), Some(d(2026, 11, 13)));
        assert_eq!(date("x nov 13th"), Some(d(2026, 11, 13)));
        assert_eq!(date("x march 5"), Some(d(2027, 3, 5)));
        assert_eq!(date("x by 5 march"), Some(d(2027, 3, 5)));
        assert_eq!(date("x next month"), Some(d(2026, 11, 18)));
        assert_eq!(date("x следующий месяц"), Some(d(2026, 11, 18)));
        assert_eq!(date("x 下个月"), Some(d(2026, 11, 18)));
        // 2026-10-18 is Sunday, so upcoming Saturday is 2026-10-24.
        assert_eq!(date("x this weekend"), Some(d(2026, 10, 24)));
        assert_eq!(date("x выходные"), Some(d(2026, 10, 24)));
        assert_eq!(parse_quick_add("call mom завтра вечером", today(), &[]).due_time, t(19, 0));
        assert_eq!(parse_quick_add("standup tomorrow morning", today(), &[]).due_time, t(9, 0));
        assert_eq!(parse_quick_add("Pay rent by 5 march", today(), &[]).title, "Pay rent");
    }

    #[test]
    fn times() {
        let time = |s: &str| parse_quick_add(s, today(), &[]).due_time;
        assert_eq!(time("x 9:05"), t(9, 5));
        assert_eq!(time("x 6:30am"), t(6, 30));
        assert_eq!(time("x 12am"), t(0, 0));
        assert_eq!(time("x 12pm"), t(12, 0));
        assert_eq!(time("x 13pm"), None);
        // A time without a date is for today.
        let p = parse_quick_add("standup 9am", today(), &[]);
        assert_eq!((p.title.as_str(), p.due_date, p.due_time), ("standup", Some(today()), t(9, 0)));
    }

    #[test]
    fn russian_keywords() {
        let p = parse_quick_add("Купить молоко завтра в 18:00 #дом !высокий", today(), &[]);
        assert_eq!(p.title, "Купить молоко");
        assert_eq!((p.due_date, p.due_time), (Some(d(2026, 10, 19)), t(18, 0)));
        assert_eq!(p.priority, Some(Priority::High));
        assert_eq!(date("x послезавтра"), Some(d(2026, 10, 20)));
        assert_eq!(date("x через 3 дня"), Some(d(2026, 10, 21)));
        assert_eq!(date("x через неделю"), Some(d(2026, 10, 25)));
        assert_eq!(date("x в пятницу"), Some(d(2026, 10, 23)));
        assert_eq!(date("x 13 ноября"), Some(d(2026, 11, 13)));
        assert_eq!(parse_quick_add("x 9ч", today(), &[]).due_time, t(9, 0));
    }

    #[test]
    fn numbers_and_words_that_are_not_dates_stay() {
        let p = parse_quick_add("Buy 3 apples at the store", today(), &[]);
        assert_eq!((p.title.as_str(), p.due_date), ("Buy 3 apples at the store", None));
        let p = parse_quick_add("Read 25:99 and #", today(), &[]);
        assert_eq!(p.title, "Read 25:99 and #");
        assert_eq!(parse_quick_add("Read page 18", today(), &[]).due_time, None);
        assert_eq!(date("Buy 5 marbles"), None);
    }

    #[test]
    fn repeats() {
        let repeat = |s: &str| parse_quick_add(s, today(), &[]).repeat;
        assert_eq!(repeat("x every day"), Some(Recurrence::new(Frequency::Daily, 1)));
        assert_eq!(repeat("x every 2 weeks"), Some(Recurrence::new(Frequency::Weekly, 2)));
        assert_eq!(repeat("x monthly"), Some(Recurrence::new(Frequency::Monthly, 1)));
        assert_eq!(repeat("x каждую неделю"), Some(Recurrence::new(Frequency::Weekly, 1)));
        assert_eq!(repeat("x ежегодно"), Some(Recurrence::new(Frequency::Yearly, 1)));
        assert_eq!(repeat("x every time"), None);
        let p = parse_quick_add("Water the plants every 3 days at 9am", today(), &[]);
        assert_eq!(p.title, "Water the plants");
        assert_eq!(p.repeat, Some(Recurrence::new(Frequency::Daily, 3)));
        assert_eq!(p.due_time, t(9, 0));
    }

    #[test]
    fn known_lists_only() {
        let lists = ["Work".to_owned()];
        let p = parse_quick_add("Report ^work", today(), &lists);
        assert_eq!((p.title.as_str(), p.list.as_deref()), ("Report", Some("Work")));
        let p = parse_quick_add("Report ^garden", today(), &lists);
        assert_eq!((p.title.as_str(), p.list), ("Report ^garden", None));
    }

    #[test]
    fn recognized_spans() {
        let lists: [String; 0] = [];
        let input = "Call mom tomorrow 9am #work";
        let parsed = parse_quick_add(input, today(), &lists);
        let spans: Vec<&str> = parsed.spans.iter().map(|r| &input[r.clone()]).collect();
        assert_eq!(spans, ["tomorrow 9am", "#work"]);
        assert!(parse_quick_add("just words", today(), &lists).spans.is_empty());
    }

    #[test]
    fn smart_recognition_switches() {
        let lists: [String; 0] = [];
        let off = Recognition { dates: false, tags: false, ..Default::default() };
        let parsed = parse_quick_add_with("Call mom tomorrow #family", today(), &lists, off);
        assert_eq!((parsed.title.as_str(), parsed.due_date, parsed.tags.len()), ("Call mom tomorrow #family", None, 0));

        let keep = Recognition { remove_date_text: false, remove_tag_text: false, ..Default::default() };
        let parsed = parse_quick_add_with("Call mom tomorrow #family", today(), &lists, keep);
        assert_eq!(parsed.title, "Call mom tomorrow #family");
        assert_eq!(parsed.due_date, Some(today() + chrono::Duration::days(1)));
        assert_eq!(parsed.tags, ["family"]);
    }

    #[test]
    fn priorities_and_duplicate_tags() {
        let p = parse_quick_add("x !medium #Home #home #work", today(), &[]);
        assert_eq!(p.priority, Some(Priority::Medium));
        assert_eq!(p.tags, ["Home", "work"]);
        assert_eq!(parse_quick_add("x !3", today(), &[]).priority, Some(Priority::Low));
        assert_eq!(parse_quick_add("hooray!", today(), &[]).title, "hooray!");
    }

    #[test]
    fn weekday_repeats_next_week_and_chinese_tokens() {
        // today() is 2026-10-16 (Friday), so next Monday is 2026-10-19.
        let p = parse_quick_add("Standup every weekday 9:00", today(), &[]);
        assert_eq!(p.title, "Standup");
        assert_eq!(p.repeat.as_ref().map(|r| (r.frequency, r.weekdays)), Some((Frequency::Weekly, 0b0001_1111)));

        let p = parse_quick_add("Sync по будням", today(), &[]);
        assert_eq!(p.title, "Sync");
        assert_eq!(p.repeat.as_ref().map(|r| r.weekdays), Some(0b0001_1111));

        let p = parse_quick_add("Sprint review every monday", today(), &[]);
        assert_eq!(p.title, "Sprint review");
        assert_eq!(p.repeat.as_ref().map(|r| (r.frequency, r.weekdays)), Some((Frequency::Weekly, 1 << 0)));
        assert_eq!(p.due_date, Some(NaiveDate::from_ymd_opt(2026, 10, 19).unwrap()));

        let p = parse_quick_add("Plan sprint next week", today(), &[]);
        assert_eq!(p.title, "Plan sprint");
        assert_eq!(p.due_date, Some(today() + Duration::days(7)));

        let p = parse_quick_add("买菜 明天 每天 !高", today(), &[]);
        assert_eq!(p.title, "买菜");
        assert_eq!(p.due_date, Some(today() + Duration::days(1)));
        assert_eq!(p.repeat, Some(Recurrence::new(Frequency::Daily, 1)));
        assert_eq!(p.priority, Some(Priority::High));
    }

    #[test]
    fn recognizes_slash_dates_ordinal_day_first_next_year_and_zh_relative() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        assert_eq!(parse_quick_add("Trip 2026/11/25", today(), &[]).due_date, Some(d(2026, 11, 25)));
        assert_eq!(parse_quick_add("Trip 11/25", today(), &[]).due_date, Some(d(2026, 11, 25)));
        assert_eq!(parse_quick_add("Trip 11/25/2027", today(), &[]).due_date, Some(d(2027, 11, 25)));
        assert_eq!(parse_quick_add("Event 13th nov", today(), &[]).due_date, Some(d(2026, 11, 13)));
        assert_eq!(parse_quick_add("Renew next year", today(), &[]).due_date, Some(d(2027, 10, 18)));
        assert_eq!(parse_quick_add("Renew 明年", today(), &[]).due_date, Some(d(2027, 10, 18)));
        assert_eq!(parse_quick_add("汇报 3天后", today(), &[]).due_date, Some(d(2026, 10, 21)));
        assert_eq!(parse_quick_add("复盘 2周后", today(), &[]).due_date, Some(d(2026, 11, 1)));
        assert_eq!(parse_quick_add("体检 1个月后", today(), &[]).due_date, Some(d(2026, 11, 18)));
    }

    #[test]
    fn recognizes_extended_repeat_phrases() {
        let p = parse_quick_add("Water plants every other day", today(), &[]);
        assert_eq!(p.title, "Water plants");
        assert_eq!(p.repeat, Some(Recurrence::new(Frequency::Daily, 2)));

        let p = parse_quick_add("Payroll biweekly", today(), &[]);
        assert_eq!(p.title, "Payroll");
        assert_eq!(p.repeat, Some(Recurrence::new(Frequency::Weekly, 2)));

        let p = parse_quick_add("Clean house every weekend", today(), &[]);
        assert_eq!(p.title, "Clean house");
        assert_eq!(p.repeat.as_ref().map(|r| (r.frequency, r.weekdays)), Some((Frequency::Weekly, 0b0110_0000)));

        let p = parse_quick_add("Уборка по выходным", today(), &[]);
        assert_eq!(p.title, "Уборка");
        assert_eq!(p.repeat.as_ref().map(|r| r.weekdays), Some(0b0110_0000));

        let p = parse_quick_add("周会 每周一", today(), &[]);
        assert_eq!(p.title, "周会");
        assert_eq!(p.repeat.as_ref().map(|r| (r.frequency, r.weekdays)), Some((Frequency::Weekly, 1 << 0)));
    }

    #[test]
    fn recognizes_tilde_list_and_shorthand_priorities() {
        let lists = vec!["Work".to_owned(), "Personal".to_owned()];
        let p = parse_quick_add("Send report ~work !!!", today(), &lists);
        assert_eq!(p.title, "Send report");
        assert_eq!(p.list.as_deref(), Some("Work"));
        assert_eq!(p.priority, Some(Priority::High));

        let p = parse_quick_add("Review PR !m ~personal", today(), &lists);
        assert_eq!(p.title, "Review PR");
        assert_eq!(p.list.as_deref(), Some("Personal"));
        assert_eq!(p.priority, Some(Priority::Medium));

        let p = parse_quick_add("Read book !l", today(), &lists);
        assert_eq!(p.title, "Read book");
        assert_eq!(p.priority, Some(Priority::Low));

        let p = parse_quick_add("Deploy !! !p1", today(), &lists);
        assert_eq!(p.priority, Some(Priority::High));
    }
}
