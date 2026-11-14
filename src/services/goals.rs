//! Goals: create and edit goals, manage milestones, link existing tasks and
//! read progress and focus time back out.
//!
//! Tasks are never copied. A task carries `goal_id` (and optionally
//! `milestone_id`), so a task belongs to at most one goal and there can be no
//! duplicate links. Removing a goal or milestone only clears those fields.
//! Focus time comes from the same spans Insights uses, so a session is never
//! counted twice.

use std::sync::{Arc, Mutex};

use chrono::{Duration, NaiveDate, NaiveDateTime, Utc};

use crate::models::{AppStateStore, Goal, GoalPriority, GoalStatus, Milestone, ProgressMode, Task, TimeEntryKind};
use crate::repository::StorageRepository;
use crate::services::insights::collect_spans;
use crate::services::store_handle::StoreHandle;

const MAX_TITLE: usize = 120;
const MAX_TEXT: usize = 2000;
const MAX_CATEGORY: usize = 40;
const MAX_UNIT: usize = 24;
/// Most milestones one goal can hold.
pub const MAX_MILESTONES: usize = 60;

/// What the person typed into the goal form.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalDraft {
    pub title: String,
    pub description: String,
    pub motivation: String,
    pub category: String,
    pub notes: String,
    pub priority: GoalPriority,
    pub mode: ProgressMode,
    pub target_date: Option<NaiveDate>,
    pub current_value: f64,
    pub target_value: f64,
    pub unit: String,
    /// Measured goals: count logged focus hours instead of a typed value.
    pub track_focus: bool,
}

impl GoalDraft {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            description: String::new(),
            motivation: String::new(),
            category: String::new(),
            notes: String::new(),
            priority: GoalPriority::Medium,
            mode: ProgressMode::Milestones,
            target_date: None,
            current_value: 0.0,
            target_value: 0.0,
            unit: String::new(),
            track_focus: false,
        }
    }

    /// Checks the draft and returns a trimmed copy, or a message for the person.
    pub fn validated(&self) -> Result<GoalDraft, String> {
        let mut d = self.clone();
        d.title = d.title.trim().to_string();
        d.description = d.description.trim().to_string();
        d.motivation = d.motivation.trim().to_string();
        d.category = d.category.trim().to_string();
        d.notes = d.notes.trim().to_string();
        d.unit = d.unit.trim().to_string();
        if d.title.is_empty() {
            return Err("Give the goal a title.".to_string());
        }
        if d.title.chars().count() > MAX_TITLE {
            return Err(format!("Keep the title under {MAX_TITLE} characters."));
        }
        for text in [&d.description, &d.motivation, &d.notes] {
            if text.chars().count() > MAX_TEXT {
                return Err(format!("Keep each text under {MAX_TEXT} characters."));
            }
        }
        if d.category.chars().count() > MAX_CATEGORY {
            return Err(format!("Keep the category under {MAX_CATEGORY} characters."));
        }
        if d.unit.chars().count() > MAX_UNIT {
            return Err(format!("Keep the unit under {MAX_UNIT} characters."));
        }
        if d.mode == ProgressMode::Measured {
            if !d.target_value.is_finite() || d.target_value <= 0.0 {
                return Err("Set a target above zero.".to_string());
            }
            if !d.current_value.is_finite() || d.current_value < 0.0 {
                return Err("The current value cannot be negative.".to_string());
            }
        }
        Ok(d)
    }
}

/// Which goals the dashboard shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalFilter {
    /// Active and paused goals: everything still in play.
    InPlay,
    Active,
    Paused,
    Completed,
    Archived,
}

impl GoalFilter {
    pub fn parse(value: &str) -> Self {
        match value {
            "active" => GoalFilter::Active,
            "paused" => GoalFilter::Paused,
            "completed" => GoalFilter::Completed,
            "archived" => GoalFilter::Archived,
            _ => GoalFilter::InPlay,
        }
    }

    fn keeps(&self, status: GoalStatus) -> bool {
        match self {
            GoalFilter::InPlay => matches!(status, GoalStatus::Active | GoalStatus::Paused),
            GoalFilter::Active => status == GoalStatus::Active,
            GoalFilter::Paused => status == GoalStatus::Paused,
            GoalFilter::Completed => status == GoalStatus::Completed,
            GoalFilter::Archived => status == GoalStatus::Archived,
        }
    }
}

/// How the dashboard orders goals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalSort {
    Priority,
    Deadline,
    Progress,
    Recent,
}

impl GoalSort {
    pub fn parse(value: &str) -> Self {
        match value {
            "deadline" => GoalSort::Deadline,
            "progress" => GoalSort::Progress,
            "recent" => GoalSort::Recent,
            _ => GoalSort::Priority,
        }
    }
}

/// Goals matching `filter` and the search text, ordered by `sort`.
pub fn list_goals<'a>(store: &'a AppStateStore, filter: GoalFilter, sort: GoalSort, search: &str) -> Vec<&'a Goal> {
    let needle = search.trim().to_lowercase();
    let mut goals: Vec<&Goal> = store
        .goals
        .iter()
        .filter(|g| filter.keeps(g.status))
        .filter(|g| needle.is_empty() || g.title.to_lowercase().contains(&needle) || g.category.to_lowercase().contains(&needle))
        .collect();
    match sort {
        GoalSort::Priority => goals.sort_by(|a, b| {
            // Same priority: dated goals first, soonest first.
            a.priority.cmp(&b.priority).then(a.target_date.is_none().cmp(&b.target_date.is_none())).then(a.target_date.cmp(&b.target_date))
        }),
        // Goals with a date first, soonest first; undated ones last.
        GoalSort::Deadline => goals.sort_by_key(|g| (g.target_date.is_none(), g.target_date)),
        GoalSort::Progress => goals.sort_by(|a, b| {
            let (pa, pb) = (a.progress().percent.unwrap_or(-1.0), b.progress().percent.unwrap_or(-1.0));
            pb.partial_cmp(&pa).unwrap_or(std::cmp::Ordering::Equal)
        }),
        GoalSort::Recent => goals.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
    }
    goals
}

