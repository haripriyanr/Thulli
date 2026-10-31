//! Notes, folders, quick capture, meeting notes and the brag book.
//!
//! Two parts live here:
//!
//! * [`NoteEditor`]: a block model for note bodies. A body is stored as plain
//!   markdown-like text (headings, bullets, numbered lists, checklists, code
//!   blocks and link lines); the editor turns it into [`Block`]s, applies edits
//!   (Enter, Tab, slash commands, ...) and writes it back. It has no UI or
//!   storage dependency, so it is fully unit tested.
//! * [`NotesService`]: note, folder, capture, meeting and brag workflows over
//!   the shared [`StoreHandle`].

use chrono::{DateTime, Duration, Local, NaiveDate, Utc};

use crate::models::{BragEntry, MeetingNote, Note, NoteFolder, QuickCapture, Task, TaskList, TaskSection};
use crate::services::store_handle::StoreHandle;

// ---------------------------------------------------------------------------
// Block model
// ---------------------------------------------------------------------------

/// What a block looks like and how it is written to text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    /// Levels 1 to 3.
    Heading(u8),
    Bullet,
    Numbered,
    Todo,
    /// Multi-line, written between fences.
    Code,
    /// A link card: `text` is the title, `url` the address.
    Link,
}

impl BlockKind {
    /// Stable name used by the UI.
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockKind::Paragraph => "text",
            BlockKind::Heading(1) => "h1",
            BlockKind::Heading(2) => "h2",
            BlockKind::Heading(_) => "h3",
            BlockKind::Bullet => "bullet",
            BlockKind::Numbered => "numbered",
            BlockKind::Todo => "todo",
            BlockKind::Code => "code",
            BlockKind::Link => "link",
        }
    }

    fn is_list(&self) -> bool {
        matches!(self, BlockKind::Bullet | BlockKind::Numbered | BlockKind::Todo)
    }

    fn is_plain_text(&self) -> bool {
        !matches!(self, BlockKind::Code | BlockKind::Link)
    }
}

/// One editable block of a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Nesting level for list items (0 to [`MAX_INDENT`]).
    pub indent: u8,
    /// Only meaningful for [`BlockKind::Todo`].
    pub checked: bool,
    pub text: String,
    /// Only meaningful for [`BlockKind::Link`].
    pub url: String,
}

/// Deepest list nesting.
pub const MAX_INDENT: u8 = 4;

impl Block {
    pub fn new(kind: BlockKind, text: impl Into<String>) -> Self {
        Self { kind, indent: 0, checked: false, text: text.into(), url: String::new() }
    }

    pub fn paragraph(text: impl Into<String>) -> Self {
        Self::new(BlockKind::Paragraph, text)
    }
}

/// Result of an edit: which block should take the cursor, and whether the
/// block list changed shape (so the UI must rebuild rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EditEffect {
    pub focus: Option<usize>,
    pub structural: bool,
}

impl EditEffect {
    fn none() -> Self {
        Self::default()
    }
    fn at(index: usize) -> Self {
        Self { focus: Some(index), structural: true }
    }
}

/// A slash command offered while typing `/` in an empty block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCommand {
    pub id: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
}

const SLASH_COMMANDS: [SlashCommand; 9] = [
    SlashCommand { id: "text", label: "Text", hint: "Plain paragraph" },
    SlashCommand { id: "h1", label: "Heading 1", hint: "Big section title" },
    SlashCommand { id: "h2", label: "Heading 2", hint: "Medium section title" },
    SlashCommand { id: "h3", label: "Heading 3", hint: "Small section title" },
    SlashCommand { id: "bullet", label: "Bullet list", hint: "Simple bulleted list" },
    SlashCommand { id: "numbered", label: "Numbered list", hint: "List with numbers" },
    SlashCommand { id: "todo", label: "Checklist", hint: "Track things to do" },
    SlashCommand { id: "code", label: "Code block", hint: "Monospaced text" },
    SlashCommand { id: "link", label: "Link", hint: "A link card" },
];

/// Slash commands whose label or id contains `query` (case-insensitive).
/// `query` is the text after the `/`.
pub fn slash_commands(query: &str) -> Vec<SlashCommand> {
    let q = query.trim().to_lowercase();
    SLASH_COMMANDS
        .iter()
        .copied()
        .filter(|c| q.is_empty() || c.id.contains(&q) || c.label.to_lowercase().contains(&q))
        .collect()
}

/// Block-based editing session over one note body.
#[derive(Debug, Clone, Default)]
pub struct NoteEditor {
    blocks: Vec<Block>,
}

