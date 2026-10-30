//! Domain entities and OOP state models for Thulli.
//!
//! Entities are encapsulated structs with factory constructors and state
//! transitions. They are split by area; `AppStateStore` is the root aggregate
//! persisted by the repository layer.

mod chat;
mod focus;
mod list;
mod notes;
mod preferences;
mod shield;
mod tag;
mod task;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

pub use chat::{
    AssistantMemory, ChatMessage, ChatRole, ChatThread, MemoryKind, MemoryRevision, SuggestionKind,
    SuggestionState,
};
pub use focus::{DayPlan, FocusSession, TimeEntry, TimeEntryKind};
pub use list::{ListFolder, TaskList};
pub use notes::{BragEntry, MeetingNote, Note, NoteFolder, QuickCapture};
pub use preferences::{
    AccentTheme, AppShortcuts, ExpandedWindowMode, FocusDisplayMode, GlobalShortcuts, PanelSide,
    Preferences, ShortcutBinding, SuggestionToggles, ThemeChoice, WeekStart, SESSION_MAX_MINS,
    SESSION_MIN_MINS, SESSION_STEP_MINS,
};
pub use shield::{BlockedApp, BlockedSite};
pub use tag::Tag;
pub use task::{MatrixQuadrant, Task, TaskHistoryEntry, TaskSection, TaskStatus};

/// Former name of [`Preferences`].
pub type ThulliPreferences = Preferences;

/// Version of the persisted layout. Bump when a migration is added.
pub const CURRENT_SCHEMA_VERSION: u32 = 3;

/// Root aggregate persisted in encrypted storage.
///
/// Every field carries `#[serde(default)]`, so files written by older builds
/// still load; `migrate` then upgrades them in place.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStateStore {
    /// 0 (or absent) for files written before versioning existed.
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub lists: Vec<TaskList>,
    #[serde(default)]
    pub list_folders: Vec<ListFolder>,
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub note_folders: Vec<NoteFolder>,
    #[serde(default)]
    pub quick_captures: Vec<QuickCapture>,
    #[serde(default)]
    pub brag_entries: Vec<BragEntry>,
    #[serde(default)]
    pub meeting_notes: Vec<MeetingNote>,
    #[serde(default)]
    pub focus_sessions: Vec<FocusSession>,
    #[serde(default)]
    pub time_entries: Vec<TimeEntry>,
    #[serde(default)]
    pub day_plans: Vec<DayPlan>,
    #[serde(default)]
    pub blocked_apps: Vec<BlockedApp>,
    #[serde(default)]
    pub blocked_sites: Vec<BlockedSite>,
    #[serde(default)]
    pub chat_threads: Vec<ChatThread>,
    #[serde(default)]
    pub assistant_memories: Vec<AssistantMemory>,
    #[serde(default)]
    pub suggestion_states: Vec<SuggestionState>,
    #[serde(default)]
    pub preferences: Preferences,
}

impl Default for AppStateStore {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            lists: vec![TaskList::new_personal()],
            list_folders: Vec::new(),
            tags: Vec::new(),
            tasks: Vec::new(),
            notes: Vec::new(),
            note_folders: Vec::new(),
            quick_captures: Vec::new(),
            brag_entries: Vec::new(),
            meeting_notes: Vec::new(),
            focus_sessions: Vec::new(),
            time_entries: Vec::new(),
            day_plans: Vec::new(),
            blocked_apps: Vec::new(),
            blocked_sites: Vec::new(),
            chat_threads: Vec::new(),
            assistant_memories: Vec::new(),
            suggestion_states: Vec::new(),
            preferences: Preferences::default(),
        }
    }
}

