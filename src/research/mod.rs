//! Deep research: answer a question from the web, in stages, with sources.
//!
//! The code decides what happens and in what order; the small on-device model
//! is only asked narrow questions it can answer well: which searches to run,
//! which facts one page chunk holds, how to merge notes, and finally the
//! answer with numbered sources. Pages are read a few at a time, the search
//! can go for several rounds (each round asks what is still missing), and a
//! budget caps pages, rounds and time. Everything can be stopped at any point
//! and the notes gathered so far are still turned into an answer.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::search::{self, FetchedPage, SearchOptions, SearchOutcome, SearchResult};
use crate::services::assistant::{ChatEngine, EngineMessage, EngineRole};

/// How much effort a question gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// One round, a handful of pages. Seconds to a few minutes.
    Quick,
    /// Two rounds, up to about twenty pages.
    Standard,
    /// Up to six rounds and over a hundred pages. Can take an hour on a CPU.
    Deep,
}

/// The limits a [`Depth`] stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub rounds: usize,
    pub queries_per_round: usize,
    pub results_per_query: usize,
    pub pages_per_round: usize,
    pub max_pages: usize,
    /// Characters of one page handed to the model at a time.
    pub chunk_chars: usize,
    /// Most chunks read from one page.
    pub chunks_per_page: usize,
    pub fetch_threads: usize,
    pub time_limit: Duration,
}

impl Depth {
    pub fn budget(self) -> Budget {
        match self {
            Depth::Quick => Budget { rounds: 1, queries_per_round: 2, results_per_query: 6, pages_per_round: 5, max_pages: 5, chunk_chars: 3500, chunks_per_page: 1, fetch_threads: 4, time_limit: Duration::from_secs(15 * 60) },
            Depth::Standard => Budget { rounds: 2, queries_per_round: 3, results_per_query: 8, pages_per_round: 10, max_pages: 20, chunk_chars: 3500, chunks_per_page: 2, fetch_threads: 4, time_limit: Duration::from_secs(45 * 60) },
            Depth::Deep => Budget { rounds: 6, queries_per_round: 4, results_per_query: 10, pages_per_round: 20, max_pages: 120, chunk_chars: 3500, chunks_per_page: 2, fetch_threads: 6, time_limit: Duration::from_secs(120 * 60) },
        }
    }
}

/// What the research is doing now; a screen turns these into steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A round of searching starts (1-based).
    Round(usize),
    Planned(Vec<String>),
    Searching(String),
    Found { query: String, count: usize },
    Reading { url: String, done: usize, total: usize },
    /// Facts taken from a page; `n` is its source number.
    Note { n: usize, title: String, url: String },
    Skipped { url: String, why: String },
    Merging,
    Writing,
}

/// A page an answer relies on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub n: usize,
    pub title: String,
    pub url: String,
}

/// The finished research.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub question: String,
    /// The answer, with [1]-style marks pointing at `sources`.
    pub answer: String,
    pub sources: Vec<Source>,
    pub pages_read: usize,
    pub queries: Vec<String>,
    pub rounds: usize,
    pub stopped: bool,
    pub elapsed: Duration,
}

/// The model, as research needs it: one question in, one answer out.
pub trait Brain {
    fn ask(&self, system: &str, user: &str, stop: &AtomicBool) -> Result<String, String>;
}

/// The web, as research needs it.
pub trait Web: Sync {
    fn search(&self, query: &str, max: usize) -> SearchOutcome;
    fn fetch(&self, url: &str, max_chars: usize) -> Result<FetchedPage, String>;
}

/// The assistant's model behind [`Brain`].
pub struct EngineBrain(pub Arc<dyn ChatEngine>);

impl Brain for EngineBrain {
    fn ask(&self, system: &str, user: &str, stop: &AtomicBool) -> Result<String, String> {
        let messages = [EngineMessage::new(EngineRole::System, system), EngineMessage::new(EngineRole::User, user)];
        let raw = self.0.generate(&messages, &mut |_| !stop.load(Ordering::Relaxed)).map_err(|e| e.user_message())?;
        Ok(crate::ai::llm::strip_thinking(&raw).trim().to_string())
    }
}

