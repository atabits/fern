//! The Pomodoro countdown, as plain state over millisecond timestamps so it can be tested.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusTimer {
    pub length_ms: i64,
    pub started_at: i64,
    paused_at: Option<i64>,
    paused_total: i64,
}

impl FocusTimer {
    pub fn start(now: i64, length_ms: i64) -> Self {
        Self { length_ms, started_at: now, paused_at: None, paused_total: 0 }
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn pause(&mut self, now: i64) {
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
    }

    pub fn resume(&mut self, now: i64) {
        if let Some(paused_at) = self.paused_at.take() {
            self.paused_total += now - paused_at;
        }
    }

    /// Focused time so far, excluding pauses.
    pub fn elapsed(&self, now: i64) -> i64 {
        let end = self.paused_at.unwrap_or(now);
        (end - self.started_at - self.paused_total).clamp(0, self.length_ms)
    }

    pub fn remaining(&self, now: i64) -> i64 {
        self.length_ms - self.elapsed(now)
    }

    pub fn finished(&self, now: i64) -> bool {
        self.remaining(now) == 0
    }
}

/// The break after a finished Pomodoro.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakKind {
    Short,
    Long,
}

/// Pomodoros between long breaks.
pub const POMOS_PER_LONG_BREAK: usize = 4;

/// The break after the `pomos_today`-th Pomodoro of the day: every fourth gets a long one.
pub fn break_after(pomos_today: usize) -> BreakKind {
    if pomos_today > 0 && pomos_today % POMOS_PER_LONG_BREAK == 0 { BreakKind::Long } else { BreakKind::Short }
}

/// "25:00", "04:05".
pub fn format_clock(ms: i64) -> String {
    let seconds = (ms.max(0) + 999) / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    #[test]
    fn every_fourth_pomo_gets_a_long_break() {
        let kinds: Vec<BreakKind> = (1..=8).map(break_after).collect();
        assert_eq!(kinds.iter().filter(|k| **k == BreakKind::Long).count(), 2);
        assert_eq!((kinds[3], kinds[7], kinds[0]), (BreakKind::Long, BreakKind::Long, BreakKind::Short));
    }

    #[test]
    fn counts_down_and_excludes_pauses() {
        let mut timer = FocusTimer::start(0, 25 * MIN);
        assert_eq!(timer.remaining(10 * MIN), 15 * MIN);
        timer.pause(10 * MIN);
        assert_eq!(timer.remaining(20 * MIN), 15 * MIN);
        timer.resume(20 * MIN);
        assert_eq!(timer.remaining(25 * MIN), 10 * MIN);
        assert!(!timer.finished(34 * MIN));
        assert!(timer.finished(35 * MIN));
        assert_eq!(timer.elapsed(99 * MIN), 25 * MIN);
    }

    #[test]
    fn clock_format() {
        assert_eq!(format_clock(25 * MIN), "25:00");
        assert_eq!(format_clock(4 * MIN + 4_001), "04:05");
        assert_eq!(format_clock(0), "00:00");
    }
}
