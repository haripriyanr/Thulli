//! Sounds, alerts and celebration.
//!
//! Everything here is plain logic except the two small adapters at the bottom
//! (`RodioPlayer`, `ToastNotifier`), which sit behind traits so tests never
//! open an audio device or show a notification.
//!
//! * Sounds are synthesised from a few sine tones, so Thulli ships no audio files.
//! * [`AlertEngine`] watches the focus timer and the schedule and says which
//!   alerts are due; [`plan`] turns an alert into a sound and a message using
//!   the person's preferences.
//! * [`plan_celebration`] decides what the success moment looks like.

use std::cell::RefCell;
use std::collections::HashSet;
use std::f32::consts::PI;
use std::num::NonZero;

use chrono::{DateTime, Duration, Utc};

use crate::models::{Preferences, Task, TaskStatus};
use crate::services::timer::{TickOutcome, TimerPhase, TimerPurpose, TimerSnapshot};

// ---------------------------------------------------------------------------
// Synthesised sounds
// ---------------------------------------------------------------------------

/// Samples per second of generated sounds.
pub const SAMPLE_RATE: u32 = 44_100;

/// One tone in a sound: frequency in Hz, start and length in ms, loudness
/// 0..1, and how fast it fades (bigger is quicker).
struct Note {
    freq: f32,
    start_ms: u32,
    len_ms: u32,
    gain: f32,
    decay: f32,
}

const fn note(freq: f32, start_ms: u32, len_ms: u32, gain: f32, decay: f32) -> Note {
    Note { freq, start_ms, len_ms, gain, decay }
}

fn recipe(id: &str) -> Vec<Note> {
    match id {
        "soft-chime" => vec![note(660.0, 0, 700, 0.5, 4.5), note(990.0, 120, 800, 0.35, 4.0)],
        "bright-bell" => vec![note(1318.0, 0, 900, 0.5, 5.0), note(1760.0, 0, 700, 0.25, 6.0)],
        "gentle-pulse" => vec![note(440.0, 0, 260, 0.45, 7.0), note(440.0, 320, 260, 0.35, 7.0)],
        "low-gong" => vec![note(196.0, 0, 1400, 0.6, 2.6), note(392.0, 0, 1000, 0.2, 3.5)],
        "rising-tone" => vec![note(523.0, 0, 220, 0.4, 5.0), note(659.0, 180, 220, 0.4, 5.0), note(784.0, 360, 500, 0.45, 4.5)],
        "double-tap" => vec![note(880.0, 0, 140, 0.5, 14.0), note(880.0, 190, 140, 0.5, 14.0)],
        "wood-tick" => vec![note(760.0, 0, 90, 0.6, 30.0)],
        // "basic-ding" and anything unknown
        _ => vec![note(880.0, 0, 800, 0.55, 5.5), note(1760.0, 0, 400, 0.12, 8.0)],
    }
}

/// Renders a catalogue sound to mono samples in -1..1.
pub fn synthesize(id: &str) -> Vec<f32> {
    let notes = recipe(id);
    let total_ms = notes.iter().map(|n| n.start_ms + n.len_ms).max().unwrap_or(0);
    let total = (SAMPLE_RATE as u64 * total_ms as u64 / 1000) as usize;
    let mut out = vec![0.0f32; total];
    for n in &notes {
        let first = (SAMPLE_RATE as u64 * n.start_ms as u64 / 1000) as usize;
        let count = (SAMPLE_RATE as u64 * n.len_ms as u64 / 1000) as usize;
        for i in 0..count {
            let Some(slot) = out.get_mut(first + i) else { break };
            let t = i as f32 / SAMPLE_RATE as f32;
            let attack = (t / 0.004).min(1.0);
            let release = (((count - i) as f32) / (SAMPLE_RATE as f32 * 0.01)).min(1.0);
            let env = attack * release * (-t * n.decay).exp();
            *slot += (2.0 * PI * n.freq * t).sin() * n.gain * env;
        }
    }
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.9 {
        for s in &mut out {
            *s *= 0.9 / peak;
        }
    }
    out
}

