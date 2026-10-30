//! Assistant conversations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    #[default]
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    #[serde(default)]
    pub role: ChatRole,
    #[serde(default)]
    pub text: String,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

impl ChatMessage {
    pub fn new(role: ChatRole, text: impl Into<String>) -> Self {
        Self { id: Uuid::new_v4().to_string(), role, text: text.into(), created_at: Utc::now() }
    }
}

/// One conversation with the on-device assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatThread {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

impl ChatThread {
    pub fn new(title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self { id: Uuid::new_v4().to_string(), title: title.into(), messages: Vec::new(), created_at: now, updated_at: now }
    }

    pub fn push(&mut self, role: ChatRole, text: impl Into<String>) -> &ChatMessage {
        self.messages.push(ChatMessage::new(role, text));
        self.updated_at = Utc::now();
        self.messages.last().expect("just pushed")
    }
}