/// Counts for the dashboard header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GoalCounts {
    pub active: usize,
    pub paused: usize,
    pub completed: usize,
    pub overdue: usize,
}

pub fn counts(store: &AppStateStore, today: NaiveDate) -> GoalCounts {
    let mut c = GoalCounts::default();
    for g in &store.goals {
        match g.status {
            GoalStatus::Active => c.active += 1,
            GoalStatus::Paused => c.paused += 1,
            GoalStatus::Completed => c.completed += 1,
            GoalStatus::Archived => {}
        }
        if g.is_overdue(today) {
            c.overdue += 1;
        }
    }
    c
}

/// The top-level task a task or breakdown step belongs to.
fn root_task<'a>(store: &'a AppStateStore, mut task: &'a Task) -> &'a Task {
    // The step chain is short; the cap only guards against a corrupt loop.
    for _ in 0..64 {
        match task.parent_id.as_deref().and_then(|p| store.task(p)) {
            Some(parent) => task = parent,
            None => break,
        }
    }
    task
}

/// Top-level tasks linked to a goal, in saved order.
pub fn linked_tasks<'a>(store: &'a AppStateStore, goal_id: &str) -> Vec<&'a Task> {
    store.tasks.iter().filter(|t| !t.is_step() && t.goal_id.as_deref() == Some(goal_id)).collect()
}

/// Linked tasks done over linked tasks in total.
pub fn task_progress(store: &AppStateStore, goal_id: &str) -> (usize, usize) {
    let tasks = linked_tasks(store, goal_id);
    let done = tasks.iter().filter(|t| !t.is_pending() && t.status == crate::models::TaskStatus::Completed).count();
    (done, tasks.len())
}

/// Focus spans that belong to a goal: Focus-kind time on its linked tasks or
/// their steps. Uses the Insights span list, so nothing is counted twice.
fn goal_spans(store: &AppStateStore, goal_id: &str) -> Vec<crate::services::insights::Span> {
    collect_spans(store)
        .into_iter()
        .filter(|s| s.kind == TimeEntryKind::Focus)
        .filter(|s| {
            s.task_id
                .as_deref()
                .and_then(|id| store.task(id))
                .map(|t| root_task(store, t))
                .is_some_and(|root| root.goal_id.as_deref() == Some(goal_id))
        })
        .collect()
}

/// The goal a focus span counts toward: the goal of its top-level task.
fn span_goal<'a>(store: &'a AppStateStore, task_id: Option<&str>) -> Option<&'a str> {
    let task = store.task(task_id?)?;
    root_task(store, task).goal_id.as_deref()
}

/// Focus seconds for every goal at once: `(total, last 7 days)`. Reads the
/// span list once, so the dashboard stays quick with many goals.
pub fn focus_summary(store: &AppStateStore, today: NaiveDate) -> std::collections::HashMap<String, (u64, u64)> {
    let from = (today - Duration::days(6)).and_hms_opt(0, 0, 0).unwrap_or_default();
    let to = (today + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap_or_default();
    let mut out: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();
    for span in collect_spans(store).iter().filter(|s| s.kind == TimeEntryKind::Focus) {
        if let Some(goal) = span_goal(store, span.task_id.as_deref()) {
            let entry = out.entry(goal.to_string()).or_default();
            entry.0 += span.duration_secs();
            entry.1 += span.secs_between(from, to);
        }
    }
    out
}

/// Why a goal appears in a weekly review list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewReason {
    /// Milestones finished and tasks done in the window.
    Progressed { milestones: usize, tasks: usize },
    /// Target date coming up (days from today) or already past (negative).
    Deadline { days: i64 },
    /// An open milestone with a date inside the next seven days.
    MilestoneDue { title: String, days: i64 },
    /// No milestone, task or focus activity for this many days.
    Quiet { days: i64 },
    /// No date set: what to work on next, if a milestone is open.
    NextUp { milestone: Option<String> },
}

/// One goal in a review list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewItem {
    pub goal_id: String,
    pub title: String,
    pub reason: ReviewReason,
}

/// The last seven days across all goals, built only from saved records.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WeeklyReview {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub milestones_done: usize,
    pub tasks_done: usize,
    pub focus_secs: u64,
    pub progressed: Vec<ReviewItem>,
    pub upcoming: Vec<ReviewItem>,
    pub attention: Vec<ReviewItem>,
    /// What to put first next week: overdue goals, then those due soonest,
    /// then high priority ones. At most three.
    pub priorities: Vec<ReviewItem>,
}

/// Days without any activity before an active goal counts as quiet.
pub const QUIET_DAYS: i64 = 14;

