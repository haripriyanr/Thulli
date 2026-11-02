//! Focus timer driven by `tick`: count-up on a task against its estimate
//! (with overtime detection), countdown for breaks, sprints and sessions, and
//! a count-up for planning time.
//!
//! The UI layer owns a `slint::Timer` that calls [`TimerService::tick`] a few
//! times per second. Elapsed time is measured from `Instant`s, so late ticks
//! never lose time, and every transition has an `_at` variant taking an
//! explicit `Instant` so tests do not need to sleep.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Duration as WallDuration, Local, Utc};

use crate::models::{FocusSession, Task, TaskSection, TimeEntry, TimeEntryKind};
use crate::services::store_handle::StoreHandle;
use crate::services::today::{local_date, queue_ids, format_duration};

/// Whether the timer counts down to zero or up without limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerMode {
    /// Focus session with a planned length.
    Countdown,
    /// Open-ended, used to track planning time.
    CountUp,
}

/// What the running timer is measuring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerPurpose {
    /// Counting up on a task against its estimate.
    Task,
    /// A countdown focus session with a chosen length.
    Session,
    /// A pomodoro work sprint (countdown).
    Sprint,
    /// A break (countdown).
    Break,
    /// Planning the day (count-up).
    Planning,
}

/// Break length used when none is chosen.
pub const DEFAULT_BREAK_MINS: u32 = 5;

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
    pub purpose: TimerPurpose,
    pub phase: TimerPhase,
    pub planned_secs: u64,
    /// Estimate for a count-up task timer; zero when there is none.
    pub estimate_secs: u64,
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

    /// Seconds past the estimate on a count-up task timer.
    pub fn overtime_secs(&self) -> u64 {
        if self.mode == TimerMode::CountUp && self.estimate_secs > 0 {
            self.elapsed_secs.saturating_sub(self.estimate_secs)
        } else {
            0
        }
    }

    pub fn is_overtime(&self) -> bool {
        self.overtime_secs() > 0
    }

    /// Clock text: HH:MM:SS for task timers, MM:SS otherwise.
    pub fn formatted(&self) -> String {
        if self.purpose == TimerPurpose::Task {
            format_hh_mm_ss(self.display_secs())
        } else {
            format_mm_ss(self.display_secs())
        }
    }
}

/// What a tick did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TickOutcome {
    /// Whole seconds counted by this tick; accrue these to the active task.
    pub advanced_secs: u64,
    /// True on the one tick where a countdown reached zero.
    pub finished: bool,
    /// True on the one tick where a task timer passed its estimate.
    pub entered_overtime: bool,
}

/// Formats seconds as MM:SS (minutes may exceed two digits).
pub fn format_mm_ss(total_secs: u64) -> String {
    format!("{:02}:{:02}", total_secs / 60, total_secs % 60)
}

/// Formats seconds as HH:MM:SS.
pub fn format_hh_mm_ss(total_secs: u64) -> String {
    format!("{:02}:{:02}:{:02}", total_secs / 3600, (total_secs / 60) % 60, total_secs % 60)
}

struct State {
    mode: TimerMode,
    purpose: TimerPurpose,
    phase: TimerPhase,
    estimate_secs: u64,
    planned_secs: u64,
    elapsed_secs: u64,
    last_tick: Option<Instant>,
    task_id: Option<String>,
}

impl State {
    fn snapshot(&self) -> TimerSnapshot {
        TimerSnapshot {
            mode: self.mode,
            purpose: self.purpose,
            phase: self.phase,
            planned_secs: self.planned_secs,
            estimate_secs: self.estimate_secs,
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
        let mut entered_overtime = false;
        if self.mode == TimerMode::Countdown {
            let left = self.planned_secs.saturating_sub(self.elapsed_secs);
            if advanced >= left {
                advanced = left;
                finished = true;
            }
        }
        if self.mode == TimerMode::CountUp
            && self.estimate_secs > 0
            && self.elapsed_secs <= self.estimate_secs
            && self.elapsed_secs + advanced > self.estimate_secs
        {
            entered_overtime = true;
        }
        self.elapsed_secs += advanced;
        if finished {
            self.phase = TimerPhase::Finished;
            self.last_tick = None;
        }
        TickOutcome { advanced_secs: advanced, finished, entered_overtime }
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
                purpose: TimerPurpose::Session,
                phase: TimerPhase::Idle,
                estimate_secs: 0,
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

    /// Starts a countdown focus session.
    pub fn start_countdown(&self, minutes: u32) {
        self.start_countdown_at(minutes, Instant::now());
    }

    pub fn start_countdown_at(&self, minutes: u32, now: Instant) {
        self.begin(TimerMode::Countdown, TimerPurpose::Session, u64::from(minutes) * 60, 0, None, now);
    }

    /// Starts a pomodoro work sprint (countdown).
    pub fn start_sprint(&self, minutes: u32) {
        self.start_sprint_at(minutes, Instant::now());
    }

    pub fn start_sprint_at(&self, minutes: u32, now: Instant) {
        self.begin(TimerMode::Countdown, TimerPurpose::Sprint, u64::from(minutes) * 60, 0, None, now);
    }

    /// Starts a break countdown; use `DEFAULT_BREAK_MINS` when none is chosen.
    pub fn start_break(&self, minutes: u32) {
        self.start_break_at(minutes, Instant::now());
    }

    pub fn start_break_at(&self, minutes: u32, now: Instant) {
        self.begin(TimerMode::Countdown, TimerPurpose::Break, u64::from(minutes) * 60, 0, None, now);
    }

    /// Starts counting up on a task against its estimate (zero for none).
    pub fn start_task_count_up(&self, task_id: &str, estimate_mins: u32) {
        self.start_task_count_up_at(task_id, estimate_mins, Instant::now());
    }

    pub fn start_task_count_up_at(&self, task_id: &str, estimate_mins: u32, now: Instant) {
        self.begin(
            TimerMode::CountUp,
            TimerPurpose::Task,
            0,
            u64::from(estimate_mins) * 60,
            Some(task_id.to_string()),
            now,
        );
    }

    /// Like [`start_task_count_up_at`](Self::start_task_count_up_at) with the
    /// estimate in seconds, so time already spent can be taken off it.
    pub fn start_task_count_up_secs_at(&self, task_id: &str, estimate_secs: u64, now: Instant) {
        self.begin(TimerMode::CountUp, TimerPurpose::Task, 0, estimate_secs, Some(task_id.to_string()), now);
    }

    /// Starts counting up from zero (planning time).
    pub fn start_count_up(&self) {
        self.start_count_up_at(Instant::now());
    }

    pub fn start_count_up_at(&self, now: Instant) {
        self.begin(TimerMode::CountUp, TimerPurpose::Planning, 0, 0, None, now);
    }

    fn begin(
        &self,
        mode: TimerMode,
        purpose: TimerPurpose,
        planned_secs: u64,
        estimate_secs: u64,
        task_id: Option<String>,
        now: Instant,
    ) {
        let mut s = self.lock();
        s.mode = mode;
        s.purpose = purpose;
        s.phase = TimerPhase::Running;
        s.planned_secs = planned_secs;
        s.estimate_secs = estimate_secs;
        s.elapsed_secs = 0;
        s.last_tick = Some(now);
        // Breaks and planning are not tied to a task; keep an attached task otherwise.
        match purpose {
            TimerPurpose::Break | TimerPurpose::Planning => s.task_id = None,
            TimerPurpose::Task => s.task_id = task_id,
            _ => {}
        }
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
        s.purpose = TimerPurpose::Session;
        s.estimate_secs = 0;
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

// ---------------------------------------------------------------------------
// Focus runs: recording and orchestration
// ---------------------------------------------------------------------------

/// How often (in counted seconds) open time blocks are saved while running.
pub const FLUSH_EVERY_SECS: u64 = 10;
/// Sessions shorter than this ask whether to keep them.
pub const SHORT_SESSION_SECS: u64 = 60;

/// A point in time on both clocks: the monotonic one that drives the timer
/// and the wall clock that stamps saved blocks. Tests build their own.
#[derive(Debug, Clone, Copy)]
pub struct Moment {
    pub instant: Instant,
    pub wall: DateTime<Utc>,
}

impl Moment {
    pub fn now() -> Self {
        Self { instant: Instant::now(), wall: Utc::now() }
    }

    /// The moment `secs` seconds later.
    pub fn after_secs(self, secs: u64) -> Self {
        Self { instant: self.instant + Duration::from_secs(secs), wall: self.wall + WallDuration::seconds(secs as i64) }
    }
}

/// What the person chose when a very short session was finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortChoice {
    /// Keep the session as recorded.
    Save,
    /// Remove the session and the time it added to tasks.
    Delete,
}

/// Result of finishing a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishOutcome {
    pub focused_secs: u64,
    pub discarded: bool,
}

/// Result of choosing a task to focus on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOutcome {
    /// A new run started on the task.
    Started,
    /// A running session now accrues time to the task.
    Attached,
    /// The task is only remembered; it is used when the session starts.
    Chosen,
    /// No such task.
    Missing,
}

/// Result of completing the active task.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompleteOutcome {
    pub completed_task: Option<String>,
    pub next_task: Option<String>,
    /// True when the task was finished within its estimate (or had none).
    pub on_time: bool,
}

