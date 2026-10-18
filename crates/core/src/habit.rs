//! Habits: things to do every day, checked in per date, with totals and streaks.

use std::collections::HashSet;

use std::collections::HashMap;

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};

use crate::model::Id;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Habit {
    pub id: Id,
    pub name: String,
    /// "Reach a certain amount" per day; `None` is the original's "Achieve it all".
    pub goal: Option<Goal>,
    /// Times of day to be reminded, earliest first.
    pub reminders: Vec<NaiveTime>,
}

/// Habit reminders firing in `(after, until]`: every reminder time of every day, unless the habit
/// is already done that day. `done` maps habits to their completed days.
pub fn reminders_between<'a>(
    habits: &'a [Habit],
    done: &HashMap<crate::model::Id, HashSet<NaiveDate>>,
    after: NaiveDateTime,
    until: NaiveDateTime,
) -> Vec<(&'a Habit, NaiveDateTime)> {
    let mut fired = Vec::new();
    for habit in habits {
        let mut day = after.date();
        while day <= until.date() {
            let done_today = done.get(&habit.id).is_some_and(|days| days.contains(&day));
            for time in &habit.reminders {
                let at = day.and_time(*time);
                if at > after && at <= until && !done_today {
                    fired.push((habit, at));
                }
            }
            day += Duration::days(1);
        }
    }
    fired.sort_by_key(|(_, at)| *at);
    fired
}

/// A daily amount to reach, e.g. 8 glasses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Goal {
    pub amount: u32,
    pub unit: String,
}

/// One day's check-in: the amount done and an optional note ("check-in thoughts").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkin {
    pub date: NaiveDate,
    pub value: u32,
    pub note: Option<String>,
}

/// The amount done in the month containing `month`, and in total, for habits with a goal.
pub fn completion(checkins: &[Checkin], month: NaiveDate) -> (u32, u32) {
    let in_month = |date: &NaiveDate| date.year() == month.year() && date.month() == month.month();
    let monthly = checkins.iter().filter(|c| in_month(&c.date)).map(|c| c.value).sum();
    let total = checkins.iter().map(|c| c.value).sum();
    (monthly, total)
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
    fn habit_reminders_skip_done_days() {
        let at = |day, h| d(day).and_hms_opt(h, 0, 0).unwrap();
        let habit = Habit {
            id: uuid::Uuid::now_v7(),
            name: "Read".into(),
            goal: None,
            reminders: vec![NaiveTime::from_hms_opt(9, 0, 0).unwrap(), NaiveTime::from_hms_opt(21, 0, 0).unwrap()],
        };
        let habits = [habit.clone()];
        let mut done = HashMap::new();
        let fired = reminders_between(&habits, &done, at(17, 8), at(18, 10));
        let times: Vec<NaiveDateTime> = fired.iter().map(|(_, at)| *at).collect();
        assert_eq!(times, [at(17, 9), at(17, 21), at(18, 9)]);
        done.insert(habit.id, HashSet::from([d(17)]));
        assert_eq!(reminders_between(&habits, &done, at(17, 8), at(18, 10)).len(), 1);
    }

    #[test]
    fn completion_sums_amounts() {
        let c = |date, value| Checkin { date, value, note: None };
        let checkins = [c(d(1), 2), c(d(9), 3), c(NaiveDate::from_ymd_opt(2026, 9, 30).unwrap(), 2)];
        assert_eq!(completion(&checkins, d(18)), (5, 7));
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