/// Builds the review for the seven days ending on `today`.
pub fn weekly_review(store: &AppStateStore, today: NaiveDate) -> WeeklyReview {
    let from = today - Duration::days(6);
    let from_dt = from.and_hms_opt(0, 0, 0).unwrap_or_default();
    let to_dt = (today + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap_or_default();
    let local_day = |dt: chrono::DateTime<Utc>| crate::services::insights::to_local(dt).date();
    let in_window = |d: NaiveDate| d >= from && d <= today;

    // Focus per goal in the window, and the last day each goal had focus.
    let mut focus: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    let mut last_focus: std::collections::HashMap<&str, NaiveDate> = std::collections::HashMap::new();
    let spans = collect_spans(store);
    for s in spans.iter().filter(|s| s.kind == TimeEntryKind::Focus) {
        if let Some(goal) = span_goal(store, s.task_id.as_deref()) {
            *focus.entry(goal).or_default() += s.secs_between(from_dt, to_dt);
            let e = last_focus.entry(goal).or_insert(s.end.date());
            *e = (*e).max(s.end.date());
        }
    }

    let mut review = WeeklyReview { from, to: today, ..Default::default() };
    review.focus_secs = focus.values().sum();

    let live = |g: &&Goal| matches!(g.status, GoalStatus::Active | GoalStatus::Paused | GoalStatus::Completed);
    for g in store.goals.iter().filter(live) {
        let milestones = g.milestones.iter().filter(|m| m.completed_at.is_some_and(|d| in_window(local_day(d)))).count();
        let tasks = store
            .tasks
            .iter()
            .filter(|t| !t.is_step() && t.goal_id.as_deref() == Some(g.id.as_str()))
            .filter(|t| t.completed_at.is_some_and(|d| in_window(local_day(d))))
            .count();
        review.milestones_done += milestones;
        review.tasks_done += tasks;
        if milestones + tasks > 0 || focus.get(g.id.as_str()).copied().unwrap_or(0) > 0 {
            review.progressed.push(ReviewItem { goal_id: g.id.clone(), title: g.title.clone(), reason: ReviewReason::Progressed { milestones, tasks } });
        }
    }

    for g in store.goals.iter().filter(|g| g.status == GoalStatus::Active) {
        if let Some(d) = g.target_date {
            let days = (d - today).num_days();
            if days <= 7 {
                review.upcoming.push(ReviewItem { goal_id: g.id.clone(), title: g.title.clone(), reason: ReviewReason::Deadline { days } });
            }
        }
        if let Some((m, d)) = g.open_milestones().filter_map(|m| m.target_date.map(|d| (m, d))).min_by_key(|(_, d)| *d) {
            let days = (d - today).num_days();
            if (0..=7).contains(&days) {
                review.upcoming.push(ReviewItem { goal_id: g.id.clone(), title: g.title.clone(), reason: ReviewReason::MilestoneDue { title: m.title.clone(), days } });
            }
        }
        // Quiet: nothing finished, no focus, and not touched for two weeks.
        let last_touch = [
            Some(local_day(g.updated_at)),
            last_focus.get(g.id.as_str()).copied(),
            g.milestones.iter().filter_map(|m| m.completed_at).map(local_day).max(),
        ]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or_else(|| local_day(g.created_at));
        let quiet = (today - last_touch).num_days();
        if quiet >= QUIET_DAYS {
            review.attention.push(ReviewItem { goal_id: g.id.clone(), title: g.title.clone(), reason: ReviewReason::Quiet { days: quiet } });
        }
    }
    // Overdue goals need attention too, most overdue first.
    let mut overdue: Vec<ReviewItem> = review
        .upcoming
        .iter()
        .filter(|i| matches!(i.reason, ReviewReason::Deadline { days } if days < 0))
        .cloned()
        .collect();
    overdue.sort_by_key(|i| match i.reason {
        ReviewReason::Deadline { days } => days,
        _ => 0,
    });
    for item in overdue.into_iter().rev() {
        review.attention.insert(0, item);
    }
    review.upcoming.sort_by_key(|i| match i.reason {
        ReviewReason::Deadline { days } | ReviewReason::MilestoneDue { days, .. } => days,
        _ => i64::MAX,
    });

    // Priorities: overdue, then soonest deadline, then high priority, then the rest.
    let mut ranked: Vec<(&Goal, (i64, GoalPriority))> = store
        .goals
        .iter()
        .filter(|g| g.status == GoalStatus::Active)
        .map(|g| (g, (g.target_date.map_or(i64::MAX, |d| (d - today).num_days()), g.priority)))
        .collect();
    ranked.sort_by(|a, b| a.1.cmp(&b.1));
    review.priorities = ranked
        .into_iter()
        .take(3)
        .map(|(g, (days, _))| ReviewItem {
            goal_id: g.id.clone(),
            title: g.title.clone(),
            reason: if days == i64::MAX {
                ReviewReason::NextUp { milestone: g.next_milestone().map(|m| m.title.clone()) }
            } else {
                ReviewReason::Deadline { days }
            },
        })
        .collect();
    review
}

/// Total recorded focus seconds on a goal.
pub fn focus_secs(store: &AppStateStore, goal_id: &str) -> u64 {
    goal_spans(store, goal_id).iter().map(|s| s.duration_secs()).sum()
}

/// Focus seconds on a goal inside `[from, to)` (local time).
pub fn focus_secs_between(store: &AppStateStore, goal_id: &str, from: NaiveDateTime, to: NaiveDateTime) -> u64 {
    goal_spans(store, goal_id).iter().map(|s| s.secs_between(from, to)).sum()
}

/// Focus seconds per day for the `days` days ending on `today`, oldest first.
pub fn focus_by_day(store: &AppStateStore, goal_id: &str, today: NaiveDate, days: i64) -> Vec<(NaiveDate, u64)> {
    let spans = goal_spans(store, goal_id);
    (0..days)
        .rev()
        .map(|back| {
            let day = today - Duration::days(back);
            (day, spans.iter().map(|s| s.secs_on(day)).sum())
        })
        .collect()
}

/// One recorded focus session on a goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalSession {
    pub start: NaiveDateTime,
    pub secs: u64,
    pub task_title: String,
}

/// The latest `limit` focus sessions on a goal, newest first.
pub fn recent_sessions(store: &AppStateStore, goal_id: &str, limit: usize) -> Vec<GoalSession> {
    let mut spans = goal_spans(store, goal_id);
    spans.sort_by(|a, b| b.start.cmp(&a.start));
    spans
        .into_iter()
        .take(limit)
        .map(|s| GoalSession {
            start: s.start,
            secs: s.duration_secs(),
            task_title: s.task_id.as_deref().and_then(|id| store.task(id)).map(|t| t.title.clone()).unwrap_or_default(),
        })
        .collect()
}

/// Service managing goals, milestones and their task links.
pub struct GoalService {
    handle: StoreHandle,
}

impl GoalService {
    pub fn new(store: Arc<Mutex<AppStateStore>>, repository: Arc<dyn StorageRepository>) -> Self {
        Self { handle: StoreHandle::new(store, repository) }
    }

    pub fn from_handle(handle: StoreHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }

    fn apply(goal: &mut Goal, d: &GoalDraft) {
        goal.title = d.title.clone();
        goal.description = d.description.clone();
        goal.motivation = d.motivation.clone();
        goal.category = d.category.clone();
        goal.notes = d.notes.clone();
        goal.priority = d.priority;
        goal.progress_mode = d.mode;
        goal.target_date = d.target_date;
        goal.unit = d.unit.clone();
        goal.track_focus = d.track_focus && d.mode == ProgressMode::Measured;
        if goal.track_focus {
            goal.unit = "h".to_string();
        }
        if d.mode == ProgressMode::Measured {
            goal.current_value = d.current_value;
            goal.target_value = d.target_value;
        }
    }

