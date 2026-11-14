//! Turns research examples into training rows with the exact prompts the app
//! sends to the model during deep research.
//!
//!   cargo run --example research_rows -- examples.jsonl rows.jsonl
//!
//! Input line (one per example), by "kind":
//!   plan:    {id,lang,split,kind,language,question,queries:[..]}
//!   extract: {id,lang,split,kind,question,chunk,facts|null}
//!   gap:     {id,lang,split,kind,question,searches:[..],notes:[[n,text],..],missing:[..]}
//!   merge:   {id,lang,split,kind,notes:[[n,text],..],merged}
//!   answer:  {id,lang,split,kind,language,question,notes:[[n,text],..],answer}
//! Output line: {id,lang,split,domain:"research_<kind>",system,user,target}.

use std::io::Write as _;

use serde_json::{json, Value};
use thulli_core::research::{answer_prompt, answer_user, extract_user, gap_prompt, gap_user, merge_user, plan_prompt, EXTRACT_PROMPT, MERGE_PROMPT};

fn notes(v: &Value) -> Vec<(usize, String)> {
    v.as_array().into_iter().flatten().map(|p| (p[0].as_u64().unwrap_or(0) as usize, p[1].as_str().unwrap_or("").to_string())).collect()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let input = std::fs::read_to_string(args.next().expect("examples.jsonl")).expect("read");
    let mut out = std::io::BufWriter::new(std::fs::File::create(args.next().expect("rows.jsonl")).expect("create"));
    let mut n = 0;
    for line in input.lines().filter(|l| !l.trim().is_empty()) {
        let e: Value = serde_json::from_str(line).expect("json");
        let kind = e["kind"].as_str().expect("kind");
        let question = e["question"].as_str().unwrap_or("");
        let language = e["language"].as_str().unwrap_or("English");
        let (system, user, target) = match kind {
            "plan" => {
                let q = strings(&e["queries"]);
                (plan_prompt(language, q.len().clamp(2, 4)), question.to_string(), serde_json::to_string(&json!({ "queries": q })).unwrap())
            }
            "extract" => (EXTRACT_PROMPT.to_string(), extract_user(question, e["chunk"].as_str().unwrap_or("")), e["facts"].as_str().unwrap_or("NONE").to_string()),
            "gap" => {
                let m = strings(&e["missing"]);
                (gap_prompt(3), gap_user(question, &strings(&e["searches"]), &notes(&e["notes"])), serde_json::to_string(&json!({ "queries": m })).unwrap())
            }
            "merge" => (MERGE_PROMPT.to_string(), merge_user(&notes(&e["notes"])), e["merged"].as_str().unwrap_or("").to_string()),
            "answer" => (answer_prompt(language), answer_user(question, &notes(&e["notes"])), e["answer"].as_str().unwrap_or("").to_string()),
            other => panic!("unknown kind {other}"),
        };
        let row = json!({ "id": e["id"], "lang": e["lang"], "split": e["split"], "domain": format!("research_{kind}"), "system": system, "user": user, "target": target });
        writeln!(out, "{}", serde_json::to_string(&row).unwrap()).unwrap();
        n += 1;
    }
    println!("{n} rows");
}