/// The real web through [`crate::search`].
pub struct LiveWeb {
    pub language: String,
}

impl Web for LiveWeb {
    fn search(&self, query: &str, max: usize) -> SearchOutcome {
        search::search(query, &SearchOptions { max_results: max, language: self.language.clone(), ..Default::default() })
    }
    fn fetch(&self, url: &str, max_chars: usize) -> Result<FetchedPage, String> {
        search::fetch_page(url, max_chars)
    }
}

// ---------------------------------------------------------------- text helpers

/// Words of `text` for comparing meaning loosely. Languages written without
/// spaces (Chinese, Japanese, Thai, ...) contribute pairs of letters instead.
pub fn terms(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for word in text.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        let chars: Vec<char> = word.chars().collect();
        let unspaced = chars.iter().any(|&c| matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x9FFF | 0xAC00..=0xD7AF | 0x0E00..=0x0E7F | 0x0F00..=0x0FFF | 0x1000..=0x109F | 0x1780..=0x17FF));
        if unspaced && chars.len() > 1 {
            for pair in chars.windows(2) {
                out.insert(pair.iter().collect());
            }
        } else if chars.len() > 1 || !chars[0].is_ascii() {
            out.insert(word.to_string());
        }
    }
    out
}

/// 0 to 1: how many of the question's terms appear in `text`.
pub fn relevance(question_terms: &HashSet<String>, text: &str) -> f32 {
    if question_terms.is_empty() {
        return 0.0;
    }
    let t = terms(text);
    question_terms.iter().filter(|q| t.contains(*q)).count() as f32 / question_terms.len() as f32
}

