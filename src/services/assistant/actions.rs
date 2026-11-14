//! The small action format the model may answer with, and its strict parser.
//!
//! Model output is untrusted. Nothing in it changes any data on its own: it is
//! parsed into [`ProposedAction`]s, shown to the user, and only applied after a
//! click. Every field is checked here: unknown fields, wrong types, oversized
//! text and bad dates are rejected rather than repaired.
//!
//! Format (inside the reply, optionally in a code fence):
//!
//! ```json
//! {"actions": [
//!   {"type": "create_task", "title": "Write report", "section": "today", "estimate_mins": 30, "list": "Work"},
//!   {"type": "add_subtask", "task": "Write report", "title": "Outline"},
//!   {"type": "create_note", "title": "Ideas", "body": "..."},
//!   {"type": "schedule_task", "task": "Write report", "at": "2026-05-04T09:30"},
//!   {"type": "set_quadrant", "task": "a1b2c3", "quadrant": "do_asap"},
//!   {"type": "start_focus", "task": "Write report"},
//!   {"type": "update_task", "task": "a1b2c3", "title": "...", "estimate_mins": 45, "deadline": "2026-05-09"},
//!   {"type": "complete_task", "task": "a1b2c3", "done": true},
//!   {"type": "delete_task", "task": "a1b2c3"},
//!   {"type": "create_space", "name": "Exams", "color": "#8B5CF6"},
//!   {"type": "rename_space", "space": "Exams", "name": "Finals"},
//!   {"type": "delete_space", "space": "Finals"},
//!   {"type": "stop_focus"}
//! ]}
//!
//! `create_task` may also carry `quadrant`, `do_date` (YYYY-MM-DD) and
//! `steps`. A task or subtask is referred to by its short id or exact title.
//! Oversized estimates are capped and extra steps dropped, so one long plan
//! is trimmed rather than lost.
//! ```

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;

use chrono::NaiveDate;

use crate::models::{GoalPriority, MatrixQuadrant, TaskSection};

/// Most actions accepted from one reply.
pub const MAX_ACTIONS: usize = 25;
/// Most steps proposed under one new task.
pub const MAX_STEPS: usize = 20;
/// Longest task or note title, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// Longest note body, in characters.
pub const MAX_BODY_CHARS: usize = 4000;
/// Longest estimate, in minutes.
pub const MAX_ESTIMATE_MINS: u32 = 480;

/// A change the model proposes. Applied only after the user agrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposedAction {
    CreateTask {
        title: String,
        section: TaskSection,
        estimate_mins: u32,
        list: Option<String>,
        quadrant: Option<MatrixQuadrant>,
        do_date: Option<NaiveDate>,
        deadline: Option<NaiveDate>,
        steps: Vec<String>,
    },
    AddSubtask { task: String, title: String },
    CreateNote { title: String, body: String },
    ScheduleTask { task: String, at: DateTime<Utc> },
    SetQuadrant { task: String, quadrant: MatrixQuadrant },
    /// Starts a focus round on the task once the changes are applied.
    StartFocus { task: String },
    /// Stops the running timer and logs the time to its task.
    StopFocus,
    /// Records time already worked on a task (a timesheet entry).
    LogTime { task: String, minutes: u32, date: Option<NaiveDate> },
    UpdateTask {
        task: String,
        title: Option<String>,
        estimate_mins: Option<u32>,
        section: Option<TaskSection>,
        notes: Option<String>,
        do_date: Option<Option<NaiveDate>>,
        deadline: Option<Option<NaiveDate>>,
        list: Option<String>,
    },
    CompleteTask { task: String, done: bool },
    DeleteTask { task: String },
    CreateSpace { name: String, color: Option<String> },
    RenameSpace { space: String, name: String },
    DeleteSpace { space: String },
    /// A goal with its milestones and the tasks that lead to it.
    CreateGoal {
        title: String,
        description: String,
        motivation: String,
        category: String,
        priority: GoalPriority,
        target_date: Option<NaiveDate>,
        milestones: Vec<MilestoneDraft>,
        tasks: Vec<GoalTaskDraft>,
    },
    /// Links an existing task to an existing goal.
    LinkTask { goal: String, task: String, milestone: Option<String> },
}

/// A proposed milestone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MilestoneDraft {
    pub title: String,
    pub date: Option<NaiveDate>,
}

/// A proposed task for a new goal, optionally pinned to a milestone by title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalTaskDraft {
    pub title: String,
    pub estimate_mins: u32,
    pub milestone: Option<String>,
}

/// Most milestones and tasks one proposed goal can carry; extras are dropped.
pub const MAX_GOAL_MILESTONES: usize = 12;
pub const MAX_GOAL_TASKS: usize = 20;

/// Display name of a quadrant.
pub fn quadrant_title(q: MatrixQuadrant) -> &'static str {
    match q {
        MatrixQuadrant::DoAsap => "Do ASAP",
        MatrixQuadrant::DoLaterToday => "Do Later Today",
        MatrixQuadrant::DoMidWeek => "Do Mid Week",
        MatrixQuadrant::LeaveTillEnd => "Leave Till End",
    }
}

