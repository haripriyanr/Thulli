//! Domain entities and OOP state models for Thulli.
//!
//! Provides encapsulated domain objects with validation, state transitions,
//! and factory methods designed for reusability across desktop productivity tools.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single actionable task or focus item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub estimate_mins: u32,
    pub time_taken_secs: u64,
    pub completed: bool,
    pub completed_at: Option<DateTime<Utc>>,
    pub board: String, // "today", "week", "backlog", "done"
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
}

impl Task {
    /// Factory constructor for creating a new pending task.
    pub fn new(list_id: impl Into<String>, title: impl Into<String>, estimate_mins: u32, board: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            list_id: list_id.into(),
            title: title.into(),
            estimate_mins,
            time_taken_secs: 0,
            completed: false,
            completed_at: None,
            board: board.into(),
            sort_order: 0,
            created_at: Utc::now(),
        }
    }

    /// Toggles the completion state of the task, updating timestamps and target board.
    pub fn toggle_completion(&mut self) -> bool {
        self.completed = !self.completed;
        if self.completed {
            self.completed_at = Some(Utc::now());
            self.board = "done".to_string();
        } else {
            self.completed_at = None;
            if self.board == "done" {
                self.board = "today".to_string();
            }
        }
        self.completed
    }

    /// Checks if this task belongs to a specific list.
    pub fn belongs_to_list(&self, list_id: &str) -> bool {
        self.list_id == list_id
    }

    /// Checks if this task is still pending.
    pub fn is_pending(&self) -> bool {
        !self.completed
    }
}

/// A user-defined task list or board category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskList {
    pub id: String,
    pub name: String,
    pub color: String,
    pub is_archived: bool,
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
}

impl TaskList {
    /// Factory constructor for creating a new custom task list.
    pub fn new(name: impl Into<String>, color: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            color: color.into(),
            is_archived: false,
            sort_order: 0,
            created_at: Utc::now(),
        }
    }

    /// Factory constructor for the default "Personal" list.
    pub fn new_personal() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: "Personal".to_string(),
            color: "#6FDBAB".to_string(),
            is_archived: false,
            sort_order: 0,
            created_at: Utc::now(),
        }
    }

    /// Toggles archive status.
    pub fn toggle_archived(&mut self) -> bool {
        self.is_archived = !self.is_archived;
        self.is_archived
    }
}

/// Application shielding rule to block distracting processes during focus sprints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedApp {
    pub id: String,
    pub name: String,
    pub process_name: String,
    pub active: bool,
}

impl BlockedApp {
    /// Creates a new blocked process shield rule.
    pub fn new(name: impl Into<String>, process_name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            process_name: process_name.into(),
            active: true,
        }
    }

    /// Toggles whether this blocking rule is active.
    pub fn toggle_active(&mut self) -> bool {
        self.active = !self.active;
        self.active
    }
}

/// User customization and focus sprint preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThulliPreferences {
    pub timezone: String,
    pub sprint_enabled: bool,
    pub work_sprint_mins: u32,
    pub break_time_mins: u32,
    pub default_break_mins: u32,
    pub scrolling_title: bool,
    pub timed_alerts: bool,
    pub alert_interval_mins: u32,
    pub sound_id: String,
    pub app_shield_active: bool,
    pub web_shield_active: bool,
    pub quick_notes: String,
    pub active_mode: String,
}

impl Default for ThulliPreferences {
    fn default() -> Self {
        Self {
            timezone: "(GMT+05:30) Asia/Calcutta".to_string(),
            sprint_enabled: false,
            work_sprint_mins: 60,
            break_time_mins: 15,
            default_break_mins: 10,
            scrolling_title: true,
            timed_alerts: true,
            alert_interval_mins: 30,
            sound_id: "basic-ding".to_string(),
            app_shield_active: false,
            web_shield_active: false,
            quick_notes: "Thulli Scratchpad\n- Keep focus ideas here without leaving your flow.".to_string(),
            active_mode: "dashboard".to_string(),
        }
    }
}

/// AppStateStore represents the root domain aggregate persisted in encrypted storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStateStore {
    pub lists: Vec<TaskList>,
    pub tasks: Vec<Task>,
    pub blocked_apps: Vec<BlockedApp>,
    pub preferences: ThulliPreferences,
}

impl Default for AppStateStore {
    fn default() -> Self {
        Self {
            lists: vec![TaskList::new_personal()],
            tasks: Vec::new(),
            blocked_apps: Vec::new(),
            preferences: ThulliPreferences::default(),
        }
    }
}

impl AppStateStore {
    /// Adds a task to the aggregate, returning a clone of the created task.
    pub fn add_task(&mut self, task: Task) -> Task {
        let task_clone = task.clone();
        self.tasks.push(task);
        task_clone
    }

    /// Removes a task by ID. Returns true if a task was removed.
    pub fn remove_task(&mut self, task_id: &str) -> bool {
        let initial_len = self.tasks.len();
        self.tasks.retain(|t| t.id != task_id);
        self.tasks.len() < initial_len
    }

    /// Toggles a task's completion by ID. Returns the new completion status if found.
    pub fn toggle_task(&mut self, task_id: &str) -> Option<bool> {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            Some(t.toggle_completion())
        } else {
            None
        }
    }

    /// Adds a new list to the aggregate, returning the new list's ID.
    pub fn add_list(&mut self, list: TaskList) -> String {
        let id = list.id.clone();
        self.lists.push(list);
        id
    }

    /// Removes a list and cascades deletion to all associated tasks.
    pub fn remove_list_cascade(&mut self, list_id: &str) {
        self.lists.retain(|l| l.id != list_id);
        self.tasks.retain(|t| t.list_id != list_id);
    }

    /// Finds the first non-archived list, or the first list if all are archived.
    pub fn get_primary_list(&self) -> Option<&TaskList> {
        self.lists.iter().find(|l| !l.is_archived).or_else(|| self.lists.first())
    }

    /// Calculates pending task counts for all lists.
    pub fn count_pending_for_list(&self, list_id: &str) -> usize {
        self.tasks.iter().filter(|t| t.list_id == list_id && !t.completed).count()
    }
}
