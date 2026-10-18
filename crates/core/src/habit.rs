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
    pub frequency: Frequency,
    /// An emoji or up to two characters shown instead of the name's initial.
    pub icon: Option<String>,
    /// The icon's background, a hex color.
    pub color: Option<String>,
    /// The habit is not due before this day.
    pub start_date: Option<NaiveDate>,
    /// "Goal Days": the number of check-in days to reach; `None` is "Forever".
    pub goal_days: Option<u32>,
    pub section: HabitSection,
    /// Archived habits are kept with their history but hidden and not reminded.
    pub archived: bool,
    /// "Checked-in Style": checked-in days show the habit's icon instead of a checkmark.
    pub icon_checkins: bool,
}

/// A habit's section: the original's default ones or one the user added ("Add Section").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HabitSection {
    Morning,
    Afternoon,
    Night,
    /// A section from the habit sections table.
    Custom(crate::model::Id),
    #[default]
    Others,
}

/// A section the user added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomSection {
    pub id: crate::model::Id,
    pub name: String,
}

impl HabitSection {
    /// The default sections; custom ones go between Night and Others.
    pub const DEFAULTS: [Self; 4] = [Self::Morning, Self::Afternoon, Self::Night, Self::Others];

    /// All sections in display order.
    pub fn ordered(custom: &[CustomSection]) -> Vec<Self> {
        let mut sections = vec![Self::Morning, Self::Afternoon, Self::Night];
        sections.extend(custom.iter().map(|c| Self::Custom(c.id)));
        sections.push(Self::Others);
        sections
    }

    pub fn key(self) -> String {
        match self {
            Self::Morning => "morning".into(),
            Self::Afternoon => "afternoon".into(),
            Self::Night => "night".into(),
            Self::Custom(id) => format!("custom:{id}"),
            Self::Others => "others".into(),
        }
    }

    pub fn from_key(key: &str) -> Self {
        if let Some(id) = key.strip_prefix("custom:").and_then(|id| id.parse().ok()) {
            return Self::Custom(id);
        }
        Self::DEFAULTS.into_iter().find(|s| s.key() == key).unwrap_or_default()
    }
}

/// The original's "Goal Days" presets; "Custom" allows any number.
pub const GOAL_DAYS_PRESETS: [u32; 5] = [7, 21, 30, 100, 365];

impl Habit {
    /// Whether the habit should be done on `date`: on or after its start, per its frequency.
    pub fn is_due(&self, date: NaiveDate, done: &HashSet<NaiveDate>) -> bool {
        self.start_date.is_none_or(|start| date >= start) && self.frequency.is_due(date, done)
    }
}

/// How often a habit is meant to be done, as in the original's Frequency menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frequency {
    /// On the given weekdays (bit 0 = Monday); all seven is "every day".
    Daily { weekdays: u8 },
    /// Any N days a week.
    Weekly { days: u8 },
    /// Every N days.
    Interval { days: u32 },
}

impl Default for Frequency {
    fn default() -> Self {
        Self::Daily { weekdays: Self::EVERY_DAY }
    }
}

impl Frequency {
    pub const EVERY_DAY: u8 = 0b111_1111;

    /// Whether the habit should be done on `date`, given the days it was done.
    pub fn is_due(&self, date: NaiveDate, done: &HashSet<NaiveDate>) -> bool {
        match *self {
            Self::Daily { weekdays } => weekdays & (1 << date.weekday().num_days_from_monday()) != 0,
            Self::Weekly { days } => {
                let monday = date - Duration::days(date.weekday().num_days_from_monday() as i64);
                let this_week = (0..7).map(|i| monday + Duration::days(i)).filter(|d| *d != date && done.contains(d)).count();
                this_week < days as usize
            }
            Self::Interval { days } => {
                (1..days.max(1) as i64).all(|ago| !done.contains(&(date - Duration::days(ago))))
            }
        }
    }

    /// Stored as "daily:127", "weekly:2" or "interval:3".
    pub fn to_storage(&self) -> String {
        match *self {
            Self::Daily { weekdays } => format!("daily:{weekdays}"),
            Self::Weekly { days } => format!("weekly:{days}"),
            Self::Interval { days } => format!("interval:{days}"),
        }
    }

