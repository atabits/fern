//! Focus (Pomodoro) sessions and their statistics.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Timelike};

use crate::model::Id;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusSession {
    pub id: Id,
    /// The task the session was spent on, if any.
    pub task_id: Option<Id>,
    pub title: String,
    /// Unix milliseconds.
    pub started_at: i64,
    pub ended_at: i64,
    /// Timed with the stopwatch rather than as a Pomodoro.
    pub stopwatch: bool,
    /// The record's note, shown under its title.
    pub note: String,
}

/// A timer of the Pomodoro list (the original's "Add Timer"): a name and icon, and how it times.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusTimer {
    pub id: Id,
    pub name: String,
    /// An emoji or short text; `None` shows the name's initial.
    pub icon: Option<String>,
    /// The Pomo length.
    pub minutes: i64,
    /// Timed with the stopwatch instead of a Pomo.
    pub stopwatch: bool,
    pub archived: bool,
    /// The icon's background color (`#rrggbb`); `None` is the accent color.
    pub color: Option<String>,
}

impl FocusTimer {
    /// A new timer as the original's dialog starts it: a 25-minute Pomo.
    pub fn new(name: &str) -> Self {
        Self { id: uuid::Uuid::now_v7(), name: name.to_owned(), icon: None, minutes: 25, stopwatch: false, archived: false, color: None }
    }

    /// How long a session of this timer runs: its own Pomo length, or `None` for a stopwatch,
    /// which counts up until stopped.
    pub fn length_ms(&self) -> Option<i64> {
        (!self.stopwatch).then(|| self.minutes.max(1) * 60_000)
    }
}

/// Batch Management's filters, as in the original.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecordFilter {
    /// 0 All, 1 Today, 2 Yesterday, 3 This Week, 4 Last Week, 5 This Month, 6 Last Month,
    /// 7 Custom (`days`).
    pub date: u8,
    /// The Custom date choice: from the first to the last day, both included.
    pub days: Option<(NaiveDate, NaiveDate)>,
    /// Shorter than this many minutes; `None` is all.
    pub under: Option<i64>,
    /// The Custom duration choice: from the least to the most minutes, both included. It
    /// replaces `under`.
    pub minutes: Option<(i64, i64)>,
    /// `Some(true)` only records linked to a task, `Some(false)` only unlinked ones.
    pub linked: Option<bool>,
}

impl RecordFilter {
    /// The original's limits for a Custom duration: "5~720 mins".
    pub const CUSTOM_MINUTES: (i64, i64) = (5, 720);

    /// Reads the Custom duration's Min and Max fields: whole minutes within the limits, the least
    /// first; an empty field is that end's limit.
    pub fn parse_minutes(least: &str, most: &str) -> Option<(i64, i64)> {
        let (low, high) = Self::CUSTOM_MINUTES;
        let read = |text: &str, empty: i64| match text.trim() {
            "" => Some(empty),
            text => text.parse::<i64>().ok().filter(|m| (low..=high).contains(m)),
        };
        let (least, most) = (read(least, low)?, read(most, high)?);
        (least <= most).then_some((least, most))
    }

    pub fn matches(&self, session: &FocusSession, today: NaiveDate, week_start: chrono::Weekday) -> bool {
        let day = session.start_local().date_naive();
        let week = today - Duration::days(((today.weekday().num_days_from_monday() + 7 - week_start.num_days_from_monday()) % 7) as i64);
        let month = today.with_day(1).expect("every month has a first day");
        let last_month = (month - Duration::days(1)).with_day(1).expect("every month has a first day");
        let date_ok = match self.date {
            1 => day == today,
            2 => day == today - Duration::days(1),
            3 => day >= week && day <= today,
            4 => day >= week - Duration::weeks(1) && day < week,
            5 => day >= month && day <= today,
            6 => day >= last_month && day < month,
            7 => self.days.is_none_or(|(first, last)| day >= first && day <= last),
            _ => true,
        };
        let length_ok = match self.minutes {
            Some((least, most)) => (least..=most).contains(&session.minutes()),
            None => self.under.is_none_or(|limit| session.minutes() < limit),
        };
        date_ok
            && length_ok
            && self.linked.is_none_or(|linked| session.task_id.is_some() == linked)
    }
}

impl FocusSession {
    pub fn minutes(&self) -> i64 {
        ((self.ended_at - self.started_at).max(0) + 30_000) / 60_000
    }

    pub fn start_local(&self) -> DateTime<Local> {
        local(self.started_at)
    }

    pub fn end_local(&self) -> DateTime<Local> {
        local(self.ended_at)
    }
}

