//! On-device assistant conversations.
//!
//! Foundation stub: add behaviour here. The service reads and writes state
//! through the shared [`StoreHandle`].

use crate::services::store_handle::StoreHandle;

/// On-device assistant conversations.
pub struct AssistantService {
    handle: StoreHandle,
}

impl AssistantService {
    pub fn new(handle: StoreHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }
}
