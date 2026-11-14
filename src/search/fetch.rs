//! Fetching a web page for the assistant to read. Careful by design: only
//! http and https, never a private or local address (checked again on every
//! redirect), the site's robots.txt is respected, and the size is capped.

use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use super::text::html_to_text;

/// Most bytes read from one page.
pub const MAX_BODY_BYTES: u64 = 3 * 1024 * 1024;
/// Most redirects followed by hand.
const MAX_REDIRECTS: usize = 4;

/// A page reduced to its readable text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedPage {
    pub url: String,
    pub title: String,
    pub text: String,
    /// True when `text` was cut at the requested limit.
    pub truncated: bool,
}

/// True for addresses a web page must never make us visit: this machine,
/// the local network, link-local (including cloud metadata), and unspecified.
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast() || v4.octets()[0] == 0 || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (seg[0] & 0xfe00) == 0xfc00 || (seg[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Splits `https://host:port/path` into (scheme, host, port, path+query).
pub fn split_url(url: &str) -> Option<(String, String, u16, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let (authority, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (h.trim_matches(['[', ']']).to_string(), p.parse().ok()?),
        _ => (authority.trim_matches(['[', ']']).to_string(), if scheme == "https" { 443 } else { 80 }),
    };
    if host.is_empty() {
        return None;
    }
    let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    Some((scheme, host, port, path.split('#').next().unwrap_or("/").to_string()))
}

/// Refuses an address that leads somewhere private.
pub fn check_public(url: &str) -> Result<(), String> {
    let (_, host, port, _) = split_url(url).ok_or("that is not a web address")?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") || host.ends_with(".internal") {
        return Err("local addresses are not allowed".into());
    }
    let addrs: Vec<_> = (host.as_str(), port).to_socket_addrs().map_err(|_| "the address could not be found".to_string())?.collect();
    if addrs.is_empty() || addrs.iter().any(|a| is_private_ip(a.ip())) {
        return Err("private addresses are not allowed".into());
    }
    Ok(())
}

/// What a robots.txt says about the `*` user agent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Robots {
    disallow: Vec<String>,
    allow: Vec<String>,
}

impl Robots {
    pub fn parse(text: &str) -> Self {
        let mut r = Robots::default();
        let mut applies = false;
        let mut in_group_header = false;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once(':') else { continue };
            let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
            match key.as_str() {
                "user-agent" => {
                    if !in_group_header {
                        applies = false;
                    }
                    in_group_header = true;
                    if value == "*" {
                        applies = true;
                    }
                }
                "disallow" | "allow" => {
                    in_group_header = false;
                    if applies && !value.is_empty() {
                        if key == "disallow" { &mut r.disallow } else { &mut r.allow }.push(value.to_string());
                    }
                }
                _ => in_group_header = false,
            }
        }
        r
    }

    /// The longest matching rule wins; allow beats disallow on a tie.
    pub fn allows(&self, path: &str) -> bool {
        let matches = |rule: &str| {
            let rule = rule.trim_end_matches('$');
            match rule.split_once('*') {
                Some((a, b)) => path.starts_with(a) && path[a.len()..].contains(b),
                None => path.starts_with(rule),
            }
        };
        let best = |rules: &[String]| rules.iter().filter(|r| matches(r)).map(|r| r.len()).max();
        match (best(&self.disallow), best(&self.allow)) {
            (None, _) => true,
            (Some(d), Some(a)) => a >= d,
            (Some(_), None) => false,
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .redirects(0)
        .user_agent(concat!("Thulli/", env!("CARGO_PKG_VERSION"), " (local assistant; reads pages you ask about)"))
        .build()
}

/// GET with the address checked on every hop. Returns (final url, content type, body).
fn get_checked(url: &str, accept: &str) -> Result<(String, String, String), String> {
    let agent = agent();
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        check_public(&current)?;
        // A slow site gets one more try before giving up.
        let mut attempt = agent.get(&current).set("Accept", accept).call();
        if matches!(attempt, Err(ureq::Error::Transport(_))) {
            attempt = agent.get(&current).set("Accept", accept).call();
        }
        let resp = match attempt {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) if (300..400).contains(&code) => r,
            Err(ureq::Error::Status(code, _)) => return Err(format!("the site answered with error {code}")),
            Err(e) => return Err(format!("the page could not be loaded: {e}")),
        };
        if (300..400).contains(&resp.status()) {
            let loc = resp.header("Location").ok_or("a redirect had no destination")?.to_string();
            current = resolve(&current, &loc);
            continue;
        }
        let ctype = resp.header("Content-Type").unwrap_or("").to_ascii_lowercase();
        let mut bytes = Vec::new();
        resp.into_reader().take(MAX_BODY_BYTES).read_to_end(&mut bytes).map_err(|e| format!("the page could not be read: {e}"))?;
        return Ok((current, ctype, String::from_utf8_lossy(&bytes).into_owned()));
    }
    Err("too many redirects".into())
}

