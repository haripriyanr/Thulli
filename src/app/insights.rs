//! Wiring for the insights screen (ui/screens/insights.slint).
//!
//! Thin layer: the numbers come from `thulli_core::services::insights`; this
//! file converts them to Slint models and handles the buttons.

use std::cell::RefCell;
use std::rc::Rc;

use chrono::Local;
use slint::ComponentHandle;
use thulli_core::services::insights::{
    area_path, format_duration, line_path, normalise, pace_label, pace_points, parse_hhmm, sessions_csv, simple_pdf, week_label, InsightsData,
    TaskFilter,
};

use crate::app::context::AppContext;
use crate::app::convert::task_model;
use crate::app::reports::{bar, list_choices, list_option, model, save_export, stat_tile};
use crate::{ActivityEntryModel, ChartPointModel, HeatCellModel, InsightsState, MainWindow};

struct View {
    week_offset: i32,
    list: String,
    task_filter: String,
}

impl View {
    fn data(&self, ctx: &AppContext) -> InsightsData {
        ctx.insights.insights(self.week_offset, list_option(&self.list), TaskFilter::parse(&self.task_filter), Local::now().date_naive())
    }
}

fn points(points: Vec<(f32, f32)>) -> Vec<ChartPointModel> {
    points.into_iter().map(|(x, y)| ChartPointModel { x, y }).collect()
}

fn share(fraction: f32) -> String {
    format!("{}%", (fraction * 100.0).round() as u32)
}

