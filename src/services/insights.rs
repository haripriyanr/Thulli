//! Insights and reports built from tracked time and finished tasks.
//!
//! All calculations are pure functions over [`Span`]s (blocks of tracked time
//! in local time), so they are easy to test on fixed sample data. The
//! [`InsightsService`] gathers spans from the shared state, runs the
//! calculations and applies hand edits to the session log.
//!
//! Where time comes from: every [`TimeEntry`] is a span. A finished
//! [`FocusSession`] that has no entry pointing at it also counts as a focus
//! span, so nothing is lost or counted twice.
//!
//! Definitions used on screen:
//! - Worked time is focus, planning and other time. Breaks are tracked apart.
//! - A day "counts" towards a streak when 10 minutes or more were worked.
//! - A done task is on time when it was finished by its deadline; with no
//!   deadline it is on time when it stayed within its estimate. Tasks with
//!   neither are left out of the ratio.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc, Weekday};

use crate::models::{AppStateStore, Task, TaskSection, TaskStatus, TimeEntry, TimeEntryKind, WeekStart};
use crate::services::store_handle::StoreHandle;

/// A day counts towards the streak at this much worked time.
pub const STREAK_MIN_SECS: u64 = 600;
/// Longest custom range accepted, in days.
pub const MAX_RANGE_DAYS: i64 = 731;

// ---------------------------------------------------------------------------
// Time blocks
// ---------------------------------------------------------------------------

/// Where a span was stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanSource {
    Entry,
    Session,
}

/// A block of tracked time in local time.
#[derive(Debug, Clone)]
pub struct Span {
    pub id: String,
    pub source: SpanSource,
    pub kind: TimeEntryKind,
    pub task_id: Option<String>,
    pub label: String,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
}

impl Span {
    pub fn is_break(&self) -> bool {
        self.kind == TimeEntryKind::Break
    }

    pub fn duration_secs(&self) -> u64 {
        (self.end - self.start).num_seconds().max(0) as u64
    }

    /// Seconds of this span inside `[from, to)`.
    pub fn secs_between(&self, from: NaiveDateTime, to: NaiveDateTime) -> u64 {
        let start = self.start.max(from);
        let end = self.end.min(to);
        (end - start).num_seconds().max(0) as u64
    }

    /// Seconds of this span that fall on `date`.
    pub fn secs_on(&self, date: NaiveDate) -> u64 {
        let from = date.and_hms_opt(0, 0, 0).unwrap_or_default();
        self.secs_between(from, from + Duration::days(1))
    }

    /// Calls `f(piece_start, secs)` for each part of the span, split at hour boundaries.
    fn for_each_hour_piece(&self, mut f: impl FnMut(NaiveDateTime, u64)) {
        let mut cur = self.start;
        while cur < self.end {
            let hour_start = cur.date().and_hms_opt(cur.hour(), 0, 0).unwrap_or(cur);
            let next = hour_start + Duration::hours(1);
            let piece_end = next.min(self.end);
            f(cur, (piece_end - cur).num_seconds().max(0) as u64);
            cur = piece_end;
        }
    }
}

/// Converts a stored UTC time to local time.
pub fn to_local(dt: DateTime<Utc>) -> NaiveDateTime {
    dt.with_timezone(&Local).naive_local()
}

/// Converts a local time to UTC. `None` only for times skipped by a clock change.
pub fn to_utc(local: NaiveDateTime) -> Option<DateTime<Utc>> {
    Local.from_local_datetime(&local).earliest().map(|d| d.with_timezone(&Utc))
}

/// Every block of tracked time in the state, oldest first.
pub fn collect_spans(store: &AppStateStore) -> Vec<Span> {
    let linked: HashSet<&str> = store.time_entries.iter().filter_map(|e| e.session_id.as_deref()).collect();
    let mut spans: Vec<Span> = store
        .time_entries
        .iter()
        .map(|e| Span {
            id: e.id.clone(),
            source: SpanSource::Entry,
            kind: e.kind,
            task_id: e.task_id.clone(),
            label: e.label.clone(),
            start: to_local(e.started_at),
            end: to_local(e.ended_at),
        })
        .collect();
    for s in &store.focus_sessions {
        if linked.contains(s.id.as_str()) || s.focused_secs == 0 {
            continue;
        }
        let start = to_local(s.started_at);
        spans.push(Span {
            id: s.id.clone(),
            source: SpanSource::Session,
            kind: TimeEntryKind::Focus,
            task_id: s.task_id.clone(),
            label: String::new(),
            start,
            end: start + Duration::seconds(s.focused_secs as i64),
        });
    }
    spans.sort_by_key(|s| s.start);
    spans
}

/// Keeps only spans whose task is in `list_id`. `None` keeps everything.
pub fn filter_by_list(store: &AppStateStore, spans: Vec<Span>, list_id: Option<&str>) -> Vec<Span> {
    let Some(list_id) = list_id.filter(|l| !l.is_empty()) else { return spans };
    spans
        .into_iter()
        .filter(|s| s.task_id.as_deref().and_then(|id| store.task(id)).is_some_and(|t| t.list_id == list_id))
        .collect()
}

// ---------------------------------------------------------------------------
// Dates and formatting
// ---------------------------------------------------------------------------

/// True when `date` counts towards goals.
pub fn counts_towards_goal(date: NaiveDate, count_weekends: bool) -> bool {
    count_weekends || !matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
}

/// First day of the week containing `date`.
pub fn week_start_of(date: NaiveDate, week_start: WeekStart) -> NaiveDate {
    let back = match week_start {
        WeekStart::Monday => date.weekday().num_days_from_monday(),
        WeekStart::Sunday => date.weekday().num_days_from_sunday(),
        WeekStart::Saturday => (date.weekday().num_days_from_monday() + 2) % 7,
    };
    date - Duration::days(i64::from(back))
}

/// First and last day of the week `offset` weeks away from the one holding `today`.
pub fn week_bounds(today: NaiveDate, week_start: WeekStart, offset: i32) -> (NaiveDate, NaiveDate) {
    let start = week_start_of(today, week_start) + Duration::weeks(i64::from(offset));
    (start, start + Duration::days(6))
}

/// "This week", "Last week", "Next week" or the date span.
pub fn week_label(offset: i32, from: NaiveDate, to: NaiveDate) -> String {
    match offset {
        0 => "This week".to_string(),
        -1 => "Last week".to_string(),
        1 => "Next week".to_string(),
        _ => format!("{} to {}", from.format("%d %b"), to.format("%d %b")),
    }
}

/// Parses `YYYY-MM-DD`.
pub fn parse_date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()
}

/// Parses `HH:MM` (also `H:MM`).
pub fn parse_hhmm(text: &str) -> Option<chrono::NaiveTime> {
    chrono::NaiveTime::parse_from_str(text.trim(), "%H:%M").ok()
}

/// Parses `YYYY-MM-DD HH:MM`.
pub fn parse_date_time(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M").ok()
}

/// Date span for a quick range (`7d`, `30d`, `90d`) ending today, or the custom one.
pub fn range_bounds(range: &str, today: NaiveDate, custom: Option<(NaiveDate, NaiveDate)>) -> (NaiveDate, NaiveDate) {
    let days = match range {
        "30d" => 30,
        "90d" => 90,
        "custom" => {
            if let Some((a, b)) = custom {
                let (from, to) = if a <= b { (a, b) } else { (b, a) };
                let from = from.max(to - Duration::days(MAX_RANGE_DAYS - 1));
                return (from, to);
            }
            7
        }
        _ => 7,
    };
    (today - Duration::days(days - 1), today)
}

/// "2h 05m", "45m", "0m".
pub fn format_duration(secs: u64) -> String {
    let mins = secs / 60;
    if mins >= 60 { format!("{}h {:02}m", mins / 60, mins % 60) } else { format!("{mins}m") }
}

pub fn weekday_name(index: u32) -> &'static str {
    ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"][(index % 7) as usize]
}

pub fn month_name(index: u32) -> &'static str {
    [
        "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
        "December",
    ][(index % 12) as usize]
}

/// "14:00 to 15:00" for hour 14.
pub fn hour_label(hour: u32) -> String {
    format!("{:02}:00 to {:02}:00", hour % 24, (hour + 1) % 24)
}

// ---------------------------------------------------------------------------
// Per-day totals
// ---------------------------------------------------------------------------

/// Tracked time on one day, in seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayTotals {
    pub date: NaiveDate,
    pub focus: u64,
    pub planning: u64,
    pub other: u64,
    pub breaks: u64,
}

impl DayTotals {
    pub fn new(date: NaiveDate) -> Self {
        Self { date, focus: 0, planning: 0, other: 0, breaks: 0 }
    }

