//! Wiring for the calendar screen (ui/screens/calendar.slint): the unscheduled
//! rail, the day and week grids, scheduling by clicking a slot, and navigation.

use std::cell::Cell;
use std::rc::Rc;

use chrono::{Duration, Local, NaiveDate, Timelike};
use slint::{ComponentHandle, SharedString};
use thulli_core::services::tasks::calendar::{self, CalendarView};

use crate::app::context::AppContext;
use crate::app::convert::{hex_color, model_of, task_model};
use crate::{CalendarItemModel, CalendarState, MainWindow, TimeBlockModel};

/// Height of one hour on the grid, in logical pixels. Must match the Slint layout.
const HOUR_PX: f32 = 48.0;
/// Clicked slots snap to this many minutes.
const SNAP_MINS: i32 = 15;

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let anchor = Rc::new(Cell::new(Local::now().date_naive()));

    let refresh = {
        let (ctx, anchor, weak) = (ctx.clone(), anchor.clone(), window.as_weak());
        move || {
            if let Some(w) = weak.upgrade() {
                fill(&w, &ctx, anchor.get());
            }
        }
    };

    let state = window.global::<CalendarState>();

    {
        let r = refresh.clone();
        state.on_request_refresh(move || r());
    }

    {
        let (r, weak) = (refresh.clone(), window.as_weak());
        state.on_set_view(move |view| {
            if let Some(w) = weak.upgrade() {
                // The month view is not part of this slice; keep to day and week.
                if matches!(view.as_str(), "day" | "week") {
                    w.global::<CalendarState>().set_view(view);
                }
            }
            r();
        });
    }

    {
        let (anchor, r, weak) = (anchor.clone(), refresh.clone(), window.as_weak());
        state.on_previous(move || {
            if let Some(w) = weak.upgrade() {
                let view = CalendarView::parse(&w.global::<CalendarState>().get_view());
                anchor.set(calendar::shift(view, anchor.get(), -1));
            }
            r();
        });
    }

    {
        let (anchor, r, weak) = (anchor.clone(), refresh.clone(), window.as_weak());
        state.on_next(move || {
            if let Some(w) = weak.upgrade() {
                let view = CalendarView::parse(&w.global::<CalendarState>().get_view());
                anchor.set(calendar::shift(view, anchor.get(), 1));
            }
            r();
        });
    }

    {
        let (anchor, r) = (anchor.clone(), refresh.clone());
        state.on_go_today(move || {
            anchor.set(Local::now().date_naive());
            r();
        });
    }

    {
        let (ctx, anchor, r, weak) = (ctx.clone(), anchor.clone(), refresh.clone(), window.as_weak());
        state.on_slot_clicked(move |day, y| {
            let Some(w) = weak.upgrade() else { return };
            let state = w.global::<CalendarState>();
            let view = CalendarView::parse(&state.get_view());
            let first = ctx.handle.read(|st| calendar::range_start(view, anchor.get(), st.preferences.week_start));
            let date = first + Duration::days(i64::from(day));
            let minutes = calendar::snap_mins((y / HOUR_PX * 60.0).round() as i32, SNAP_MINS);
            let at = calendar::slot_to_utc(date, minutes);

            let armed = state.get_armed_task_id().to_string();
            if armed.is_empty() {
                ctx.tasks.create_scheduled(&ctx.history, "New task", "", at);
            } else {
                ctx.tasks.schedule_task(&ctx.history, &armed, Some(at));
                state.set_armed_task_id("".into());
            }
            r();
        });
    }

    {
        let (ctx, r) = (ctx.clone(), refresh.clone());
        state.on_unschedule(move |id| {
            ctx.tasks.schedule_task(&ctx.history, &id, None);
            r();
        });
    }

    {
        let (ctx, r) = (ctx.clone(), refresh.clone());
        state.on_undo(move || {
            ctx.history.undo();
            r();
        });
    }

    {
        let (ctx, r) = (ctx.clone(), refresh.clone());
        state.on_redo(move || {
            ctx.history.redo();
            r();
        });
    }

    refresh();
}

/// Rebuilds the grid, the rail and the heading for the anchor date.
fn fill(window: &MainWindow, ctx: &AppContext, anchor: NaiveDate) {
    let state = window.global::<CalendarState>();
    let view = CalendarView::parse(&state.get_view());
    let now = Local::now();
    let today = now.date_naive();

    let (grid, range_label) = ctx.handle.read(|st| {
        let ws = st.preferences.week_start;
        let first = calendar::range_start(view, anchor, ws);
        let data = calendar::build(st, view, anchor, "");
        let tracked = if view == CalendarView::Day { calendar::tracked_blocks(st, anchor) } else { Vec::new() };

        let items: Vec<CalendarItemModel> = data
            .items
            .iter()
            .map(|item| CalendarItemModel {
                id: item.task_id.clone().into(),
                task_id: item.task_id.clone().into(),
                title: item.title.clone().into(),
                day_index: item.day_index,
                start_mins: item.start_mins,
                length_mins: item.length_mins,
                color: hex_color(&item.color_hex),
            })
            .collect();
        let blocks: Vec<TimeBlockModel> = tracked
            .iter()
            .map(|b| TimeBlockModel {
                id: b.id.clone().into(),
                kind: b.kind.into(),
                label: b.label.clone().into(),
                start_mins: b.start_mins,
                length_mins: b.length_mins,
            })
            .collect();
        let labels: Vec<SharedString> = (0..view.day_count())
            .map(|i| (first + Duration::days(i)).format("%a %-d").to_string().into())
            .collect();

        let idx = (today - first).num_days();
        let now_day = if (0..view.day_count()).contains(&idx) { idx as i32 } else { -1 };
        let grid = Grid {
            labels,
            items,
            blocks,
            now_day,
            now_minutes: (now.hour() * 60 + now.minute()) as i32,
            unscheduled_today: data.unscheduled_today.iter().map(|t| task_model(st, t)).collect(),
            unscheduled_this_week: data.unscheduled_this_week.iter().map(|t| task_model(st, t)).collect(),
            unscheduled_backlog: data.unscheduled_backlog.iter().map(|t| task_model(st, t)).collect(),
        };
        (grid, calendar::range_label(view, anchor, ws))
    });

    state.set_range_label(range_label.into());
    state.set_day_labels(model_of(grid.labels));
    state.set_items(model_of(grid.items));
    state.set_tracked(model_of(grid.blocks));
    state.set_now_day(grid.now_day);
    state.set_now_minutes(grid.now_minutes);
    state.set_unscheduled_today(model_of(grid.unscheduled_today));
    state.set_unscheduled_this_week(model_of(grid.unscheduled_this_week));
    state.set_unscheduled_backlog(model_of(grid.unscheduled_backlog));
    state.set_can_undo(ctx.history.can_undo());
    state.set_can_redo(ctx.history.can_redo());
}

/// Everything the grid needs, converted to Slint models while the store is locked.
struct Grid {
    labels: Vec<SharedString>,
    items: Vec<CalendarItemModel>,
    blocks: Vec<TimeBlockModel>,
    now_day: i32,
    now_minutes: i32,
    unscheduled_today: Vec<crate::TaskModel>,
    unscheduled_this_week: Vec<crate::TaskModel>,
    unscheduled_backlog: Vec<crate::TaskModel>,
}