impl NoteEditor {
    /// Parses a stored body. An empty body gives one empty paragraph.
    pub fn from_body(body: &str) -> Self {
        let mut blocks = parse_body(body);
        if blocks.is_empty() {
            blocks.push(Block::paragraph(""));
        }
        Self { blocks }
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn block(&self, index: usize) -> Option<&Block> {
        self.blocks.get(index)
    }

    /// The text form written to storage.
    pub fn to_body(&self) -> String {
        serialize_blocks(&self.blocks)
    }

    /// Marker shown in front of a block: a bullet dot or the list number.
    /// Empty for other kinds.
    pub fn marker(&self, index: usize) -> String {
        match self.blocks.get(index) {
            Some(b) if b.kind == BlockKind::Bullet => "\u{2022}".to_string(),
            Some(b) if b.kind == BlockKind::Numbered => {
                let mut n = 1;
                let mut i = index;
                while i > 0 {
                    let prev = &self.blocks[i - 1];
                    if prev.kind == BlockKind::Numbered && prev.indent == b.indent {
                        n += 1;
                    } else if prev.kind.is_list() && prev.indent > b.indent {
                        // deeper children do not break the sequence
                    } else {
                        break;
                    }
                    i -= 1;
                }
                format!("{n}.")
            }
            _ => String::new(),
        }
    }

    /// Replaces a block's text. Handles pasted line breaks (split into blocks)
    /// and typed shortcuts such as `- `, `1. `, `# `, `[] ` and three backticks.
    pub fn set_text(&mut self, index: usize, text: &str) -> EditEffect {
        let Some(block) = self.blocks.get_mut(index) else { return EditEffect::none() };
        if block.kind == BlockKind::Code || block.kind == BlockKind::Link {
            block.text = text.to_string();
            return EditEffect::none();
        }

        if text.contains('\n') {
            let normalized = text.replace("\r\n", "\n");
            let mut lines = normalized.split('\n');
            block.text = lines.next().unwrap_or("").to_string();
            let (kind, indent) = (block.kind, block.indent);
            let mut at = index;
            for line in lines {
                at += 1;
                let mut b = Block::new(if kind.is_list() { kind } else { BlockKind::Paragraph }, line);
                b.indent = indent;
                self.blocks.insert(at, b);
            }
            return EditEffect::at(at);
        }

        block.text = text.to_string();
        if block.kind == BlockKind::Paragraph {
            if let Some((kind, rest)) = shortcut(text) {
                block.kind = kind;
                block.text = rest.to_string();
                return EditEffect::at(index);
            }
        }
        EditEffect::none()
    }

    /// Enter pressed at byte offset `cursor` of block `index`.
    pub fn enter(&mut self, index: usize, cursor: usize) -> EditEffect {
        let Some(block) = self.blocks.get(index) else { return EditEffect::none() };
        let kind = block.kind;
        if kind == BlockKind::Code {
            return EditEffect::none();
        }
        // Enter on an empty list item leaves the list (or goes up a level).
        if kind.is_list() && block.text.is_empty() {
            let b = &mut self.blocks[index];
            if b.indent > 0 {
                b.indent -= 1;
            } else {
                b.kind = BlockKind::Paragraph;
                b.checked = false;
            }
            return EditEffect::at(index);
        }
        let cursor = clamp_to_boundary(&block.text, cursor);
        let tail = block.text[cursor..].to_string();
        let new_kind = if kind.is_list() { kind } else { BlockKind::Paragraph };
        let indent = block.indent;
        self.blocks[index].text.truncate(cursor);
        let mut next = Block::new(new_kind, tail);
        next.indent = if new_kind.is_list() { indent } else { 0 };
        self.blocks.insert(index + 1, next);
        EditEffect::at(index + 1)
    }

    /// Backspace pressed with the cursor at the very start of block `index`.
    /// Returns `None` when nothing special applies (normal deletion).
    pub fn backspace_at_start(&mut self, index: usize) -> Option<EditEffect> {
        let block = self.blocks.get(index)?;
        if block.kind != BlockKind::Paragraph && block.kind != BlockKind::Link {
            // A formatted block drops its formatting first.
            let b = &mut self.blocks[index];
            if b.indent > 0 {
                b.indent -= 1;
            } else {
                b.kind = BlockKind::Paragraph;
                b.checked = false;
            }
            return Some(EditEffect::at(index));
        }
        if index == 0 {
            return None;
        }
        let prev_ok = self.blocks[index - 1].kind.is_plain_text();
        if block.kind == BlockKind::Paragraph && prev_ok {
            let removed = self.blocks.remove(index);
            self.blocks[index - 1].text.push_str(&removed.text);
            return Some(EditEffect::at(index - 1));
        }
        if block.text.is_empty() {
            self.blocks.remove(index);
            return Some(EditEffect::at(index - 1));
        }
        None
    }

    /// Removes a block, keeping at least one.
    pub fn delete_block(&mut self, index: usize) -> EditEffect {
        if index >= self.blocks.len() {
            return EditEffect::none();
        }
        self.blocks.remove(index);
        if self.blocks.is_empty() {
            self.blocks.push(Block::paragraph(""));
        }
        EditEffect::at(index.min(self.blocks.len() - 1))
    }

    /// Tab: nest a list item one level deeper than the item above it.
    pub fn indent(&mut self, index: usize) -> EditEffect {
        let ceiling = match index.checked_sub(1).and_then(|i| self.blocks.get(i)) {
            Some(prev) if prev.kind.is_list() => (prev.indent + 1).min(MAX_INDENT),
            _ => 0,
        };
        match self.blocks.get_mut(index) {
            Some(b) if b.kind.is_list() && b.indent < ceiling => {
                b.indent += 1;
                EditEffect::at(index)
            }
            _ => EditEffect::none(),
        }
    }

    /// Shift+Tab: move a list item one level out.
    pub fn outdent(&mut self, index: usize) -> EditEffect {
        match self.blocks.get_mut(index) {
            Some(b) if b.kind.is_list() && b.indent > 0 => {
                b.indent -= 1;
                EditEffect::at(index)
            }
            _ => EditEffect::none(),
        }
    }

    /// Ctrl+Enter: make the block a checklist item, or tick/untick it if it
    /// already is one.
    pub fn toggle_todo(&mut self, index: usize) -> EditEffect {
        match self.blocks.get_mut(index) {
            Some(b) if b.kind == BlockKind::Todo => {
                b.checked = !b.checked;
                EditEffect::at(index)
            }
            Some(b) if b.kind.is_plain_text() => {
                b.kind = BlockKind::Todo;
                b.checked = false;
                EditEffect::at(index)
            }
            _ => EditEffect::none(),
        }
    }

    /// Ticks or unticks a checklist item (used by the checkbox).
    pub fn set_checked(&mut self, index: usize, checked: bool) {
        if let Some(b) = self.blocks.get_mut(index) {
            if b.kind == BlockKind::Todo {
                b.checked = checked;
            }
        }
    }

    /// Applies a slash command to a block, clearing the typed `/query`.
    pub fn apply_slash(&mut self, index: usize, command_id: &str) -> EditEffect {
        let Some(block) = self.blocks.get_mut(index) else { return EditEffect::none() };
        block.text.clear();
        block.checked = false;
        match command_id {
            "text" => block.kind = BlockKind::Paragraph,
            "h1" => block.kind = BlockKind::Heading(1),
            "h2" => block.kind = BlockKind::Heading(2),
            "h3" => block.kind = BlockKind::Heading(3),
            "bullet" => block.kind = BlockKind::Bullet,
            "numbered" => block.kind = BlockKind::Numbered,
            "todo" => block.kind = BlockKind::Todo,
            "code" => block.kind = BlockKind::Code,
            "link" => {
                block.kind = BlockKind::Link;
                block.url = "https://".to_string();
            }
            _ => {}
        }
        if !block.kind.is_list() {
            block.indent = 0;
        }
        EditEffect::at(index)
    }

    /// Ctrl+Alt+K: turn `text` (the selection, or the block text) into a link
    /// card pointing at `url`. When `text` is the whole block text the block is
    /// replaced; otherwise the card is inserted after it.
    pub fn add_link(&mut self, index: usize, text: &str, url: &str) -> EditEffect {
        let url = normalize_url(url);
        if url.is_empty() || index >= self.blocks.len() {
            return EditEffect::none();
        }
        let title = if text.trim().is_empty() { url.clone() } else { text.trim().to_string() };
        let mut card = Block::new(BlockKind::Link, title);
        card.url = url;
        let whole = self.blocks[index].text.trim() == text.trim() || text.trim().is_empty();
        if whole && self.blocks[index].kind.is_plain_text() && !self.blocks[index].kind.is_list() {
            self.blocks[index] = card;
            if index + 1 == self.blocks.len() {
                self.blocks.push(Block::paragraph(""));
            }
            return EditEffect::at(index);
        }
        self.blocks.insert(index + 1, card);
        EditEffect::at(index + 1)
    }

    /// Sets the address of a link block.
    pub fn set_url(&mut self, index: usize, url: &str) {
        if let Some(b) = self.blocks.get_mut(index) {
            if b.kind == BlockKind::Link {
                b.url = url.trim().to_string();
            }
        }
    }

    /// Appends text as a new paragraph at the end (used by "send to note").
    pub fn append_paragraph(&mut self, text: &str) {
        if self.blocks.len() == 1 && self.blocks[0].text.is_empty() && self.blocks[0].kind == BlockKind::Paragraph {
            self.blocks[0].text = text.to_string();
        } else {
            self.blocks.push(Block::paragraph(text));
        }
    }
}

fn clamp_to_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn normalize_url(url: &str) -> String {
    let u = url.trim();
    if u.is_empty() || u == "https://" || u == "http://" {
        return String::new();
    }
    if u.contains("://") || u.starts_with("mailto:") {
        u.to_string()
    } else {
        format!("https://{u}")
    }
}

/// Typing a markdown shortcut at the start of a paragraph converts it.
fn shortcut(text: &str) -> Option<(BlockKind, &str)> {
    let table: [(&str, BlockKind); 9] = [
        ("### ", BlockKind::Heading(3)),
        ("## ", BlockKind::Heading(2)),
        ("# ", BlockKind::Heading(1)),
        ("[] ", BlockKind::Todo),
        ("[ ] ", BlockKind::Todo),
        ("- ", BlockKind::Bullet),
        ("* ", BlockKind::Bullet),
        ("1. ", BlockKind::Numbered),
        ("```", BlockKind::Code),
    ];
    table.iter().find_map(|(prefix, kind)| text.strip_prefix(prefix).map(|rest| (*kind, rest)))
}

// ---------------------------------------------------------------------------
// Text form
// ---------------------------------------------------------------------------

/// Parses markdown-like text into blocks.
pub fn parse_body(body: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines = body.replace("\r\n", "\n");
    if lines.ends_with('\n') {
        lines.pop();
    }
    if lines.is_empty() {
        return blocks;
    }
    let mut iter = lines.split('\n');
    while let Some(line) = iter.next() {
        if line.trim_start() == "```" || line.starts_with("```") {
            let mut code: Vec<&str> = Vec::new();
            for inner in iter.by_ref() {
                if inner.trim_end() == "```" {
                    break;
                }
                code.push(inner);
            }
            blocks.push(Block::new(BlockKind::Code, code.join("\n")));
            continue;
        }
        blocks.push(parse_line(line));
    }
    blocks
}

fn parse_line(line: &str) -> Block {
    let trimmed = line.trim_start_matches(' ');
    let spaces = line.len() - trimmed.len();
    let indent = ((spaces / 2) as u8).min(MAX_INDENT);

    let list = |kind: BlockKind, rest: &str, checked: bool| {
        let mut b = Block::new(kind, rest);
        b.indent = indent;
        b.checked = checked;
        b
    };
    if let Some(rest) = trimmed.strip_prefix("- [ ] ") {
        return list(BlockKind::Todo, rest, false);
    }
    if let Some(rest) = trimmed.strip_prefix("- [x] ").or_else(|| trimmed.strip_prefix("- [X] ")) {
        return list(BlockKind::Todo, rest, true);
    }
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return list(BlockKind::Bullet, rest, false);
    }
    if let Some(dot) = trimmed.find(". ") {
        if dot > 0 && trimmed[..dot].chars().all(|c| c.is_ascii_digit()) {
            return list(BlockKind::Numbered, &trimmed[dot + 2..], false);
        }
    }
    for (prefix, level) in [("### ", 3u8), ("## ", 2), ("# ", 1)] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Block::new(BlockKind::Heading(level), rest);
        }
    }
    if let Some(rest) = line.strip_prefix('[') {
        if let Some((title, tail)) = rest.split_once("](") {
            if let Some(url) = tail.strip_suffix(')') {
                let mut b = Block::new(BlockKind::Link, title);
                b.url = url.to_string();
                return b;
            }
        }
    }
    // An escaped paragraph that would otherwise look like formatting.
    Block::paragraph(line.strip_prefix('\\').unwrap_or(line))
}