/// Plays catalogue sounds.
pub trait SoundPlayer {
    fn play(&self, sound_id: &str);
}

/// Plays through the default output device. Opens the device on first use and
/// keeps it open. Not `Send`: keep it on the UI thread.
#[derive(Default)]
pub struct RodioPlayer {
    sink: RefCell<Option<rodio::MixerDeviceSink>>,
}

impl RodioPlayer {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SoundPlayer for RodioPlayer {
    fn play(&self, sound_id: &str) {
        let mut slot = self.sink.borrow_mut();
        if slot.is_none() {
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut sink) => {
                    sink.log_on_drop(false);
                    *slot = Some(sink);
                }
                Err(e) => {
                    eprintln!("[Sound] No audio output: {e}");
                    return;
                }
            }
        }
        let Some(sink) = slot.as_ref() else { return };
        let samples: Vec<rodio::Sample> = synthesize(sound_id).into_iter().map(|s| s as rodio::Sample).collect();
        let buffer = rodio::buffer::SamplesBuffer::new(NonZero::new(1).unwrap(), NonZero::new(SAMPLE_RATE).unwrap(), samples);
        let player = rodio::Player::connect_new(sink.mixer());
        player.append(buffer);
        player.detach();
    }
}

// ---------------------------------------------------------------------------
// Alerts
// ---------------------------------------------------------------------------

/// Something worth telling the person about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertEvent {
    /// A regular check-in during a run ("minutes" is time worked so far).
    TimedAlert { minutes: u64 },
    /// A task run went past its estimate.
    Overtime,
    BreakStarted,
    /// A work run (task, session or sprint) began.
    WorkStarted,
    BreakEnded,
    SessionFinished,
    /// Worked long enough without a break.
    BreakReminder,
    /// A scheduled task starts soon.
    ScheduledReminder { task_id: String, title: String, mins_left: i64 },
    /// A scheduled task is due now.
    TaskDue { task_id: String, title: String },
}

/// What to do about an alert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertAction {
    pub sound: Option<String>,
    /// Show a system notification with this title and body.
    pub notification: Option<(String, String)>,
    /// Flash the window (setting `alert_flash`).
    pub flash: bool,
    /// Animate the timer pill (setting `pill_alert_animated`).
    pub animate_pill: bool,
}

/// Turns an alert into actions under the person's preferences.
pub fn plan(event: &AlertEvent, prefs: &Preferences) -> AlertAction {
    let quiet = |sound: &str| AlertAction {
        sound: Some(sound.to_string()),
        notification: None,
        flash: false,
        animate_pill: false,
    };
    let loud = |sound: &str, title: &str, body: String| AlertAction {
        sound: Some(sound.to_string()),
        notification: Some((title.to_string(), body)),
        flash: prefs.alert_flash,
        animate_pill: prefs.pill_alert_animated,
    };
    match event {
        AlertEvent::TimedAlert { minutes } => {
            loud(&prefs.alert_sound_id, "Still on it", format!("{minutes} minutes in. Keep going or take a breather."))
        }
        AlertEvent::Overtime => loud(&prefs.overtime_sound_id, "Over your estimate", "This task has run past the time you planned.".into()),
        AlertEvent::BreakStarted => quiet(&prefs.break_start_sound_id),
        AlertEvent::WorkStarted => quiet(&prefs.task_start_sound_id),
        AlertEvent::BreakEnded => loud(&prefs.notification_sound_id, "Break is over", "Ready when you are.".into()),
        AlertEvent::SessionFinished => loud(&prefs.notification_sound_id, "Session complete", "Nice work. Time to wrap up or rest.".into()),
        AlertEvent::BreakReminder => loud(&prefs.break_reminder_sound_id, "Time for a break", "You have been working for a while.".into()),
        AlertEvent::ScheduledReminder { title, mins_left, .. } => {
            let when = if *mins_left <= 1 { "in a minute".to_string() } else { format!("in {mins_left} minutes") };
            loud(&prefs.reminder_sound_id, title, format!("Starts {when}."))
        }
        AlertEvent::TaskDue { title, .. } => loud(&prefs.notification_sound_id, title, "It is time to start.".into()),
    }
}