impl ProposedAction {
    /// One line describing the change, in plain words.
    pub fn describe(&self) -> String {
        match self {
            ProposedAction::CreateTask { title, section, estimate_mins, list, quadrant, do_date, deadline, steps } => {
                let mut s = format!("Add task \"{title}\" to {}", section.title());
                if *estimate_mins > 0 {
                    s.push_str(&format!(" ({} min)", estimate_mins));
                }
                if let Some(list) = list {
                    s.push_str(&format!(" in {list}"));
                }
                if let Some(q) = quadrant {
                    s.push_str(&format!(", {}", quadrant_title(*q)));
                }
                if let Some(d) = do_date {
                    s.push_str(&format!(", on {}", d.format("%a %-d %b")));
                }
                if let Some(d) = deadline {
                    s.push_str(&format!(", due {}", d.format("%a %-d %b")));
                }
                if !steps.is_empty() {
                    s.push_str(&format!(", {} {}", steps.len(), if steps.len() == 1 { "step" } else { "steps" }));
                }
                s
            }
            ProposedAction::AddSubtask { task, title } => format!("Add step \"{title}\" under \"{task}\""),
            ProposedAction::CreateNote { title, .. } => format!("Create note \"{title}\""),
            ProposedAction::ScheduleTask { task, at } => {
                format!("Schedule \"{task}\" for {}", at.with_timezone(&Local).format("%a %d %b, %H:%M"))
            }
            ProposedAction::SetQuadrant { task, quadrant } => format!("Sort \"{task}\" into {}", quadrant_title(*quadrant)),
            ProposedAction::StartFocus { task } => format!("Start a focus round on \"{task}\""),
            ProposedAction::StopFocus => "Stop the timer".to_string(),
            ProposedAction::LogTime { task, minutes, date } => match date {
                Some(d) => format!("Log {minutes} min on \"{task}\" ({})", d.format("%a %-d %b")),
                None => format!("Log {minutes} min on \"{task}\""),
            },
            ProposedAction::UpdateTask { task, title, .. } => match title {
                Some(t) => format!("Edit \"{task}\" (now \"{t}\")"),
                None => format!("Edit \"{task}\""),
            },
            ProposedAction::CompleteTask { task, done: true } => format!("Mark \"{task}\" done"),
            ProposedAction::CompleteTask { task, done: false } => format!("Reopen \"{task}\""),
            ProposedAction::DeleteTask { task } => format!("Delete \"{task}\""),
            ProposedAction::CreateSpace { name, .. } => format!("Create space \"{name}\""),
            ProposedAction::RenameSpace { space, name } => format!("Rename space \"{space}\" to \"{name}\""),
            ProposedAction::DeleteSpace { space } => format!("Delete space \"{space}\" and its tasks"),
            ProposedAction::CreateGoal { title, target_date, milestones, tasks, .. } => {
                let mut s = format!("Create goal \"{title}\"");
                if let Some(d) = target_date {
                    s.push_str(&format!(", by {}", d.format("%-d %b %Y")));
                }
                if !milestones.is_empty() {
                    s.push_str(&format!(", {} {}", milestones.len(), if milestones.len() == 1 { "milestone" } else { "milestones" }));
                }
                if !tasks.is_empty() {
                    s.push_str(&format!(", {} {}", tasks.len(), if tasks.len() == 1 { "task" } else { "tasks" }));
                }
                s
            }
            ProposedAction::LinkTask { goal, task, .. } => format!("Link \"{task}\" to goal \"{goal}\""),
        }
    }
}

/// Result of reading one model reply.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedReply {
    /// The reply with the action block removed; what the user reads.
    pub text: String,
    /// Valid proposals, in order.
    pub actions: Vec<ProposedAction>,
    /// Why each rejected proposal was refused (for diagnostics and tests).
    pub rejected: Vec<String>,
    /// The reply tried to change something (it held action objects, valid or not).
    /// With no valid action left, the screen says plainly that nothing changed.
    pub attempted: bool,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawAction {
    CreateTask {
        title: String,
        #[serde(default)]
        section: Option<String>,
        #[serde(default)]
        estimate_mins: Option<u32>,
        #[serde(default, alias = "space")]
        list: Option<String>,
        #[serde(default)]
        quadrant: Option<String>,
        #[serde(default)]
        do_date: Option<String>,
        #[serde(default, alias = "due", alias = "due_date")]
        deadline: Option<String>,
        #[serde(default)]
        steps: Option<Vec<String>>,
        // Small models add notes to new tasks; accepted and ignored.
        #[serde(default)]
        #[allow(dead_code)]
        notes: Option<String>,
    },
    AddSubtask {
        task: String,
        title: String,
    },
    CreateNote {
        title: String,
        #[serde(default)]
        body: Option<String>,
    },
    ScheduleTask {
        task: String,
        at: String,
    },
    SetQuadrant {
        task: String,
        quadrant: String,
    },
    StartFocus {
        #[serde(alias = "title")]
        task: String,
    },
    StopFocus {},
    LogTime {
        task: String,
        #[serde(alias = "duration_minutes", alias = "mins", alias = "duration_mins")]
        minutes: u32,
        #[serde(default)]
        date: Option<String>,
    },
    UpdateTask {
        task: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        estimate_mins: Option<u32>,
        #[serde(default)]
        section: Option<String>,
        #[serde(default)]
        notes: Option<String>,
        #[serde(default)]
        do_date: Option<String>,
        #[serde(default)]
        deadline: Option<String>,
        #[serde(default, alias = "list")]
        space: Option<String>,
    },
    CompleteTask {
        task: String,
        #[serde(default)]
        done: Option<bool>,
    },
    DeleteTask {
        task: String,
    },
    CreateSpace {
        name: String,
        #[serde(default)]
        color: Option<String>,
    },
    RenameSpace {
        space: String,
        name: String,
    },
    DeleteSpace {
        space: String,
    },
    CreateGoal {
        title: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        motivation: Option<String>,
        #[serde(default)]
        category: Option<String>,
        #[serde(default)]
        priority: Option<String>,
        #[serde(default, alias = "deadline")]
        target_date: Option<String>,
        #[serde(default)]
        milestones: Option<Vec<RawMilestone>>,
        #[serde(default)]
        tasks: Option<Vec<RawGoalTask>>,
    },
    LinkTask {
        goal: String,
        task: String,
        #[serde(default)]
        milestone: Option<String>,
    },
}

/// A milestone as the model writes it: a bare title or an object.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawMilestone {
    Title(String),
    Full {
        title: String,
        #[serde(default, alias = "target_date")]
        date: Option<String>,
    },
}

/// A task inside a proposed goal, or just its title.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawGoalTask {
    Title(String),
    Full {
        title: String,
        #[serde(default)]
        estimate_mins: Option<u32>,
        #[serde(default)]
        milestone: Option<String>,
    },
}