    pub fn from_storage(text: &str) -> Self {
        let (kind, n) = text.split_once(':').unwrap_or((text, ""));
        match (kind, n.parse::<u32>()) {
            ("daily", Ok(mask)) if mask & Self::EVERY_DAY as u32 != 0 => Self::Daily { weekdays: mask as u8 & Self::EVERY_DAY },
            ("weekly", Ok(days)) if (1..=7).contains(&days) => Self::Weekly { days: days as u8 },
            ("interval", Ok(days)) if days >= 1 => Self::Interval { days },
            _ => Self::default(),
        }
    }
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
    for habit in habits.iter().filter(|h| !h.archived) {
        let mut day = after.date();
        while day <= until.date() {
            let days_done = done.get(&habit.id);
            let done_today = days_done.is_some_and(|days| days.contains(&day))
                || !habit.is_due(day, days_done.unwrap_or(&HashSet::new()));
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

/// Snoozed habit reminders coming due in `(after, until]`, unless the habit was archived or done
/// that day meanwhile. `snoozes` are (habit, when to remind again).
pub fn snoozed_between<'a>(
    habits: &'a [Habit],
    done: &HashMap<crate::model::Id, HashSet<NaiveDate>>,
    snoozes: &[(crate::model::Id, NaiveDateTime)],
    after: NaiveDateTime,
    until: NaiveDateTime,
) -> Vec<(&'a Habit, NaiveDateTime)> {
    snoozes
        .iter()
        .filter(|(_, at)| *at > after && *at <= until)
        .filter_map(|(id, at)| {
            let habit = habits.iter().find(|h| h.id == *id && !h.archived)?;
            let done_that_day = done.get(id).is_some_and(|days| days.contains(&at.date()));
            (!done_that_day).then_some((habit, *at))
        })
        .collect()
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
    stats_for(&Frequency::default(), checkins, today)
}

/// Statistics where the streak only counts the days the habit is scheduled on: a Monday,
/// Wednesday, Friday habit keeps its streak over the days in between. Weekly and interval habits
/// count consecutive check-ins.
pub fn stats_for(frequency: &Frequency, checkins: &HashSet<NaiveDate>, today: NaiveDate) -> HabitStats {
    let scheduled = |day: NaiveDate| match frequency {
        Frequency::Daily { weekdays } => weekdays & (1 << day.weekday().num_days_from_monday()) != 0,
        _ => checkins.contains(&day),
    };
    let mut day = today;
    let mut streak = 0;
    // Today does not break the streak until it is over.
    if !checkins.contains(&today) {
        day -= Duration::days(1);
    }
    let oldest = checkins.iter().min().copied().unwrap_or(today);
    while day >= oldest {
        if checkins.contains(&day) {
            streak += 1;
        } else if scheduled(day) {
            break;
        } else if !matches!(frequency, Frequency::Daily { .. }) {
            break;
        }
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
    month_stats_for(&Frequency::default(), checkins, month, today)
}

/// Like [`month_stats`], with the rate counted over the scheduled days of a daily habit.
pub fn month_stats_for(frequency: &Frequency, checkins: &HashSet<NaiveDate>, month: NaiveDate, today: NaiveDate) -> MonthStats {
    let in_month = |date: &NaiveDate| date.year() == month.year() && date.month() == month.month();
    let days = checkins.iter().filter(|date| in_month(date)).count();
    let first = month.with_day(1).unwrap_or(month);
    let next = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
    }
    .unwrap_or(first);
    let last = if today >= next { next - Duration::days(1) } else { today };
    let elapsed = if today < first {
        0
    } else {
        let mut count = 0;
        let mut day = first;
        while day <= last {
            if !matches!(frequency, Frequency::Daily { weekdays } if weekdays & (1 << day.weekday().num_days_from_monday()) == 0) {
                count += 1;
            }
            day += Duration::days(1);
        }
        count
    };
    let rate = if elapsed == 0 { 0 } else { (days as i64 * 100 / elapsed) as u32 };
    MonthStats { days, rate }
}

/// Habits and their check-ins as CSV: one line per check-in, habits without any get one line
/// with empty check-in fields.
pub fn export_csv(habits: &[(Habit, Vec<Checkin>)]) -> String {
    let field = |text: &str| {
        if text.contains([',', '"', '\n']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_owned() }
    };
    let mut out = String::from("habit,archived,goal,unit,date,amount,note\n");
    for (habit, checkins) in habits {
        let goal = habit.goal.as_ref().map(|g| g.amount.to_string()).unwrap_or_default();
        let unit = habit.goal.as_ref().map(|g| field(&g.unit)).unwrap_or_default();
        let prefix = format!("{},{},{goal},{unit}", field(&habit.name), habit.archived);
        if checkins.is_empty() {
            out.push_str(&format!("{prefix},,,\n"));
        }
        for checkin in checkins {
            let note = checkin.note.as_deref().map(field).unwrap_or_default();
            out.push_str(&format!("{prefix},{},{},{note}\n", checkin.date, checkin.value));
        }
    }
    out
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
            frequency: Frequency::default(),
            icon: None,
            color: None,
            start_date: None,
            goal_days: None,
            section: HabitSection::Others,
            archived: false,
            icon_checkins: false,
        };
        let habits = [habit.clone()];
        let mut done = HashMap::new();
        let fired = reminders_between(&habits, &done, at(17, 8), at(18, 10));
        let times: Vec<NaiveDateTime> = fired.iter().map(|(_, at)| *at).collect();
        assert_eq!(times, [at(17, 9), at(17, 21), at(18, 9)]);
        done.insert(habit.id, HashSet::from([d(17)]));
        assert_eq!(reminders_between(&habits, &done, at(17, 8), at(18, 10)).len(), 1);

        // Nothing before the start date.
        let later = [Habit { start_date: Some(d(19)), ..habit.clone() }];
        assert!(reminders_between(&later, &HashMap::new(), at(17, 8), at(18, 23)).is_empty());
        assert!(!later[0].is_due(d(18), &HashSet::new()) && later[0].is_due(d(19), &HashSet::new()));

        // Archived habits are not reminded.
        let archived = [Habit { archived: true, ..habit.clone() }];
        assert!(reminders_between(&archived, &HashMap::new(), at(17, 8), at(18, 23)).is_empty());

        // A snoozed reminder comes back once, unless the day was checked in meanwhile.
        let snoozes = [(habit.id, at(18, 9) + Duration::minutes(15))];
        let back = snoozed_between(&habits, &HashMap::new(), &snoozes, at(18, 9), at(18, 10));
        assert_eq!(back.iter().map(|(_, at)| *at).collect::<Vec<_>>(), [snoozes[0].1]);
        assert!(snoozed_between(&habits, &HashMap::new(), &snoozes, at(18, 10), at(18, 11)).is_empty());
        let done_today = HashMap::from([(habit.id, HashSet::from([d(18)]))]);
        assert!(snoozed_between(&habits, &done_today, &snoozes, at(18, 9), at(18, 10)).is_empty());
        assert!(snoozed_between(&archived, &HashMap::new(), &snoozes, at(18, 9), at(18, 10)).is_empty());
        assert_eq!(HabitSection::from_key("night"), HabitSection::Night);
        assert_eq!(HabitSection::from_key("custom"), HabitSection::Others);
        let custom = CustomSection { id: uuid::Uuid::now_v7(), name: "Pets".into() };
        let section = HabitSection::Custom(custom.id);
        assert_eq!(HabitSection::from_key(&section.key()), section);
        assert_eq!(HabitSection::ordered(&[custom])[3], section);
    }

