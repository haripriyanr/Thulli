//! The places the assistant can search. Each provider turns a query into a
//! request and its reply into [`SearchResult`]s. The parsers are plain
//! functions of the reply text so they can be tested without a network.
//!
//! Sources: Wikipedia, arXiv, OpenAlex, Crossref and Hacker News through their
//! public APIs, and DuckDuckGo's plain HTML page for general web results.

use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;
use std::sync::OnceLock;

use super::text::{collapse_spaces, decode_entities, strip_tags, url_decode, url_encode};
use super::SearchResult;

/// A search source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    /// DuckDuckGo's plain page.
    Web,
    Bing,
    Wikipedia,
    Arxiv,
    OpenAlex,
    Crossref,
    HackerNews,
}

impl ProviderKind {
    /// Every source, general web first.
    pub const ALL: [ProviderKind; 7] = [
        ProviderKind::Web,
        ProviderKind::Bing,
        ProviderKind::Wikipedia,
        ProviderKind::Arxiv,
        ProviderKind::OpenAlex,
        ProviderKind::Crossref,
        ProviderKind::HackerNews,
    ];

    /// Short name stored on each result.
    pub fn name(self) -> &'static str {
        match self {
            ProviderKind::Web => "web",
            ProviderKind::Bing => "bing",
            ProviderKind::Wikipedia => "wikipedia",
            ProviderKind::Arxiv => "arxiv",
            ProviderKind::OpenAlex => "openalex",
            ProviderKind::Crossref => "crossref",
            ProviderKind::HackerNews => "hackernews",
        }
    }

    /// The request address for `query`, asking for about `n` results.
    pub fn url(self, query: &str, n: usize, language: &str) -> Option<String> {
        let q = url_encode(query);
        let lang = if language.len() >= 2 && language.chars().all(|c| c.is_ascii_alphabetic()) { language.to_ascii_lowercase() } else { "en".to_string() };
        Some(match self {
            ProviderKind::Web => format!("https://html.duckduckgo.com/html/?q={q}"),
            ProviderKind::Bing => {
                let cvid = bing_cvid(query);
                format!("https://www.bing.com/search?q={q}&pq={q}&cvid={cvid}&filters=rcrse%3A%221%22&FORM=PERE&ghc=1&lq=0&qs=n&sk=&sp=-1")
            }
            ProviderKind::Wikipedia => {
                format!("https://{lang}.wikipedia.org/w/api.php?action=query&list=search&srsearch={q}&format=json&utf8=1&srlimit={n}")
            }
            ProviderKind::Arxiv => format!("https://export.arxiv.org/api/query?search_query=all:{q}&max_results={n}"),
            ProviderKind::OpenAlex => format!("https://api.openalex.org/works?search={q}&per-page={n}"),
            ProviderKind::Crossref => format!("https://api.crossref.org/works?query={q}&rows={n}"),
            ProviderKind::HackerNews => format!("https://hn.algolia.com/api/v1/search?query={q}&hitsPerPage={n}"),
        })
    }

    /// Extra request headers some sources need.
    pub fn headers(self, query: &str) -> Vec<(&'static str, String)> {
        match self {
            ProviderKind::Bing => vec![("Cookie", format!("SRCHHPGUSR=IG={}", bing_cvid(query)))],
            _ => Vec::new(),
        }
    }

    /// Reads the reply body into results. `language` only matters for Wikipedia links.
    pub fn parse(self, body: &str, language: &str) -> Vec<SearchResult> {
        match self {
            ProviderKind::Web => parse_duckduckgo(body),
            ProviderKind::Bing => parse_bing(body),
            ProviderKind::Wikipedia => parse_wikipedia(body, language),
            ProviderKind::Arxiv => parse_arxiv(body),
            ProviderKind::OpenAlex => parse_openalex(body),
            ProviderKind::Crossref => parse_crossref(body),
            ProviderKind::HackerNews => parse_hackernews(body),
        }
    }
}

