//! Focus sessions, tracked time and the daily plan.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single focus session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusSession {
    pub id: String,
    /// Task being worked on; may change during the session.
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub planned_secs: u64,
    #[serde(default)]
    pub focused_secs: u64,
    #[serde(default = "Utc::now")]
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
}

impl FocusSession {
    pub fn new(planned_secs: u64, task_id: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            task_id,
            planned_secs,
            focused_secs: 0,
            started_at: Utc::now(),
            ended_at: None,
        }
    }

    pub fn finish(&mut self, focused_secs: u64) {
        self.focused_secs = focused_secs;
        self.ended_at = Some(Utc::now());
    }

    pub fn is_finished(&self) -> bool {
        self.ended_at.is_some()
    }
}

/// What a block of tracked time was spent on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TimeEntryKind {
    Planning,
    #[default]
    Focus,
    Break,
    Other,
}

impl TimeEntryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TimeEntryKind::Planning => "planning",
            TimeEntryKind::Focus => "focus",
            TimeEntryKind::Break => "break",
            TimeEntryKind::Other => "other",
        }
    }
}

/// A block on the day timeline; feeds the calendar panel and Insights.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeEntry {
    pub id: String,
    #[serde(default)]
    pub kind: TimeEntryKind,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub label: String,
    /// True when the round ran to its full length.
    #[serde(default)]
    pub completed: bool,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
}

impl TimeEntry {
    pub fn new(kind: TimeEntryKind, started_at: DateTime<Utc>, ended_at: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            kind,
            task_id: None,
            session_id: None,
            label: String::new(),
            completed: false,
            started_at,
            ended_at,
        }
    }

    pub fn duration_secs(&self) -> u64 {
        (self.ended_at - self.started_at).num_seconds().max(0) as u64
    }
}

/// The plan for one calendar day: Plan, then Work, then Wrap up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayPlan {
    pub date: NaiveDate,
    /// True once planning finished (or was skipped).
    #[serde(default)]
    pub planned: bool,
    #[serde(default)]
    pub planned_task_ids: Vec<String>,
    #[serde(default)]
    pub planning_secs: u64,
    #[serde(default)]
    pub wrapped_up: bool,
    #[serde(default)]
    pub wrapped_up_at: Option<DateTime<Utc>>,
    /// Free text from the brain-dump step of wrapping up.
    #[serde(default)]
    pub brain_dump: String,
}

impl DayPlan {
    pub fn new(date: NaiveDate) -> Self {
        Self {
            date,
            planned: false,
            planned_task_ids: Vec::new(),
            planning_secs: 0,
            wrapped_up: false,
            wrapped_up_at: None,
            brain_dump: String::new(),
        }
    }

    pub fn finish_planning(&mut self, planning_secs: u64) {
        self.planned = true;
        self.planning_secs += planning_secs;
    }

    pub fn wrap_up(&mut self, brain_dump: impl Into<String>) {
        self.wrapped_up = true;
        self.wrapped_up_at = Some(Utc::now());
        self.brain_dump = brain_dump.into();
    }
}