    #[test]
    fn frequencies() {
        // 2026-10-19 is a Monday.
        let mon_wed_fri = Frequency::Daily { weekdays: 0b10101 };
        let none = HashSet::new();
        assert!(mon_wed_fri.is_due(d(19), &none) && !mon_wed_fri.is_due(d(20), &none));

        let twice_a_week = Frequency::Weekly { days: 2 };
        let done: HashSet<_> = [d(19), d(20)].into();
        assert!(!twice_a_week.is_due(d(21), &done), "done twice this week already");
        assert!(twice_a_week.is_due(d(20), &done), "the day itself does not count against it");
        assert!(twice_a_week.is_due(d(26), &done), "a new week");

        let every_third_day = Frequency::Interval { days: 3 };
        let done: HashSet<_> = [d(19)].into();
        assert!(!every_third_day.is_due(d(20), &done) && !every_third_day.is_due(d(21), &done));
        assert!(every_third_day.is_due(d(22), &done));

        for f in [mon_wed_fri, twice_a_week, every_third_day, Frequency::default()] {
            assert_eq!(Frequency::from_storage(&f.to_storage()), f);
        }
        assert_eq!(Frequency::from_storage("daily:0"), Frequency::default());

        // The streak skips unscheduled days: Mon, Wed, Fri checked, today is Saturday.
        let checked: HashSet<_> = [d(19), d(21), d(23)].into();
        assert_eq!(stats_for(&mon_wed_fri, &checked, d(24)).streak, 3);
        assert_eq!(stats_for(&Frequency::default(), &checked, d(24)).streak, 1);
        // Rate over scheduled days: Mon 19 to Sat 24 has 3 scheduled days, all done.
        let rate = month_stats_for(&mon_wed_fri, &checked, d(1), d(24)).rate;
        let all_days = month_stats_for(&Frequency::default(), &checked, d(1), d(24)).rate;
        assert!(rate > all_days);
    }

    #[test]
    fn csv_export() {
        let habit = |name: &str| Habit {
            id: uuid::Uuid::now_v7(),
            name: name.into(),
            goal: None,
            reminders: Vec::new(),
            frequency: Frequency::default(),
            icon: None,
            color: None,
            start_date: None,
            goal_days: None,
            section: HabitSection::Others,
            archived: false,
            icon_checkins: false,
        };
        let water = Habit { goal: Some(Goal { amount: 8, unit: "glasses".into() }), ..habit("Water, daily") };
        let checkins = vec![Checkin { date: d(18), value: 8, note: Some("said \"done\"".into()) }];
        let csv = export_csv(&[(water, checkins), (habit("Read"), Vec::new())]);
        assert_eq!(
            csv,
            "habit,archived,goal,unit,date,amount,note\n\
             \"Water, daily\",false,8,glasses,2026-10-18,8,\"said \"\"done\"\"\"\n\
             Read,false,,,,,\n"
        );
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
