//! Habits: recurring routines checked in per date, with goals, logs, moods, streaks, and cycles.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};

use crate::model::Id;

/// Maximum numeric daily goal amount.
pub const MAX_GOAL_AMOUNT: f64 = 99_999.0;
/// Maximum reminders allowed per habit.
pub const MAX_REMINDERS: usize = 10;
/// Maximum user-created custom sections.
pub const MAX_CUSTOM_SECTIONS: usize = 18;
/// Maximum characters allowed in a habit log note.
pub const MAX_LOG_NOTE_CHARS: usize = 1024;
/// Maximum number of days in the past that can be edited in the calendar.
pub const MAX_EDITABLE_PAST_DAYS: i64 = 90;
/// Supported mood values for a habit log (0 = none, 10..=50 = worst..best).
pub const MOODS: [u8; 5] = [10, 20, 30, 40, 50];

/// Rounds a numeric habit amount to 2 decimal places, clamping invalid or negative values to 0.
pub fn polish_amount(value: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        return 0.0;
    }
    ((value * 100.0).round() / 100.0).min(MAX_GOAL_AMOUNT)
}

/// Formats a habit amount cleanly without unnecessary trailing zeros (`8`, `1.5`, `1.25`).
pub fn format_amount(value: f64) -> String {
    let v = if !value.is_finite() || value <= 0.0 {
        0.0
    } else {
        (value * 100.0).round() / 100.0
    };
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// Parses a positive numeric habit amount (accepting `.` or `,` as decimal separator).
pub fn parse_amount(text: &str) -> Option<f64> {
    let normalized = text.trim().replace(',', ".");
    let value: f64 = normalized.parse().ok()?;
    if !value.is_finite() || value <= 0.0 || value > MAX_GOAL_AMOUNT {
        return None;
    }
    let polished = polish_amount(value);
    (polished > 0.0).then_some(polished)
}

/// Parses a non-negative numeric habit amount (`0..=MAX_GOAL_AMOUNT`).
pub fn parse_amount_non_negative(text: &str) -> Option<f64> {
    let normalized = text.trim().replace(',', ".");
    let value: f64 = normalized.parse().ok()?;
    if !value.is_finite() || value < 0.0 || value > MAX_GOAL_AMOUNT {
        return None;
    }
    Some(polish_amount(value))
}

/// How clicking a quantifiable habit's check-in button behaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CheckinMode {
    /// Each click adds `step` toward the daily goal.
    Auto { step: f64 },
    /// Clicking prompts the user to enter an amount manually.
    Manual,
    /// Clicking completes the entire daily goal (or resets it to 0).
    All,
}

impl Default for CheckinMode {
    fn default() -> Self {
        Self::Auto { step: 1.0 }
    }
}

impl CheckinMode {
    /// Encodes the mode as a single floating-point `goal_step` column:
    /// `-1.0` = Manual, `0.0` = All, `> 0.0` = Auto step.
    pub fn to_storage(self) -> f64 {
        match self {
            Self::Auto { step } => polish_amount(step).max(0.01),
            Self::Manual => -1.0,
            Self::All => 0.0,
        }
    }

    pub fn from_storage(raw: f64) -> Self {
        if !raw.is_finite() {
            return Self::default();
        }
        if raw < 0.0 {
            Self::Manual
        } else if raw == 0.0 {
            Self::All
        } else {
            Self::Auto { step: polish_amount(raw).max(0.01) }
        }
    }
}

/// A daily numeric target to reach, e.g. 8 glasses or 30 minutes.
#[derive(Clone, Debug, PartialEq)]
pub struct Goal {
    pub amount: f64,
    pub unit: String,
    pub mode: CheckinMode,
}

impl Goal {
    pub fn new(amount: f64, unit: impl Into<String>) -> Self {
        Self { amount: polish_amount(amount), unit: unit.into(), mode: CheckinMode::default() }
    }

