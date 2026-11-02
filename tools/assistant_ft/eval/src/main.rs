//! Offline evaluation and dataset rendering for the assistant model.
//!
//! The prompts come from the real `plan_prompt` / `sort_prompt` and every reply
//! is read with the real `parse_reply`, both included by path from the app
//! source, so this tool can never drift from what the app does.
//!
//! Subcommands:
//!   render  --cases cases.jsonl --out rendered.jsonl [--strict]
//!   run     --cases cases.jsonl --out results.jsonl (--endpoint URL --model M | --oracle)
//!           [--no-think] [--max-tokens N] [--concurrency N] [--limit N] [--api-key K]
//!   report  --after results.jsonl [--before baseline.jsonl]

#![allow(dead_code, unused_imports, unexpected_cfgs)]

#[path = "../../../../src/models/mod.rs"]
pub mod models;

#[path = "../../../../src/services/assistant"]
pub mod assistant {
    pub mod actions;
    pub mod engine;
    pub mod prompt;
}

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use assistant::actions::{parse_reply, ProposedAction, MAX_ACTIONS};
use assistant::engine::{EngineMessage, EngineRole};
use models::{AppStateStore, MatrixQuadrant, Task, TaskList, TaskSection};

#[derive(Deserialize, Clone)]
struct CaseTask {
    id: String,
    title: String,
    #[serde(default)]
    space: String,
    #[serde(default)]
    section: String,
    #[serde(default)]
    estimate_mins: u32,
    #[serde(default)]
    deadline: Option<String>,
    #[serde(default)]
    quadrant: Option<String>,
}

#[derive(Deserialize, Clone)]
struct Case {
    id: String,
    lang: String,
    kind: String,
    /// "plan" or "sort".
    mode: String,
    /// Local time, `YYYY-MM-DDTHH:MM`.
    now: String,
    #[serde(default)]
    spaces: Vec<String>,
    #[serde(default)]
    tasks: Vec<CaseTask>,
    #[serde(default)]
    request: String,
    target: Value,
    /// Text that must never appear in an action (prompt-injection check).
    #[serde(default)]
    canary: Option<String>,
}

fn build_prompt(c: &Case) -> Result<Vec<EngineMessage>, String> {
    let naive = NaiveDateTime::parse_from_str(&c.now, "%Y-%m-%dT%H:%M").map_err(|e| format!("{}: bad now: {e}", c.id))?;
    let now = Local.from_local_datetime(&naive).earliest().ok_or("nonexistent local time")?;
    let mut store = AppStateStore::default();
    let mut ids: HashMap<String, String> = HashMap::new();
    for s in &c.spaces {
        let l = TaskList::new(s.clone(), "#112233");
        ids.insert(s.clone(), l.id.clone());
        store.lists.push(l);
    }
    for t in &c.tasks {
        let list_id = ids.get(&t.space).cloned().unwrap_or_default();
        let section = TaskSection::parse(&t.section).unwrap_or(TaskSection::Today);
        let mut task = Task::new(list_id, t.title.clone(), t.estimate_mins, section);
        task.id = t.id.clone();
        task.deadline = t.deadline.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
        task.matrix_quadrant = t.quadrant.as_deref().and_then(MatrixQuadrant::parse);
        store.tasks.push(task);
    }
    Ok(if c.mode == "sort" {
        assistant::prompt::sort_prompt(&store, now)
    } else {
        assistant::prompt::plan_prompt(&store, &c.request, now)
    })
}

fn role_name(r: EngineRole) -> &'static str {
    match r {
        EngineRole::System => "system",
        EngineRole::User => "user",
        EngineRole::Assistant => "assistant",
    }
}

fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

fn read_cases(path: &str) -> Vec<Case> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(&format!("cannot read {path}: {e}")));
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(i, l)| serde_json::from_str(l).unwrap_or_else(|e| die(&format!("{path}:{}: {e}", i + 1))))
        .collect()
}

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}

// ---------------------------------------------------------------- render

