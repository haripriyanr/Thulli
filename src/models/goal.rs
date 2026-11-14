//! Goals: long-term outcomes, their milestones and how progress is counted.
//!
//! A task belongs to at most one goal (`Task::goal_id`) and may be pinned to
//! one of that goal's milestones (`Task::milestone_id`). Tasks are never
//! copied into a goal, so removing a goal only clears those two fields.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Lifecycle of a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    #[default]
    Active,
    Paused,
    Completed,
    Archived,
}

impl GoalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalStatus::Active => "active",
            GoalStatus::Paused => "paused",
            GoalStatus::Completed => "completed",
            GoalStatus::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(GoalStatus::Active),
            "paused" => Some(GoalStatus::Paused),
            "completed" => Some(GoalStatus::Completed),
            "archived" => Some(GoalStatus::Archived),
            _ => None,
        }
    }
}

/// How a goal's progress is counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProgressMode {
    /// Completed milestones over all milestones.
    #[default]
    Milestones,
    /// A current value against a target value, such as 12 courses.
    Measured,
}

impl ProgressMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProgressMode::Milestones => "milestones",
            ProgressMode::Measured => "measured",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "milestones" => Some(ProgressMode::Milestones),
            "measured" => Some(ProgressMode::Measured),
            _ => None,
        }
    }
}

/// Importance of a goal; the dashboard sorts by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalPriority {
    High,
    #[default]
    Medium,
    Low,
}

impl GoalPriority {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalPriority::High => "high",
            GoalPriority::Medium => "medium",
            GoalPriority::Low => "low",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "high" => Some(GoalPriority::High),
            "medium" => Some(GoalPriority::Medium),
            "low" => Some(GoalPriority::Low),
            _ => None,
        }
    }
}

/// A checkpoint on the way to a goal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Milestone {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub target_date: Option<NaiveDate>,
    /// Set when the milestone is ticked off; `None` while it is open.
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub sort_order: i32,
}

impl Milestone {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            title: title.into(),
            description: String::new(),
            target_date: None,
            completed_at: None,
            sort_order: 0,
        }
    }

    pub fn is_done(&self) -> bool {
        self.completed_at.is_some()
    }
}

/// One entry of a goal's progress history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalEvent {
    pub at: DateTime<Utc>,
    pub event: String,
}

/// A long-term objective.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Why the goal matters to the person.
    #[serde(default)]
    pub motivation: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub priority: GoalPriority,
    #[serde(default)]
    pub status: GoalStatus,
    #[serde(default)]
    pub progress_mode: ProgressMode,
    #[serde(default)]
    pub target_date: Option<NaiveDate>,
    #[serde(default)]
    pub milestones: Vec<Milestone>,
    /// Measured mode: where the person is now.
    #[serde(default)]
    pub current_value: f64,
    /// Measured mode: where they want to be.
    #[serde(default)]
    pub target_value: f64,
    #[serde(default)]
    pub unit: String,
    /// Measured mode: the current value is the focus hours logged on the linked tasks.
    #[serde(default)]
    pub track_focus: bool,
    #[serde(default)]
    pub history: Vec<GoalEvent>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

impl Goal {
    pub fn new(title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            title: title.into(),
            description: String::new(),
            motivation: String::new(),
            category: String::new(),
            notes: String::new(),
            priority: GoalPriority::Medium,
            status: GoalStatus::Active,
            progress_mode: ProgressMode::Milestones,
            target_date: None,
            milestones: Vec::new(),
            current_value: 0.0,
            target_value: 0.0,
            unit: String::new(),
            track_focus: false,
            history: vec![GoalEvent { at: now, event: "Created".to_string() }],
            completed_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Adds a line to the history and bumps the update time.
    pub fn record(&mut self, event: impl Into<String>) {
        let now = Utc::now();
        self.history.push(GoalEvent { at: now, event: event.into() });
        self.updated_at = now;
    }

    /// Sets the status, keeping `completed_at` in step.
    pub fn set_status(&mut self, status: GoalStatus) {
        if self.status == status {
            return;
        }
        self.status = status;
        self.completed_at = if status == GoalStatus::Completed { Some(Utc::now()) } else { None };
        self.record(match status {
            GoalStatus::Active => "Resumed",
            GoalStatus::Paused => "Paused",
            GoalStatus::Completed => "Completed",
            GoalStatus::Archived => "Archived",
        });
    }

    pub fn milestone(&self, id: &str) -> Option<&Milestone> {
        self.milestones.iter().find(|m| m.id == id)
    }

    pub fn milestone_mut(&mut self, id: &str) -> Option<&mut Milestone> {
        self.milestones.iter_mut().find(|m| m.id == id)
    }

    /// Milestones still open, in order.
    pub fn open_milestones(&self) -> impl Iterator<Item = &Milestone> {
        self.milestones.iter().filter(|m| !m.is_done())
    }

    /// The first open milestone: what comes next.
    pub fn next_milestone(&self) -> Option<&Milestone> {
        self.open_milestones().next()
    }

    /// Renumbers `sort_order` to match the current order of the list.
    pub fn renumber_milestones(&mut self) {
        for (i, m) in self.milestones.iter_mut().enumerate() {
            m.sort_order = i as i32;
        }
    }

    /// True when the target date has passed and the goal is still in play.
    pub fn is_overdue(&self, today: NaiveDate) -> bool {
        self.status == GoalStatus::Active && self.target_date.is_some_and(|d| d < today)
    }

    pub fn progress(&self) -> GoalProgress {
        GoalProgress::of(self)
    }
}

/// How far a goal has come, ready to show.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalProgress {
    /// 0 to 100, or `None` when there is nothing to measure yet (no
    /// milestones, or a measured goal without a valid target).
    pub percent: Option<f32>,
    pub done: f64,
    pub total: f64,
    pub remaining: f64,
    pub mode: ProgressMode,
}

