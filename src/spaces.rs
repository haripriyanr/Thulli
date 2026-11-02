//! Builds the Spaces screen models: the space list, the board columns, the
//! matrix quadrants and the calendar week. A space is a saved task list.
//! Nothing here writes; it only reads the state it is given.

use chrono::{Duration as ChronoDuration, Local, NaiveDate};
use slint::Color;
use thulli_core::models::{AppStateStore, MatrixQuadrant, Task, TaskSection};
use thulli_core::services::tasks::calendar::{self, CalendarView};
use thulli_core::services::tasks::matrix;
use thulli_core::services::tasks::{BoardScope, BoardView};

use crate::{CalendarDay, SpaceColumn, SpaceRow, SpaceStep, SpaceTask};

/// Colour used for "All spaces" and for a space whose colour cannot be read.
const FALLBACK_COLOR: Color = Color::from_rgb_u8(0xA7, 0x8B, 0xFA);

/// Parses "#RRGGBB". Anything else gives the fallback colour.
pub fn hex_color(hex: &str) -> Color {
    let digits = hex.trim().trim_start_matches('#');
    if digits.len() != 6 {
        return FALLBACK_COLOR;
    }
    match u32::from_str_radix(digits, 16) {
        Ok(v) => Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8),
        Err(_) => FALLBACK_COLOR,
    }
}

/// Every space that is not archived, in saved order, with its open tasks.
pub fn space_rows(store: &AppStateStore) -> Vec<SpaceRow> {
    let mut lists: Vec<_> = store.lists.iter().filter(|l| !l.is_archived).collect();
    lists.sort_by_key(|l| l.sort_order);
    lists
        .into_iter()
        .map(|list| SpaceRow {
            id: list.id.clone().into(),
            name: list.name.clone().into(),
            color: hex_color(&list.color),
            hex: list.color.clone().into(),
            letter: list.badge_letter().into(),
            count: store.count_pending_for_list(&list.id) as i32,
        })
        .collect()
}

/// One task as a card.
pub fn card(store: &AppStateStore, task: &Task) -> SpaceTask {
    let list = store.lists.iter().find(|l| l.id == task.list_id);
    // Dates only: the card shows the estimate on its own.
    let mut meta = Vec::new();
    if let Some(at) = task.scheduled_at {
        meta.push(at.with_timezone(&Local).format("%a %-d %b, %H:%M").to_string());
    } else if let Some(day) = task.do_date {
        meta.push(day.format("%a %-d %b").to_string());
    }
    if let Some(deadline) = task.deadline {
        meta.push(format!("Due {}", deadline.format("%-d %b")));
    }
    let steps: Vec<SpaceStep> = store
        .steps_of(&task.id)
        .into_iter()
        .map(|step| SpaceStep { id: step.id.clone().into(), title: step.title.clone().into(), done: step.is_completed() })
        .collect();
    let iso = |d: Option<NaiveDate>| d.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
    SpaceTask {
        id: task.id.clone().into(),
        title: task.title.clone().into(),
        meta: meta.join("  ·  ").into(),
        done: task.is_completed(),
        section: task.section.as_str().into(),
        quadrant: task.matrix_quadrant.map(|q| q.as_str()).unwrap_or("").into(),
        color: list.map(|l| hex_color(&l.color)).unwrap_or(FALLBACK_COLOR),
        letter: list.map(|l| l.badge_letter()).unwrap_or_default().into(),
        notes: task.notes.clone().into(),
        estimate: task.estimate_mins as i32,
        do_date: iso(task.do_date).into(),
        deadline: iso(task.deadline).into(),
        steps_done: steps.iter().filter(|s| s.done).count() as i32,
        steps: crate::convert::model(steps),
        header: "".into(),
    }
}

/// Cards for finished tasks, newest first, with a heading on the first card
/// of each day.
fn done_cards(store: &AppStateStore, tasks: &[Task]) -> Vec<SpaceTask> {
    let mut last = String::new();
    tasks
        .iter()
        .map(|task| {
            let mut c = card(store, task);
            let day = task
                .completed_at
                .map(|at| at.with_timezone(&Local).format("%a, %-d %b %Y").to_string())
                .unwrap_or_else(|| "Earlier".to_string());
            if day != last {
                c.header = day.clone().into();
                last = day;
            }
            c
        })
        .collect()
}

fn column(store: &AppStateStore, id: &str, title: &str, caption: &str, done: usize, total: usize, tasks: &[Task]) -> SpaceColumn {
    SpaceColumn {
        id: id.into(),
        title: title.into(),
        caption: caption.into(),
        done: done as i32,
        total: total as i32,
        tasks: crate::convert::model(tasks.iter().map(|t| card(store, t)).collect()),
    }
}