fn local(ms: i64) -> DateTime<Local> {
    Local.timestamp_millis_opt(ms).single().unwrap_or_else(Local::now)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FocusStats {
    pub today_count: usize,
    pub today_minutes: i64,
    pub total_count: usize,
    pub total_minutes: i64,
}

/// Overview numbers; a session counts for the day it started on.
pub fn focus_stats(sessions: &[FocusSession], today: NaiveDate) -> FocusStats {
    let mut stats = FocusStats::default();
    for session in sessions {
        stats.total_count += 1;
        stats.total_minutes += session.minutes();
        if session.start_local().date_naive() == today {
            stats.today_count += 1;
            stats.today_minutes += session.minutes();
        }
    }
    stats
}

/// Focus minutes per day for `days` days from `start`, for the Trends chart and the Year Grid.
pub fn minutes_by_day(sessions: &[FocusSession], start: NaiveDate, days: usize) -> Vec<i64> {
    let mut out = vec![0; days];
    for session in sessions {
        let offset = (session.start_local().date_naive() - start).num_days();
        if (0..days as i64).contains(&offset) {
            out[offset as usize] += session.minutes();
        }
    }
    out
}

/// The Statistics' "Timeline": each session of the week starting on `first` as (day index 0–6,
/// start minute, end minute) within its day; a session over midnight is split in two.
pub fn timeline(sessions: &[FocusSession], first: NaiveDate) -> Vec<(usize, u32, u32)> {
    let minute = |t: DateTime<Local>| t.hour() * 60 + t.minute();
    let mut blocks = Vec::new();
    for session in sessions {
        let (start, end) = (session.start_local(), session.end_local());
        let mut day = start.date_naive();
        while day <= end.date_naive() {
            let from = if day == start.date_naive() { minute(start) } else { 0 };
            let to = if day == end.date_naive() { minute(end) } else { 24 * 60 };
            let index = (day - first).num_days();
            if (0..7).contains(&index) && to > from {
                blocks.push((index as usize, from, to));
            }
            day += Duration::days(1);
        }
    }
    blocks
}

/// What the Statistics' "Details" splits focus time by: the original's List, Tag or Task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusKey {
    /// The session's task's list (`None`: the Inbox, or a session without a task).
    List(Option<Id>),
    /// One of the task's tags (`None`: no tags, or no task); a session counts for each tag.
    Tag(Option<String>),
    /// The session's title, which is its task's title or what it was named after.
    Task(String),
}

/// Focused minutes between `first` and `last` per list, tag or task, most first. `by` picks the
/// kind: 0 list, 1 tag, 2 task.
pub fn focus_by(
    sessions: &[FocusSession],
    tasks: &[crate::model::Task],
    by: u8,
    first: NaiveDate,
    last: NaiveDate,
) -> Vec<(FocusKey, i64)> {
    let mut totals: Vec<(FocusKey, i64)> = Vec::new();
    for session in sessions {
        let day = session.start_local().date_naive();
        if day < first || day > last {
            continue;
        }
        let task = session.task_id.and_then(|id| tasks.iter().find(|t| t.id == id));
        let keys = match by {
            0 => vec![FocusKey::List(task.and_then(|t| t.list_id))],
            1 => match task.filter(|t| !t.tags.is_empty()) {
                Some(task) => task.tags.iter().map(|t| FocusKey::Tag(Some(t.clone()))).collect(),
                None => vec![FocusKey::Tag(None)],
            },
            _ => vec![FocusKey::Task(task.map_or_else(|| session.title.clone(), |t| t.title.clone()))],
        };
        for key in keys {
            match totals.iter_mut().find(|(k, _)| *k == key) {
                Some((_, minutes)) => *minutes += session.minutes(),
                None => totals.push((key, session.minutes())),
            }
        }
    }
    totals.sort_by_key(|(_, minutes)| std::cmp::Reverse(*minutes));
    totals
}

/// "Most Focused Time": minutes per hour of day in the month containing `month`, split across
/// the hours each session covers.
pub fn minutes_by_hour(sessions: &[FocusSession], month: NaiveDate) -> [i64; 24] {
    let mut out = [0; 24];
    for session in sessions {
        let start = session.start_local();
        if start.year() != month.year() || start.month() != month.month() {
            continue;
        }
        let mut at = start;
        let end = session.end_local();
        while at < end {
            let next_hour = (at + Duration::hours(1)).with_minute(0).and_then(|t| t.with_second(0)).unwrap_or(end);
            let until = next_hour.min(end);
            out[at.hour() as usize] += (until - at).num_minutes();
            at = until;
        }
    }
    out
}