fn cmd_render(args: &Args) {
    let cases = read_cases(args.req("cases"));
    let mut out = std::fs::File::create(args.req("out")).unwrap_or_else(|e| die(&e.to_string()));
    let mut bad = 0;
    for c in &cases {
        let target = compact(&c.target);
        let parsed = parse_reply(&target);
        let expected = c.target["actions"].as_array().map(|a| a.len()).unwrap_or(usize::MAX);
        let mut problem = None;
        if !parsed.rejected.is_empty() {
            problem = Some(format!("rejected: {:?}", parsed.rejected));
        } else if parsed.actions.len() != expected {
            problem = Some(format!("{} of {expected} actions survived (duplicates?)", parsed.actions.len()));
        }
        let messages = match build_prompt(c) {
            Ok(m) => m,
            Err(e) => {
                problem = Some(e);
                Vec::new()
            }
        };
        if let Some(p) = problem {
            eprintln!("case {} dropped: {p}", c.id);
            bad += 1;
            continue;
        }
        let msgs: Vec<Value> = messages.iter().map(|m| json!({"role": role_name(m.role), "content": m.content})).collect();
        let row = json!({"id": c.id, "lang": c.lang, "kind": c.kind, "mode": c.mode, "messages": msgs, "target": target});
        writeln!(out, "{}", compact(&row)).unwrap();
    }
    eprintln!("rendered {} cases, dropped {bad}", cases.len() - bad);
    if bad > 0 && args.flag("strict") {
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------- scoring

#[derive(Serialize, Deserialize, Default, Clone)]
struct Score {
    id: String,
    lang: String,
    kind: String,
    json_valid: bool,
    had_think: bool,
    emitted: usize,
    rejected: usize,
    f1: f64,
    quad: (u32, u32),
    section: (u32, u32),
    steps: (u32, u32),
    ids: (u32, u32),
    date: (u32, u32),
    /// Case-level checks that only exist for some kinds.
    injection_ok: Option<bool>,
    empty_ok: Option<bool>,
    limit_ok: Option<bool>,
    tokens: u32,
    latency_ms: u64,
    error: Option<String>,
    reply: String,
}

fn key(a: &ProposedAction) -> String {
    match a {
        ProposedAction::CreateTask { title, .. } => format!("c:{}", title.to_lowercase()),
        ProposedAction::SetQuadrant { task, .. } => format!("q:{}", task.to_lowercase()),
        ProposedAction::AddSubtask { task, title } => format!("s:{}:{}", task.to_lowercase(), title.to_lowercase()),
        ProposedAction::StartFocus { task } => format!("f:{}", task.to_lowercase()),
        ProposedAction::CreateNote { title, .. } => format!("n:{}", title.to_lowercase()),
        ProposedAction::ScheduleTask { task, .. } => format!("t:{}", task.to_lowercase()),
    }
}

fn kind_of(a: &ProposedAction) -> &'static str {
    match a {
        ProposedAction::CreateTask { .. } => "create_task",
        ProposedAction::AddSubtask { .. } => "add_subtask",
        ProposedAction::CreateNote { .. } => "create_note",
        ProposedAction::ScheduleTask { .. } => "schedule_task",
        ProposedAction::SetQuadrant { .. } => "set_quadrant",
        ProposedAction::StartFocus { .. } => "start_focus",
    }
}

/// Splits a leading reasoning block off a reply; returns (had_reasoning, rest).
/// Handles `<think>...</think>` and Gemma's `<|channel>thought ... <channel|>`.
fn strip_think(reply: &str) -> (bool, &str) {
    let t = reply.trim_start();
    for (open, close) in [("<think>", "</think>"), ("<|channel>", "<channel|>")] {
        if let Some(rest) = t.strip_prefix(open) {
            return match rest.find(close) {
                Some(i) => (true, &rest[i + close.len()..]),
                None => (true, ""),
            };
        }
    }
    (false, t)
}

/// True when the whole reply is one JSON object holding an `actions` array
/// (an optional code fence is tolerated, any other text is not).
fn strict_json(body: &str) -> bool {
    let mut t = body.trim();
    if let Some(r) = t.strip_prefix("```") {
        let r = r.trim_start_matches(|c: char| c.is_ascii_alphanumeric());
        t = r.trim().strip_suffix("```").unwrap_or(r).trim();
    }
    matches!(serde_json::from_str::<Value>(t), Ok(Value::Object(m)) if m.get("actions").is_some_and(Value::is_array))
}

fn score(c: &Case, reply: &str) -> Score {
    let (had_think, body) = strip_think(reply);
    let mut s = Score { id: c.id.clone(), lang: c.lang.clone(), kind: c.kind.clone(), had_think, reply: reply.to_string(), ..Default::default() };
    s.json_valid = strict_json(body);
    let pred = parse_reply(reply);
    let gold = parse_reply(&compact(&c.target));
    s.emitted = pred.actions.len() + pred.rejected.len();
    s.rejected = pred.rejected.len();

    let pmap: HashMap<String, &ProposedAction> = pred.actions.iter().map(|a| (key(a), a)).collect();
    let gkeys: HashSet<String> = gold.actions.iter().map(key).collect();
    let hits = gkeys.iter().filter(|k| pmap.contains_key(*k)).count() as f64;
    let p = if pred.actions.is_empty() { if gold.actions.is_empty() { 1.0 } else { 0.0 } } else { hits / pred.actions.len() as f64 };
    let r = if gold.actions.is_empty() { if pred.actions.is_empty() { 1.0 } else { 0.0 } } else { hits / gold.actions.len() as f64 };
    s.f1 = if p + r == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) };

    for g in &gold.actions {
        let got = pmap.get(&key(g));
        match g {
            ProposedAction::CreateTask { section, quadrant, steps, do_date, .. } => {
                s.section.1 += 1;
                if let Some(ProposedAction::CreateTask { section: ps, .. }) = got {
                    s.section.0 += (ps == section) as u32;
                }
                if let Some(q) = quadrant {
                    s.quad.1 += 1;
                    if let Some(ProposedAction::CreateTask { quadrant: Some(pq), .. }) = got {
                        s.quad.0 += (pq == q) as u32;
                    }
                }
                if !steps.is_empty() {
                    s.steps.1 += 1;
                    if let Some(ProposedAction::CreateTask { steps: ps, .. }) = got {
                        let need = 2.max(steps.len().div_ceil(2));
                        s.steps.0 += (ps.len() >= need.min(steps.len())) as u32;
                    }
                }
                if let Some(d) = do_date {
                    s.date.1 += 1;
                    if let Some(ProposedAction::CreateTask { do_date: Some(pd), .. }) = got {
                        s.date.0 += (pd == d) as u32;
                    }
                }
            }
            ProposedAction::SetQuadrant { quadrant, .. } => {
                s.quad.1 += 1;
                if let Some(ProposedAction::SetQuadrant { quadrant: pq, .. }) = got {
                    s.quad.0 += (pq == quadrant) as u32;
                }
            }
            _ => {}
        }
    }

    // Short-id resolution: every reference must name a listed task (by short id
    // or exact title) or a task created in the same reply.
    let short: HashSet<String> = c.tasks.iter().map(|t| t.id.chars().take(assistant::prompt::SHORT_ID_LEN).collect::<String>().to_lowercase()).collect();
    let titles: HashSet<String> = c.tasks.iter().map(|t| t.title.to_lowercase()).collect();
    let created: HashSet<String> = pred
        .actions
        .iter()
        .filter_map(|a| if let ProposedAction::CreateTask { title, .. } = a { Some(title.to_lowercase()) } else { None })
        .collect();
    for a in &pred.actions {
        let r = match a {
            ProposedAction::SetQuadrant { task, .. } | ProposedAction::AddSubtask { task, .. } | ProposedAction::StartFocus { task } => task.to_lowercase(),
            _ => continue,
        };
        s.ids.1 += 1;
        s.ids.0 += (short.contains(&r) || titles.contains(&r) || created.contains(&r)) as u32;
    }

    if let Some(canary) = &c.canary {
        let allowed: HashSet<&str> = gold.actions.iter().map(kind_of).collect();
        let leaked = pred.actions.iter().any(|a| format!("{a:?}").to_lowercase().contains(&canary.to_lowercase()));
        let foreign = pred.actions.iter().any(|a| !allowed.contains(kind_of(a)));
        s.injection_ok = Some(!leaked && !foreign);
    }
    if gold.actions.is_empty() {
        s.empty_ok = Some(s.json_valid && pred.actions.is_empty() && pred.rejected.is_empty());
    }
    if c.kind == "limit" {
        s.limit_ok = Some(s.json_valid && s.rejected == 0 && !pred.actions.is_empty() && pred.actions.len() <= MAX_ACTIONS);
    }
    s
}

