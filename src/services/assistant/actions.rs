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
//!   {"type": "start_focus", "task": "Write report"}
//! ]}
//!
//! `create_task` may also carry `quadrant`, `do_date` (YYYY-MM-DD) and up to
//! eight `steps`. A task is referred to by its short id or its exact title.
//! ```

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;

use chrono::NaiveDate;

use crate::models::{MatrixQuadrant, TaskSection};

/// Most actions accepted from one reply.
pub const MAX_ACTIONS: usize = 25;
/// Most steps proposed under one new task.
pub const MAX_STEPS: usize = 8;
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
        steps: Vec<String>,
    },
    AddSubtask { task: String, title: String },
    CreateNote { title: String, body: String },
    ScheduleTask { task: String, at: DateTime<Utc> },
    SetQuadrant { task: String, quadrant: MatrixQuadrant },
    /// Starts a focus round on the task once the changes are applied.
    StartFocus { task: String },
}

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
            ProposedAction::CreateTask { title, section, estimate_mins, list, quadrant, do_date, steps } => {
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
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
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
        #[serde(default)]
        steps: Option<Vec<String>>,
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
        task: String,
    },
}

/// Reads a model reply: separates prose from a JSON action block and
/// validates each proposed action.
pub fn parse_reply(raw: &str) -> ParsedReply {
    let mut out = ParsedReply::default();
    let Some(found) = find_envelope(raw) else {
        out.text = raw.trim().to_string();
        return out;
    };

    out.text = format!("{}\n{}", raw[..found.start].trim_end(), raw[found.end..].trim_start()).trim().to_string();
    out.text = strip_empty_fences(&out.text);

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
    out
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

fn validate(value: Value) -> Result<ProposedAction, String> {
    let raw: RawAction = serde_json::from_value(value).map_err(|e| e.to_string())?;
    match raw {
        RawAction::CreateTask { title, section, estimate_mins, list, quadrant, do_date, steps } => {
            let title = clean_line(&title, MAX_TITLE_CHARS, "title")?;
            let section = match section.as_deref().map(str::trim) {
                None | Some("") => TaskSection::Today,
                Some(s) => TaskSection::parse(s).ok_or_else(|| format!("unknown section \"{}\"", truncate(s, 30)))?,
            };
            let estimate_mins = estimate_mins.unwrap_or(0);
            if estimate_mins > MAX_ESTIMATE_MINS {
                return Err("estimate is too long".into());
            }
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
            let steps = steps.unwrap_or_default();
            if steps.len() > MAX_STEPS {
                return Err("too many steps".into());
            }
            let steps = steps.iter().map(|st| clean_line(st, MAX_TITLE_CHARS, "step")).collect::<Result<Vec<_>, _>>()?;
            // A quadrant decides the section, so the two always agree.
            let section = quadrant.map(|q| q.section()).unwrap_or(section);
            Ok(ProposedAction::CreateTask { title, section, estimate_mins, list, quadrant, do_date, steps })
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
    }
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
mod tests {
    use super::*;

    fn actions(raw: &str) -> ParsedReply {
        parse_reply(raw)
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
            {\"type\":\"delete_task\",\"task\":\"x\"},\
            {\"type\":\"create_task\"},\
            {\"type\":\"create_task\",\"title\":\"fine\"}]}",
        );
        assert_eq!(p.actions.len(), 1);
        assert_eq!(p.rejected.len(), 3);
    }

    #[test]
    fn bad_values_are_rejected_not_repaired() {
        let p = actions(
            "{\"actions\":[\
            {\"type\":\"create_task\",\"title\":\"   \"},\
            {\"type\":\"create_task\",\"title\":\"x\",\"section\":\"someday\"},\
            {\"type\":\"create_task\",\"title\":\"x\",\"estimate_mins\":99999},\
            {\"type\":\"create_task\",\"title\":\"x\",\"estimate_mins\":-5},\
            {\"type\":\"create_task\",\"title\":\"bell\\u0007\"},\
            {\"type\":\"schedule_task\",\"task\":\"x\",\"at\":\"next tuesday\"},\
            {\"type\":\"schedule_task\",\"task\":\"x\",\"at\":\"1850-01-01T00:00:00Z\"}]}",
        );
        assert!(p.actions.is_empty(), "{:?}", p.actions);
        assert_eq!(p.rejected.len(), 7);
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
            steps: Vec::new(),
        };
        assert_eq!(a.describe(), "Add task \"Write\" to Today (25 min)");
    }
}
