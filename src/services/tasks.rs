//! Task workflows: smart entry, status changes and section moves.

use std::sync::{Arc, Mutex};

use crate::models::{AppStateStore, Task, TaskList, TaskSection, TaskStatus};
use crate::parser::parse_task_title;
use crate::repository::StorageRepository;
use crate::services::store_handle::StoreHandle;

/// Service coordinating task creation, state changes and list associations.
pub struct TaskService {
    handle: StoreHandle,
}

impl TaskService {
    pub fn new(store: Arc<Mutex<AppStateStore>>, repository: Arc<dyn StorageRepository>) -> Self {
        Self { handle: StoreHandle::new(store, repository) }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }

    /// Parses user input, resolves list fallback, and persists the new task.
    /// `default_section` is used when the text carries no section tag.
    pub fn add_smart_task(&self, raw_input: &str, list_id: &str, default_section: TaskSection) -> Option<Task> {
        let cleaned = raw_input.trim();
        if cleaned.is_empty() {
            return None;
        }

        self.handle.mutate(|st| {
            let parsed = if st.preferences.auto_parse_estimates {
                let p = parse_task_title(cleaned, default_section.as_str());
                (p.cleaned_title, p.estimate_mins, TaskSection::from_legacy_board(&p.target_board))
            } else {
                (cleaned.to_string(), 0, default_section)
            };

            let effective_list_id = if list_id.is_empty() {
                match st.get_primary_list() {
                    Some(first) => first.id.clone(),
                    None => st.add_list(TaskList::new_personal()),
                }
            } else {
                list_id.to_string()
            };

            let mut task = Task::new(effective_list_id, parsed.0, parsed.1, parsed.2);
            task.sort_order = st.tasks.iter().map(|t| t.sort_order).max().map_or(0, |m| m + 1);
            Some(st.add_task(task))
        })
    }

    /// Toggles task completion and saves state.
    pub fn toggle_task(&self, task_id: &str) -> Option<bool> {
        self.handle.mutate(|st| st.toggle_task(task_id))
    }

    /// Sets an explicit status (completed, cancelled, waiting, open).
    pub fn set_status(&self, task_id: &str, status: TaskStatus) -> bool {
        self.handle.mutate(|st| st.task_mut(task_id).map(|t| t.set_status(status)).is_some())
    }

    /// Moves a task to another planning section.
    pub fn move_to_section(&self, task_id: &str, section: TaskSection) -> bool {
        self.handle.mutate(|st| st.task_mut(task_id).map(|t| t.move_to_section(section)).is_some())
    }

    /// Prepares a task for a focus run: it moves to Today and is reopened if
    /// it was waiting or cancelled. Returns the task's estimate in minutes.
    pub fn start_focus(&self, task_id: &str) -> Option<u32> {
        self.handle.mutate(|st| {
            st.task_mut(task_id).map(|t| {
                t.prepare_for_focus();
                t.estimate_mins
            })
        })
    }

    /// Deletes a task and its breakdown steps.
    pub fn delete_task(&self, task_id: &str) -> bool {
        self.handle.mutate(|st| st.remove_task(task_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn service() -> TaskService {
        TaskService::new(Arc::new(Mutex::new(AppStateStore::default())), Arc::new(MemoryRepo))
    }

    #[test]
    fn smart_entry_parses_estimate_and_section() {
        let svc = service();
        let t = svc.add_smart_task("Write report 45m @week", "", TaskSection::Today).unwrap();
        assert_eq!(t.title, "Write report");
        assert_eq!(t.estimate_mins, 45);
        assert_eq!(t.section, TaskSection::ThisWeek);
    }

    #[test]
    fn parsing_can_be_switched_off() {
        let svc = service();
        svc.handle().mutate(|st| st.preferences.auto_parse_estimates = false);
        let t = svc.add_smart_task("Write report 45m", "", TaskSection::Backlog).unwrap();
        assert_eq!(t.title, "Write report 45m");
        assert_eq!(t.section, TaskSection::Backlog);
    }

    #[test]
    fn starting_focus_moves_task_to_today() {
        let svc = service();
        let t = svc.add_smart_task("x", "", TaskSection::Backlog).unwrap();
        svc.set_status(&t.id, TaskStatus::Waiting);
        assert_eq!(svc.start_focus(&t.id), Some(25));
        let t = svc.handle().read(|s| s.task(&t.id).cloned()).unwrap();
        assert_eq!((t.section, t.status), (TaskSection::Today, TaskStatus::Open));
        assert!(svc.start_focus("missing").is_none());
    }

    #[test]
    fn status_and_section_changes_apply() {
        let svc = service();
        let t = svc.add_smart_task("x", "", TaskSection::Today).unwrap();
        assert!(svc.set_status(&t.id, TaskStatus::Waiting));
        assert!(svc.move_to_section(&t.id, TaskSection::Backlog));
        let st = svc.handle().read(|s| s.task(&t.id).cloned()).unwrap();
        assert_eq!((st.status, st.section), (TaskStatus::Waiting, TaskSection::Backlog));
    }
}