/// Makes a redirect target absolute.
pub fn resolve(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    let Some((scheme, host, port, path)) = split_url(base) else { return location.to_string() };
    let default = if scheme == "https" { 443 } else { 80 };
    let origin = if port == default { format!("{scheme}://{host}") } else { format!("{scheme}://{host}:{port}") };
    if let Some(rest) = location.strip_prefix("//") {
        return format!("{scheme}://{rest}");
    }
    if location.starts_with('/') {
        return format!("{origin}{location}");
    }
    let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    format!("{origin}{dir}/{location}")
}

/// Loads `url` and returns its readable text, cut to `max_chars`.
pub fn fetch_page(url: &str, max_chars: usize) -> Result<FetchedPage, String> {
    check_public(url)?;
    // robots.txt: if it cannot be read, the page is treated as allowed.
    if let Some((scheme, host, port, path)) = split_url(url) {
        let robots_url = format!("{scheme}://{host}:{port}/robots.txt");
        if let Ok((_, _, body)) = get_checked(&robots_url, "text/plain") {
            if !Robots::parse(&body).allows(&path) {
                return Err("the site asks not to be read by programs".into());
            }
        }
    }
    let (final_url, ctype, body) = get_checked(url, "text/html,application/xhtml+xml,text/plain;q=0.9")?;
    if ctype.contains("pdf") {
        return Err("PDF files are not supported yet".into());
    }
    let (title, text) = if ctype.contains("html") || ctype.is_empty() { html_to_text(&body) } else if ctype.starts_with("text/") { (String::new(), body.trim().to_string()) } else { return Err(format!("this kind of file ({ctype}) cannot be read")) };
    let truncated = text.chars().count() > max_chars;
    let text: String = if truncated { text.chars().take(max_chars).collect() } else { text };
    Ok(FetchedPage { url: final_url, title, text, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_addresses_are_refused() {
        for ip in ["127.0.0.1", "10.1.2.3", "192.168.0.5", "172.16.0.1", "169.254.169.254", "0.0.0.0", "100.64.0.1", "::1", "fe80::1", "fc00::1", "::ffff:127.0.0.1"] {
            assert!(is_private_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "93.184.216.34", "2606:4700:4700::1111"] {
            assert!(!is_private_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn only_web_addresses_with_public_hosts_pass() {
        assert!(check_public("file:///etc/passwd").is_err());
        assert!(check_public("ftp://example.com/x").is_err());
        assert!(check_public("http://localhost:8080/").is_err());
        assert!(check_public("http://127.0.0.1/").is_err());
        assert!(check_public("http://[::1]/").is_err());
        assert!(check_public("http://printer.local/").is_err());
    }

    #[test]
    fn urls_split_and_redirects_resolve() {
        assert_eq!(split_url("https://a.test:8443/p?q=1#frag"), Some(("https".into(), "a.test".into(), 8443, "/p?q=1".into())));
        assert_eq!(split_url("http://a.test"), Some(("http".into(), "a.test".into(), 80, "/".into())));
        assert_eq!(split_url("https://user:pw@a.test/x").map(|u| u.1), Some("a.test".into()));
        assert_eq!(resolve("https://a.test/dir/page", "other"), "https://a.test/dir/other");
        assert_eq!(resolve("https://a.test/dir/page", "/root"), "https://a.test/root");
        assert_eq!(resolve("https://a.test/x", "//b.test/y"), "https://b.test/y");
        assert_eq!(resolve("https://a.test/x", "https://c.test/"), "https://c.test/");
    }

    #[test]
    fn robots_rules_apply() {
        let r = Robots::parse("User-agent: Googlebot\nDisallow: /\n\nUser-agent: *\nDisallow: /private\nAllow: /private/open\nDisallow: /tmp*.bak$\n");
        assert!(r.allows("/index.html"));
        assert!(!r.allows("/private/data"));
        assert!(r.allows("/private/open/page"));
        assert!(!r.allows("/tmp/a.bak"));
        assert!(Robots::parse("").allows("/anything"));
        assert!(!Robots::parse("User-agent: *\nDisallow: /").allows("/x"));
    }
}
