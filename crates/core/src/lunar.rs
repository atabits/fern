//! Chinese lunar calendar conversion and recurrence helpers (1900–2100).

use chrono::{Duration, NaiveDate};

/// Bit-packed lunar year descriptors for 1900..=2100 (same table TickTick uses):
/// - bits 0..=3: leap month (1..=12, or 0 if none)
/// - bit 16: leap month has 30 days (1) or 29 days (0)
/// - bits 15..=4: months 1..=12 have 30 days (1) or 29 days (0)
const LUNAR_INFO: [u32; 201] = [
    0x04bd8, 0x04ae0, 0x0a570, 0x054d5, 0x0d260, 0x0d950, 0x16554, 0x056a0, 0x09ad0, 0x055d2,
    0x04ae0, 0x0a5b6, 0x0a4d0, 0x0d250, 0x1d255, 0x0b540, 0x0d6a0, 0x0ada2, 0x095b0, 0x14977,
    0x04970, 0x0a4b0, 0x0b4b5, 0x06a50, 0x06d40, 0x1ab54, 0x02b60, 0x09570, 0x052f2, 0x04970,
    0x06566, 0x0d4a0, 0x0ea50, 0x16a95, 0x05ad0, 0x02b60, 0x186e3, 0x092e0, 0x1c8d7, 0x0c950,
    0x0d4a0, 0x1d8a6, 0x0b550, 0x056a0, 0x1a5b4, 0x025d0, 0x092d0, 0x0d2b2, 0x0a950, 0x0b557,
    0x06ca0, 0x0b550, 0x15355, 0x04da0, 0x0a5b0, 0x14573, 0x052b0, 0x0a9a8, 0x0e950, 0x06aa0,
    0x0aea6, 0x0ab50, 0x04b60, 0x0aae4, 0x0a570, 0x05260, 0x0f263, 0x0d950, 0x05b57, 0x056a0,
    0x096d0, 0x04dd5, 0x04ad0, 0x0a4d0, 0x0d4d4, 0x0d250, 0x0d558, 0x0b540, 0x0b6a0, 0x195a6,
    0x095b0, 0x049b0, 0x0a974, 0x0a4b0, 0x0b27a, 0x06a50, 0x06d40, 0x0af46, 0x0ab60, 0x09570,
    0x04af5, 0x04970, 0x064b0, 0x074a3, 0x0ea50, 0x06b58, 0x05ac0, 0x0ab60, 0x096d5, 0x092e0,
    0x0c960, 0x0d954, 0x0d4a0, 0x0da50, 0x07552, 0x056a0, 0x0abb7, 0x025d0, 0x092d0, 0x0cab5,
    0x0a950, 0x0b4a0, 0x0baa4, 0x0ad50, 0x055d9, 0x04ba0, 0x0a5b0, 0x15176, 0x052b0, 0x0a930,
    0x07954, 0x06aa0, 0x0ad50, 0x05b52, 0x04b60, 0x0a6e6, 0x0a4e0, 0x0d260, 0x0ea65, 0x0d530,
    0x05aa0, 0x076a3, 0x096d0, 0x04afb, 0x04ad0, 0x0a4d0, 0x1d0b6, 0x0d250, 0x0d520, 0x0dd45,
    0x0b5a0, 0x056d0, 0x055b2, 0x049b0, 0x0a577, 0x0a4b0, 0x0aa50, 0x1b255, 0x06d20, 0x0ada0,
    0x14b63, 0x09370, 0x049f8, 0x04970, 0x064b0, 0x168a6, 0x0ea50, 0x06aa0, 0x1a6c4, 0x0aae0,
    0x092e0, 0x0d2e3, 0x0c960, 0x0d557, 0x0d4a0, 0x0da50, 0x05d55, 0x056a0, 0x0a6d0, 0x055d4,
    0x052d0, 0x0a9b8, 0x0a950, 0x0b4a0, 0x0b6a6, 0x0ad50, 0x055a0, 0x0aba4, 0x0a5b0, 0x052b0,
    0x0b273, 0x06930, 0x07337, 0x06aa0, 0x0ad50, 0x14b55, 0x04b60, 0x0a570, 0x054e4, 0x0d160,
    0x0e968, 0x0d520, 0x0daa0, 0x16aa6, 0x056d0, 0x04ae0, 0x0a9d4, 0x0a2d0, 0x0d150, 0x0f252,
    0x0d520,
];