fn is_work(purpose: TimerPurpose) -> bool {
    matches!(purpose, TimerPurpose::Task | TimerPurpose::Session | TimerPurpose::Sprint)
}

/// How long after its start time a task still counts as "due now".
const DUE_WINDOW: Duration = Duration::minutes(5);

/// Decides which alerts are due. Feed it timer ticks and, every so often,
/// the schedule. It remembers what it has already announced.
#[derive(Debug, Default)]
pub struct AlertEngine {
    last_key: Option<(TimerPurpose, Option<String>)>,
    last_elapsed: u64,
    interval_bucket: u64,
    work_secs_since_break: u64,
    announced: HashSet<(String, i64, bool)>,
}

impl AlertEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Call on each timer tick that advanced time or finished a countdown.
    pub fn on_tick(&mut self, prefs: &Preferences, snap: &TimerSnapshot, outcome: &TickOutcome) -> Vec<AlertEvent> {
        let mut events = Vec::new();
        let key = (snap.purpose, snap.task_id.clone());
        let new_run = self.last_key.as_ref() != Some(&key) || snap.elapsed_secs < self.last_elapsed;

        if new_run {
            self.interval_bucket = 0;
            if snap.purpose == TimerPurpose::Break {
                self.work_secs_since_break = 0;
                events.push(AlertEvent::BreakStarted);
            } else if is_work(snap.purpose) && snap.phase == TimerPhase::Running {
                events.push(AlertEvent::WorkStarted);
            }
        }

        if snap.phase == TimerPhase::Running && is_work(snap.purpose) {
            let interval = u64::from(prefs.alert_interval_mins.max(1)) * 60;
            let bucket = snap.elapsed_secs / interval;
            if prefs.timed_alerts && bucket > self.interval_bucket {
                self.interval_bucket = bucket;
                events.push(AlertEvent::TimedAlert { minutes: snap.elapsed_secs / 60 });
            }
            if prefs.break_reminder_mins > 0 {
                self.work_secs_since_break += outcome.advanced_secs;
                if self.work_secs_since_break >= u64::from(prefs.break_reminder_mins) * 60 {
                    self.work_secs_since_break = 0;
                    events.push(AlertEvent::BreakReminder);
                }
            }
        }

        if outcome.entered_overtime && prefs.overtime_alert {
            events.push(AlertEvent::Overtime);
        }
        if outcome.finished {
            events.push(if snap.purpose == TimerPurpose::Break {
                AlertEvent::BreakEnded
            } else {
                AlertEvent::SessionFinished
            });
        }

        self.last_key = Some(key);
        self.last_elapsed = snap.elapsed_secs;
        events
    }

    /// Call every few seconds with the current time and tasks. Fires each
    /// reminder and each due notice once.
    pub fn check_schedule(&mut self, now: DateTime<Utc>, tasks: &[Task], prefs: &Preferences) -> Vec<AlertEvent> {
        let mut events = Vec::new();
        for task in tasks.iter().filter(|t| t.status == TaskStatus::Open) {
            let Some(at) = task.scheduled_at else { continue };
            let stamp = at.timestamp();
            let mins = task.reminder_mins_before.unwrap_or(prefs.reminder_mins_before);
            if mins > 0 && now >= at - Duration::minutes(i64::from(mins)) && now < at {
                if self.announced.insert((task.id.clone(), stamp, false)) {
                    let secs_left = (at - now).num_seconds();
                    events.push(AlertEvent::ScheduledReminder {
                        task_id: task.id.clone(),
                        title: task.title.clone(),
                        mins_left: (secs_left + 59) / 60,
                    });
                }
            } else if now >= at && now < at + DUE_WINDOW && self.announced.insert((task.id.clone(), stamp, true)) {
                events.push(AlertEvent::TaskDue { task_id: task.id.clone(), title: task.title.clone() });
            }
        }
        events
    }
}

/// Shows a system notification.
pub trait SystemNotifier {
    fn notify(&self, title: &str, body: &str);
}