impl GoalProgress {
    pub fn of(goal: &Goal) -> Self {
        match goal.progress_mode {
            ProgressMode::Milestones => {
                let total = goal.milestones.len() as f64;
                let done = goal.milestones.iter().filter(|m| m.is_done()).count() as f64;
                let percent = (total > 0.0).then(|| (done / total * 100.0) as f32);
                GoalProgress { percent, done, total, remaining: total - done, mode: ProgressMode::Milestones }
            }
            ProgressMode::Measured => {
                let (cur, target) = (goal.current_value, goal.target_value);
                let valid = target.is_finite() && target > 0.0 && cur.is_finite();
                let percent = valid.then(|| (cur / target * 100.0).clamp(0.0, 100.0) as f32);
                let remaining = if valid { (target - cur).max(0.0) } else { 0.0 };
                GoalProgress { percent, done: cur, total: target, remaining, mode: ProgressMode::Measured }
            }
        }
    }

    /// Whole percent for display; 0 when there is nothing to measure.
    pub fn whole_percent(&self) -> i32 {
        self.percent.map_or(0, |p| p.round() as i32)
    }

    /// True when every milestone is done, or the target is reached.
    pub fn is_complete(&self) -> bool {
        self.percent.is_some_and(|p| p >= 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(mut m: Milestone) -> Milestone {
        m.completed_at = Some(Utc::now());
        m
    }

    #[test]
    fn milestone_progress_counts_done_over_total() {
        let mut g = Goal::new("Ship it");
        g.milestones = vec![done(Milestone::new("a")), Milestone::new("b"), Milestone::new("c"), Milestone::new("d")];
        let p = g.progress();
        assert_eq!(p.percent, Some(25.0));
        assert_eq!((p.done, p.total, p.remaining), (1.0, 4.0, 3.0));
        assert!(!p.is_complete());
    }

    #[test]
    fn no_milestones_means_no_percentage_not_zero_or_nan() {
        let g = Goal::new("Empty");
        let p = g.progress();
        assert_eq!(p.percent, None);
        assert_eq!(p.whole_percent(), 0);
        assert!(!p.is_complete());
    }

    #[test]
    fn measured_progress_is_clamped_but_keeps_real_values() {
        let mut g = Goal::new("Read");
        g.progress_mode = ProgressMode::Measured;
        g.target_value = 20.0;
        g.current_value = 5.0;
        assert_eq!(g.progress().percent, Some(25.0));
        g.current_value = 30.0;
        let p = g.progress();
        assert_eq!(p.percent, Some(100.0));
        assert_eq!(p.done, 30.0);
        assert_eq!(p.remaining, 0.0);
        assert!(p.is_complete());
        g.current_value = -4.0;
        assert_eq!(g.progress().percent, Some(0.0));
    }

    #[test]
    fn measured_goal_with_bad_target_has_no_percentage() {
        let mut g = Goal::new("x");
        g.progress_mode = ProgressMode::Measured;
        for bad in [0.0, -3.0, f64::NAN, f64::INFINITY] {
            g.target_value = bad;
            g.current_value = 2.0;
            assert_eq!(g.progress().percent, None, "target {bad}");
        }
    }

    #[test]
    fn status_changes_keep_completion_time_and_history() {
        let mut g = Goal::new("x");
        g.set_status(GoalStatus::Completed);
        assert!(g.completed_at.is_some());
        g.set_status(GoalStatus::Active);
        assert!(g.completed_at.is_none());
        assert_eq!(g.history.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(), ["Created", "Completed", "Resumed"]);
        let before = g.history.len();
        g.set_status(GoalStatus::Active);
        assert_eq!(g.history.len(), before);
    }

    #[test]
    fn overdue_only_counts_active_goals_past_their_date() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let mut g = Goal::new("x");
        g.target_date = Some(today.pred_opt().unwrap());
        assert!(g.is_overdue(today));
        g.target_date = Some(today);
        assert!(!g.is_overdue(today));
        g.target_date = Some(today.pred_opt().unwrap());
        g.set_status(GoalStatus::Paused);
        assert!(!g.is_overdue(today));
    }

    #[test]
    fn old_files_without_goal_fields_still_load() {
        let g: Goal = serde_json::from_str(r#"{"id":"g","title":"t"}"#).unwrap();
        assert_eq!(g.status, GoalStatus::Active);
        assert_eq!(g.progress_mode, ProgressMode::Milestones);
        assert!(g.milestones.is_empty());
    }
}