/// Today's Pomodoros and minutes minus yesterday's, for "N from yesterday".
pub fn change_from_yesterday(sessions: &[FocusSession], today: NaiveDate) -> (i64, i64) {
    let day = |date: NaiveDate| {
        let stats = focus_stats(sessions, date);
        (stats.today_count as i64, stats.today_minutes)
    };
    let (count, minutes) = day(today);
    let (before_count, before_minutes) = day(today - Duration::days(1));
    (count - before_count, minutes - before_minutes)
}

/// "2h30m", "45m", "0m".
pub fn format_minutes(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m}m"),
    }
}

/// The Focus Record as CSV, oldest first: start and end in local time, minutes and title.
pub fn export_csv(sessions: &[FocusSession]) -> String {
    let mut sorted: Vec<&FocusSession> = sessions.iter().collect();
    sorted.sort_by_key(|s| s.started_at);
    let mut out = String::from("start,end,minutes,title\n");
    for s in sorted {
        let title = if s.title.contains([',', '"', '\n']) {
            format!("\"{}\"", s.title.replace('"', "\"\""))
        } else {
            s.title.clone()
        };
        out.push_str(&format!(
            "{},{},{},{title}\n",
            s.start_local().format("%Y-%m-%d %H:%M"),
            s.end_local().format("%Y-%m-%d %H:%M"),
            s.minutes()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn session(start: DateTime<Local>, minutes: i64) -> FocusSession {
        let started_at = start.timestamp_millis();
        FocusSession {
            id: Uuid::now_v7(),
            task_id: None,
            title: "x".into(),
            started_at,
            ended_at: started_at + minutes * 60_000,
            stopwatch: false,
            note: String::new(),
        }
    }

    #[test]
    fn stats_split_today_and_total() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let at = |d: u32, h: u32| Local.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap();
        let sessions = [session(at(18, 9), 25), session(at(18, 14), 50), session(at(17, 9), 25)];
        assert_eq!(
            focus_stats(&sessions, today),
            FocusStats { today_count: 2, today_minutes: 75, total_count: 3, total_minutes: 100 }
        );
    }

    #[test]
    fn csv_export_is_oldest_first() {
        let at = |h: u32| Local.with_ymd_and_hms(2026, 10, 18, h, 0, 0).unwrap();
        let mut late = session(at(14), 50);
        late.title = "Write, then edit".into();
        let csv = export_csv(&[late, session(at(9), 25)]);
        assert_eq!(
            csv,
            "start,end,minutes,title\n2026-10-18 09:00,2026-10-18 09:25,25,x\n\
             2026-10-18 14:00,2026-10-18 14:50,50,\"Write, then edit\"\n"
        );
    }

    #[test]
    fn charts() {
        let at = |d: u32, h: u32, m: u32| Local.with_ymd_and_hms(2026, 10, d, h, m, 0).unwrap();
        let sessions = [session(at(18, 9, 30), 60), session(at(17, 14, 0), 25), session(at(12, 9, 0), 25)];
        let day = |d| NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
        assert_eq!(minutes_by_day(&sessions, day(12), 7), [25, 0, 0, 0, 0, 25, 60]);
        let hours = minutes_by_hour(&sessions, day(1));
        assert_eq!((hours[9], hours[10], hours[14]), (25 + 30, 30, 25));
        assert_eq!(change_from_yesterday(&sessions, day(18)), (0, 35));
    }

    #[test]
    fn minute_formatting() {
        assert_eq!(format_minutes(150), "2h30m");
        assert_eq!(format_minutes(45), "45m");
        assert_eq!(format_minutes(120), "2h");
        assert_eq!(format_minutes(0), "0m");
    }

    #[test]
    fn focus_split_by_list_tag_or_task() {
        let store = crate::Store::open_in_memory().unwrap();
        let work = store.create_list("Work").unwrap();
        let report = store.add_task("report").unwrap();
        store.set_list(report.id, Some(work.id)).unwrap();
        store.set_tags(report.id, &["deep".to_owned(), "office".to_owned()]).unwrap();
        let now = Local::now().timestamp_millis();
        store.add_focus_session(Some(report.id), "report", now - 50 * 60_000, now - 25 * 60_000).unwrap();
        store.add_focus_session(None, "Reading", now - 20 * 60_000, now - 10 * 60_000).unwrap();
        let (sessions, tasks) = (store.focus_sessions().unwrap(), store.tasks().unwrap());
        let today = Local::now().date_naive();

        assert_eq!(focus_by(&sessions, &tasks, 0, today, today), [(FocusKey::List(Some(work.id)), 25), (FocusKey::List(None), 10)]);
        let by_tag = focus_by(&sessions, &tasks, 1, today, today);
        assert_eq!(by_tag.len(), 3);
        assert!(by_tag.contains(&(FocusKey::Tag(Some("deep".into())), 25)) && by_tag.contains(&(FocusKey::Tag(None), 10)));
        assert_eq!(focus_by(&sessions, &tasks, 2, today, today)[1], (FocusKey::Task("Reading".into()), 10));
    }

    #[test]
    fn timeline_blocks() {
        let at = |d: u32, h: u32, m: u32| {
            Local.from_local_datetime(&NaiveDate::from_ymd_opt(2026, 10, d).unwrap().and_hms_opt(h, m, 0).unwrap()).unwrap().timestamp_millis()
        };
        let session = |start: i64, end: i64| FocusSession { id: uuid::Uuid::now_v7(), task_id: None, title: "x".into(), started_at: start, ended_at: end, stopwatch: false, note: String::new() };
        let monday = NaiveDate::from_ymd_opt(2026, 10, 12).unwrap();
        let sessions = [session(at(15, 13, 30), at(15, 16, 0)), session(at(13, 23, 0), at(14, 1, 0)), session(at(20, 9, 0), at(20, 10, 0))];
        assert_eq!(timeline(&sessions, monday), [(3, 810, 960), (1, 1380, 1440), (2, 0, 60)]);
    }

    #[test]
    fn timer_length() {
        let mut timer = FocusTimer::new("Reading");
        timer.minutes = 45;
        assert_eq!(timer.length_ms(), Some(45 * 60_000), "a timer runs its own Pomo length");
        timer.stopwatch = true;
        assert_eq!(timer.length_ms(), None);
    }

    #[test]
    fn batch_filters() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let at = |day: NaiveDate, minutes: i64, task: bool| {
            let start = Local.from_local_datetime(&day.and_hms_opt(10, 0, 0).unwrap()).unwrap().timestamp_millis();
            FocusSession {
                id: uuid::Uuid::now_v7(),
                task_id: task.then(uuid::Uuid::now_v7),
                title: "x".into(),
                started_at: start,
                ended_at: start + minutes * 60_000,
                stopwatch: false,
                note: String::new(),
            }
        };
        let monday = chrono::Weekday::Mon;
        let short_today = at(today, 8, true);
        let long_yesterday = at(today - Duration::days(1), 50, false);
        let last_month = at(NaiveDate::from_ymd_opt(2026, 9, 20).unwrap(), 30, false);
        let all = RecordFilter::default();
        assert!(all.matches(&last_month, today, monday));
        let today_only = RecordFilter { date: 1, ..all };
        assert!(today_only.matches(&short_today, today, monday) && !today_only.matches(&long_yesterday, today, monday));
        assert!(RecordFilter { date: 2, ..all }.matches(&long_yesterday, today, monday));
        assert!(RecordFilter { date: 3, ..all }.matches(&long_yesterday, today, monday), "Saturday is in the week of Sunday the 18th");
        assert!(RecordFilter { date: 6, ..all }.matches(&last_month, today, monday));
        assert!(!RecordFilter { date: 5, ..all }.matches(&last_month, today, monday));
        let short = RecordFilter { under: Some(10), ..all };
        assert!(short.matches(&short_today, today, monday) && !short.matches(&long_yesterday, today, monday));
        let linked = RecordFilter { linked: Some(true), ..all };
        assert!(linked.matches(&short_today, today, monday) && !linked.matches(&long_yesterday, today, monday));

        // Custom choices.
        let september = RecordFilter { date: 7, days: Some((NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(), NaiveDate::from_ymd_opt(2026, 9, 30).unwrap())), ..all };
        assert!(september.matches(&last_month, today, monday) && !september.matches(&short_today, today, monday));
        let half_hour = RecordFilter { under: Some(10), minutes: Some((25, 50)), ..all };
        assert!(half_hour.matches(&last_month, today, monday) && half_hour.matches(&long_yesterday, today, monday));
        assert!(!half_hour.matches(&short_today, today, monday), "a custom range replaces Less Than");
        assert_eq!(RecordFilter::parse_minutes("20", "40"), Some((20, 40)));
        assert_eq!(RecordFilter::parse_minutes("", " 90 "), Some((5, 90)));
        assert_eq!(RecordFilter::parse_minutes("30", ""), Some((30, 720)));
        assert_eq!(RecordFilter::parse_minutes("40", "20"), None);
        assert_eq!(RecordFilter::parse_minutes("1", "20"), None, "below the original's 5 minutes");
        assert_eq!(RecordFilter::parse_minutes("x", "20"), None);
    }
}
