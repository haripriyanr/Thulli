//! Undo and redo seam: an action history over the shared state.
//!
//! A service that wants its change to be undoable builds an
//! [`UndoableAction`] describing the change and hands it to
//! [`ActionHistory::record`] after applying it. The board and matrix screens
//! then call `undo` and `redo`. Only one concrete action ships with the
//! foundation (`MoveSection`); add more next to the service they belong to.

use std::sync::Mutex;

use crate::models::{AppStateStore, TaskSection};
use crate::services::store_handle::StoreHandle;

/// Most actions kept; older ones are dropped.
const MAX_HISTORY: usize = 100;

/// A reversible change to the application state.
pub trait UndoableAction: Send {
    /// Short description, such as "Move task".
    fn label(&self) -> String;
    /// Reverses the change.
    fn undo(&self, store: &mut AppStateStore);
    /// Applies the change again after an undo.
    fn redo(&self, store: &mut AppStateStore);
}

#[derive(Default)]
struct Stacks {
    undo: Vec<Box<dyn UndoableAction>>,
    redo: Vec<Box<dyn UndoableAction>>,
}

/// Linear undo and redo history. Recording a new action clears the redo stack.
pub struct ActionHistory {
    handle: StoreHandle,
    stacks: Mutex<Stacks>,
}

impl ActionHistory {
    pub fn new(handle: StoreHandle) -> Self {
        Self { handle, stacks: Mutex::new(Stacks::default()) }
    }

    fn stacks(&self) -> std::sync::MutexGuard<'_, Stacks> {
        self.stacks.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Records an action that has already been applied.
    pub fn record(&self, action: Box<dyn UndoableAction>) {
        let mut stacks = self.stacks();
        stacks.redo.clear();
        stacks.undo.push(action);
        if stacks.undo.len() > MAX_HISTORY {
            stacks.undo.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.stacks().undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.stacks().redo.is_empty()
    }

    /// Undoes the latest action. Returns its label, or `None` when empty.
    pub fn undo(&self) -> Option<String> {
        let action = self.stacks().undo.pop()?;
        self.handle.mutate(|st| action.undo(st));
        let label = action.label();
        self.stacks().redo.push(action);
        Some(label)
    }

    /// Redoes the latest undone action. Returns its label, or `None` when empty.
    pub fn redo(&self) -> Option<String> {
        let action = self.stacks().redo.pop()?;
        self.handle.mutate(|st| action.redo(st));
        let label = action.label();
        self.stacks().undo.push(action);
        Some(label)
    }
}

/// Moving a task between board sections.
pub struct MoveSection {
    pub task_id: String,
    pub from: TaskSection,
    pub to: TaskSection,
}

impl UndoableAction for MoveSection {
    fn label(&self) -> String {
        "Move task".to_string()
    }

    fn undo(&self, store: &mut AppStateStore) {
        if let Some(t) = store.task_mut(&self.task_id) {
            t.move_to_section(self.from);
        }
    }

    fn redo(&self, store: &mut AppStateStore) {
        if let Some(t) = store.task_mut(&self.task_id) {
            t.move_to_section(self.to);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Task;
    use crate::repository::{LoadReport, StorageRepository};
    use std::sync::Arc;

    struct MemoryRepo;
    impl StorageRepository for MemoryRepo {
        fn load_with_report(&self) -> LoadReport {
            LoadReport { store: AppStateStore::default(), recovery: None }
        }
        fn save(&self, _: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
    }

    #[test]
    fn undo_and_redo_move_a_task_back_and_forth() {
        let handle = StoreHandle::new(Arc::new(Mutex::new(AppStateStore::default())), Arc::new(MemoryRepo));
        let id = handle.mutate(|st| st.add_task(Task::new("l", "x", 5, TaskSection::Backlog)).id);
        let history = ActionHistory::new(handle.clone());

        handle.mutate(|st| st.task_mut(&id).unwrap().move_to_section(TaskSection::Today));
        history.record(Box::new(MoveSection { task_id: id.clone(), from: TaskSection::Backlog, to: TaskSection::Today }));
        assert!(history.can_undo() && !history.can_redo());

        assert_eq!(history.undo().as_deref(), Some("Move task"));
        assert_eq!(handle.read(|s| s.task(&id).unwrap().section), TaskSection::Backlog);
        assert!(history.redo().is_some());
        assert_eq!(handle.read(|s| s.task(&id).unwrap().section), TaskSection::Today);
        assert!(history.redo().is_none());
    }
}