    fn add(&mut self, kind: TimeEntryKind, secs: u64) {
        match kind {
            TimeEntryKind::Focus => self.focus += secs,
            TimeEntryKind::Planning => self.planning += secs,
            TimeEntryKind::Other => self.other += secs,
            TimeEntryKind::Break => self.breaks += secs,
        }
    }

    /// Focus, planning and other time.
    pub fn worked(&self) -> u64 {
        self.focus + self.planning + self.other
    }

    /// Worked time plus breaks.
    pub fn total(&self) -> u64 {
        self.worked() + self.breaks
    }
}

/// One entry per day from `from` to `to` inclusive, splitting spans at midnight.
pub fn daily_totals(spans: &[Span], from: NaiveDate, to: NaiveDate) -> Vec<DayTotals> {
    if to < from {
        return Vec::new();
    }
    let n = (to - from).num_days() as usize + 1;
    let mut out: Vec<DayTotals> = (0..n).map(|i| DayTotals::new(from + Duration::days(i as i64))).collect();
    for span in spans {
        let mut day = span.start.date().max(from);
        let last = span.end.date().min(to);
        while day <= last {
            let secs = span.secs_on(day);
            if secs > 0 {
                out[(day - from).num_days() as usize].add(span.kind, secs);
            }
            match day.succ_opt() {
                Some(next) => day = next,
                None => break,
            }
        }
    }
    out
}

/// Sum of the worked time in `days`.
pub fn total_worked(days: &[DayTotals]) -> u64 {
    days.iter().map(DayTotals::worked).sum()
}

// ---------------------------------------------------------------------------
// Pace
// ---------------------------------------------------------------------------

/// Cumulative worked time against the goal pace, one value per counted day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaceReport {
    /// Counted days of the period.
    pub days: Vec<NaiveDate>,
    /// Worked seconds so far at the end of each counted day up to today.
    pub worked_cum: Vec<u64>,
    /// Goal seconds at the end of each counted day.
    pub pace_cum: Vec<u64>,
    /// Worked minus pace as of the latest day reached (negative is behind).
    pub delta_secs: i64,
}

pub fn pace(days: &[DayTotals], goal_secs_per_day: u64, count_weekends: bool, today: NaiveDate) -> PaceReport {
    let mut report =
        PaceReport { days: Vec::new(), worked_cum: Vec::new(), pace_cum: Vec::new(), delta_secs: 0 };
    let (mut worked, mut pace) = (0u64, 0u64);
    for d in days.iter().filter(|d| counts_towards_goal(d.date, count_weekends)) {
        pace += goal_secs_per_day;
        report.days.push(d.date);
        report.pace_cum.push(pace);
        if d.date <= today {
            worked += d.worked();
            report.worked_cum.push(worked);
        }
    }
    if let Some(w) = report.worked_cum.last() {
        report.delta_secs = *w as i64 - report.pace_cum[report.worked_cum.len() - 1] as i64;
    }
    report
}

/// "On pace, 1h 10m ahead", "40m behind pace".
pub fn pace_label(report: &PaceReport) -> String {
    if report.worked_cum.is_empty() {
        return String::new();
    }
    let delta = report.delta_secs;
    if delta == 0 {
        "On pace".to_string()
    } else if delta > 0 {
        format!("On pace, {} ahead", format_duration(delta as u64))
    } else {
        format!("{} behind pace", format_duration(delta.unsigned_abs()))
    }
}

/// Chart points as `(x, y)` in 0 to 1 (y of 0 is the bottom): `(worked, pace)`.
/// Both lines start at the origin; x spreads the counted days evenly.
pub fn pace_points(report: &PaceReport) -> (Vec<(f32, f32)>, Vec<(f32, f32)>) {
    let n = report.pace_cum.len();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }
    let max = report.worked_cum.iter().chain(report.pace_cum.iter()).copied().max().unwrap_or(0).max(1) as f32;
    let x = |i: usize| i as f32 / n as f32;
    let mut pace = vec![(0.0, 0.0)];
    pace.extend(report.pace_cum.iter().enumerate().map(|(i, v)| (x(i + 1), *v as f32 / max)));
    let mut worked = Vec::new();
    if !report.worked_cum.is_empty() {
        worked.push((0.0, 0.0));
        worked.extend(report.worked_cum.iter().enumerate().map(|(i, v)| (x(i + 1), *v as f32 / max)));
    }
    (worked, pace)
}

/// SVG path commands for a line through `points` in a 0 to 1000 box (y of 0 in the
/// input is the bottom). Empty for fewer than two points.
pub fn line_path(points: &[(f32, f32)]) -> String {
    if points.len() < 2 {
        return String::new();
    }
    points
        .iter()
        .enumerate()
        .map(|(i, (x, y))| format!("{} {:.1} {:.1}", if i == 0 { "M" } else { "L" }, x * 1000.0, (1.0 - y) * 1000.0))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Like [`line_path`], closed down to the bottom edge so it can be filled.
pub fn area_path(points: &[(f32, f32)]) -> String {
    let (Some(first), Some(last)) = (points.first(), points.last()) else { return String::new() };
    if points.len() < 2 {
        return String::new();
    }
    format!("{} L {:.1} 1000 L {:.1} 1000 Z", line_path(points), last.0 * 1000.0, first.0 * 1000.0)
}

// ---------------------------------------------------------------------------
// Ratio, bars, heatmap, streak
// ---------------------------------------------------------------------------

/// Share of worked time by kind (fractions add up to 1, or all 0 with no time).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeRatio {
    pub focus: f32,
    pub planning: f32,
    pub other: f32,
    pub total_secs: u64,
}

pub fn time_ratio(days: &[DayTotals]) -> TimeRatio {
    let focus: u64 = days.iter().map(|d| d.focus).sum();
    let planning: u64 = days.iter().map(|d| d.planning).sum();
    let other: u64 = days.iter().map(|d| d.other).sum();
    let total = focus + planning + other;
    if total == 0 {
        return TimeRatio { focus: 0.0, planning: 0.0, other: 0.0, total_secs: 0 };
    }
    let t = total as f32;
    TimeRatio { focus: focus as f32 / t, planning: planning as f32 / t, other: other as f32 / t, total_secs: total }
}

/// Scales values to 0 to 1 of `max` (a zero max gives zeros).
pub fn normalise(values: &[u64], max: u64) -> Vec<f32> {
    values.iter().map(|v| if max == 0 { 0.0 } else { (*v as f32 / max as f32).min(1.0) }).collect()
}

/// One cell of the consistency heatmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeatDay {
    pub date: NaiveDate,
    pub secs: u64,
    /// 0 (nothing, or still in the future) to 4 (goal reached).
    pub level: u8,
    pub future: bool,
}

/// Heat level for `secs` worked against a goal.
pub fn heat_level(secs: u64, goal_secs: u64) -> u8 {
    if secs == 0 {
        return 0;
    }
    let goal = if goal_secs == 0 { 3600 } else { goal_secs } as f64;
    let ratio = secs as f64 / goal;
    match ratio {
        r if r < 0.25 => 1,
        r if r < 0.5 => 2,
        r if r < 1.0 => 3,
        _ => 4,
    }
}

/// `weeks` columns of seven days, aligned to the week start, ending with the week of `reference`.
/// Cell `i` sits in column `i / 7` and row `i % 7`. Days after `today` are marked as future.
pub fn heatmap(
    spans: &[Span],
    reference: NaiveDate,
    today: NaiveDate,
    week_start: WeekStart,
    weeks: u32,
    goal_secs: u64,
) -> Vec<HeatDay> {
    let first = week_start_of(reference, week_start) - Duration::weeks(i64::from(weeks) - 1);
    let last = first + Duration::days(i64::from(weeks) * 7 - 1);
    daily_totals(spans, first, last)
        .into_iter()
        .map(|d| {
            let future = d.date > today;
            let secs = d.worked();
            HeatDay { date: d.date, secs, level: if future { 0 } else { heat_level(secs, goal_secs) }, future }
        })
        .collect()
}

/// Consecutive counted days ending today (or yesterday while today is still empty).
pub fn streak(totals: &[DayTotals], today: NaiveDate, count_weekends: bool) -> u32 {
    let worked: HashMap<NaiveDate, u64> = totals.iter().map(|d| (d.date, d.worked())).collect();
    let met = |d: NaiveDate| worked.get(&d).copied().unwrap_or(0) >= STREAK_MIN_SECS;
    let mut day = today;
    if !met(day) {
        day -= Duration::days(1);
    }
    let mut n = 0;
    for _ in 0..MAX_RANGE_DAYS * 2 {
        if counts_towards_goal(day, count_weekends) {
            if met(day) {
                n += 1;
            } else {
                break;
            }
        }
        day -= Duration::days(1);
    }
    n
}