const MIN_YEAR: i32 = 1900;
const MAX_YEAR: i32 = 2100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LunarDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub is_leap: bool,
}

fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(1900, 1, 31).expect("1900-01-31 is valid")
}

fn year_info(year: i32) -> Option<u32> {
    if !(MIN_YEAR..=MAX_YEAR).contains(&year) {
        return None;
    }
    Some(LUNAR_INFO[(year - MIN_YEAR) as usize])
}

pub fn leap_month(year: i32) -> u8 {
    year_info(year).map_or(0, |info| (info & 0xf) as u8)
}

pub fn leap_month_days(year: i32) -> u32 {
    let Some(info) = year_info(year) else { return 0 };
    if leap_month(year) == 0 {
        0
    } else if (info & 0x10000) != 0 {
        30
    } else {
        29
    }
}

pub fn month_days(year: i32, month: u8) -> u32 {
    let Some(info) = year_info(year) else { return 0 };
    if !(1..=12).contains(&month) {
        return 0;
    }
    if (info & (0x10000 >> month)) != 0 { 30 } else { 29 }
}

pub fn year_days(year: i32) -> u32 {
    if year_info(year).is_none() {
        return 0;
    }
    let regular: u32 = (1..=12).map(|m| month_days(year, m)).sum();
    regular + leap_month_days(year)
}

pub fn gregorian_to_lunar(date: NaiveDate) -> Option<LunarDate> {
    let mut offset = (date - epoch()).num_days();
    if offset < 0 {
        return None;
    }

    let mut year = MIN_YEAR;
    while year <= MAX_YEAR {
        let days = i64::from(year_days(year));
        if offset < days {
            break;
        }
        offset -= days;
        year += 1;
    }
    if year > MAX_YEAR {
        return None;
    }

    let leap = leap_month(year);
    let mut is_leap = false;
    let mut month = 1u8;
    while month <= 12 {
        let days = i64::from(if is_leap { leap_month_days(year) } else { month_days(year, month) });
        if offset < days {
            return Some(LunarDate {
                year,
                month,
                day: (offset + 1) as u8,
                is_leap,
            });
        }
        offset -= days;
        if !is_leap && leap == month {
            is_leap = true;
        } else {
            is_leap = false;
            month += 1;
        }
    }
    None
}

pub fn lunar_to_gregorian(lunar: LunarDate) -> Option<NaiveDate> {
    if year_info(lunar.year).is_none() || !(1..=12).contains(&lunar.month) {
        return None;
    }
    let mut days = 0i64;
    for y in MIN_YEAR..lunar.year {
        days += i64::from(year_days(y));
    }

    let leap = leap_month(lunar.year);
    for m in 1..lunar.month {
        days += i64::from(month_days(lunar.year, m));
        if m == leap {
            days += i64::from(leap_month_days(lunar.year));
        }
    }

    let target_is_leap = lunar.is_leap && leap == lunar.month;
    if target_is_leap {
        days += i64::from(month_days(lunar.year, lunar.month));
    }

    let max_day = if target_is_leap {
        leap_month_days(lunar.year)
    } else {
        month_days(lunar.year, lunar.month)
    };
    let clamped_day = u32::from(lunar.day).clamp(1, max_day);
    days += i64::from(clamped_day - 1);

    epoch().checked_add_signed(Duration::days(days))
}

/// Advances `interval` lunar months chronologically (stepping through any leap month) and maps
/// the original lunar day clamped to the target month's length.
pub fn next_lunar_monthly(from: NaiveDate, interval: u32) -> Option<NaiveDate> {
    let mut cur = gregorian_to_lunar(from)?;
    let target_day = cur.day;
    let steps = interval.max(1);
    for _ in 0..steps {
        let leap = leap_month(cur.year);
        if !cur.is_leap && leap == cur.month {
            cur.is_leap = true;
        } else {
            cur.is_leap = false;
            if cur.month == 12 {
                cur.year += 1;
                cur.month = 1;
            } else {
                cur.month += 1;
            }
        }
        year_info(cur.year)?;
    }
    cur.day = target_day;
    lunar_to_gregorian(cur)
}

