//! Converts saved domain data into the row and bar models the screens show.
//! Nothing here writes; it only reads the state it is given.

use chrono::{Duration as ChronoDuration, Local, NaiveDate};
use slint::{ModelRc, VecModel};
use thulli_core::models::{AppStateStore, Note, Task, TaskSection, TimeEntryKind};
use thulli_core::services::insights::{self, format_duration};
use thulli_core::services::notes::preview_of;

use crate::{DayBar, NoteRow, TaskRow};

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

/// Focus seconds per day for the seven days ending on `today`.
pub fn week_bars(store: &AppStateStore, today: NaiveDate) -> Vec<DayBar> {
    let spans = insights::collect_spans(store);
    let from = today - ChronoDuration::days(6);
    let totals = insights::daily_totals(&spans, from, today);
    let peak = totals.iter().map(|day| day.focus).max().unwrap_or(0).max(1);
    totals
        .iter()
        .map(|day| DayBar {
            label: day.date.format("%a").to_string().into(),
            value: day.focus as f32 / peak as f32,
            value_label: format_duration(day.focus).into(),
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
