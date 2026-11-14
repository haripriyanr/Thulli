//! Tries the web search layer for real, from this machine.
//!
//!   cargo run --example search_try -- "your question"            asks every source
//!   cargo run --example search_try -- "your question" fetch      also reads the top page

use std::time::Instant;
use thulli_core::search::{fetch_page, providers::ProviderKind, search, search_one, SearchOptions};

fn main() {
    let mut args = std::env::args().skip(1);
    let query = args.next().unwrap_or_else(|| "rust ownership".to_string());
    let fetch = args.next().as_deref() == Some("fetch");

    println!("each source on its own:");
    for kind in ProviderKind::ALL {
        let t = Instant::now();
        match search_one(kind, &query, 5, "en") {
            Ok(list) => println!("  {:<11} {:>2} results in {:.1}s{}", kind.name(), list.len(), t.elapsed().as_secs_f32(), list.first().map(|r| format!("  first: {}", r.title)).unwrap_or_default()),
            Err(why) => println!("  {:<11} failed: {why}", kind.name()),
        }
    }

    println!("\nmerged:");
    let outcome = search(&query, &SearchOptions { max_results: 8, ..Default::default() });
    for (i, r) in outcome.results.iter().enumerate() {
        println!("  {}. [{}] {}\n     {}\n     {}", i + 1, r.source, r.title, r.url, r.snippet.chars().take(110).collect::<String>());
    }
    for (kind, why) in &outcome.problems {
        println!("  (problem with {}: {why})", kind.name());
    }

    if fetch {
        if let Some(top) = outcome.results.first() {
            println!("\nreading {} ...", top.url);
            match fetch_page(&top.url, 1500) {
                Ok(p) => println!("  title: {}\n  {} chars{}\n  {}", p.title, p.text.chars().count(), if p.truncated { " (cut)" } else { "" }, p.text.chars().take(400).collect::<String>().replace('\n', " | ")),
                Err(why) => println!("  could not read it: {why}"),
            }
        }
    }
}
