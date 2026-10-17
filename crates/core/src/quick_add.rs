//! Разбор строки быстрого ввода: «Купить молоко завтра в 18:00 #дом !1».
//!
//! Распознаётся (русский и английский):
//! - теги `#дом`;
//! - приоритет `!1`/`!2`/`!3` (высокий/средний/низкий), `!high`, `!высокий` и т.п.;
//! - даты: «сегодня», «завтра», «послезавтра», дни недели, «через 3 дня», «через неделю»,
//!   `13.11`, `13.11.2026`, `2026-11-13`, «13 ноября»;
//! - время: `18:00`, `18ч`.
//!
//! Распознанные слова (и предлог «в»/«на» перед ними) убираются из названия.

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveTime, Weekday};

use crate::model::Priority;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedTask {
    pub title: String,
    pub due_date: Option<NaiveDate>,
    pub due_time: Option<NaiveTime>,
    pub priority: Option<Priority>,
    pub tags: Vec<String>,
}

pub fn parse_quick_add(input: &str, today: NaiveDate) -> ParsedTask {
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
        } else if let Some(priority) = word.strip_prefix('!').and_then(parse_priority) {
            parsed.priority = Some(priority);
            used[i] = true;
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

    // Время без даты — на сегодня, как в TickTick.
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

/// Отмечает `len` слов с позиции `start` и предлог прямо перед ними.
fn mark_with_preposition(used: &mut [bool], lower: &[String], start: usize, len: usize) {
    used[start..start + len].iter_mut().for_each(|u| *u = true);
    if start > 0
        && !used[start - 1]
        && matches!(lower[start - 1].as_str(), "в" | "во" | "на" | "к" | "до" | "at" | "on" | "by")
    {
        used[start - 1] = true;
    }
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
    if let Some((h, m)) = word.split_once(':') {
        if h.is_empty() || h.len() > 2 || m.len() != 2 {
            return None;
        }
        return NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, 0);
    }
    let hours = word.strip_suffix("ч")?;
    if hours.is_empty() || hours.len() > 2 {
        return None;
    }
    NaiveTime::from_hms_opt(hours.parse().ok()?, 0, 0)
}

/// Дата в начале `words` и сколько слов она заняла.
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
        // Тот же день недели означает следующую неделю.
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
        // «13 марта» в ноябре — это март следующего года.
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

fn parse_month(word: &str) -> Option<u32> {
    const PREFIXES: [&[&str]; 12] = [
        &["янв", "jan"],
        &["фев", "feb"],
        &["мар", "mar"],
        &["апр", "apr"],
        &["мая", "май", "may"],
        &["июн", "jun"],
        &["июл", "jul"],
        &["авг", "aug"],
        &["сен", "sep"],
        &["окт", "oct"],
        &["ноя", "nov"],
        &["дек", "dec"],
    ];
    PREFIXES
        .iter()
        .position(|prefixes| prefixes.iter().any(|p| word.starts_with(p)))
        .map(|i| i as u32 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 18 октября 2026 — воскресенье.
    fn today() -> NaiveDate {
        d(2026, 10, 18)
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn t(h: u32, m: u32) -> Option<NaiveTime> {
        NaiveTime::from_hms_opt(h, m, 0)
    }

    #[test]
    fn plain_title_is_untouched() {
        let p = parse_quick_add("  Купить   молоко ", today());
        assert_eq!(p, ParsedTask { title: "Купить молоко".into(), ..Default::default() });
    }

    #[test]
    fn full_example() {
        let p = parse_quick_add("Купить молоко завтра в 18:00 #дом !1", today());
        assert_eq!(p.title, "Купить молоко");
        assert_eq!(p.due_date, Some(d(2026, 10, 19)));
        assert_eq!(p.due_time, t(18, 0));
        assert_eq!(p.tags, ["дом"]);
        assert_eq!(p.priority, Some(Priority::High));
    }

    #[test]
    fn relative_dates() {
        let date = |s: &str| parse_quick_add(s, today()).due_date;
        assert_eq!(date("x сегодня"), Some(today()));
        assert_eq!(date("x послезавтра"), Some(d(2026, 10, 20)));
        assert_eq!(date("x через 3 дня"), Some(d(2026, 10, 21)));
        assert_eq!(date("x через неделю"), Some(d(2026, 10, 25)));
        assert_eq!(date("x через 2 месяца"), Some(d(2026, 12, 18)));
        assert_eq!(date("x in 2 weeks"), Some(d(2026, 11, 1)));
    }

    #[test]
    fn weekdays_point_forward() {
        let date = |s: &str| parse_quick_add(s, today()).due_date;
        assert_eq!(date("отчёт в пятницу"), Some(d(2026, 10, 23)));
        assert_eq!(date("отчёт пн"), Some(d(2026, 10, 19)));
        // Сегодня воскресенье — «в воскресенье» значит через неделю.
        assert_eq!(date("отчёт в воскресенье"), Some(d(2026, 10, 25)));
        assert_eq!(parse_quick_add("отчёт в пятницу", today()).title, "отчёт");
    }

    #[test]
    fn absolute_dates() {
        let date = |s: &str| parse_quick_add(s, today()).due_date;
        assert_eq!(date("x 13.11"), Some(d(2026, 11, 13)));
        assert_eq!(date("x 01.03"), Some(d(2027, 3, 1)));
        assert_eq!(date("x 13.11.2027"), Some(d(2027, 11, 13)));
        assert_eq!(date("x 2026-12-31"), Some(d(2026, 12, 31)));
        assert_eq!(date("x 13 ноября"), Some(d(2026, 11, 13)));
        assert_eq!(date("x до 5 марта"), Some(d(2027, 3, 5)));
        assert_eq!(parse_quick_add("Оплата до 5 марта", today()).title, "Оплата");
    }

    #[test]
    fn time_without_date_is_today() {
        let p = parse_quick_add("созвон 9ч", today());
        assert_eq!((p.title.as_str(), p.due_date, p.due_time), ("созвон", Some(today()), t(9, 0)));
    }

    #[test]
    fn numbers_and_words_that_are_not_dates_stay() {
        let p = parse_quick_add("Купить 3 яблока в магазине", today());
        assert_eq!(p.title, "Купить 3 яблока в магазине");
        assert_eq!(p.due_date, None);
        let p = parse_quick_add("Прочитать 25:99 и #", today());
        assert_eq!(p.title, "Прочитать 25:99 и #");
    }

    #[test]
    fn priorities_and_duplicate_tags() {
        let p = parse_quick_add("x !средний #Дом #дом #работа", today());
        assert_eq!(p.priority, Some(Priority::Medium));
        assert_eq!(p.tags, ["Дом", "работа"]);
        assert_eq!(parse_quick_add("x !3", today()).priority, Some(Priority::Low));
        assert_eq!(parse_quick_add("ура!", today()).title, "ура!");
    }
}