/// End points of one donut arc in a 0 to 1000 box (centre 500, radius 400).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcGeometry {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// True when the arc covers more than half a turn.
    pub large: bool,
}

/// Arcs for slices given as fractions of a turn, starting at the top and going
/// clockwise. Slices too small to see give `None`.
pub fn donut_geometry(fractions: &[f32]) -> Vec<Option<ArcGeometry>> {
    let point = |turn: f32| {
        let angle = turn * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        (500.0 + 400.0 * angle.cos(), 500.0 + 400.0 * angle.sin())
    };
    let mut start = 0.0f32;
    fractions
        .iter()
        .map(|f| {
            let f = f.clamp(0.0, 0.9999);
            let begin = start;
            start += f;
            if f < 0.002 {
                return None;
            }
            let (x0, y0) = point(begin);
            let (x1, y1) = point(begin + f);
            Some(ArcGeometry { x0, y0, x1, y1, large: f > 0.5 })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tasks worked on, productivity, time by list, on-time ratio
// ---------------------------------------------------------------------------

/// Task filter on the insights screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskFilter {
    #[default]
    All,
    Done,
    InProgress,
}

impl TaskFilter {
    pub fn parse(value: &str) -> Self {
        match value {
            "done" => TaskFilter::Done,
            "progress" => TaskFilter::InProgress,
            _ => TaskFilter::All,
        }
    }

    pub fn matches(&self, status: TaskStatus) -> bool {
        match self {
            TaskFilter::All => true,
            TaskFilter::Done => status == TaskStatus::Completed,
            TaskFilter::InProgress => matches!(status, TaskStatus::Open | TaskStatus::Waiting),
        }
    }
}

/// Seconds worked per task inside `[from, to]`, most time first. Breaks are ignored.
pub fn tasks_worked(spans: &[Span], from: NaiveDate, to: NaiveDate) -> Vec<(String, u64)> {
    let lo = from.and_hms_opt(0, 0, 0).unwrap_or_default();
    let hi = to.and_hms_opt(0, 0, 0).unwrap_or_default() + Duration::days(1);
    let mut per_task: HashMap<&str, u64> = HashMap::new();
    for s in spans.iter().filter(|s| !s.is_break()) {
        if let Some(id) = &s.task_id {
            let secs = s.secs_between(lo, hi);
            if secs > 0 {
                *per_task.entry(id.as_str()).or_default() += secs;
            }
        }
    }
    let mut out: Vec<(String, u64)> = per_task.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Worked seconds by hour of day, weekday (Monday is 0) and month (January is 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Distribution {
    pub hours: [u64; 24],
    pub weekdays: [u64; 7],
    pub months: [u64; 12],
}

impl Distribution {
    pub fn from_spans(spans: &[Span]) -> Self {
        let mut d = Distribution { hours: [0; 24], weekdays: [0; 7], months: [0; 12] };
        for s in spans.iter().filter(|s| !s.is_break()) {
            s.for_each_hour_piece(|at, secs| {
                d.hours[at.hour() as usize] += secs;
                d.weekdays[at.weekday().num_days_from_monday() as usize] += secs;
                d.months[at.month0() as usize] += secs;
            });
        }
        d
    }

    fn best(values: &[u64]) -> Option<u32> {
        let max = *values.iter().max()?;
        if max == 0 { None } else { values.iter().position(|v| *v == max).map(|i| i as u32) }
    }

    pub fn best_hour(&self) -> Option<u32> {
        Self::best(&self.hours)
    }

    pub fn best_weekday(&self) -> Option<u32> {
        Self::best(&self.weekdays)
    }

    pub fn best_month(&self) -> Option<u32> {
        Self::best(&self.months)
    }
}

/// Time attributed to one list (`None` for time without a task).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListShare {
    pub list_id: Option<String>,
    pub secs: u64,
}

/// Worked time per list, most time first. `list_of` maps a task id to its list id.
pub fn time_by_list(spans: &[Span], from: NaiveDate, to: NaiveDate, list_of: impl Fn(&str) -> Option<String>) -> Vec<ListShare> {
    let lo = from.and_hms_opt(0, 0, 0).unwrap_or_default();
    let hi = to.and_hms_opt(0, 0, 0).unwrap_or_default() + Duration::days(1);
    let mut per: HashMap<Option<String>, u64> = HashMap::new();
    for s in spans.iter().filter(|s| !s.is_break()) {
        let secs = s.secs_between(lo, hi);
        if secs > 0 {
            let list = s.task_id.as_deref().and_then(&list_of);
            *per.entry(list).or_default() += secs;
        }
    }
    let mut out: Vec<ListShare> = per.into_iter().map(|(list_id, secs)| ListShare { list_id, secs }).collect();
    out.sort_by(|a, b| b.secs.cmp(&a.secs).then_with(|| a.list_id.cmp(&b.list_id)));
    out
}

/// Done tasks finished on time out of those that can be judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnTime {
    pub on_time: usize,
    pub measured: usize,
}

impl OnTime {
    /// "75% (3 of 4)", or empty when nothing can be judged.
    pub fn label(&self) -> String {
        if self.measured == 0 {
            return String::new();
        }
        format!("{}% ({} of {})", self.on_time * 100 / self.measured, self.on_time, self.measured)
    }
}

pub fn on_time_ratio(done: &[&Task]) -> OnTime {
    let mut result = OnTime { on_time: 0, measured: 0 };
    for t in done {
        if let Some(deadline) = t.deadline {
            result.measured += 1;
            let finished = t.completed_at.map(|c| to_local(c).date()).unwrap_or(deadline);
            if finished <= deadline {
                result.on_time += 1;
            }
        } else if t.estimate_mins > 0 {
            result.measured += 1;
            if t.time_taken_secs <= u64::from(t.estimate_mins) * 60 {
                result.on_time += 1;
            }
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Session records, CSV and PDF
// ---------------------------------------------------------------------------

/// A span with its names resolved, ready to show or export.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: String,
    pub title: String,
    pub list_name: String,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub duration_secs: u64,
    pub kind: TimeEntryKind,
}

impl SessionRecord {
    pub fn is_break(&self) -> bool {
        self.kind == TimeEntryKind::Break
    }

    pub fn date(&self) -> NaiveDate {
        self.start.date()
    }
}

fn default_title(kind: TimeEntryKind) -> &'static str {
    match kind {
        TimeEntryKind::Planning => "Planning",
        TimeEntryKind::Focus => "Focus session",
        TimeEntryKind::Break => "Break",
        TimeEntryKind::Other => "Other",
    }
}

/// Resolves titles and list names for `spans`, newest first.
pub fn session_records(store: &AppStateStore, spans: &[&Span]) -> Vec<SessionRecord> {
    let mut out: Vec<SessionRecord> = spans
        .iter()
        .map(|s| {
            let task = s.task_id.as_deref().and_then(|id| store.task(id));
            let list = task.and_then(|t| store.lists.iter().find(|l| l.id == t.list_id));
            let title = task
                .map(|t| t.title.clone())
                .filter(|t| !t.is_empty())
                .or_else(|| Some(s.label.clone()).filter(|l| !l.is_empty()))
                .unwrap_or_else(|| default_title(s.kind).to_string());
            SessionRecord {
                id: s.id.clone(),
                title,
                list_name: list.map(|l| l.name.clone()).unwrap_or_default(),
                start: s.start,
                end: s.end,
                duration_secs: s.duration_secs(),
                kind: s.kind,
            }
        })
        .collect();
    out.sort_by(|a, b| b.start.cmp(&a.start));
    out
}

/// Quotes a CSV field when it holds a comma, quote or line break.
pub fn csv_escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// CSV text for a session log, oldest first.
pub fn sessions_csv(records: &[SessionRecord]) -> String {
    let mut out = String::from("Title,List,Type,Date,Start,End,Minutes\n");
    let mut sorted: Vec<&SessionRecord> = records.iter().collect();
    sorted.sort_by_key(|r| r.start);
    for r in sorted {
        let row = [
            csv_escape(&r.title),
            csv_escape(&r.list_name),
            r.kind.as_str().to_string(),
            r.start.format("%Y-%m-%d").to_string(),
            r.start.format("%H:%M").to_string(),
            r.end.format("%H:%M").to_string(),
            (r.duration_secs / 60).to_string(),
        ];
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// A minimal text-only PDF (A4, Helvetica). Characters outside printable ASCII become `?`.
pub fn simple_pdf(title: &str, lines: &[String]) -> Vec<u8> {
    const FIRST_PAGE_LINES: usize = 48;
    const PAGE_LINES: usize = 54;

    fn clean(text: &str) -> String {
        text.chars()
            .map(|c| match c {
                '\\' => "\\\\".to_string(),
                '(' => "\\(".to_string(),
                ')' => "\\)".to_string(),
                ' '..='~' => c.to_string(),
                _ => "?".to_string(),
            })
            .collect()
    }

    let mut pages: Vec<&[String]> = Vec::new();
    let mut rest = lines;
    let take = FIRST_PAGE_LINES.min(rest.len());
    pages.push(&rest[..take]);
    rest = &rest[take..];
    while !rest.is_empty() {
        let take = PAGE_LINES.min(rest.len());
        pages.push(&rest[..take]);
        rest = &rest[take..];
    }

    let mut out: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let mut push_obj = |out: &mut Vec<u8>, body: String| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", offsets.len(), body).as_bytes());
    };

    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", 5 + 2 * i)).collect();
    push_obj(&mut out, "<< /Type /Catalog /Pages 2 0 R >>".to_string());
    push_obj(&mut out, format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()));
    push_obj(&mut out, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());
    push_obj(&mut out, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".to_string());
    for (i, page) in pages.iter().enumerate() {
        let mut content = String::new();
        let top = if i == 0 {
            content.push_str(&format!("BT /F2 16 Tf 50 800 Td ({}) Tj ET\n", clean(title)));
            764
        } else {
            800
        };
        content.push_str(&format!("BT /F1 10 Tf 14 TL 50 {top} Td\n"));
        for line in page.iter() {
            content.push_str(&format!("({}) Tj T*\n", clean(line)));
        }
        content.push_str("ET\n");
        push_obj(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
                6 + 2 * i
            ),
        );
        push_obj(&mut out, format!("<< /Length {} >>\nstream\n{}endstream", content.len(), content));
    }

    let xref_at = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for off in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n", offsets.len() + 1, xref_at).as_bytes(),
    );
    out
}

// ---------------------------------------------------------------------------
// Assembled screens
// ---------------------------------------------------------------------------

/// A task and the time spent on it in a period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkedTask {
    pub task_id: String,
    pub secs: u64,
}

/// Everything the insights screen shows for one week.
#[derive(Debug, Clone)]
pub struct InsightsData {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub goal_secs: u64,
    pub pace: PaceReport,
    pub worked_secs: u64,
    /// Average worked time over the counted days reached so far.
    pub avg_secs: u64,
    pub tasks_done: usize,
    pub ratio: TimeRatio,
    /// Counted days of the week.
    pub bars: Vec<DayTotals>,
    pub heat: Vec<HeatDay>,
    pub streak: u32,
    pub worked_tasks: Vec<WorkedTask>,
    pub activity: Vec<SessionRecord>,
}

/// Totals row of the reports screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportTotals {
    pub work_days: usize,
    pub tasks_done: usize,
    pub worked_secs: u64,
    pub tasks_worked: usize,
    /// Worked time divided by the tasks worked on.
    pub avg_secs_per_task: u64,
    pub break_secs: u64,
}