fn render(window: &MainWindow, ctx: &AppContext, view: &View) {
    let state = window.global::<InsightsState>();
    let data = view.data(ctx);

    state.set_period_label(week_label(view.week_offset, data.from, data.to).into());
    state.set_list_filter(view.list.as_str().into());
    state.set_task_filter(view.task_filter.as_str().into());
    state.set_lists(model(list_choices(ctx)));

    let (worked, pace) = pace_points(&data.pace);
    state.set_worked_path(line_path(&worked).into());
    state.set_worked_area_path(area_path(&worked).into());
    state.set_pace_path(line_path(&pace).into());
    state.set_worked_line(model(points(worked)));
    state.set_pace_line(model(points(pace)));
    state.set_pace_label(pace_label(&data.pace).into());

    let days_reached = data.pace.worked_cum.len();
    state.set_tiles(model(vec![
        stat_tile("Time worked", format_duration(data.worked_secs), format!("{} a day", format_duration(data.avg_secs))),
        stat_tile("Daily goal", format_duration(data.goal_secs), format!("{days_reached} of {} days so far", data.pace.days.len())),
        stat_tile("Tasks done", data.tasks_done.to_string(), "finished this week"),
    ]));

    // Per-day bars on one scale that also fits the goal.
    let max = data.bars.iter().map(|d| d.worked()).max().unwrap_or(0).max(data.goal_secs);
    state.set_workday_bars(model(data.bars.iter().map(|d| bar(d.date.format("%a").to_string(), d.worked(), max)).collect()));
    let goal = if data.goal_secs == 0 || max == 0 { -1.0 } else { normalise(&[data.goal_secs], max)[0] };
    state.set_goal_line(goal);

    state.set_heatmap(model(
        data.heat
            .iter()
            .map(|h| HeatCellModel {
                label: if h.future { String::new() } else { h.date.format("%a %d %b").to_string() }.into(),
                level: i32::from(h.level),
            })
            .collect(),
    ));
    state.set_streak_days(data.streak as i32);

    let r = &data.ratio;
    state.set_ratio_focus(r.focus);
    state.set_ratio_planning(r.planning);
    state.set_ratio_other(r.other);
    state.set_ratio_focus_label(share(r.focus).into());
    state.set_ratio_planning_label(share(r.planning).into());
    state.set_ratio_other_label(share(r.other).into());

    state.set_worked_tasks(model(ctx.handle.read(|s| {
        data.worked_tasks
            .iter()
            .filter_map(|w| s.task(&w.task_id).map(|t| (t, w.secs)))
            .map(|(t, secs)| {
                let mut m = task_model(s, t);
                // Show the time spent in this week, not the lifetime total.
                m.tracked_mins = (secs / 60) as i32;
                m
            })
            .collect()
    })));

    state.set_activity(model(
        data.activity
            .iter()
            .map(|r| ActivityEntryModel {
                id: r.id.clone().into(),
                task_title: r.title.clone().into(),
                day_label: r.start.format("%a %d %b").to_string().into(),
                time_label: format!("{} - {}", r.start.format("%H:%M"), r.end.format("%H:%M")).into(),
                duration_label: format_duration(r.duration_secs).into(),
                kind: r.kind.as_str().into(),
            })
            .collect(),
    ));
}

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let view = Rc::new(RefCell::new(View { week_offset: 0, list: String::new(), task_filter: "all".into() }));

    let refresh: Rc<dyn Fn()> = {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        let view = view.clone();
        Rc::new(move || {
            if let Some(w) = weak.upgrade() {
                render(&w, &ctx, &view.borrow());
            }
        })
    };
    let state = window.global::<InsightsState>();

    state.on_refresh({
        let refresh = refresh.clone();
        move || refresh()
    });

    state.on_previous_period({
        let (view, refresh) = (view.clone(), refresh.clone());
        move || {
            view.borrow_mut().week_offset -= 1;
            refresh();
        }
    });

    state.on_next_period({
        let (view, refresh) = (view.clone(), refresh.clone());
        move || {
            view.borrow_mut().week_offset += 1;
            refresh();
        }
    });

    state.on_set_list_filter({
        let (view, refresh) = (view.clone(), refresh.clone());
        move |id| {
            view.borrow_mut().list = id.to_string();
            refresh();
        }
    });

    state.on_set_task_filter({
        let (view, refresh) = (view.clone(), refresh.clone());
        move |filter| {
            view.borrow_mut().task_filter = filter.to_string();
            refresh();
        }
    });

    state.on_begin_edit_activity({
        let (ctx, weak) = (ctx.clone(), window.as_weak());
        move |id| {
            let (Some(w), Some(r)) = (weak.upgrade(), ctx.insights.find_record(&id)) else { return };
            let st = w.global::<InsightsState>();
            st.set_edit_title(r.title.as_str().into());
            st.set_edit_start(r.start.format("%H:%M").to_string().into());
            st.set_edit_end(r.end.format("%H:%M").to_string().into());
            st.set_error("".into());
            st.set_edit_id(id);
        }
    });

    state.on_save_activity({
        let (ctx, refresh, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        move |id, start, end| {
            let Some(w) = weak.upgrade() else { return };
            let st = w.global::<InsightsState>();
            let result = match (ctx.insights.find_record(&id), parse_hhmm(&start), parse_hhmm(&end)) {
                (Some(r), Some(s), Some(e)) => {
                    // Keep the day; an end before the start means the next morning.
                    let day = r.start.date();
                    let from = day.and_time(s);
                    let mut to = day.and_time(e);
                    if to <= from {
                        to += chrono::Duration::days(1);
                    }
                    ctx.insights.update_session(&id, None, None, from, to)
                }
                (None, _, _) => Err("That entry no longer exists.".to_string()),
                _ => Err("Use times like 09:30.".to_string()),
            };
            match result {
                Ok(()) => {
                    st.set_error("".into());
                    st.set_edit_id("".into());
                }
                Err(message) => st.set_error(message.into()),
            }
            refresh();
        }
    });

    state.on_delete_activity({
        let (ctx, refresh, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        move |id| {
            ctx.insights.delete_session(&id);
            if let Some(w) = weak.upgrade() {
                let st = w.global::<InsightsState>();
                st.set_edit_id("".into());
                st.set_error("".into());
            }
            refresh();
        }
    });

    state.on_export_csv({
        let (ctx, view, weak) = (ctx.clone(), view.clone(), window.as_weak());
        move || {
            let Some(w) = weak.upgrade() else { return };
            let data = view.borrow().data(&ctx);
            let name = format!("thulli-week-{}.csv", data.from.format("%Y%m%d"));
            save_export(&w, &name, sessions_csv(&data.activity).as_bytes());
        }
    });

    state.on_export_report({
        let (ctx, view, weak) = (ctx.clone(), view.clone(), window.as_weak());
        move || {
            let Some(w) = weak.upgrade() else { return };
            let v = view.borrow();
            let data = v.data(&ctx);
            let report = ctx.insights.report(data.from, data.to, list_option(&v.list));
            let name = format!("thulli-report-{}-to-{}.pdf", data.from.format("%Y%m%d"), data.to.format("%Y%m%d"));
            save_export(&w, &name, &simple_pdf("Thulli report", &report.text_lines()));
        }
    });

    refresh();
}
