//! Runs the on-device planner over a file of test requests (one JSON object
//! per line: {"lang","domain","text"}) and scores each reply. The models load
//! once. Results go to a JSONL file; summarise it with the PowerShell snippet
//! in AGENTS.md or any JSON tool.
//!
//!   cargo run --release --example ai_eval -- cases.jsonl results.jsonl [skip] [limit]
//!
//! The test data is synthetic: a small fixture store with English task
//! titles, so a request in another language has to find them.

use std::io::Write as _;
use std::time::Instant;

use chrono::{Duration, Local, NaiveTime, TimeZone, Utc};
use serde_json::{json, Value};
use thulli_core::ai::{pack, Models};
use thulli_core::models::{AppStateStore, Task, TaskList, TaskSection};
use thulli_core::services::assistant::{chat_messages, parse_reply, plan_messages, ChatEngine, ProposedAction};

fn fixture(lang: &str) -> AppStateStore {
    let mut store = AppStateStore::default();
    store.preferences.language = lang.to_string();
    let list = TaskList::new("Work", "#5B8DEF");
    let list_id = list.id.clone();
    store.lists.push(list);
    let tomorrow = Local::now().date_naive() + Duration::days(1);
    let at_ten = Local
        .from_local_datetime(&tomorrow.and_time(NaiveTime::from_hms_opt(10, 0, 0).unwrap()))
        .earliest()
        .map(|d| d.with_timezone(&Utc));
    for (title, mins, section, scheduled) in [
        ("Write project report", 120, TaskSection::Today, None),
        ("Submit tax form", 30, TaskSection::ThisWeek, None),
        ("Prepare slides", 60, TaskSection::ThisWeek, at_ten),
        ("Team review", 60, TaskSection::ThisWeek, at_ten),
    ] {
        let mut t = Task::new(list_id.clone(), title, mins, section);
        t.scheduled_at = scheduled;
        store.tasks.push(t);
    }
    store
}

/// True/false when the reply should be in a non-Latin script; None for Latin-script languages.
fn script_ok(lang: &str, text: &str) -> Option<bool> {
    let any = |f: &dyn Fn(char) -> bool| text.chars().any(|c| f(c));
    let r = |lo: u32, hi: u32| move |c: char| (lo..=hi).contains(&(c as u32));
    Some(match lang {
        "zh" | "yue" => any(&r(0x4E00, 0x9FFF)),
        "ja" => any(&r(0x3040, 0x30FF)) || any(&r(0x4E00, 0x9FFF)),
        "ko" => any(&r(0xAC00, 0xD7AF)),
        "ar" | "arz" | "ary" | "ars" | "apc" | "acm" | "afb" | "arq" | "apd" | "aeb" | "ayl" | "fa" | "ur" | "ps" | "pbt" | "ug" | "sd" | "ks" => {
            any(&r(0x0600, 0x06FF))
        }
        "he" => any(&r(0x0590, 0x05FF)),
        "hi" | "mr" | "ne" | "bho" | "mai" => any(&r(0x0900, 0x097F)),
        "bn" | "as" => any(&r(0x0980, 0x09FF)),
        "ta" => any(&r(0x0B80, 0x0BFF)),
        "te" => any(&r(0x0C00, 0x0C7F)),
        "th" => any(&r(0x0E00, 0x0E7F)),
        "ru" | "uk" | "bg" | "be" | "sr" | "mk" | "kk" | "tt" | "ba" => any(&r(0x0400, 0x04FF)),
        "el" => any(&r(0x0370, 0x03FF)),
        "ka" => any(&r(0x10A0, 0x10FF)),
        "hy" => any(&r(0x0530, 0x058F)),
        "am" | "ti" => any(&r(0x1200, 0x137F)),
        _ => return None,
    })
}