/// Advances `interval` lunar years keeping the same `(month, day)` (non-leap if the target year
/// has no matching leap month).
pub fn next_lunar_yearly(from: NaiveDate, interval: u32) -> Option<NaiveDate> {
    let cur = gregorian_to_lunar(from)?;
    let target_year = cur.year.checked_add(i32::try_from(interval.max(1)).ok()?)?;
    year_info(target_year)?;
    let is_leap = cur.is_leap && leap_month(target_year) == cur.month;
    lunar_to_gregorian(LunarDate {
        year: target_year,
        month: cur.month,
        day: cur.day,
        is_leap,
    })
}

pub fn format_short(lunar: LunarDate, zh: bool) -> String {
    if !zh {
        return if lunar.is_leap {
            format!("Lunar L{}/{}", lunar.month, lunar.day)
        } else {
            format!("Lunar {}/{}", lunar.month, lunar.day)
        };
    }
    const MONTHS: [&str; 12] = ["正", "二", "三", "四", "五", "六", "七", "八", "九", "十", "冬", "腊"];
    const DIGITS: [&str; 10] = ["日", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    let month_str = MONTHS.get(lunar.month.saturating_sub(1) as usize).copied().unwrap_or("");
    let prefix = if lunar.is_leap { "闰" } else { "" };
    let day_str = match lunar.day {
        1..=9 => format!("初{}", DIGITS[lunar.day as usize]),
        10 => "初十".to_owned(),
        11..=19 => format!("十{}", DIGITS[(lunar.day - 10) as usize]),
        20 => "二十".to_owned(),
        21..=29 => format!("廿{}", DIGITS[(lunar.day - 20) as usize]),
        30 => "三十".to_owned(),
        d => d.to_string(),
    };
    format!("{prefix}{month_str}月{day_str}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn known_lunar_dates_and_roundtrips() {
        assert_eq!(
            gregorian_to_lunar(d(2024, 2, 10)),
            Some(LunarDate { year: 2024, month: 1, day: 1, is_leap: false })
        );
        assert_eq!(
            gregorian_to_lunar(d(2025, 1, 29)),
            Some(LunarDate { year: 2025, month: 1, day: 1, is_leap: false })
        );
        assert_eq!(
            gregorian_to_lunar(d(2026, 2, 17)),
            Some(LunarDate { year: 2026, month: 1, day: 1, is_leap: false })
        );
        assert_eq!(
            gregorian_to_lunar(d(2027, 2, 6)),
            Some(LunarDate { year: 2027, month: 1, day: 1, is_leap: false })
        );
        assert_eq!(
            gregorian_to_lunar(d(2026, 9, 25)),
            Some(LunarDate { year: 2026, month: 8, day: 15, is_leap: false })
        );

        for date in [d(2024, 2, 10), d(2025, 7, 25), d(2026, 2, 17), d(2026, 9, 25), d(2028, 12, 31)] {
            let lunar = gregorian_to_lunar(date).unwrap();
            assert_eq!(lunar_to_gregorian(lunar), Some(date));
        }
    }

    #[test]
    fn monthly_and_yearly_recurrence() {
        assert_eq!(next_lunar_yearly(d(2026, 2, 17), 1), Some(d(2027, 2, 6)));
        assert_eq!(next_lunar_yearly(d(2024, 2, 10), 2), Some(d(2026, 2, 17)));
        // 2025 has a leap 6th month (leap_month(2025) == 6):
        let m6 = lunar_to_gregorian(LunarDate { year: 2025, month: 6, day: 15, is_leap: false }).unwrap();
        let next = next_lunar_monthly(m6, 1).unwrap();
        assert_eq!(
            gregorian_to_lunar(next),
            Some(LunarDate { year: 2025, month: 6, day: 15, is_leap: true })
        );
        assert_eq!(format_short(LunarDate { year: 2026, month: 1, day: 1, is_leap: false }, true), "正月初一");
        assert_eq!(format_short(LunarDate { year: 2025, month: 6, day: 15, is_leap: true }, false), "Lunar L6/15");
    }
}