/// The `count` stretches of `text` (about `size` characters each, cut at
/// paragraph breaks) that best match the question, in page order.
pub fn best_chunks(text: &str, question_terms: &HashSet<String>, size: usize, count: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        if !current.is_empty() && current.chars().count() + para.chars().count() > size {
            chunks.push(std::mem::take(&mut current));
        }
        if para.chars().count() > size {
            // One enormous paragraph: cut it.
            let chars: Vec<char> = para.chars().collect();
            for piece in chars.chunks(size) {
                chunks.push(piece.iter().collect());
            }
            continue;
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(para);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    let mut scored: Vec<(usize, f32)> = chunks.iter().enumerate().map(|(i, c)| (i, relevance(question_terms, c))).collect();
    // Earlier chunks win ties: pages put the point first.
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    let mut keep: Vec<usize> = scored.into_iter().take(count).map(|(i, _)| i).collect();
    keep.sort_unstable();
    keep.into_iter().map(|i| chunks[i].clone()).collect()
}

/// The queries in a model reply such as `{"queries":["a","b"]}`.
pub fn parse_queries(reply: &str, max: usize) -> Vec<String> {
    let Some(start) = reply.find('{') else { return Vec::new() };
    let mut stream = serde_json::Deserializer::from_str(&reply[start..]).into_iter::<Value>();
    let Some(Ok(v)) = stream.next() else { return Vec::new() };
    let mut seen = HashSet::new();
    v["queries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|q| q.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|q| !q.is_empty() && q.chars().count() <= 200 && seen.insert(q.to_lowercase()))
        .take(max)
        .collect()
}

/// The numbers inside [1]-style marks of `text`.
pub fn cited_numbers(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find('[') {
        let after = &rest[i + 1..];
        let Some(j) = after.find(']') else { break };
        for part in after[..j].split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|p| !p.is_empty()) {
            if let Ok(n) = part.parse::<usize>() {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
        }
        rest = &after[j + 1..];
    }
    out
}

// ---------------------------------------------------------------- prompts

pub fn plan_prompt(language: &str, n: usize) -> String {
    format!("You plan web searches. Reply with JSON only: {{\"queries\":[...]}} holding {n} short, different search queries that together find what is needed to answer the question. Use the language the web pages are most likely written in. No explanations.\nThe user writes in {language}.")
}

pub fn gap_prompt(n: usize) -> String {
    format!("You check research notes against a question. If the notes are enough to answer it, reply {{\"queries\":[]}}. If something important is missing, reply JSON only: {{\"queries\":[...]}} with at most {n} short new search queries for what is missing. Do not repeat earlier queries.")
}

pub const EXTRACT_PROMPT: &str = "You read one part of a web page for someone researching a question. Write only the facts from the text that help answer the question, as short plain sentences, at most 120 words. Copy numbers, names and dates exactly. If the text has nothing useful, reply exactly NONE. Do not add anything that is not in the text.";

pub const MERGE_PROMPT: &str = "You merge research notes. Each note starts with a source tag like [3]. Write one shorter set of notes that keeps every distinct fact and keeps the source tags next to the facts they support. Do not invent anything.";

pub fn answer_prompt(language: &str) -> String {
    format!("You answer a question using only the numbered research notes. Every sentence that states a fact must end with the tag of the note it came from, for example: \"Bloom filters never give false negatives [2].\" Use only tags that exist. If the notes disagree, say so. If they do not answer the question, say what is missing. Be clear, correct and well organised. Write the answer in {language}.")
}

/// The user message for pulling facts out of one part of a page.
pub fn extract_user(question: &str, chunk: &str) -> String {
    format!("Question: {question}\n\nText:\n{chunk}")
}

/// The user message for checking what the notes still lack. `notes` are (source number, text).
pub fn gap_user(question: &str, searches: &[String], notes: &[(usize, String)]) -> String {
    let notes_text = notes.iter().map(|(n, t)| format!("[{n}] {t}")).collect::<Vec<_>>().join("\n");
    format!("Question: {question}\nSearches so far: {}\nNotes:\n{notes_text}", searches.join(" | "))
}

/// The user message for merging a group of notes.
pub fn merge_user(notes: &[(usize, String)]) -> String {
    notes.iter().map(|(n, t)| format!("[{n}] {t}")).collect::<Vec<_>>().join("\n")
}

/// The user message for writing the final answer.
pub fn answer_user(question: &str, notes: &[(usize, String)]) -> String {
    format!("Question: {question}\n\nNotes:\n{}", merge_user(notes))
}

// ---------------------------------------------------------------- the run

struct Note {
    n: usize,
    text: String,
}

fn out_of_time(started: Instant, budget: &Budget, stop: &AtomicBool) -> bool {
    stop.load(Ordering::Relaxed) || started.elapsed() >= budget.time_limit
}

/// Researches `question` and writes the answer. `language` is the name of the
/// language to answer in ("English"); `lang_code` picks regional sources ("en").
pub fn run(question: &str, depth: Depth, language: &str, brain: &dyn Brain, web: &dyn Web, stop: &AtomicBool, on: &mut dyn FnMut(Event)) -> Report {
    let started = Instant::now();
    let budget = depth.budget();
    let qterms = terms(question);
    let mut queries_done: Vec<String> = Vec::new();
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut sources: Vec<Source> = Vec::new();
    let mut notes: Vec<Note> = Vec::new();
    let mut pages_read = 0usize;
    let mut rounds_run = 0usize;

    // Round 1 starts from a plan; later rounds from what is missing.
    for round in 1..=budget.rounds {
        if out_of_time(started, &budget, stop) || pages_read >= budget.max_pages {
            break;
        }
        let next_queries: Vec<String>;
        if round == 1 {
            let asked = brain.ask(&plan_prompt(language, budget.queries_per_round), question, stop).unwrap_or_default();
            let mut planned = parse_queries(&asked, budget.queries_per_round);
            // A model that gives nothing usable still gets the question searched.
            if planned.is_empty() {
                planned = vec![question.trim().to_string()];
            }
            next_queries = planned;
        } else {
            let pairs: Vec<(usize, String)> = notes.iter().map(|n| (n.n, n.text.clone())).collect();
            let user = gap_user(question, &queries_done, &pairs);
            let asked = brain.ask(&gap_prompt(budget.queries_per_round), &user, stop).unwrap_or_default();
            let done: HashSet<String> = queries_done.iter().map(|q| q.to_lowercase()).collect();
            next_queries = parse_queries(&asked, budget.queries_per_round).into_iter().filter(|q| !done.contains(&q.to_lowercase())).collect::<Vec<_>>();
            if next_queries.is_empty() {
                break;
            }
        }
        rounds_run = round;
        on(Event::Round(round));
        on(Event::Planned(next_queries.clone()));

        // Search every query, then rank the pages not read yet.
        let mut candidates: Vec<(f32, SearchResult)> = Vec::new();
        for q in &next_queries {
            if out_of_time(started, &budget, stop) {
                break;
            }
            on(Event::Searching(q.clone()));
            let outcome = web.search(q, budget.results_per_query);
            on(Event::Found { query: q.clone(), count: outcome.results.len() });
            queries_done.push(q.clone());
            for (rank, r) in outcome.results.into_iter().enumerate() {
                let key = search::normalize_url(&r.url);
                if seen_urls.contains(&key) || candidates.iter().any(|(_, c)| search::normalize_url(&c.url) == key) {
                    continue;
                }
                // Meaning match first, search position as the tie-breaker.
                // Encyclopedia and ordinary web pages usually explain best; papers are
                // welcome but should not crowd them out.
                let boost = match r.source {
                    "wikipedia" => 0.35,
                    "web" | "bing" => 0.1,
                    _ => 0.0,
                };
                let score = relevance(&qterms, &format!("{} {}", r.title, r.snippet)) + 1.0 / (rank as f32 + 2.0) + boost;
                candidates.push((score, r));
            }
        }
        candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let room = budget.max_pages - pages_read;
        let batch: Vec<SearchResult> = candidates.into_iter().take(budget.pages_per_round.min(room)).map(|(_, r)| r).collect();
        for r in &batch {
            seen_urls.insert(search::normalize_url(&r.url));
        }

        // Fetch a few at a time; the model reads them one by one.
        let total = batch.len();
        for group in batch.chunks(budget.fetch_threads.max(1)) {
            if out_of_time(started, &budget, stop) {
                break;
            }
            let pages: Vec<(SearchResult, Result<FetchedPage, String>)> = std::thread::scope(|scope| {
                let handles: Vec<_> = group.iter().map(|r| scope.spawn(move || (r.clone(), web.fetch(&r.url, 60_000)))).collect();
                handles.into_iter().filter_map(|h| h.join().ok()).collect()
            });
            for (hit, page) in pages {
                if out_of_time(started, &budget, stop) {
                    break;
                }
                pages_read += 1;
                on(Event::Reading { url: hit.url.clone(), done: pages_read, total });
                let page = match page {
                    Ok(p) if !p.text.trim().is_empty() => p,
                    Ok(_) => {
                        on(Event::Skipped { url: hit.url, why: "the page had no text".into() });
                        continue;
                    }
                    Err(why) => {
                        on(Event::Skipped { url: hit.url, why });
                        continue;
                    }
                };
                let mut facts = Vec::new();
                for chunk in best_chunks(&page.text, &qterms, budget.chunk_chars, budget.chunks_per_page) {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let user = extract_user(question, &chunk);
                    match brain.ask(EXTRACT_PROMPT, &user, stop) {
                        Ok(t) if !t.trim().is_empty() && !t.trim().eq_ignore_ascii_case("none") && !t.trim().starts_with("NONE") => facts.push(t.trim().to_string()),
                        _ => {}
                    }
                }
                if facts.is_empty() {
                    on(Event::Skipped { url: hit.url, why: "nothing useful on it".into() });
                    continue;
                }
                let n = sources.len() + 1;
                let title = if page.title.is_empty() { hit.title.clone() } else { page.title.clone() };
                sources.push(Source { n, title: title.clone(), url: page.url.clone() });
                notes.push(Note { n, text: facts.join(" ") });
                on(Event::Note { n, title, url: page.url });
            }
        }
    }

    // Too many notes for the model to read at once: merge them in groups.
    const NOTES_LIMIT_CHARS: usize = 7000;
    while notes.iter().map(|n| n.text.chars().count() + 6).sum::<usize>() > NOTES_LIMIT_CHARS && notes.len() > 1 && !stop.load(Ordering::Relaxed) {
        on(Event::Merging);
        let before = notes.len();
        let mut merged = Vec::new();
        for group in notes.chunks(6) {
            let joined = merge_user(&group.iter().map(|n| (n.n, n.text.clone())).collect::<Vec<_>>());
            let text = if group.len() == 1 { joined } else { brain.ask(MERGE_PROMPT, &joined, stop).ok().filter(|t| !t.trim().is_empty()).unwrap_or(joined) };
            merged.push(Note { n: group[0].n, text });
        }
        notes = merged;
        // A merge that did not shrink anything would never end.
        if notes.len() >= before {
            let keep = notes.iter().map(|n| n.text.chars().count() + 6).scan(0usize, |t, c| { *t += c; Some(*t) }).take_while(|t| *t <= NOTES_LIMIT_CHARS).count().max(1);
            notes.truncate(keep);
            break;
        }
    }

    on(Event::Writing);
    let stopped = stop.load(Ordering::Relaxed);
    let answer = if notes.is_empty() {
        String::new()
    } else {
        let user = answer_user(question, &notes.iter().map(|n| (n.n, n.text.clone())).collect::<Vec<_>>());
        // The answer is still written after a stop: the notes are already paid for.
        let never = AtomicBool::new(false);
        brain.ask(&answer_prompt(language), &user, &never).unwrap_or_default()
    };

    // Keep only the sources the answer points at (all, if it pointed at none).
    let cited = cited_numbers(&answer);
    let mut shown: Vec<Source> = sources.iter().filter(|s| cited.contains(&s.n)).cloned().collect();
    if shown.is_empty() {
        shown = sources.iter().take(10).cloned().collect();
    }
    Report { question: question.to_string(), answer, sources: shown, pages_read, queries: queries_done, rounds: rounds_run, stopped, elapsed: started.elapsed() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Answers each kind of request from a fixed script.
    struct FakeBrain {
        seen: Mutex<Vec<String>>,
        gaps: Mutex<Vec<String>>,
    }
    impl FakeBrain {
        fn new(gaps: &[&str]) -> Self {
            Self { seen: Mutex::new(Vec::new()), gaps: Mutex::new(gaps.iter().map(|s| s.to_string()).collect()) }
        }
    }
    impl Brain for FakeBrain {
        fn ask(&self, system: &str, user: &str, _stop: &AtomicBool) -> Result<String, String> {
            self.seen.lock().unwrap().push(system.chars().take(24).collect());
            if system.starts_with("You plan web searches") {
                Ok(r#"{"queries":["alpha topic","beta topic"]}"#.into())
            } else if system.starts_with("You check research notes") {
                let mut g = self.gaps.lock().unwrap();
                Ok(if g.is_empty() { r#"{"queries":[]}"#.into() } else { g.remove(0) })
            } else if system.starts_with("You read one part") {
                Ok(if user.contains("USELESS") { "NONE".into() } else { "The fact is 42.".into() })
            } else if system.starts_with("You merge research notes") {
                Ok(user.lines().map(|l| l.chars().take(30).collect::<String>()).collect::<Vec<_>>().join(" "))
            } else {
                Ok("It is 42 [1][2].".into())
            }
        }
    }

    struct FakeWeb {
        pages: HashMap<String, Result<String, String>>,
        searches: Mutex<Vec<String>>,
    }
    impl FakeWeb {
        fn new() -> Self {
            let mut pages = HashMap::new();
            pages.insert("https://a.test/1".into(), Ok("The meaning of life is 42.\n\nMore about the topic.".into()));
            pages.insert("https://a.test/2".into(), Ok("Another page: the answer to the topic is 42.".into()));
            pages.insert("https://a.test/3".into(), Ok("USELESS page about nothing.".into()));
            pages.insert("https://a.test/4".into(), Err("the site answered with error 500".into()));
            Self { pages, searches: Mutex::new(Vec::new()) }
        }
    }
    impl Web for FakeWeb {
        fn search(&self, query: &str, _max: usize) -> SearchOutcome {
            self.searches.lock().unwrap().push(query.to_string());
            let urls: &[&str] = if query.starts_with("alpha") { &["https://a.test/1", "https://a.test/3"] } else if query.starts_with("beta") { &["https://a.test/2", "https://a.test/4", "https://a.test/1/"] } else { &["https://a.test/9"] };
            SearchOutcome {
                results: urls.iter().map(|u| SearchResult { title: format!("Title {u}"), url: u.to_string(), snippet: "topic".into(), source: "web" }).collect(),
                problems: Vec::new(),
            }
        }
        fn fetch(&self, url: &str, _max: usize) -> Result<FetchedPage, String> {
            match self.pages.get(url) {
                Some(Ok(t)) => Ok(FetchedPage { url: url.to_string(), title: String::new(), text: t.clone(), truncated: false }),
                Some(Err(e)) => Err(e.clone()),
                None => Err("not found".into()),
            }
        }
    }

    #[test]
    fn replies_are_read_loosely() {
        assert_eq!(parse_queries("Sure! {\"queries\":[\"a  b\",\"A B\",\"\",\"c\"]} thanks", 5), vec!["a b".to_string(), "c".to_string()]);
        assert!(parse_queries("no json", 3).is_empty());
        assert_eq!(parse_queries(r#"{"queries":["1","2","3"]}"#, 2).len(), 2);
        assert_eq!(cited_numbers("A [1] and [2, 3]; again [1], [x] [4"), vec![1, 2, 3]);
    }

    #[test]
    fn meaning_match_handles_unspaced_scripts() {
        let q = terms("What is photosynthesis?");
        assert!(relevance(&q, "Photosynthesis is how plants make food") > relevance(&q, "Cats sleep a lot"));
        let zh = terms("光合作用是什么");
        assert!(relevance(&zh, "光合作用是植物制造养分的过程") > 0.5);
    }

    #[test]
    fn the_best_stretches_of_a_page_are_chosen_in_order() {
        let text = "Intro about cooking.\n\nLong filler about weather and sport and travel.\n\nThe moon orbit and moon phases explained.\n\nMore moon facts here.";
        let q = terms("moon");
        let picked = best_chunks(text, &q, 60, 2);
        assert_eq!(picked.len(), 2);
        assert!(picked.iter().all(|c| c.contains("moon")));
        let long = "x".repeat(10_000);
        assert!(best_chunks(&long, &q, 3500, 1)[0].chars().count() <= 3500);
    }

    #[test]
    fn a_quick_run_reads_pages_and_cites_them() {
        let (brain, web, stop) = (FakeBrain::new(&[]), FakeWeb::new(), AtomicBool::new(false));
        let mut events = Vec::new();
        let r = run("what is the topic answer", Depth::Quick, "English", &brain, &web, &stop, &mut |e| events.push(e));
        assert_eq!(r.queries, vec!["alpha topic".to_string(), "beta topic".to_string()]);
        assert_eq!(r.rounds, 1);
        // /1 and /1/ are one page; /3 has nothing, /4 fails.
        assert_eq!(r.pages_read, 4);
        assert_eq!(r.sources.len(), 2);
        assert!(r.answer.contains("[1]") && r.answer.contains("[2]"));
        assert!(events.iter().any(|e| matches!(e, Event::Skipped { why, .. } if why.contains("nothing useful"))));
        assert!(events.iter().any(|e| matches!(e, Event::Skipped { why, .. } if why.contains("500"))));
        assert!(events.contains(&Event::Writing));
        assert!(!r.stopped);
    }

    #[test]
    fn later_rounds_search_what_is_missing_and_stop_when_nothing_is() {
        let (brain, web, stop) = (FakeBrain::new(&[r#"{"queries":["gamma topic","alpha topic"]}"#]), FakeWeb::new(), AtomicBool::new(false));
        let r = run("topic", Depth::Standard, "English", &brain, &web, &stop, &mut |_| {});
        assert_eq!(r.rounds, 2, "round 3 found nothing missing");
        assert!(r.queries.contains(&"gamma topic".to_string()));
        assert_eq!(r.queries.iter().filter(|q| q.as_str() == "alpha topic").count(), 1, "a repeated query is not run again");
    }

    #[test]
    fn a_stop_still_gives_an_answer_from_what_was_read() {
        let (brain, web, stop) = (FakeBrain::new(&[]), FakeWeb::new(), AtomicBool::new(false));
        let mut seen = 0;
        let r = {
            let stop_ref = &stop;
            run("topic", Depth::Deep, "English", &brain, &web, stop_ref, &mut |e| {
                if matches!(e, Event::Note { .. }) {
                    seen += 1;
                    stop_ref.store(true, Ordering::Relaxed);
                }
            })
        };
        assert!(r.stopped);
        assert_eq!(seen, 1);
        assert_eq!(r.sources.len(), 1);
        assert!(!r.answer.is_empty());
    }

    #[test]
    fn no_pages_means_no_invented_answer() {
        struct Empty;
        impl Web for Empty {
            fn search(&self, _: &str, _: usize) -> SearchOutcome {
                SearchOutcome::default()
            }
            fn fetch(&self, _: &str, _: usize) -> Result<FetchedPage, String> {
                Err("no".into())
            }
        }
        let r = run("anything", Depth::Quick, "English", &FakeBrain::new(&[]), &Empty, &AtomicBool::new(false), &mut |_| {});
        assert!(r.answer.is_empty() && r.sources.is_empty());
    }

    #[test]
    fn many_notes_are_merged_before_the_answer() {
        struct Wide;
        impl Web for Wide {
            fn search(&self, q: &str, _: usize) -> SearchOutcome {
                SearchOutcome { results: (0..8).map(|i| SearchResult { title: format!("T{i}"), url: format!("https://w.test/{q}/{i}"), snippet: "topic".into(), source: "web" }).collect(), problems: Vec::new() }
            }
            fn fetch(&self, url: &str, _: usize) -> Result<FetchedPage, String> {
                Ok(FetchedPage { url: url.to_string(), title: String::new(), text: "topic text".into(), truncated: false })
            }
        }
        struct Chatty;
        impl Brain for Chatty {
            fn ask(&self, system: &str, user: &str, _: &AtomicBool) -> Result<String, String> {
                Ok(if system.starts_with("You plan") { r#"{"queries":["q1","q2","q3"]}"#.into() } else if system.starts_with("You check") { r#"{"queries":[]}"#.into() } else if system.starts_with("You read") { "word ".repeat(160) } else if system.starts_with("You merge") { user.chars().take(300).collect() } else { "Done [1].".into() })
            }
        }
        let mut merged = false;
        let r = run("topic", Depth::Standard, "English", &Chatty, &Wide, &AtomicBool::new(false), &mut |e| merged |= e == Event::Merging);
        assert!(merged, "10 notes of 800 characters do not fit one prompt");
        assert_eq!(r.answer, "Done [1].");
    }
}
