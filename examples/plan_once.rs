//! Sends one request (file given as the argument) to the on-device planner.
use std::time::Instant;
use thulli_core::ai::{pack, Models};
use thulli_core::services::assistant::{plan_messages, ChatEngine};

fn main() {
    let request = std::fs::read_to_string(std::env::args().nth(1).expect("prompt file")).expect("read");
    let models = Models::default();
    let llm = models.llm(&mut |p: pack::PackProgress| println!("  {}", p.describe())).expect("planner");
    let store = thulli_core::models::AppStateStore::default();
    let prompt = plan_messages(&store, &request);
    let t = Instant::now();
    let mut n = 0usize;
    let reply = llm.generate(&prompt, &mut |_| { n += 1; true }).expect("reply");
    println!("{n} pieces in {:.0?}", t.elapsed());
    let answer = thulli_core::ai::llm::strip_thinking(&reply);
    println!("--- answer ---\n{answer}\n---");
    let parsed = thulli_core::services::assistant::parse_reply(answer);
    println!("parsed actions: {}", parsed.actions.len());
    for a in &parsed.actions { println!("  {a:?}"); }
    println!("rejected: {:?}", parsed.rejected);
}