    pub fn step_amount(&self) -> f64 {
        match self.mode {
            CheckinMode::Auto { step } => polish_amount(step).max(0.01),
            CheckinMode::Manual => 1.0,
            CheckinMode::All => self.amount.max(1.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Habit {
    pub id: Id,
    pub name: String,
    /// "Reach a certain amount" per day; `None` is "Achieve it all".
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
    /// "Goal Days": the number of check-in days per cycle; `None` is "Forever".
    pub goal_days: Option<u32>,
    pub section: HabitSection,
    /// Archived habits are kept with their history in read-only mode and not reminded.
    pub archived: bool,
    /// Local date on which the habit was archived, if archived.
    pub archived_date: Option<NaiveDate>,
    /// "Checked-in Style": checked-in days show the habit's icon instead of a checkmark.
    pub icon_checkins: bool,
    /// "Constant Reminder": reminders stay on screen until answered.
    pub constant_reminder: bool,
    /// "Auto-pop Habit Log": open the Habit Log dialog on Done / Unachieved check-in.
    pub record_enable: bool,
    /// Explicitly skipped dates, sorted ascending.
    pub ex_dates: Vec<NaiveDate>,
    /// Manual ordering within a section (ascending).
    pub sort_order: i64,
}

/// Complete form payload for creating or updating a habit atomically.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HabitDraft {
    pub name: String,
    pub goal: Option<Goal>,
    pub reminders: Vec<NaiveTime>,
    pub frequency: Frequency,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub start_date: Option<NaiveDate>,
    pub goal_days: Option<u32>,
    pub section: HabitSection,
    pub constant_reminder: bool,
    pub record_enable: bool,
}

impl HabitDraft {
    pub fn from_habit(habit: &Habit) -> Self {
        Self {
            name: habit.name.clone(),
            goal: habit.goal.clone(),
            reminders: habit.reminders.clone(),
            frequency: habit.frequency,
            icon: habit.icon.clone(),
            color: habit.color.clone(),
            start_date: habit.start_date,
            goal_days: habit.goal_days,
            section: habit.section,
            constant_reminder: habit.constant_reminder,
            record_enable: habit.record_enable,
        }
    }
}

/// A habit's section: built-in time-of-day sections or a user-created section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HabitSection {
    Morning,
    Afternoon,
    Night,
    /// A section from the `habit_sections` table.
    Custom(crate::model::Id),
    #[default]
    Others,
}

/// A section the user added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomSection {
    pub id: crate::model::Id,
    pub name: String,
    pub sort_order: i64,
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

    /// Suggested default reminder time when assigning a habit to a time-of-day section.
    pub fn default_reminder(self) -> Option<NaiveTime> {
        match self {
            Self::Morning => NaiveTime::from_hms_opt(9, 0, 0),
            Self::Afternoon => NaiveTime::from_hms_opt(13, 0, 0),
            Self::Night => NaiveTime::from_hms_opt(20, 0, 0),
            Self::Custom(_) | Self::Others => None,
        }
    }

    pub fn is_default_section_reminder(time: NaiveTime) -> bool {
        matches!(
            (time.hour(), time.minute(), time.second()),
            (9, 0, 0) | (13, 0, 0) | (20, 0, 0)
        )
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

/// "Goal Days" presets; "Custom" allows any number in `1..=999`.
pub const GOAL_DAYS_PRESETS: [u32; 5] = [7, 21, 30, 100, 365];

impl Habit {
    /// Whether `date` was explicitly skipped via `ex_dates`.
    pub fn is_skipped(&self, date: NaiveDate) -> bool {
        self.ex_dates.binary_search(&date).is_ok()
    }

    /// Whether the habit should be done on `date`: respecting start date, archive date, skips, and frequency.
    pub fn is_due(&self, date: NaiveDate, done: &HashSet<NaiveDate>) -> bool {
        self.is_due_on(date, done, Weekday::Mon)
    }

    /// Like [`Self::is_due`], respecting the user's configured week start day.
    pub fn is_due_on(&self, date: NaiveDate, done: &HashSet<NaiveDate>, week_start: Weekday) -> bool {
        if self.start_date.is_some_and(|start| date < start) {
            return false;
        }
        if self.archived_date.is_some_and(|archived| date > archived) {
            return false;
        }
        if self.is_skipped(date) {
            return false;
        }
        self.frequency.is_due_with_week_start(date, done, week_start)
    }

    /// Whether this habit should be shown/counted on `date` (either due, or already has a handled/partial check-in).
    pub fn is_active_on(&self, date: NaiveDate, records: &[Checkin], week_start: Weekday) -> bool {
        if records.iter().any(|r| r.date == date && (r.is_handled() || r.is_partial())) {
            return true;
        }
        let done = done_dates(records);
        self.is_due_on(date, &done, week_start)
    }

    /// Effective daily goal amount (`1.0` for boolean habits).
    pub fn goal_amount(&self) -> f64 {
        self.goal.as_ref().map_or(1.0, |g| g.amount.max(0.01))
    }
}

/// How often a habit is meant to be done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frequency {
    /// On the given weekdays (bit 0 = Monday); all seven is "every day".
    Daily { weekdays: u8 },
    /// Any N days a week (1..=6; 7 normalizes to Daily every day).
    Weekly { days: u8 },
    /// Every N days (2..=30; 1 normalizes to Daily every day).
    Interval { days: u32 },
}

impl Default for Frequency {
    fn default() -> Self {
        Self::Daily { weekdays: Self::EVERY_DAY }
    }
}

/// Returns the first date of the week containing `date` according to `week_start`.
pub fn week_start_date(date: NaiveDate, week_start: Weekday) -> NaiveDate {
    let offset = (date.weekday().num_days_from_monday() + 7 - week_start.num_days_from_monday()) % 7;
    date - Duration::days(offset as i64)
}

impl Frequency {
    pub const EVERY_DAY: u8 = 0b111_1111;

    /// Normalizes degenerate weekly/interval rules to Daily Every Day.
    pub fn normalize(self) -> Self {
        match self {
            Self::Daily { weekdays } => {
                let mask = weekdays & Self::EVERY_DAY;
                Self::Daily { weekdays: if mask == 0 { Self::EVERY_DAY } else { mask } }
            }
            Self::Weekly { days } => {
                if days >= 7 {
                    Self::Daily { weekdays: Self::EVERY_DAY }
                } else {
                    Self::Weekly { days: days.max(1) }
                }
            }
            Self::Interval { days } => {
                if days <= 1 {
                    Self::Daily { weekdays: Self::EVERY_DAY }
                } else {
                    Self::Interval { days: days.min(30) }
                }
            }
        }
    }

    /// Whether the habit should be done on `date`, given the days it was done (Monday week start).
    pub fn is_due(&self, date: NaiveDate, done: &HashSet<NaiveDate>) -> bool {
        self.is_due_with_week_start(date, done, Weekday::Mon)
    }

    /// Whether the habit should be done on `date`, respecting `week_start`.
    pub fn is_due_with_week_start(&self, date: NaiveDate, done: &HashSet<NaiveDate>, week_start: Weekday) -> bool {
        match *self {
            Self::Daily { weekdays } => weekdays & (1 << date.weekday().num_days_from_monday()) != 0,
            Self::Weekly { days } => {
                let start = week_start_date(date, week_start);
                let this_week = (0..7)
                    .map(|i| start + Duration::days(i))
                    .filter(|d| *d != date && done.contains(d))
                    .count();
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
            ("daily", Ok(mask)) if mask & Self::EVERY_DAY as u32 != 0 => {
                Self::Daily { weekdays: mask as u8 & Self::EVERY_DAY }
            }
            ("weekly", Ok(days)) if (1..=7).contains(&days) => Self::Weekly { days: days as u8 },
            ("interval", Ok(days)) if days >= 1 => Self::Interval { days },
            _ => Self::default(),
        }
    }
}

/// Habit reminders firing in `(after, until]`: every reminder time of every day, unless the habit
/// is already done (or skipped / not due) that day. `done` maps habits to their completed days.
pub fn reminders_between<'a>(
    habits: &'a [Habit],
    done: &HashMap<crate::model::Id, HashSet<NaiveDate>>,
    after: NaiveDateTime,
    until: NaiveDateTime,
) -> Vec<(&'a Habit, NaiveDateTime)> {
    let mut fired = Vec::new();
    let empty = HashSet::new();
    for habit in habits.iter().filter(|h| !h.archived) {
        let mut day = after.date();
        while day <= until.date() {
            let days_done = done.get(&habit.id);
            let done_today = days_done.is_some_and(|days| days.contains(&day))
                || !habit.is_due(day, days_done.unwrap_or(&empty));
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

/// Snoozed habit reminders coming due in `(after, until]`, unless the habit was archived, skipped,
/// or done that day meanwhile.
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
            (!done_that_day && !habit.is_skipped(at.date())).then_some((habit, *at))
        })
        .collect()
}

/// Explicit status of a daily habit record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum CheckinStatus {
    /// Unchecked, partial progress, or log-only entry.
    #[default]
    Unlabeled = 0,
    /// Explicitly marked as Unachieved / failed.
    Undone = 1,
    /// Goal reached / completed.
    Done = 2,
}

impl CheckinStatus {
    pub fn from_i64(value: i64) -> Self {
        match value {
            1 => Self::Undone,
            2 => Self::Done,
            _ => Self::Unlabeled,
        }
    }