/// Reads a model reply: separates prose from a JSON action block and
/// validates each proposed action.
pub fn parse_reply(raw: &str) -> ParsedReply {
    let mut out = ParsedReply::default();
    // Small models sometimes write the JSON with every quote escaped (\"),
    // as if it sat inside a string. Undo that once, then read it as usual.
    let unescaped;
    let raw = if find_envelope(raw).is_none() && raw.contains("\\\":") {
        unescaped = raw.replace("\\\"", "\"").replace("\\n", "\n");
        unescaped.as_str()
    } else {
        raw
    };
    let repaired;
    let raw = if find_envelope(raw).is_none() {
        match repair_envelope(raw) {
            Some(r) if find_envelope(&r).is_some() => {
                repaired = r;
                repaired.as_str()
            }
            _ => raw,
        }
    } else {
        raw
    };
    // A block with actions wins; an empty one gives salvage a chance (the model
    // may have put one action in the root object) before it counts as empty.
    let read = find_envelope(raw);
    let join = |e: &Envelope| format!("{}\n{}", raw[..e.start].trim_end(), raw[e.end..].trim_start()).trim().to_string();
    let (found, text) = match read {
        Some(e) if !e.actions.is_empty() => {
            let t = join(&e);
            (e, t)
        }
        other => match salvage_envelope(raw).or(other) {
            Some(e) => {
                let t = join(&e);
                (e, t)
            }
            None => {
                // No block at all: the model may have written each action as a loose
                // object, one per line. Read those; anything else that looks like
                // JSON is kept out of what the user reads.
                let (actions, rest) = loose_actions(raw);
                if actions.is_empty() {
                    let (text, hid) = hide_json(raw.trim());
                    out.text = text;
                    out.attempted = hid;
                    return out;
                }
                (Envelope { start: 0, end: 0, actions }, rest)
            }
        },
    };

    out.attempted = !found.actions.is_empty();
    out.text = hide_json(&strip_empty_fences(&text)).0;

    for (i, value) in found.actions.into_iter().enumerate() {
        if i >= MAX_ACTIONS {
            out.rejected.push(format!("action {}: too many actions", i + 1));
            continue;
        }
        match validate(value) {
            Ok(action) => {
                if !out.actions.contains(&action) {
                    out.actions.push(action);
                }
            }
            Err(why) => out.rejected.push(format!("action {}: {why}", i + 1)),
        }
    }
    drop_stray_deletes(&mut out);
    out
}

/// Action objects written loose in the text (no `{"actions":[...]}` around them),
/// and the text with them taken out. Only objects with a `type` count.
fn loose_actions(raw: &str) -> (Vec<Value>, String) {
    let mut actions = Vec::new();
    let mut rest = String::with_capacity(raw.len());
    let mut i = 0;
    while let Some(rel) = raw[i..].find('{') {
        let s = i + rel;
        match balanced_end(raw, s) {
            Some(e) => match serde_json::from_str::<Value>(&raw[s..e]) {
                Ok(v) if v.get("type").and_then(Value::as_str).is_some() => {
                    rest.push_str(&raw[i..s]);
                    actions.push(v);
                    i = e;
                }
                _ => {
                    rest.push_str(&raw[i..s + 1]);
                    i = s + 1;
                }
            },
            None => break,
        }
    }
    rest.push_str(&raw[i..]);
    // Commas and brackets left between the objects.
    let rest = rest.lines().filter(|l| !l.trim().trim_matches(|c| c == ',' || c == '[' || c == ']').is_empty() || l.trim().is_empty()).collect::<Vec<_>>().join("\n");
    (actions, rest.trim().to_string())
}

/// Removes JSON the user should never read: fenced code blocks and lines that
/// are only JSON (they start with a brace or bracket, or an "actions" key).
/// Returns the text and whether anything was removed.
fn hide_json(text: &str) -> (String, bool) {
    let mut out: Vec<&str> = Vec::new();
    let mut removed = false;
    let mut in_fence = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_fence = !in_fence;
            removed = true;
            continue;
        }
        // "[1] ..." is a source mark, not JSON: a bracket counts only before {, " or ].
        let json_list = t.starts_with('[') && t[1..].trim_start().starts_with(['{', '"', ']']) || t == "[";
        if in_fence || t.starts_with('{') || t.starts_with('}') || json_list || t.starts_with(']') || t.starts_with("\"actions\"") {
            removed = removed || !t.is_empty();
            continue;
        }
        out.push(line);
    }
    // Collapse the blank lines the removal leaves behind.
    let mut text = out.join("\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    (text.trim().to_string(), removed)
}

/// A reply that mixes deletions with other changes is a planning or
/// rescheduling answer, which never needs to delete anything. Drop the
/// deletions and say so. A reply made only of deletions is kept: that is
/// what a plain "delete this" request produces.
fn drop_stray_deletes(out: &mut ParsedReply) {
    let is_delete = |a: &ProposedAction| matches!(a, ProposedAction::DeleteTask { .. } | ProposedAction::DeleteSpace { .. });
    if out.actions.iter().all(is_delete) {
        return;
    }
    let before = out.actions.len();
    out.actions.retain(|a| !is_delete(a));
    for _ in out.actions.len()..before {
        out.rejected.push("a plan may not delete anything".to_string());
    }
}

