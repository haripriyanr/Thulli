//! Task entity: sections, statuses, breakdown steps and time tracking.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where a task sits in the planning horizon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskSection {
    #[default]
    Today,
    Sooner,
    Later,
}

impl TaskSection {
    /// Maps a legacy board name (`today`, `week`, `backlog`) to a section.
    /// Unknown names fall back to `Today`.
    pub fn from_legacy_board(board: &str) -> Self {
        match board.trim().to_lowercase().as_str() {
            "week" | "sooner" => TaskSection::Sooner,
            "backlog" | "later" => TaskSection::Later,
            _ => TaskSection::Today,
        }
    }

    /// Stable lowercase identifier used by the UI layer.
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskSection::Today => "today",
            TaskSection::Sooner => "sooner",
            TaskSection::Later => "later",
        }
    }

    /// Parses the identifier produced by [`TaskSection::as_str`].
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "today" => Some(TaskSection::Today),
            "sooner" => Some(TaskSection::Sooner),
            "later" => Some(TaskSection::Later),
            _ => None,
        }
    }
}

/// Lifecycle state of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    #[default]
    Open,
    Completed,
    Cancelled,
    Waiting,
}

impl TaskStatus {
    /// Stable lowercase identifier used by the UI layer.
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Open => "open",
            TaskStatus::Completed => "completed",
            TaskStatus::Cancelled => "cancelled",
            TaskStatus::Waiting => "waiting",
        }
    }

    /// Parses the identifier produced by [`TaskStatus::as_str`].
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(TaskStatus::Open),
            "completed" => Some(TaskStatus::Completed),
            "cancelled" => Some(TaskStatus::Cancelled),
            "waiting" => Some(TaskStatus::Waiting),
            _ => None,
        }
    }
}

/// A single actionable task, or a breakdown step when `parent_id` is set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    #[serde(default)]
    pub list_id: String,
    /// Parent task for breakdown steps; steps may nest to any depth.
    #[serde(default)]
    pub parent_id: Option<String>,
    pub title: String,
    /// Free-form note lines (bullets, links) shown under the task.
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub estimate_mins: u32,
    /// Total time worked on this task, in seconds.
    #[serde(default)]
    pub time_taken_secs: u64,
    #[serde(default)]
    pub section: TaskSection,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default)]
    pub sort_order: i32,
    /// Optional schedule; a reminder fires `reminder_mins_before` earlier.
    #[serde(default)]
    pub scheduled_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub reminder_mins_before: Option<u32>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_worked_at: Option<DateTime<Utc>>,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,

    /// Pre-migration fields: read from old files, never written back.
    #[serde(default, rename = "board", skip_serializing)]
    legacy_board: Option<String>,
    #[serde(default, rename = "completed", skip_serializing)]
    legacy_completed: Option<bool>,
}

impl Task {
    /// Factory constructor for a new open task in the given section.
    pub fn new(
        list_id: impl Into<String>,
        title: impl Into<String>,
        estimate_mins: u32,
        section: TaskSection,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            list_id: list_id.into(),
            parent_id: None,
            title: title.into(),
            notes: String::new(),
            estimate_mins,
            time_taken_secs: 0,
            section,
            status: TaskStatus::Open,
            sort_order: 0,
            scheduled_at: None,
            reminder_mins_before: None,
            completed_at: None,
            last_worked_at: None,
            created_at: Utc::now(),
            legacy_board: None,
            legacy_completed: None,
        }
    }

    /// Factory constructor for a breakdown step under `parent`.
    pub fn new_step(parent: &Task, title: impl Into<String>) -> Self {
        let mut step = Task::new(parent.list_id.clone(), title, 0, parent.section);
        step.parent_id = Some(parent.id.clone());
        step
    }

    /// Folds legacy `board` / `completed` values into section and status.
    /// Idempotent; called by `AppStateStore::migrate`.
    pub fn migrate_legacy(&mut self) {
        if let Some(board) = self.legacy_board.take() {
            if board.eq_ignore_ascii_case("done") {
                self.status = TaskStatus::Completed;
            } else {
                self.section = TaskSection::from_legacy_board(&board);
            }
        }
        if let Some(true) = self.legacy_completed.take() {
            self.status = TaskStatus::Completed;
        }
        if self.status == TaskStatus::Completed && self.completed_at.is_none() {
            self.completed_at = Some(self.created_at);
        }
    }

    /// Sets the status, maintaining `completed_at`.
    pub fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
        self.completed_at = if status == TaskStatus::Completed { Some(Utc::now()) } else { None };
    }

    pub fn complete(&mut self) {
        self.set_status(TaskStatus::Completed);
    }

    pub fn cancel(&mut self) {
        self.set_status(TaskStatus::Cancelled);
    }

    pub fn mark_waiting(&mut self) {
        self.set_status(TaskStatus::Waiting);
    }

    pub fn reopen(&mut self) {
        self.set_status(TaskStatus::Open);
    }

    /// Toggles between completed and open. Returns true when now completed.
    pub fn toggle_completion(&mut self) -> bool {
        if self.is_completed() {
            self.reopen();
        } else {
            self.complete();
        }
        self.is_completed()
    }

    pub fn move_to_section(&mut self, section: TaskSection) {
        self.section = section;
    }

    /// Adds worked time to this task.
    pub fn add_time(&mut self, secs: u64) {
        self.time_taken_secs = self.time_taken_secs.saturating_add(secs);
        self.last_worked_at = Some(Utc::now());
    }

    pub fn belongs_to_list(&self, list_id: &str) -> bool {
        self.list_id == list_id
    }

    pub fn is_step(&self) -> bool {
        self.parent_id.is_some()
    }

    pub fn is_completed(&self) -> bool {
        self.status == TaskStatus::Completed
    }

    /// Pending means still to be done: open or waiting.
    pub fn is_pending(&self) -> bool {
        matches!(self.status, TaskStatus::Open | TaskStatus::Waiting)
    }
}
