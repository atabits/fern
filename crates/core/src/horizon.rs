//! Horizon planning buckets (Week, Month, Quarter, Year, Decade, Life) and pure query helpers.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use chrono::{Datelike as _, Duration, NaiveDate, Weekday};

use crate::model::{Id, List, ScheduleBucket, Task};
use crate::recurrence::{Frequency, Recurrence};

const MIN_YEAR: i32 = 1;
const MAX_YEAR: i32 = 9999;

/// Granularity tier of a [`Horizon`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HorizonGranularity {
    Week,
    Month,
    Quarter,
    Year,
    Decade,
    Life,
}

/// Time horizon bucket attached to a task instead of an exact date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub enum Horizon {
    Week { year: i32, week: u32 },
    Month { year: i32, month: u32 },
    Quarter { year: i32, quarter: u32 },
    Year(i32),
    Decade(i32),
    Life,
}

/// Number of ISO 8601 weeks in `year` (52 or 53). December 28 is always in the last ISO week.
pub fn iso_weeks_in_year(year: i32) -> u32 {
    NaiveDate::from_ymd_opt(year, 12, 28)
        .map(|d| d.iso_week().week())
        .unwrap_or(52)
}

/// Inclusive Monday..=Sunday calendar bounds for ISO `year` and `week`.
pub fn week_bounds(year: i32, week: u32) -> Option<(NaiveDate, NaiveDate)> {
    if week == 0 || week > iso_weeks_in_year(year) {
        return None;
    }
    let monday = NaiveDate::from_isoywd_opt(year, week, Weekday::Mon)?;
    let sunday = monday.checked_add_signed(Duration::days(6))?;
    Some((monday, sunday))
}

fn parse_ascii_digits_i32(raw: &str) -> Option<i32> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}

fn parse_ascii_digits_u32(raw: &str) -> Option<u32> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}

impl Horizon {
    /// Canonical storage and wire code (`"W2026-47"`, `"M2026-11"`, `"Q2026-4"`, `"Y2026"`, `"D2020"`, `"LIFE"`).
    pub fn as_code(&self) -> String {
        match *self {
            Self::Week { year, week } => format!("W{year}-{week:02}"),
            Self::Month { year, month } => format!("M{year}-{month:02}"),
            Self::Quarter { year, quarter } => format!("Q{year}-{quarter}"),
            Self::Year(year) => format!("Y{year}"),
            Self::Decade(start_year) => format!("D{start_year}"),
            Self::Life => "LIFE".to_owned(),
        }
    }

