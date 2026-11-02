//! Converts saved domain data into the row and bar models the screens show.
//! Nothing here writes; it only reads the state it is given.

use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, NaiveDateTime, Timelike};
use slint::{ModelRc, VecModel};
use thulli_core::models::{AppStateStore, Note, Task, TaskSection, TimeEntry, TimeEntryKind};
use thulli_core::services::insights::{self, format_duration};
use thulli_core::services::notes::preview_of;

use crate::{DayBar, NoteRow, SessionRow, TaskOption, TaskRow};
use thulli_core::models::{MatrixQuadrant, TaskStatus};

/// Wraps rows in the model type Slint expects.
pub fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// Identifier used for a section at the UI boundary.
pub fn section_id(section: TaskSection) -> &'static str {
    match section {
        TaskSection::Backlog => "backlog",
        TaskSection::ThisWeek => "week",
        TaskSection::Today => "today",
    }
}

/// Parses a section identifier from the UI boundary.
pub fn section_from_id(id: &str) -> Option<TaskSection> {
    match id {
        "backlog" => Some(TaskSection::Backlog),
        "week" => Some(TaskSection::ThisWeek),
        "today" => Some(TaskSection::Today),
        _ => None,
    }
}

/// One top-level task as a row. Subtasks are shown under their parent elsewhere.
pub fn task_row(store: &AppStateStore, task: &Task) -> TaskRow {
    let list_name = store
        .lists
        .iter()
        .find(|list| list.id == task.list_id)
        .map(|list| list.name.clone())
        .unwrap_or_default();
    let meta = if task.estimate_mins > 0 {
        format!("{list_name}  ·  {} min", task.estimate_mins)
    } else {
        list_name
    };
    TaskRow {
        id: task.id.clone().into(),
        title: task.title.clone().into(),
        meta: meta.into(),
        done: task.is_completed(),
        section: section_id(task.section).into(),
    }
}

/// Rows for top-level tasks that match `keep`, in saved order.
pub fn task_rows(store: &AppStateStore, keep: impl Fn(&Task) -> bool) -> Vec<TaskRow> {
    store
        .tasks
        .iter()
        .filter(|task| task.parent_id.is_none() && keep(task))
        .map(|task| task_row(store, task))
        .collect()
}

pub fn note_row(note: &Note) -> NoteRow {
    let title = if note.title.trim().is_empty() {
        "Untitled".to_string()
    } else {
        note.title.clone()
    };
    NoteRow {
        id: note.id.clone().into(),
        title: title.into(),
        preview: preview_of(&note.body).into(),
        updated: thulli_core::services::notes::format_stamp(
            note.updated_at.with_timezone(&Local),
            Local::now(),
        )
        .into(),
    }
}

/// Completed focus sessions per day for the seven days ending on `today`.
pub fn week_bars(store: &AppStateStore, today: NaiveDate) -> Vec<DayBar> {
    let from = today - ChronoDuration::days(6);
    let mut counts = [0u32; 7];
    for entry in store.time_entries.iter().filter(|e| e.kind == TimeEntryKind::Focus) {
        let day = entry.ended_at.with_timezone(&Local).date_naive();
        if day >= from && day <= today {
            counts[(day - from).num_days() as usize] += 1;
        }
    }
    let peak = counts.iter().copied().max().unwrap_or(0).max(1);
    counts
        .iter()
        .enumerate()
        .map(|(i, &count)| DayBar {
            label: (from + ChronoDuration::days(i as i64)).format("%a").to_string().into(),
            value: count as f32 / peak as f32,
            value_label: count.to_string().into(),
        })
        .collect()
}

/// The most recent focus sessions, newest first, with their saved names.
pub fn recent_sessions(store: &AppStateStore, limit: usize) -> Vec<SessionRow> {
    let mut focus: Vec<&TimeEntry> = store
        .time_entries
        .iter()
        .filter(|e| e.kind == TimeEntryKind::Focus)
        .collect();
    focus.sort_by(|a, b| b.ended_at.cmp(&a.ended_at));
    focus
        .into_iter()
        .take(limit)
        .map(|entry| {
            let title = if entry.label.trim().is_empty() {
                "Focus session".to_string()
            } else {
                entry.label.clone()
            };
            let when = entry.ended_at.with_timezone(&Local).format("%a %d %b, %H:%M");
            SessionRow {
                title: title.into(),
                detail: format!("{when}  ·  {}", format_duration(entry.duration_secs())).into(),
            }
        })
        .collect()
}