/// Windows toast notifications. Shown from a short-lived thread so a slow
/// notification service never stalls the interface.
pub struct ToastNotifier;

impl SystemNotifier for ToastNotifier {
    fn notify(&self, title: &str, body: &str) {
        let (title, body) = (title.to_string(), body.to_string());
        std::thread::spawn(move || {
            if let Err(e) = notify_rust::Notification::new().appname("Thulli").summary(&title).body(&body).show() {
                eprintln!("[Notify] Could not show notification: {e}");
            }
        });
    }
}

/// Carries out an alert's sound and notification. Flash and pill animation
/// are left to the caller because they touch the window.
pub fn perform(action: &AlertAction, sounds: &dyn SoundPlayer, notifier: &dyn SystemNotifier) {
    if let Some(sound) = &action.sound {
        sounds.play(sound);
    }
    if let Some((title, body)) = &action.notification {
        notifier.notify(title, body);
    }
}

// ---------------------------------------------------------------------------
// Celebration
// ---------------------------------------------------------------------------

/// What the success moment should include.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Celebration {
    /// Show the full success screen.
    pub screen: bool,
    /// Play the Slint-drawn animation.
    pub animation: bool,
    /// Artwork set id (see `CELEBRATION_SETS`).
    pub set: String,
    pub sound: Option<String>,
}

