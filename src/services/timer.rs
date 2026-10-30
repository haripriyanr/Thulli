//! Focus timer: a real countdown (and a planning count-up) driven by `tick`.
//!
//! The UI layer owns a `slint::Timer` that calls [`TimerService::tick`] a few
//! times per second. Elapsed time is measured from `Instant`s, so late ticks
//! never lose time, and every transition has an `_at` variant taking an
//! explicit `Instant` so tests do not need to sleep.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Whether the timer counts down to zero or up without limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerMode {
    /// Focus session with a planned length.
    Countdown,
    /// Open-ended, used to track planning time.
    CountUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerPhase {
    Idle,
    Running,
    Paused,
    Finished,
}

/// Point-in-time view of the timer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerSnapshot {
    pub mode: TimerMode,
    pub phase: TimerPhase,
    pub planned_secs: u64,
    pub elapsed_secs: u64,
    pub task_id: Option<String>,
}

impl TimerSnapshot {
    /// Seconds left in a countdown; zero for count-up.
    pub fn remaining_secs(&self) -> u64 {
        match self.mode {
            TimerMode::Countdown => self.planned_secs.saturating_sub(self.elapsed_secs),
            TimerMode::CountUp => 0,
        }
    }

    /// Seconds shown on the clock: remaining for countdown, elapsed for count-up.
    pub fn display_secs(&self) -> u64 {
        match self.mode {
            TimerMode::Countdown => self.remaining_secs(),
            TimerMode::CountUp => self.elapsed_secs,
        }
    }

    pub fn formatted(&self) -> String {
        format_mm_ss(self.display_secs())
    }
}

/// What a tick did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TickOutcome {
    /// Whole seconds counted by this tick; accrue these to the active task.
    pub advanced_secs: u64,
    /// True on the one tick where a countdown reached zero.
    pub finished: bool,
}

/// Formats seconds as MM:SS (minutes may exceed two digits).
pub fn format_mm_ss(total_secs: u64) -> String {
    format!("{:02}:{:02}", total_secs / 60, total_secs % 60)
}

struct State {
    mode: TimerMode,
    phase: TimerPhase,
    planned_secs: u64,
    elapsed_secs: u64,
    last_tick: Option<Instant>,
    task_id: Option<String>,
}

impl State {
    fn snapshot(&self) -> TimerSnapshot {
        TimerSnapshot {
            mode: self.mode,
            phase: self.phase,
            planned_secs: self.planned_secs,
            elapsed_secs: self.elapsed_secs,
            task_id: self.task_id.clone(),
        }
    }

    fn tick_at(&mut self, now: Instant) -> TickOutcome {
        if self.phase != TimerPhase::Running {
            return TickOutcome::default();
        }
        let Some(last) = self.last_tick else {
            self.last_tick = Some(now);
            return TickOutcome::default();
        };
        let whole = now.saturating_duration_since(last).as_secs();
        if whole == 0 {
            return TickOutcome::default();
        }
        self.last_tick = Some(last + Duration::from_secs(whole));

        let mut advanced = whole;
        let mut finished = false;
        if self.mode == TimerMode::Countdown {
            let left = self.planned_secs.saturating_sub(self.elapsed_secs);
            if advanced >= left {
                advanced = left;
                finished = true;
            }
        }
        self.elapsed_secs += advanced;
        if finished {
            self.phase = TimerPhase::Finished;
            self.last_tick = None;
        }
        TickOutcome { advanced_secs: advanced, finished }
    }
}

/// Thread-safe focus timer.
pub struct TimerService {
    default_secs: u64,
    state: Mutex<State>,
}

