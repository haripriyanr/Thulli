//! High-level service layer orchestrating domain logic, state, and persistence.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use crate::models::{AppStateStore, Task, TaskList};
use crate::parser::parse_task_title;
use crate::repository::StorageRepository;

/// Service coordinating task creation, state toggling, and list associations.
pub struct TaskService {
    store: Arc<Mutex<AppStateStore>>,
    repository: Arc<dyn StorageRepository>,
}

impl TaskService {
    pub fn new(store: Arc<Mutex<AppStateStore>>, repository: Arc<dyn StorageRepository>) -> Self {
        Self { store, repository }
    }

    /// Parses user natural language input, resolves list fallback, and persists the new task.
    pub fn add_smart_task(&self, raw_input: &str, list_id: &str, default_board: &str) -> Option<Task> {
        let cleaned = raw_input.trim();
        if cleaned.is_empty() {
            return None;
        }

        let parsed = parse_task_title(cleaned, default_board);
        let mut st = self.store.lock().unwrap();

        // Fallback: If list_id is empty, use primary list or seed default
        let effective_list_id = if list_id.is_empty() {
            if let Some(first) = st.get_primary_list() {
                first.id.clone()
            } else {
                let default_list = TaskList::new_personal();
                let new_id = default_list.id.clone();
                st.add_list(default_list);
                new_id
            }
        } else {
            list_id.to_string()
        };

        let new_task = Task::new(effective_list_id, parsed.cleaned_title, parsed.estimate_mins, parsed.target_board);
        let created = st.add_task(new_task);
        let _ = self.repository.save(&st);

        Some(created)
    }

    /// Toggles task completion and saves state.
    pub fn toggle_task(&self, task_id: &str) -> Option<bool> {
        let mut st = self.store.lock().unwrap();
        let res = st.toggle_task(task_id);
        if res.is_some() {
            let _ = self.repository.save(&st);
        }
        res
    }

    /// Deletes a task by ID.
    pub fn delete_task(&self, task_id: &str) -> bool {
        let mut st = self.store.lock().unwrap();
        let deleted = st.remove_task(task_id);
        if deleted {
            let _ = self.repository.save(&st);
        }
        deleted
    }
}

/// Service managing task categories and board lists.
pub struct ListService {
    store: Arc<Mutex<AppStateStore>>,
    repository: Arc<dyn StorageRepository>,
}

impl ListService {
    pub fn new(store: Arc<Mutex<AppStateStore>>, repository: Arc<dyn StorageRepository>) -> Self {
        Self { store, repository }
    }

    /// Creates and persists a new custom list.
    pub fn create_list(&self, name: &str, color: &str) -> TaskList {
        let new_list = TaskList::new(name, color);
        let mut st = self.store.lock().unwrap();
        st.add_list(new_list.clone());
        let _ = self.repository.save(&st);
        new_list
    }

    /// Deletes a list and all associated tasks.
    pub fn delete_list(&self, list_id: &str) {
        let mut st = self.store.lock().unwrap();
        st.remove_list_cascade(list_id);
        let _ = self.repository.save(&st);
    }
}

/// Thread-safe focus sprint countdown timer service.
pub struct TimerService {
    running: Arc<AtomicBool>,
    remaining_seconds: Arc<AtomicU32>,
    initial_duration_seconds: u32,
}

impl TimerService {
    pub fn new(minutes: u32) -> Self {
        let duration = minutes * 60;
        Self {
            running: Arc::new(AtomicBool::new(false)),
            remaining_seconds: Arc::new(AtomicU32::new(duration)),
            initial_duration_seconds: duration,
        }
    }

    /// Toggles timer running state. Returns the new running boolean.
    pub fn toggle(&self) -> bool {
        let cur = self.running.load(Ordering::SeqCst);
        let next = !cur;
        self.running.store(next, Ordering::SeqCst);
        next
    }

    /// Resets timer back to initial duration and pauses.
    pub fn reset(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.remaining_seconds.store(self.initial_duration_seconds, Ordering::SeqCst);
    }

    /// Formats remaining duration into MM:SS.
    pub fn formatted_display(&self) -> String {
        let total_secs = self.remaining_seconds.load(Ordering::SeqCst);
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!("{:02}:{:02}", mins, secs)
    }

    /// Current running state.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