    /// Parses a canonical horizon code, validating calendar bounds.
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw == "LIFE" {
            return Some(Self::Life);
        }
        if let Some(rest) = raw.strip_prefix('W') {
            let (y_str, w_str) = rest.split_once('-')?;
            let year = parse_ascii_digits_i32(y_str)?;
            let week = parse_ascii_digits_u32(w_str)?;
            if !(MIN_YEAR..=MAX_YEAR).contains(&year) {
                return None;
            }
            let max_week = iso_weeks_in_year(year);
            if !(1..=max_week).contains(&week) {
                return None;
            }
            return Some(Self::Week { year, week });
        }
        if let Some(rest) = raw.strip_prefix('M') {
            let (y_str, m_str) = rest.split_once('-')?;
            let year = parse_ascii_digits_i32(y_str)?;
            let month = parse_ascii_digits_u32(m_str)?;
            if !(MIN_YEAR..=MAX_YEAR).contains(&year) || !(1..=12).contains(&month) {
                return None;
            }
            return Some(Self::Month { year, month });
        }
        if let Some(rest) = raw.strip_prefix('Q') {
            let (y_str, q_str) = rest.split_once('-')?;
            let year = parse_ascii_digits_i32(y_str)?;
            let quarter = parse_ascii_digits_u32(q_str)?;
            if !(MIN_YEAR..=MAX_YEAR).contains(&year) || !(1..=4).contains(&quarter) {
                return None;
            }
            return Some(Self::Quarter { year, quarter });
        }
        if let Some(rest) = raw.strip_prefix('Y') {
            let year = parse_ascii_digits_i32(rest)?;
            if !(MIN_YEAR..=MAX_YEAR).contains(&year) {
                return None;
            }
            return Some(Self::Year(year));
        }
        if let Some(rest) = raw.strip_prefix('D') {
            let start_year = parse_ascii_digits_i32(rest)?;
            if !(MIN_YEAR..=MAX_YEAR).contains(&start_year) || start_year % 10 != 0 {
                return None;
            }
            return Some(Self::Decade(start_year));
        }
        None
    }

    pub fn current_week(today: NaiveDate) -> Self {
        let iso = today.iso_week();
        Self::Week { year: iso.year(), week: iso.week() }
    }

    pub fn current_month(today: NaiveDate) -> Self {
        Self::Month { year: today.year(), month: today.month() }
    }

    pub fn current_quarter(today: NaiveDate) -> Self {
        Self::Quarter { year: today.year(), quarter: (today.month() - 1) / 3 + 1 }
    }

    pub fn current_year(today: NaiveDate) -> Self {
        Self::Year(today.year())
    }

    pub fn current_decade(today: NaiveDate) -> Self {
        Self::Decade(today.year().div_euclid(10) * 10)
    }

    pub fn current_for(granularity: HorizonGranularity, today: NaiveDate) -> Self {
        match granularity {
            HorizonGranularity::Week => Self::current_week(today),
            HorizonGranularity::Month => Self::current_month(today),
            HorizonGranularity::Quarter => Self::current_quarter(today),
            HorizonGranularity::Year => Self::current_year(today),
            HorizonGranularity::Decade => Self::current_decade(today),
            HorizonGranularity::Life => Self::Life,
        }
    }

    pub fn granularity(self) -> HorizonGranularity {
        match self {
            Self::Week { .. } => HorizonGranularity::Week,
            Self::Month { .. } => HorizonGranularity::Month,
            Self::Quarter { .. } => HorizonGranularity::Quarter,
            Self::Year(_) => HorizonGranularity::Year,
            Self::Decade(_) => HorizonGranularity::Decade,
            Self::Life => HorizonGranularity::Life,
        }
    }

    /// Shifts the horizon by `delta` periods at its own granularity. `Life` stays `Life`.
    pub fn shift(self, delta: i32) -> Self {
        match self {
            Self::Week { year, week } => {
                let Some(mon) = NaiveDate::from_isoywd_opt(year, week, Weekday::Mon) else {
                    return self;
                };
                let Some(days) = i64::from(delta).checked_mul(7) else {
                    return self;
                };
                let Some(shifted) = mon.checked_add_signed(Duration::days(days)) else {
                    return self;
                };
                let iso = shifted.iso_week();
                Self::Week { year: iso.year(), week: iso.week() }
            }
            Self::Month { year, month } => {
                let total = i64::from(year) * 12 + i64::from(month.saturating_sub(1)) + i64::from(delta);
                let new_year = total.div_euclid(12);
                let new_month = (total.rem_euclid(12) + 1) as u32;
                i32::try_from(new_year).map_or(self, |y| Self::Month { year: y, month: new_month })
            }
            Self::Quarter { year, quarter } => {
                let total = i64::from(year) * 4 + i64::from(quarter.saturating_sub(1)) + i64::from(delta);
                let new_year = total.div_euclid(4);
                let new_quarter = (total.rem_euclid(4) + 1) as u32;
                i32::try_from(new_year).map_or(self, |y| Self::Quarter { year: y, quarter: new_quarter })
            }
            Self::Year(year) => Self::Year(year.saturating_add(delta)),
            Self::Decade(start_year) => {
                let base = start_year.div_euclid(10) * 10;
                Self::Decade(base.saturating_add(delta.saturating_mul(10)))
            }
            Self::Life => Self::Life,
        }
    }

    /// Whether `self` lies strictly before the current period of the same granularity at `today`.
    pub fn is_past_relative_to(self, today: NaiveDate) -> bool {
        match self {
            Self::Week { .. } => self < Self::current_week(today),
            Self::Month { .. } => self < Self::current_month(today),
            Self::Quarter { .. } => self < Self::current_quarter(today),
            Self::Year(_) => self < Self::current_year(today),
            Self::Decade(_) => self < Self::current_decade(today),
            Self::Life => false,
        }
    }

    /// Advances a repeating horizon task to its next horizon bucket when `repeat` matches this granularity.
    pub fn advance_for_recurrence(self, repeat: &Recurrence) -> Option<Self> {
        if repeat.is_specific_dates() || repeat.forgetting_curve {
            return None;
        }
        let interval = repeat.interval.max(1);
        match (self, repeat.frequency) {
            (Self::Week { .. }, Frequency::Weekly) => {
                let delta = i32::try_from(interval).ok()?;
                Some(self.shift(delta))
            }
            (Self::Month { .. }, Frequency::Monthly) => {
                let delta = i32::try_from(interval).ok()?;
                Some(self.shift(delta))
            }
            (Self::Quarter { .. }, Frequency::Monthly) if interval.is_multiple_of(3) => {
                let delta = i32::try_from(interval / 3).ok()?;
                Some(self.shift(delta))
            }
            (Self::Year(_), Frequency::Yearly) => {
                let delta = i32::try_from(interval).ok()?;
                Some(self.shift(delta))
            }
            (Self::Year(_), Frequency::Monthly) if interval.is_multiple_of(12) => {
                let delta = i32::try_from(interval / 12).ok()?;
                Some(self.shift(delta))
            }
            _ => None,
        }
    }
}