/// Plans a celebration for a finished task (`from_break` false) or a finished
/// break (`from_break` true). Returns `None` when nothing is switched on.
pub fn plan_celebration(prefs: &Preferences, over_estimate: bool, from_break: bool) -> Option<Celebration> {
    let animation = if from_break { prefs.success_animation_on_break_end } else { prefs.success_animation };
    let screen = !from_break && prefs.success_screen;
    let sound = (prefs.success_sound && !from_break).then(|| prefs.success_sound_id.clone());
    if !animation && !screen && sound.is_none() {
        return None;
    }
    let set = if over_estimate { &prefs.success_set_over_estimate } else { &prefs.success_set_on_time };
    Some(Celebration { screen, animation, set: set.clone(), sound })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TaskSection;
    use crate::services::preferences::SOUND_CHOICES;
    use crate::services::timer::TimerMode;
    use std::cell::RefCell;

    fn snap(purpose: TimerPurpose, task: Option<&str>, elapsed: u64, phase: TimerPhase) -> TimerSnapshot {
        TimerSnapshot {
            mode: TimerMode::CountUp,
            purpose,
            phase,
            planned_secs: 0,
            estimate_secs: 0,
            elapsed_secs: elapsed,
            task_id: task.map(str::to_string),
        }
    }

    fn adv(secs: u64) -> TickOutcome {
        TickOutcome { advanced_secs: secs, ..Default::default() }
    }

    #[test]
    fn every_catalogue_sound_renders_within_range() {
        for (id, _) in SOUND_CHOICES {
            let s = synthesize(id);
            assert!(s.len() > SAMPLE_RATE as usize / 20, "{id} too short");
            assert!(s.iter().all(|v| v.abs() <= 1.0), "{id} clips");
            assert!(s.iter().any(|v| v.abs() > 0.1), "{id} is silent");
        }
        assert_eq!(synthesize("unknown"), synthesize("basic-ding"));
    }

    #[test]
    fn timed_alerts_fire_once_per_interval() {
        let mut prefs = Preferences::default();
        prefs.alert_interval_mins = 10;
        let mut e = AlertEngine::new();
        let first = e.on_tick(&prefs, &snap(TimerPurpose::Task, Some("a"), 1, TimerPhase::Running), &adv(1));
        assert_eq!(first, vec![AlertEvent::WorkStarted]);
        assert!(e.on_tick(&prefs, &snap(TimerPurpose::Task, Some("a"), 599, TimerPhase::Running), &adv(1)).is_empty());
        let at10 = e.on_tick(&prefs, &snap(TimerPurpose::Task, Some("a"), 600, TimerPhase::Running), &adv(1));
        assert_eq!(at10, vec![AlertEvent::TimedAlert { minutes: 10 }]);
        assert!(e.on_tick(&prefs, &snap(TimerPurpose::Task, Some("a"), 601, TimerPhase::Running), &adv(1)).is_empty());
        let at20 = e.on_tick(&prefs, &snap(TimerPurpose::Task, Some("a"), 1200, TimerPhase::Running), &adv(1));
        assert_eq!(at20, vec![AlertEvent::TimedAlert { minutes: 20 }]);
    }

    #[test]
    fn timed_alerts_can_be_switched_off() {
        let mut prefs = Preferences::default();
        prefs.timed_alerts = false;
        prefs.alert_interval_mins = 1;
        let mut e = AlertEngine::new();
        e.on_tick(&prefs, &snap(TimerPurpose::Task, Some("a"), 1, TimerPhase::Running), &adv(1));
        let ev = e.on_tick(&prefs, &snap(TimerPurpose::Task, Some("a"), 120, TimerPhase::Running), &adv(1));
        assert!(ev.is_empty());
    }

    #[test]
    fn overtime_and_finish_events() {
        let prefs = Preferences::default();
        let mut e = AlertEngine::new();
        let ot = TickOutcome { advanced_secs: 1, entered_overtime: true, ..Default::default() };
        let ev = e.on_tick(&prefs, &snap(TimerPurpose::Task, Some("a"), 301, TimerPhase::Running), &ot);
        assert!(ev.contains(&AlertEvent::Overtime));

        let mut quiet = prefs.clone();
        quiet.overtime_alert = false;
        let mut e2 = AlertEngine::new();
        let ev2 = e2.on_tick(&quiet, &snap(TimerPurpose::Task, Some("a"), 301, TimerPhase::Running), &ot);
        assert!(!ev2.contains(&AlertEvent::Overtime));

        let done = TickOutcome { advanced_secs: 1, finished: true, ..Default::default() };
        let brk = e.on_tick(&prefs, &snap(TimerPurpose::Break, None, 300, TimerPhase::Finished), &done);
        assert!(brk.contains(&AlertEvent::BreakEnded));
        let ses = e.on_tick(&prefs, &snap(TimerPurpose::Session, None, 5400, TimerPhase::Finished), &done);
        assert!(ses.contains(&AlertEvent::SessionFinished));
    }

    #[test]
    fn break_start_and_reminder() {
        let mut prefs = Preferences::default();
        prefs.break_reminder_mins = 2;
        prefs.timed_alerts = false;
        let mut e = AlertEngine::new();
        let mut events = Vec::new();
        for s in 1..=120u64 {
            events.extend(e.on_tick(&prefs, &snap(TimerPurpose::Task, Some("a"), s, TimerPhase::Running), &adv(1)));
        }
        assert_eq!(events.iter().filter(|x| **x == AlertEvent::BreakReminder).count(), 1);
        let ev = e.on_tick(&prefs, &snap(TimerPurpose::Break, None, 1, TimerPhase::Running), &adv(1));
        assert_eq!(ev, vec![AlertEvent::BreakStarted]);
    }

    #[test]
    fn restarting_a_run_counts_as_new() {
        let prefs = Preferences::default();
        let mut e = AlertEngine::new();
        e.on_tick(&prefs, &snap(TimerPurpose::Task, Some("a"), 500, TimerPhase::Running), &adv(1));
        let ev = e.on_tick(&prefs, &snap(TimerPurpose::Task, Some("a"), 1, TimerPhase::Running), &adv(1));
        assert_eq!(ev, vec![AlertEvent::WorkStarted]);
    }

    fn scheduled(at: DateTime<Utc>) -> Task {
        let mut t = Task::new("l", "Standup", 15, TaskSection::Today);
        t.scheduled_at = Some(at);
        t
    }

    #[test]
    fn scheduled_reminder_and_due_fire_once() {
        let prefs = Preferences::default(); // 5 minutes before
        let at = Utc::now() + Duration::minutes(30);
        let tasks = vec![scheduled(at)];
        let mut e = AlertEngine::new();
        assert!(e.check_schedule(at - Duration::minutes(10), &tasks, &prefs).is_empty());
        let r = e.check_schedule(at - Duration::minutes(4), &tasks, &prefs);
        assert!(matches!(r.as_slice(), [AlertEvent::ScheduledReminder { mins_left: 4, .. }]));
        assert!(e.check_schedule(at - Duration::minutes(3), &tasks, &prefs).is_empty());
        let d = e.check_schedule(at + Duration::seconds(5), &tasks, &prefs);
        assert!(matches!(d.as_slice(), [AlertEvent::TaskDue { .. }]));
        assert!(e.check_schedule(at + Duration::seconds(10), &tasks, &prefs).is_empty());
        // long past: nothing
        let mut e2 = AlertEngine::new();
        assert!(e2.check_schedule(at + Duration::hours(2), &tasks, &prefs).is_empty());
    }

    #[test]
    fn per_task_reminder_overrides_and_done_tasks_are_skipped() {
        let prefs = Preferences::default();
        let at = Utc::now() + Duration::minutes(60);
        let mut t = scheduled(at);
        t.reminder_mins_before = Some(15);
        let mut e = AlertEngine::new();
        assert_eq!(e.check_schedule(at - Duration::minutes(14), &[t.clone()], &prefs).len(), 1);
        let mut done = scheduled(at);
        done.status = TaskStatus::Completed;
        assert!(AlertEngine::new().check_schedule(at - Duration::minutes(2), &[done], &prefs).is_empty());
        let mut zero = prefs.clone();
        zero.reminder_mins_before = 0;
        assert!(AlertEngine::new().check_schedule(at - Duration::minutes(2), &[scheduled(at)], &zero).is_empty());
    }

    #[test]
    fn plan_uses_the_chosen_sounds() {
        let mut prefs = Preferences::default();
        prefs.overtime_sound_id = "low-gong".into();
        prefs.alert_flash = true;
        let a = plan(&AlertEvent::Overtime, &prefs);
        assert_eq!(a.sound.as_deref(), Some("low-gong"));
        assert!(a.flash && a.animate_pill && a.notification.is_some());
        prefs.pill_alert_animated = false;
        assert!(!plan(&AlertEvent::Overtime, &prefs).animate_pill);
        let b = plan(&AlertEvent::BreakStarted, &prefs);
        assert!(b.notification.is_none());
        assert_eq!(b.sound, Some(prefs.break_start_sound_id.clone()));
    }

    struct Recorder(RefCell<Vec<String>>);
    impl SoundPlayer for Recorder {
        fn play(&self, id: &str) {
            self.0.borrow_mut().push(format!("sound:{id}"));
        }
    }
    impl SystemNotifier for Recorder {
        fn notify(&self, title: &str, _body: &str) {
            self.0.borrow_mut().push(format!("note:{title}"));
        }
    }

    #[test]
    fn perform_plays_and_notifies() {
        let rec = Recorder(RefCell::new(Vec::new()));
        let action = plan(&AlertEvent::BreakReminder, &Preferences::default());
        perform(&action, &rec, &rec);
        assert_eq!(rec.0.borrow().as_slice(), ["sound:soft-chime", "note:Time for a break"]);
    }

    #[test]
    fn celebration_follows_settings() {
        let mut prefs = Preferences::default();
        let c = plan_celebration(&prefs, false, false).unwrap();
        assert!(c.screen && c.animation);
        assert_eq!(c.set, "sunrise");
        assert_eq!(plan_celebration(&prefs, true, false).unwrap().set, "steady");
        assert_eq!(c.sound.as_deref(), Some("soft-chime"));
        // break end: only the animation, and only when enabled
        assert!(plan_celebration(&prefs, false, true).is_none());
        prefs.success_animation_on_break_end = true;
        let b = plan_celebration(&prefs, false, true).unwrap();
        assert!(b.animation && !b.screen && b.sound.is_none());
        prefs.success_screen = false;
        prefs.success_animation = false;
        prefs.success_sound = false;
        assert!(plan_celebration(&prefs, false, false).is_none());
    }
}