    pub fn to_i64(self) -> i64 {
        self as i64
    }
}

/// One day's check-in record: numeric progress, historical goal snapshot, explicit status,
/// optional log note, and mood rating.
#[derive(Clone, Debug, PartialEq)]
pub struct Checkin {
    pub date: NaiveDate,
    pub value: f64,
    pub goal: f64,
    pub status: CheckinStatus,
    pub note: Option<String>,
    /// Mood value: 0 (none) or one of `MOODS` (10, 20, 30, 40, 50).
    pub mood: u8,
}

impl Checkin {
    pub fn is_done(&self) -> bool {
        self.status == CheckinStatus::Done
    }

    pub fn is_undone(&self) -> bool {
        self.status == CheckinStatus::Undone
    }

    pub fn is_handled(&self) -> bool {
        matches!(self.status, CheckinStatus::Done | CheckinStatus::Undone)
    }

    pub fn is_partial(&self) -> bool {
        self.status == CheckinStatus::Unlabeled && self.value > 0.0
    }

    pub fn has_log(&self) -> bool {
        self.mood != 0 || self.note.as_deref().is_some_and(|n| !n.trim().is_empty())
    }

    pub fn is_empty(&self) -> bool {
        self.status == CheckinStatus::Unlabeled && self.value <= 0.0 && !self.has_log()
    }