/// What to show after a countdown ran out by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowUp {
    /// Nothing special happened.
    None,
    /// A focus session ended.
    SessionEnded { focused_secs: u64 },
    /// A work sprint ended and a break started.
    SprintEnded { focused_secs: u64 },
    /// A break ended; the next task is waiting to be started.
    BreakEnded,
}

/// What the focus view is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusMode {
    Idle,
    Task,
    Session,
    Sprint,
    Break,
    Planning,
}

impl FocusMode {
    pub fn from_purpose(purpose: TimerPurpose) -> Self {
        match purpose {
            TimerPurpose::Task => FocusMode::Task,
            TimerPurpose::Session => FocusMode::Session,
            TimerPurpose::Sprint => FocusMode::Sprint,
            TimerPurpose::Break => FocusMode::Break,
            TimerPurpose::Planning => FocusMode::Planning,
        }
    }
}

/// Everything the focus screens display, computed from the timer and tasks.
#[derive(Debug, Clone, PartialEq)]
pub struct FocusView {
    pub mode: FocusMode,
    pub phase: TimerPhase,
    pub task_id: Option<String>,
    pub task_title: String,
    pub task_notes: String,
    pub clock: String,
    pub estimate_label: String,
    /// 0 to 1 ring progress.
    pub progress: f32,
    pub overtime: bool,
    pub paused: bool,
    /// A break is over and the next task has not been started yet.
    pub awaiting_next: bool,
}

/// One row of today's queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
    pub task_id: String,
    pub title: String,
    pub done: bool,
    pub active: bool,
}

/// Today's tasks with progress: done entries first would hide the work left,
/// so pending tasks come first and finished ones follow.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueueView {
    pub done: usize,
    pub total: usize,
    pub entries: Vec<QueueEntry>,
}

struct Segment {
    entry_id: String,
    task_id: Option<String>,
    started_at: DateTime<Utc>,
    accrued: u64,
    unflushed: u64,
}

struct ActiveRun {
    purpose: TimerPurpose,
    /// For task runs: the task the run began on.
    first_task: Option<String>,
    session_id: Option<String>,
    last_elapsed: u64,
    total: u64,
    task_secs: HashMap<String, u64>,
}

struct ClosedRun {
    session_id: Option<String>,
    task_secs: HashMap<String, u64>,
}

#[derive(Default)]
struct RunState {
    run: Option<ActiveRun>,
    segment: Option<Segment>,
    closed: Option<ClosedRun>,
    /// Task chosen before a session starts.
    pending_task: Option<String>,
    /// Task to come back to when the current break ends.
    break_return: Option<String>,
    awaiting_next: bool,
}

/// Records every run to `FocusSession` and `TimeEntry`, accrues time to
/// tasks, and carries out the focus actions (start, pause, finish, done,
/// skip, break) over the shared timer.
///
/// Call [`observe`](Self::observe) regularly (the UI does so every tick):
/// recording is derived from the timer's state, so runs started by other
/// screens straight on the timer are recorded too.
pub struct FocusRunService {
    handle: StoreHandle,
    timer: Arc<TimerService>,
    state: Mutex<RunState>,
}

impl FocusRunService {
    pub fn new(handle: StoreHandle, timer: Arc<TimerService>) -> Self {
        Self { handle, timer, state: Mutex::new(RunState::default()) }
    }