fn kind(a: &ProposedAction) -> &'static str {
    match a {
        ProposedAction::CreateTask { .. } => "create_task",
        ProposedAction::AddSubtask { .. } => "add_subtask",
        ProposedAction::CreateNote { .. } => "create_note",
        ProposedAction::ScheduleTask { .. } => "schedule_task",
        ProposedAction::SetQuadrant { .. } => "set_quadrant",
        ProposedAction::StartFocus { .. } => "start_focus",
        ProposedAction::StopFocus => "stop_focus",
        ProposedAction::LogTime { .. } => "log_time",
        ProposedAction::UpdateTask { .. } => "update_task",
        ProposedAction::CompleteTask { .. } => "complete_task",
        ProposedAction::DeleteTask { .. } => "delete_task",
        ProposedAction::CreateSpace { .. } => "create_space",
        ProposedAction::RenameSpace { .. } => "rename_space",
        ProposedAction::DeleteSpace { .. } => "delete_space",
        ProposedAction::CreateGoal { .. } => "create_goal",
        ProposedAction::LinkTask { .. } => "link_task",
    }
}

/// Visible text of the proposals, for the script check.
fn visible(actions: &[ProposedAction]) -> String {
    actions.iter().map(|a| a.describe()).collect::<Vec<_>>().join(" ")
}

/// Did the reply do the right thing for this domain? Judged by structure only.
fn passes(domain: &str, actions: &[ProposedAction], text: &str) -> bool {
    let types: Vec<&str> = actions.iter().map(kind).collect();
    match domain {
        "create_task" => types.contains(&"create_task"),
        "due_priority" => actions.iter().any(|a| {
            matches!(a, ProposedAction::CreateTask { quadrant, do_date, .. } if quadrant.is_some() || do_date.is_some())
                || matches!(a, ProposedAction::UpdateTask { deadline: Some(_), .. })
        }),
        "start_pomodoro" => types.contains(&"start_focus"),
        // No action exists for logging time; the right behaviour is not to invent a task.
        "timesheet" => !types.contains(&"create_task") && !types.contains(&"delete_task"),
        "reschedule" => types.contains(&"schedule_task") || types.contains(&"update_task"),
        "multi_week_plan" => types.contains(&"create_goal") || types.iter().filter(|t| **t == "create_task").count() >= 3,
        // Questions: a written answer and no changes to the user's data.
        "productivity_patterns" | "general_knowledge" => actions.is_empty() && text.trim().chars().count() > 20,
        _ => false,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cases_path = args.next().expect("cases.jsonl");
    let out_path = args.next().expect("results.jsonl");
    let skip: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let limit: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);

    let cases: Vec<Value> = std::fs::read_to_string(&cases_path)
        .expect("read cases")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("case line"))
        .collect();

    let models = Models::default();
    let llm = models.llm(&mut |p: pack::PackProgress| println!("  {}", p.describe())).expect("planner");
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&out_path).expect("results file");

    for (i, case) in cases.iter().enumerate().skip(skip).take(limit) {
        let lang = case["lang"].as_str().unwrap_or("en");
        let domain = case["domain"].as_str().unwrap_or("");
        let text = case["text"].as_str().unwrap_or("");
        let store = fixture(lang);
        let chat = matches!(domain, "productivity_patterns" | "general_knowledge");
        let messages = if chat { chat_messages(&store, text) } else { plan_messages(&store, text) };

        let started = Instant::now();
        let mut pieces = 0usize;
        let reply = llm.generate(&messages, &mut |_| {
            pieces += 1;
            true
        });
        let secs = started.elapsed().as_secs_f64();
        let (reply, error) = match reply {
            Ok(r) => (r, String::new()),
            Err(e) => (String::new(), e.user_message()),
        };
        let answer = thulli_core::ai::llm::strip_thinking(&reply).to_string();
        let parsed = parse_reply(&answer);
        let types: Vec<&str> = parsed.actions.iter().map(kind).collect();
        let shown = format!("{} {}", visible(&parsed.actions), parsed.text);
        let line = json!({
            "n": i,
            "lang": lang,
            "domain": domain,
            "secs": (secs * 10.0).round() / 10.0,
            "pieces": pieces,
            "capped": pieces >= 3072,
            "parsed_ok": !parsed.actions.is_empty() || chat,
            "actions": types,
            "rejected": parsed.rejected.len(),
            "pass": passes(domain, &parsed.actions, &parsed.text),
            "script_ok": script_ok(lang, &shown),
            "error": error,
            "reply": answer.chars().take(1200).collect::<String>(),
        });
        writeln!(out, "{line}").ok();
        out.flush().ok();
        println!("[{i}] {lang} {domain}: pass={} actions={:?} {secs:.0}s", passes(domain, &parsed.actions, &parsed.text), types);
    }
}