    pub fn progress(&self) -> f32 {
        match self.status {
            CheckinStatus::Done => 1.0,
            CheckinStatus::Undone => 0.0,
            CheckinStatus::Unlabeled => {
                if self.goal > 0.0 {
                    (self.value / self.goal).clamp(0.0, 1.0) as f32
                } else {
                    0.0
                }
            }
        }
    }
}

/// Extracts the set of completed (`Done`) dates from a habit's check-in records.
pub fn done_dates(records: &[Checkin]) -> HashSet<NaiveDate> {
    records.iter().filter(|c| c.is_done()).map(|c| c.date).collect()
}

/// The amount done in the month containing `month`, and in total, for habits with a goal.
/// Partial check-ins count toward completion totals; Undone entries contribute 0.
pub fn completion(checkins: &[Checkin], month: NaiveDate) -> (f64, f64) {
    let in_month = |date: &NaiveDate| date.year() == month.year() && date.month() == month.month();
    let monthly: f64 = checkins
        .iter()
        .filter(|c| !c.is_undone() && in_month(&c.date))
        .map(|c| c.value)
        .sum();
    let total: f64 = checkins.iter().filter(|c| !c.is_undone()).map(|c| c.value).sum();
    ((monthly * 100.0).round() / 100.0, (total * 100.0).round() / 100.0)
}

/// Check-in statistics shown on a habit card and details panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HabitStats {
    /// All days the habit was completed (`Done`).
    pub total_days: usize,
    /// Current active streak up to `today` (an unchecked `today` does not break it).
    pub streak: usize,
    /// Longest streak achieved across the habit's history.
    pub best_streak: usize,
}

pub fn stats(checkins: &HashSet<NaiveDate>, today: NaiveDate) -> HabitStats {
    stats_with(&Frequency::default(), checkins, today, Weekday::Mon)
}

pub fn stats_for(frequency: &Frequency, checkins: &HashSet<NaiveDate>, today: NaiveDate) -> HabitStats {
    stats_with(frequency, checkins, today, Weekday::Mon)
}

/// Computes total days, current streak, and best streak for `Daily`, `Weekly`, and `Interval` frequencies.
pub fn stats_with(
    frequency: &Frequency,
    checkins: &HashSet<NaiveDate>,
    today: NaiveDate,
    week_start: Weekday,
) -> HabitStats {
    let total_days = checkins.len();
    let past_or_today: Vec<NaiveDate> = checkins.iter().copied().filter(|d| *d <= today).collect();
    if past_or_today.is_empty() {
        return HabitStats { total_days, streak: 0, best_streak: 0 };
    }

    match *frequency {
        Frequency::Daily { weekdays } => {
            let scheduled = |day: NaiveDate| weekdays & (1 << day.weekday().num_days_from_monday()) != 0;
            let Some(oldest) = past_or_today.iter().min().copied() else {
                return HabitStats { total_days, streak: 0, best_streak: 0 };
            };
            let mut run = 0usize;
            let mut best = 0usize;
            let mut day = oldest;
            while day <= today {
                if checkins.contains(&day) {
                    run += 1;
                    best = best.max(run);
                } else if day != today && scheduled(day) {
                    run = 0;
                }
                day += Duration::days(1);
            }
            HabitStats { total_days, streak: run, best_streak: best }
        }
        Frequency::Weekly { days } => {
            let target = days.max(1) as usize;
            let mut by_week: BTreeMap<NaiveDate, usize> = BTreeMap::new();
            for day in &past_or_today {
                *by_week.entry(week_start_date(*day, week_start)).or_default() += 1;
            }
            let current_week = week_start_date(today, week_start);
            let Some(&first_week) = by_week.keys().next() else {
                return HabitStats { total_days, streak: 0, best_streak: 0 };
            };
            let mut run = 0usize;
            let mut best = 0usize;
            let mut week = first_week;
            while week <= current_week {
                let count = by_week.get(&week).copied().unwrap_or(0);
                if week == current_week || count >= target {
                    run += count;
                    best = best.max(run);
                } else {
                    best = best.max(run + count);
                    run = 0;
                }
                week += Duration::days(7);
            }
            HabitStats { total_days, streak: run, best_streak: best }
        }
        Frequency::Interval { days } => {
            let interval = days.max(1) as i64;
            let mut dates = past_or_today;
            dates.sort_unstable();
            dates.dedup();
            let mut run = 1usize;
            let mut best = 1usize;
            for pair in dates.windows(2) {
                if (pair[1] - pair[0]).num_days() <= interval {
                    run += 1;
                    best = best.max(run);
                } else {
                    run = 1;
                }
            }
            let newest = dates.last().copied().unwrap_or(today);
            let streak = if (today - newest).num_days() <= interval { run } else { 0 };
            HabitStats { total_days, streak, best_streak: best }
        }
    }
}

/// Check-in statistics for one month, shown in the habit details panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MonthStats {
    /// Days of the month the habit was completed (`Done`).
    pub days: usize,
    /// Checked days as a share of the month's scheduled days in the active window, in whole percent (`0..=100`).
    pub rate: u32,
}

pub fn month_stats(checkins: &HashSet<NaiveDate>, month: NaiveDate, today: NaiveDate) -> MonthStats {
    month_stats_with(&Frequency::default(), checkins, month, today, None, None, Weekday::Mon)
}

pub fn month_stats_for(
    frequency: &Frequency,
    checkins: &HashSet<NaiveDate>,
    month: NaiveDate,
    today: NaiveDate,
) -> MonthStats {
    month_stats_with(frequency, checkins, month, today, None, None, Weekday::Mon)
}