impl fmt::Display for Horizon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_code())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseHorizonError(String);

impl fmt::Display for ParseHorizonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid horizon: {}", self.0)
    }
}

impl std::error::Error for ParseHorizonError {}

impl FromStr for Horizon {
    type Err = ParseHorizonError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| ParseHorizonError(s.trim().to_owned()))
    }
}

fn hidden_list_ids(lists: &[List]) -> HashSet<Id> {
    lists
        .iter()
        .filter(|l| l.archived || l.completed_at.is_some() || !l.in_smart_lists)
        .map(|l| l.id)
        .collect()
}

fn is_visible_in_horizons(task: &Task, hidden_lists: &HashSet<Id>) -> bool {
    !task.is_deleted()
        && !task.is_wont_do()
        && !task.list_id.is_some_and(|id| hidden_lists.contains(&id))
}

/// Whether `task` belongs in the Horizons Inbox (unassigned, undated, no horizon, no Things bucket).
pub fn is_horizons_inbox(task: &Task) -> bool {
    !task.is_deleted()
        && !task.is_wont_do()
        && task.list_id.is_none()
        && task.due_date.is_none()
        && task.start_date.is_none()
        && task.horizon.is_none()
        && task.schedule_bucket == ScheduleBucket::None
}

/// Partitions tasks for a single calendar `day` into `(tasks_for_day, overdue_open_tasks)`.
///
/// Overdue open tasks are only populated when `day == today`.
pub fn horizons_day_partition<'a>(
    tasks: &'a [Task],
    lists: &[List],
    day: NaiveDate,
    today: NaiveDate,
    include_completed: bool,
) -> (Vec<&'a Task>, Vec<&'a Task>) {
    let hidden = hidden_list_ids(lists);
    let mut for_day = Vec::new();
    let mut overdue = Vec::new();
    let collect_overdue = day == today;
    for task in tasks {
        if !is_visible_in_horizons(task, &hidden) {
            continue;
        }
        let Some(effective_day) = task.due_date.or(task.start_date) else {
            continue;
        };
        if effective_day == day {
            if include_completed || !task.is_completed() {
                for_day.push(task);
            }
        } else if collect_overdue && !task.is_completed() && effective_day < today {
            overdue.push(task);
        }
    }
    (for_day, overdue)
}

/// Partitions tasks for `target` horizon into `(tasks_in_target, past_open_rollover_tasks)`.
///
/// Past rollover tasks are populated only when `target` is the current period for `today`
/// (never for `Horizon::Life`).
pub fn horizons_bucket_partition<'a>(
    tasks: &'a [Task],
    lists: &[List],
    target: Horizon,
    today: NaiveDate,
    include_completed: bool,
) -> (Vec<&'a Task>, Vec<&'a Task>) {
    let hidden = hidden_list_ids(lists);
    let mut in_target = Vec::new();
    let mut past_rollover = Vec::new();
    let is_current = target != Horizon::Life && target == Horizon::current_for(target.granularity(), today);
    let target_granularity = target.granularity();

    for task in tasks {
        if !is_visible_in_horizons(task, &hidden) {
            continue;
        }
        let Some(horizon) = task.horizon else {
            continue;
        };
        if horizon == target {
            if include_completed || !task.is_completed() {
                in_target.push(task);
            }
        } else if is_current
            && !task.is_completed()
            && horizon.granularity() == target_granularity
            && horizon.is_past_relative_to(today)
        {
            past_rollover.push(task);
        }
    }
    (in_target, past_rollover)
}