fn result(title: String, url: String, snippet: String, source: &'static str) -> Option<SearchResult> {
    let title = collapse_spaces(&title);
    if title.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    Some(SearchResult { title, url, snippet: collapse_spaces(&snippet), source })
}

pub fn parse_wikipedia(body: &str, language: &str) -> Vec<SearchResult> {
    let lang = if language.len() >= 2 && language.chars().all(|c| c.is_ascii_alphabetic()) { language.to_ascii_lowercase() } else { "en".to_string() };
    let Ok(v) = serde_json::from_str::<Value>(body) else { return Vec::new() };
    v["query"]["search"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let title = h["title"].as_str()?.to_string();
            let url = format!("https://{lang}.wikipedia.org/wiki/{}", url_encode(&title.replace(' ', "_")).replace('+', "_"));
            result(title, url, strip_tags(h["snippet"].as_str().unwrap_or("")), "wikipedia")
        })
        .collect()
}

pub fn parse_arxiv(body: &str) -> Vec<SearchResult> {
    static ENTRY: OnceLock<Regex> = OnceLock::new();
    let entry = ENTRY.get_or_init(|| Regex::new(r"(?s)<entry>(.*?)</entry>").unwrap());
    let field = |tag: &str, text: &str| -> String {
        let re = Regex::new(&format!(r"(?s)<{tag}[^>]*>(.*?)</{tag}>")).unwrap();
        re.captures(text).map(|c| decode_entities(&c[1])).unwrap_or_default()
    };
    entry
        .captures_iter(body)
        .filter_map(|c| {
            let e = &c[1];
            let id = field("id", e);
            let url = id.trim().replace("http://", "https://");
            result(field("title", e), url, field("summary", e), "arxiv")
        })
        .collect()
}

pub fn parse_openalex(body: &str) -> Vec<SearchResult> {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return Vec::new() };
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let title = w["display_name"].as_str()?.to_string();
            let url = w["primary_location"]["landing_page_url"]
                .as_str()
                .or_else(|| w["doi"].as_str())
                .or_else(|| w["id"].as_str())?
                .to_string();
            let year = w["publication_year"].as_i64().map(|y| y.to_string()).unwrap_or_default();
            let venue = w["primary_location"]["source"]["display_name"].as_str().unwrap_or("");
            let cited = w["cited_by_count"].as_i64().unwrap_or(0);
            let snippet = format!("{venue} {year}, cited {cited} times").trim().to_string();
            result(title, url, snippet, "openalex")
        })
        .collect()
}

pub fn parse_crossref(body: &str) -> Vec<SearchResult> {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return Vec::new() };
    v["message"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let title = w["title"].get(0)?.as_str()?.to_string();
            let url = w["URL"].as_str()?.to_string();
            let venue = w["container-title"].get(0).and_then(Value::as_str).unwrap_or("");
            let year = w["issued"]["date-parts"][0][0].as_i64().map(|y| y.to_string()).unwrap_or_default();
            let abstract_text = strip_tags(w["abstract"].as_str().unwrap_or(""));
            let snippet = if abstract_text.is_empty() { format!("{venue} {year}").trim().to_string() } else { abstract_text };
            result(title, url, snippet, "crossref")
        })
        .collect()
}

pub fn parse_hackernews(body: &str) -> Vec<SearchResult> {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return Vec::new() };
    v["hits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let title = h["title"].as_str().or_else(|| h["story_title"].as_str())?.to_string();
            let id = h["objectID"].as_str()?;
            let url = h["url"].as_str().map(str::to_string).unwrap_or_else(|| format!("https://news.ycombinator.com/item?id={id}"));
            let snippet = format!("{} points, {} comments", h["points"].as_i64().unwrap_or(0), h["num_comments"].as_i64().unwrap_or(0));
            result(title, url, snippet, "hackernews")
        })
        .collect()
}

