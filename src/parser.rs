use regex::Regex;
use std::sync::LazyLock;

static DURATION_HM: LazyLock<Regex> = LazyLock::new(|| {
    // Matches: 1h 30m, 1h, 1.5h, 2 hrs
    Regex::new(r"(?i)\b(\d+(?:\.\d+)?)\s*(?:h|hrs|hours?)(?:\s*(\d+)\s*(?:m|mins?|minutes?))?\b").unwrap()
});

static DURATION_M_ONLY: LazyLock<Regex> = LazyLock::new(|| {
    // Matches: 45m, 30 mins, 90m
    Regex::new(r"(?i)\b(\d+)\s*(?:m|mins?|minutes?)\b").unwrap()
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

#[derive(Debug, PartialEq, Clone)]
pub struct ParsedTaskTitle {
    pub cleaned_title: String,
    pub estimate_mins: u32,
    pub target_board: String, // "today", "week", "backlog"
}

/// High performance natural language title parser with duration and tag extraction
pub fn parse_task_title(input: &str, default_board: &str) -> ParsedTaskTitle {
    let mut title = input.trim().to_string();
    let mut target_board = default_board.to_string();
    let mut estimate_mins = 25; // Default estimate

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
    } else if let Some(caps) = DURATION_HM.captures(&title) {
        // 3. Parse "1h 30m" or "2h"
        let h_val = caps.get(1).map(|m| m.as_str().parse::<f32>().unwrap_or(0.0)).unwrap_or(0.0);
        let m_val = caps.get(2).map(|m| m.as_str().parse::<u32>().unwrap_or(0)).unwrap_or(0);
        if h_val > 0.0 || m_val > 0 {
            estimate_mins = (h_val * 60.0) as u32 + m_val;
            title = DURATION_HM.replace(&title, "").to_string();
        }
    } else if let Some(caps) = DURATION_M_ONLY.captures(&title) {
        // 4. Parse "45m" or "30 mins"
        let m_val = caps.get(1).map(|m| m.as_str().parse::<u32>().unwrap_or(0)).unwrap_or(0);
        if m_val > 0 {
            estimate_mins = m_val;
            title = DURATION_M_ONLY.replace(&title, "").to_string();
        }
    } else if let Some(caps) = SHORTHAND_TIME.captures(&title) {
        // 5. Parse 4-digit shorthand e.g. 0130
        if let (Some(h_m), Some(m_m)) = (caps.get(1), caps.get(2)) {
            let h: u32 = h_m.as_str().parse().unwrap_or(0);
            let m: u32 = m_m.as_str().parse().unwrap_or(0);
            if h > 0 || m > 0 {
                estimate_mins = h * 60 + m;
                title = SHORTHAND_TIME.replace(&title, "").to_string();
            }
        }
    }

    // Clean whitespace
    let cleaned = title
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ");

    ParsedTaskTitle {
        cleaned_title: if cleaned.is_empty() { "Untitled Task".to_string() } else { cleaned },
        estimate_mins,
        target_board,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smart_parser() {
        let p1 = parse_task_title("Build auth 45m @today", "today");
        assert_eq!(p1.cleaned_title, "Build auth");
        assert_eq!(p1.estimate_mins, 45);
        assert_eq!(p1.target_board, "today");

        let p2 = parse_task_title("Write documentation 1h 30m", "today");
        assert_eq!(p2.cleaned_title, "Write documentation");
        assert_eq!(p2.estimate_mins, 90);

        let p3 = parse_task_title("Refactor engine @week", "today");
        assert_eq!(p3.cleaned_title, "Refactor engine");
        assert_eq!(p3.estimate_mins, 25);
        assert_eq!(p3.target_board, "week");

        let p4 = parse_task_title("Plain simple todo", "today");
        assert_eq!(p4.cleaned_title, "Plain simple todo");
        assert_eq!(p4.estimate_mins, 25);
        assert_eq!(p4.target_board, "today");
    }
}
