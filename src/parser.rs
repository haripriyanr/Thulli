use regex::Regex;
use std::sync::LazyLock;

static TIME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    // Matches: 1h 30m, 45m, 2h, 1.5h, 90 mins, 0130, 1:30
    Regex::new(r"(?i)\b(?:(\d+(?:\.\d+)?)\s*(?:h|hrs|hours?))?\s*(?:(\d+)\s*(?:m|mins?|minutes?))?\b").unwrap()
});

static SHORTHAND_TIME: LazyLock<Regex> = LazyLock::new(|| {
    // Matches 4-digit military time like 0130 (1 hr 30 mins) or 0045 (45 mins)
    Regex::new(r"\b(\d{2})(\d{2})\b").unwrap()
});

static COLON_TIME: LazyLock<Regex> = LazyLock::new(|| {
    // Matches 1:30 or 0:45
    Regex::new(r"\b(\d{1,2}):(\d{2})\b").unwrap()
});

static TAG_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    // Matches @today, @week, @this-week, @backlog
    Regex::new(r"(?i)@(today|this-?week|week|backlog)\b").unwrap()
});

#[derive(Debug, PartialEq)]
pub struct ParsedTaskTitle {
    pub cleaned_title: String,
    pub estimate_mins: u32,
    pub target_board: String, // "today", "week", "backlog"
}

/// Reverse-engineered directly from Blitzit's `useTitleParser.js`
pub fn parse_task_title(input: &str, default_board: &str) -> ParsedTaskTitle {
    let mut title = input.trim().to_string();
    let mut target_board = default_board.to_string();
    let mut estimate_mins = 25; // Blitzit default

    // 1. Parse @board tags
    if let Some(caps) = TAG_REGEX.captures(&title) {
        let tag = caps.get(1).unwrap().as_str().to_lowercase();
        target_board = match tag.as_str() {
            "today" => "today".to_string(),
            "week" | "this-week" | "thisweek" => "week".to_string(),
            "backlog" => "backlog".to_string(),
            _ => default_board.to_string(),
        };
        title = TAG_REGEX.replace(&title, "").to_string();
    }

    // 2. Parse colon time like 1:30 (90m) or 0:45 (45m)
    if let Some(caps) = COLON_TIME.captures(&title) {
        if let (Some(h_m), Some(m_m)) = (caps.get(1), caps.get(2)) {
            let h: u32 = h_m.as_str().parse().unwrap_or(0);
            let m: u32 = m_m.as_str().parse().unwrap_or(0);
            if h > 0 || m > 0 {
                estimate_mins = h * 60 + m;
                title = COLON_TIME.replace(&title, "").to_string();
            }
        }
    } else if let Some(caps) = SHORTHAND_TIME.captures(&title) {
        // 3. Parse 4-digit shorthand e.g. 0130
        if let (Some(h_m), Some(m_m)) = (caps.get(1), caps.get(2)) {
            let h: u32 = h_m.as_str().parse().unwrap_or(0);
            let m: u32 = m_m.as_str().parse().unwrap_or(0);
            if h > 0 || m > 0 {
                estimate_mins = h * 60 + m;
                title = SHORTHAND_TIME.replace(&title, "").to_string();
            }
        }
    } else {
        // 4. Parse text expressions like "1h 30m" or "45m"
        let mut total_mins = 0;
        let mut matched = false;

        for caps in TIME_REGEX.captures_iter(&title) {
            let h_val = caps.get(1).map(|m| m.as_str().parse::<f32>().unwrap_or(0.0)).unwrap_or(0.0);
            let m_val = caps.get(2).map(|m| m.as_str().parse::<u32>().unwrap_or(0)).unwrap_or(0);
            if h_val > 0.0 || m_val > 0 {
                total_mins += (h_val * 60.0) as u32 + m_val;
                matched = true;
            }
        }

        if matched && total_mins > 0 {
            estimate_mins = total_mins;
            title = TIME_REGEX.replace_all(&title, "").to_string();
        }
    }

    // Clean whitespace
    let cleaned_title = title
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ");

    ParsedTaskTitle {
        cleaned_title: if cleaned_title.is_empty() { "Untitled Task".to_string() } else { cleaned_title },
        estimate_mins,
        target_board,
    }
}