/// Focus seconds recorded on `today`.
pub fn focus_secs_on(store: &AppStateStore, today: NaiveDate) -> u64 {
    let spans = insights::collect_spans(store);
    spans
        .iter()
        .filter(|span| span.kind == TimeEntryKind::Focus)
        .map(|span| span.secs_on(today))
        .sum()
}

/// Focus seconds in the seven days ending on `today`.
pub fn focus_secs_this_week(store: &AppStateStore, today: NaiveDate) -> u64 {
    let spans = insights::collect_spans(store);
    let from = today - ChronoDuration::days(6);
    insights::total_worked(&insights::daily_totals(&spans, from, today))
}

/// Number of focus spans that started in the seven days ending on `today`.
pub fn focus_sessions_this_week(store: &AppStateStore, today: NaiveDate) -> usize {
    let from = today - ChronoDuration::days(6);
    insights::collect_spans(store)
        .iter()
        .filter(|span| span.kind == TimeEntryKind::Focus)
        .filter(|span| {
            let day = span.start.date();
            day >= from && day <= today
        })
        .count()
}

/// Consecutive days, ending today, that met the focus threshold.
pub fn focus_streak(store: &AppStateStore, today: NaiveDate) -> u32 {
    let spans = insights::collect_spans(store);
    let from = today - ChronoDuration::days(59);
    let totals = insights::daily_totals(&spans, from, today);
    insights::streak(&totals, today, store.preferences.count_weekends)
}

/// Focus activity for one range of the Pomodoro analytics panel.
pub struct Activity {
    pub bars: Vec<DayBar>,
    pub sessions: usize,
    pub focus_secs: u64,
    /// Label of the busiest slot, or empty when there are no sessions.
    pub best: String,
    /// Count of the busiest slot, used as the top of the chart axis.
    pub axis_max: u32,
}

/// Counts focus sessions into slots. `slot` maps a session's local end time to
/// a slot index, or `None` when it falls outside the range.
fn tally(
    focus: &[(NaiveDateTime, u64)],
    slots: usize,
    slot: impl Fn(NaiveDateTime) -> Option<usize>,
) -> (Vec<u32>, u64) {
    let mut counts = vec![0u32; slots];
    let mut secs = 0;
    for (ended, duration) in focus {
        if let Some(i) = slot(*ended) {
            if i < slots {
                counts[i] += 1;
                secs += duration;
            }
        }
    }
    (counts, secs)
}

fn activity_from(labels: Vec<(String, String)>, counts: Vec<u32>, secs: u64) -> Activity {
    let peak = counts.iter().copied().max().unwrap_or(0).max(1);
    let bars = labels
        .iter()
        .zip(&counts)
        .map(|((short, _), &count)| DayBar {
            label: short.clone().into(),
            value: count as f32 / peak as f32,
            value_label: count.to_string().into(),
        })
        .collect();
    let sessions = counts.iter().sum::<u32>() as usize;
    let (best_index, best_count) = counts
        .iter()
        .enumerate()
        .fold((0, 0), |acc, (i, &c)| if c > acc.1 { (i, c) } else { acc });
    let best = if best_count == 0 { String::new() } else { labels[best_index].1.clone() };
    Activity { bars, sessions, focus_secs: secs, best, axis_max: peak }
}