/// Writes blocks to markdown-like text (no trailing newline).
pub fn serialize_blocks(blocks: &[Block]) -> String {
    let mut out: Vec<String> = Vec::with_capacity(blocks.len());
    for b in blocks {
        let pad = "  ".repeat(b.indent as usize);
        out.push(match b.kind {
            BlockKind::Paragraph => {
                if parse_line(&b.text).kind != BlockKind::Paragraph || b.text.starts_with("```") || b.text.starts_with('\\')
                {
                    format!("\\{}", b.text)
                } else {
                    b.text.clone()
                }
            }
            BlockKind::Heading(n) => format!("{} {}", "#".repeat(n.clamp(1, 3) as usize), b.text),
            BlockKind::Bullet => format!("{pad}- {}", b.text),
            BlockKind::Numbered => format!("{pad}1. {}", b.text),
            BlockKind::Todo => format!("{pad}- [{}] {}", if b.checked { 'x' } else { ' ' }, b.text),
            BlockKind::Code => format!("```\n{}\n```", b.text),
            BlockKind::Link => format!("[{}]({})", b.text, b.url),
        });
    }
    out.join("\n")
}

/// First line of a body with formatting marks removed, for list previews.
pub fn preview_of(body: &str) -> String {
    for block in parse_body(body) {
        if block.kind == BlockKind::Code {
            continue;
        }
        let text = block.text.trim();
        if !text.is_empty() {
            return text.chars().take(120).collect();
        }
    }
    String::new()
}

