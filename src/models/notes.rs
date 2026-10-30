//! Notes, folders, quick capture, meeting notes and the brag book.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A rich-text note. `body` holds the editor's text representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    /// `None` means the Unsorted bucket.
    #[serde(default)]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// Task this note was written for, if any (used by floating notes).
    #[serde(default)]
    pub task_id: Option<String>,
    /// Set when moved to Trash.
    #[serde(default)]
    pub trashed_at: Option<DateTime<Utc>>,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

impl Note {
    pub fn new(title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            folder_id: None,
            title: title.into(),
            body: String::new(),
            task_id: None,
            trashed_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn set_body(&mut self, body: impl Into<String>) {
        self.body = body.into();
        self.updated_at = Utc::now();
    }

    pub fn move_to_trash(&mut self) {
        self.trashed_at = Some(Utc::now());
    }

    pub fn restore(&mut self) {
        self.trashed_at = None;
    }

    pub fn is_trashed(&self) -> bool {
        self.trashed_at.is_some()
    }

    /// Number of other notes that link to this one with `[[Title]]`.
    pub fn backlink_count(&self, all: &[Note]) -> usize {
        let needle = format!("[[{}]]", self.title);
        all.iter().filter(|n| n.id != self.id && !n.is_trashed() && n.body.contains(&needle)).count()
    }
}

/// A folder grouping notes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteFolder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

impl NoteFolder {
    pub fn new(name: impl Into<String>) -> Self {
        Self { id: Uuid::new_v4().to_string(), name: name.into(), sort_order: 0, created_at: Utc::now() }
    }
}

/// One line in the Quick Capture scratch inbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickCapture {
    pub id: String,
    pub text: String,
    /// True once turned into a task or note.
    #[serde(default)]
    pub processed: bool,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

impl QuickCapture {
    pub fn new(text: impl Into<String>) -> Self {
        Self { id: Uuid::new_v4().to_string(), text: text.into(), processed: false, created_at: Utc::now() }
    }
}

/// A dated accomplishment in the Brag Book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BragEntry {
    pub id: String,
    pub text: String,
    pub date: NaiveDate,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

impl BragEntry {
    pub fn new(text: impl Into<String>, date: NaiveDate) -> Self {
        Self { id: Uuid::new_v4().to_string(), text: text.into(), date, created_at: Utc::now() }
    }
}

/// Notes captured during a meeting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingNote {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "Utc::now")]
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
}

impl MeetingNote {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            title: title.into(),
            body: String::new(),
            started_at: Utc::now(),
            ended_at: None,
        }
    }

    pub fn end(&mut self) {
        self.ended_at = Some(Utc::now());
    }
}