impl AppStateStore {
    /// Parses stored JSON and upgrades it to the current schema.
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let mut store: AppStateStore = serde_json::from_slice(bytes)?;
        store.migrate();
        Ok(store)
    }

    /// Upgrades data written by older builds. Safe to call repeatedly.
    pub fn migrate(&mut self) {
        for task in &mut self.tasks {
            task.migrate_legacy();
        }
        self.preferences.migrate();
        if self.lists.is_empty() {
            self.lists.push(TaskList::new_personal());
        }
        self.schema_version = CURRENT_SCHEMA_VERSION;
    }

    /// Adds a task to the aggregate, returning a clone of the created task.
    pub fn add_task(&mut self, task: Task) -> Task {
        let task_clone = task.clone();
        self.tasks.push(task);
        task_clone
    }

    /// Removes a task and all of its breakdown steps. Returns true if anything was removed.
    pub fn remove_task(&mut self, task_id: &str) -> bool {
        let mut doomed = vec![task_id.to_string()];
        let mut i = 0;
        while i < doomed.len() {
            let parent = doomed[i].clone();
            doomed.extend(self.tasks.iter().filter(|t| t.parent_id.as_deref() == Some(parent.as_str())).map(|t| t.id.clone()));
            i += 1;
        }
        let initial_len = self.tasks.len();
        self.tasks.retain(|t| !doomed.contains(&t.id));
        self.tasks.len() < initial_len
    }

    pub fn task(&self, task_id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == task_id)
    }

    pub fn task_mut(&mut self, task_id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == task_id)
    }

    /// Direct breakdown steps of a task, in sort order.
    pub fn steps_of(&self, task_id: &str) -> Vec<&Task> {
        let mut steps: Vec<&Task> = self.tasks.iter().filter(|t| t.parent_id.as_deref() == Some(task_id)).collect();
        steps.sort_by_key(|t| t.sort_order);
        steps
    }

    /// Toggles a task's completion by ID. Returns the new completion status if found.
    pub fn toggle_task(&mut self, task_id: &str) -> Option<bool> {
        self.task_mut(task_id).map(|t| t.toggle_completion())
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

    /// Counts top-level tasks still to be done in a list.
    pub fn count_pending_for_list(&self, list_id: &str) -> usize {
        self.tasks.iter().filter(|t| t.list_id == list_id && !t.is_step() && t.is_pending()).count()
    }

    /// The plan for `date`, if one exists.
    pub fn day_plan(&self, date: NaiveDate) -> Option<&DayPlan> {
        self.day_plans.iter().find(|p| p.date == date)
    }

    /// The plan for `date`, created on first use.
    pub fn day_plan_mut(&mut self, date: NaiveDate) -> &mut DayPlan {
        if let Some(pos) = self.day_plans.iter().position(|p| p.date == date) {
            &mut self.day_plans[pos]
        } else {
            self.day_plans.push(DayPlan::new(date));
            self.day_plans.last_mut().expect("just pushed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_boards_map_to_sections() {
        assert_eq!(TaskSection::from_legacy_board("today"), TaskSection::Today);
        assert_eq!(TaskSection::from_legacy_board("week"), TaskSection::ThisWeek);
        assert_eq!(TaskSection::from_legacy_board("backlog"), TaskSection::Backlog);
    }

    #[test]
    fn removing_a_task_removes_its_steps() {
        let mut store = AppStateStore::default();
        let parent = store.add_task(Task::new("l", "Parent", 10, TaskSection::Today));
        let step = Task::new_step(&parent, "Step");
        let step = store.add_task(step);
        store.add_task(Task::new_step(&step, "Sub-step"));
        store.add_task(Task::new("l", "Other", 10, TaskSection::Today));
        assert!(store.remove_task(&parent.id));
        assert_eq!(store.tasks.len(), 1);
    }

    #[test]
    fn status_transitions_track_completion_time() {
        let mut t = Task::new("l", "x", 5, TaskSection::Backlog);
        assert!(t.toggle_completion());
        assert!(t.completed_at.is_some());
        t.mark_waiting();
        assert!(t.completed_at.is_none() && t.is_pending());
    }

    #[test]
    fn sections_store_as_board_names_and_old_names_still_load() {
        assert_eq!(serde_json::to_string(&TaskSection::ThisWeek).unwrap(), "\"this_week\"");
        assert_eq!(serde_json::from_str::<TaskSection>("\"sooner\"").unwrap(), TaskSection::ThisWeek);
        assert_eq!(serde_json::from_str::<TaskSection>("\"later\"").unwrap(), TaskSection::Backlog);
        assert_eq!(MatrixQuadrant::DoMidWeek.section(), TaskSection::ThisWeek);
    }

    #[test]
    fn new_defaults_follow_the_spec() {
        let p = Preferences::default();
        assert_eq!(p.default_break_mins, 5);
        assert_eq!(p.focus_display_mode, FocusDisplayMode::Expanded);
        assert!(p.suggestions.plan_my_day);
        assert_eq!(p.app_shortcuts.preferences.keys, "Ctrl+,");
    }

    #[test]
    fn memories_keep_their_history() {
        let mut m = AssistantMemory::new(MemoryKind::Preference, "Likes mornings");
        m.edit("Prefers early mornings");
        assert_eq!(m.history.len(), 1);
        assert_eq!(m.history[0].text, "Likes mornings");
    }

    #[test]
    fn task_history_records_moves_and_status() {
        let mut t = Task::new("l", "x", 5, TaskSection::Backlog);
        t.prepare_for_focus();
        t.complete();
        assert_eq!(t.section, TaskSection::Today);
        assert!(t.history.iter().any(|h| h.event == "Moved to Today"));
        assert!(t.history.iter().any(|h| h.event == "Marked completed"));
    }

    #[test]
    fn session_length_is_clamped_to_picker_steps() {
        assert_eq!(Preferences::clamp_session_mins(3), 10);
        assert_eq!(Preferences::clamp_session_mins(92), 90);
        assert_eq!(Preferences::clamp_session_mins(999), 180);
    }
}
