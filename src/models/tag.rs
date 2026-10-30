//! Reusable task tags.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A tag with an optional emoji and a colour, managed in preferences.
/// Tasks refer to tags by id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub color: String,
}

impl Tag {
    pub fn new(name: impl Into<String>, color: impl Into<String>) -> Self {
        Self { id: Uuid::new_v4().to_string(), name: name.into(), emoji: None, color: color.into() }
    }

    pub fn with_emoji(mut self, emoji: impl Into<String>) -> Self {
        self.emoji = Some(emoji.into());
        self
    }
}