/// Last-resort repair of an action block that small models often get nearly
/// right: a missing comma between two objects, or missing closing brackets.
/// Only used when the reply has no readable block as written.
fn repair_envelope(raw: &str) -> Option<String> {
    let start = raw.find("{\"actions\"")?;
    let end = raw.rfind(['}', ']'])? + 1;
    if end <= start {
        return None;
    }
    let body = &raw[start..end];
    // Two objects side by side inside the array need a comma.
    let mut fixed = String::with_capacity(body.len() + 8);
    let chars: Vec<char> = body.chars().collect();
    let (mut in_str, mut escaped) = (false, false);
    let mut stack: Vec<char> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        fixed.push(c);
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '{' | '[' => stack.push(c),
                '}' | ']' => {
                    stack.pop();
                    // `}` then whitespace then `{` while an array is open.
                    if c == '}' && stack.last() == Some(&'[') {
                        let mut j = i + 1;
                        while j < chars.len() && chars[j].is_whitespace() {
                            j += 1;
                        }
                        if j < chars.len() && chars[j] == '{' {
                            fixed.push(',');
                        }
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    if in_str {
        fixed.push('"');
    }
    while let Some(open) = stack.pop() {
        fixed.push(if open == '{' { '}' } else { ']' });
    }
    Some(fixed)
}

/// Last resort: picks every complete, valid action object out of a block
/// that is broken elsewhere (stray text after it, a cut-off tail, extra keys
/// beyond the closing bracket).
fn salvage_envelope(raw: &str) -> Option<Envelope> {
    let key = raw.find("\"actions\"")?;
    let start = raw[..key].rfind('{')?;
    let open = key + raw[key..].find('[')? + 1;
    let mut actions = Vec::new();
    let (mut i, mut end) = (open, open);
    // The model sometimes writes one action straight into the root object,
    // next to an empty "actions" list.
    if let Some(e) = balanced_end(raw, start) {
        if let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(&raw[start..e]) {
            if map.contains_key("type") {
                map.remove("actions");
                actions.push(Value::Object(map));
                end = e;
                i = e;
            }
        }
    }
    while let Some(rel) = raw[i..].find('{') {
        let s = i + rel;
        match balanced_end(raw, s) {
            Some(e) => match serde_json::from_str::<Value>(&raw[s..e]) {
                Ok(v) if v.get("type").is_some() => {
                    actions.push(v);
                    i = e;
                    end = e;
                }
                _ => i = s + 1,
            },
            None => i = s + 1,
        }
    }
    (!actions.is_empty()).then_some(Envelope { start, end, actions })
}

struct Envelope {
    start: usize,
    end: usize,
    actions: Vec<Value>,
}

/// Finds the first JSON object in `raw` that has an `actions` array, along
/// with the span to cut from the visible text (including a code fence around it).
fn find_envelope(raw: &str) -> Option<Envelope> {
    let mut from = 0;
    while let Some(rel) = raw[from..].find('{') {
        let start = from + rel;
        match balanced_end(raw, start) {
            Some(end) => {
                if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&raw[start..end]) {
                    if let Some(Value::Array(actions)) = map.get("actions") {
                        let actions = actions.clone();
                        // Include surrounding code fence markers in the cut.
                        let mut s = start;
                        let mut e = end;
                        let before = raw[..s].trim_end();
                        if let Some(fence_start) = before.rfind("```") {
                            let between = &raw[fence_start + 3..s];
                            if between.trim().chars().all(|c| c.is_ascii_alphanumeric()) {
                                s = fence_start;
                            }
                        }
                        let after = raw[e..].trim_start();
                        if after.starts_with("```") {
                            e = raw.len() - after.len() + 3;
                        }
                        return Some(Envelope { start: s, end: e, actions });
                    }
                }
                from = start + 1;
            }
            None => from = start + 1,
        }
    }
    None
}

/// Index just past the `}` matching the `{` at `start`, honouring strings.
fn balanced_end(s: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in s[start..].char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + i + c.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_empty_fences(text: &str) -> String {
    text.replace("```json\n```", "").replace("```\n```", "").trim().to_string()
}

fn validate(mut value: Value) -> Result<ProposedAction, String> {
    // A task written as a wrapper object ({"type":"create_task","task":{...}}) is
    // read as if its fields sat beside the type.
    if let Some(map) = value.as_object_mut() {
        if matches!(map.get("task"), Some(Value::Object(_))) {
            if let Some(Value::Object(inner)) = map.remove("task") {
                for (k, v) in inner {
                    map.entry(k).or_insert(v);
                }
            }
        }
    }
    // A new goal often arrives with stray task-style keys (do_asap, space...).
    // They mean nothing here, so drop them instead of refusing the goal.
    if value.get("type").and_then(Value::as_str) == Some("create_goal") {
        const KEEP: [&str; 10] =
            ["type", "title", "description", "motivation", "category", "priority", "target_date", "deadline", "milestones", "tasks"];
        if let Some(map) = value.as_object_mut() {
            map.retain(|k, _| KEEP.contains(&k.as_str()));
        }
    }
    let raw: RawAction = serde_json::from_value(value).map_err(|e| e.to_string())?;
    match raw {
        RawAction::CreateTask { title, section, estimate_mins, list, quadrant, do_date, deadline, steps, .. } => {
            let title = clean_line(&title, MAX_TITLE_CHARS, "title")?;
            let section = match section.as_deref().map(str::trim) {
                None | Some("") => TaskSection::Today,
                Some(s) => TaskSection::parse(s).ok_or_else(|| format!("unknown section \"{}\"", truncate(s, 30)))?,
            };
            let estimate_mins = estimate_mins.unwrap_or(0).min(MAX_ESTIMATE_MINS);
            let list = match list {
                Some(l) if !l.trim().is_empty() => Some(clean_line(&l, 80, "list")?),
                _ => None,
            };
            let quadrant = match quadrant.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(q) => Some(parse_quadrant(q)?),
            };
            let do_date = match do_date.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(d) => Some(NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| "do_date is not YYYY-MM-DD".to_string())?),
            };
            let deadline = match deadline.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(d) => Some(NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| "deadline is not YYYY-MM-DD".to_string())?),
            };
            let steps: Vec<String> =
                steps.unwrap_or_default().iter().filter_map(|st| clean_line(st, MAX_TITLE_CHARS, "step").ok()).take(MAX_STEPS).collect();
            // A quadrant decides the section, so the two always agree.
            let section = quadrant.map(|q| q.section()).unwrap_or(section);
            Ok(ProposedAction::CreateTask { title, section, estimate_mins, list, quadrant, do_date, deadline, steps })
        }
        RawAction::AddSubtask { task, title } => Ok(ProposedAction::AddSubtask {
            task: clean_line(&task, MAX_TITLE_CHARS, "task")?,
            title: clean_line(&title, MAX_TITLE_CHARS, "title")?,
        }),
        RawAction::CreateNote { title, body } => {
            let title = clean_line(&title, MAX_TITLE_CHARS, "title")?;
            let body = clean_text(body.as_deref().unwrap_or(""), MAX_BODY_CHARS, "body")?;
            Ok(ProposedAction::CreateNote { title, body })
        }
        RawAction::ScheduleTask { task, at } => Ok(ProposedAction::ScheduleTask {
            task: clean_line(&task, MAX_TITLE_CHARS, "task")?,
            at: parse_time(&at)?,
        }),
        RawAction::SetQuadrant { task, quadrant } => Ok(ProposedAction::SetQuadrant {
            task: clean_line(&task, MAX_TITLE_CHARS, "task")?,
            quadrant: parse_quadrant(quadrant.trim())?,
        }),
        RawAction::StartFocus { task } => Ok(ProposedAction::StartFocus { task: clean_line(&task, MAX_TITLE_CHARS, "task")? }),
        RawAction::StopFocus {} => Ok(ProposedAction::StopFocus),
        RawAction::LogTime { task, minutes, date } => {
            if minutes == 0 {
                return Err("minutes must be above zero".to_string());
            }
            let date = match date.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(d) => Some(NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| "date is not YYYY-MM-DD".to_string())?),
            };
            Ok(ProposedAction::LogTime { task: clean_line(&task, MAX_TITLE_CHARS, "task")?, minutes: minutes.min(24 * 60), date })
        }
        RawAction::UpdateTask { task, title, estimate_mins, section, notes, do_date, deadline, space } => {
            let opt_line = |v: Option<String>, field: &str| -> Result<Option<String>, String> {
                match v {
                    Some(t) if !t.trim().is_empty() => clean_line(&t, MAX_TITLE_CHARS, field).map(Some),
                    _ => Ok(None),
                }
            };
            let section = match section.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(s) => Some(TaskSection::parse(s).ok_or_else(|| format!("unknown section \"{}\"", truncate(s, 30)))?),
            };
            Ok(ProposedAction::UpdateTask {
                task: clean_line(&task, MAX_TITLE_CHARS, "task")?,
                title: opt_line(title, "title")?,
                estimate_mins: estimate_mins.map(|m| m.min(MAX_ESTIMATE_MINS)),
                section,
                notes: notes.map(|n| clean_text(&n, MAX_BODY_CHARS, "notes")).transpose()?,
                do_date: do_date.map(|d| opt_date(&d, "do_date")).transpose()?,
                deadline: deadline.map(|d| opt_date(&d, "deadline")).transpose()?,
                list: opt_line(space, "space")?,
            })
        }
        RawAction::CompleteTask { task, done } => {
            Ok(ProposedAction::CompleteTask { task: clean_line(&task, MAX_TITLE_CHARS, "task")?, done: done.unwrap_or(true) })
        }
        RawAction::DeleteTask { task } => Ok(ProposedAction::DeleteTask { task: clean_line(&task, MAX_TITLE_CHARS, "task")? }),
        RawAction::CreateSpace { name, color } => Ok(ProposedAction::CreateSpace {
            name: clean_line(&name, 80, "name")?,
            color: color.filter(|c| is_hex_color(c.trim())).map(|c| c.trim().to_string()),
        }),
        RawAction::RenameSpace { space, name } => Ok(ProposedAction::RenameSpace {
            space: clean_line(&space, 80, "space")?,
            name: clean_line(&name, 80, "name")?,
        }),
        RawAction::DeleteSpace { space } => Ok(ProposedAction::DeleteSpace { space: clean_line(&space, 80, "space")? }),
        RawAction::CreateGoal { title, description, motivation, category, priority, target_date, milestones, tasks } => {
            let priority = match priority.as_deref().map(str::trim) {
                None | Some("") => GoalPriority::Medium,
                Some(p) => GoalPriority::parse(&p.to_lowercase()).ok_or_else(|| format!("unknown priority \"{}\"", truncate(p, 20)))?,
            };
            // A bad date on a milestone drops the date, not the whole goal.
            let milestones: Vec<MilestoneDraft> = milestones
                .unwrap_or_default()
                .into_iter()
                .filter_map(|m| {
                    let (t, d) = match m {
                        RawMilestone::Title(t) => (t, None),
                        RawMilestone::Full { title, date } => (title, date),
                    };
                    let title = clean_line(&t, MAX_TITLE_CHARS, "milestone").ok()?;
                    let date = d.and_then(|d| opt_date(&d, "date").ok().flatten());
                    Some(MilestoneDraft { title, date })
                })
                .take(MAX_GOAL_MILESTONES)
                .collect();
            let tasks: Vec<GoalTaskDraft> = tasks
                .unwrap_or_default()
                .into_iter()
                .filter_map(|t| {
                    let (title, est, ms) = match t {
                        RawGoalTask::Title(t) => (t, None, None),
                        RawGoalTask::Full { title, estimate_mins, milestone } => (title, estimate_mins, milestone),
                    };
                    let title = clean_line(&title, MAX_TITLE_CHARS, "task").ok()?;
                    let milestone = ms.and_then(|m| clean_line(&m, MAX_TITLE_CHARS, "milestone").ok());
                    Some(GoalTaskDraft { title, estimate_mins: est.unwrap_or(0).min(MAX_ESTIMATE_MINS), milestone })
                })
                .take(MAX_GOAL_TASKS)
                .collect();
            Ok(ProposedAction::CreateGoal {
                title: clean_line(&title, MAX_TITLE_CHARS, "title")?,
                description: clean_text(&description.unwrap_or_default(), MAX_BODY_CHARS, "description")?,
                motivation: clean_text(&motivation.unwrap_or_default(), MAX_BODY_CHARS, "motivation")?,
                category: clean_line(&category.unwrap_or_default(), 40, "category").unwrap_or_default(),
                priority,
                target_date: match target_date {
                    Some(d) => opt_date(&d, "target_date")?,
                    None => None,
                },
                milestones,
                tasks,
            })
        }
        RawAction::LinkTask { goal, task, milestone } => Ok(ProposedAction::LinkTask {
            goal: clean_line(&goal, MAX_TITLE_CHARS, "goal")?,
            task: clean_line(&task, MAX_TITLE_CHARS, "task")?,
            milestone: milestone.and_then(|m| clean_line(&m, MAX_TITLE_CHARS, "milestone").ok()),
        }),
    }
}

