//! Reading and changing user preferences.
//!
//! Foundation stub: add behaviour here. The service reads and writes state
//! through the shared [`StoreHandle`].

use crate::services::store_handle::StoreHandle;

/// Reading and changing user preferences.
pub struct PreferencesService {
    handle: StoreHandle,
}

impl PreferencesService {
    pub fn new(handle: StoreHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }
}
