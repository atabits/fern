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
    pub due_time: Option<NaiveTime>,
    pub priority: Option<Priority>,
    pub tags: Vec<String>,
    pub repeat: Option<Recurrence>,
    /// The list name as the user spelled it when creating the list.
    pub list: Option<String>,
}

/// `lists` holds the names of existing lists; `^name` is recognized only for those.
pub fn parse_quick_add(input: &str, today: NaiveDate, lists: &[String]) -> ParsedTask {
    let words: Vec<&str> = input.split_whitespace().collect();
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut used = vec![false; words.len()];
    let mut parsed = ParsedTask::default();

    let mut i = 0;
    while i < words.len() {
        let word = lower[i].as_str();

        if let Some(tag) = words[i].strip_prefix('#').filter(|t| !t.is_empty()) {
            if !parsed.tags.iter().any(|t: &String| t.to_lowercase() == tag.to_lowercase()) {
                parsed.tags.push(tag.to_owned());
            }
            used[i] = true;
        } else if let Some(list) = word
            .strip_prefix('^')
            .and_then(|name| lists.iter().find(|l| l.to_lowercase() == name))
        {
            parsed.list = Some(list.clone());
            used[i] = true;
        } else if let Some(priority) = word.strip_prefix('!').and_then(parse_priority) {
            parsed.priority = Some(priority);
            used[i] = true;
        } else if parsed.repeat.is_none()
            && let Some((repeat, len)) = parse_repeat(&lower[i..])
        {
            parsed.repeat = Some(repeat);
            used[i..i + len].iter_mut().for_each(|u| *u = true);
            i += len;
            continue;
        } else if parsed.due_time.is_none()
            && let Some(time) = parse_time(word)
        {
            parsed.due_time = Some(time);
            mark_with_preposition(&mut used, &lower, i, 1);
        } else if parsed.due_date.is_none()
            && let Some((date, len)) = parse_date(&lower[i..], today)
        {
            parsed.due_date = Some(date);
            mark_with_preposition(&mut used, &lower, i, len);
            i += len;
            continue;
        }
        i += 1;
    }

    // A time without a date means today, as in TickTick.
    if parsed.due_time.is_some() && parsed.due_date.is_none() {
        parsed.due_date = Some(today);
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

/// A repeat rule at the start of `words` and how many words it took.
fn parse_repeat(words: &[String]) -> Option<(Recurrence, usize)> {
    let single = |word: &str| {
        Some(match word {
            "daily" | "ежедневно" => Frequency::Daily,
            "weekly" | "еженедельно" => Frequency::Weekly,
            "monthly" | "ежемесячно" => Frequency::Monthly,
            "yearly" | "annually" | "ежегодно" => Frequency::Yearly,
            _ => return None,
        })
    };
    if let Some(frequency) = single(&words[0]) {
        return Some((Recurrence::new(frequency, 1), 1));
    }
    if !matches!(words[0].as_str(), "every" | "каждый" | "каждую" | "каждое" | "каждые") || words.len() < 2 {
        return None;
    }
    let (interval, unit, len) = match words[1].parse::<u32>() {
        Ok(n) if words.len() >= 3 => (n, words[2].as_str(), 3),
        Ok(_) => return None,
        Err(_) => (1, words[1].as_str(), 2),
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
    Some((Recurrence::new(frequency, interval), len))
}

fn parse_priority(value: &str) -> Option<Priority> {
    match value {
        "1" | "high" | "высокий" | "в" => Some(Priority::High),
        "2" | "medium" | "средний" | "с" => Some(Priority::Medium),
        "3" | "low" | "низкий" | "н" => Some(Priority::Low),
        _ => None,
    }
}

fn parse_time(word: &str) -> Option<NaiveTime> {
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
        "сегодня" | "today" => return Some((today, 1)),
        "завтра" | "tomorrow" => return Some((today + Duration::days(1), 1)),
        "послезавтра" => return Some((today + Duration::days(2), 1)),
        _ => {}
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
    if let Some(date) = parse_numeric_date(first, today) {
        return Some((date, 1));
    }
    if words.len() >= 2
        && let (Ok(day), Some(month)) = (first.parse::<u32>(), parse_month(&words[1]))
    {
        let date = NaiveDate::from_ymd_opt(today.year(), month, day)?;
        // "13 March" said in November means March of next year.
        let date = if date < today { NaiveDate::from_ymd_opt(today.year() + 1, month, day)? } else { date };
        return Some((date, 2));
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
    let parts: Vec<&str> = word.split('.').collect();
    let num = |s: &str| s.parse::<u32>().ok();
    match parts.as_slice() {
        [d, m] if d.len() <= 2 && m.len() <= 2 => {
            let date = NaiveDate::from_ymd_opt(today.year(), num(m)?, num(d)?)?;
            if date < today { NaiveDate::from_ymd_opt(today.year() + 1, num(m)?, num(d)?) } else { Some(date) }
        }
        [d, m, y] if d.len() <= 2 && m.len() <= 2 && y.len() == 4 => {
            NaiveDate::from_ymd_opt(y.parse().ok()?, num(m)?, num(d)?)
        }
        _ => None,
    }
}

fn parse_weekday(word: &str) -> Option<Weekday> {
    Some(match word {
        "пн" | "понедельник" | "mon" | "monday" => Weekday::Mon,
        "вт" | "вторник" | "tue" | "tuesday" => Weekday::Tue,
        "ср" | "среда" | "среду" | "wed" | "wednesday" => Weekday::Wed,
        "чт" | "четверг" | "thu" | "thursday" => Weekday::Thu,
        "пт" | "пятница" | "пятницу" | "fri" | "friday" => Weekday::Fri,
        "сб" | "суббота" | "субботу" | "sat" | "saturday" => Weekday::Sat,
        "вс" | "воскресенье" | "sun" | "sunday" => Weekday::Sun,
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
        assert_eq!(date("x by 5 march"), Some(d(2027, 3, 5)));
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
    fn priorities_and_duplicate_tags() {
        let p = parse_quick_add("x !medium #Home #home #work", today(), &[]);
        assert_eq!(p.priority, Some(Priority::Medium));
        assert_eq!(p.tags, ["Home", "work"]);
        assert_eq!(parse_quick_add("x !3", today(), &[]).priority, Some(Priority::Low));
        assert_eq!(parse_quick_add("hooray!", today(), &[]).title, "hooray!");
    }
}
