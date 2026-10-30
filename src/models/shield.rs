//! Distraction shield rules.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn default_true() -> bool {
    true
}

/// Application blocked while a focus session runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedApp {
    pub id: String,
    pub name: String,
    pub process_name: String,
    #[serde(default = "default_true")]
    pub active: bool,
}

impl BlockedApp {
    pub fn new(name: impl Into<String>, process_name: impl Into<String>) -> Self {
        Self { id: Uuid::new_v4().to_string(), name: name.into(), process_name: process_name.into(), active: true }
    }

    pub fn toggle_active(&mut self) -> bool {
        self.active = !self.active;
        self.active
    }
}

/// Website blocked while a focus session runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedSite {
    pub id: String,
    /// Host name such as `example.com`, without scheme or path.
    pub domain: String,
    #[serde(default = "default_true")]
    pub active: bool,
}

impl BlockedSite {
    pub fn new(domain: impl Into<String>) -> Self {
        Self { id: Uuid::new_v4().to_string(), domain: domain.into(), active: true }
    }

    pub fn toggle_active(&mut self) -> bool {
        self.active = !self.active;
        self.active
    }
}
