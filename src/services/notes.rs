//! Notes, folders, quick capture, meeting notes and the brag book.
//!
//! Foundation stub: add behaviour here. The service reads and writes state
//! through the shared [`StoreHandle`].

use crate::services::store_handle::StoreHandle;

/// Notes, folders, quick capture, meeting notes and the brag book.
pub struct NotesService {
    handle: StoreHandle,
}

impl NotesService {
    pub fn new(handle: StoreHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }
}