/// Friendly timestamp: "Just now", "12 min ago", "Today 14:05", "Yesterday",
/// "Mar 4" or "Mar 4, 2024".
pub fn format_stamp(at: DateTime<Local>, now: DateTime<Local>) -> String {
    let age = now.signed_duration_since(at);
    if age < Duration::minutes(1) {
        return "Just now".to_string();
    }
    if age < Duration::hours(1) {
        return format!("{} min ago", age.num_minutes());
    }
    let (d, today) = (at.date_naive(), now.date_naive());
    if d == today {
        return format!("Today {}", at.format("%H:%M"));
    }
    if Some(d) == today.pred_opt() {
        return "Yesterday".to_string();
    }
    if d.format("%Y").to_string() == today.format("%Y").to_string() {
        format!("{} {}", at.format("%b"), d.format("%e").to_string().trim())
    } else {
        format!("{}, {}", at.format("%b %d"), at.format("%Y"))
    }
}

/// Splits an optional leading `YYYY-MM-DD` off a brag entry.
fn split_brag_date(input: &str, today: NaiveDate) -> (NaiveDate, String) {
    let t = input.trim();
    if t.len() > 10 && t.is_char_boundary(10) {
        if let Ok(d) = NaiveDate::parse_from_str(&t[..10], "%Y-%m-%d") {
            let rest = t[10..].trim_start_matches([' ', ':', '-']).trim();
            if !rest.is_empty() {
                return (d, rest.to_string());
            }
        }
    }
    (today, t.to_string())
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Which notes a sidebar section lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteScope {
    All,
    Unsorted,
    Trash,
    Folder(String),
}

impl NoteScope {
    /// Maps the UI section name; non-note sections give `None`.
    pub fn from_section(section: &str) -> Option<Self> {
        match section {
            "all" => Some(Self::All),
            "unsorted" => Some(Self::Unsorted),
            "trash" => Some(Self::Trash),
            "quick-capture" | "meetings" | "brag-book" | "" => None,
            id => Some(Self::Folder(id.to_string())),
        }
    }
}

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

    // Notes ---------------------------------------------------------------

    /// Notes in `scope`, most recently edited first.
    pub fn notes_in(&self, scope: &NoteScope) -> Vec<Note> {
        self.handle.read(|st| {
            let mut notes: Vec<Note> = st
                .notes
                .iter()
                .filter(|n| match scope {
                    NoteScope::Trash => n.is_trashed(),
                    NoteScope::All => !n.is_trashed(),
                    NoteScope::Unsorted => !n.is_trashed() && n.folder_id.is_none(),
                    NoteScope::Folder(id) => !n.is_trashed() && n.folder_id.as_deref() == Some(id.as_str()),
                })
                .cloned()
                .collect();
            notes.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            notes
        })
    }

    pub fn note(&self, id: &str) -> Option<Note> {
        self.handle.read(|st| st.notes.iter().find(|n| n.id == id).cloned())
    }

    /// Number of other notes linking to `note` with `[[Title]]`.
    pub fn backlink_count(&self, note: &Note) -> usize {
        self.handle.read(|st| note.backlink_count(&st.notes))
    }

    /// Creates an empty note in `folder` (Unsorted when `None`).
    pub fn create_note(&self, folder_id: Option<&str>) -> Note {
        self.handle.mutate(|st| {
            let mut note = Note::new("Untitled");
            note.folder_id = folder_id.filter(|id| st.note_folders.iter().any(|f| f.id == *id)).map(String::from);
            st.notes.push(note.clone());
            note
        })
    }

    /// Saves title and body. Returns false if the note no longer exists.
    pub fn update_note(&self, id: &str, title: &str, body: &str) -> bool {
        self.handle.mutate(|st| match st.notes.iter_mut().find(|n| n.id == id) {
            Some(n) => {
                if n.title != title || n.body != body {
                    n.title = title.to_string();
                    n.set_body(body);
                }
                true
            }
            None => false,
        })
    }

    pub fn move_note(&self, id: &str, folder_id: Option<&str>) -> bool {
        self.handle.mutate(|st| {
            let valid = folder_id.is_none_or(|f| st.note_folders.iter().any(|x| x.id == f));
            match st.notes.iter_mut().find(|n| n.id == id) {
                Some(n) if valid => {
                    n.folder_id = folder_id.map(String::from);
                    true
                }
                _ => false,
            }
        })
    }

    pub fn trash_note(&self, id: &str) -> bool {
        self.handle.mutate(|st| st.notes.iter_mut().find(|n| n.id == id).map(|n| n.move_to_trash()).is_some())
    }

    pub fn restore_note(&self, id: &str) -> bool {
        self.handle.mutate(|st| st.notes.iter_mut().find(|n| n.id == id).map(|n| n.restore()).is_some())
    }

    /// Permanently removes a note that is in Trash.
    pub fn delete_note_forever(&self, id: &str) -> bool {
        self.handle.mutate(|st| {
            let before = st.notes.len();
            st.notes.retain(|n| !(n.id == id && n.is_trashed()));
            st.notes.len() < before
        })
    }

    /// Permanently removes everything in Trash; returns how many notes went.
    pub fn empty_trash(&self) -> usize {
        self.handle.mutate(|st| {
            let before = st.notes.len();
            st.notes.retain(|n| !n.is_trashed());
            before - st.notes.len()
        })
    }

    // Folders -------------------------------------------------------------

    pub fn folders(&self) -> Vec<NoteFolder> {
        self.handle.read(|st| {
            let mut f = st.note_folders.clone();
            f.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.created_at.cmp(&b.created_at)));
            f
        })
    }

    /// Creates a folder; blank names are refused.
    pub fn create_folder(&self, name: &str) -> Option<NoteFolder> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        self.handle.mutate(|st| {
            let mut f = NoteFolder::new(name);
            f.sort_order = st.note_folders.iter().map(|x| x.sort_order).max().map_or(0, |m| m + 1);
            st.note_folders.push(f.clone());
            Some(f)
        })
    }

    pub fn rename_folder(&self, id: &str, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        self.handle.mutate(|st| st.note_folders.iter_mut().find(|f| f.id == id).map(|f| f.name = name.to_string()).is_some())
    }

    /// Deletes a folder; its notes move to Unsorted.
    pub fn delete_folder(&self, id: &str) -> bool {
        self.handle.mutate(|st| {
            let before = st.note_folders.len();
            st.note_folders.retain(|f| f.id != id);
            for n in st.notes.iter_mut().filter(|n| n.folder_id.as_deref() == Some(id)) {
                n.folder_id = None;
            }
            st.note_folders.len() < before
        })
    }

    /// Live (non-trashed) notes in a folder.
    pub fn folder_note_count(&self, id: &str) -> usize {
        self.handle.read(|st| st.notes.iter().filter(|n| !n.is_trashed() && n.folder_id.as_deref() == Some(id)).count())
    }

    // Task links ----------------------------------------------------------

    /// Notes written for a task, most recent first.
    pub fn notes_for_task(&self, task_id: &str) -> Vec<Note> {
        self.handle.read(|st| {
            let mut v: Vec<Note> =
                st.notes.iter().filter(|n| !n.is_trashed() && n.task_id.as_deref() == Some(task_id)).cloned().collect();
            v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            v
        })
    }

    /// The task's most recent note, creating one titled after the task if none exists.
    pub fn note_for_task(&self, task_id: &str) -> Option<Note> {
        if let Some(n) = self.notes_for_task(task_id).into_iter().next() {
            return Some(n);
        }
        self.handle.mutate(|st| {
            let title = st.task(task_id)?.title.clone();
            let mut note = Note::new(title);
            note.task_id = Some(task_id.to_string());
            st.notes.push(note.clone());
            Some(note)
        })
    }

    /// Links a note to a task, or unlinks it when `task_id` is `None`.
    pub fn set_note_task(&self, note_id: &str, task_id: Option<&str>) -> bool {
        self.handle.mutate(|st| {
            let valid = task_id.is_none_or(|t| st.task(t).is_some());
            match st.notes.iter_mut().find(|n| n.id == note_id) {
                Some(n) if valid => {
                    n.task_id = task_id.map(String::from);
                    true
                }
                _ => false,
            }
        })
    }

    pub fn task_title(&self, task_id: &str) -> Option<String> {
        self.handle.read(|st| st.task(task_id).map(|t| t.title.clone()))
    }

    /// Open tasks a note could be linked to, newest first (title and id).
    pub fn linkable_tasks(&self, limit: usize) -> Vec<(String, String)> {
        self.handle.read(|st| {
            let mut v: Vec<&Task> = st.tasks.iter().filter(|t| !t.is_step() && t.is_pending()).collect();
            v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            v.into_iter().take(limit).map(|t| (t.id.clone(), t.title.clone())).collect()
        })
    }

    /// Turns selected text into a new task in the first list (This Week).
    /// Empty text is refused. The title is the first line, trimmed.
    pub fn create_task_from_text(&self, text: &str) -> Option<Task> {
        let title: String = text.lines().map(str::trim).find(|l| !l.is_empty())?.chars().take(200).collect();
        self.handle.mutate(|st| {
            let list_id = match st.get_primary_list() {
                Some(l) => l.id.clone(),
                None => st.add_list(TaskList::new_personal()),
            };
            let mut task = Task::new(list_id, title, 0, TaskSection::ThisWeek);
            task.sort_order = st.tasks.iter().map(|t| t.sort_order).max().map_or(0, |m| m + 1);
            Some(st.add_task(task))
        })
    }

    // Quick capture -------------------------------------------------------

    /// Adds a line to the inbox; blank text is refused.
    pub fn add_capture(&self, text: &str) -> Option<QuickCapture> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        self.handle.mutate(|st| {
            let c = QuickCapture::new(text);
            st.quick_captures.push(c.clone());
            Some(c)
        })
    }

    /// Unprocessed captures first, newest first within each group.
    pub fn captures(&self) -> Vec<QuickCapture> {
        self.handle.read(|st| {
            let mut v = st.quick_captures.clone();
            v.sort_by(|a, b| a.processed.cmp(&b.processed).then(b.created_at.cmp(&a.created_at)));
            v
        })
    }

    pub fn delete_capture(&self, id: &str) -> bool {
        self.handle.mutate(|st| {
            let before = st.quick_captures.len();
            st.quick_captures.retain(|c| c.id != id);
            st.quick_captures.len() < before
        })
    }

    /// Turns a capture into a task and marks it processed.
    pub fn capture_to_task(&self, id: &str) -> Option<Task> {
        let text = self.handle.read(|st| st.quick_captures.iter().find(|c| c.id == id).map(|c| c.text.clone()))?;
        let task = self.create_task_from_text(&text)?;
        self.mark_capture_processed(id);
        Some(task)
    }

    /// Turns a capture into a note (titled with its first line) and marks it processed.
    pub fn capture_to_note(&self, id: &str) -> Option<Note> {
        let text = self.handle.read(|st| st.quick_captures.iter().find(|c| c.id == id).map(|c| c.text.clone()))?;
        let note = self.handle.mutate(|st| {
            let title: String = text.lines().next().unwrap_or("").chars().take(60).collect();
            let mut n = Note::new(if title.trim().is_empty() { "Untitled".to_string() } else { title });
            n.set_body(text.clone());
            st.notes.push(n.clone());
            n
        });
        self.mark_capture_processed(id);
        Some(note)
    }

    fn mark_capture_processed(&self, id: &str) {
        self.handle.mutate(|st| {
            if let Some(c) = st.quick_captures.iter_mut().find(|c| c.id == id) {
                c.processed = true;
            }
        });
    }

    // Meeting notes -------------------------------------------------------

    pub fn start_meeting(&self, title: &str) -> MeetingNote {
        let title = if title.trim().is_empty() { "Meeting" } else { title.trim() };
        self.handle.mutate(|st| {
            let m = MeetingNote::new(title);
            st.meeting_notes.push(m.clone());
            m
        })
    }

    /// Meetings, newest first.
    pub fn meetings(&self) -> Vec<MeetingNote> {
        self.handle.read(|st| {
            let mut v = st.meeting_notes.clone();
            v.sort_by(|a, b| b.started_at.cmp(&a.started_at));
            v
        })
    }

    pub fn meeting(&self, id: &str) -> Option<MeetingNote> {
        self.handle.read(|st| st.meeting_notes.iter().find(|m| m.id == id).cloned())
    }

    pub fn update_meeting(&self, id: &str, title: &str, body: &str) -> bool {
        self.handle.mutate(|st| match st.meeting_notes.iter_mut().find(|m| m.id == id) {
            Some(m) => {
                m.title = title.to_string();
                m.body = body.to_string();
                true
            }
            None => false,
        })
    }

    pub fn end_meeting(&self, id: &str) -> bool {
        self.handle.mutate(|st| st.meeting_notes.iter_mut().find(|m| m.id == id).map(|m| m.end()).is_some())
    }

    pub fn delete_meeting(&self, id: &str) -> bool {
        self.handle.mutate(|st| {
            let before = st.meeting_notes.len();
            st.meeting_notes.retain(|m| m.id != id);
            st.meeting_notes.len() < before
        })
    }

    // Brag book -----------------------------------------------------------

    /// Adds an accomplishment dated today, or on the date given as a leading
    /// `YYYY-MM-DD`. Blank text is refused.
    pub fn add_brag(&self, input: &str) -> Option<BragEntry> {
        self.add_brag_on(input, Local::now().date_naive())
    }

    pub fn add_brag_on(&self, input: &str, today: NaiveDate) -> Option<BragEntry> {
        let (date, text) = split_brag_date(input, today);
        if text.is_empty() {
            return None;
        }
        self.handle.mutate(|st| {
            let e = BragEntry::new(text, date);
            st.brag_entries.push(e.clone());
            Some(e)
        })
    }

    /// Entries, newest date first.
    pub fn brags(&self) -> Vec<BragEntry> {
        self.handle.read(|st| {
            let mut v = st.brag_entries.clone();
            v.sort_by(|a, b| b.date.cmp(&a.date).then(b.created_at.cmp(&a.created_at)));
            v
        })
    }

    pub fn delete_brag(&self, id: &str) -> bool {
        self.handle.mutate(|st| {
            let before = st.brag_entries.len();
            st.brag_entries.retain(|b| b.id != id);
            st.brag_entries.len() < before
        })
    }
}