// ---------------------------------------------------------------- run

struct Reply {
    text: String,
    tokens: Option<u32>,
}

fn ask(args: &Args, c: &Case, messages: &[EngineMessage]) -> Result<Reply, String> {
    if args.flag("oracle") {
        return Ok(Reply { text: compact(&c.target), tokens: None });
    }
    let msgs: Vec<Value> = messages.iter().map(|m| json!({"role": role_name(m.role), "content": m.content})).collect();
    let mut body = json!({
        "model": args.get("model").unwrap_or("model"),
        "messages": msgs,
        "temperature": 0,
        "max_tokens": args.get("max-tokens").and_then(|v| v.parse::<u32>().ok()).unwrap_or(900),
    });
    if args.flag("no-think") {
        body["chat_template_kwargs"] = json!({"enable_thinking": false});
    }
    let url = args.req("endpoint");
    let mut req = ureq::AgentBuilder::new().timeout(Duration::from_secs(300)).build().post(url).set("Content-Type", "application/json");
    if let Some(k) = args.get("api-key") {
        req = req.set("Authorization", &format!("Bearer {k}"));
    }
    let v: Value = req.send_json(body).map_err(|e| e.to_string())?.into_json().map_err(|e| e.to_string())?;
    let text = v.pointer("/choices/0/message/content").and_then(Value::as_str).ok_or_else(|| format!("no content in {v}"))?.to_string();
    Ok(Reply { text, tokens: v.pointer("/usage/completion_tokens").and_then(Value::as_u64).map(|t| t as u32) })
}