/// Focus sessions for the chosen range: "day" (two-hour blocks today),
/// "week" (last seven days), "month" (last thirty days) or "year" (last twelve
/// months). Any other value is treated as "week".
pub fn activity(store: &AppStateStore, today: NaiveDate, range: &str) -> Activity {
    let focus: Vec<(NaiveDateTime, u64)> = store
        .time_entries
        .iter()
        .filter(|e| e.kind == TimeEntryKind::Focus)
        .map(|e| (e.ended_at.with_timezone(&Local).naive_local(), e.duration_secs()))
        .collect();

    match range {
        "day" => {
            let labels = (0..12)
                .map(|i| {
                    let h = i * 2;
                    (format!("{h:02}"), format!("{h:02}:00 to {:02}:00", h + 2))
                })
                .collect();
            let (counts, secs) = tally(&focus, 12, |dt| {
                (dt.date() == today).then(|| (dt.hour() / 2) as usize)
            });
            activity_from(labels, counts, secs)
        }
        "month" => {
            let from = today - ChronoDuration::days(29);
            let labels = (0..30)
                .map(|i| {
                    let day = from + ChronoDuration::days(i as i64);
                    let short = if i % 5 == 0 { day.format("%d").to_string() } else { String::new() };
                    (short, day.format("%d %b").to_string())
                })
                .collect();
            let (counts, secs) = tally(&focus, 30, |dt| {
                let d = dt.date();
                (d >= from && d <= today).then(|| (d - from).num_days() as usize)
            });
            activity_from(labels, counts, secs)
        }
        "year" => {
            let now_total = today.year() * 12 + today.month0() as i32;
            let first_of = |total: i32| NaiveDate::from_ymd_opt(total / 12, (total % 12) as u32 + 1, 1);
            let labels = (0..12)
                .map(|i| {
                    let total = now_total - (11 - i);
                    match first_of(total) {
                        Some(day) => (day.format("%b").to_string(), day.format("%B %Y").to_string()),
                        None => (String::new(), String::new()),
                    }
                })
                .collect();
            let (counts, secs) = tally(&focus, 12, |dt| {
                let d = dt.date();
                let back = now_total - (d.year() * 12 + d.month0() as i32);
                (0..12).contains(&back).then(|| (11 - back) as usize)
            });
            activity_from(labels, counts, secs)
        }
        _ => {
            let from = today - ChronoDuration::days(6);
            let labels = (0..7)
                .map(|i| {
                    let day = from + ChronoDuration::days(i as i64);
                    (day.format("%a").to_string(), day.format("%A %d %b").to_string())
                })
                .collect();
            let (counts, secs) = tally(&focus, 7, |dt| {
                let d = dt.date();
                (d >= from && d <= today).then(|| (d - from).num_days() as usize)
            });
            activity_from(labels, counts, secs)
        }
    }
}

/// The task picker's rows: "None" first, then open tasks grouped as Today,
/// Upcoming, Overdue and No due date. Each group is sorted by saved order.
pub fn task_options(
    store: &AppStateStore,
    today: NaiveDate,
    search: &str,
    focus_mins: u32,
) -> Vec<TaskOption> {
    let needle = search.trim().to_lowercase();
    let headers = ["Today", "Upcoming", "Overdue", "No due date"];
    let group_of = |due: Option<NaiveDate>| match due {
        Some(day) if day == today => 0,
        Some(day) if day > today => 1,
        Some(_) => 2,
        None => 3,
    };

    let mut rows: Vec<(usize, i32, String, TaskOption)> = store
        .tasks
        .iter()
        .filter(|task| task.parent_id.is_none())
        .filter(|task| matches!(task.status, TaskStatus::Open | TaskStatus::Waiting))
        .filter(|task| needle.is_empty() || task.title.to_lowercase().contains(&needle))
        .map(|task| {
            let due = task.deadline.or(task.do_date);
            let group = group_of(due);
            let done = store
                .time_entries
                .iter()
                .filter(|e| {
                    e.kind == TimeEntryKind::Focus && e.completed && e.task_id.as_deref() == Some(task.id.as_str())
                })
                .count();
            let estimate = if task.estimate_mins > 0 {
                task.estimate_mins.div_ceil(focus_mins.max(1))
            } else {
                0
            };
            let sessions = if estimate > 0 {
                format!("{done} of {estimate} sessions")
            } else {
                format!("{done} sessions")
            };
            let priority = match task.matrix_quadrant {
                Some(MatrixQuadrant::DoAsap) => "high",
                Some(MatrixQuadrant::DoLaterToday) | Some(MatrixQuadrant::DoMidWeek) => "medium",
                Some(MatrixQuadrant::LeaveTillEnd) => "low",
                None => "",
            };
            let option = TaskOption {
                id: task.id.clone().into(),
                title: task.title.clone().into(),
                meta: sessions.into(),
                priority: priority.into(),
                header: "".into(),
            };
            (group, task.sort_order, task.title.to_lowercase(), option)
        })
        .collect();
    rows.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));

    let mut options = vec![TaskOption {
        id: "".into(),
        title: "None".into(),
        meta: "Focus only".into(),
        priority: "".into(),
        header: "".into(),
    }];
    let mut previous_group = None;
    for (group, _, _, mut option) in rows {
        if previous_group != Some(group) {
            option.header = headers[group].into();
            previous_group = Some(group);
        }
        options.push(option);
    }
    options
}
