//! Вычисления для календаря выбора даты.

use chrono::{Datelike, Duration, Months, NaiveDate};

/// Первое число месяца, в котором лежит `date`.
pub fn month_start(date: NaiveDate) -> NaiveDate {
    date.with_day(1).expect("первое число существует всегда")
}

pub fn add_months(month: NaiveDate, delta: i32) -> NaiveDate {
    let month = month_start(month);
    if delta >= 0 {
        month.checked_add_months(Months::new(delta as u32)).unwrap_or(month)
    } else {
        month.checked_sub_months(Months::new(delta.unsigned_abs())).unwrap_or(month)
    }
}

/// 42 дня (6 недель с понедельника), покрывающие месяц `month`.
pub fn month_grid(month: NaiveDate) -> Vec<NaiveDate> {
    let first = month_start(month);
    let offset = first.weekday().num_days_from_monday() as i64;
    let start = first - Duration::days(offset);
    (0..42).map(|i| start + Duration::days(i)).collect()
}

pub fn month_title(month: NaiveDate) -> String {
    const NAMES: [&str; 12] = [
        "Январь", "Февраль", "Март", "Апрель", "Май", "Июнь", "Июль", "Август", "Сентябрь",
        "Октябрь", "Ноябрь", "Декабрь",
    ];
    format!("{} {}", NAMES[month.month0() as usize], month.year())
}

pub const WEEKDAYS: [&str; 7] = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn grid_starts_on_monday_and_covers_month() {
        // 1 октября 2026 — четверг.
        let grid = month_grid(d(2026, 10, 18));
        assert_eq!(grid.len(), 42);
        assert_eq!(grid[0], d(2026, 9, 28));
        assert_eq!(grid[3], d(2026, 10, 1));
        assert!(grid.contains(&d(2026, 10, 31)));
    }

    #[test]
    fn month_navigation() {
        assert_eq!(add_months(d(2026, 1, 31), 1), d(2026, 2, 1));
        assert_eq!(add_months(d(2026, 1, 15), -1), d(2025, 12, 1));
        assert_eq!(month_title(d(2026, 10, 18)), "Октябрь 2026");
    }
}
