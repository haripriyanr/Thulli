//! List lifecycle: create, duplicate, archive, delete.

use std::sync::{Arc, Mutex};

use crate::models::{AppStateStore, TaskList};
use crate::repository::StorageRepository;
use crate::services::store_handle::StoreHandle;

/// Service managing task lists.
pub struct ListService {
    handle: StoreHandle,
}

impl ListService {
    pub fn new(store: Arc<Mutex<AppStateStore>>, repository: Arc<dyn StorageRepository>) -> Self {
        Self { handle: StoreHandle::new(store, repository) }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }

    /// Creates and persists a new custom list.
    pub fn create_list(&self, name: &str, color: &str) -> TaskList {
        let new_list = TaskList::new(name, color);
        self.handle.mutate(|st| st.add_list(new_list.clone()));
        new_list
    }

    /// Deletes a list and all associated tasks.
    pub fn delete_list(&self, list_id: &str) {
        self.handle.mutate(|st| st.remove_list_cascade(list_id));
    }
}
