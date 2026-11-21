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

    /// A stopwatch: counts up with no end, until it is stopped.
    pub fn stopwatch(now: i64) -> Self {
        Self::start(now, STOPWATCH_LENGTH)
    }

    pub fn is_stopwatch(&self) -> bool {
        self.length_ms == STOPWATCH_LENGTH
    }

    /// What the clock shows: time left on a Pomodoro, time so far on a stopwatch.
    pub fn display(&self, now: i64) -> i64 {
        if self.is_stopwatch() { self.elapsed(now) } else { self.remaining(now) }
    }

    /// Formats the timer for display: countdown rounds partial seconds up so `00:00` only
    /// appears when finished, while a stopwatch floors elapsed seconds so `t = 1ms` shows `00:00`.
    pub fn format_display(&self, now: i64) -> String {
        if self.is_stopwatch() {
            format_stopwatch_clock(self.elapsed(now))
        } else {
            format_clock(self.remaining(now))
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn pause(&mut self, now: i64) {
        if self.paused_at.is_none() {
            // Guard against backward wall-clock jumps so pausing before `started_at + paused_total`
            // cannot later inflate `paused_total` on resume.
            let floor = self.started_at.saturating_add(self.paused_total);
            self.paused_at = Some(now.max(floor));
        }
    }

    pub fn resume(&mut self, now: i64) {
        if let Some(paused_at) = self.paused_at.take() {
            // Guard against backward `SystemTime` jumps while paused so `paused_total` never shrinks.
            let delta = now.saturating_sub(paused_at).max(0);
            self.paused_total = self.paused_total.saturating_add(delta);
        }
    }

    /// Focused time so far, excluding pauses.
    pub fn elapsed(&self, now: i64) -> i64 {
        let end = self.paused_at.unwrap_or(now);
        end.saturating_sub(self.started_at)
            .saturating_sub(self.paused_total)
            .clamp(0, self.length_ms)
    }

    pub fn remaining(&self, now: i64) -> i64 {
        self.length_ms.saturating_sub(self.elapsed(now))
    }

    pub fn finished(&self, now: i64) -> bool {
        self.remaining(now) == 0
    }

    /// Scheduled completion, including pauses, even when a tick arrives late.
    /// FocusTimer's completion callback, RVA 0x005af1dc.
    pub fn completed_at(&self) -> i64 {
        self.started_at
            .saturating_add(self.paused_total)
            .saturating_add(self.length_ms)
    }
}

/// A stopwatch's length: far beyond any session, so it never finishes.
const STOPWATCH_LENGTH: i64 = i64::MAX / 4;

/// The break after a finished Pomodoro.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakKind {
    Short,
    Long,
}

/// FocusConfig's runtime counter, independent of saved records and calendar dates.
/// StartFocus/GetRelaxMinutes/Reset: RVAs 0x000c8fb0/0x000c9134/0x000c92e0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FocusCycle {
    count: usize,
    last_completed_at: Option<i64>,
}

pub const DEFAULT_LONG_BREAK_EVERY: usize = 4;
const CYCLE_IDLE_MS: i64 = 60 * 60_000;
/// Constants::get_MinFocusDuration, RVA 0x002556af, with HasSyncedShortFocus=false.
pub const MIN_FOCUS_MS: i64 = 5 * 60_000;

/// FocusConfig::SetStartSeconds, RVA 0x000c91c0 (normal local capability).
pub fn pomo_length_ms(minutes: i64) -> i64 {
    if minutes == 0 { 25 * 60_000 } else { minutes.max(MIN_FOCUS_MS / 60_000) * 60_000 }
}

impl FocusCycle {
    fn idle_too_long(&self, now: i64) -> bool {
        self.last_completed_at.is_none_or(|last| now.saturating_sub(last) > CYCLE_IDLE_MS)
    }

    pub fn start(&mut self, now: i64, stopwatch: bool, interval: usize) {
        if stopwatch || self.idle_too_long(now) || (interval > 0 && self.count > 0 && self.count.is_multiple_of(interval)) {
            self.count = 0;
        }
        self.count += 1;
    }

    pub fn complete(&mut self, completed_at: i64) {
        self.last_completed_at = Some(completed_at);
    }

    pub fn break_kind(&self, now: i64, interval: usize) -> BreakKind {
        if self.idle_too_long(now) || (interval > 0 && self.count > 0 && !self.count.is_multiple_of(interval)) {
            BreakKind::Short
        } else {
            BreakKind::Long
        }
    }

    /// Reset clears the count but retains LastPomoCompleteTime in the original.
    pub fn reset(&mut self) {
        self.count = 0;
    }
}