/// Exact-dated tasks (`due_date` or fallback `start_date`) falling inside ISO week `(year, week)`,
/// optionally excluding `exclude_day` (e.g. `Some(today)`).
pub fn horizons_week_dated_tasks<'a>(
    tasks: &'a [Task],
    lists: &[List],
    year: i32,
    week: u32,
    exclude_day: Option<NaiveDate>,
    include_completed: bool,
) -> Vec<&'a Task> {
    let Some((mon, sun)) = week_bounds(year, week) else {
        return Vec::new();
    };
    let hidden = hidden_list_ids(lists);
    tasks
        .iter()
        .filter(|task| {
            if !is_visible_in_horizons(task, &hidden) {
                return false;
            }
            if !include_completed && task.is_completed() {
                return false;
            }
            let Some(effective_day) = task.due_date.or(task.start_date) else {
                return false;
            };
            effective_day >= mon && effective_day <= sun && exclude_day != Some(effective_day)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Kind, Priority};
    use uuid::Uuid;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn make_task(title: &str) -> Task {
        Task {
            id: Uuid::now_v7(),
            list_id: None,
            parent_id: None,
            kind: Kind::Task,
            title: title.to_owned(),
            notes: String::new(),
            priority: Priority::None,
            due_date: None,
            due_time: None,
            repeat: None,
            tags: Vec::new(),
            checklist: None,
            completed_at: None,
            deleted_at: None,
            wont_do_at: None,
            in_progress_at: None,
            end_date: None,
            end_time: None,
            location: None,
            timezone: None,
            is_floating: false,
            snoozed_until: None,
            constant_reminder: false,
            pinned_at: None,
            section_id: None,
            modified_at: None,
            reminders: Vec::new(),
            estimate: None,
            start_date: None,
            schedule_bucket: ScheduleBucket::None,
            horizon: None,
            created_at: 0,
        }
    }

    fn make_list(name: &str, archived: bool, in_smart_lists: bool, completed_at: Option<i64>) -> List {
        List {
            id: Uuid::now_v7(),
            name: name.to_owned(),
            color: None,
            open_count: 0,
            icon: None,
            folder: None,
            kind: Kind::Task,
            in_smart_lists,
            archived,
            notes: String::new(),
            deadline: None,
            completed_at,
        }
    }

    #[test]
    fn iso_weeks_and_week_bounds_handle_52_and_53_week_years() {
        assert_eq!(iso_weeks_in_year(2025), 52);
        assert_eq!(iso_weeks_in_year(2026), 53);
        assert_eq!(iso_weeks_in_year(2027), 52);
        assert_eq!(iso_weeks_in_year(2020), 53);

        // 2026-W01 starts on Mon 2025-12-29 and ends Sun 2026-01-04.
        assert_eq!(week_bounds(2026, 1), Some((d(2025, 12, 29), d(2026, 1, 4))));
        // 2026-W53 starts on Mon 2026-12-28 and ends Sun 2027-01-03.
        assert_eq!(week_bounds(2026, 53), Some((d(2026, 12, 28), d(2027, 1, 3))));
        // 2025 only has 52 weeks.
        assert_eq!(week_bounds(2025, 53), None);
        assert_eq!(week_bounds(2026, 0), None);
    }

    #[test]
    fn parse_and_as_code_roundtrip_and_validation() {
        let cases = [
            (Horizon::Week { year: 2026, week: 1 }, "W2026-01"),
            (Horizon::Week { year: 2026, week: 53 }, "W2026-53"),
            (Horizon::Month { year: 2026, month: 3 }, "M2026-03"),
            (Horizon::Month { year: 2026, month: 12 }, "M2026-12"),
            (Horizon::Quarter { year: 2026, quarter: 1 }, "Q2026-1"),
            (Horizon::Quarter { year: 2026, quarter: 4 }, "Q2026-4"),
            (Horizon::Year(2026), "Y2026"),
            (Horizon::Decade(2020), "D2020"),
            (Horizon::Life, "LIFE"),
        ];
        for (horizon, code) in cases {
            assert_eq!(horizon.as_code(), code);
            assert_eq!(Horizon::parse(code), Some(horizon));
            assert_eq!(Horizon::parse(&format!("  {code}  ")), Some(horizon));
            assert_eq!(code.parse::<Horizon>().ok(), Some(horizon));
            assert_eq!(horizon.to_string(), code);
        }

        // Invalid codes must be rejected.
        for bad in [
            "",
            "life",
            "W2025-53", // 2025 has 52 weeks
            "W2026-00",
            "W2026-54",
            "W0-01",
            "W-2026-01",
            "W2026-+1",
            "M2026-00",
            "M2026-13",
            "Q2026-0",
            "Q2026-5",
            "Y0",
            "Y10000",
            "D2025", // not divisible by 10
            "D0",
            "X2026",
            "2026-11-23",
        ] {
            assert_eq!(Horizon::parse(bad), None, "expected {bad:?} to be rejected");
        }
    }

    #[test]
    fn shift_across_year_and_53_week_boundaries() {
        // 2025 (52 weeks) -> 2026 (53 weeks) -> 2027 (52 weeks)
        let w52_2025 = Horizon::Week { year: 2025, week: 52 };
        let w01_2026 = w52_2025.shift(1);
        assert_eq!(w01_2026, Horizon::Week { year: 2026, week: 1 });
        assert_eq!(w01_2026.shift(-1), w52_2025);

        let w52_2026 = Horizon::Week { year: 2026, week: 52 };
        let w53_2026 = w52_2026.shift(1);
        assert_eq!(w53_2026, Horizon::Week { year: 2026, week: 53 });
        let w01_2027 = w53_2026.shift(1);
        assert_eq!(w01_2027, Horizon::Week { year: 2027, week: 1 });
        assert_eq!(w01_2027.shift(-1), w53_2026);
        assert_eq!(w01_2027.shift(-2), w52_2026);

        // Months across year boundaries.
        assert_eq!(
            Horizon::Month { year: 2026, month: 12 }.shift(1),
            Horizon::Month { year: 2027, month: 1 }
        );
        assert_eq!(
            Horizon::Month { year: 2026, month: 1 }.shift(-1),
            Horizon::Month { year: 2025, month: 12 }
        );
        assert_eq!(
            Horizon::Month { year: 2026, month: 1 }.shift(-14),
            Horizon::Month { year: 2024, month: 11 }
        );

        // Quarters across year boundaries.
        assert_eq!(
            Horizon::Quarter { year: 2026, quarter: 4 }.shift(1),
            Horizon::Quarter { year: 2027, quarter: 1 }
        );
        assert_eq!(
            Horizon::Quarter { year: 2026, quarter: 1 }.shift(-1),
            Horizon::Quarter { year: 2025, quarter: 4 }
        );

        // Years, Decades, and Life.
        assert_eq!(Horizon::Year(2026).shift(3), Horizon::Year(2029));
        assert_eq!(Horizon::Year(2026).shift(-6), Horizon::Year(2020));
        assert_eq!(Horizon::Decade(2020).shift(1), Horizon::Decade(2030));
        assert_eq!(Horizon::Decade(2020).shift(-2), Horizon::Decade(2000));
        assert_eq!(Horizon::Life.shift(5), Horizon::Life);
        assert_eq!(Horizon::Life.shift(-5), Horizon::Life);
    }

    #[test]
    fn current_periods_and_is_past_relative_to() {
        let today = d(2026, 11, 23); // Monday of 2026-W48
        assert_eq!(Horizon::current_week(today), Horizon::Week { year: 2026, week: 48 });
        assert_eq!(Horizon::current_month(today), Horizon::Month { year: 2026, month: 11 });
        assert_eq!(Horizon::current_quarter(today), Horizon::Quarter { year: 2026, quarter: 4 });
        assert_eq!(Horizon::current_year(today), Horizon::Year(2026));
        assert_eq!(Horizon::current_decade(today), Horizon::Decade(2020));

        assert!(Horizon::Week { year: 2026, week: 47 }.is_past_relative_to(today));
        assert!(!Horizon::Week { year: 2026, week: 48 }.is_past_relative_to(today));
        assert!(!Horizon::Week { year: 2026, week: 49 }.is_past_relative_to(today));

        assert!(Horizon::Month { year: 2026, month: 10 }.is_past_relative_to(today));
        assert!(!Horizon::Month { year: 2026, month: 11 }.is_past_relative_to(today));

        assert!(Horizon::Quarter { year: 2026, quarter: 3 }.is_past_relative_to(today));
        assert!(!Horizon::Quarter { year: 2026, quarter: 4 }.is_past_relative_to(today));

        assert!(Horizon::Year(2025).is_past_relative_to(today));
        assert!(!Horizon::Year(2026).is_past_relative_to(today));

        assert!(Horizon::Decade(2010).is_past_relative_to(today));
        assert!(!Horizon::Decade(2020).is_past_relative_to(today));

        assert!(!Horizon::Life.is_past_relative_to(today));
    }

    #[test]
    fn advance_for_recurrence_matches_granularity_rules() {
        let w = Horizon::Week { year: 2026, week: 53 };
        assert_eq!(
            w.advance_for_recurrence(&Recurrence::new(Frequency::Weekly, 1)),
            Some(Horizon::Week { year: 2027, week: 1 })
        );
        assert_eq!(
            w.advance_for_recurrence(&Recurrence::new(Frequency::Weekly, 2)),
            Some(Horizon::Week { year: 2027, week: 2 })
        );
        assert_eq!(w.advance_for_recurrence(&Recurrence::new(Frequency::Daily, 1)), None);
        assert_eq!(w.advance_for_recurrence(&Recurrence::new(Frequency::Monthly, 1)), None);

        let m = Horizon::Month { year: 2026, month: 11 };
        assert_eq!(
            m.advance_for_recurrence(&Recurrence::new(Frequency::Monthly, 2)),
            Some(Horizon::Month { year: 2027, month: 1 })
        );
        assert_eq!(m.advance_for_recurrence(&Recurrence::new(Frequency::Weekly, 1)), None);

        let q = Horizon::Quarter { year: 2026, quarter: 4 };
        assert_eq!(
            q.advance_for_recurrence(&Recurrence::new(Frequency::Monthly, 3)),
            Some(Horizon::Quarter { year: 2027, quarter: 1 })
        );
        assert_eq!(
            q.advance_for_recurrence(&Recurrence::new(Frequency::Monthly, 6)),
            Some(Horizon::Quarter { year: 2027, quarter: 2 })
        );
        assert_eq!(q.advance_for_recurrence(&Recurrence::new(Frequency::Monthly, 2)), None);

        let y = Horizon::Year(2026);
        assert_eq!(
            y.advance_for_recurrence(&Recurrence::new(Frequency::Yearly, 1)),
            Some(Horizon::Year(2027))
        );
        assert_eq!(
            y.advance_for_recurrence(&Recurrence::new(Frequency::Monthly, 12)),
            Some(Horizon::Year(2027))
        );
        assert_eq!(
            y.advance_for_recurrence(&Recurrence::new(Frequency::Monthly, 24)),
            Some(Horizon::Year(2028))
        );
        assert_eq!(y.advance_for_recurrence(&Recurrence::new(Frequency::Monthly, 6)), None);

        assert_eq!(Horizon::Decade(2020).advance_for_recurrence(&Recurrence::new(Frequency::Yearly, 1)), None);
        assert_eq!(Horizon::Life.advance_for_recurrence(&Recurrence::new(Frequency::Yearly, 1)), None);
    }

    #[test]
    fn inbox_day_bucket_and_week_dated_partitions() {
        let today = d(2026, 11, 23); // Mon of 2026-W48
        let yesterday = d(2026, 11, 22);
        let wed = d(2026, 11, 25);
        let next_week_mon = d(2026, 11, 30);

        let visible_list = make_list("Work", false, true, None);
        let archived_list = make_list("Old", true, true, None);
        let non_smart_list = make_list("Hidden", false, false, None);
        let lists = vec![visible_list.clone(), archived_list.clone(), non_smart_list.clone()];

        let inbox_task = make_task("Inbox");
        assert!(is_horizons_inbox(&inbox_task));

        let mut horizon_task = make_task("Current week");
        horizon_task.horizon = Some(Horizon::current_week(today));
        assert!(!is_horizons_inbox(&horizon_task));

        let mut past_week_open = make_task("Past week open");
        past_week_open.horizon = Some(Horizon::Week { year: 2026, week: 47 });

        let mut past_week_done = make_task("Past week done");
        past_week_done.horizon = Some(Horizon::Week { year: 2026, week: 46 });
        past_week_done.completed_at = Some(100);

        let mut past_month_open = make_task("Past month open");
        past_month_open.horizon = Some(Horizon::Month { year: 2026, month: 10 });

        let mut current_week_done = make_task("Current week done");
        current_week_done.horizon = Some(Horizon::current_week(today));
        current_week_done.completed_at = Some(200);

        let mut archived_week = make_task("Archived week");
        archived_week.list_id = Some(archived_list.id);
        archived_week.horizon = Some(Horizon::current_week(today));

        let mut today_due = make_task("Today due");
        today_due.due_date = Some(today);
        today_due.list_id = Some(visible_list.id);

        let mut overdue_due = make_task("Overdue due");
        overdue_due.due_date = Some(yesterday);

        let mut overdue_start_only = make_task("Overdue start only");
        overdue_start_only.start_date = Some(yesterday);

        let mut wed_start_only = make_task("Wed start only");
        wed_start_only.start_date = Some(wed);

        let mut wed_done = make_task("Wed done");
        wed_done.due_date = Some(wed);
        wed_done.completed_at = Some(300);

        let mut hidden_today = make_task("Hidden today");
        hidden_today.due_date = Some(today);
        hidden_today.list_id = Some(non_smart_list.id);

        let mut next_week_due = make_task("Next week due");
        next_week_due.due_date = Some(next_week_mon);

        let tasks = vec![
            inbox_task,
            horizon_task,
            past_week_open,
            past_week_done,
            past_month_open,
            current_week_done,
            archived_week,
            today_due,
            overdue_due,
            overdue_start_only,
            wed_start_only,
            wed_done,
            hidden_today,
            next_week_due,
        ];

        // Day partition on today includes overdue open tasks and excludes hidden lists.
        let (today_items, today_overdue) = horizons_day_partition(&tasks, &lists, today, today, false);
        assert_eq!(today_items.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["Today due"]);
        assert_eq!(
            today_overdue.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec!["Overdue due", "Overdue start only"]
        );

        // Day partition on non-today never populates overdue; respects include_completed.
        let (wed_open, wed_overdue) = horizons_day_partition(&tasks, &lists, wed, today, false);
        assert_eq!(wed_open.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["Wed start only"]);
        assert!(wed_overdue.is_empty());
        let (wed_all, _) = horizons_day_partition(&tasks, &lists, wed, today, true);
        assert_eq!(
            wed_all.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec!["Wed start only", "Wed done"]
        );

        // Bucket partition on current week pulls past open weeks only (not past months, not completed, not archived).
        let (w_open, w_rollover) =
            horizons_bucket_partition(&tasks, &lists, Horizon::current_week(today), today, false);
        assert_eq!(w_open.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["Current week"]);
        assert_eq!(w_rollover.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["Past week open"]);

        let (w_all, _) =
            horizons_bucket_partition(&tasks, &lists, Horizon::current_week(today), today, true);
        assert_eq!(
            w_all.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec!["Current week", "Current week done"]
        );

        // Bucket partition on a non-current week (next week) never includes past rollover.
        let (next_w_items, next_w_rollover) = horizons_bucket_partition(
            &tasks,
            &lists,
            Horizon::current_week(today).shift(1),
            today,
            false,
        );
        assert!(next_w_items.is_empty());
        assert!(next_w_rollover.is_empty());

        // Week dated tasks helper respects week bounds and exclude_day.
        let dated_excl_today = horizons_week_dated_tasks(&tasks, &lists, 2026, 48, Some(today), false);
        assert_eq!(
            dated_excl_today.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec!["Wed start only"]
        );
        let dated_all = horizons_week_dated_tasks(&tasks, &lists, 2026, 48, None, true);
        assert_eq!(
            dated_all.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec!["Today due", "Wed start only", "Wed done"]
        );
    }
}
