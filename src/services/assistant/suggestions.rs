//! In-app suggestions: plan my day, welcome back, take a break, wind down,
//! prioritize. They appear only inside Thulli and never while a focus session
//! is running.

use chrono::{DateTime, Duration, Local, Timelike};

use crate::models::{AppStateStore, SuggestionKind};

use super::prompt::{done_on, focus_mins_on, open_today};

/// A prompt shown in the assistant screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub kind: SuggestionKind,
    pub title: String,
    pub body: String,
}

/// How long a dismissed or accepted suggestion stays quiet.
const DISMISS_HOURS: i64 = 6;
const ACCEPT_HOURS: i64 = 12;
/// At most this many suggestions are shown at once.
const MAX_SHOWN: usize = 2;
/// Focus minutes in a day after which a break is suggested.
const BREAK_AFTER_MINS: u64 = 90;
/// Days away after which "welcome back" is shown.
const AWAY_DAYS: i64 = 3;

pub fn kind_id(kind: SuggestionKind) -> &'static str {
    match kind {
        SuggestionKind::PlanMyDay => "plan_my_day",
        SuggestionKind::WelcomeBack => "welcome_back",
        SuggestionKind::TakeABreak => "take_a_break",
        SuggestionKind::WindDown => "wind_down",
        SuggestionKind::Prioritize => "prioritize",
    }
}

pub fn parse_kind(id: &str) -> Option<SuggestionKind> {
    Some(match id {
        "plan_my_day" => SuggestionKind::PlanMyDay,
        "welcome_back" => SuggestionKind::WelcomeBack,
        "take_a_break" => SuggestionKind::TakeABreak,
        "wind_down" => SuggestionKind::WindDown,
        "prioritize" => SuggestionKind::Prioritize,
        _ => return None,
    })
}

fn enabled(store: &AppStateStore, kind: SuggestionKind) -> bool {
    let t = &store.preferences.suggestions;
    match kind {
        SuggestionKind::PlanMyDay => t.plan_my_day,
        SuggestionKind::WelcomeBack => t.welcome_back,
        SuggestionKind::TakeABreak => t.take_a_break,
        SuggestionKind::WindDown => t.wind_down,
        SuggestionKind::Prioritize => t.prioritize,
    }
}

fn snoozed(store: &AppStateStore, kind: SuggestionKind, now: DateTime<Local>) -> bool {
    store
        .suggestion_states
        .iter()
        .find(|s| s.kind == kind)
        .is_some_and(|s| s.is_snoozed(now.with_timezone(&chrono::Utc)))
}

fn last_activity(store: &AppStateStore) -> Option<DateTime<chrono::Utc>> {
    let entries = store.time_entries.iter().map(|e| e.ended_at);
    let tasks = store.tasks.iter().filter_map(|t| t.last_worked_at.or(t.completed_at));
    entries.chain(tasks).max()
}

/// Suggestions that apply right now. Empty while focusing.
pub fn compute(store: &AppStateStore, now: DateTime<Local>, focus_active: bool) -> Vec<Suggestion> {
    if focus_active {
        return Vec::new();
    }
    let today = now.date_naive();
    let hour = now.hour();
    let done = done_on(store, today).len();
    let mins = focus_mins_on(store, today);
    let day = store.day_plan(today);

    let mut out = Vec::new();
    let mut push = |kind: SuggestionKind, title: &str, body: &str| {
        if enabled(store, kind) && !snoozed(store, kind, now) {
            out.push(Suggestion { kind, title: title.to_string(), body: body.to_string() });
        }
    };

    if hour >= 17 && (done > 0 || mins > 0) && !day.is_some_and(|d| d.wrapped_up) {
        push(SuggestionKind::WindDown, "Ready to wind down?", "Look back on today and set up a calm finish.");
    }
    if mins >= BREAK_AFTER_MINS {
        push(SuggestionKind::TakeABreak, "Time for a break", "You have focused for a good while. A short rest helps.");
    }
    if open_today(store) >= 5 {
        push(SuggestionKind::Prioritize, "A full day ahead", "Want help choosing what to do first?");
    }
    let away = last_activity(store).is_some_and(|t| now.with_timezone(&chrono::Utc) - t >= Duration::days(AWAY_DAYS));
    if away {
        push(SuggestionKind::WelcomeBack, "Welcome back", "Want a quick catch-up on where things stand?");
    }
    if hour < 12 && !day.is_some_and(|d| d.planned) {
        push(SuggestionKind::PlanMyDay, "Plan your day", "Pick what matters most for today.");
    }
    out.truncate(MAX_SHOWN);
    out
}

