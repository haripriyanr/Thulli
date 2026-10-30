//! Read-only reports built from tracked time and finished tasks.
//!
//! Foundation stub: add behaviour here. The service reads and writes state
//! through the shared [`StoreHandle`].

use crate::services::store_handle::StoreHandle;

/// Read-only reports built from tracked time and finished tasks.
pub struct InsightsService {
    handle: StoreHandle,
}

impl InsightsService {
    pub fn new(handle: StoreHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }
}