    /// Brings goals that count focus hours up to date with the logged time.
    /// Returns true when something changed.
    pub fn sync_focus_hours(&self) -> bool {
        let updates: Vec<(String, f64)> = self.handle.read(|s| {
            s.goals
                .iter()
                .filter(|g| g.track_focus && g.progress_mode == ProgressMode::Measured)
                .map(|g| (g.id.clone(), ((focus_secs(s, &g.id) as f64 / 3600.0) * 10.0).round() / 10.0))
                .filter(|(id, h)| s.goals.iter().any(|g| g.id == *id && (g.current_value - h).abs() > 1e-9))
                .collect()
        });
        if updates.is_empty() {
            return false;
        }
        self.handle.mutate(|s| {
            for (id, h) in &updates {
                if let Some(g) = s.goals.iter_mut().find(|g| g.id == *id) {
                    g.current_value = *h;
                }
            }
        });
        true
    }

    pub fn create(&self, draft: &GoalDraft) -> Result<Goal, String> {
        let d = draft.validated()?;
        let mut goal = Goal::new(d.title.clone());
        Self::apply(&mut goal, &d);
        self.handle.mutate(|st| st.goals.push(goal.clone()));
        Ok(goal)
    }

    pub fn update(&self, goal_id: &str, draft: &GoalDraft) -> Result<(), String> {
        let d = draft.validated()?;
        self.handle.mutate(|st| {
            let goal = st.goals.iter_mut().find(|g| g.id == goal_id).ok_or("That goal no longer exists.")?;
            Self::apply(goal, &d);
            goal.record("Edited");
            Ok(())
        })
    }

    pub fn set_status(&self, goal_id: &str, status: GoalStatus) -> Result<(), String> {
        self.handle.mutate(|st| {
            let goal = st.goals.iter_mut().find(|g| g.id == goal_id).ok_or("That goal no longer exists.")?;
            goal.set_status(status);
            Ok(())
        })
    }

    /// Removes a goal. Its tasks stay; they just stop pointing at it.
    pub fn delete(&self, goal_id: &str) -> Result<(), String> {
        self.handle.mutate(|st| {
            let before = st.goals.len();
            st.goals.retain(|g| g.id != goal_id);
            if st.goals.len() == before {
                return Err("That goal no longer exists.".to_string());
            }
            for t in st.tasks.iter_mut().filter(|t| t.goal_id.as_deref() == Some(goal_id)) {
                t.goal_id = None;
                t.milestone_id = None;
            }
            Ok(())
        })
    }

    /// Measured goals: sets where the person is now.
    pub fn set_current_value(&self, goal_id: &str, value: f64) -> Result<(), String> {
        if !value.is_finite() || value < 0.0 {
            return Err("The current value cannot be negative.".to_string());
        }
        self.handle.mutate(|st| {
            let goal = st.goals.iter_mut().find(|g| g.id == goal_id).ok_or("That goal no longer exists.")?;
            if goal.progress_mode != ProgressMode::Measured {
                return Err("This goal counts milestones, not a number.".to_string());
            }
            goal.current_value = value;
            goal.record(format!("Progress set to {value}"));
            Ok(())
        })
    }

    // ----- milestones -----