/// The message sent to the assistant when a suggestion is accepted.
pub fn accept_prompt(kind: SuggestionKind) -> Option<&'static str> {
    match kind {
        SuggestionKind::PlanMyDay => Some("Help me plan my day."),
        SuggestionKind::WelcomeBack => Some("Catch me up on where things stand."),
        SuggestionKind::WindDown => Some("Wind-down Reflection: help me look back on today."),
        SuggestionKind::Prioritize => Some("Quick Prioritize: what should I do first?"),
        SuggestionKind::TakeABreak => None,
    }
}

pub fn dismiss_duration() -> Duration {
    Duration::hours(DISMISS_HOURS)
}

pub fn accept_duration() -> Duration {
    Duration::hours(ACCEPT_HOURS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SuggestionState, Task, TaskSection, TimeEntry, TimeEntryKind};
    use chrono::TimeZone;

    fn at(h: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 5, 4, h, 0, 0).unwrap()
    }

    #[test]
    fn nothing_is_shown_during_focus() {
        let store = AppStateStore::default();
        assert!(!compute(&store, at(9), false).is_empty());
        assert!(compute(&store, at(9), true).is_empty());
    }

    #[test]
    fn morning_suggests_planning_and_toggles_switch_it_off() {
        let mut store = AppStateStore::default();
        assert_eq!(compute(&store, at(9), false)[0].kind, SuggestionKind::PlanMyDay);
        store.preferences.suggestions.plan_my_day = false;
        assert!(compute(&store, at(9), false).is_empty());
    }

    #[test]
    fn evening_after_work_suggests_wind_down() {
        let mut store = AppStateStore::default();
        let mut t = Task::new("l", "x", 0, TaskSection::Today);
        t.complete();
        t.completed_at = Some(at(16).with_timezone(&chrono::Utc));
        store.tasks.push(t);
        let kinds: Vec<_> = compute(&store, at(18), false).iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&SuggestionKind::WindDown));
        assert!(compute(&store, at(14), false).iter().all(|s| s.kind != SuggestionKind::WindDown));
    }

    #[test]
    fn long_focus_suggests_a_break() {
        let mut store = AppStateStore::default();
        let start = at(10).with_timezone(&chrono::Utc);
        store.time_entries.push(TimeEntry::new(TimeEntryKind::Focus, start, start + Duration::minutes(95)));
        assert!(compute(&store, at(13), false).iter().any(|s| s.kind == SuggestionKind::TakeABreak));
    }

    #[test]
    fn many_open_tasks_suggest_prioritizing() {
        let mut store = AppStateStore::default();
        for i in 0..5 {
            store.tasks.push(Task::new("l", format!("t{i}"), 0, TaskSection::Today));
        }
        assert!(compute(&store, at(14), false).iter().any(|s| s.kind == SuggestionKind::Prioritize));
    }

    #[test]
    fn dismissed_suggestions_stay_quiet() {
        let mut store = AppStateStore::default();
        let mut st = SuggestionState::new(SuggestionKind::PlanMyDay);
        st.dismiss_until((at(9) + dismiss_duration()).with_timezone(&chrono::Utc));
        store.suggestion_states.push(st);
        assert!(compute(&store, at(9), false).is_empty());
    }

    #[test]
    fn kind_ids_round_trip() {
        for k in [
            SuggestionKind::PlanMyDay,
            SuggestionKind::WelcomeBack,
            SuggestionKind::TakeABreak,
            SuggestionKind::WindDown,
            SuggestionKind::Prioritize,
        ] {
            assert_eq!(parse_kind(kind_id(k)), Some(k));
        }
        assert_eq!(parse_kind("nope"), None);
    }
}
