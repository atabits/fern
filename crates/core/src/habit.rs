//! Habits: things to do every day, checked in per date, with totals and streaks.

use std::collections::HashSet;

use chrono::{Duration, NaiveDate};

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
}
