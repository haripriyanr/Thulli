//! Small text helpers for web pages: percent-encoding, entity decoding and a
//! readable-text extractor. No outside crates; written for this app.

use regex::Regex;
use std::sync::OnceLock;

/// Percent-encodes `s` for a query string.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decodes `%XX` escapes and `+` in a query value.
pub fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                if let Some(v) = hex {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Replaces the common HTML entities and numeric references.
pub fn decode_entities(s: &str) -> String {
    static NUM: OnceLock<Regex> = OnceLock::new();
    let num = NUM.get_or_init(|| Regex::new(r"&#(x[0-9a-fA-F]+|[0-9]+);").unwrap());
    let replaced = num.replace_all(s, |c: &regex::Captures| {
        let raw = &c[1];
        let code = if let Some(hex) = raw.strip_prefix('x') { u32::from_str_radix(hex, 16).ok() } else { raw.parse().ok() };
        code.and_then(char::from_u32).map(String::from).unwrap_or_default()
    });
    replaced
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&ndash;", "-")
        .replace("&mdash;", "-")
        .replace("&hellip;", "...")
        .replace("&amp;", "&")
}

/// Removes tags from a snippet and tidies the spaces.
pub fn strip_tags(s: &str) -> String {
    static TAG: OnceLock<Regex> = OnceLock::new();
    let tag = TAG.get_or_init(|| Regex::new(r"(?s)<[^>]*>").unwrap());
    collapse_spaces(&decode_entities(&tag.replace_all(s, "")))
}

/// Turns runs of white space into single spaces.
pub fn collapse_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The title and readable text of an HTML page. Scripts, styles and page
/// furniture are dropped; `<article>` or `<main>` is preferred when present.
/// Paragraph breaks are kept as blank lines.
pub fn html_to_text(html: &str) -> (String, String) {
    static TITLE: OnceLock<Regex> = OnceLock::new();
    static JUNK: OnceLock<Vec<Regex>> = OnceLock::new();
    static MAIN: OnceLock<Vec<Regex>> = OnceLock::new();
    static BREAK: OnceLock<Regex> = OnceLock::new();
    static COMMENT: OnceLock<Regex> = OnceLock::new();
    let title_re = TITLE.get_or_init(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
    // One pattern per tag: Rust's regex has no back-references.
    let junk = JUNK.get_or_init(|| {
        ["script", "style", "noscript", "svg", "nav", "footer", "header", "aside", "form", "iframe", "template"]
            .iter()
            .map(|t| Regex::new(&format!(r"(?is)<{t}\b.*?</{t}\s*>")).unwrap())
            .collect()
    });
    let mains = MAIN.get_or_init(|| ["article", "main"].iter().map(|t| Regex::new(&format!(r"(?is)<{t}\b[^>]*>(.*)</{t}\s*>")).unwrap()).collect());
    let brk = BREAK.get_or_init(|| Regex::new(r"(?i)</?(p|div|br|li|ul|ol|h[1-6]|tr|table|section|blockquote|pre)\b[^>]*>").unwrap());
    let comment = COMMENT.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").unwrap());

    let title = title_re.captures(html).map(|c| strip_tags(&c[1])).unwrap_or_default();
    let cleaned = comment.replace_all(html, "");
    let mut cleaned = cleaned.to_string();
    for re in junk {
        cleaned = re.replace_all(&cleaned, " ").into_owned();
    }
    let body = mains.iter().find_map(|re| re.captures(&cleaned).map(|c| c[1].to_string())).unwrap_or(cleaned);
    let with_breaks = brk.replace_all(&body, "\n");
    let text = decode_entities(&regex::Regex::new(r"(?s)<[^>]*>").unwrap().replace_all(&with_breaks, ""));
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = collapse_spaces(line);
        if line.is_empty() {
            if lines.last().is_some_and(|l| !l.is_empty()) {
                lines.push(String::new());
            }
        } else {
            lines.push(line);
        }
    }
    (title, lines.join("\n").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_round_trip() {
        let q = "rust & café 100%";
        assert_eq!(url_decode(&url_encode(q)), q);
        assert_eq!(url_encode("a b"), "a+b");
        assert_eq!(url_decode("a%2Fb%3Fc"), "a/b?c");
        assert_eq!(url_decode("100%"), "100%");
    }

    #[test]
    fn entities_decode() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt; &#65;&#x42; &quot;q&quot;"), "a & b <c> AB \"q\"");
        assert_eq!(strip_tags("<span class=\"x\">Hello</span>, <b>world</b>&nbsp;!"), "Hello, world !");
    }

    #[test]
    fn pages_become_readable_text() {
        let html = r#"<html><head><title>My &amp; Page</title><style>p{color:red}</style></head>
            <body><nav>Home | About</nav><header>Site</header>
            <article><h1>Heading</h1><p>First   paragraph.</p><p>Second<br>line.</p>
            <script>alert(1)</script></article><footer>(c)</footer></body></html>"#;
        let (title, text) = html_to_text(html);
        assert_eq!(title, "My & Page");
        assert!(text.contains("Heading"));
        assert!(text.contains("First paragraph."));
        assert!(text.contains("Second\nline."));
        assert!(!text.contains("alert") && !text.contains("Home") && !text.contains("(c)") && !text.contains("color"));
    }

    #[test]
    fn pages_without_article_still_give_text() {
        let (_, text) = html_to_text("<body><div>One</div><div>Two</div></body>");
        assert_eq!(text, "One\n\nTwo");
    }
}