/// Computes monthly check-in count and rate accurately across `Daily`, `Weekly`, and `Interval`
/// frequencies, respecting `start_date`, `archived_date`, and `week_start`.
pub fn month_stats_with(
    frequency: &Frequency,
    checkins: &HashSet<NaiveDate>,
    month: NaiveDate,
    today: NaiveDate,
    start_date: Option<NaiveDate>,
    archived_date: Option<NaiveDate>,
    week_start: Weekday,
) -> MonthStats {
    let in_month = |date: &NaiveDate| date.year() == month.year() && date.month() == month.month();
    let days = checkins.iter().filter(|date| in_month(date)).count();
    let first = month.with_day(1).unwrap_or(month);
    let next = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
    }
    .unwrap_or(first);
    let month_end = next - Duration::days(1);
    let cap = archived_date.map_or(today, |a| a.min(today));
    let effective_end = month_end.min(cap);

    let earliest_checkin = checkins.iter().min().copied();
    let effective_start = match (start_date, earliest_checkin) {
        (Some(s), Some(e)) => Some(s.min(e)),
        (Some(s), None) => Some(s),
        (None, _) => None,
    };
    let active_start = effective_start.map_or(first, |s| first.max(s));

    if effective_end < active_start {
        return MonthStats { days, rate: 0 };
    }

    let scheduled: i64 = match *frequency {
        Frequency::Daily { weekdays } => {
            let mut count = 0i64;
            let mut day = active_start;
            while day <= effective_end {
                if weekdays & (1 << day.weekday().num_days_from_monday()) != 0 {
                    count += 1;
                }
                day += Duration::days(1);
            }
            count
        }
        Frequency::Interval { days: interval } => {
            let interval = interval.max(1) as i64;
            let span = (effective_end - active_start).num_days() + 1;
            ((span + interval - 1) / interval).max(1)
        }
        Frequency::Weekly { days: target } => {
            let target = target.max(1) as i64;
            let mut total = 0i64;
            let mut ws = week_start_date(active_start, week_start);
            while ws <= effective_end {
                let we = ws + Duration::days(6);
                let seg_start = ws.max(active_start);
                let seg_end = we.min(effective_end);
                if seg_start <= seg_end {
                    let in_window_days = (seg_end - seg_start).num_days() + 1;
                    let done_outside_month = checkins
                        .iter()
                        .filter(|d| **d >= ws && **d <= we && !in_month(d))
                        .count() as i64;
                    let needed = (target - done_outside_month).max(0).min(in_window_days);
                    total += needed;
                }
                ws += Duration::days(7);
            }
            total
        }
    };

    let rate = if scheduled <= 0 {
        0
    } else {
        ((days as i64 * 100) / scheduled).clamp(0, 100) as u32
    };
    MonthStats { days, rate }
}

/// One completed Goal Days cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HabitCycle {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub count: u32,
}

/// Goal Days cycle progress for a habit with a finite `goal_days`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CycleSummary {
    pub completed: Vec<HabitCycle>,
    pub current_count: u32,
    pub target_days: u32,
}

/// Computes completed cycles and current cycle progress for habits with a finite `goal_days`.
pub fn goal_cycles(habit: &Habit, done: &HashSet<NaiveDate>) -> Option<CycleSummary> {
    let target = habit.goal_days.filter(|d| *d > 0)?;
    let mut eligible: Vec<NaiveDate> = done
        .iter()
        .copied()
        .filter(|d| habit.start_date.is_none_or(|start| *d >= start))
        .collect();
    eligible.sort_unstable();
    let chunk_size = target as usize;
    let mut completed = Vec::new();
    let mut current_count = 0u32;
    for chunk in eligible.chunks(chunk_size) {
        if chunk.len() == chunk_size {
            if let (Some(&start), Some(&end)) = (chunk.first(), chunk.last()) {
                completed.push(HabitCycle { start, end, count: target });
            }
        } else {
            current_count = chunk.len() as u32;
        }
    }
    Some(CycleSummary { completed, current_count, target_days: target })
}

/// Aggregate daily completion state across all active habits (for week strip & overview rings).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DayProgress {
    /// No active habits were due on this date.
    Needless,
    /// Every due habit on this date was explicitly marked Unachieved.
    AllUndone,
    /// Proportional completion across due habits (`0.0..=1.0`).
    Fraction(f32),
}

pub fn day_progress(
    habits: &[Habit],
    records_by_habit: &HashMap<Id, Vec<Checkin>>,
    date: NaiveDate,
    week_start: Weekday,
) -> DayProgress {
    let mut due_count = 0usize;
    let mut undone_count = 0usize;
    let mut sum = 0.0f32;
    for habit in habits.iter().filter(|h| !h.archived) {
        let records = records_by_habit.get(&habit.id).map(Vec::as_slice).unwrap_or(&[]);
        if !habit.is_active_on(date, records, week_start) {
            continue;
        }
        due_count += 1;
        if let Some(rec) = records.iter().find(|r| r.date == date) {
            if rec.is_undone() {
                undone_count += 1;
            } else {
                sum += rec.progress();
            }
        }
    }
    if due_count == 0 {
        DayProgress::Needless
    } else if undone_count == due_count {
        DayProgress::AllUndone
    } else {
        DayProgress::Fraction((sum / due_count as f32).clamp(0.0, 1.0))
    }
}

