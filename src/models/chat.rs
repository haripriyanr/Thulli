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

/// What kind of fact a memory holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    #[default]
    Preference,
    Commitment,
    PersonOrProject,
    Pattern,
}

impl MemoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryKind::Preference => "preference",
            MemoryKind::Commitment => "commitment",
            MemoryKind::PersonOrProject => "person_or_project",
            MemoryKind::Pattern => "pattern",
        }
    }
}

/// An earlier wording of a memory, kept so the user can see its history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRevision {
    pub text: String,
    pub at: DateTime<Utc>,
}

/// A fact the assistant keeps about the user. The user can add, edit, archive
/// and review the history of every memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMemory {
    pub id: String,
    #[serde(default)]
    pub kind: MemoryKind,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub history: Vec<MemoryRevision>,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

impl AssistantMemory {
    pub fn new(kind: MemoryKind, text: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            kind,
            text: text.into(),
            archived: false,
            history: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Replaces the text, keeping the old wording in the history.
    pub fn edit(&mut self, text: impl Into<String>) {
        let old = std::mem::replace(&mut self.text, text.into());
        self.history.push(MemoryRevision { text: old, at: Utc::now() });
        self.updated_at = Utc::now();
    }

    pub fn archive(&mut self) {
        self.archived = true;
        self.updated_at = Utc::now();
    }
}

/// The in-app prompts the assistant may show (never during focus).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKind {
    PlanMyDay,
    WelcomeBack,
    TakeABreak,
    WindDown,
    Prioritize,
}

/// Bookkeeping so a suggestion is not repeated too often.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionState {
    pub kind: SuggestionKind,
    #[serde(default)]
    pub last_shown_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub snoozed_until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub dismissed_count: u32,
}

impl SuggestionState {
    pub fn new(kind: SuggestionKind) -> Self {
        Self { kind, last_shown_at: None, snoozed_until: None, dismissed_count: 0 }
    }

    pub fn mark_shown(&mut self) {
        self.last_shown_at = Some(Utc::now());
    }

    pub fn dismiss_until(&mut self, until: DateTime<Utc>) {
        self.dismissed_count += 1;
        self.snoozed_until = Some(until);
    }

    pub fn is_snoozed(&self, now: DateTime<Utc>) -> bool {
        self.snoozed_until.is_some_and(|t| t > now)
    }
}
