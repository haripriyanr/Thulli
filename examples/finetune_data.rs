//! Turns example specs into training and test rows with the planner's exact
//! prompt, and checks that every gold answer is accepted by the real parser.
//!
//!   cargo run --example finetune_data -- specs.jsonl rows.jsonl
//!
//! Spec line: {"id","lang","domain","split","now":"2026-11-09T09:00","spaces":[..],
//! "goals":[..],"tasks":[{"id","title","mins","section","at","deadline","space"}],
//! "focus":[{"date","hour","mins"}],"request","gold":{"actions":[..]}}
//! Row line: the same keys plus "system", "user", "target", "gold_ok", "why".

use std::io::Write as _;

use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::{json, Value};
use thulli_core::models::{AppStateStore, Goal, Task, TaskList, TaskSection, TimeEntry, TimeEntryKind};
use thulli_core::services::assistant::{parse_reply, plan_messages_at};

fn local(dt: NaiveDateTime) -> chrono::DateTime<Local> {
    Local.from_local_datetime(&dt).earliest().expect("local time")
}

fn section(s: &str) -> TaskSection {
    match s {
        "today" => TaskSection::Today,
        "backlog" => TaskSection::Backlog,
        _ => TaskSection::ThisWeek,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let input = std::fs::read_to_string(args.next().expect("specs.jsonl")).expect("read specs");
    let mut out = std::io::BufWriter::new(std::fs::File::create(args.next().expect("rows.jsonl")).expect("create"));
    let (mut total, mut bad) = (0, 0);
    for line in input.lines().filter(|l| !l.trim().is_empty()) {
        let spec: Value = serde_json::from_str(line).expect("spec json");
        let now = local(NaiveDateTime::parse_from_str(spec["now"].as_str().unwrap(), "%Y-%m-%dT%H:%M").expect("now"));
        let mut store = AppStateStore::default();
        store.preferences.language = spec["lang"].as_str().unwrap().to_string();
        let mut lists: Vec<(String, String)> = Vec::new();
        for name in spec["spaces"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let l = TaskList::new(name, "#5B8DEF");
            lists.push((name.to_string(), l.id.clone()));
            store.lists.push(l);
        }
        for title in spec["goals"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            store.goals.push(Goal::new(title));
        }
        for t in spec["tasks"].as_array().into_iter().flatten() {
            let list_id = t["space"]
                .as_str()
                .and_then(|n| lists.iter().find(|(name, _)| name == n))
                .or(lists.first())
                .map(|(_, id)| id.clone())
                .unwrap_or_default();
            let mut task = Task::new(list_id, t["title"].as_str().unwrap(), t["mins"].as_u64().unwrap_or(0) as u32, section(t["section"].as_str().unwrap_or("this_week")));
            task.id = t["id"].as_str().unwrap().to_string();
            if let Some(at) = t["at"].as_str() {
                task.scheduled_at = Some(local(NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M").expect("at")).with_timezone(&Utc));
            }
            if let Some(d) = t["deadline"].as_str() {
                task.deadline = Some(NaiveDate::parse_from_str(d, "%Y-%m-%d").expect("deadline"));
            }
            store.tasks.push(task);
        }
        for f in spec["focus"].as_array().into_iter().flatten() {
            let date = NaiveDate::parse_from_str(f["date"].as_str().unwrap(), "%Y-%m-%d").expect("focus date");
            let start = local(date.and_hms_opt(f["hour"].as_u64().unwrap() as u32, 0, 0).unwrap()).with_timezone(&Utc);
            let end = start + chrono::Duration::minutes(f["mins"].as_i64().unwrap());
            store.time_entries.push(TimeEntry::new(TimeEntryKind::Focus, start, end));
        }
        let msgs = plan_messages_at(&store, spec["request"].as_str().unwrap(), now);
        // The spec may carry its own compact answer, so key order ("type" first) is kept.
        let target = spec["target"].as_str().map(str::to_string).unwrap_or_else(|| serde_json::to_string(&spec["gold"]).unwrap());
        let parsed = parse_reply(&target);
        let want = spec["gold"]["actions"].as_array().map_or(0, Vec::len);
        let ok = parsed.rejected.is_empty() && parsed.actions.len() == want;
        total += 1;
        if !ok {
            bad += 1;
        }
        let mut row = spec.clone();
        row["system"] = json!(msgs[0].content);
        row["user"] = json!(msgs[1].content);
        row["target"] = json!(target);
        row["gold_ok"] = json!(ok);
        row["why"] = json!(parsed.rejected);
        writeln!(out, "{}", serde_json::to_string(&row).unwrap()).unwrap();
    }
    println!("{total} rows, {bad} with a gold answer the parser does not fully accept");
}