/// Which tasks the board shows: one space, or every space for "".
pub fn board_scope(space: &str) -> BoardScope {
    if space.is_empty() { BoardScope::All } else { BoardScope::List(space.to_string()) }
}

/// Backlog, This Week, Today and Done as card columns.
pub fn board(store: &AppStateStore, view: &BoardView) -> Vec<SpaceColumn> {
    view.columns
        .iter()
        .map(|c| {
            let mut col = column(store, c.section, c.title, "", c.done, c.total, &c.tasks);
            if c.section == "done" {
                col.total = c.tasks.len() as i32;
                col.tasks = crate::convert::model(done_cards(store, &c.tasks));
            }
            col
        })
        .collect()
}

/// The four quadrants and the unsorted groups.
pub fn matrix_columns(store: &AppStateStore, space: &str) -> (Vec<SpaceColumn>, Vec<SpaceColumn>, usize) {
    let view = matrix::build(store, space);
    let quadrant = |q: MatrixQuadrant, title: &str, caption: &str, tasks: &[Task]| {
        column(store, q.as_str(), title, caption, 0, tasks.len(), tasks)
    };
    let quadrants = vec![
        quadrant(MatrixQuadrant::DoAsap, "Do ASAP", "Today · Urgent and important", &view.do_asap),
        quadrant(MatrixQuadrant::DoLaterToday, "Do Later Today", "Today · Important, after the first push", &view.do_later_today),
        quadrant(MatrixQuadrant::DoMidWeek, "Do Mid Week", "This Week · Important, not today", &view.do_mid_week),
        quadrant(MatrixQuadrant::LeaveTillEnd, "Leave Till End", "Backlog · Low urgency or low value", &view.leave_till_end),
    ];
    let unsorted = vec![
        column(store, "today", "Today", "", 0, view.unsorted_today.len(), &view.unsorted_today),
        column(store, "this_week", "This Week", "", 0, view.unsorted_this_week.len(), &view.unsorted_this_week),
        column(store, "backlog", "Backlog", "", 0, view.unsorted_backlog.len(), &view.unsorted_backlog),
    ];
    (quadrants, unsorted, view.unsorted_count())
}

/// The week that contains `anchor`: its heading, its seven days and the
/// tasks that have no day yet.
pub fn calendar_week(
    store: &AppStateStore,
    space: &str,
    anchor: NaiveDate,
    today: NaiveDate,
) -> (String, Vec<CalendarDay>, Vec<SpaceColumn>) {
    let week_start = store.preferences.week_start;
    let first = calendar::range_start(CalendarView::Week, anchor, week_start);
    let data = calendar::build(store, CalendarView::Week, anchor, space);
    let days = (0..7)
        .map(|i| {
            let date = first + ChronoDuration::days(i);
            let mut items: Vec<_> = data.items.iter().filter(|item| item.day_index == i as i32).collect();
            // Whole-day tasks first, then by time of day.
            items.sort_by_key(|item| item.start_mins);
            let tasks: Vec<SpaceTask> = items
                .iter()
                .filter_map(|item| store.task(&item.task_id))
                .map(|task| card(store, task))
                .collect();
            CalendarDay {
                name: date.format("%a").to_string().to_uppercase().into(),
                number: date.format("%-d").to_string().into(),
                today: date == today,
                tasks: crate::convert::model(tasks),
            }
        })
        .collect();
    let unscheduled = vec![
        column(store, "today", "Today", "", 0, data.unscheduled_today.len(), &data.unscheduled_today),
        column(store, "this_week", "This Week", "", 0, data.unscheduled_this_week.len(), &data.unscheduled_this_week),
        column(store, "backlog", "Backlog", "", 0, data.unscheduled_backlog.len(), &data.unscheduled_backlog),
    ];
    (calendar::range_label(CalendarView::Week, anchor, week_start), days, unscheduled)
}

/// The day `index` (0 to 6) of the week containing `anchor`.
pub fn week_day(store: &AppStateStore, anchor: NaiveDate, index: i32) -> NaiveDate {
    calendar::range_start(CalendarView::Week, anchor, store.preferences.week_start) + ChronoDuration::days(index as i64)
}

/// Parses a section id from the screen.
pub fn section(id: &str) -> Option<TaskSection> {
    TaskSection::parse(id).or_else(|| crate::convert::section_from_id(id))
}
