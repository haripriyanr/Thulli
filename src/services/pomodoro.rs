//! Pomodoro cycle: focus rounds, short breaks and a long break after a set
//! number of focus rounds.
//!
//! The timer counts whole seconds. `tick()` is called once per second by the
//! shell, so no wall-clock drift can accumulate and pausing keeps the exact
//! remaining time.

use super::timer::format_mm_ss;
use crate::models::Preferences;

/// The three phases of a pomodoro cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PomodoroPhase {
    Focus,
    ShortBreak,
    LongBreak,
}

impl PomodoroPhase {
    /// Stable identifier used at the UI boundary.
    pub fn as_str(self) -> &'static str {
        match self {
            PomodoroPhase::Focus => "focus",
            PomodoroPhase::ShortBreak => "short_break",
            PomodoroPhase::LongBreak => "long_break",
        }
    }

    /// Parses the identifier produced by [`PomodoroPhase::as_str`].
    pub fn from_str_id(id: &str) -> Option<Self> {
        match id {
            "focus" => Some(PomodoroPhase::Focus),
            "short_break" => Some(PomodoroPhase::ShortBreak),
            "long_break" => Some(PomodoroPhase::LongBreak),
            _ => None,
        }
    }
}

/// Durations and cycle rules for a pomodoro timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PomodoroSettings {
    pub focus_secs: u64,
    pub short_break_secs: u64,
    pub long_break_secs: u64,
    /// Focus rounds completed before a long break is due.
    pub rounds_before_long_break: u32,
    /// Start the next phase on its own when a phase finishes.
    pub auto_advance: bool,
    /// Take breaks between focus rounds. Off goes from focus to focus.
    pub breaks: bool,
}

impl PomodoroSettings {
    /// Builds the cycle rules from the saved preferences.
    pub fn from_preferences(prefs: &Preferences) -> Self {
        Self {
            focus_secs: u64::from(prefs.default_session_mins) * 60,
            short_break_secs: u64::from(prefs.default_break_mins) * 60,
            long_break_secs: u64::from(prefs.pomodoro_long_break_mins) * 60,
            rounds_before_long_break: prefs.pomodoro_rounds,
            auto_advance: prefs.pomodoro_auto_advance,
            breaks: prefs.pomodoro_breaks,
        }
    }
}

impl Default for PomodoroSettings {
    fn default() -> Self {
        Self {
            focus_secs: 50 * 60,
            short_break_secs: 10 * 60,
            long_break_secs: 20 * 60,
            rounds_before_long_break: 4,
            auto_advance: false,
            breaks: true,
        }
    }
}

/// Pomodoro state machine. Pure logic: no clocks, threads or UI types.
#[derive(Debug, Clone)]
pub struct PomodoroTimer {
    settings: PomodoroSettings,
    phase: PomodoroPhase,
    remaining_secs: u64,
    running: bool,
    /// Focus rounds finished in the current cycle.
    completed_rounds: u32,
    /// Set when a phase runs out; cleared by the next action on the timer.
    finished: bool,
}

impl Default for PomodoroTimer {
    fn default() -> Self {
        Self::new(PomodoroSettings::default())
    }
}

impl PomodoroTimer {
    pub fn new(settings: PomodoroSettings) -> Self {
        let remaining_secs = settings.focus_secs;
        Self {
            settings,
            phase: PomodoroPhase::Focus,
            remaining_secs,
            running: false,
            completed_rounds: 0,
            finished: false,
        }
    }

    /// True once a phase has run out and nothing has been started since.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Goes back to the focus round: from a break this selects the focus phase,
    /// and from a focus round it restarts that round at full length.
    pub fn previous(&mut self) {
        self.select(PomodoroPhase::Focus);
    }

    pub fn phase(&self) -> PomodoroPhase {
        self.phase
    }

