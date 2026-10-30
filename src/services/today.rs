//! Daily workflow: plan, work and wrap up, plus the day timeline.
//!
//! Foundation stub: add behaviour here. The service reads and writes state
//! through the shared [`StoreHandle`].

use crate::services::store_handle::StoreHandle;

/// Daily workflow: plan, work and wrap up, plus the day timeline.
pub struct DayPlanService {
    handle: StoreHandle,
}

impl DayPlanService {
    pub fn new(handle: StoreHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }
}
