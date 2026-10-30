//! Distraction shield rules and enforcement while a session runs.
//!
//! Foundation stub: add behaviour here. The service reads and writes state
//! through the shared [`StoreHandle`].

use crate::services::store_handle::StoreHandle;

/// Distraction shield rules and enforcement while a session runs.
pub struct ShieldService {
    handle: StoreHandle,
}

impl ShieldService {
    pub fn new(handle: StoreHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }
}