    pub fn remaining_secs(&self) -> u64 {
        self.remaining_secs
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn completed_rounds(&self) -> u32 {
        self.completed_rounds
    }

    pub fn rounds_before_long_break(&self) -> u32 {
        self.settings.rounds_before_long_break.max(1)
    }

    /// Full length of the current phase in seconds.
    pub fn phase_total_secs(&self) -> u64 {
        Self::duration_of(&self.settings, self.phase)
    }

    /// Fraction of the current phase that has elapsed, from 0.0 to 1.0.
    pub fn progress(&self) -> f32 {
        let total = self.phase_total_secs();
        if total == 0 {
            return 0.0;
        }
        let elapsed = total.saturating_sub(self.remaining_secs);
        (elapsed as f32 / total as f32).clamp(0.0, 1.0)
    }

    /// Round shown to the user: the round in progress during focus, and the
    /// round just finished during a break.
    pub fn round_number(&self) -> u32 {
        let rounds = self.rounds_before_long_break();
        match self.phase {
            PomodoroPhase::Focus => (self.completed_rounds + 1).min(rounds),
            _ => self.completed_rounds.clamp(1, rounds),
        }
    }

    /// Remaining time as `MM:SS`.
    pub fn display(&self) -> String {
        format_mm_ss(self.remaining_secs)
    }

    /// Seconds spent in the current phase so far.
    pub fn elapsed_in_phase(&self) -> u64 {
        self.phase_total_secs().saturating_sub(self.remaining_secs)
    }

    /// Applies new durations and rules. The phase and round stay as they are.
    /// Its remaining time is re-based only if it has not started yet, or if
    /// the new length is shorter than what is left.
    pub fn configure(&mut self, settings: PomodoroSettings) {
        let untouched = self.remaining_secs == self.phase_total_secs();
        self.settings = settings;
        if untouched || self.remaining_secs > self.phase_total_secs() {
            self.remaining_secs = self.phase_total_secs();
        }
    }

    /// Starts or resumes the countdown. Does nothing once the phase has ended.
    pub fn start(&mut self) {
        if self.remaining_secs > 0 {
            self.running = true;
            self.finished = false;
        }
    }

    pub fn pause(&mut self) {
        self.running = false;
    }

    /// Flips between running and paused. Returns the new running state.
    pub fn toggle(&mut self) -> bool {
        if self.running {
            self.pause();
        } else {
            self.start();
        }
        self.running
    }

    /// Restores the current phase to its full length and stops it.
    /// Completed rounds are kept.
    pub fn reset(&mut self) {
        self.running = false;
        self.finished = false;
        self.remaining_secs = self.phase_total_secs();
    }

    /// Jumps to a phase at its full length and stops the timer.
    pub fn select(&mut self, phase: PomodoroPhase) {
        self.phase = phase;
        self.running = false;
        self.finished = false;
        self.remaining_secs = self.phase_total_secs();
    }

    /// Ends the current phase early and moves to the next one. A skipped
    /// focus round is not counted. The running state carries over, so
    /// skipping while running keeps the clock going into the next phase.
    pub fn next(&mut self) {
        self.finished = false;
        self.advance();
    }

    /// Advances the clock by one second. Returns the phase that finished
    /// during this tick, if any. Ticks while paused are ignored.
    pub fn tick(&mut self) -> Option<PomodoroPhase> {
        if !self.running {
            return None;
        }
        self.remaining_secs = self.remaining_secs.saturating_sub(1);
        if self.remaining_secs > 0 {
            return None;
        }

        let finished = self.phase;
        if finished == PomodoroPhase::Focus {
            self.completed_rounds += 1;
        }
        self.advance();
        self.running = self.settings.auto_advance;
        self.finished = true;
        Some(finished)
    }

    fn advance(&mut self) {
        let next = match self.phase {
            PomodoroPhase::Focus if !self.settings.breaks => PomodoroPhase::Focus,
            PomodoroPhase::Focus => {
                let rounds = self.rounds_before_long_break();
                if self.completed_rounds > 0 && self.completed_rounds % rounds == 0 {
                    PomodoroPhase::LongBreak
                } else {
                    PomodoroPhase::ShortBreak
                }
            }
            PomodoroPhase::ShortBreak => PomodoroPhase::Focus,
            PomodoroPhase::LongBreak => {
                self.completed_rounds = 0;
                PomodoroPhase::Focus
            }
        };
        self.phase = next;
        self.remaining_secs = self.phase_total_secs();
    }

    fn duration_of(settings: &PomodoroSettings, phase: PomodoroPhase) -> u64 {
        match phase {
            PomodoroPhase::Focus => settings.focus_secs,
            PomodoroPhase::ShortBreak => settings.short_break_secs,
            PomodoroPhase::LongBreak => settings.long_break_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quick() -> PomodoroTimer {
        PomodoroTimer::new(PomodoroSettings {
            focus_secs: 3,
            short_break_secs: 2,
            long_break_secs: 4,
            rounds_before_long_break: 2,
            auto_advance: false,
            breaks: true,
        })
    }

    fn tick_n(timer: &mut PomodoroTimer, n: u32) {
        for _ in 0..n {
            timer.tick();
        }
    }

    #[test]
    fn starts_stopped_in_focus_with_full_duration() {
        let timer = PomodoroTimer::default();
        assert_eq!(timer.phase(), PomodoroPhase::Focus);
        assert!(!timer.is_running());
        assert_eq!(timer.remaining_secs(), 50 * 60);
        assert_eq!(timer.display(), "50:00");
        assert_eq!(timer.progress(), 0.0);
    }

    #[test]
    fn ticks_do_nothing_while_paused() {
        let mut timer = quick();
        tick_n(&mut timer, 5);
        assert_eq!(timer.remaining_secs(), 3);
    }

    #[test]
    fn pause_keeps_remaining_time_and_resume_continues() {
        let mut timer = PomodoroTimer::new(PomodoroSettings {
            focus_secs: 10,
            ..PomodoroSettings::default()
        });
        assert!(timer.toggle());
        tick_n(&mut timer, 4);
        assert!(!timer.toggle());
        tick_n(&mut timer, 3);
        assert_eq!(timer.remaining_secs(), 6);
        assert!(timer.toggle());
        tick_n(&mut timer, 2);
        assert_eq!(timer.remaining_secs(), 4);
    }

    #[test]
    fn finishing_focus_counts_round_and_moves_to_short_break_stopped() {
        let mut timer = quick();
        timer.start();
        assert_eq!(timer.tick(), None);
        assert_eq!(timer.tick(), None);
        assert_eq!(timer.tick(), Some(PomodoroPhase::Focus));
        assert_eq!(timer.phase(), PomodoroPhase::ShortBreak);
        assert_eq!(timer.completed_rounds(), 1);
        assert_eq!(timer.remaining_secs(), 2);
        assert!(!timer.is_running());
    }

    #[test]
    fn auto_advance_keeps_the_clock_running_into_the_next_phase() {
        let mut timer = PomodoroTimer::new(PomodoroSettings {
            focus_secs: 1,
            auto_advance: true,
            ..PomodoroSettings::default()
        });
        timer.start();
        assert_eq!(timer.tick(), Some(PomodoroPhase::Focus));
        assert_eq!(timer.phase(), PomodoroPhase::ShortBreak);
        assert!(timer.is_running());
    }

    #[test]
    fn long_break_after_configured_rounds_then_cycle_restarts() {
        let mut timer = quick();
        for _ in 0..2 {
            assert_eq!(timer.phase(), PomodoroPhase::Focus);
            timer.start();
            tick_n(&mut timer, 3);
            if timer.phase() == PomodoroPhase::ShortBreak {
                timer.start();
                tick_n(&mut timer, 2);
            }
        }
        assert_eq!(timer.phase(), PomodoroPhase::LongBreak);
        assert_eq!(timer.remaining_secs(), 4);

        timer.start();
        tick_n(&mut timer, 4);
        assert_eq!(timer.phase(), PomodoroPhase::Focus);
        assert_eq!(timer.completed_rounds(), 0);
        assert_eq!(timer.round_number(), 1);
    }

    #[test]
    fn next_skips_phase_without_counting_focus_and_keeps_running_state() {
        let mut timer = quick();
        timer.start();
        timer.next();
        assert_eq!(timer.phase(), PomodoroPhase::ShortBreak);
        assert_eq!(timer.completed_rounds(), 0);
        assert!(timer.is_running());

        let mut paused = quick();
        paused.next();
        assert_eq!(paused.phase(), PomodoroPhase::ShortBreak);
        assert!(!paused.is_running());
    }

    #[test]
    fn next_from_break_returns_to_focus_full_length() {
        let mut timer = quick();
        timer.select(PomodoroPhase::ShortBreak);
        timer.next();
        assert_eq!(timer.phase(), PomodoroPhase::Focus);
        assert_eq!(timer.remaining_secs(), 3);
    }

    #[test]
    fn reset_restores_full_phase_and_stops_but_keeps_rounds() {
        let mut timer = quick();
        timer.start();
        tick_n(&mut timer, 3);
        assert_eq!(timer.completed_rounds(), 1);
        timer.start();
        tick_n(&mut timer, 1);
        timer.reset();
        assert!(!timer.is_running());
        assert_eq!(timer.phase(), PomodoroPhase::ShortBreak);
        assert_eq!(timer.remaining_secs(), 2);
        assert_eq!(timer.completed_rounds(), 1);
    }

    #[test]
    fn select_changes_phase_and_stops() {
        let mut timer = quick();
        timer.start();
        timer.select(PomodoroPhase::LongBreak);
        assert_eq!(timer.phase(), PomodoroPhase::LongBreak);
        assert_eq!(timer.remaining_secs(), 4);
        assert!(!timer.is_running());
    }

    #[test]
    fn without_breaks_focus_follows_focus() {
        let mut timer = PomodoroTimer::new(PomodoroSettings { breaks: false, ..quick_settings() });
        timer.start();
        tick_n(&mut timer, 3);
        assert_eq!(timer.phase(), PomodoroPhase::Focus);
        assert_eq!(timer.completed_rounds(), 1);
        timer.next();
        assert_eq!(timer.phase(), PomodoroPhase::Focus);
    }

    #[test]
    fn progress_tracks_elapsed_fraction() {
        let mut timer = quick();
        timer.start();
        tick_n(&mut timer, 1);
        let p = timer.progress();
        assert!((p - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn round_number_reflects_cycle_position() {
        let mut timer = quick();
        assert_eq!(timer.round_number(), 1);
        timer.start();
        tick_n(&mut timer, 3);
        assert_eq!(timer.round_number(), 1);
        assert_eq!(timer.phase(), PomodoroPhase::ShortBreak);
    }

    #[test]
    fn configure_rebases_untouched_phase_and_keeps_started_one() {
        let mut timer = quick();
        timer.configure(PomodoroSettings { focus_secs: 9, ..quick_settings() });
        assert_eq!(timer.remaining_secs(), 9);
        timer.start();
        tick_n(&mut timer, 2);
        timer.configure(PomodoroSettings { focus_secs: 20, ..quick_settings() });
        assert_eq!(timer.remaining_secs(), 7);
    }

    fn quick_settings() -> PomodoroSettings {
        PomodoroSettings {
            focus_secs: 3,
            short_break_secs: 2,
            long_break_secs: 4,
            rounds_before_long_break: 2,
            auto_advance: false,
            breaks: true,
        }
    }

    #[test]
    fn finished_flag_is_set_on_phase_end_and_cleared_by_start() {
        let mut timer = quick();
        timer.start();
        tick_n(&mut timer, 3);
        assert!(timer.is_finished());
        timer.start();
        assert!(!timer.is_finished());
    }

    #[test]
    fn previous_goes_back_to_full_focus_round() {
        let mut timer = quick();
        timer.select(PomodoroPhase::ShortBreak);
        timer.previous();
        assert_eq!(timer.phase(), PomodoroPhase::Focus);
        assert_eq!(timer.remaining_secs(), 3);
        assert!(!timer.is_running());
    }

    #[test]
    fn phase_ids_round_trip() {
        for phase in [PomodoroPhase::Focus, PomodoroPhase::ShortBreak, PomodoroPhase::LongBreak] {
            assert_eq!(PomodoroPhase::from_str_id(phase.as_str()), Some(phase));
        }
        assert_eq!(PomodoroPhase::from_str_id("nope"), None);
    }
}