/// A list's share of worked time, with display details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListSlice {
    pub name: String,
    /// `#RRGGBB`.
    pub color: String,
    pub secs: u64,
}

/// Everything the reports screen shows for a date range.
#[derive(Debug, Clone)]
pub struct ReportData {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub totals: ReportTotals,
    pub days: Vec<DayTotals>,
    pub by_list: Vec<ListSlice>,
    pub best_hour: Option<u32>,
    pub best_weekday: Option<u32>,
    pub best_month: Option<u32>,
    pub on_time: OnTime,
    pub done_task_ids: Vec<String>,
    pub sessions: Vec<SessionRecord>,
}

impl ReportData {
    /// Lines for the PDF export.
    pub fn text_lines(&self) -> Vec<String> {
        let mut l = vec![
            format!("{} to {}", self.from.format("%d %b %Y"), self.to.format("%d %b %Y")),
            String::new(),
            format!("Work days: {}", self.totals.work_days),
            format!("Tasks done: {}", self.totals.tasks_done),
            format!("Time worked: {}", format_duration(self.totals.worked_secs)),
            format!("Average time per task: {}", format_duration(self.totals.avg_secs_per_task)),
            format!("Break time: {}", format_duration(self.totals.break_secs)),
        ];
        if let Some(h) = self.best_hour {
            l.push(format!("Most productive hour: {}", hour_label(h)));
        }
        if let Some(d) = self.best_weekday {
            l.push(format!("Most productive day: {}", weekday_name(d)));
        }
        if let Some(m) = self.best_month {
            l.push(format!("Most productive month: {}", month_name(m)));
        }
        if !self.on_time.label().is_empty() {
            l.push(format!("Done tasks on time: {}", self.on_time.label()));
        }
        l.push(String::new());
        l.push("Time by list".to_string());
        for s in &self.by_list {
            l.push(format!("  {}: {}", s.name, format_duration(s.secs)));
        }
        l.push(String::new());
        l.push("Sessions".to_string());
        for r in self.sessions.iter().rev() {
            l.push(format!(
                "  {}  {}-{}  {}  {}",
                r.start.format("%Y-%m-%d"),
                r.start.format("%H:%M"),
                r.end.format("%H:%M"),
                format_duration(r.duration_secs),
                r.title
            ));
        }
        l
    }
}

fn done_in_range<'a>(store: &'a AppStateStore, from: NaiveDate, to: NaiveDate, list: Option<&str>) -> Vec<&'a Task> {
    let mut done: Vec<&Task> = store
        .tasks
        .iter()
        .filter(|t| !t.is_step() && t.is_completed())
        .filter(|t| list.map_or(true, |l| t.list_id == l))
        .filter(|t| t.completed_at.map(to_local).is_some_and(|c| c.date() >= from && c.date() <= to))
        .collect();
    done.sort_by(|a, b| b.completed_at.cmp(&a.completed_at));
    done
}

