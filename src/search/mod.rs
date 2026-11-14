//! Web search for the assistant, written for this app.
//!
//! Several sources are asked at once (general web pages, Wikipedia, papers,
//! forums), their answers are merged into one ranked list, and any page can
//! be fetched as readable text. Nothing here runs unless the person has
//! turned web search on; every request goes straight from this device to the
//! site, with no server of ours in between.

pub mod fetch;
pub mod providers;
pub mod text;

use std::collections::HashMap;
use std::time::Duration;

pub use fetch::{fetch_page, FetchedPage};
pub use providers::ProviderKind;

/// One hit from a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// Which source found it (several may have; this is the best-ranked one).
    pub source: &'static str,
}

/// What a search should do.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Most results returned after merging.
    pub max_results: usize,
    /// Sources to ask; empty means all of them.
    pub sources: Vec<ProviderKind>,
    /// Language code for sources that depend on it (Wikipedia), such as "en".
    pub language: String,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self { max_results: 10, sources: Vec::new(), language: "en".to_string() }
    }
}

/// The merged list, and why any source gave nothing.
#[derive(Debug, Clone, Default)]
pub struct SearchOutcome {
    pub results: Vec<SearchResult>,
    pub problems: Vec<(ProviderKind, String)>,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .redirects(3)
        .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:139.0) Gecko/20100101 Firefox/139.0")
        .build()
}

/// Asks one source and reads its answer.
pub fn search_one(kind: ProviderKind, query: &str, n: usize, language: &str) -> Result<Vec<SearchResult>, String> {
    let Some(url) = kind.url(query, n, language) else { return Ok(Vec::new()) };
    let mut req = agent().get(&url).set("Accept", "text/html,application/json,application/atom+xml;q=0.9,*/*;q=0.5").set("Accept-Language", "en-US,en;q=0.8");
    for (k, v) in kind.headers(query) {
        req = req.set(k, &v);
    }
    let body = match req.call() {
        Ok(r) => r.into_string().map_err(|e| format!("the answer could not be read: {e}"))?,
        Err(ureq::Error::Status(code, _)) => return Err(format!("answered with error {code}")),
        Err(e) => return Err(format!("could not be reached: {e}")),
    };
    Ok(kind.parse(&body, language))
}

/// A web address with the parts that do not matter removed, to spot duplicates.
pub fn normalize_url(url: &str) -> String {
    let mut u = url.trim().to_ascii_lowercase();
    for p in ["https://", "http://"] {
        if let Some(rest) = u.strip_prefix(p) {
            u = rest.to_string();
        }
    }
    if let Some(rest) = u.strip_prefix("www.") {
        u = rest.to_string();
    }
    let u = u.split('#').next().unwrap_or("").to_string();
    let (path, query) = match u.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (u.clone(), String::new()),
    };
    let kept: Vec<&str> = query.split('&').filter(|kv| !kv.starts_with("utm_") && !kv.starts_with("fbclid") && !kv.is_empty()).collect();
    let path = path.trim_end_matches('/').to_string();
    if kept.is_empty() { path } else { format!("{path}?{}", kept.join("&")) }
}

/// Merges ranked lists from several sources. A result's score is the sum of
/// 1/(rank+1) over the sources that found it, so a page two sources agree on
/// beats a page only one found. Duplicates (same address) are joined.
pub fn merge(lists: Vec<Vec<SearchResult>>, max_results: usize) -> Vec<SearchResult> {
    let mut by_url: HashMap<String, (f32, usize, SearchResult)> = HashMap::new();
    let mut order = 0usize;
    for list in lists {
        for (rank, r) in list.into_iter().enumerate() {
            let key = normalize_url(&r.url);
            let gain = 1.0 / (rank as f32 + 1.0);
            by_url
                .entry(key)
                .and_modify(|(score, _, kept)| {
                    *score += gain;
                    if kept.snippet.is_empty() && !r.snippet.is_empty() {
                        kept.snippet = r.snippet.clone();
                    }
                })
                .or_insert_with(|| {
                    order += 1;
                    (gain, order, r)
                });
        }
    }
    let mut all: Vec<(f32, usize, SearchResult)> = by_url.into_values().collect();
    all.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    all.into_iter().take(max_results).map(|(_, _, r)| r).collect()
}

/// Searches the chosen sources at the same time and merges the answers.
pub fn search(query: &str, options: &SearchOptions) -> SearchOutcome {
    let query = query.trim();
    if query.is_empty() {
        return SearchOutcome::default();
    }
    let kinds: Vec<ProviderKind> = if options.sources.is_empty() { ProviderKind::ALL.to_vec() } else { options.sources.clone() };
    // Ask for a few more than needed from each, since merging drops duplicates.
    let per_source = (options.max_results + 4).min(20);
    let answers: Vec<(ProviderKind, Result<Vec<SearchResult>, String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = kinds.iter().map(|&k| scope.spawn(move || (k, search_one(k, query, per_source, &options.language)))).collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    let mut outcome = SearchOutcome::default();
    let mut lists = Vec::new();
    for (kind, answer) in answers {
        match answer {
            Ok(list) if !list.is_empty() => lists.push(list),
            Ok(_) => {}
            Err(why) => outcome.problems.push((kind, why)),
        }
    }
    outcome.results = merge(lists, options.max_results);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(url: &str, source: &'static str) -> SearchResult {
        SearchResult { title: url.to_string(), url: url.to_string(), snippet: String::new(), source }
    }

    #[test]
    fn addresses_are_compared_without_noise() {
        assert_eq!(normalize_url("https://www.Example.com/a/?utm_source=x&id=2#top"), "example.com/a?id=2");
        assert_eq!(normalize_url("http://example.com/a"), normalize_url("https://www.example.com/a/"));
        assert_ne!(normalize_url("https://example.com/a?id=1"), normalize_url("https://example.com/a?id=2"));
    }

    #[test]
    fn agreement_between_sources_wins() {
        let a = vec![hit("https://one.test", "web"), hit("https://shared.test", "web")];
        let b = vec![hit("https://shared.test/", "bing"), hit("https://two.test", "bing")];
        let merged = merge(vec![a, b], 10);
        assert_eq!(merged[0].url, "https://shared.test");
        assert_eq!(merged.len(), 3, "the shared page is joined, not repeated");
        assert_eq!(merge(vec![vec![hit("https://x.test", "web")]], 0).len(), 0);
    }

    #[test]
    fn an_empty_query_searches_nothing() {
        assert!(search("   ", &SearchOptions::default()).results.is_empty());
    }
}
