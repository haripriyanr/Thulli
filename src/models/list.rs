//! Task lists (also used as projects).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A user-defined list of tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskList {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: String,
    /// Optional uploaded icon (jpg, png or svg) shown instead of the letter badge.
    #[serde(default)]
    pub icon_path: Option<String>,
    #[serde(default)]
    pub is_archived: bool,
    #[serde(default)]
    pub archived_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

impl TaskList {
    pub fn new(name: impl Into<String>, color: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            color: color.into(),
            icon_path: None,
            is_archived: false,
            archived_at: None,
            sort_order: 0,
            created_at: Utc::now(),
        }
    }

    /// The default "Personal" list.
    pub fn new_personal() -> Self {
        Self::new("Personal", "#6FDBAB")
    }

    pub fn with_icon(mut self, icon_path: impl Into<String>) -> Self {
        self.icon_path = Some(icon_path.into());
        self
    }

    /// A copy with a fresh id and a "copy" suffix, never archived.
    pub fn duplicate(&self) -> Self {
        let mut copy = Self::new(format!("{} copy", self.name), self.color.clone());
        copy.icon_path = self.icon_path.clone();
        copy.sort_order = self.sort_order;
        copy
    }

    /// Archives the list, or restores it when already archived.
    pub fn toggle_archived(&mut self) -> bool {
        self.is_archived = !self.is_archived;
        self.archived_at = if self.is_archived { Some(Utc::now()) } else { None };
        self.is_archived
    }

    /// First letter of the name, upper-cased, for the letter badge.
    pub fn badge_letter(&self) -> String {
        self.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()
    }
}