/// DuckDuckGo's HTML results page: each hit is a `result__a` link (its real
/// address is in the `uddg` parameter) followed by a `result__snippet`.
pub fn parse_duckduckgo(body: &str) -> Vec<SearchResult> {
    static LINK: OnceLock<Regex> = OnceLock::new();
    static SNIPPET: OnceLock<Regex> = OnceLock::new();
    let link = LINK.get_or_init(|| Regex::new(r#"(?s)<a[^>]*class="[^"]*result__a[^"]*"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap());
    let snippet = SNIPPET.get_or_init(|| Regex::new(r#"(?s)<a[^>]*class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</a>"#).unwrap());
    let snippets: Vec<(usize, String)> = snippet.captures_iter(body).map(|c| (c.get(0).unwrap().start(), strip_tags(&c[1]))).collect();
    link.captures_iter(body)
        .filter_map(|c| {
            let start = c.get(0).unwrap().start();
            let href = decode_entities(&c[1]);
            let url = match href.find("uddg=") {
                Some(i) => url_decode(href[i + 5..].split('&').next().unwrap_or("")),
                None if href.starts_with("//") => format!("https:{href}"),
                None => href,
            };
            // The first snippet after this link belongs to it.
            let snip = snippets.iter().find(|(pos, _)| *pos > start).map(|(_, s)| s.clone()).unwrap_or_default();
            // Skip DuckDuckGo's own ads.
            if url.contains("duckduckgo.com/y.js") {
                return None;
            }
            result(strip_tags(&c[2]), url, snip, "web")
        })
        .collect()
}

/// A search page described by CSS selectors: one `result` element per hit,
/// then the title, link and description inside it. The Bing recipe
/// follows the open metasearch2 project (CC0).
fn parse_with_selectors(body: &str, source: &'static str, result_sel: &str, title: &str, href: &str, desc: &str, clean: fn(&str) -> String) -> Vec<SearchResult> {
    let doc = Html::parse_document(body);
    let (Ok(r), Ok(t), Ok(h), Ok(d)) = (Selector::parse(result_sel), Selector::parse(title), Selector::parse(href), Selector::parse(desc)) else {
        return Vec::new();
    };
    doc.select(&r)
        .filter_map(|item| {
            let title = item.select(&t).next()?.text().collect::<String>();
            let link = item.select(&h).next()?.value().attr("href")?;
            let snippet = item.select(&d).map(|e| e.text().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(" ");
            result(title, clean(link), snippet, source)
        })
        .collect()
}

pub fn parse_bing(body: &str) -> Vec<SearchResult> {
    parse_with_selectors(body, "bing", "#b_results > li.b_algo", "h2 > a", "a[href]", ".b_caption > p, p.b_algoSlug, .b_caption .ipText", bing_unwrap)
}

/// Bing wraps links in `bing.com/ck/a?...&u=a1<base64>`; this finds the real address.
pub fn bing_unwrap(url: &str) -> String {
    let Some(rest) = url.strip_prefix("https://www.bing.com/ck/a?") else { return url.to_string() };
    let Some(u) = rest.split('&').find_map(|p| p.strip_prefix("u=")) else { return url.to_string() };
    let Some(b64) = u.get(2..) else { return String::new() };
    let mut bits = 0u32;
    let mut n = 0;
    let mut out = Vec::new();
    for c in b64.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => continue,
        };
        bits = (bits << 6) | v as u32;
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((bits >> n) as u8);
            bits &= (1 << n) - 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A repeatable 32-character id Bing asks for with each search.
fn bing_cvid(query: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut out = String::new();
    for round in 0..2u8 {
        for b in query.bytes().chain(std::iter::once(round)) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        out.push_str(&format!("{h:016X}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wikipedia_hits_become_links() {
        let body = r#"{"query":{"search":[{"title":"Rust (programming language)","snippet":"A <span class=\"searchmatch\">language</span> &amp; more"}]}}"#;
        let r = parse_wikipedia(body, "en");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].url, "https://en.wikipedia.org/wiki/Rust_%28programming_language%29");
        assert_eq!(r[0].snippet, "A language & more");
        assert_eq!(r[0].source, "wikipedia");
        assert!(parse_wikipedia("not json", "en").is_empty());
    }

    #[test]
    fn arxiv_entries_are_read() {
        let body = "<feed><entry><id>http://arxiv.org/abs/1234.5678v1</id><title>A  Study\nof Things</title><summary>We study &amp; test.</summary></entry></feed>";
        let r = parse_arxiv(body);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].url, "https://arxiv.org/abs/1234.5678v1");
        assert_eq!(r[0].title, "A Study of Things");
        assert_eq!(r[0].snippet, "We study & test.");
    }

    #[test]
    fn openalex_crossref_and_hn_are_read() {
        let oa = r#"{"results":[{"display_name":"Paper","doi":"https://doi.org/10.1/x","publication_year":2020,"cited_by_count":7,"primary_location":{"landing_page_url":null,"source":{"display_name":"Journal"}}}]}"#;
        let r = parse_openalex(oa);
        assert_eq!((r[0].url.as_str(), r[0].snippet.as_str()), ("https://doi.org/10.1/x", "Journal 2020, cited 7 times"));
        let cr = r#"{"message":{"items":[{"title":["Work"],"URL":"https://doi.org/10.2/y","container-title":["Venue"],"issued":{"date-parts":[[2019]]}}]}}"#;
        assert_eq!(parse_crossref(cr)[0].snippet, "Venue 2019");
        let hn = r#"{"hits":[{"title":"Show HN","objectID":"42","points":10,"num_comments":3},{"title":"Link","objectID":"43","url":"https://x.test/a"}]}"#;
        let r = parse_hackernews(hn);
        assert_eq!(r[0].url, "https://news.ycombinator.com/item?id=42");
        assert_eq!(r[1].url, "https://x.test/a");
    }

    #[test]
    fn duckduckgo_links_are_unwrapped_and_ads_dropped() {
        let body = r#"
            <div class="result"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage%3Fa%3D1&amp;rut=abc">Example <b>Page</b></a>
            <a class="result__snippet" href="x">A short &amp; useful <b>snippet</b>.</a></div>
            <div class="result"><a class="result__a" href="https://duckduckgo.com/y.js?ad=1">Ad</a></div>
            <div class="result"><a class="result__a" href="https://direct.test/x">Direct</a></div>"#;
        let r = parse_duckduckgo(body);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].url, "https://example.com/page?a=1");
        assert_eq!(r[0].title, "Example Page");
        assert_eq!(r[0].snippet, "A short & useful snippet.");
        assert_eq!(r[1].url, "https://direct.test/x");
    }

    #[test]
    fn request_addresses_are_encoded() {
        assert!(ProviderKind::Wikipedia.url("a b&c", 5, "de").unwrap().starts_with("https://de.wikipedia.org/w/api.php?"));
        assert!(ProviderKind::Wikipedia.url("x", 5, "zh-CN").unwrap().contains("https://en.wikipedia.org"));
        assert!(ProviderKind::Arxiv.url("quantum computing", 3, "en").unwrap().contains("all:quantum+computing"));
    }

    #[test]
    fn selector_recipes_read_pages() {
        let bing = r#"<ol id="b_results"><li class="b_algo"><h2><a href="https://www.bing.com/ck/a?!&&p=1&u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS9wYWdl&ntb=1">Bing hit</a></h2><div class="b_caption"><p>Caption text</p></div></li></ol>"#;
        let r = parse_bing(bing);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].url, "https://example.com/page");
        assert_eq!(r[0].snippet, "Caption text");
    }

    #[test]
    fn bing_ids_are_repeatable() {
        assert_eq!(bing_cvid("x").len(), 32);
        assert_eq!(bing_cvid("x"), bing_cvid("x"));
    }
}
