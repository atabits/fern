//! Habits: things to do every day, checked in per date, with totals and streaks.

use std::collections::HashSet;

use chrono::{Datelike, Duration, NaiveDate};

use crate::model::Id;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Habit {
    pub id: Id,
    pub name: String,
}

/// Check-in statistics shown on a habit card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HabitStats {
    /// All days the habit was checked in.
    pub total_days: usize,
    /// Consecutive checked days ending today, or yesterday when today is not checked yet.
    pub streak: usize,
}

pub fn stats(checkins: &HashSet<NaiveDate>, today: NaiveDate) -> HabitStats {
    let mut day = if checkins.contains(&today) { today } else { today - Duration::days(1) };
    let mut streak = 0;
    while checkins.contains(&day) {
        streak += 1;
        day -= Duration::days(1);
    }
    HabitStats { total_days: checkins.len(), streak }
}

/// Check-in statistics for one month, shown in the habit details panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MonthStats {
    /// Days of the month the habit was checked in.
    pub days: usize,
    /// Checked days as a share of the month's days so far (all of them for a past month), in
    /// whole percent.
    pub rate: u32,
}

/// Statistics for the month containing `month`.
pub fn month_stats(checkins: &HashSet<NaiveDate>, month: NaiveDate, today: NaiveDate) -> MonthStats {
    let in_month = |date: &NaiveDate| date.year() == month.year() && date.month() == month.month();
    let days = checkins.iter().filter(|date| in_month(date)).count();
    let first = month.with_day(1).unwrap_or(month);
    let next = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
    }
    .unwrap_or(first);
    let elapsed = if today < first {
        0
    } else if today >= next {
        (next - first).num_days()
    } else {
        (today - first).num_days() + 1
    };
    let rate = if elapsed == 0 { 0 } else { (days as i64 * 100 / elapsed) as u32 };
    MonthStats { days, rate }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
    }

    #[test]
    fn streak_counts_back_from_today_or_yesterday() {
        let today = d(18);
        let checked: HashSet<_> = [d(15), d(16), d(17)].into();
        assert_eq!(stats(&checked, today), HabitStats { total_days: 3, streak: 3 });
        let checked: HashSet<_> = [d(10), d(16), d(18)].into();
        assert_eq!(stats(&checked, today), HabitStats { total_days: 3, streak: 1 });
        let checked: HashSet<_> = [d(10), d(15)].into();
        assert_eq!(stats(&checked, today).streak, 0);
    }

    #[test]
    fn month_stats_count_the_days_so_far() {
        let today = d(10);
        let checked: HashSet<_> = [d(1), d(2), d(9), NaiveDate::from_ymd_opt(2026, 9, 30).unwrap()].into();
        assert_eq!(month_stats(&checked, d(1), today), MonthStats { days: 3, rate: 30 });
        let september = NaiveDate::from_ymd_opt(2026, 9, 5).unwrap();
        assert_eq!(month_stats(&checked, september, today), MonthStats { days: 1, rate: 3 });
        let november = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        assert_eq!(month_stats(&checked, november, today), MonthStats::default());
    }
}
