//! Shared access to the application state and its repository.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::models::AppStateStore;
use crate::repository::StorageRepository;

/// Cloneable handle every service uses to read and change the root aggregate.
///
/// `mutate` saves after each change so services never forget to persist.
#[derive(Clone)]
pub struct StoreHandle {
    store: Arc<Mutex<AppStateStore>>,
    repository: Arc<dyn StorageRepository>,
}

impl StoreHandle {
    pub fn new(store: Arc<Mutex<AppStateStore>>, repository: Arc<dyn StorageRepository>) -> Self {
        Self { store, repository }
    }

    fn lock(&self) -> MutexGuard<'_, AppStateStore> {
        self.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Runs `f` with read access to the state.
    pub fn read<R>(&self, f: impl FnOnce(&AppStateStore) -> R) -> R {
        f(&self.lock())
    }

    /// Runs `f` with write access, then saves. Save failures are logged.
    pub fn mutate<R>(&self, f: impl FnOnce(&mut AppStateStore) -> R) -> R {
        let mut guard = self.lock();
        let result = f(&mut guard);
        if let Err(e) = self.repository.save(&guard) {
            eprintln!("[Storage] Save failed: {e}");
        }
        result
    }

    /// Writes the current state without changing it.
    pub fn save(&self) {
        let guard = self.lock();
        if let Err(e) = self.repository.save(&guard) {
            eprintln!("[Storage] Save failed: {e}");
        }
    }
}
