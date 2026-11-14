//! Runs deep research for real: the on-device model and the live web.
//!
//!   cargo run --release --example research_try -- "question" [quick|standard|deep]

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use thulli_core::ai::{pack, Models};
use thulli_core::research::{run, Depth, EngineBrain, Event, LiveWeb};

fn main() {
    let mut args = std::env::args().skip(1);
    let question = args.next().unwrap_or_else(|| "How does a Bloom filter work and when is it useful?".to_string());
    let depth = match args.next().as_deref() {
        Some("deep") => Depth::Deep,
        Some("standard") => Depth::Standard,
        _ => Depth::Quick,
    };
    let models = Models::default();
    let llm = models.llm(&mut |p: pack::PackProgress| println!("  {}", p.describe())).expect("planner");
    let brain = EngineBrain(llm as Arc<dyn thulli_core::services::assistant::ChatEngine>);
    let web = LiveWeb { language: "en".to_string() };
    let stop = AtomicBool::new(false);
    let t = Instant::now();
    let report = run(&question, depth, "English", &brain, &web, &stop, &mut |e| {
        let at = t.elapsed().as_secs();
        match e {
            Event::Round(n) => println!("[{at:>4}s] round {n}"),
            Event::Planned(q) => println!("[{at:>4}s]   plan: {}", q.join(" | ")),
            Event::Searching(q) => println!("[{at:>4}s]   searching: {q}"),
            Event::Found { query, count } => println!("[{at:>4}s]   found {count} for: {query}"),
            Event::Reading { url, done, total } => println!("[{at:>4}s]   reading {done}/{total}: {url}"),
            Event::Note { n, title, .. } => println!("[{at:>4}s]   note [{n}] {title}"),
            Event::Skipped { url, why } => println!("[{at:>4}s]   skipped {url}: {why}"),
            Event::Merging => println!("[{at:>4}s]   merging notes"),
            Event::Writing => println!("[{at:>4}s]   writing the answer"),
        }
    });
    println!("\n=== answer ({} pages read, {} rounds, {:.0}s) ===\n{}\n", report.pages_read, report.rounds, report.elapsed.as_secs_f32(), report.answer);
    for s in &report.sources {
        println!("[{}] {} - {}", s.n, s.title, s.url);
    }
}