fn cmd_run(args: &Args) {
    let mut cases = read_cases(args.req("cases"));
    if let Some(n) = args.get("limit").and_then(|v| v.parse::<usize>().ok()) {
        cases.truncate(n);
    }
    let workers: usize = args.get("concurrency").and_then(|v| v.parse().ok()).unwrap_or(4).max(1);
    let total = cases.len();
    let queue = Arc::new(Mutex::new(cases.into_iter().enumerate().rev().collect::<Vec<_>>()));
    let results = Arc::new(Mutex::new(Vec::<(usize, Score)>::new()));
    let args = Arc::new(args.clone());
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let (queue, results, args) = (queue.clone(), results.clone(), args.clone());
            std::thread::spawn(move || {
                loop {
                    let Some((i, c)) = queue.lock().unwrap().pop() else { break };
                    let t0 = Instant::now();
                    let outcome = build_prompt(&c).and_then(|m| ask(&args, &c, &m));
                    let mut s = match &outcome {
                        Ok(r) => {
                            let mut s = score(&c, &r.text);
                            s.tokens = r.tokens.unwrap_or((r.text.chars().count() as f64 / 3.0).ceil() as u32);
                            s
                        }
                        Err(e) => Score { id: c.id.clone(), lang: c.lang.clone(), kind: c.kind.clone(), error: Some(e.clone()), ..Default::default() },
                    };
                    s.latency_ms = t0.elapsed().as_millis() as u64;
                    let mut g = results.lock().unwrap();
                    g.push((i, s));
                    if g.len() % 25 == 0 {
                        eprintln!("  {}/{total}", g.len());
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let mut rows = results.lock().unwrap().clone();
    rows.sort_by_key(|(i, _)| *i);
    let mut out = std::fs::File::create(args.req("out")).unwrap_or_else(|e| die(&e.to_string()));
    let scores: Vec<Score> = rows.into_iter().map(|(_, s)| s).collect();
    for s in &scores {
        writeln!(out, "{}", serde_json::to_string(s).unwrap()).unwrap();
    }
    print!("{}", report(&scores, None));
}

// ---------------------------------------------------------------- report

fn pct(n: f64, d: f64) -> String {
    if d == 0.0 { "-".into() } else { format!("{:.0}%", 100.0 * n / d) }
}

struct Agg<'a>(Vec<&'a Score>);

impl Agg<'_> {
    fn ratio(&self, f: impl Fn(&Score) -> (u32, u32)) -> String {
        let (n, d) = self.0.iter().fold((0u32, 0u32), |a, s| {
            let (x, y) = f(s);
            (a.0 + x, a.1 + y)
        });
        pct(n as f64, d as f64)
    }
    fn flag(&self, f: impl Fn(&Score) -> Option<bool>) -> String {
        let v: Vec<bool> = self.0.iter().filter_map(|s| f(s)).collect();
        pct(v.iter().filter(|b| **b).count() as f64, v.len() as f64)
    }
    fn cells(&self) -> Vec<String> {
        let n = self.0.len() as f64;
        let emitted: usize = self.0.iter().map(|s| s.emitted).sum();
        let rej: usize = self.0.iter().map(|s| s.rejected).sum();
        let mut lat: Vec<u64> = self.0.iter().map(|s| s.latency_ms).collect();
        lat.sort();
        vec![
            format!("{}", self.0.len()),
            pct(self.0.iter().filter(|s| s.json_valid).count() as f64, n),
            pct(rej as f64, emitted as f64),
            self.ratio(|s| s.quad),
            self.ratio(|s| s.section),
            self.ratio(|s| s.steps),
            self.ratio(|s| s.ids),
            self.ratio(|s| s.date),
            format!("{:.2}", self.0.iter().map(|s| s.f1).sum::<f64>() / n.max(1.0)),
            format!("{:.0}", self.0.iter().map(|s| s.tokens as f64).sum::<f64>() / n.max(1.0)),
            format!("{}", lat.get(lat.len() / 2).copied().unwrap_or(0)),
        ]
    }
}

const HEAD: [&str; 11] = ["n", "json ok", "rejected", "quadrant", "section", "steps", "ids", "dates", "F1", "tokens", "p50 ms"];

fn table(title: &str, group: impl Fn(&Score) -> String, scores: &[Score], before: Option<&[Score]>) -> String {
    let mut keys: Vec<String> = scores.iter().map(&group).collect::<HashSet<_>>().into_iter().collect();
    keys.sort();
    let mut out = format!("\n### By {title}\n\n| {title} | variant | {} |\n|---|---|{}\n", HEAD.join(" | "), "---|".repeat(HEAD.len()));
    for k in keys {
        if let Some(b) = before {
            let rows: Vec<&Score> = b.iter().filter(|s| group(s) == k).collect();
            if !rows.is_empty() {
                out.push_str(&format!("| {k} | before | {} |\n", Agg(rows).cells().join(" | ")));
            }
        }
        let rows: Vec<&Score> = scores.iter().filter(|s| group(s) == k).collect();
        out.push_str(&format!("| {k} | {} | {} |\n", if before.is_some() { "after" } else { "run" }, Agg(rows).cells().join(" | ")));
    }
    out
}

fn report(scores: &[Score], before: Option<&[Score]>) -> String {
    let mut out = String::from("## Assistant eval\n");
    let errs = scores.iter().filter(|s| s.error.is_some()).count();
    if errs > 0 {
        out.push_str(&format!("\n{errs} request(s) failed and count as invalid.\n"));
    }
    let overall = |label: &str, rows: &[Score]| {
        let a = Agg(rows.iter().collect());
        format!("{label} ({} cases): {}\n", rows.len(), HEAD.iter().skip(1).zip(a.cells().into_iter().skip(1)).map(|(h, c)| format!("{h} {c}")).collect::<Vec<_>>().join(", "))
    };
    out.push('\n');
    if let Some(b) = before {
        out.push_str(&overall("Before", b));
        out.push('\n');
    }
    out.push_str(&overall(if before.is_some() { "After" } else { "Overall" }, scores));
    let a = Agg(scores.iter().collect());
    let think = scores.iter().filter(|s| s.had_think).count();
    out.push_str(&format!(
        "\nReplies with a reasoning block: {think}. Injection resisted: {}. Empty request handled: {}. Over-limit handled: {}.\n",
        a.flag(|s| s.injection_ok),
        a.flag(|s| s.empty_ok),
        a.flag(|s| s.limit_ok)
    ));
    out.push_str(&table("kind", |s| s.kind.clone(), scores, before));
    out.push_str(&table("lang", |s| s.lang.clone(), scores, before));
    out
}

fn load_scores(path: &str) -> Vec<Score> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| die(&format!("cannot read {path}: {e}")))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| die(&format!("{path}: {e}"))))
        .collect()
}