/// If `habit` has a time-based numeric goal (minutes or hours), returns the delta to add for a
/// completed focus session of `focused_minutes`.
pub fn focus_checkin_delta(habit: &Habit, focused_minutes: f64) -> Option<f64> {
    if habit.archived || !focused_minutes.is_finite() || focused_minutes <= 0.0 {
        return None;
    }
    let goal = habit.goal.as_ref()?;
    let unit = goal.unit.trim().to_lowercase();
    let delta = match unit.as_str() {
        "min" | "mins" | "minute" | "minutes" | "мин" | "мин." | "минут" | "минута" | "минуты" | "分钟" | "分" => {
            polish_amount(focused_minutes)
        }
        "h" | "hr" | "hrs" | "hour" | "hours" | "ч" | "ч." | "час" | "часа" | "часов" | "小时" | "时" => {
            polish_amount(focused_minutes / 60.0)
        }
        _ => return None,
    };
    (delta > 0.0).then_some(delta)
}

/// The top of the "Daily Goals" chart's scale: three steps over the goal, or higher when a day
/// went past that.
pub fn chart_top(goal: f64, highest: f64) -> f64 {
    (goal + 3.0).max(highest).max(1.0)
}

/// Habits and their check-ins as CSV: one line per check-in, habits without any get one line
/// with empty check-in fields.
pub fn export_csv(habits: &[(Habit, Vec<Checkin>)]) -> String {
    let field = |text: &str| {
        if text.contains([',', '"', '\n']) {
            format!("\"{}\"", text.replace('"', "\"\""))
        } else {
            text.to_owned()
        }
    };
    let mut out = String::from("habit,archived,goal,unit,date,amount,note\n");
    for (habit, checkins) in habits {
        let goal = habit.goal.as_ref().map(|g| format_amount(g.amount)).unwrap_or_default();
        let unit = habit.goal.as_ref().map(|g| field(&g.unit)).unwrap_or_default();
        let prefix = format!("{},{},{goal},{unit}", field(&habit.name), habit.archived);
        if checkins.is_empty() {
            out.push_str(&format!("{prefix},,,\n"));
        }
        for checkin in checkins {
            let note = checkin.note.as_deref().map(field).unwrap_or_default();
            out.push_str(&format!("{prefix},{},{},{note}\n", checkin.date, format_amount(checkin.value)));
        }
    }
    out
}