/// A date, or "" / "none" to clear it.
fn opt_date(d: &str, field: &str) -> Result<Option<NaiveDate>, String> {
    match d.trim() {
        "" | "none" | "null" => Ok(None),
        d => NaiveDate::parse_from_str(d, "%Y-%m-%d").map(Some).map_err(|_| format!("{field} is not YYYY-MM-DD")),
    }
}

fn is_hex_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit())
}

fn parse_quadrant(q: &str) -> Result<MatrixQuadrant, String> {
    MatrixQuadrant::parse(q).ok_or_else(|| format!("unknown quadrant \"{}\"", truncate(q, 30)))
}

/// Single-line text: control characters are refused, ends trimmed.
fn clean_line(s: &str, max: usize, field: &str) -> Result<String, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err(format!("{field} is empty"));
    }
    if t.chars().count() > max {
        return Err(format!("{field} is too long"));
    }
    if t.chars().any(char::is_control) {
        return Err(format!("{field} has control characters"));
    }
    Ok(t.to_string())
}

/// Multi-line text: newlines and tabs allowed, other control characters refused.
fn clean_text(s: &str, max: usize, field: &str) -> Result<String, String> {
    let t = s.trim();
    if t.chars().count() > max {
        return Err(format!("{field} is too long"));
    }
    if t.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(format!("{field} has control characters"));
    }
    Ok(t.replace('\r', ""))
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Accepts an RFC 3339 time, or a local `YYYY-MM-DDTHH:MM[:SS]` / `YYYY-MM-DD HH:MM`.
fn parse_time(s: &str) -> Result<DateTime<Utc>, String> {
    let s = s.trim();
    let parsed = if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        dt.with_timezone(&Utc)
    } else {
        let naive = ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
            .iter()
            .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
            .ok_or_else(|| "time is not in the expected format".to_string())?;
        Local
            .from_local_datetime(&naive)
            .earliest()
            .ok_or_else(|| "time does not exist".to_string())?
            .with_timezone(&Utc)
    };
    let year = parsed.with_timezone(&Local).format("%Y").to_string().parse::<i32>().unwrap_or(0);
    if !(2000..=2100).contains(&year) {
        return Err("time is out of range".into());
    }
    Ok(parsed)
}

