//! Focus (Pomodoro) sessions and their statistics.

use chrono::{DateTime, Local, NaiveDate, TimeZone};

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

/// "2h30m", "45m", "0m".
pub fn format_minutes(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m}m"),
    }
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
    fn minute_formatting() {
        assert_eq!(format_minutes(150), "2h30m");
        assert_eq!(format_minutes(45), "45m");
        assert_eq!(format_minutes(120), "2h");
        assert_eq!(format_minutes(0), "0m");
    }
}