/// Builds the insights data for the week `week_offset` weeks from the current one.
pub fn build_insights(
    store: &AppStateStore,
    week_offset: i32,
    list: Option<&str>,
    task_filter: TaskFilter,
    today: NaiveDate,
) -> InsightsData {
    let prefs = &store.preferences;
    let list = list.filter(|l| !l.is_empty());
    let goal_secs = u64::from(prefs.daily_goal_mins) * 60;
    let (from, to) = week_bounds(today, prefs.week_start, week_offset);
    let spans = filter_by_list(store, collect_spans(store), list);

    let days = daily_totals(&spans, from, to);
    let pace = pace(&days, goal_secs, prefs.count_weekends, today);
    let bars: Vec<DayTotals> =
        days.iter().filter(|d| counts_towards_goal(d.date, prefs.count_weekends)).cloned().collect();
    let worked_secs = total_worked(&days);
    let elapsed = pace.worked_cum.len().max(1) as u64;
    let reference = to.min(today);
    let lookback = daily_totals(&spans, reference - Duration::days(400), reference);

    let worked_tasks = tasks_worked(&spans, from, to)
        .into_iter()
        .filter(|(id, _)| store.task(id).is_some_and(|t| task_filter.matches(t.status)))
        .map(|(task_id, secs)| WorkedTask { task_id, secs })
        .collect();

    let in_week: Vec<&Span> = spans
        .iter()
        .filter(|s| s.secs_between(from.and_hms_opt(0, 0, 0).unwrap_or_default(), (to + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap_or_default()) > 0)
        .collect();

    InsightsData {
        from,
        to,
        goal_secs,
        pace,
        worked_secs,
        avg_secs: worked_secs / elapsed,
        tasks_done: done_in_range(store, from, to, list).len(),
        ratio: time_ratio(&days),
        bars,
        heat: heatmap(&spans, reference, today, prefs.week_start, 5, goal_secs),
        streak: streak(&lookback, reference, prefs.count_weekends),
        worked_tasks,
        activity: session_records(store, &in_week),
    }
}

/// Builds the report for `from` to `to`, optionally for one list.
pub fn build_report(store: &AppStateStore, from: NaiveDate, to: NaiveDate, list: Option<&str>) -> ReportData {
    let list = list.filter(|l| !l.is_empty());
    let spans = filter_by_list(store, collect_spans(store), list);
    let lo = from.and_hms_opt(0, 0, 0).unwrap_or_default();
    let hi = to.and_hms_opt(0, 0, 0).unwrap_or_default() + Duration::days(1);
    let in_range: Vec<Span> = spans.iter().filter(|s| s.secs_between(lo, hi) > 0).cloned().collect();

    let days = daily_totals(&spans, from, to);
    let worked_secs = total_worked(&days);
    let worked_tasks = tasks_worked(&spans, from, to);
    let done = done_in_range(store, from, to, list);
    let dist = Distribution::from_spans(&in_range);

    let by_list = time_by_list(&spans, from, to, |task_id| store.task(task_id).map(|t| t.list_id.clone()))
        .into_iter()
        .map(|share| {
            let found = share.list_id.as_deref().and_then(|id| store.lists.iter().find(|l| l.id == id));
            ListSlice {
                name: found.map(|l| l.name.clone()).unwrap_or_else(|| "No list".to_string()),
                color: found.map(|l| l.color.clone()).unwrap_or_else(|| "#9EABBE".to_string()),
                secs: share.secs,
            }
        })
        .collect();

    let refs: Vec<&Span> = in_range.iter().collect();
    ReportData {
        from,
        to,
        totals: ReportTotals {
            work_days: days.iter().filter(|d| d.worked() > 0).count(),
            tasks_done: done.len(),
            worked_secs,
            tasks_worked: worked_tasks.len(),
            avg_secs_per_task: if worked_tasks.is_empty() { 0 } else { worked_secs / worked_tasks.len() as u64 },
            break_secs: days.iter().map(|d| d.breaks).sum(),
        },
        days,
        by_list,
        best_hour: dist.best_hour(),
        best_weekday: dist.best_weekday(),
        best_month: dist.best_month(),
        on_time: on_time_ratio(&done),
        done_task_ids: done.iter().map(|t| t.id.clone()).collect(),
        sessions: session_records(store, &refs),
    }
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Read-only reports built from tracked time and finished tasks, plus hand
/// edits to the session log.
pub struct InsightsService {
    handle: StoreHandle,
}

/// Finds a task with this title in the list, or creates one.
fn resolve_task(store: &mut AppStateStore, list_id: &str, title: &str) -> String {
    let list_id = if store.lists.iter().any(|l| l.id == list_id) {
        list_id.to_string()
    } else {
        store.get_primary_list().map(|l| l.id.clone()).unwrap_or_default()
    };
    let existing = store
        .tasks
        .iter()
        .find(|t| !t.is_step() && t.list_id == list_id && t.title.trim().eq_ignore_ascii_case(title.trim()));
    if let Some(t) = existing {
        return t.id.clone();
    }
    store.add_task(Task::new(list_id, title.trim(), 0, TaskSection::Today)).id
}

/// Moves `old_secs` of logged time from one task to another (or resizes it on the same task).
fn retime_task(store: &mut AppStateStore, old: Option<&str>, new: Option<&str>, old_secs: u64, new_secs: u64) {
    if old == new {
        if let Some(t) = new.and_then(|id| store.task_mut(id)) {
            t.time_taken_secs = t.time_taken_secs.saturating_sub(old_secs).saturating_add(new_secs);
        }
        return;
    }
    if let Some(t) = old.and_then(|id| store.task_mut(id)) {
        t.time_taken_secs = t.time_taken_secs.saturating_sub(old_secs);
    }
    if let Some(t) = new.and_then(|id| store.task_mut(id)) {
        t.time_taken_secs = t.time_taken_secs.saturating_add(new_secs);
    }
}

impl InsightsService {
    pub fn new(handle: StoreHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }

    pub fn insights(&self, week_offset: i32, list: Option<&str>, filter: TaskFilter, today: NaiveDate) -> InsightsData {
        self.handle.read(|s| build_insights(s, week_offset, list, filter, today))
    }

    pub fn report(&self, from: NaiveDate, to: NaiveDate, list: Option<&str>) -> ReportData {
        self.handle.read(|s| build_report(s, from, to, list))
    }

    /// Looks up one logged block by id (a time entry or a finished session).
    pub fn find_record(&self, id: &str) -> Option<SessionRecord> {
        self.handle.read(|s| {
            let spans = collect_spans(s);
            let found: Vec<&Span> = spans.iter().filter(|sp| sp.id == id).collect();
            session_records(s, &found).into_iter().next()
        })
    }

    /// The list a logged block belongs to (through its task), if any.
    pub fn list_id_of(&self, id: &str) -> Option<String> {
        self.handle.read(|s| {
            let task_id = collect_spans(s).into_iter().find(|sp| sp.id == id)?.task_id?;
            s.task(&task_id).map(|t| t.list_id.clone())
        })
    }

    /// Logs a block by hand. A task session is attached to the list's task with this title
    /// (created when missing), so it shows up under that list. Returns the new entry id.
    pub fn add_session(
        &self,
        title: &str,
        list_id: &str,
        start: NaiveDateTime,
        end: NaiveDateTime,
        is_break: bool,
    ) -> Result<String, String> {
        let (start_utc, end_utc) = validate_times(start, end)?;
        if !is_break && title.trim().is_empty() {
            return Err("Give the session a name.".to_string());
        }
        let secs = (end - start).num_seconds() as u64;
        Ok(self.handle.mutate(|st| {
            let kind = if is_break { TimeEntryKind::Break } else { TimeEntryKind::Focus };
            let mut entry = TimeEntry::new(kind, start_utc, end_utc);
            if is_break {
                entry.label = if title.trim().is_empty() { "Break".to_string() } else { title.trim().to_string() };
            } else {
                let task_id = resolve_task(st, list_id, title);
                retime_task(st, None, Some(&task_id), 0, secs);
                entry.task_id = Some(task_id);
            }
            let id = entry.id.clone();
            st.time_entries.push(entry);
            id
        }))
    }

    /// Changes the times of a logged block, and its name or list when given.
    pub fn update_session(
        &self,
        id: &str,
        title: Option<&str>,
        list_id: Option<&str>,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> Result<(), String> {
        let (start_utc, end_utc) = validate_times(start, end)?;
        let new_secs = (end - start).num_seconds() as u64;
        self.handle.mutate(|st| {
            let (old_task, old_secs, is_break) = if let Some(e) = st.time_entries.iter().find(|e| e.id == id) {
                (e.task_id.clone(), e.duration_secs(), e.kind == TimeEntryKind::Break)
            } else if let Some(s) = st.focus_sessions.iter().find(|s| s.id == id) {
                (s.task_id.clone(), s.focused_secs, false)
            } else {
                return Err("That session no longer exists.".to_string());
            };

            let mut new_task = old_task.clone();
            if !is_break {
                if let (Some(title), Some(list_id)) = (title.map(str::trim), list_id) {
                    let current = old_task.as_deref().and_then(|t| st.task(t));
                    let same = current.is_some_and(|t| t.title.trim() == title && (list_id.is_empty() || t.list_id == list_id));
                    if !same {
                        if title.is_empty() {
                            return Err("Give the session a name.".to_string());
                        }
                        let list_id = if list_id.is_empty() { current.map(|t| t.list_id.clone()).unwrap_or_default() } else { list_id.to_string() };
                        new_task = Some(resolve_task(st, &list_id, title));
                    }
                }
            }
            retime_task(st, old_task.as_deref(), new_task.as_deref(), old_secs, new_secs);

            if let Some(e) = st.time_entries.iter_mut().find(|e| e.id == id) {
                e.started_at = start_utc;
                e.ended_at = end_utc;
                e.task_id = new_task;
                if is_break {
                    if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
                        e.label = t.to_string();
                    }
                }
            } else if let Some(s) = st.focus_sessions.iter_mut().find(|s| s.id == id) {
                s.started_at = start_utc;
                s.ended_at = Some(end_utc);
                s.focused_secs = new_secs;
                s.task_id = new_task;
            }
            Ok(())
        })
    }

    /// Removes a logged block and takes its time off the task. Returns false when not found.
    pub fn delete_session(&self, id: &str) -> bool {
        self.handle.mutate(|st| {
            let mut found = false;
            let mut removed: Vec<(Option<String>, u64)> = Vec::new();
            let mut session_ids: Vec<String> = Vec::new();
            st.time_entries.retain(|e| {
                if e.id == id {
                    found = true;
                    removed.push((e.task_id.clone(), e.duration_secs()));
                    session_ids.extend(e.session_id.clone());
                    false
                } else {
                    true
                }
            });
            if !found {
                st.focus_sessions.retain(|s| {
                    if s.id == id {
                        found = true;
                        removed.push((s.task_id.clone(), s.focused_secs));
                        false
                    } else {
                        true
                    }
                });
            } else {
                // Drop the linked session too, or it would reappear as a span of its own.
                st.focus_sessions.retain(|s| !session_ids.contains(&s.id));
            }
            for (task, secs) in removed {
                retime_task(st, task.as_deref(), None, secs, 0);
            }
            found
        })
    }
}

fn validate_times(start: NaiveDateTime, end: NaiveDateTime) -> Result<(DateTime<Utc>, DateTime<Utc>), String> {
    if end <= start {
        return Err("The end time must be after the start time.".to_string());
    }
    if end - start > Duration::hours(24) {
        return Err("A session cannot be longer than 24 hours.".to_string());
    }
    match (to_utc(start), to_utc(end)) {
        (Some(s), Some(e)) => Ok((s, e)),
        _ => Err("That time does not exist on this date.".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::FocusSession;
    use crate::repository::{LoadReport, StorageRepository};
    use std::sync::{Arc, Mutex};

    fn dt(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
    }

    fn day(text: &str) -> NaiveDate {
        parse_date(text).unwrap()
    }

    fn span(kind: TimeEntryKind, task: Option<&str>, start: &str, end: &str) -> Span {
        Span {
            id: format!("{start}-{end}"),
            source: SpanSource::Entry,
            kind,
            task_id: task.map(str::to_string),
            label: String::new(),
            start: dt(start),
            end: dt(end),
        }
    }

    fn focus(task: &str, start: &str, end: &str) -> Span {
        span(TimeEntryKind::Focus, Some(task), start, end)
    }

    /// Monday 2 March 2026 to Sunday 8 March 2026.
    fn week_sample() -> Vec<Span> {
        vec![
            focus("a", "2026-03-02 09:00", "2026-03-02 11:00"),                         // Mon 2h
            span(TimeEntryKind::Break, None, "2026-03-02 11:00", "2026-03-02 11:15"),   // Mon break
            span(TimeEntryKind::Planning, None, "2026-03-03 08:30", "2026-03-03 09:00"), // Tue 30m
            focus("b", "2026-03-03 09:00", "2026-03-03 12:00"),                         // Tue 3h
            span(TimeEntryKind::Other, None, "2026-03-04 14:00", "2026-03-04 15:00"),   // Wed 1h
            focus("a", "2026-03-06 23:30", "2026-03-07 00:30"),                         // Fri 30m, Sat 30m
        ]
    }

    #[test]
    fn dates_in_the_sample_are_the_weekdays_assumed() {
        assert_eq!(day("2026-03-02").weekday(), Weekday::Mon);
        assert_eq!(day("2026-03-08").weekday(), Weekday::Sun);
    }

    #[test]
    fn daily_totals_split_at_midnight_and_by_kind() {
        let days = daily_totals(&week_sample(), day("2026-03-02"), day("2026-03-08"));
        assert_eq!(days.len(), 7);
        assert_eq!((days[0].focus, days[0].breaks), (7200, 900));
        assert_eq!((days[1].planning, days[1].focus, days[1].worked()), (1800, 10800, 12600));
        assert_eq!(days[2].other, 3600);
        assert_eq!(days[4].worked(), 1800);
        assert_eq!(days[5].worked(), 1800);
        assert_eq!(days[0].total(), 8100);
        assert_eq!(total_worked(&days), 7200 + 12600 + 3600 + 1800 + 1800);
    }

    #[test]
    fn pace_compares_cumulative_work_with_the_goal() {
        let days = daily_totals(&week_sample(), day("2026-03-02"), day("2026-03-08"));
        // 3 hour goal, weekends not counted, today is Wednesday.
        let report = pace(&days, 3 * 3600, false, day("2026-03-04"));
        assert_eq!(report.days.len(), 5);
        assert_eq!(report.worked_cum, vec![7200, 19800, 23400]);
        assert_eq!(report.pace_cum, vec![10800, 21600, 32400, 43200, 54000]);
        assert_eq!(report.delta_secs, 23400 - 32400);
        assert_eq!(pace_label(&report), "2h 30m behind pace");

        let ahead = pace(&days, 3600, false, day("2026-03-04"));
        assert_eq!(ahead.delta_secs, 23400 - 10800);
        assert_eq!(pace_label(&ahead), "On pace, 3h 30m ahead");

        let future = pace(&days, 3600, false, day("2026-02-20"));
        assert!(future.worked_cum.is_empty());
        assert_eq!(pace_label(&future), "");
    }

    #[test]
    fn pace_counts_weekends_only_when_asked() {
        let days = daily_totals(&week_sample(), day("2026-03-02"), day("2026-03-08"));
        assert_eq!(pace(&days, 3600, true, day("2026-03-08")).pace_cum.len(), 7);
        assert_eq!(pace(&days, 3600, false, day("2026-03-08")).pace_cum.len(), 5);
    }

    #[test]
    fn pace_points_are_scaled_to_the_chart() {
        let report = PaceReport {
            days: vec![day("2026-03-02"), day("2026-03-03")],
            worked_cum: vec![1800],
            pace_cum: vec![3600, 7200],
            delta_secs: -1800,
        };
        let (worked, pace) = pace_points(&report);
        assert_eq!(pace, vec![(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)]);
        assert_eq!(worked, vec![(0.0, 0.0), (0.5, 0.25)]);
        assert_eq!(pace_points(&PaceReport { days: vec![], worked_cum: vec![], pace_cum: vec![], delta_secs: 0 }).1.len(), 0);
    }

    #[test]
    fn line_and_area_paths() {
        let pts = [(0.0, 0.0), (0.5, 0.25), (1.0, 1.0)];
        assert_eq!(line_path(&pts), "M 0.0 1000.0 L 500.0 750.0 L 1000.0 0.0");
        assert_eq!(area_path(&pts), "M 0.0 1000.0 L 500.0 750.0 L 1000.0 0.0 L 1000.0 1000 L 0.0 1000 Z");
        assert_eq!(line_path(&pts[..1]), "");
        assert_eq!(area_path(&[]), "");
    }

    #[test]
    fn time_ratio_shares_add_up() {
        let days = daily_totals(&week_sample(), day("2026-03-02"), day("2026-03-08"));
        let r = time_ratio(&days);
        let total = (7200 + 10800 + 1800 + 3600 + 3600) as f32;
        assert!((r.focus - 21600.0 / total).abs() < 1e-6);
        assert!((r.planning - 1800.0 / total).abs() < 1e-6);
        assert!((r.focus + r.planning + r.other - 1.0).abs() < 1e-5);
        assert_eq!(time_ratio(&[]).total_secs, 0);
    }

    #[test]
    fn bars_normalise_against_the_maximum() {
        assert_eq!(normalise(&[0, 50, 100], 100), vec![0.0, 0.5, 1.0]);
        assert_eq!(normalise(&[5], 0), vec![0.0]);
        assert_eq!(normalise(&[200], 100), vec![1.0]);
    }

    #[test]
    fn heat_levels_follow_the_goal() {
        assert_eq!(heat_level(0, 3600), 0);
        assert_eq!(heat_level(60, 3600), 1);
        assert_eq!(heat_level(900, 3600), 2);
        assert_eq!(heat_level(1800, 3600), 3);
        assert_eq!(heat_level(2700, 3600), 3);
        assert_eq!(heat_level(3600, 3600), 4);
        assert_eq!(heat_level(60, 0), 1);
    }

    #[test]
    fn heatmap_is_aligned_to_the_week_start_and_marks_the_future() {
        let spans = week_sample();
        let cells = heatmap(&spans, day("2026-03-04"), day("2026-03-04"), WeekStart::Monday, 2, 3 * 3600);
        assert_eq!(cells.len(), 14);
        assert_eq!(cells[0].date, day("2026-02-23"));
        assert_eq!(cells[7].date, day("2026-03-02"));
        assert_eq!(cells[7].level, 3); // 2h of a 3h goal
        assert_eq!(cells[9].level, 2); // 1h of 3h
        assert!(cells[10].future && cells[10].level == 0);
        assert!(!cells[9].future);

        let sunday_start = heatmap(&spans, day("2026-03-04"), day("2026-03-04"), WeekStart::Sunday, 1, 3600);
        assert_eq!(sunday_start[0].date, day("2026-03-01"));
        let saturday_start = heatmap(&spans, day("2026-03-04"), day("2026-03-04"), WeekStart::Saturday, 1, 3600);
        assert_eq!(saturday_start[0].date, day("2026-02-28"));
    }

    fn worked_days(dates: &[&str]) -> Vec<DayTotals> {
        let mut days: Vec<DayTotals> = dates
            .iter()
            .map(|d| {
                let mut t = DayTotals::new(day(d));
                t.focus = 3600;
                t
            })
            .collect();
        days.sort_by_key(|d| d.date);
        days
    }

    #[test]
    fn streak_counts_back_from_today() {
        let days = worked_days(&["2026-03-02", "2026-03-03", "2026-03-04"]);
        assert_eq!(streak(&days, day("2026-03-04"), false), 3);
        // Today still empty: the run up to yesterday keeps counting.
        assert_eq!(streak(&days, day("2026-03-05"), false), 3);
        // A gap breaks it.
        assert_eq!(streak(&days, day("2026-03-06"), false), 0);
        // Days under ten minutes do not count.
        let mut short = days.clone();
        short[2].focus = 300;
        assert_eq!(streak(&short, day("2026-03-04"), false), 2);
    }

    #[test]
    fn streak_skips_weekends_unless_they_count() {
        let days = worked_days(&["2026-02-26", "2026-02-27", "2026-03-02"]);
        assert_eq!(streak(&days, day("2026-03-02"), false), 3);
        assert_eq!(streak(&days, day("2026-03-02"), true), 1);
    }

    #[test]
    fn tasks_worked_ranks_by_time_and_ignores_breaks() {
        let mut spans = week_sample();
        spans.push(span(TimeEntryKind::Break, Some("a"), "2026-03-05 10:00", "2026-03-05 12:00"));
        let ranked = tasks_worked(&spans, day("2026-03-02"), day("2026-03-08"));
        assert_eq!(ranked, vec![("a".to_string(), 10800), ("b".to_string(), 10800)]);
    }

    #[test]
    fn tasks_worked_respects_the_date_range() {
        let spans = week_sample();
        let only_monday = tasks_worked(&spans, day("2026-03-02"), day("2026-03-02"));
        assert_eq!(only_monday, vec![("a".to_string(), 7200)]);
    }

    #[test]
    fn distribution_finds_the_best_hour_day_and_month() {
        let d = Distribution::from_spans(&week_sample());
        // Hour 9 holds Monday 1h and Tuesday 1h; hour 10 holds 2h.
        assert_eq!(d.hours[9], 7200);
        assert_eq!(d.hours[10], 7200);
        assert_eq!(d.best_hour(), Some(9)); // ties go to the earlier hour
        assert_eq!(d.best_weekday(), Some(1)); // Tuesday: 3h30m of work
        assert_eq!(d.best_month(), Some(2)); // March
        assert_eq!(weekday_name(1), "Tuesday");
        assert_eq!(month_name(2), "March");
        assert_eq!(hour_label(9), "09:00 to 10:00");
        assert_eq!(hour_label(23), "23:00 to 00:00");
        assert_eq!(Distribution::from_spans(&[]).best_hour(), None);
    }

    #[test]
    fn time_by_list_groups_tasks_and_keeps_untracked_time_apart() {
        let spans = week_sample();
        let shares = time_by_list(&spans, day("2026-03-02"), day("2026-03-08"), |t| match t {
            "a" => Some("work".to_string()),
            "b" => Some("home".to_string()),
            _ => None,
        });
        assert_eq!(shares[0], ListShare { list_id: Some("home".to_string()), secs: 10800 });
        assert_eq!(shares.iter().find(|s| s.list_id.as_deref() == Some("work")).unwrap().secs, 7200 + 3600);
        assert_eq!(shares.iter().find(|s| s.list_id.is_none()).unwrap().secs, 1800 + 3600);
        assert_eq!(shares.len(), 3);
    }

    #[test]
    fn on_time_ratio_uses_deadlines_then_estimates() {
        let mut by_deadline = Task::new("l", "a", 0, TaskSection::Today);
        by_deadline.deadline = Some(day("2026-03-10"));
        by_deadline.completed_at = to_utc(dt("2026-03-09 10:00"));
        let mut late = Task::new("l", "b", 0, TaskSection::Today);
        late.deadline = Some(day("2026-03-01"));
        late.completed_at = to_utc(dt("2026-03-05 10:00"));
        let mut within = Task::new("l", "c", 60, TaskSection::Today);
        within.time_taken_secs = 3000;
        let mut over = Task::new("l", "d", 30, TaskSection::Today);
        over.time_taken_secs = 3000;
        let unmeasured = Task::new("l", "e", 0, TaskSection::Today);
        let ratio = on_time_ratio(&[&by_deadline, &late, &within, &over, &unmeasured]);
        assert_eq!(ratio, OnTime { on_time: 2, measured: 4 });
        assert_eq!(ratio.label(), "50% (2 of 4)");
        assert_eq!(on_time_ratio(&[&unmeasured]).label(), "");
    }

    #[test]
    fn donut_arcs_start_at_the_top_and_run_clockwise() {
        let arcs = donut_geometry(&[0.25, 0.75, 0.0005]);
        let first = arcs[0].unwrap();
        assert!((first.x0 - 500.0).abs() < 0.01 && (first.y0 - 100.0).abs() < 0.01);
        assert!((first.x1 - 900.0).abs() < 0.01 && (first.y1 - 500.0).abs() < 0.01); // a quarter turn: 3 o'clock
        assert!(!first.large);
        let second = arcs[1].unwrap();
        assert!((second.x0 - 900.0).abs() < 0.01);
        assert!(second.large);
        assert!(arcs[2].is_none());
        // A single full slice stays drawable.
        assert!(donut_geometry(&[1.0])[0].is_some());
    }

    #[test]
    fn ranges_and_weeks() {
        let today = day("2026-03-04");
        assert_eq!(range_bounds("7d", today, None), (day("2026-02-26"), today));
        assert_eq!(range_bounds("30d", today, None).0, day("2026-02-03"));
        assert_eq!(range_bounds("90d", today, None).0, day("2025-12-05"));
        let custom = Some((day("2026-03-03"), day("2026-02-01")));
        assert_eq!(range_bounds("custom", today, custom), (day("2026-02-01"), day("2026-03-03")));
        assert_eq!(range_bounds("custom", today, None), range_bounds("7d", today, None));
        let huge = range_bounds("custom", today, Some((day("2000-01-01"), today)));
        assert_eq!((huge.1 - huge.0).num_days() + 1, MAX_RANGE_DAYS);

        assert_eq!(week_bounds(today, WeekStart::Monday, 0), (day("2026-03-02"), day("2026-03-08")));
        assert_eq!(week_bounds(today, WeekStart::Sunday, 0), (day("2026-03-01"), day("2026-03-07")));
        assert_eq!(week_bounds(today, WeekStart::Monday, -1).0, day("2026-02-23"));
        assert_eq!(week_label(0, today, today), "This week");
        assert_eq!(week_label(-1, today, today), "Last week");
        assert_eq!(week_label(3, day("2026-03-23"), day("2026-03-29")), "23 Mar to 29 Mar");
    }

    #[test]
    fn parsing_and_formatting() {
        assert_eq!(parse_date("2026-03-04"), Some(day("2026-03-04")));
        assert_eq!(parse_date("04/03/2026"), None);
        assert!(parse_hhmm("9:05").is_some());
        assert!(parse_hhmm("25:00").is_none());
        assert_eq!(parse_date_time("2026-03-04 09:30"), Some(dt("2026-03-04 09:30")));
        assert_eq!(format_duration(45 * 60), "45m");
        assert_eq!(format_duration(125 * 60), "2h 05m");
        assert_eq!(format_duration(0), "0m");
        assert_eq!(TaskFilter::parse("done"), TaskFilter::Done);
        assert!(TaskFilter::InProgress.matches(TaskStatus::Waiting));
        assert!(!TaskFilter::InProgress.matches(TaskStatus::Completed));
        assert!(TaskFilter::All.matches(TaskStatus::Cancelled));
    }

    #[test]
    fn csv_quotes_awkward_fields() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        let records = vec![
            SessionRecord {
                id: "2".into(),
                title: "Later, one".into(),
                list_name: "Work".into(),
                start: dt("2026-03-03 10:00"),
                end: dt("2026-03-03 10:30"),
                duration_secs: 1800,
                kind: TimeEntryKind::Focus,
            },
            SessionRecord {
                id: "1".into(),
                title: "Break".into(),
                list_name: String::new(),
                start: dt("2026-03-02 09:00"),
                end: dt("2026-03-02 09:05"),
                duration_secs: 300,
                kind: TimeEntryKind::Break,
            },
        ];
        let csv = sessions_csv(&records);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "Title,List,Type,Date,Start,End,Minutes");
        assert_eq!(lines[1], "Break,,break,2026-03-02,09:00,09:05,5");
        assert_eq!(lines[2], "\"Later, one\",Work,focus,2026-03-03,10:00,10:30,30");
    }

    #[test]
    fn pdf_has_a_valid_structure() {
        let lines: Vec<String> = (0..120).map(|i| format!("Line {i} (with) \\ and caf\u{e9}")).collect();
        let pdf = simple_pdf("Report", &lines);
        let text = String::from_utf8_lossy(&pdf).to_string();
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.trim_end().ends_with("%%EOF"));
        assert!(text.contains("/Count 3"));
        assert!(text.contains("Line 5 \\(with\\) \\\\ and caf?"));
        // The xref offset points at the xref table and every object offset at its header.
        let xref_at: usize = text.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        assert!(text[xref_at..].starts_with("xref"));
        let table: Vec<&str> = text[xref_at..].lines().skip(3).take(4 + 6).collect();
        for (i, row) in table.iter().enumerate() {
            let off: usize = row[..10].parse().unwrap();
            assert!(text[off..].starts_with(&format!("{} 0 obj", i + 1)), "object {}", i + 1);
        }
        assert!(simple_pdf("Empty", &[]).len() > 100);
    }

    // ----- store level -----

    struct MemoryRepo;
    impl StorageRepository for MemoryRepo {
        fn load_with_report(&self) -> LoadReport {
            LoadReport { store: AppStateStore::default(), recovery: None }
        }
        fn save(&self, _: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
    }

    fn service() -> (InsightsService, StoreHandle, String) {
        let store = AppStateStore::with_space();
        let list_id = store.lists[0].id.clone();
        let handle = StoreHandle::new(Arc::new(Mutex::new(store)), Arc::new(MemoryRepo));
        (InsightsService::new(handle.clone()), handle, list_id)
    }

    #[test]
    fn manual_sessions_attach_to_a_task_and_show_in_the_report() {
        let (svc, handle, list) = service();
        let id = svc.add_session("Write docs", &list, dt("2026-03-03 10:00"), dt("2026-03-03 11:30"), false).unwrap();
        svc.add_session("write docs", &list, dt("2026-03-04 10:00"), dt("2026-03-04 10:30"), false).unwrap();
        svc.add_session("", &list, dt("2026-03-04 10:30"), dt("2026-03-04 10:40"), true).unwrap();

        // One task, reused by title; its time is the sum.
        let (tasks, secs) = handle.read(|s| (s.tasks.len(), s.tasks[0].time_taken_secs));
        assert_eq!((tasks, secs), (1, 5400 + 1800));

        let report = svc.report(day("2026-03-02"), day("2026-03-08"), None);
        assert_eq!(report.totals.worked_secs, 7200);
        assert_eq!(report.totals.work_days, 2);
        assert_eq!(report.totals.tasks_worked, 1);
        assert_eq!(report.totals.avg_secs_per_task, 7200);
        assert_eq!(report.totals.break_secs, 600);
        assert_eq!(report.sessions.len(), 3);
        assert_eq!(report.sessions[0].title, "Break");
        assert_eq!(report.by_list.len(), 1);
        assert_eq!(report.by_list[0].name, "Personal");
        assert_eq!(report.best_weekday, Some(1));
        assert!(svc.find_record(&id).is_some());
        assert_eq!(svc.list_id_of(&id).as_deref(), Some(list.as_str()));

        // A list filter that matches nothing leaves an empty report.
        assert_eq!(svc.report(day("2026-03-02"), day("2026-03-08"), Some("other")).sessions.len(), 0);
        assert!(report.text_lines().iter().any(|l| l.contains("Write docs")));
    }

    #[test]
    fn manual_sessions_are_validated() {
        let (svc, _, list) = service();
        assert!(svc.add_session("x", &list, dt("2026-03-03 11:00"), dt("2026-03-03 10:00"), false).is_err());
        assert!(svc.add_session("  ", &list, dt("2026-03-03 10:00"), dt("2026-03-03 11:00"), false).is_err());
        assert!(svc.add_session("x", &list, dt("2026-03-03 10:00"), dt("2026-03-05 11:00"), false).is_err());
    }

    #[test]
    fn editing_a_session_moves_time_between_tasks() {
        let (svc, handle, list) = service();
        let id = svc.add_session("One", &list, dt("2026-03-03 10:00"), dt("2026-03-03 11:00"), false).unwrap();
        svc.update_session(&id, Some("One"), Some(&list), dt("2026-03-03 10:00"), dt("2026-03-03 10:30")).unwrap();
        assert_eq!(handle.read(|s| s.tasks[0].time_taken_secs), 1800);

        svc.update_session(&id, Some("Two"), Some(&list), dt("2026-03-03 10:00"), dt("2026-03-03 10:45")).unwrap();
        handle.read(|s| {
            assert_eq!(s.tasks.len(), 2);
            assert_eq!(s.tasks[0].time_taken_secs, 0);
            assert_eq!(s.tasks[1].time_taken_secs, 2700);
            assert_eq!(s.time_entries[0].task_id.as_deref(), Some(s.tasks[1].id.as_str()));
        });
        assert!(svc.update_session("missing", None, None, dt("2026-03-03 10:00"), dt("2026-03-03 10:45")).is_err());
        assert!(svc.update_session(&id, None, None, dt("2026-03-03 10:45"), dt("2026-03-03 10:00")).is_err());

        // Times only: names stay.
        svc.update_session(&id, None, None, dt("2026-03-03 09:00"), dt("2026-03-03 10:00")).unwrap();
        assert_eq!(svc.find_record(&id).unwrap().title, "Two");
        assert_eq!(handle.read(|s| s.tasks[1].time_taken_secs), 3600);
    }

    #[test]
    fn deleting_a_session_removes_its_time() {
        let (svc, handle, list) = service();
        let id = svc.add_session("One", &list, dt("2026-03-03 10:00"), dt("2026-03-03 11:00"), false).unwrap();
        assert!(svc.delete_session(&id));
        assert!(!svc.delete_session(&id));
        handle.read(|s| {
            assert!(s.time_entries.is_empty());
            assert_eq!(s.tasks[0].time_taken_secs, 0);
        });
    }

    #[test]
    fn finished_sessions_without_entries_count_once() {
        let (svc, handle, _) = service();
        let started = to_utc(dt("2026-03-03 09:00")).unwrap();
        let (linked_id, lonely_id) = handle.mutate(|st| {
            let mut linked = FocusSession::new(3600, None);
            linked.started_at = started;
            linked.finish(3600);
            let mut lonely = FocusSession::new(3600, None);
            lonely.started_at = to_utc(dt("2026-03-03 13:00")).unwrap();
            lonely.finish(1800);
            let mut entry = TimeEntry::new(TimeEntryKind::Focus, started, started + Duration::hours(1));
            entry.session_id = Some(linked.id.clone());
            let ids = (linked.id.clone(), lonely.id.clone());
            st.time_entries.push(entry);
            st.focus_sessions.push(linked);
            st.focus_sessions.push(lonely);
            ids
        });
        let report = svc.report(day("2026-03-03"), day("2026-03-03"), None);
        assert_eq!(report.totals.worked_secs, 3600 + 1800);
        assert_eq!(report.sessions.len(), 2);

        // Editing and deleting a session-only block works on the session itself.
        svc.update_session(&lonely_id, None, None, dt("2026-03-03 13:00"), dt("2026-03-03 14:00")).unwrap();
        assert_eq!(svc.report(day("2026-03-03"), day("2026-03-03"), None).totals.worked_secs, 7200);
        assert!(svc.delete_session(&lonely_id));
        // Deleting the entry also drops the session it came from.
        let entry_id = handle.read(|s| s.time_entries[0].id.clone());
        assert!(svc.delete_session(&entry_id));
        assert_eq!(svc.report(day("2026-03-03"), day("2026-03-03"), None).totals.worked_secs, 0);
        assert!(handle.read(|s| s.focus_sessions.iter().all(|f| f.id != linked_id)));
    }

    #[test]
    fn insights_for_a_week() {
        let (svc, handle, list) = service();
        for (title, start, end) in [
            ("Plan", "2026-03-02 09:00", "2026-03-02 11:00"),
            ("Build", "2026-03-03 09:00", "2026-03-03 10:00"),
            ("Build", "2026-03-04 09:00", "2026-03-04 09:30"),
        ] {
            svc.add_session(title, &list, dt(start), dt(end), false).unwrap();
        }
        handle.mutate(|st| {
            let id = st.tasks.iter().find(|t| t.title == "Plan").unwrap().id.clone();
            st.task_mut(&id).unwrap().complete();
            st.task_mut(&id).unwrap().completed_at = to_utc(dt("2026-03-03 12:00"));
            st.preferences.daily_goal_mins = 120;
            st.preferences.count_weekends = false;
        });
        let data = svc.insights(0, None, TaskFilter::All, day("2026-03-04"));
        assert_eq!((data.from, data.to), (day("2026-03-02"), day("2026-03-08")));
        assert_eq!(data.worked_secs, 3 * 3600 + 1800);
        assert_eq!(data.bars.len(), 5);
        assert_eq!(data.pace.worked_cum.len(), 3);
        assert_eq!(data.avg_secs, (3 * 3600 + 1800) / 3);
        assert_eq!(data.heat.len(), 35);
        assert_eq!(data.streak, 3);
        assert_eq!(data.activity.len(), 3);
        assert_eq!(data.worked_tasks.len(), 2);
        assert_eq!(data.tasks_done, 1);
        assert_eq!(svc.insights(0, None, TaskFilter::Done, day("2026-03-04")).worked_tasks.len(), 1);
        assert_eq!(svc.insights(0, None, TaskFilter::InProgress, day("2026-03-04")).worked_tasks.len(), 1);
        assert_eq!(svc.insights(-1, None, TaskFilter::All, day("2026-03-04")).worked_secs, 0);
    }
}