/// Formats countdown milliseconds as `"25:00"` / `"04:05"`, rounding partial seconds up so
/// `"00:00"` only appears once remaining time reaches zero.
pub fn format_clock(ms: i64) -> String {
    let seconds = ms.max(0).saturating_add(999) / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// Formats elapsed stopwatch milliseconds as `"00:00"` / `"04:04"`, flooring partial seconds
/// so count-up display never jumps ahead by +1s at `t = 1ms`.
pub fn format_stopwatch_clock(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    #[test]
    fn stopwatch_counts_up_and_never_finishes() {
        let mut watch = FocusTimer::stopwatch(0);
        assert!(watch.is_stopwatch());
        assert_eq!(watch.display(90 * MIN), 90 * MIN);
        assert!(!watch.finished(24 * 60 * MIN));
        watch.pause(10 * MIN);
        watch.resume(15 * MIN);
        assert_eq!(watch.display(20 * MIN), 15 * MIN);
        let pomo = FocusTimer::start(0, 25 * MIN);
        assert_eq!(pomo.display(5 * MIN), 20 * MIN);
    }

    #[test]
    fn cycle_uses_runtime_starts_and_completions() {
        let mut cycle = FocusCycle::default();
        for n in 1..=8 {
            let start = (n - 1) * 30 * MIN;
            cycle.start(start, false, DEFAULT_LONG_BREAK_EVERY);
            cycle.complete(start + 25 * MIN);
            assert_eq!(cycle.break_kind(start + 25 * MIN, DEFAULT_LONG_BREAK_EVERY), if n % 4 == 0 { BreakKind::Long } else { BreakKind::Short });
        }
    }

    #[test]
    fn cycle_resets_after_strictly_more_than_an_hour() {
        let mut cycle = FocusCycle { count: 3, last_completed_at: Some(0) };
        let mut late = cycle;
        cycle.start(60 * MIN, false, 4);
        cycle.complete(85 * MIN);
        assert_eq!(cycle.break_kind(85 * MIN, 4), BreakKind::Long);
        late.start(60 * MIN + 1, false, 4);
        late.complete(85 * MIN + 1);
        assert_eq!(late.break_kind(85 * MIN + 1, 4), BreakKind::Short);
        assert_eq!(cycle.break_kind(145 * MIN, 4), BreakKind::Long);
        assert_eq!(cycle.break_kind(145 * MIN + 1, 4), BreakKind::Short);
    }

    #[test]
    fn reset_and_stopwatch_do_not_use_saved_session_totals() {
        let mut cycle = FocusCycle { count: 3, last_completed_at: Some(0) };
        cycle.reset();
        cycle.start(MIN, false, 4);
        assert_eq!(cycle.count, 1);
        cycle.count = 3;
        cycle.start(2 * MIN, true, 4);
        assert_eq!(cycle.count, 1);
        cycle.start(3 * MIN, false, 4);
        assert_eq!(cycle.count, 2);
    }

    #[test]
    fn cycle_accepts_configured_intervals() {
        let mut cycle = FocusCycle::default();
        for n in 1..=3 {
            cycle.start(n * MIN, false, 3);
            cycle.complete(n * MIN);
        }
        assert_eq!(cycle.break_kind(3 * MIN, 3), BreakKind::Long);
        cycle.start(4 * MIN, false, 3);
        assert_eq!(cycle.count, 1);
    }

    #[test]
    fn countdown_uses_normal_minimum_and_zero_fallback() {
        assert_eq!(pomo_length_ms(0), 25 * MIN);
        assert_eq!(pomo_length_ms(1), 5 * MIN);
        assert_eq!(pomo_length_ms(-1), 5 * MIN);
        assert_eq!(pomo_length_ms(40), 40 * MIN);
        assert_eq!(MIN_FOCUS_MS, 300_000);
    }

    #[test]
    fn late_tick_keeps_scheduled_completion_including_pauses() {
        let mut timer = FocusTimer::start(10 * MIN, 25 * MIN);
        timer.pause(20 * MIN);
        timer.resume(25 * MIN);
        assert!(timer.finished(100 * MIN));
        assert_eq!(timer.completed_at(), 40 * MIN);
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
        assert_eq!(format_clock(-5_000), "00:00");
    }

    #[test]
    fn stopwatch_clock_format_floors_without_rounding_up() {
        assert_eq!(format_stopwatch_clock(0), "00:00");
        assert_eq!(format_stopwatch_clock(1), "00:00");
        assert_eq!(format_stopwatch_clock(999), "00:00");
        assert_eq!(format_stopwatch_clock(1_000), "00:01");
        assert_eq!(format_stopwatch_clock(4 * MIN + 4_999), "04:04");
        assert_eq!(format_stopwatch_clock(-5_000), "00:00");

        let watch = FocusTimer::stopwatch(10_000);
        assert_eq!(watch.format_display(10_001), "00:00");
        assert_eq!(watch.format_display(11_500), "00:01");

        let pomo = FocusTimer::start(10_000, 25 * MIN);
        assert_eq!(pomo.format_display(10_001), "25:00");
        assert_eq!(pomo.format_display(11_000), "24:59");
    }

    #[test]
    fn backward_clock_jump_never_corrupts_pause_or_elapsed() {
        // 1. Backward jump while paused must not make `paused_total` negative or inflate elapsed.
        let mut timer = FocusTimer::start(100 * MIN, 25 * MIN);
        timer.pause(110 * MIN);
        assert_eq!(timer.elapsed(110 * MIN), 10 * MIN);
        // System clock jumps backward by 5 minutes before resume.
        timer.resume(105 * MIN);
        assert_eq!(timer.paused_total, 0);
        // Once clock catches back up, elapsed is never inflated by a negative pause total.
        assert_eq!(timer.elapsed(110 * MIN), 10 * MIN);
        assert_eq!(timer.remaining(110 * MIN), 15 * MIN);

        // 2. Backward jump while running never yields negative elapsed or remaining > length.
        let running = FocusTimer::start(100 * MIN, 25 * MIN);
        assert_eq!(running.elapsed(90 * MIN), 0);
        assert_eq!(running.remaining(90 * MIN), 25 * MIN);

        // 3. Backward jump before pause floors `paused_at` at `started_at + paused_total`,
        // preventing an inflated pause delta when the clock jumps forward before resume.
        let mut early_pause = FocusTimer::start(100 * MIN, 25 * MIN);
        early_pause.pause(95 * MIN);
        assert_eq!(early_pause.elapsed(95 * MIN), 0);
        early_pause.resume(102 * MIN);
        assert_eq!(early_pause.paused_total, 2 * MIN);
        assert_eq!(early_pause.elapsed(105 * MIN), 3 * MIN);
    }
}