    pub fn add_milestone(&self, goal_id: &str, title: &str, target_date: Option<NaiveDate>, description: &str) -> Result<Milestone, String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("Give the milestone a title.".to_string());
        }
        if title.chars().count() > MAX_TITLE {
            return Err(format!("Keep the title under {MAX_TITLE} characters."));
        }
        self.handle.mutate(|st| {
            let goal = st.goals.iter_mut().find(|g| g.id == goal_id).ok_or("That goal no longer exists.")?;
            if goal.milestones.len() >= MAX_MILESTONES {
                return Err(format!("A goal can hold up to {MAX_MILESTONES} milestones."));
            }
            let mut m = Milestone::new(title);
            m.target_date = target_date;
            m.description = description.trim().chars().take(MAX_TEXT).collect();
            goal.milestones.push(m.clone());
            goal.renumber_milestones();
            goal.record(format!("Added milestone \"{title}\""));
            Ok(m)
        })
    }

    pub fn edit_milestone(&self, goal_id: &str, milestone_id: &str, title: &str, target_date: Option<NaiveDate>, description: &str) -> Result<(), String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("Give the milestone a title.".to_string());
        }
        if title.chars().count() > MAX_TITLE {
            return Err(format!("Keep the title under {MAX_TITLE} characters."));
        }
        self.handle.mutate(|st| {
            let goal = st.goals.iter_mut().find(|g| g.id == goal_id).ok_or("That goal no longer exists.")?;
            let m = goal.milestone_mut(milestone_id).ok_or("That milestone no longer exists.")?;
            m.title = title.to_string();
            m.target_date = target_date;
            m.description = description.trim().chars().take(MAX_TEXT).collect();
            goal.record("Edited a milestone");
            Ok(())
        })
    }

    /// Removes a milestone; tasks pinned to it stay linked to the goal.
    pub fn delete_milestone(&self, goal_id: &str, milestone_id: &str) -> Result<(), String> {
        self.handle.mutate(|st| {
            let goal = st.goals.iter_mut().find(|g| g.id == goal_id).ok_or("That goal no longer exists.")?;
            let before = goal.milestones.len();
            goal.milestones.retain(|m| m.id != milestone_id);
            if goal.milestones.len() == before {
                return Err("That milestone no longer exists.".to_string());
            }
            goal.renumber_milestones();
            goal.record("Removed a milestone");
            for t in st.tasks.iter_mut().filter(|t| t.milestone_id.as_deref() == Some(milestone_id)) {
                t.milestone_id = None;
            }
            Ok(())
        })
    }

    /// Moves a milestone up (`-1`) or down (`1`) one place.
    pub fn move_milestone(&self, goal_id: &str, milestone_id: &str, delta: i32) -> Result<(), String> {
        self.handle.mutate(|st| {
            let goal = st.goals.iter_mut().find(|g| g.id == goal_id).ok_or("That goal no longer exists.")?;
            let from = goal.milestones.iter().position(|m| m.id == milestone_id).ok_or("That milestone no longer exists.")?;
            let to = (from as i64 + delta as i64).clamp(0, goal.milestones.len() as i64 - 1) as usize;
            if to != from {
                let m = goal.milestones.remove(from);
                goal.milestones.insert(to, m);
                goal.renumber_milestones();
                goal.updated_at = Utc::now();
            }
            Ok(())
        })
    }

    /// Ticks a milestone on or off. Finishing tasks never does this by itself.
    pub fn set_milestone_done(&self, goal_id: &str, milestone_id: &str, done: bool) -> Result<(), String> {
        self.handle.mutate(|st| {
            let goal = st.goals.iter_mut().find(|g| g.id == goal_id).ok_or("That goal no longer exists.")?;
            let m = goal.milestone_mut(milestone_id).ok_or("That milestone no longer exists.")?;
            if m.is_done() == done {
                return Ok(());
            }
            m.completed_at = done.then(Utc::now);
            let title = m.title.clone();
            goal.record(if done { format!("Completed \"{title}\"") } else { format!("Reopened \"{title}\"") });
            Ok(())
        })
    }

    // ----- task links -----

    /// Links an existing task (or the top-level task of a step) to a goal,
    /// optionally pinned to one of its milestones. A task already linked to
    /// another goal is moved, so a task never sits in two goals.
    pub fn link_task(&self, goal_id: &str, task_id: &str, milestone_id: Option<&str>) -> Result<(), String> {
        self.handle.mutate(|st| {
            let goal = st.goals.iter().find(|g| g.id == goal_id).ok_or("That goal no longer exists.")?;
            if let Some(mid) = milestone_id {
                if goal.milestone(mid).is_none() {
                    return Err("That milestone no longer exists.".to_string());
                }
            }
            let root_id = {
                let task = st.task(task_id).ok_or("That task no longer exists.")?;
                root_task(st, task).id.clone()
            };
            let task = st.task_mut(&root_id).ok_or("That task no longer exists.")?;
            task.goal_id = Some(goal_id.to_string());
            task.milestone_id = milestone_id.map(str::to_string);
            Ok(())
        })
    }

    /// Unlinks a task from its goal. The task itself is untouched.
    pub fn unlink_task(&self, task_id: &str) -> Result<(), String> {
        self.handle.mutate(|st| {
            let task = st.task_mut(task_id).ok_or("That task no longer exists.")?;
            task.goal_id = None;
            task.milestone_id = None;
            Ok(())
        })
    }

    /// Pins a linked task to a milestone, or clears the pin with `None`.
    pub fn pin_task(&self, task_id: &str, milestone_id: Option<&str>) -> Result<(), String> {
        self.handle.mutate(|st| {
            let goal_id = st.task(task_id).and_then(|t| t.goal_id.clone()).ok_or("Link the task to the goal first.")?;
            if let Some(mid) = milestone_id {
                let ok = st.goals.iter().find(|g| g.id == goal_id).is_some_and(|g| g.milestone(mid).is_some());
                if !ok {
                    return Err("That milestone no longer exists.".to_string());
                }
            }
            if let Some(t) = st.task_mut(task_id) {
                t.milestone_id = milestone_id.map(str::to_string);
            }
            Ok(())
        })
    }

    /// Open top-level tasks not yet linked to any goal, newest first, that
    /// match `search`. These are the candidates in the link picker.
    pub fn linkable_tasks(store: &AppStateStore, search: &str, limit: usize) -> Vec<Task> {
        let needle = search.trim().to_lowercase();
        let mut tasks: Vec<&Task> = store
            .tasks
            .iter()
            .filter(|t| !t.is_step() && t.is_pending() && t.goal_id.is_none())
            .filter(|t| needle.is_empty() || t.title.to_lowercase().contains(&needle))
            .collect();
        tasks.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        tasks.into_iter().take(limit).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FocusSession, TaskSection, TaskStatus, TimeEntry};
    use crate::repository::LoadReport;
    use chrono::TimeZone;

    struct MemoryRepo;
    impl StorageRepository for MemoryRepo {
        fn load_with_report(&self) -> LoadReport {
            LoadReport { store: AppStateStore::default(), recovery: None }
        }
        fn save(&self, _: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
    }

    fn service() -> GoalService {
        GoalService::new(Arc::new(Mutex::new(AppStateStore::with_space())), Arc::new(MemoryRepo))
    }

    fn add_task(svc: &GoalService, title: &str) -> String {
        svc.handle().mutate(|st| {
            let list = st.lists[0].id.clone();
            st.add_task(Task::new(list, title, 30, TaskSection::Today)).id
        })
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn create_validates_and_trims() {
        let svc = service();
        assert!(svc.create(&GoalDraft::new("   ")).is_err());
        assert!(svc.create(&GoalDraft::new("x".repeat(121))).is_err());
        let mut d = GoalDraft::new("  Learn Rust  ");
        d.category = " study ".into();
        let g = svc.create(&d).unwrap();
        assert_eq!((g.title.as_str(), g.category.as_str()), ("Learn Rust", "study"));
        assert_eq!(svc.handle().read(|s| s.goals.len()), 1);
    }

    #[test]
    fn measured_goals_need_a_valid_target() {
        let svc = service();
        let mut d = GoalDraft::new("Read");
        d.mode = ProgressMode::Measured;
        assert!(svc.create(&d).is_err());
        d.target_value = -2.0;
        assert!(svc.create(&d).is_err());
        d.target_value = 20.0;
        d.current_value = -1.0;
        assert!(svc.create(&d).is_err());
        d.current_value = 4.0;
        d.unit = "books".into();
        let g = svc.create(&d).unwrap();
        assert_eq!(g.progress().percent, Some(20.0));
    }

    #[test]
    fn update_and_status_changes_persist_and_record_history() {
        let svc = service();
        let g = svc.create(&GoalDraft::new("A")).unwrap();
        let mut d = GoalDraft::new("B");
        d.priority = GoalPriority::High;
        svc.update(&g.id, &d).unwrap();
        svc.set_status(&g.id, GoalStatus::Paused).unwrap();
        svc.set_status(&g.id, GoalStatus::Completed).unwrap();
        let got = svc.handle().read(|s| s.goals[0].clone());
        assert_eq!((got.title.as_str(), got.priority, got.status), ("B", GoalPriority::High, GoalStatus::Completed));
        assert!(got.completed_at.is_some());
        assert!(got.history.len() >= 4);
        assert!(svc.update("nope", &d).is_err());
        assert!(svc.set_status("nope", GoalStatus::Active).is_err());
    }

    #[test]
    fn deleting_a_goal_keeps_its_tasks() {
        let svc = service();
        let g = svc.create(&GoalDraft::new("A")).unwrap();
        let t = add_task(&svc, "Write intro");
        svc.link_task(&g.id, &t, None).unwrap();
        svc.delete(&g.id).unwrap();
        svc.handle().read(|s| {
            assert!(s.goals.is_empty());
            let task = s.task(&t).expect("task survives");
            assert!(task.goal_id.is_none() && task.milestone_id.is_none());
        });
        assert!(svc.delete(&g.id).is_err());
    }

    #[test]
    fn milestones_add_edit_reorder_complete_and_delete() {
        let svc = service();
        let g = svc.create(&GoalDraft::new("A")).unwrap();
        let a = svc.add_milestone(&g.id, "One", None, "").unwrap();
        let b = svc.add_milestone(&g.id, "Two", Some(day("2026-12-01")), "second").unwrap();
        let c = svc.add_milestone(&g.id, "Three", None, "").unwrap();
        assert!(svc.add_milestone(&g.id, " ", None, "").is_err());
        svc.move_milestone(&g.id, &c.id, -2).unwrap();
        svc.handle().read(|s| {
            let ids: Vec<_> = s.goals[0].milestones.iter().map(|m| m.id.as_str()).collect();
            assert_eq!(ids, [c.id.as_str(), a.id.as_str(), b.id.as_str()]);
            assert_eq!(s.goals[0].milestones[0].sort_order, 0);
        });
        svc.move_milestone(&g.id, &c.id, -5).unwrap(); // already first: no change, no error
        svc.edit_milestone(&g.id, &a.id, "Uno", None, "d").unwrap();
        svc.set_milestone_done(&g.id, &a.id, true).unwrap();
        svc.handle().read(|s| {
            assert_eq!(s.goals[0].progress().percent.map(|p| p.round() as i32), Some(33));
            assert!(s.goals[0].milestone(&a.id).unwrap().completed_at.is_some());
            assert_eq!(s.goals[0].next_milestone().unwrap().id, c.id);
        });
        svc.set_milestone_done(&g.id, &a.id, false).unwrap();
        svc.delete_milestone(&g.id, &b.id).unwrap();
        assert!(svc.delete_milestone(&g.id, &b.id).is_err());
        svc.handle().read(|s| assert_eq!(s.goals[0].milestones.len(), 2));
    }

    #[test]
    fn finishing_tasks_does_not_complete_a_milestone() {
        let svc = service();
        let g = svc.create(&GoalDraft::new("A")).unwrap();
        let m = svc.add_milestone(&g.id, "M", None, "").unwrap();
        let t = add_task(&svc, "T");
        svc.link_task(&g.id, &t, Some(&m.id)).unwrap();
        svc.handle().mutate(|st| st.task_mut(&t).unwrap().set_status(TaskStatus::Completed));
        svc.handle().read(|s| {
            assert!(!s.goals[0].milestone(&m.id).unwrap().is_done());
            assert_eq!(task_progress(s, &g.id), (1, 1));
        });
    }

    #[test]
    fn linking_never_duplicates_and_moves_between_goals() {
        let svc = service();
        let g1 = svc.create(&GoalDraft::new("One")).unwrap();
        let g2 = svc.create(&GoalDraft::new("Two")).unwrap();
        let t = add_task(&svc, "T");
        svc.link_task(&g1.id, &t, None).unwrap();
        svc.link_task(&g1.id, &t, None).unwrap();
        svc.handle().read(|s| assert_eq!(linked_tasks(s, &g1.id).len(), 1));
        svc.link_task(&g2.id, &t, None).unwrap();
        svc.handle().read(|s| {
            assert!(linked_tasks(s, &g1.id).is_empty());
            assert_eq!(linked_tasks(s, &g2.id).len(), 1);
        });
        assert!(svc.link_task("missing", &t, None).is_err());
        assert!(svc.link_task(&g2.id, "missing", None).is_err());
        assert!(svc.link_task(&g2.id, &t, Some("no-such-milestone")).is_err());
        svc.unlink_task(&t).unwrap();
        svc.handle().read(|s| assert!(s.task(&t).is_some() && linked_tasks(s, &g2.id).is_empty()));
    }

    #[test]
    fn linking_a_step_links_its_top_level_task() {
        let svc = service();
        let g = svc.create(&GoalDraft::new("A")).unwrap();
        let parent = add_task(&svc, "Parent");
        let step = svc.handle().mutate(|st| {
            let p = st.task(&parent).unwrap().clone();
            st.add_task(Task::new_step(&p, "Step")).id
        });
        svc.link_task(&g.id, &step, None).unwrap();
        svc.handle().read(|s| {
            assert_eq!(s.task(&parent).unwrap().goal_id.as_deref(), Some(g.id.as_str()));
            assert!(s.task(&step).unwrap().goal_id.is_none());
        });
    }

    #[test]
    fn deleting_a_milestone_unpins_tasks_but_keeps_the_link() {
        let svc = service();
        let g = svc.create(&GoalDraft::new("A")).unwrap();
        let m = svc.add_milestone(&g.id, "M", None, "").unwrap();
        let t = add_task(&svc, "T");
        svc.link_task(&g.id, &t, Some(&m.id)).unwrap();
        svc.delete_milestone(&g.id, &m.id).unwrap();
        svc.handle().read(|s| {
            let task = s.task(&t).unwrap();
            assert_eq!(task.goal_id.as_deref(), Some(g.id.as_str()));
            assert!(task.milestone_id.is_none());
        });
    }

    #[test]
    fn measured_current_value_can_be_updated() {
        let svc = service();
        let mut d = GoalDraft::new("Read");
        d.mode = ProgressMode::Measured;
        d.target_value = 10.0;
        let g = svc.create(&d).unwrap();
        svc.set_current_value(&g.id, 7.0).unwrap();
        assert!(svc.set_current_value(&g.id, -1.0).is_err());
        svc.handle().read(|s| assert_eq!(s.goals[0].progress().percent, Some(70.0)));
        let other = svc.create(&GoalDraft::new("Milestone goal")).unwrap();
        assert!(svc.set_current_value(&other.id, 3.0).is_err());
    }

    #[test]
    fn dashboard_filters_sorts_and_searches() {
        let svc = service();
        let mut a = GoalDraft::new("Alpha");
        a.priority = GoalPriority::Low;
        a.target_date = Some(day("2026-12-01"));
        let mut b = GoalDraft::new("Beta");
        b.priority = GoalPriority::High;
        let mut c = GoalDraft::new("Gamma");
        c.priority = GoalPriority::High;
        c.target_date = Some(day("2026-11-20"));
        let a = svc.create(&a).unwrap();
        svc.create(&b).unwrap();
        svc.create(&c).unwrap();
        svc.set_status(&a.id, GoalStatus::Paused).unwrap();
        svc.handle().read(|s| {
            let names = |v: Vec<&Goal>| v.iter().map(|g| g.title.clone()).collect::<Vec<_>>();
            assert_eq!(names(list_goals(s, GoalFilter::InPlay, GoalSort::Priority, "")), ["Gamma", "Beta", "Alpha"]);
            assert_eq!(names(list_goals(s, GoalFilter::Active, GoalSort::Deadline, "")), ["Gamma", "Beta"]);
            assert_eq!(names(list_goals(s, GoalFilter::Paused, GoalSort::Recent, "")), ["Alpha"]);
            assert_eq!(names(list_goals(s, GoalFilter::InPlay, GoalSort::Priority, " ALP ")), ["Alpha"]);
            assert!(list_goals(s, GoalFilter::Completed, GoalSort::Priority, "").is_empty());
            let c = counts(s, day("2026-12-05"));
            assert_eq!((c.active, c.paused, c.overdue), (2, 1, 1));
        });
    }

    #[test]
    fn focus_time_follows_linked_tasks_and_steps_without_double_counting() {
        let svc = service();
        let g = svc.create(&GoalDraft::new("A")).unwrap();
        let t = add_task(&svc, "Task");
        let other = add_task(&svc, "Unlinked");
        let step = svc.handle().mutate(|st| {
            let p = st.task(&t).unwrap().clone();
            st.add_task(Task::new_step(&p, "Step")).id
        });
        svc.link_task(&g.id, &t, None).unwrap();
        let start = Utc.with_ymd_and_hms(2026, 11, 3, 9, 0, 0).unwrap();
        svc.handle().mutate(|st| {
            // A session with a matching time entry counts once.
            let mut s = FocusSession::new(1500, Some(t.clone()));
            s.finish(1500);
            st.focus_sessions.push(s.clone());
            let mut e = TimeEntry::new(TimeEntryKind::Focus, start, start + Duration::minutes(25));
            e.task_id = Some(t.clone());
            e.session_id = Some(s.id);
            st.time_entries.push(e);
            // Time on a step counts toward the parent's goal.
            let mut e2 = TimeEntry::new(TimeEntryKind::Focus, start + Duration::hours(1), start + Duration::hours(1) + Duration::minutes(10));
            e2.task_id = Some(step.clone());
            st.time_entries.push(e2);
            // Breaks and unlinked tasks do not.
            let mut e3 = TimeEntry::new(TimeEntryKind::Break, start, start + Duration::minutes(5));
            e3.task_id = Some(t.clone());
            st.time_entries.push(e3);
            let mut e4 = TimeEntry::new(TimeEntryKind::Focus, start, start + Duration::minutes(40));
            e4.task_id = Some(other.clone());
            st.time_entries.push(e4);
        });
        svc.handle().read(|s| {
            assert_eq!(focus_secs(s, &g.id), (25 + 10) * 60);
            assert_eq!(focus_secs(s, "nothing"), 0);
            let sessions = recent_sessions(s, &g.id, 5);
            assert_eq!(sessions.len(), 2);
            assert_eq!(sessions[0].secs, 600);
            let local_day = crate::services::insights::to_local(start).date();
            let week = focus_by_day(s, &g.id, local_day, 7);
            assert_eq!(week.len(), 7);
            assert_eq!(week.iter().map(|(_, secs)| secs).sum::<u64>(), 35 * 60);
            let summary = focus_summary(s, local_day);
            assert_eq!(summary.get(&g.id), Some(&(35 * 60, 35 * 60)));
            assert!(summary.get("nothing").is_none());
        });
    }

    #[test]
    fn unfinished_sessions_without_time_are_not_counted() {
        let svc = service();
        let g = svc.create(&GoalDraft::new("A")).unwrap();
        let t = add_task(&svc, "T");
        svc.link_task(&g.id, &t, None).unwrap();
        svc.handle().mutate(|st| st.focus_sessions.push(FocusSession::new(1500, Some(t.clone()))));
        svc.handle().read(|s| assert_eq!(focus_secs(s, &g.id), 0));
    }

    #[test]
    fn link_picker_lists_only_free_open_top_level_tasks() {
        let svc = service();
        let g = svc.create(&GoalDraft::new("A")).unwrap();
        let linked = add_task(&svc, "Linked");
        let free = add_task(&svc, "Write report");
        let done = add_task(&svc, "Write notes");
        svc.link_task(&g.id, &linked, None).unwrap();
        svc.handle().mutate(|st| st.task_mut(&done).unwrap().set_status(TaskStatus::Completed));
        svc.handle().read(|s| {
            let got = GoalService::linkable_tasks(s, "write", 10);
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].id, free);
        });
    }

    #[test]
    fn weekly_review_uses_only_real_records() {
        let svc = service();
        let today = crate::services::insights::to_local(Utc::now()).date();
        // Nothing saved: an empty review, not made-up numbers.
        svc.handle().read(|s| {
            let r = weekly_review(s, today);
            assert_eq!((r.milestones_done, r.tasks_done, r.focus_secs), (0, 0, 0));
            assert!(r.progressed.is_empty() && r.upcoming.is_empty() && r.attention.is_empty() && r.priorities.is_empty());
            assert_eq!(r.from, today - Duration::days(6));
        });

        let mut d = GoalDraft::new("Ship it");
        d.target_date = Some(today + Duration::days(3));
        let g = svc.create(&d).unwrap();
        let m1 = svc.add_milestone(&g.id, "Draft", None, "").unwrap();
        svc.add_milestone(&g.id, "Review", Some(today + Duration::days(2)), "").unwrap();
        let t = add_task(&svc, "Write");
        svc.link_task(&g.id, &t, None).unwrap();
        svc.set_milestone_done(&g.id, &m1.id, true).unwrap();
        svc.handle().mutate(|st| st.task_mut(&t).unwrap().set_status(TaskStatus::Completed));
        let start = Utc::now() - Duration::hours(2);
        svc.handle().mutate(|st| {
            let mut e = TimeEntry::new(TimeEntryKind::Focus, start, start + Duration::minutes(30));
            e.task_id = Some(t.clone());
            st.time_entries.push(e);
        });
        svc.handle().read(|s| {
            let r = weekly_review(s, today);
            assert_eq!((r.milestones_done, r.tasks_done, r.focus_secs), (1, 1, 30 * 60));
            assert_eq!(r.progressed.len(), 1);
            assert_eq!(r.progressed[0].reason, ReviewReason::Progressed { milestones: 1, tasks: 1 });
            // The goal date and the open milestone date are both coming up, soonest first.
            assert_eq!(r.upcoming.len(), 2);
            assert_eq!(r.upcoming[0].reason, ReviewReason::MilestoneDue { title: "Review".into(), days: 2 });
            assert_eq!(r.upcoming[1].reason, ReviewReason::Deadline { days: 3 });
            assert!(r.attention.is_empty());
            assert_eq!(r.priorities.len(), 1);
            assert_eq!(r.priorities[0].reason, ReviewReason::Deadline { days: 3 });
        });
    }

    #[test]
    fn weekly_review_flags_overdue_and_quiet_goals_and_ranks_priorities() {
        let svc = service();
        let today = crate::services::insights::to_local(Utc::now()).date();
        let mut late = GoalDraft::new("Late");
        late.target_date = Some(today - Duration::days(4));
        let late = svc.create(&late).unwrap();
        let mut high = GoalDraft::new("High");
        high.priority = GoalPriority::High;
        svc.create(&high).unwrap();
        let mut soon = GoalDraft::new("Soon");
        soon.target_date = Some(today + Duration::days(20));
        svc.create(&soon).unwrap();
        let mut paused = GoalDraft::new("Paused");
        paused.target_date = Some(today - Duration::days(9));
        let paused = svc.create(&paused).unwrap();
        svc.set_status(&paused.id, GoalStatus::Paused).unwrap();
        // Make one goal old and untouched.
        svc.handle().mutate(|st| {
            let g = st.goals.iter_mut().find(|g| g.title == "High").unwrap();
            g.created_at = Utc::now() - Duration::days(40);
            g.updated_at = Utc::now() - Duration::days(30);
        });
        svc.handle().read(|s| {
            let r = weekly_review(s, today);
            let names: Vec<_> = r.attention.iter().map(|i| i.title.as_str()).collect();
            assert!(names.contains(&"Late") && names.contains(&"High"));
            assert!(!names.contains(&"Paused"), "paused goals are not nagged");
            assert!(r.attention.iter().any(|i| i.goal_id == late.id && matches!(i.reason, ReviewReason::Deadline { days: -4 })));
            let order: Vec<_> = r.priorities.iter().map(|i| i.title.as_str()).collect();
            assert_eq!(order, ["Late", "Soon", "High"]);
        });
    }

    #[test]
    fn schema_four_files_load_with_no_goals_and_keep_their_tasks() {
        let old = r#"{"schema_version":4,"tasks":[{"id":"t","title":"Old","list_id":"l"}]}"#;
        let store = AppStateStore::from_json(old.as_bytes()).unwrap();
        assert_eq!(store.schema_version, crate::models::CURRENT_SCHEMA_VERSION);
        assert!(store.goals.is_empty());
        assert_eq!(store.tasks.len(), 1);
        assert!(store.tasks[0].goal_id.is_none());
        // And it round-trips through JSON with goals attached.
        let mut store = store;
        let mut g = Goal::new("G");
        g.milestones.push(Milestone::new("M"));
        store.tasks[0].goal_id = Some(g.id.clone());
        store.goals.push(g);
        let again = AppStateStore::from_json(&serde_json::to_vec(&store).unwrap()).unwrap();
        assert_eq!(again.goals.len(), 1);
        assert_eq!(again.tasks[0].goal_id, store.tasks[0].goal_id);
    }
    #[test]
    fn hours_goals_follow_logged_focus_time() {
        let svc = service();
        let mut d = GoalDraft::new("Study");
        d.mode = ProgressMode::Measured;
        d.target_value = 10.0;
        d.track_focus = true;
        let g = svc.create(&d).unwrap();
        let t = add_task(&svc, "Read");
        svc.link_task(&g.id, &t, None).unwrap();
        assert!(!svc.sync_focus_hours(), "no time logged yet");
        let start = Utc.with_ymd_and_hms(2026, 11, 3, 9, 0, 0).unwrap();
        svc.handle().mutate(|st| {
            let mut e = TimeEntry::new(TimeEntryKind::Focus, start, start + Duration::minutes(90));
            e.task_id = Some(t.clone());
            st.time_entries.push(e);
        });
        assert!(svc.sync_focus_hours());
        assert!(!svc.sync_focus_hours(), "second sync changes nothing");
        svc.handle().read(|s| {
            assert_eq!(s.goals[0].current_value, 1.5);
            assert_eq!(s.goals[0].unit, "h");
            assert_eq!(s.goals[0].progress().percent, Some(15.0));
        });
    }
}