    pub fn timer(&self) -> &Arc<TimerService> {
        &self.timer
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RunState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ---- recording ----

    /// Syncs the recording with the timer. Cheap; call on every tick.
    pub fn observe(&self) {
        self.observe_at(Moment::now());
    }

    pub fn observe_at(&self, m: Moment) {
        let snap = self.timer.snapshot();
        self.observe_snapshot(&snap, m.wall);
    }

    fn observe_snapshot(&self, snap: &TimerSnapshot, now: DateTime<Utc>) {
        let mut st = self.lock();

        let ended = st.run.as_ref().is_some_and(|run| {
            snap.phase == TimerPhase::Idle
                || snap.purpose != run.purpose
                || snap.elapsed_secs < run.last_elapsed
                || (run.purpose == TimerPurpose::Task && snap.task_id != run.first_task)
        });
        if ended {
            self.close_run(&mut st, now);
        }
        if snap.phase == TimerPhase::Idle {
            return;
        }
        if st.run.is_none() {
            if snap.phase == TimerPhase::Finished {
                return;
            }
            let run = self.begin_run(snap, now);
            st.run = Some(run);
        }

        let seg_task = match snap.purpose {
            TimerPurpose::Break | TimerPurpose::Planning => None,
            _ => snap.task_id.clone(),
        };
        let switch = st.segment.as_ref().is_some_and(|s| s.task_id != seg_task);
        if switch {
            self.close_segment(&mut st);
        }

        let delta = {
            let run = st.run.as_mut().expect("run exists");
            let delta = snap.elapsed_secs.saturating_sub(run.last_elapsed);
            run.last_elapsed = snap.elapsed_secs;
            delta
        };
        if delta > 0 {
            if st.segment.is_none() {
                let started = now - WallDuration::seconds(delta as i64);
                let seg = self.open_segment(&mut st, snap.purpose, seg_task.clone(), started);
                st.segment = Some(seg);
            }
            let run = st.run.as_mut().expect("run exists");
            run.total += delta;
            if let Some(t) = &seg_task {
                *run.task_secs.entry(t.clone()).or_default() += delta;
            }
            let seg = st.segment.as_mut().expect("segment exists");
            seg.accrued += delta;
            seg.unflushed += delta;
        }

        match snap.phase {
            TimerPhase::Paused => self.close_segment(&mut st),
            TimerPhase::Finished => self.close_run(&mut st, now),
            _ => {
                let due = st.segment.as_ref().is_some_and(|s| s.unflushed >= FLUSH_EVERY_SECS);
                if due {
                    let RunState { run, segment, .. } = &mut *st;
                    if let (Some(run), Some(seg)) = (run.as_ref(), segment.as_mut()) {
                        self.flush(run, seg);
                    }
                }
            }
        }
    }

    fn begin_run(&self, snap: &TimerSnapshot, now: DateTime<Utc>) -> ActiveRun {
        let tracks_session = matches!(snap.purpose, TimerPurpose::Task | TimerPurpose::Session | TimerPurpose::Sprint);
        let session_id = tracks_session.then(|| {
            let planned = if snap.purpose == TimerPurpose::Task { snap.estimate_secs } else { snap.planned_secs };
            let mut session = FocusSession::new(planned, snap.task_id.clone());
            session.started_at = now;
            let id = session.id.clone();
            self.handle.mutate(|s| s.focus_sessions.push(session));
            id
        });
        ActiveRun {
            purpose: snap.purpose,
            first_task: snap.task_id.clone(),
            session_id,
            last_elapsed: 0,
            total: 0,
            task_secs: HashMap::new(),
        }
    }

    fn open_segment(
        &self,
        st: &mut RunState,
        purpose: TimerPurpose,
        task_id: Option<String>,
        started_at: DateTime<Utc>,
    ) -> Segment {
        let kind = match purpose {
            TimerPurpose::Break => TimeEntryKind::Break,
            TimerPurpose::Planning => TimeEntryKind::Planning,
            _ => TimeEntryKind::Focus,
        };
        let session_id = st.run.as_ref().and_then(|r| r.session_id.clone());
        let mut entry = TimeEntry::new(kind, started_at, started_at);
        entry.task_id = task_id.clone();
        entry.session_id = session_id.clone();
        let entry_id = entry.id.clone();
        self.handle.mutate(|s| {
            entry.label = match (kind, &task_id) {
                (TimeEntryKind::Planning, _) => "Planning".to_string(),
                (TimeEntryKind::Break, _) => "Break".to_string(),
                (_, Some(id)) => s.task(id).map(|t| t.title.clone()).unwrap_or_default(),
                (_, None) => "Focus session".to_string(),
            };
            s.time_entries.push(entry);
            if let (Some(sid), Some(tid)) = (&session_id, &task_id) {
                if let Some(f) = s.focus_sessions.iter_mut().find(|f| f.id == *sid) {
                    f.task_id = Some(tid.clone());
                }
            }
        });
        Segment { entry_id, task_id, started_at, accrued: 0, unflushed: 0 }
    }

    /// Writes the open block, the task's time and the session total.
    fn flush(&self, run: &ActiveRun, seg: &mut Segment) {
        let ended = seg.started_at + WallDuration::seconds(seg.accrued as i64);
        self.handle.mutate(|s| {
            if let Some(e) = s.time_entries.iter_mut().find(|e| e.id == seg.entry_id) {
                e.ended_at = ended;
            }
            if seg.unflushed > 0 {
                if let Some(t) = seg.task_id.as_deref().and_then(|id| s.task_mut(id)) {
                    t.add_time(seg.unflushed);
                }
            }
            if let Some(f) = run.session_id.as_deref().and_then(|sid| s.focus_sessions.iter_mut().find(|f| f.id == sid)) {
                f.focused_secs = run.total;
            }
        });
        seg.unflushed = 0;
    }

    fn close_segment(&self, st: &mut RunState) {
        let RunState { run, segment, .. } = st;
        if let (Some(run), Some(seg)) = (run.as_ref(), segment.as_mut()) {
            self.flush(run, seg);
        }
        *segment = None;
    }

    fn close_run(&self, st: &mut RunState, now: DateTime<Utc>) {
        self.close_segment(st);
        if let Some(run) = st.run.take() {
            if let Some(sid) = &run.session_id {
                self.handle.mutate(|s| {
                    if let Some(f) = s.focus_sessions.iter_mut().find(|f| f.id == *sid) {
                        f.finish(run.total);
                        f.ended_at = Some(now);
                    }
                });
            }
            st.closed = Some(ClosedRun { session_id: run.session_id, task_secs: run.task_secs });
        }
    }

    /// Removes the most recently closed run: its session, its blocks and the
    /// time it added to tasks.
    fn discard_closed(&self, st: &mut RunState) {
        let Some(closed) = st.closed.take() else { return };
        self.handle.mutate(|s| {
            if let Some(sid) = &closed.session_id {
                s.time_entries.retain(|e| e.session_id.as_deref() != Some(sid.as_str()));
                s.focus_sessions.retain(|f| f.id != *sid);
            }
            for (task_id, secs) in &closed.task_secs {
                if let Some(t) = s.task_mut(task_id) {
                    t.time_taken_secs = t.time_taken_secs.saturating_sub(*secs);
                }
            }
        });
    }

    /// The running block's id and current end, for the live timeline.
    pub fn live_block(&self) -> Option<(String, DateTime<Utc>)> {
        let st = self.lock();
        st.segment.as_ref().map(|s| (s.entry_id.clone(), s.started_at + WallDuration::seconds(s.accrued as i64)))
    }

    /// Total time on a task including what is not saved yet.
    pub fn task_total_secs(&self, task_id: &str) -> u64 {
        let stored = self.handle.read(|s| s.task(task_id).map_or(0, |t| t.time_taken_secs));
        let pending = self
            .lock()
            .segment
            .as_ref()
            .filter(|s| s.task_id.as_deref() == Some(task_id))
            .map_or(0, |s| s.unflushed);
        stored + pending
    }

    // ---- planning ----

    /// Starts tracking planning time.
    pub fn begin_planning_at(&self, m: Moment) {
        self.observe_at(m);
        self.timer.start_count_up_at(m.instant);
        self.observe_at(m);
    }

    /// Stops tracking planning time and adds it to the day. Returns the seconds.
    pub fn finish_planning_at(&self, m: Moment) -> u64 {
        let planning = self.timer.snapshot().purpose == TimerPurpose::Planning;
        if !planning {
            return 0;
        }
        let secs = self.timer.finish_at(m.instant).unwrap_or(0);
        self.observe_at(m);
        self.timer.reset();
        self.observe_at(m);
        secs
    }

    /// True while planning time is being tracked.
    pub fn is_planning(&self) -> bool {
        let snap = self.timer.snapshot();
        snap.purpose == TimerPurpose::Planning && matches!(snap.phase, TimerPhase::Running | TimerPhase::Paused)
    }

    // ---- starting ----

    fn prefs<R>(&self, f: impl FnOnce(&crate::models::Preferences) -> R) -> R {
        self.handle.read(|s| f(&s.preferences))
    }

    /// Starts a countdown focus session. A task chosen earlier is attached.
    pub fn start_session_at(&self, mins: u32, m: Moment) {
        let mins = crate::models::Preferences::clamp_session_mins(mins);
        self.observe_at(m);
        self.timer.start_countdown_at(mins, m.instant);
        let pending = {
            let mut st = self.lock();
            st.awaiting_next = false;
            st.break_return = None;
            st.pending_task.take()
        };
        if let Some(id) = pending {
            self.handle.mutate(|s| s.task_mut(&id).map(|t| t.prepare_for_focus()));
            self.timer.set_task(Some(id));
        } else {
            self.timer.set_task(None);
        }
        self.observe_at(m);
    }

    /// True while a session or sprint (a countdown on work) is running or paused.
    fn session_active(&self) -> bool {
        let snap = self.timer.snapshot();
        matches!(snap.purpose, TimerPurpose::Session | TimerPurpose::Sprint)
            && matches!(snap.phase, TimerPhase::Running | TimerPhase::Paused)
    }

    /// Chooses a task to focus on from the task picker. During a session the
    /// task takes over the time; during a task run focus moves to it; before
    /// anything runs it is remembered for the next session.
    pub fn choose_task_at(&self, task_id: &str, m: Moment) -> StartOutcome {
        if !self.handle.read(|s| s.task(task_id).is_some()) {
            return StartOutcome::Missing;
        }
        let snap = self.timer.snapshot();
        let busy = matches!(snap.phase, TimerPhase::Running | TimerPhase::Paused);
        if busy {
            return self.start_task_at(task_id, m);
        }
        self.lock().pending_task = Some(task_id.to_string());
        StartOutcome::Chosen
    }

    /// The task waiting for the next session, if any.
    pub fn pending_task(&self) -> Option<String> {
        self.lock().pending_task.clone()
    }

    pub fn clear_pending_task(&self) {
        self.lock().pending_task = None;
    }

    /// Starts focusing on a task now: counting up against its estimate, or a
    /// work sprint when pomodoro is on. During a session the task is attached
    /// instead. The task moves to Today.
    pub fn start_task_at(&self, task_id: &str, m: Moment) -> StartOutcome {
        let info = self.handle.mutate(|s| {
            s.task_mut(task_id).map(|t| {
                t.prepare_for_focus();
                (t.estimate_mins, t.time_taken_secs)
            })
        });
        let Some((estimate_mins, _)) = info else { return StartOutcome::Missing };
        self.observe_at(m);
        {
            let mut st = self.lock();
            st.awaiting_next = false;
            st.break_return = None;
            st.pending_task = None;
        }
        if self.session_active() {
            self.timer.set_task(Some(task_id.to_string()));
            self.observe_at(m);
            return StartOutcome::Attached;
        }

        let (sprint, work_mins) = self.prefs(|p| (p.sprint_enabled, p.work_sprint_mins.max(1)));
        if sprint {
            self.timer.start_sprint_at(work_mins, m.instant);
            self.timer.set_task(Some(task_id.to_string()));
        } else {
            let taken = self.task_total_secs(task_id);
            let estimate_secs = if estimate_mins == 0 {
                0
            } else {
                (u64::from(estimate_mins) * 60).saturating_sub(taken).max(1)
            };
            self.timer.start_task_count_up_secs_at(task_id, estimate_secs, m.instant);
        }
        self.observe_at(m);
        StartOutcome::Started
    }

    /// The global "start now" action: the first task in today's queue.
    pub fn start_first_at(&self, m: Moment) -> Option<String> {
        let first = self.handle.read(queue_ids).into_iter().next()?;
        self.start_task_at(&first, m);
        Some(first)
    }

    /// Whether "start now" can do anything.
    pub fn can_start_now(&self) -> bool {
        !self.handle.read(queue_ids).is_empty()
    }

    // ---- pause, resume, finish ----

    pub fn toggle_pause_at(&self, m: Moment) -> bool {
        let running = self.timer.toggle_at(m.instant);
        self.observe_at(m);
        running
    }

    /// If finishing now would end a session shorter than a minute, the
    /// seconds so far; the UI then asks whether to keep it.
    pub fn short_session_secs_at(&self, m: Moment) -> Option<u64> {
        self.timer.tick_at(m.instant);
        self.observe_at(m);
        let snap = self.timer.snapshot();
        let counts = matches!(snap.purpose, TimerPurpose::Task | TimerPurpose::Session | TimerPurpose::Sprint)
            && matches!(snap.phase, TimerPhase::Running | TimerPhase::Paused);
        (counts && snap.elapsed_secs < SHORT_SESSION_SECS).then_some(snap.elapsed_secs)
    }

    /// Ends the current run (session, sprint, task or break) early.
    pub fn finish_at(&self, m: Moment, short: ShortChoice) -> FinishOutcome {
        self.observe_at(m);
        let phase = self.timer.snapshot().phase;
        if matches!(phase, TimerPhase::Idle) {
            return FinishOutcome { focused_secs: 0, discarded: false };
        }
        let elapsed = self.timer.finish_at(m.instant).unwrap_or(self.timer.snapshot().elapsed_secs);
        self.observe_at(m);
        let discard = short == ShortChoice::Delete && elapsed < SHORT_SESSION_SECS;
        if discard {
            let mut st = self.lock();
            self.discard_closed(&mut st);
        }
        self.timer.reset();
        {
            let mut st = self.lock();
            st.pending_task = None;
            st.awaiting_next = false;
            st.break_return = None;
        }
        self.observe_at(m);
        FinishOutcome { focused_secs: if discard { 0 } else { elapsed }, discarded: discard }
    }

    // ---- tasks within a run ----

    /// The next task in the queue after `current` (wrapping), if another exists.
    fn next_after(&self, current: Option<&str>) -> Option<String> {
        let ids = self.handle.read(queue_ids);
        let others: Vec<&String> = ids.iter().filter(|id| Some(id.as_str()) != current).collect();
        others.first().map(|s| (*s).clone())
    }

    /// Completes the active task, then moves to the next one in the queue (or
    /// ends the run when none is left).
    pub fn complete_task_at(&self, m: Moment) -> CompleteOutcome {
        self.observe_at(m);
        let snap = self.timer.snapshot();
        let Some(task_id) = snap.task_id.clone() else { return CompleteOutcome::default() };
        if matches!(snap.phase, TimerPhase::Idle | TimerPhase::Finished) {
            return CompleteOutcome::default();
        }
        self.timer.finish_at(m.instant);
        self.observe_at(m);
        let on_time = self.handle.mutate(|s| {
            s.task_mut(&task_id)
                .map(|t| {
                    let on_time = !t.is_over_estimate();
                    t.complete();
                    on_time
                })
                .unwrap_or(true)
        });
        self.timer.reset();
        let next = self.next_after(Some(&task_id));
        if let Some(next_id) = &next {
            self.start_task_at(next_id, m);
        } else {
            self.observe_at(m);
        }
        CompleteOutcome { completed_task: Some(task_id), next_task: next, on_time }
    }

    /// Puts the active task back at the end of the queue and starts the next
    /// one. Returns the new task, or `None` when it is the only one.
    pub fn skip_at(&self, m: Moment) -> Option<String> {
        let snap = self.timer.snapshot();
        let current = snap.task_id.clone();
        let next = self.next_after(current.as_deref())?;
        if let Some(id) = &current {
            self.handle.mutate(|s| {
                let top = s.tasks.iter().map(|t| t.sort_order).max().map_or(0, |v| v + 1);
                if let Some(t) = s.task_mut(id) {
                    t.sort_order = top;
                }
            });
        }
        self.finish_current_for_switch(m);
        self.start_task_at(&next, m);
        Some(next)
    }

    /// Closes the running run so a different one can begin.
    fn finish_current_for_switch(&self, m: Moment) {
        if matches!(self.timer.snapshot().phase, TimerPhase::Running | TimerPhase::Paused) {
            self.timer.finish_at(m.instant);
            self.observe_at(m);
            self.timer.reset();
            self.observe_at(m);
        }
    }

    /// Marks the active task as waiting and moves on.
    pub fn wait_task_at(&self, m: Moment) -> Option<String> {
        let snap = self.timer.snapshot();
        let id = snap.task_id.clone()?;
        self.observe_at(m);
        self.finish_current_for_switch(m);
        self.handle.mutate(|s| s.task_mut(&id).map(|t| t.mark_waiting()));
        let next = self.next_after(Some(&id));
        if let Some(n) = &next {
            self.start_task_at(n, m);
        }
        next
    }

    /// Adds a breakdown step under a task. Returns the new step's id.
    pub fn add_step(&self, parent_id: &str, title: &str) -> Option<String> {
        let title = title.trim();
        if title.is_empty() {
            return None;
        }
        self.handle.mutate(|s| {
            let parent = s.task(parent_id)?.clone();
            let order = s.steps_of(parent_id).len() as i32;
            let mut step = Task::new_step(&parent, title);
            step.sort_order = order;
            Some(s.add_task(step).id)
        })
    }

    /// Completes or reopens a step.
    pub fn toggle_step(&self, step_id: &str) -> bool {
        self.handle.mutate(|s| s.toggle_task(step_id)).is_some()
    }

    /// Replaces a task's notes.
    pub fn set_notes(&self, task_id: &str, notes: &str) {
        self.handle.mutate(|s| s.task_mut(task_id).map(|t| t.notes = notes.to_string()));
    }

    // ---- breaks ----

    /// Starts a break (the default length when `mins` is `None`). The task
    /// goes back in the queue; the break remembers it.
    pub fn start_break_at(&self, mins: Option<u32>, m: Moment) {
        self.observe_at(m);
        let snap = self.timer.snapshot();
        let returning = match snap.purpose {
            TimerPurpose::Task | TimerPurpose::Sprint | TimerPurpose::Session
                if matches!(snap.phase, TimerPhase::Running | TimerPhase::Paused) =>
            {
                snap.task_id.clone()
            }
            _ => None,
        };
        if matches!(snap.phase, TimerPhase::Running | TimerPhase::Paused) && snap.purpose != TimerPurpose::Break {
            self.timer.pause_at(m.instant);
            self.observe_at(m);
        }
        let mins = mins.unwrap_or_else(|| self.prefs(|p| p.default_break_mins.max(1)));
        self.timer.start_break_at(mins, m.instant);
        {
            let mut st = self.lock();
            st.break_return = returning.or(st.break_return.take());
            st.awaiting_next = false;
        }
        self.observe_at(m);
    }

    /// Ends the running break early. With the "start next task" preference on,
    /// the next task starts straight away.
    pub fn end_break_at(&self, m: Moment) {
        self.observe_at(m);
        if self.timer.snapshot().purpose != TimerPurpose::Break {
            return;
        }
        self.timer.finish_at(m.instant);
        self.observe_at(m);
        self.timer.reset();
        self.observe_at(m);
        self.lock().awaiting_next = true;
        if self.prefs(|p| p.start_next_after_early_break) {
            self.continue_after_break_at(m);
        }
    }

    /// Starts the task the break came from, or the next one in the queue.
    pub fn continue_after_break_at(&self, m: Moment) -> Option<String> {
        let ret = {
            let mut st = self.lock();
            st.awaiting_next = false;
            st.break_return.take()
        };
        let still_open = |id: &str| self.handle.read(|s| s.task(id).is_some_and(|t| t.status == crate::models::TaskStatus::Open));
        let target = ret.filter(|id| still_open(id)).or_else(|| self.handle.read(queue_ids).into_iter().next())?;
        self.start_task_at(&target, m);
        Some(target)
    }

    /// Reacts to a countdown that ran out by itself: a finished sprint starts
    /// its break, a finished break waits for the next task.
    pub fn handle_finished_at(&self, m: Moment) -> FollowUp {
        self.observe_at(m);
        let snap = self.timer.snapshot();
        if snap.phase != TimerPhase::Finished {
            return FollowUp::None;
        }
        match snap.purpose {
            TimerPurpose::Sprint => {
                let task = snap.task_id.clone();
                let mins = self.prefs(|p| p.break_time_mins.max(1));
                self.timer.reset();
                self.observe_at(m);
                self.timer.start_break_at(mins, m.instant);
                {
                    let mut st = self.lock();
                    st.break_return = task;
                    st.awaiting_next = false;
                }
                self.observe_at(m);
                FollowUp::SprintEnded { focused_secs: snap.planned_secs }
            }
            TimerPurpose::Break => {
                self.timer.reset();
                self.lock().awaiting_next = true;
                self.observe_at(m);
                FollowUp::BreakEnded
            }
            TimerPurpose::Session => {
                self.timer.reset();
                self.observe_at(m);
                FollowUp::SessionEnded { focused_secs: snap.planned_secs }
            }
            _ => FollowUp::None,
        }
    }

    /// True after a break ended and the next task has not been started.
    pub fn awaiting_next(&self) -> bool {
        self.lock().awaiting_next
    }

    // ---- reading ----

    /// Today's queue with progress.
    pub fn queue(&self) -> QueueView {
        let active = self.timer.snapshot().task_id;
        let date = Local::now().date_naive();
        self.handle.read(|s| {
            let pending = queue_ids(s);
            let done_ids: Vec<&Task> = s
                .tasks
                .iter()
                .filter(|t| {
                    !t.is_step()
                        && t.section == TaskSection::Today
                        && t.is_completed()
                        && t.completed_at.is_some_and(|c| local_date(c) == date)
                })
                .collect();
            let mut entries: Vec<QueueEntry> = pending
                .iter()
                .filter_map(|id| s.task(id))
                .map(|t| QueueEntry {
                    task_id: t.id.clone(),
                    title: t.title.clone(),
                    done: false,
                    active: active.as_deref() == Some(t.id.as_str()),
                })
                .collect();
            let done = done_ids.len();
            entries.extend(done_ids.into_iter().map(|t| QueueEntry {
                task_id: t.id.clone(),
                title: t.title.clone(),
                done: true,
                active: false,
            }));
            QueueView { done, total: entries.len(), entries }
        })
    }

    /// The display state of the focus screens.
    pub fn view(&self) -> FocusView {
        let snap = self.timer.snapshot();
        let mode = if snap.phase == TimerPhase::Idle { FocusMode::Idle } else { FocusMode::from_purpose(snap.purpose) };
        let (title, notes, estimate_mins) = snap
            .task_id
            .as_deref()
            .and_then(|id| self.handle.read(|s| s.task(id).map(|t| (t.title.clone(), t.notes.clone(), t.estimate_mins))))
            .unwrap_or_default();

        let mut view = FocusView {
            mode,
            phase: snap.phase,
            task_id: snap.task_id.clone(),
            task_title: title,
            task_notes: notes,
            clock: snap.formatted(),
            estimate_label: String::new(),
            progress: 0.0,
            overtime: false,
            paused: snap.phase == TimerPhase::Paused,
            awaiting_next: self.awaiting_next() && mode == FocusMode::Idle,
        };
        match mode {
            FocusMode::Task => {
                let total = snap.task_id.as_deref().map_or(snap.elapsed_secs, |id| self.task_total_secs(id));
                let estimate = u64::from(estimate_mins) * 60;
                if estimate > 0 {
                    view.progress = (total as f32 / estimate as f32).min(1.0);
                    view.overtime = total > estimate;
                    view.estimate_label = if view.overtime {
                        format!("{} over the {} estimate", format_duration(total - estimate), format_duration(estimate))
                    } else {
                        format!("{} of {} estimate", format_duration(total), format_duration(estimate))
                    };
                } else {
                    view.estimate_label = format!("{} so far, no estimate", format_duration(total));
                }
            }
            FocusMode::Session | FocusMode::Sprint | FocusMode::Break => {
                if snap.planned_secs > 0 {
                    view.progress = (snap.elapsed_secs as f32 / snap.planned_secs as f32).min(1.0);
                }
                view.estimate_label = format!("{} left", format_duration(snap.remaining_secs()));
            }
            FocusMode::Planning | FocusMode::Idle => {}
        }
        view
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
    fn task_count_up_detects_overtime_once() {
        let t0 = Instant::now();
        let timer = TimerService::new(25);
        timer.start_task_count_up_at("task-1", 1, t0);
        assert_eq!(timer.formatted_display(), "00:00:00");

        let out = timer.tick_at(t0 + secs(59));
        assert!(!out.entered_overtime && !timer.snapshot().is_overtime());

        // Exactly at the estimate is not yet overtime.
        assert!(!timer.tick_at(t0 + secs(60)).entered_overtime);
        let out = timer.tick_at(t0 + secs(65));
        assert!(out.entered_overtime);
        assert_eq!(timer.snapshot().overtime_secs(), 5);
        assert!(!timer.tick_at(t0 + secs(70)).entered_overtime, "reported once");
        assert_eq!(timer.snapshot().task_id.as_deref(), Some("task-1"));
    }

    #[test]
    fn task_count_up_formats_hours_and_has_no_overtime_without_estimate() {
        let t0 = Instant::now();
        let timer = TimerService::new(25);
        timer.start_task_count_up_at("t", 0, t0);
        let out = timer.tick_at(t0 + secs(3725));
        assert!(!out.entered_overtime && !out.finished);
        assert_eq!(timer.formatted_display(), "01:02:05");
        assert!(!timer.snapshot().is_overtime());
    }

    #[test]
    fn break_counts_down_from_five_minutes_by_default() {
        let t0 = Instant::now();
        let timer = TimerService::new(25);
        timer.start_task_count_up_at("t", 30, t0);
        timer.pause_at(t0 + secs(10));
        timer.start_break_at(DEFAULT_BREAK_MINS, t0 + secs(10));
        let snap = timer.snapshot();
        assert_eq!(snap.purpose, TimerPurpose::Break);
        assert_eq!(timer.formatted_display(), "05:00");
        assert!(snap.task_id.is_none());
        assert!(timer.tick_at(t0 + secs(10 + 300)).finished);
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

#[cfg(test)]
mod run_tests {
    use super::*;
    use crate::models::{AppStateStore, TaskStatus};
    use crate::repository::{LoadReport, StorageRepository};

    struct MemoryRepo;
    impl StorageRepository for MemoryRepo {
        fn load_with_report(&self) -> LoadReport {
            LoadReport { store: AppStateStore::default(), recovery: None }
        }
        fn save(&self, _: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
    }

    fn setup() -> (FocusRunService, Moment) {
        let repo: Arc<dyn StorageRepository> = Arc::new(MemoryRepo);
        let handle = StoreHandle::new(Arc::new(Mutex::new(AppStateStore::default())), repo);
        let timer = Arc::new(TimerService::new(90));
        (FocusRunService::new(handle, timer), Moment::now())
    }

    fn add_task(svc: &FocusRunService, title: &str, est: u32, order: i32) -> String {
        svc.handle.mutate(|s| {
            let mut t = Task::new("l", title, est, TaskSection::Today);
            t.sort_order = order;
            s.add_task(t).id
        })
    }

    /// Advances both clocks and syncs the recorder, like a UI tick.
    fn tick(svc: &FocusRunService, m: Moment, secs: u64) -> Moment {
        let later = m.after_secs(secs);
        svc.timer.tick_at(later.instant);
        svc.observe_at(later);
        later
    }

    fn task_secs(svc: &FocusRunService, id: &str) -> u64 {
        svc.handle.read(|s| s.task(id).unwrap().time_taken_secs)
    }

    #[test]
    fn task_run_records_session_entry_and_task_time() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "Write", 30, 0);
        assert_eq!(svc.start_task_at(&a, t0), StartOutcome::Started);
        let t1 = tick(&svc, t0, 125);
        // Not yet saved, but visible.
        assert_eq!(svc.task_total_secs(&a), 125);
        svc.toggle_pause_at(t1);
        assert_eq!(task_secs(&svc, &a), 125);

        let (sessions, entries) = svc.handle.read(|s| (s.focus_sessions.clone(), s.time_entries.clone()));
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].task_id.as_deref(), Some(a.as_str()));
        assert_eq!(sessions[0].focused_secs, 125);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, TimeEntryKind::Focus);
        assert_eq!(entries[0].duration_secs(), 125);
        assert_eq!(entries[0].label, "Write");
        assert_eq!(entries[0].session_id.as_deref(), Some(sessions[0].id.as_str()));

        // Resume opens a second block in the same session.
        let t2 = t1.after_secs(600);
        svc.toggle_pause_at(t2);
        let t3 = tick(&svc, t2, 60);
        svc.finish_at(t3, ShortChoice::Save);
        let (sessions, entries) = svc.handle.read(|s| (s.focus_sessions.clone(), s.time_entries.clone()));
        assert_eq!((sessions.len(), entries.len()), (1, 2));
        assert_eq!(sessions[0].focused_secs, 185);
        assert!(sessions[0].is_finished());
        assert_eq!(task_secs(&svc, &a), 185);
        assert_eq!(svc.timer.snapshot().phase, TimerPhase::Idle);
    }