impl TimerService {
    /// Creates an idle countdown timer with a default length in minutes.
    pub fn new(minutes: u32) -> Self {
        let secs = u64::from(minutes) * 60;
        Self {
            default_secs: secs,
            state: Mutex::new(State {
                mode: TimerMode::Countdown,
                phase: TimerPhase::Idle,
                planned_secs: secs,
                elapsed_secs: 0,
                last_tick: None,
                task_id: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ---- starting ----

    pub fn start_countdown(&self, minutes: u32) {
        self.start_countdown_at(minutes, Instant::now());
    }

    pub fn start_countdown_at(&self, minutes: u32, now: Instant) {
        let mut s = self.lock();
        s.mode = TimerMode::Countdown;
        s.phase = TimerPhase::Running;
        s.planned_secs = u64::from(minutes) * 60;
        s.elapsed_secs = 0;
        s.last_tick = Some(now);
    }

    /// Starts counting up from zero (planning time).
    pub fn start_count_up(&self) {
        self.start_count_up_at(Instant::now());
    }

    pub fn start_count_up_at(&self, now: Instant) {
        let mut s = self.lock();
        s.mode = TimerMode::CountUp;
        s.phase = TimerPhase::Running;
        s.planned_secs = 0;
        s.elapsed_secs = 0;
        s.last_tick = Some(now);
    }

    // ---- pause / resume ----

    pub fn pause(&self) {
        self.pause_at(Instant::now());
    }

    /// Counts time up to `now`, then pauses. No-op unless running.
    pub fn pause_at(&self, now: Instant) {
        let mut s = self.lock();
        if s.phase == TimerPhase::Running {
            s.tick_at(now);
            if s.phase == TimerPhase::Running {
                s.phase = TimerPhase::Paused;
                s.last_tick = None;
            }
        }
    }

    pub fn resume(&self) {
        self.resume_at(Instant::now());
    }

    pub fn resume_at(&self, now: Instant) {
        let mut s = self.lock();
        if s.phase == TimerPhase::Paused {
            s.phase = TimerPhase::Running;
            s.last_tick = Some(now);
        }
    }

    /// Pauses a running timer or resumes a paused one. Returns true when now running.
    pub fn toggle(&self) -> bool {
        self.toggle_at(Instant::now())
    }

    pub fn toggle_at(&self, now: Instant) -> bool {
        match self.phase() {
            TimerPhase::Running => self.pause_at(now),
            TimerPhase::Paused => self.resume_at(now),
            _ => {}
        }
        self.is_running()
    }

    // ---- ticking ----

    /// Advances the timer to the present. Call this from the UI timer.
    pub fn tick(&self) -> TickOutcome {
        self.tick_at(Instant::now())
    }

    pub fn tick_at(&self, now: Instant) -> TickOutcome {
        self.lock().tick_at(now)
    }

    // ---- ending ----

    /// Ends the session early (running or paused). Returns the elapsed seconds,
    /// or `None` when there was nothing to finish.
    pub fn finish(&self) -> Option<u64> {
        self.finish_at(Instant::now())
    }

    pub fn finish_at(&self, now: Instant) -> Option<u64> {
        let mut s = self.lock();
        match s.phase {
            TimerPhase::Running | TimerPhase::Paused => {
                s.tick_at(now);
                s.phase = TimerPhase::Finished;
                s.last_tick = None;
                Some(s.elapsed_secs)
            }
            _ => None,
        }
    }

    /// Returns to idle with the default length and no attached task.
    pub fn reset(&self) {
        let mut s = self.lock();
        s.mode = TimerMode::Countdown;
        s.phase = TimerPhase::Idle;
        s.planned_secs = self.default_secs;
        s.elapsed_secs = 0;
        s.last_tick = None;
        s.task_id = None;
    }

    // ---- task attachment ----

    /// Chooses the task that elapsed time should accrue to.
    pub fn set_task(&self, task_id: Option<String>) {
        self.lock().task_id = task_id;
    }

    // ---- reading ----

    pub fn snapshot(&self) -> TimerSnapshot {
        self.lock().snapshot()
    }

    pub fn phase(&self) -> TimerPhase {
        self.lock().phase
    }

    pub fn is_running(&self) -> bool {
        self.phase() == TimerPhase::Running
    }

    /// Clock text (MM:SS) for the current state.
    pub fn formatted_display(&self) -> String {
        self.snapshot().formatted()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn countdown_counts_down_with_ticks() {
        let t0 = Instant::now();
        let timer = TimerService::new(25);
        assert_eq!(timer.formatted_display(), "25:00");
        timer.start_countdown_at(1, t0);
        assert_eq!(timer.formatted_display(), "01:00");

        let out = timer.tick_at(t0 + secs(1));
        assert_eq!(out.advanced_secs, 1);
        assert_eq!(timer.formatted_display(), "00:59");

        // A late tick catches up on all whole seconds.
        timer.tick_at(t0 + secs(11));
        assert_eq!(timer.formatted_display(), "00:49");
    }

    #[test]
    fn partial_seconds_carry_over() {
        let t0 = Instant::now();
        let timer = TimerService::new(1);
        timer.start_countdown_at(1, t0);
        assert_eq!(timer.tick_at(t0 + Duration::from_millis(900)).advanced_secs, 0);
        assert_eq!(timer.tick_at(t0 + Duration::from_millis(1100)).advanced_secs, 1);
        assert_eq!(timer.tick_at(t0 + Duration::from_millis(2050)).advanced_secs, 1);
        assert_eq!(timer.snapshot().elapsed_secs, 2);
    }

    #[test]
    fn pause_stops_time_and_resume_continues() {
        let t0 = Instant::now();
        let timer = TimerService::new(10);
        timer.start_countdown_at(10, t0);
        timer.pause_at(t0 + secs(30));
        assert_eq!(timer.phase(), TimerPhase::Paused);
        assert_eq!(timer.snapshot().elapsed_secs, 30);

        // Time passing while paused is not counted.
        assert_eq!(timer.tick_at(t0 + secs(300)).advanced_secs, 0);
        timer.resume_at(t0 + secs(300));
        timer.tick_at(t0 + secs(310));
        assert_eq!(timer.snapshot().elapsed_secs, 40);
        assert!(timer.is_running());
    }

    #[test]
    fn countdown_finishes_once_at_zero() {
        let t0 = Instant::now();
        let timer = TimerService::new(1);
        timer.start_countdown_at(1, t0);
        let out = timer.tick_at(t0 + secs(75));
        assert!(out.finished);
        assert_eq!(out.advanced_secs, 60, "never counts past the planned length");
        assert_eq!(timer.phase(), TimerPhase::Finished);
        assert_eq!(timer.formatted_display(), "00:00");
        assert_eq!(timer.tick_at(t0 + secs(200)), TickOutcome::default());
    }

    #[test]
    fn finish_early_reports_elapsed() {
        let t0 = Instant::now();
        let timer = TimerService::new(30);
        timer.start_countdown_at(30, t0);
        assert_eq!(timer.finish_at(t0 + secs(125)), Some(125));
        assert_eq!(timer.phase(), TimerPhase::Finished);
        assert_eq!(timer.finish_at(t0 + secs(200)), None);
    }

    #[test]
    fn reset_returns_to_idle_default() {
        let t0 = Instant::now();
        let timer = TimerService::new(25);
        timer.set_task(Some("t".into()));
        timer.start_countdown_at(5, t0);
        timer.tick_at(t0 + secs(20));
        timer.reset();
        let snap = timer.snapshot();
        assert_eq!(snap.phase, TimerPhase::Idle);
        assert_eq!(snap.elapsed_secs, 0);
        assert_eq!(timer.formatted_display(), "25:00");
        assert!(snap.task_id.is_none());
    }

    #[test]
    fn count_up_tracks_planning_time_without_limit() {
        let t0 = Instant::now();
        let timer = TimerService::new(25);
        timer.start_count_up_at(t0);
        let out = timer.tick_at(t0 + secs(3725));
        assert!(!out.finished);
        assert_eq!(timer.formatted_display(), "62:05");
        assert_eq!(timer.snapshot().mode, TimerMode::CountUp);
        assert_eq!(timer.finish_at(t0 + secs(3730)), Some(3730));
    }

    #[test]
    fn toggle_alternates_and_ignores_idle() {
        let t0 = Instant::now();
        let timer = TimerService::new(5);
        assert!(!timer.toggle_at(t0), "idle timer does not start by toggling");
        timer.start_countdown_at(5, t0);
        assert!(!timer.toggle_at(t0 + secs(1)));
        assert!(timer.toggle_at(t0 + secs(2)));
    }
}