/// Convenience for stamping UTC times in the user's zone.
pub fn stamp_utc(at: DateTime<Utc>) -> String {
    format_stamp(at.with_timezone(&Local), Local::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AppStateStore;
    use crate::repository::{LoadReport, StorageRepository};
    use std::sync::{Arc, Mutex};

    struct NullRepo;
    impl StorageRepository for NullRepo {
        fn load_with_report(&self) -> LoadReport {
            LoadReport { store: AppStateStore::default(), recovery: None }
        }
        fn save(&self, _: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
    }

    fn service() -> NotesService {
        let mut store = AppStateStore::default();
        store.migrate();
        let store = Arc::new(Mutex::new(store));
        NotesService::new(StoreHandle::new(store, Arc::new(NullRepo)))
    }

    #[test]
    fn body_round_trips() {
        let body = "# Title\nplain text\n- one\n  - nested\n1. first\n- [ ] open\n- [x] done\n```\nlet x = 1;\nlet y = 2;\n```\n[Docs](https://example.com)";
        let ed = NoteEditor::from_body(body);
        assert_eq!(ed.blocks().len(), 9);
        assert_eq!(ed.blocks()[3].indent, 1);
        assert!(ed.blocks()[6].checked);
        assert_eq!(ed.blocks()[7].text, "let x = 1;\nlet y = 2;");
        assert_eq!(ed.blocks()[8].url, "https://example.com");
        assert_eq!(ed.to_body(), body);
    }

    #[test]
    fn paragraph_that_looks_like_markup_is_escaped() {
        let mut ed = NoteEditor::from_body("");
        ed.blocks[0].text = "- not a bullet".into();
        let body = ed.to_body();
        assert_eq!(NoteEditor::from_body(&body).blocks()[0].kind, BlockKind::Paragraph);
        assert_eq!(NoteEditor::from_body(&body).blocks()[0].text, "- not a bullet");
    }

    #[test]
    fn empty_body_has_one_paragraph() {
        assert_eq!(NoteEditor::from_body("").blocks().len(), 1);
    }

    #[test]
    fn typed_shortcuts_convert_blocks() {
        let mut ed = NoteEditor::from_body("");
        let fx = ed.set_text(0, "- ");
        assert!(fx.structural);
        assert_eq!(ed.blocks()[0].kind, BlockKind::Bullet);
        ed.blocks[0].kind = BlockKind::Paragraph;
        ed.set_text(0, "## ");
        assert_eq!(ed.blocks()[0].kind, BlockKind::Heading(2));
        ed.blocks[0].kind = BlockKind::Paragraph;
        ed.set_text(0, "[] ");
        assert_eq!(ed.blocks()[0].kind, BlockKind::Todo);
    }

    #[test]
    fn enter_continues_lists_splits_text_and_exits() {
        let mut ed = NoteEditor::from_body("- alpha beta");
        let fx = ed.enter(0, 6);
        assert_eq!(fx.focus, Some(1));
        assert_eq!(ed.blocks()[0].text, "alpha ");
        assert_eq!(ed.blocks()[1].text, "beta");
        assert_eq!(ed.blocks()[1].kind, BlockKind::Bullet);
        ed.set_text(1, "");
        ed.enter(1, 0);
        assert_eq!(ed.blocks()[1].kind, BlockKind::Paragraph);
    }

    #[test]
    fn enter_after_heading_gives_paragraph_and_code_ignores_enter() {
        let mut ed = NoteEditor::from_body("# Head");
        ed.enter(0, 6);
        assert_eq!(ed.blocks()[1].kind, BlockKind::Paragraph);
        let mut code = NoteEditor::from_body("```\nx\n```");
        assert!(!code.enter(0, 1).structural);
    }

    #[test]
    fn enter_handles_multibyte_cursor() {
        let mut ed = NoteEditor::from_body("caf\u{e9}");
        ed.enter(0, 4); // inside the two-byte character
        assert_eq!(ed.blocks()[0].text, "caf");
    }

    #[test]
    fn tab_indents_only_under_a_list_item_and_caps() {
        let mut ed = NoteEditor::from_body("- a\n- b\n- c");
        assert!(!ed.indent(0).structural);
        assert!(ed.indent(1).structural);
        assert_eq!(ed.blocks()[1].indent, 1);
        assert!(!ed.indent(1).structural); // already one deeper than parent
        assert!(ed.indent(2).structural);
        assert_eq!(ed.blocks()[2].indent, 1); // one level under "b"
        assert!(ed.indent(2).structural);
        assert_eq!(ed.blocks()[2].indent, 2);
        assert!(!ed.indent(2).structural); // capped at parent + 1
        ed.outdent(2);
        assert_eq!(ed.blocks()[2].indent, 1);
    }

    #[test]
    fn numbering_counts_siblings_across_children() {
        let ed = NoteEditor::from_body("1. a\n  - child\n1. b\n1. c");
        assert_eq!(ed.marker(0), "1.");
        assert_eq!(ed.marker(2), "2.");
        assert_eq!(ed.marker(3), "3.");
        assert_eq!(ed.marker(1), "\u{2022}");
    }

    #[test]
    fn toggle_todo_converts_then_ticks() {
        let mut ed = NoteEditor::from_body("buy milk");
        ed.toggle_todo(0);
        assert_eq!(ed.blocks()[0].kind, BlockKind::Todo);
        assert!(!ed.blocks()[0].checked);
        ed.toggle_todo(0);
        assert!(ed.blocks()[0].checked);
        assert_eq!(ed.to_body(), "- [x] buy milk");
    }

    #[test]
    fn backspace_rules() {
        let mut ed = NoteEditor::from_body("- item\nsecond");
        // second paragraph merges into the bullet above
        ed.backspace_at_start(1);
        assert_eq!(ed.blocks().len(), 1);
        assert_eq!(ed.blocks()[0].text, "itemsecond");
        // formatted block drops formatting
        ed.backspace_at_start(0);
        assert_eq!(ed.blocks()[0].kind, BlockKind::Paragraph);
        // first paragraph: nothing special
        assert!(ed.backspace_at_start(0).is_none());
    }

    #[test]
    fn slash_commands_filter_and_apply() {
        assert_eq!(slash_commands("").len(), 9);
        let found = slash_commands("head");
        assert_eq!(found.iter().map(|c| c.id).collect::<Vec<_>>(), vec!["h1", "h2", "h3"]);
        let mut ed = NoteEditor::from_body("/todo");
        ed.apply_slash(0, "todo");
        assert_eq!(ed.blocks()[0].kind, BlockKind::Todo);
        assert_eq!(ed.blocks()[0].text, "");
        ed.apply_slash(0, "code");
        assert_eq!(ed.blocks()[0].kind, BlockKind::Code);
    }

    #[test]
    fn add_link_replaces_or_inserts() {
        let mut ed = NoteEditor::from_body("read the docs\nmore");
        ed.add_link(0, "read the docs", "example.com/x");
        assert_eq!(ed.blocks()[0].kind, BlockKind::Link);
        assert_eq!(ed.blocks()[0].url, "https://example.com/x");
        let mut ed2 = NoteEditor::from_body("see the docs here");
        ed2.add_link(0, "docs", "https://a.b");
        assert_eq!(ed2.blocks().len(), 2);
        assert_eq!(ed2.blocks()[1].text, "docs");
        assert!(!ed2.add_link(0, "x", "  ").structural);
    }

    #[test]
    fn pasted_lines_split_into_blocks() {
        let mut ed = NoteEditor::from_body("- a");
        let fx = ed.set_text(0, "x\ny\nz");
        assert_eq!(ed.blocks().len(), 3);
        assert_eq!(fx.focus, Some(2));
        assert!(ed.blocks().iter().all(|b| b.kind == BlockKind::Bullet));
    }

    #[test]
    fn preview_skips_blank_and_code() {
        assert_eq!(preview_of("```\nx\n```\n\n## Plan the week"), "Plan the week");
        assert_eq!(preview_of(""), "");
    }

    #[test]
    fn stamps_read_naturally() {
        let now = Local::now();
        assert_eq!(format_stamp(now, now), "Just now");
        assert_eq!(format_stamp(now - Duration::minutes(12), now), "12 min ago");
        assert!(format_stamp(now - Duration::days(400), now).contains(", "));
    }

    #[test]
    fn notes_folders_trash_flow() {
        let s = service();
        let folder = s.create_folder("  Work ").unwrap();
        assert_eq!(folder.name, "Work");
        assert!(s.create_folder("  ").is_none());

        let a = s.create_note(Some(&folder.id));
        let b = s.create_note(None);
        assert_eq!(s.notes_in(&NoteScope::All).len(), 2);
        assert_eq!(s.notes_in(&NoteScope::Unsorted).len(), 1);
        assert_eq!(s.notes_in(&NoteScope::Folder(folder.id.clone())).len(), 1);
        assert_eq!(s.folder_note_count(&folder.id), 1);

        assert!(s.update_note(&a.id, "Plan", "- x"));
        assert_eq!(s.note(&a.id).unwrap().title, "Plan");

        s.trash_note(&b.id);
        assert_eq!(s.notes_in(&NoteScope::Trash).len(), 1);
        assert_eq!(s.notes_in(&NoteScope::All).len(), 1);
        s.restore_note(&b.id);
        assert!(s.notes_in(&NoteScope::Trash).is_empty());

        // only trashed notes can be removed for good
        assert!(!s.delete_note_forever(&b.id));
        s.trash_note(&b.id);
        assert!(s.delete_note_forever(&b.id));

        assert!(s.delete_folder(&folder.id));
        assert_eq!(s.note(&a.id).unwrap().folder_id, None);
        assert_eq!(s.empty_trash(), 0);
    }

    #[test]
    fn move_rejects_unknown_folder() {
        let s = service();
        let n = s.create_note(None);
        assert!(!s.move_note(&n.id, Some("nope")));
        let f = s.create_folder("F").unwrap();
        assert!(s.move_note(&n.id, Some(&f.id)));
    }

    #[test]
    fn backlinks_count_other_notes() {
        let s = service();
        let target = s.create_note(None);
        s.update_note(&target.id, "Roadmap", "");
        let other = s.create_note(None);
        s.update_note(&other.id, "Ideas", "see [[Roadmap]]");
        let target = s.note(&target.id).unwrap();
        assert_eq!(s.backlink_count(&target), 1);
    }

    #[test]
    fn task_linked_notes() {
        let s = service();
        let task = s.create_task_from_text("  \nWrite the report\nsecond line").unwrap();
        assert_eq!(task.title, "Write the report");
        assert!(s.create_task_from_text("   ").is_none());

        let n1 = s.note_for_task(&task.id).unwrap();
        let n2 = s.note_for_task(&task.id).unwrap();
        assert_eq!(n1.id, n2.id);
        assert_eq!(n1.title, "Write the report");
        assert_eq!(s.notes_for_task(&task.id).len(), 1);
        assert!(s.note_for_task("missing").is_none());

        assert!(s.set_note_task(&n1.id, None));
        assert!(s.notes_for_task(&task.id).is_empty());
        assert!(!s.set_note_task(&n1.id, Some("missing")));
        assert!(s.linkable_tasks(5).iter().any(|(id, _)| *id == task.id));
    }

    #[test]
    fn capture_flow() {
        let s = service();
        assert!(s.add_capture("  ").is_none());
        let c1 = s.add_capture("call dentist").unwrap();
        let c2 = s.add_capture("idea: weekly review").unwrap();
        assert!(s.capture_to_task(&c1.id).is_some());
        let note = s.capture_to_note(&c2.id).unwrap();
        assert_eq!(note.title, "idea: weekly review");
        let list = s.captures();
        assert!(list.iter().all(|c| c.processed));
        assert!(s.delete_capture(&c1.id));
        assert_eq!(s.captures().len(), 1);
    }

    #[test]
    fn meetings_flow() {
        let s = service();
        let m = s.start_meeting("  ");
        assert_eq!(m.title, "Meeting");
        assert!(s.update_meeting(&m.id, "Standup", "- notes"));
        assert!(s.end_meeting(&m.id));
        assert!(s.meeting(&m.id).unwrap().ended_at.is_some());
        assert!(s.delete_meeting(&m.id));
        assert!(s.meetings().is_empty());
    }

    #[test]
    fn brag_dates_and_order() {
        let s = service();
        let today = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let a = s.add_brag_on("Shipped the beta", today).unwrap();
        assert_eq!(a.date, today);
        let b = s.add_brag_on("2026-01-05: Led the launch", today).unwrap();
        assert_eq!(b.date, NaiveDate::from_ymd_opt(2026, 1, 5).unwrap());
        assert_eq!(b.text, "Led the launch");
        assert!(s.add_brag_on("   ", today).is_none());
        let list = s.brags();
        assert_eq!(list[0].id, a.id);
        assert!(s.delete_brag(&a.id));
        assert_eq!(s.brags().len(), 1);
    }

    #[test]
    fn scope_from_section() {
        assert_eq!(NoteScope::from_section("trash"), Some(NoteScope::Trash));
        assert_eq!(NoteScope::from_section("brag-book"), None);
        assert_eq!(NoteScope::from_section("abc"), Some(NoteScope::Folder("abc".into())));
    }
}