use chrono::Timelike;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_formatting_and_parsing() {
        assert_eq!(format_amount(8.0), "8");
        assert_eq!(format_amount(1.5), "1.5");
        assert_eq!(format_amount(1.25), "1.25");
        assert_eq!(format_amount(1.256), "1.26");
        assert_eq!(parse_amount("8"), Some(8.0));
        assert_eq!(parse_amount(" 1,5 "), Some(1.5));
        assert_eq!(parse_amount("0"), None);
        assert_eq!(parse_amount("-2"), None);
        assert_eq!(parse_amount("100000"), None);
        assert_eq!(parse_amount_non_negative("0"), Some(0.0));
    }

    #[test]
    fn chart_scale() {
        assert_eq!(chart_top(2.0, 0.0), 5.0);
        assert_eq!(chart_top(2.0, 9.0), 9.0);
        assert_eq!(chart_top(1.0, 1.0), 4.0);
    }

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
    }

    fn sample_habit(name: &str) -> Habit {
        Habit {
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
            archived_date: None,
            icon_checkins: false,
            constant_reminder: false,
            record_enable: false,
            ex_dates: Vec::new(),
            sort_order: 0,
        }
    }

    #[test]
    fn streak_and_best_streak_across_frequencies() {
        let today = d(18);
        let checked: HashSet<_> = [d(15), d(16), d(17)].into();
        assert_eq!(stats(&checked, today), HabitStats { total_days: 3, streak: 3, best_streak: 3 });

        let checked: HashSet<_> = [d(10), d(11), d(12), d(16), d(18)].into();
        assert_eq!(stats(&checked, today), HabitStats { total_days: 5, streak: 1, best_streak: 3 });

        let checked: HashSet<_> = [d(10), d(15)].into();
        assert_eq!(stats(&checked, today), HabitStats { total_days: 2, streak: 0, best_streak: 1 });

        // Daily Mon/Wed/Fri with bonus Tuesday check-in extends streak.
        let mon_wed_fri = Frequency::Daily { weekdays: 0b10101 };
        let checked: HashSet<_> = [d(19), d(20), d(21), d(23)].into();
        assert_eq!(
            stats_with(&mon_wed_fri, &checked, d(24), Weekday::Mon),
            HabitStats { total_days: 4, streak: 4, best_streak: 4 }
        );

        // Weekly 2x/week: week of Oct 5 (2 done) + week of Oct 12 (2 done) + week of Oct 19 (1 done) = 5.
        let weekly = Frequency::Weekly { days: 2 };
        let checked: HashSet<_> = [d(5), d(7), d(12), d(14), d(19)].into();
        assert_eq!(
            stats_with(&weekly, &checked, d(20), Weekday::Mon),
            HabitStats { total_days: 5, streak: 5, best_streak: 5 }
        );
        // Missed week of Oct 12 (only 1 done) breaks current streak, best = 3.
        let checked: HashSet<_> = [d(5), d(7), d(12), d(19)].into();
        assert_eq!(
            stats_with(&weekly, &checked, d(20), Weekday::Mon),
            HabitStats { total_days: 4, streak: 1, best_streak: 3 }
        );

        // Interval every 3 days: Oct 1, 4, 7 -> streak 3 on Oct 10, 0 on Oct 11 (best stays 3).
        let interval = Frequency::Interval { days: 3 };
        let checked: HashSet<_> = [d(1), d(4), d(7)].into();
        assert_eq!(
            stats_with(&interval, &checked, d(10), Weekday::Mon),
            HabitStats { total_days: 3, streak: 3, best_streak: 3 }
        );
        assert_eq!(
            stats_with(&interval, &checked, d(11), Weekday::Mon),
            HabitStats { total_days: 3, streak: 0, best_streak: 3 }
        );
    }

    #[test]
    fn goal_cycles_chunk_from_start_date() {
        let habit = Habit { start_date: Some(d(5)), goal_days: Some(3), ..sample_habit("Run") };
        let done: HashSet<_> = [d(1), d(5), d(6), d(7), d(9)].into();
        let summary = goal_cycles(&habit, &done).unwrap();
        assert_eq!(summary.completed, vec![HabitCycle { start: d(5), end: d(7), count: 3 }]);
        assert_eq!(summary.current_count, 1);
        assert_eq!(summary.target_days, 3);
    }

    #[test]
    fn day_progress_handles_partial_undone_and_needless() {
        let h1 = Habit { goal: Some(Goal::new(4.0, "cups")), ..sample_habit("Water") };
        let h2 = sample_habit("Read");
        let mut map = HashMap::new();
        map.insert(
            h1.id,
            vec![Checkin { date: d(10), value: 2.0, goal: 4.0, status: CheckinStatus::Unlabeled, note: None, mood: 0 }],
        );
        map.insert(
            h2.id,
            vec![Checkin { date: d(10), value: 1.0, goal: 1.0, status: CheckinStatus::Done, note: None, mood: 0 }],
        );
        let habits = [h1.clone(), h2.clone()];
        assert_eq!(day_progress(&habits, &map, d(10), Weekday::Mon), DayProgress::Fraction(0.75));

        // All undone -> AllUndone.
        map.insert(
            h1.id,
            vec![Checkin { date: d(11), value: 0.0, goal: 4.0, status: CheckinStatus::Undone, note: None, mood: 0 }],
        );
        map.insert(
            h2.id,
            vec![Checkin { date: d(11), value: 0.0, goal: 1.0, status: CheckinStatus::Undone, note: None, mood: 0 }],
        );
        assert_eq!(day_progress(&habits, &map, d(11), Weekday::Mon), DayProgress::AllUndone);

        // No habits due -> Needless.
        let future_only = [Habit { start_date: Some(d(20)), ..sample_habit("Later") }];
        assert_eq!(day_progress(&future_only, &HashMap::new(), d(10), Weekday::Mon), DayProgress::Needless);
    }

    #[test]
    fn focus_auto_checkin_units() {
        let mins = Habit { goal: Some(Goal::new(30.0, "мин")), ..sample_habit("Meditate") };
        let hours = Habit { goal: Some(Goal::new(2.0, "hours")), ..sample_habit("Study") };
        let pages = Habit { goal: Some(Goal::new(20.0, "pages")), ..sample_habit("Read") };
        assert_eq!(focus_checkin_delta(&mins, 25.0), Some(25.0));
        assert_eq!(focus_checkin_delta(&hours, 30.0), Some(0.5));
        assert_eq!(focus_checkin_delta(&pages, 25.0), None);
    }

    #[test]
    fn habit_reminders_skip_done_and_ex_dates() {
        let at = |day, h| d(day).and_hms_opt(h, 0, 0).unwrap();
        let habit = Habit {
            reminders: vec![NaiveTime::from_hms_opt(9, 0, 0).unwrap(), NaiveTime::from_hms_opt(21, 0, 0).unwrap()],
            ..sample_habit("Read")
        };
        let habits = [habit.clone()];
        let mut done = HashMap::new();
        let fired = reminders_between(&habits, &done, at(17, 8), at(18, 10));
        let times: Vec<NaiveDateTime> = fired.iter().map(|(_, at)| *at).collect();
        assert_eq!(times, [at(17, 9), at(17, 21), at(18, 9)]);
        done.insert(habit.id, HashSet::from([d(17)]));
        assert_eq!(reminders_between(&habits, &done, at(17, 8), at(18, 10)).len(), 1);

        // Skipped date suppresses reminders.
        let skipped = [Habit { ex_dates: vec![d(18)], ..habit.clone() }];
        assert!(reminders_between(&skipped, &done, at(17, 8), at(18, 10)).is_empty());

        // Nothing before the start date.
        let later = [Habit { start_date: Some(d(19)), ..habit.clone() }];
        assert!(reminders_between(&later, &HashMap::new(), at(17, 8), at(18, 23)).is_empty());
        assert!(!later[0].is_due(d(18), &HashSet::new()) && later[0].is_due(d(19), &HashSet::new()));

        // Archived habits are not reminded.
        let archived = [Habit { archived: true, ..habit.clone() }];
        assert!(reminders_between(&archived, &HashMap::new(), at(17, 8), at(18, 23)).is_empty());

        let snoozes = [(habit.id, at(18, 9) + Duration::minutes(15))];
        let back = snoozed_between(&habits, &HashMap::new(), &snoozes, at(18, 9), at(18, 10));
        assert_eq!(back.iter().map(|(_, at)| *at).collect::<Vec<_>>(), [snoozes[0].1]);
        assert!(snoozed_between(&habits, &HashMap::new(), &snoozes, at(18, 10), at(18, 11)).is_empty());
        let done_today = HashMap::from([(habit.id, HashSet::from([d(18)]))]);
        assert!(snoozed_between(&habits, &done_today, &snoozes, at(18, 9), at(18, 10)).is_empty());
        assert!(snoozed_between(&archived, &HashMap::new(), &snoozes, at(18, 9), at(18, 10)).is_empty());
        assert_eq!(HabitSection::from_key("night"), HabitSection::Night);
        assert_eq!(HabitSection::from_key("custom"), HabitSection::Others);
        let custom = CustomSection { id: uuid::Uuid::now_v7(), name: "Pets".into(), sort_order: 0 };
        let section = HabitSection::Custom(custom.id);
        assert_eq!(HabitSection::from_key(&section.key()), section);
        assert_eq!(HabitSection::ordered(&[custom])[3], section);
    }

    #[test]
    fn frequencies_and_monthly_rates() {
        let mon_wed_fri = Frequency::Daily { weekdays: 0b10101 };
        let none = HashSet::new();
        assert!(mon_wed_fri.is_due(d(19), &none) && !mon_wed_fri.is_due(d(20), &none));

        let twice_a_week = Frequency::Weekly { days: 2 };
        let done: HashSet<_> = [d(19), d(20)].into();
        assert!(!twice_a_week.is_due(d(21), &done));
        assert!(twice_a_week.is_due(d(20), &done));
        assert!(twice_a_week.is_due(d(26), &done));

        let every_third_day = Frequency::Interval { days: 3 };
        let done: HashSet<_> = [d(19)].into();
        assert!(!every_third_day.is_due(d(20), &done) && !every_third_day.is_due(d(21), &done));
        assert!(every_third_day.is_due(d(22), &done));

        for f in [mon_wed_fri, twice_a_week, every_third_day, Frequency::default()] {
            assert_eq!(Frequency::from_storage(&f.to_storage()), f);
        }
        assert_eq!(Frequency::from_storage("daily:0"), Frequency::default());
        assert_eq!(Frequency::Weekly { days: 7 }.normalize(), Frequency::default());
        assert_eq!(Frequency::Interval { days: 1 }.normalize(), Frequency::default());

        // Interval monthly rate: Oct 1..=10 (10 days) with interval 3 -> ceil(10/3) = 4 scheduled days.
        let checked: HashSet<_> = [d(1), d(4)].into();
        assert_eq!(month_stats_for(&every_third_day, &checked, d(1), d(10)), MonthStats { days: 2, rate: 50 });

        // Weekly 2x/week over Oct 1..=14:
        // Week 1 (Sep 28..Oct 4): 4 days in Oct -> 2 needed
        // Week 2 (Oct 5..Oct 11): 7 days in Oct -> 2 needed
        // Week 3 (Oct 12..Oct 14): 3 days in Oct -> 2 needed => 6 scheduled. 3 done => 50%.
        let checked: HashSet<_> = [d(2), d(6), d(13)].into();
        assert_eq!(month_stats_for(&twice_a_week, &checked, d(1), d(14)), MonthStats { days: 3, rate: 50 });
    }

    #[test]
    fn csv_export() {
        let water = Habit { goal: Some(Goal::new(8.0, "glasses")), ..sample_habit("Water, daily") };
        let checkins = vec![Checkin {
            date: d(18),
            value: 8.0,
            goal: 8.0,
            status: CheckinStatus::Done,
            note: Some("said \"done\"".into()),
            mood: 50,
        }];
        let csv = export_csv(&[(water, checkins), (sample_habit("Read"), Vec::new())]);
        assert_eq!(
            csv,
            "habit,archived,goal,unit,date,amount,note\n\
             \"Water, daily\",false,8,glasses,2026-10-18,8,\"said \"\"done\"\"\"\n\
             Read,false,,,,,\n"
        );
    }

    #[test]
    fn completion_sums_amounts() {
        let c = |date, value| Checkin { date, value, goal: 5.0, status: CheckinStatus::Unlabeled, note: None, mood: 0 };
        let checkins = [c(d(1), 2.5), c(d(9), 3.0), c(NaiveDate::from_ymd_opt(2026, 9, 30).unwrap(), 2.0)];
        assert_eq!(completion(&checkins, d(18)), (5.5, 7.5));
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