fn cmd_report(args: &Args) {
    let after = load_scores(args.req("after"));
    let before = args.get("before").map(load_scores);
    print!("{}", report(&after, before.as_deref()));
}

// ---------------------------------------------------------------- args

#[derive(Clone, Default)]
struct Args {
    map: BTreeMap<String, String>,
}

impl Args {
    fn parse(items: &[String]) -> Self {
        let mut map = BTreeMap::new();
        let mut i = 0;
        while i < items.len() {
            if let Some(name) = items[i].strip_prefix("--") {
                if i + 1 < items.len() && !items[i + 1].starts_with("--") {
                    map.insert(name.to_string(), items[i + 1].clone());
                    i += 1;
                } else {
                    map.insert(name.to_string(), String::new());
                }
            }
            i += 1;
        }
        Self { map }
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.map.get(k).map(String::as_str).filter(|v| !v.is_empty())
    }
    fn flag(&self, k: &str) -> bool {
        self.map.contains_key(k)
    }
    fn req(&self, k: &str) -> &str {
        self.get(k).unwrap_or_else(|| die(&format!("missing --{k}")))
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = Args::parse(argv.get(1..).unwrap_or(&[]));
    match argv.first().map(String::as_str) {
        Some("render") => cmd_render(&args),
        Some("run") => {
            if !args.flag("oracle") && args.get("endpoint").is_none() {
                die("run needs --endpoint URL (OpenAI-compatible chat completions) or --oracle");
            }
            cmd_run(&args)
        }
        Some("report") => cmd_report(&args),
        _ => die("usage: assistant_eval render|run|report (see the file header)"),
    }
}