#[cfg(test)]
mod loose_reply_tests {
    use super::*;

    #[test]
    fn loose_action_lines_become_proposals_and_leave_the_text() {
        let raw = "Got it. I'll delete all your tasks.\n\n{\"type\":\"delete_task\",\"task\":\"Sample task 2\",\"done\":true}\n{\"type\":\"delete_task\",\"task\":\"Sample task 3\",\"done\":true}";
        let p = parse_reply(raw);
        assert_eq!(p.actions.len(), 2, "{:?}", p.rejected);
        assert!(p.attempted);
        assert_eq!(p.text, "Got it. I'll delete all your tasks.");
    }

    #[test]
    fn broken_json_is_hidden_and_counts_as_an_attempt() {
        let p = parse_reply("Sure.\n{\"type\": \"delete_task\", \"task\": ");
        assert!(p.actions.is_empty());
        assert!(p.attempted);
        assert_eq!(p.text, "Sure.");
    }

    #[test]
    fn source_marks_are_kept() {
        let p = parse_reply("The canal opened in 1914 [1].
[2] Ships no longer round Cape Horn.");
        assert!(!p.attempted);
        assert!(p.text.contains("[2] Ships"));
    }

    #[test]
    fn a_plain_answer_is_not_an_attempt() {
        let p = parse_reply("Paris is the capital of France.");
        assert!(!p.attempted);
        assert_eq!(p.text, "Paris is the capital of France.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(raw: &str) -> ParsedReply {
        parse_reply(raw)
    }

    #[test]
    fn goal_actions_parse_leniently_and_describe_themselves() {
        let p = actions(
            r#"{"actions":[{"type":"create_goal","title":"Learn Rust","priority":"HIGH","deadline":"2027-01-31",
            "milestones":["Basics",{"title":"Project","date":"oops"}],"tasks":["Read the book",{"title":"Build a CLI","estimate_mins":9999}]},
            {"type":"link_task","goal":"Learn Rust","task":"abc123","milestone":"Basics"},
            {"type":"create_goal","title":"","milestones":[]},
            {"type":"create_goal","title":"X","priority":"urgent"}]}"#,
        );
        assert_eq!(p.actions.len(), 2, "{:?}", p.rejected);
        assert_eq!(p.rejected.len(), 2);
        match &p.actions[0] {
            ProposedAction::CreateGoal { priority, target_date, milestones, tasks, .. } => {
                assert_eq!(*priority, GoalPriority::High);
                assert_eq!(*target_date, NaiveDate::from_ymd_opt(2027, 1, 31));
                assert_eq!(milestones.len(), 2);
                assert!(milestones[1].date.is_none());
                assert_eq!(tasks[1].estimate_mins, MAX_ESTIMATE_MINS);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(p.actions[0].describe(), "Create goal \"Learn Rust\", by 31 Jan 2027, 2 milestones, 2 tasks");
        assert_eq!(p.actions[1].describe(), "Link \"abc123\" to goal \"Learn Rust\"");
    }

    #[test]
    fn goal_milestones_and_tasks_are_capped_not_rejected() {
        let many: Vec<String> = (0..40).map(|i| format!("\"m{i}\"")).collect();
        let raw = format!("{{\"actions\":[{{\"type\":\"create_goal\",\"title\":\"Big\",\"milestones\":[{}],\"tasks\":[{}]}}]}}", many.join(","), many.join(","));
        match &actions(&raw).actions[0] {
            ProposedAction::CreateGoal { milestones, tasks, .. } => {
                assert_eq!(milestones.len(), MAX_GOAL_MILESTONES);
                assert_eq!(tasks.len(), MAX_GOAL_TASKS);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn json_with_escaped_quotes_is_still_read() {
        let raw = r#"{"actions":[{\"type\":\"create_task\",\"title\":\"Write intro\",\"estimate_mins\":30}]}"#;
        let p = actions(raw);
        assert_eq!(p.actions.len(), 1, "{:?}", p.rejected);
        assert_eq!(p.actions[0].describe(), "Add task \"Write intro\" to Today (30 min)");
        // Ordinary replies are untouched.
        assert!(actions("No quotes \\\" here").actions.is_empty());
    }

    #[test]
    fn nearly_valid_blocks_are_repaired() {
        // Missing comma between two actions and missing closing "]}".
        let raw = "{\"actions\":[{\"type\":\"create_task\",\"title\":\"A\"}\n{\"type\":\"create_task\",\"title\":\"B\"}";
        let p = actions(raw);
        assert_eq!(p.actions.len(), 2, "{:?}", p.rejected);
        // Goal closed but the array and envelope left open.
        let raw = "Here you go: {\"actions\":[{\"type\":\"create_goal\",\"title\":\"G\",\"milestones\":[\"One\"],\"tasks\":[{\"title\":\"T\"}]}";
        let p = actions(raw);
        assert_eq!(p.actions.len(), 1, "{:?}", p.rejected);
        // A truncated string is closed rather than lost.
        let p = actions("{\"actions\":[{\"type\":\"create_task\",\"title\":\"Half");
        assert!(p.actions.is_empty() || p.actions.len() == 1);
        // Prose with braces is left alone.
        assert!(actions("Use {braces} like this").actions.is_empty());
    }

    #[test]
    fn complete_actions_are_salvaged_from_a_broken_block() {
        let raw = r#"{"actions":[{"type":"create_task","title":"Keep me"},{"type":"create_task","title":"And me"}],"space":"x","do_asap":true}"} trailing"#;
        let p = actions(raw);
        assert_eq!(p.actions.len(), 2, "{:?}", p.rejected);
        // The real failure: the goal object is whole, junk follows it.
        let raw = r#"{"actions":[{"type":"create_goal","title":"G","milestones":["A"],"tasks":[{"title":"T","milestone":"A"}]}","space":"VLSI Lab","start_focus":"G"}"#;
        let p = actions(raw);
        assert_eq!(p.actions.len(), 1, "{:?}", p.rejected);
        assert!(matches!(p.actions[0], ProposedAction::CreateGoal { .. }));
    }

    #[test]
    fn partly_escaped_and_root_level_actions_are_read() {
        // Run-to-run quirks of a small model: a stray \" in the middle ...
        let raw = r#"{"actions":[{"type":"create_goal","title":"G","milestones":[{"title":"A"},{"title\":\"B"}],"tasks":[]}]}"#;
        assert_eq!(actions(raw).actions.len(), 1);
        // ... and one action written beside an empty actions list.
        let raw = r#"{"actions":[],"type":"create_goal","title":"G2","milestones":["A"],"tasks":[{"title":"T"}]}"#;
        let p = actions(raw);
        assert_eq!(p.actions.len(), 1, "{:?}", p.rejected);
        assert!(matches!(&p.actions[0], ProposedAction::CreateGoal { title, .. } if title == "G2"));
    }

    #[test]
    fn stray_keys_on_a_new_goal_are_dropped_not_fatal() {
        let p = actions(r#"{"actions":[{"type":"create_goal","title":"G","do_asap":true,"space":"Lab","milestones":["A"]}]}"#);
        assert_eq!(p.actions.len(), 1, "{:?}", p.rejected);
        // Other actions forgive stray keys too.
        let p = actions(r#"{"actions":[{"type":"delete_task","task":"x","extra":1}]}"#);
        assert_eq!(p.actions.len(), 1);
    }

    #[test]
    fn plain_reply_has_no_actions() {
        let p = actions("Just a friendly answer.");
        assert_eq!(p.text, "Just a friendly answer.");
        assert!(p.actions.is_empty());
        assert!(p.rejected.is_empty());
    }

    #[test]
    fn parses_all_four_kinds_from_a_fence() {
        let raw = "Here you go.\n```json\n{\"actions\":[\
            {\"type\":\"create_task\",\"title\":\"Write report\",\"section\":\"this_week\",\"estimate_mins\":30,\"list\":\"Work\"},\
            {\"type\":\"add_subtask\",\"task\":\"Write report\",\"title\":\"Outline\"},\
            {\"type\":\"create_note\",\"title\":\"Ideas\",\"body\":\"line one\\nline two\"},\
            {\"type\":\"schedule_task\",\"task\":\"Write report\",\"at\":\"2026-05-04T09:30:00Z\"}]}\n```\nAnything else?";
        let p = actions(raw);
        assert_eq!(p.rejected, Vec::<String>::new());
        assert_eq!(p.actions.len(), 4);
        assert_eq!(
            p.actions[0],
            ProposedAction::CreateTask {
                title: "Write report".into(),
                section: TaskSection::ThisWeek,
                estimate_mins: 30,
                list: Some("Work".into()),
                quadrant: None,
                do_date: None,
                deadline: None,
                steps: Vec::new(),
            }
        );
        assert!(matches!(p.actions[3], ProposedAction::ScheduleTask { .. }));
        assert!(p.text.starts_with("Here you go."));
        assert!(p.text.contains("Anything else?"));
        assert!(!p.text.contains("actions"));
        assert!(!p.text.contains("```"));
    }

    #[test]
    fn parses_bare_json_without_a_fence() {
        let p = actions("Done. {\"actions\":[{\"type\":\"create_task\",\"title\":\"Call Sam\"}]}");
        assert_eq!(p.actions.len(), 1);
        assert_eq!(p.text, "Done.");
        match &p.actions[0] {
            ProposedAction::CreateTask { section, estimate_mins, list, .. } => {
                assert_eq!(*section, TaskSection::Today);
                assert_eq!(*estimate_mins, 0);
                assert!(list.is_none());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn braces_inside_strings_do_not_break_scanning() {
        let p = actions("{\"actions\":[{\"type\":\"create_note\",\"title\":\"a } b\",\"body\":\"{x}\"}]}");
        assert_eq!(p.actions.len(), 1);
    }

    #[test]
    fn unknown_fields_and_types_are_rejected() {
        let p = actions(
            "{\"actions\":[\
            {\"type\":\"create_task\",\"title\":\"ok\",\"delete_everything\":true},\
            {\"type\":\"wipe_disk\",\"task\":\"x\"},\
            {\"type\":\"create_task\"},\
            {\"type\":\"create_task\",\"title\":\"fine\"}]}",
        );
        // The stray key is ignored; the unknown type and the missing title are refused.
        assert_eq!(p.actions.len(), 2);
        assert_eq!(p.rejected.len(), 2);
    }

    #[test]
    fn bad_values_are_rejected_not_repaired() {
        let p = actions(
            "{\"actions\":[\
            {\"type\":\"create_task\",\"title\":\"   \"},\
            {\"type\":\"create_task\",\"title\":\"x\",\"section\":\"someday\"},\
            {\"type\":\"create_task\",\"title\":\"x\",\"estimate_mins\":-5},\
            {\"type\":\"create_task\",\"title\":\"bell\\u0007\"},\
            {\"type\":\"schedule_task\",\"task\":\"x\",\"at\":\"next tuesday\"},\
            {\"type\":\"schedule_task\",\"task\":\"x\",\"at\":\"1850-01-01T00:00:00Z\"}]}",
        );
        assert!(p.actions.is_empty(), "{:?}", p.actions);
        assert_eq!(p.rejected.len(), 6);
    }

    #[test]
    fn long_plans_are_trimmed_not_dropped() {
        let steps: Vec<String> = (1..=30).map(|i| format!("\"Module {i}\"")).collect();
        let raw = format!(
            "{{\"actions\":[{{\"type\":\"create_task\",\"title\":\"Study\",\"estimate_mins\":720,\"steps\":[{}]}}]}}",
            steps.join(",")
        );
        match &actions(&raw).actions[..] {
            [ProposedAction::CreateTask { estimate_mins, steps, .. }] => {
                assert_eq!(*estimate_mins, MAX_ESTIMATE_MINS);
                assert_eq!(steps.len(), MAX_STEPS);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn management_actions_parse() {
        let p = actions(
            "{\"actions\":[\
            {\"type\":\"update_task\",\"task\":\"abc123\",\"title\":\"New\",\"deadline\":\"2026-05-09\"},\
            {\"type\":\"complete_task\",\"task\":\"abc123\"},\
            {\"type\":\"delete_task\",\"task\":\"abc123\"},\
            {\"type\":\"create_space\",\"name\":\"Exams\",\"color\":\"#22C55E\"},\
            {\"type\":\"rename_space\",\"space\":\"Exams\",\"name\":\"Finals\"},\
            {\"type\":\"delete_space\",\"space\":\"Finals\"},\
            {\"type\":\"stop_focus\"}]}",
        );
        // The two deletions are dropped because the reply also changes other things.
        assert_eq!(p.actions.len(), 5, "{:?}", p.rejected);
        assert_eq!(p.actions[4], ProposedAction::StopFocus);
    }

    #[test]
    fn oversized_text_is_rejected() {
        let long = "a".repeat(MAX_TITLE_CHARS + 1);
        let raw = format!("{{\"actions\":[{{\"type\":\"create_task\",\"title\":\"{long}\"}}]}}");
        assert!(actions(&raw).actions.is_empty());
        let body = "b".repeat(MAX_BODY_CHARS + 1);
        let raw = format!("{{\"actions\":[{{\"type\":\"create_note\",\"title\":\"t\",\"body\":\"{body}\"}}]}}");
        assert!(actions(&raw).actions.is_empty());
    }

    #[test]
    fn action_count_is_capped_and_duplicates_collapse() {
        let one = "{\"type\":\"create_task\",\"title\":\"T\"}";
        let many: Vec<String> = (0..MAX_ACTIONS + 5).map(|i| format!("{{\"type\":\"create_task\",\"title\":\"T{i}\"}}")).collect();
        let p = actions(&format!("{{\"actions\":[{}]}}", many.join(",")));
        assert_eq!(p.actions.len(), MAX_ACTIONS);
        assert_eq!(p.rejected.len(), 5);
        let p = actions(&format!("{{\"actions\":[{one},{one}]}}"));
        assert_eq!(p.actions.len(), 1);
    }

    #[test]
    fn json_without_an_actions_array_stays_in_the_text() {
        let raw = "Config: {\"foo\": 1}";
        let p = actions(raw);
        assert_eq!(p.text, raw);
        assert!(p.actions.is_empty());
        let p = actions("{\"actions\":\"nope\"}");
        assert!(p.actions.is_empty());
    }

    #[test]
    fn broken_json_is_left_alone() {
        let raw = "Sure {\"actions\":[{\"type\":\"create_task\",\"title\":\"x\"";
        let p = actions(raw);
        assert!(p.actions.is_empty());
        assert_eq!(p.text, raw);
    }

    #[test]
    fn planning_fields_and_new_actions_parse() {
        let p = actions(
            "{\"actions\":[\
            {\"type\":\"create_task\",\"title\":\"Report\",\"space\":\"Work\",\"quadrant\":\"do_mid_week\",\"do_date\":\"2026-05-04\",\"steps\":[\"Outline\",\"Draft\"]},\
            {\"type\":\"set_quadrant\",\"task\":\"ab12cd\",\"quadrant\":\"do_asap\"},\
            {\"type\":\"start_focus\",\"task\":\"Report\"},\
            {\"type\":\"set_quadrant\",\"task\":\"x\",\"quadrant\":\"soon\"}]}",
        );
        assert_eq!(p.rejected.len(), 1);
        match &p.actions[0] {
            ProposedAction::CreateTask { section, list, quadrant, steps, do_date, .. } => {
                assert_eq!(*section, TaskSection::ThisWeek);
                assert_eq!(list.as_deref(), Some("Work"));
                assert_eq!(*quadrant, Some(MatrixQuadrant::DoMidWeek));
                assert_eq!(steps.len(), 2);
                assert!(do_date.is_some());
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(p.actions[2], ProposedAction::StartFocus { .. }));
    }

    #[test]
    fn local_time_without_zone_is_accepted() {
        assert!(parse_time("2026-05-04T09:30").is_ok());
        assert!(parse_time("2026-05-04 09:30").is_ok());
        assert!(parse_time("04/05/2026").is_err());
    }

    #[test]
    fn descriptions_are_plain_language() {
        let a = ProposedAction::CreateTask {
            title: "Write".into(),
            section: TaskSection::Today,
            estimate_mins: 25,
            list: None,
            quadrant: None,
            do_date: None,
            deadline: None,
            steps: Vec::new(),
        };
        assert_eq!(a.describe(), "Add task \"Write\" to Today (25 min)");
    }
    #[test]
    fn deadline_and_log_time_are_read() {
        let p = parse_reply(r#"{"actions":[{"type":"create_task","title":"Essay","deadline":"2026-12-05"},{"type":"log_time","task":"Essay","minutes":45}]}"#);
        assert_eq!(p.rejected, Vec::<String>::new());
        assert!(matches!(&p.actions[0], ProposedAction::CreateTask { deadline: Some(_), .. }));
        assert_eq!(p.actions[1], ProposedAction::LogTime { task: "Essay".into(), minutes: 45, date: None });
        assert!(parse_reply(r#"{"actions":[{"type":"log_time","task":"x","minutes":0}]}"#).actions.is_empty());
    }

    #[test]
    fn common_slips_are_forgiven() {
        // A wrapper object, a title on start_focus and an extra key.
        let p = parse_reply(r#"{"actions":[{"type":"create_task","task":{"title":"Read","estimate_mins":20}},{"type":"start_focus","title":"Read","duration_minutes":25}]}"#);
        assert_eq!(p.rejected, Vec::<String>::new());
        assert_eq!(p.actions.len(), 2);
        assert_eq!(p.actions[1], ProposedAction::StartFocus { task: "Read".into() });
    }

    #[test]
    fn plans_never_delete() {
        let mixed = parse_reply(r#"{"actions":[{"type":"schedule_task","task":"A","at":"2026-12-01T09:00:00"},{"type":"delete_task","task":"B"}]}"#);
        assert_eq!(mixed.actions.len(), 1);
        assert!(mixed.rejected.iter().any(|r| r.contains("may not delete")));
        let only = parse_reply(r#"{"actions":[{"type":"delete_task","task":"B"}]}"#);
        assert_eq!(only.actions.len(), 1);
    }
}