    #[test]
    fn overtime_is_measured_against_the_whole_task() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "Report", 10, 0);
        svc.handle.mutate(|s| s.task_mut(&a).unwrap().time_taken_secs = 9 * 60);
        svc.start_task_at(&a, t0);
        // One minute of room is left.
        assert_eq!(svc.timer.snapshot().estimate_secs, 60);
        let t1 = tick(&svc, t0, 30);
        let v = svc.view();
        assert!(!v.overtime);
        assert!((v.progress - 0.95).abs() < 0.01);
        tick(&svc, t1, 45);
        let v = svc.view();
        assert!(v.overtime);
        assert!(v.estimate_label.contains("over"));
        assert!(svc.timer.snapshot().is_overtime());
    }

    #[test]
    fn session_time_goes_to_the_attached_task_and_splits_on_switch() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 0, 0);
        let b = add_task(&svc, "B", 0, 1);
        assert_eq!(svc.choose_task_at(&a, t0), StartOutcome::Chosen);
        svc.start_session_at(45, t0);
        assert_eq!(svc.timer.snapshot().task_id.as_deref(), Some(a.as_str()));
        let t1 = tick(&svc, t0, 300);
        assert_eq!(svc.choose_task_at(&b, t1), StartOutcome::Attached);
        let t2 = tick(&svc, t1, 120);
        svc.finish_at(t2, ShortChoice::Save);

        assert_eq!(task_secs(&svc, &a), 300);
        assert_eq!(task_secs(&svc, &b), 120);
        let (sessions, entries) = svc.handle.read(|s| (s.focus_sessions.clone(), s.time_entries.clone()));
        assert_eq!(sessions.len(), 1, "one session, two blocks");
        assert_eq!(sessions[0].focused_secs, 420);
        assert_eq!(sessions[0].planned_secs, 45 * 60);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn session_without_a_task_still_counts_as_focus_time() {
        let (svc, t0) = setup();
        svc.start_session_at(30, t0);
        let t1 = tick(&svc, t0, 200);
        svc.finish_at(t1, ShortChoice::Save);
        let entries = svc.handle.read(|s| s.time_entries.clone());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Focus session");
        assert_eq!(entries[0].duration_secs(), 200);
    }

    #[test]
    fn short_sessions_can_be_saved_or_deleted() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 0, 0);
        svc.choose_task_at(&a, t0);
        svc.start_session_at(30, t0);
        let t1 = tick(&svc, t0, 20);
        assert_eq!(svc.short_session_secs_at(t1), Some(20));
        let out = svc.finish_at(t1, ShortChoice::Delete);
        assert!(out.discarded);
        assert_eq!(task_secs(&svc, &a), 0);
        assert!(svc.handle.read(|s| s.focus_sessions.is_empty() && s.time_entries.is_empty()));

        svc.start_session_at(30, t1);
        let t2 = tick(&svc, t1, 20);
        let out = svc.finish_at(t2, ShortChoice::Save);
        assert!(!out.discarded);
        assert_eq!(svc.handle.read(|s| s.focus_sessions.len()), 1);

        // A long session is never discarded, whatever the choice.
        svc.start_session_at(30, t2);
        let t3 = tick(&svc, t2, 90);
        assert_eq!(svc.short_session_secs_at(t3), None);
        assert!(!svc.finish_at(t3, ShortChoice::Delete).discarded);
        assert_eq!(svc.handle.read(|s| s.focus_sessions.len()), 2);
    }

    #[test]
    fn done_completes_the_task_and_starts_the_next() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 30, 0);
        let b = add_task(&svc, "B", 30, 1);
        svc.start_task_at(&a, t0);
        let t1 = tick(&svc, t0, 600);
        let out = svc.complete_task_at(t1);
        assert_eq!(out.completed_task.as_deref(), Some(a.as_str()));
        assert_eq!(out.next_task.as_deref(), Some(b.as_str()));
        assert!(out.on_time);
        assert_eq!(task_secs(&svc, &a), 600);
        assert_eq!(svc.handle.read(|s| s.task(&a).unwrap().status), TaskStatus::Completed);
        assert_eq!(svc.timer.snapshot().task_id.as_deref(), Some(b.as_str()));
        assert_eq!(svc.timer.snapshot().phase, TimerPhase::Running);

        let q = svc.queue();
        assert_eq!((q.done, q.total), (1, 2));
        assert!(q.entries[0].active && !q.entries[0].done);
        assert!(q.entries[1].done);

        // The last task ends the run.
        let t2 = tick(&svc, t1, 60);
        let out = svc.complete_task_at(t2);
        assert_eq!(out.next_task, None);
        assert_eq!(svc.timer.snapshot().phase, TimerPhase::Idle);
        assert_eq!(svc.handle.read(|s| s.focus_sessions.len()), 2);
    }

    #[test]
    fn finishing_over_the_estimate_is_not_on_time() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 1, 0);
        svc.start_task_at(&a, t0);
        let t1 = tick(&svc, t0, 120);
        assert!(!svc.complete_task_at(t1).on_time);
    }

    #[test]
    fn skip_sends_the_task_to_the_back_of_the_queue() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 0, 0);
        let b = add_task(&svc, "B", 0, 1);
        let c = add_task(&svc, "C", 0, 2);
        svc.start_task_at(&a, t0);
        let t1 = tick(&svc, t0, 30);
        assert_eq!(svc.skip_at(t1).as_deref(), Some(b.as_str()));
        let order: Vec<String> = svc.queue().entries.into_iter().map(|e| e.task_id).collect();
        assert_eq!(order, vec![b.clone(), c, a.clone()]);
        assert_eq!(task_secs(&svc, &a), 30);
        assert_eq!(svc.timer.snapshot().task_id.as_deref(), Some(b.as_str()));
    }

    #[test]
    fn skip_does_nothing_when_there_is_no_other_task() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 0, 0);
        svc.start_task_at(&a, t0);
        assert_eq!(svc.skip_at(t0), None);
        assert_eq!(svc.timer.snapshot().task_id.as_deref(), Some(a.as_str()));
    }

    #[test]
    fn break_is_recorded_and_returns_to_the_task() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 0, 0);
        svc.start_task_at(&a, t0);
        let t1 = tick(&svc, t0, 120);
        svc.start_break_at(None, t1);
        let snap = svc.timer.snapshot();
        assert_eq!(snap.purpose, TimerPurpose::Break);
        assert_eq!(snap.planned_secs, 600);
        let t2 = tick(&svc, t1, 100);

        // Ending early with the preference off waits for the next task.
        svc.end_break_at(t2);
        assert!(svc.awaiting_next());
        assert!(svc.view().awaiting_next);
        let kinds: Vec<TimeEntryKind> = svc.handle.read(|s| s.time_entries.iter().map(|e| e.kind).collect());
        assert_eq!(kinds, vec![TimeEntryKind::Focus, TimeEntryKind::Break]);
        assert_eq!(svc.handle.read(|s| s.time_entries[1].duration_secs()), 100);
        assert_eq!(svc.handle.read(|s| s.focus_sessions.len()), 1, "breaks are not focus sessions");

        assert_eq!(svc.continue_after_break_at(t2).as_deref(), Some(a.as_str()));
        assert!(!svc.awaiting_next());
        assert_eq!(svc.timer.snapshot().purpose, TimerPurpose::Task);
    }

    #[test]
    fn ending_a_break_early_can_start_the_next_task() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 0, 0);
        svc.handle.mutate(|s| s.preferences.start_next_after_early_break = true);
        svc.start_task_at(&a, t0);
        svc.start_break_at(Some(10), t0);
        svc.end_break_at(t0.after_secs(5));
        assert_eq!(svc.timer.snapshot().task_id.as_deref(), Some(a.as_str()));
        assert!(!svc.awaiting_next());
    }

    #[test]
    fn a_break_that_runs_out_waits_for_the_next_task() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 0, 0);
        svc.start_task_at(&a, t0);
        svc.start_break_at(Some(1), t0);
        let t1 = tick(&svc, t0, 61);
        assert_eq!(svc.handle_finished_at(t1), FollowUp::BreakEnded);
        assert!(svc.awaiting_next());
        assert_eq!(svc.timer.snapshot().phase, TimerPhase::Idle);
    }

    #[test]
    fn pomodoro_sprint_runs_then_breaks() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 120, 0);
        svc.handle.mutate(|s| {
            s.preferences.sprint_enabled = true;
            s.preferences.work_sprint_mins = 2;
            s.preferences.break_time_mins = 3;
        });
        svc.start_task_at(&a, t0);
        let snap = svc.timer.snapshot();
        assert_eq!((snap.purpose, snap.planned_secs), (TimerPurpose::Sprint, 120));
        assert_eq!(svc.view().mode, FocusMode::Sprint);

        let t1 = tick(&svc, t0, 130);
        assert_eq!(svc.handle_finished_at(t1), FollowUp::SprintEnded { focused_secs: 120 });
        let snap = svc.timer.snapshot();
        assert_eq!((snap.purpose, snap.planned_secs), (TimerPurpose::Break, 180));
        assert_eq!(task_secs(&svc, &a), 120);

        let t2 = tick(&svc, t1, 181);
        assert_eq!(svc.handle_finished_at(t2), FollowUp::BreakEnded);
        assert_eq!(svc.continue_after_break_at(t2).as_deref(), Some(a.as_str()));
        assert_eq!(svc.timer.snapshot().purpose, TimerPurpose::Sprint);
    }

    #[test]
    fn a_session_that_runs_out_closes_its_record() {
        let (svc, t0) = setup();
        svc.start_session_at(10, t0);
        let t1 = tick(&svc, t0, 10 * 60 + 5);
        assert_eq!(svc.handle_finished_at(t1), FollowUp::SessionEnded { focused_secs: 600 });
        let sessions = svc.handle.read(|s| s.focus_sessions.clone());
        assert!(sessions[0].is_finished());
        assert_eq!(sessions[0].focused_secs, 600);
    }

    #[test]
    fn planning_time_becomes_a_planning_block() {
        let (svc, t0) = setup();
        svc.begin_planning_at(t0);
        assert!(svc.is_planning());
        tick(&svc, t0, 90);
        let secs = svc.finish_planning_at(t0.after_secs(95));
        assert_eq!(secs, 95);
        assert!(!svc.is_planning());
        let entries = svc.handle.read(|s| s.time_entries.clone());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, TimeEntryKind::Planning);
        assert_eq!(entries[0].label, "Planning");
        assert_eq!(entries[0].duration_secs(), 95);
        assert_eq!(svc.handle.read(|s| s.focus_sessions.len()), 0);
        assert_eq!(svc.finish_planning_at(t0.after_secs(100)), 0, "nothing to finish");
    }

    #[test]
    fn runs_started_straight_on_the_timer_are_recorded_too() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 30, 0);
        let b = add_task(&svc, "B", 30, 1);
        svc.timer.start_task_count_up_at(&a, 30, t0.instant);
        let t1 = tick(&svc, t0, 100);
        // Another screen switches task without telling anyone.
        svc.timer.start_task_count_up_at(&b, 30, t1.instant);
        let t2 = tick(&svc, t1, 50);
        svc.timer.pause_at(t2.instant);
        svc.observe_at(t2);
        assert_eq!(task_secs(&svc, &a), 100);
        assert_eq!(task_secs(&svc, &b), 50);
        assert_eq!(svc.handle.read(|s| s.focus_sessions.len()), 2);
    }

    #[test]
    fn open_blocks_are_saved_periodically_and_reported_live() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 0, 0);
        svc.start_task_at(&a, t0);
        let t1 = tick(&svc, t0, FLUSH_EVERY_SECS + 2);
        assert_eq!(task_secs(&svc, &a), FLUSH_EVERY_SECS + 2);
        let t2 = tick(&svc, t1, 3);
        assert_eq!(task_secs(&svc, &a), FLUSH_EVERY_SECS + 2, "not saved again yet");
        assert_eq!(svc.task_total_secs(&a), FLUSH_EVERY_SECS + 5);
        let (id, end) = svc.live_block().expect("a block is open");
        let entry = svc.handle.read(|s| s.time_entries.iter().find(|e| e.id == id).cloned()).unwrap();
        assert_eq!((end - entry.started_at).num_seconds() as u64, FLUSH_EVERY_SECS + 5);
        svc.toggle_pause_at(t2);
        assert!(svc.live_block().is_none());
        assert_eq!(task_secs(&svc, &a), FLUSH_EVERY_SECS + 5);
    }

    #[test]
    fn starting_focus_moves_the_task_to_today_and_start_now_uses_the_queue() {
        let (svc, t0) = setup();
        assert!(!svc.can_start_now());
        assert_eq!(svc.start_first_at(t0), None);
        let later = svc.handle.mutate(|s| s.add_task(Task::new("l", "Later", 0, TaskSection::Backlog)).id);
        assert!(!svc.can_start_now());
        assert_eq!(svc.start_task_at(&later, t0), StartOutcome::Started);
        assert_eq!(svc.handle.read(|s| s.task(&later).unwrap().section), TaskSection::Today);
        assert_eq!(svc.start_task_at("missing", t0), StartOutcome::Missing);
        assert!(svc.can_start_now());
    }

    #[test]
    fn steps_and_notes_are_editable() {
        let (svc, _) = setup();
        let a = add_task(&svc, "A", 0, 0);
        let s1 = svc.add_step(&a, "  First  ").unwrap();
        assert!(svc.add_step(&a, "   ").is_none());
        svc.add_step(&a, "Second");
        assert_eq!(svc.handle.read(|s| s.steps_of(&a).len()), 2);
        assert!(svc.toggle_step(&s1));
        assert!(svc.handle.read(|s| s.task(&s1).unwrap().is_completed()));
        svc.set_notes(&a, "- call\n- email");
        assert_eq!(svc.handle.read(|s| s.task(&a).unwrap().notes.clone()), "- call\n- email");
    }

    #[test]
    fn waiting_marks_the_task_and_moves_on() {
        let (svc, t0) = setup();
        let a = add_task(&svc, "A", 0, 0);
        let b = add_task(&svc, "B", 0, 1);
        svc.start_task_at(&a, t0);
        let t1 = tick(&svc, t0, 20);
        assert_eq!(svc.wait_task_at(t1).as_deref(), Some(b.as_str()));
        assert_eq!(svc.handle.read(|s| s.task(&a).unwrap().status), TaskStatus::Waiting);
        assert_eq!(task_secs(&svc, &a), 20);
    }

    #[test]
    fn view_describes_each_mode() {
        let (svc, t0) = setup();
        assert_eq!(svc.view().mode, FocusMode::Idle);
        svc.start_session_at(25, t0);
        let v = svc.view();
        assert_eq!((v.mode, v.clock.as_str()), (FocusMode::Session, "25:00"));
        let t1 = tick(&svc, t0, 300);
        let v = svc.view();
        assert!((v.progress - 0.2).abs() < 0.001);
        assert_eq!(v.estimate_label, "20m left");
        svc.toggle_pause_at(t1);
        assert!(svc.view().paused);
    }
}
