//! Wiring for the reports screen (ui/screens/reports.slint).
//!
//! Thin layer: the numbers come from `thulli_core::services::insights`; this
//! file converts them to Slint models and handles the buttons. The helpers at
//! the top are shared with the insights screen.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use chrono::{Local, NaiveDate};
use slint::{ComponentHandle, ModelRc, VecModel};
use thulli_core::services::insights::{
    self, donut_geometry, format_duration, hour_label, month_name, normalise, parse_date, parse_date_time, range_bounds,
    sessions_csv, simple_pdf, weekday_name, ReportData, SessionRecord,
};

use crate::app::context::AppContext;
use crate::app::convert::{hex_color, task_model};
use crate::{BarModel, ChoiceModel, DonutArcModel, DonutSliceModel, MainWindow, ReportsState, SessionRowModel, ShellState, StatTileModel};

// ----- helpers shared with the insights screen -----

pub(crate) fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}

pub(crate) fn stat_tile(title: &str, value: impl Into<String>, caption: impl Into<String>) -> StatTileModel {
    StatTileModel { title: title.into(), value: value.into().into(), caption: caption.into().into() }
}

pub(crate) fn bar(label: impl Into<String>, secs: u64, max: u64) -> BarModel {
    BarModel {
        label: label.into().into(),
        value: normalise(&[secs], max)[0],
        value_label: if secs == 0 { String::new() } else { format_duration(secs) }.into(),
    }
}

/// "All lists" followed by every list that is not archived.
pub(crate) fn list_choices(ctx: &AppContext) -> Vec<ChoiceModel> {
    ctx.handle.read(|s| {
        let mut out = vec![ChoiceModel { id: "".into(), label: "All lists".into() }];
        out.extend(
            s.lists.iter().filter(|l| !l.is_archived).map(|l| ChoiceModel { id: l.id.clone().into(), label: l.name.clone().into() }),
        );
        out
    })
}

/// `Some(id)` for a chosen list, `None` for "All lists".
pub(crate) fn list_option(id: &str) -> Option<&str> {
    Some(id).filter(|s| !s.is_empty())
}

/// Folder exports are written to: Documents when it exists, otherwise the current folder.
fn export_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|home| PathBuf::from(home).join("Documents"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Writes an export file and tells the user where it went.
pub(crate) fn save_export(window: &MainWindow, name: &str, bytes: &[u8]) {
    let path = export_dir().join(name);
    let message = match std::fs::write(&path, bytes) {
        Ok(()) => format!("Saved {}", path.display()),
        Err(e) => format!("Could not save {name}: {e}"),
    };
    window.global::<ShellState>().set_notice(message.into());
}

pub(crate) fn session_row(r: &SessionRecord) -> SessionRowModel {
    SessionRowModel {
        id: r.id.clone().into(),
        title: r.title.clone().into(),
        list_name: r.list_name.clone().into(),
        date_label: r.start.format("%a %d %b").to_string().into(),
        start_label: r.start.format("%H:%M").to_string().into(),
        end_label: r.end.format("%H:%M").to_string().into(),
        duration_label: format_duration(r.duration_secs).into(),
        is_break: r.is_break(),
    }
}

// ----- screen state -----

struct View {
    range: String,
    custom: Option<(NaiveDate, NaiveDate)>,
    list: String,
    show_breaks: bool,
}

impl View {
    fn bounds(&self, today: NaiveDate) -> (NaiveDate, NaiveDate) {
        range_bounds(&self.range, today, self.custom)
    }

    fn report(&self, ctx: &AppContext, today: NaiveDate) -> ReportData {
        let (from, to) = self.bounds(today);
        ctx.insights.report(from, to, list_option(&self.list))
    }

    /// Sessions as shown (breaks hidden when switched off).
    fn visible_sessions<'a>(&self, data: &'a ReportData) -> Vec<&'a SessionRecord> {
        data.sessions.iter().filter(|r| self.show_breaks || !r.is_break()).collect()
    }
}

fn render(window: &MainWindow, ctx: &AppContext, view: &View) {
    let state = window.global::<ReportsState>();
    let today = Local::now().date_naive();
    let data = view.report(ctx, today);

    state.set_range(view.range.as_str().into());
    state.set_list_filter(view.list.as_str().into());
    state.set_show_breaks(view.show_breaks);
    state.set_range_label(format!("{} to {}", data.from.format("%d %b %Y"), data.to.format("%d %b %Y")).into());
    state.set_lists(model(list_choices(ctx)));

    let t = &data.totals;
    state.set_totals(model(vec![
        stat_tile("Work days", t.work_days.to_string(), "with time tracked"),
        stat_tile("Tasks done", t.tasks_done.to_string(), format!("{} worked on", t.tasks_worked)),
        stat_tile("Time worked", format_duration(t.worked_secs), format!("{} of breaks", format_duration(t.break_secs))),
        stat_tile("Average per task", format_duration(t.avg_secs_per_task), "time on each task"),
    ]));

    // Daily bars share one scale so they can be compared.
    let max = data.days.iter().map(|d| d.total()).max().unwrap_or(0);
    let label_of = |d: &insights::DayTotals| -> String {
        if data.days.len() <= 7 { d.date.format("%a").to_string() } else { d.date.format("%-d").to_string() }
    };
    state.set_daily_tasks(model(data.days.iter().map(|d| bar(label_of(d), d.worked(), max)).collect()));
    state.set_daily_breaks(model(data.days.iter().map(|d| bar(label_of(d), d.breaks, max)).collect()));
    state.set_daily_total(model(data.days.iter().map(|d| bar(label_of(d), d.total(), max)).collect()));

    // Time by list.
    let total_secs: u64 = data.by_list.iter().map(|s| s.secs).sum();
    let fractions: Vec<f32> =
        data.by_list.iter().map(|s| if total_secs == 0 { 0.0 } else { s.secs as f32 / total_secs as f32 }).collect();
    state.set_time_by_list(model(
        data.by_list
            .iter()
            .zip(&fractions)
            .map(|(s, f)| DonutSliceModel {
                label: s.name.clone().into(),
                fraction: *f,
                color: hex_color(&s.color),
                value_label: format_duration(s.secs).into(),
            })
            .collect(),
    ));
    state.set_donut_arcs(model(
        donut_geometry(&fractions)
            .into_iter()
            .zip(&data.by_list)
            .filter_map(|(g, s)| {
                g.map(|g| DonutArcModel { x0: g.x0, y0: g.y0, x1: g.x1, y1: g.y1, large: g.large, color: hex_color(&s.color) })
            })
            .collect(),
    ));
    state.set_donut_total(format_duration(total_secs).into());

    state.set_done_tasks(model(ctx.handle.read(|s| {
        data.done_task_ids.iter().filter_map(|id| s.task(id)).map(|t| task_model(s, t)).collect()
    })));

    state.set_most_productive_hour(data.best_hour.map(hour_label).unwrap_or_default().into());
    state.set_most_productive_day(data.best_weekday.map(|d| weekday_name(d).to_string()).unwrap_or_default().into());
    state.set_most_productive_month(data.best_month.map(|m| month_name(m).to_string()).unwrap_or_default().into());
    state.set_on_time_ratio(data.on_time.label().into());

    state.set_sessions(model(view.visible_sessions(&data).into_iter().map(session_row).collect()));
}

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let view = Rc::new(RefCell::new(View { range: "7d".into(), custom: None, list: String::new(), show_breaks: true }));

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
    let state = window.global::<ReportsState>();

    state.on_refresh({
        let refresh = refresh.clone();
        move || refresh()
    });

    state.on_set_range({
        let (view, refresh, weak) = (view.clone(), refresh.clone(), window.as_weak());
        move |range| {
            {
                let mut v = view.borrow_mut();
                v.range = range.to_string();
                // Opening "Custom" for the first time starts from the current range.
                if v.range == "custom" && v.custom.is_none() {
                    let today = Local::now().date_naive();
                    v.custom = Some(range_bounds("30d", today, None));
                }
                if let (Some(w), Some((from, to))) = (weak.upgrade(), v.custom) {
                    let st = w.global::<ReportsState>();
                    st.set_custom_from(from.format("%Y-%m-%d").to_string().into());
                    st.set_custom_to(to.format("%Y-%m-%d").to_string().into());
                }
            }
            refresh();
        }
    });

    state.on_set_custom_range({
        let (view, refresh, weak) = (view.clone(), refresh.clone(), window.as_weak());
        move |from, to| {
            match (parse_date(&from), parse_date(&to)) {
                (Some(a), Some(b)) => {
                    {
                        let mut v = view.borrow_mut();
                        v.range = "custom".into();
                        v.custom = Some((a, b));
                    }
                    if let Some(w) = weak.upgrade() {
                        w.global::<ReportsState>().set_error("".into());
                    }
                }
                _ => {
                    if let Some(w) = weak.upgrade() {
                        w.global::<ReportsState>().set_error("Enter dates like 2026-03-04.".into());
                    }
                }
            }
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

    state.on_set_show_breaks({
        let (view, refresh) = (view.clone(), refresh.clone());
        move |show| {
            view.borrow_mut().show_breaks = show;
            refresh();
        }
    });

    state.on_begin_add_session({
        let (ctx, weak) = (ctx.clone(), window.as_weak());
        move || {
            let Some(w) = weak.upgrade() else { return };
            let st = w.global::<ReportsState>();
            let primary = ctx.handle.read(|s| s.get_primary_list().map(|l| l.id.clone()).unwrap_or_default());
            st.set_form_id("".into());
            st.set_form_title("".into());
            st.set_form_list_id(primary.into());
            st.set_form_date(Local::now().format("%Y-%m-%d").to_string().into());
            st.set_form_start("09:00".into());
            st.set_form_end("10:00".into());
            st.set_form_is_break(false);
            st.set_error("".into());
            st.set_form_visible(true);
        }
    });

    state.on_edit_session({
        let (ctx, weak) = (ctx.clone(), window.as_weak());
        move |id| {
            let (Some(w), Some(r)) = (weak.upgrade(), ctx.insights.find_record(&id)) else { return };
            let st = w.global::<ReportsState>();
            st.set_form_id(id);
            st.set_form_title(r.title.as_str().into());
            st.set_form_list_id(ctx.insights.list_id_of(&r.id).unwrap_or_default().into());
            st.set_form_date(r.start.format("%Y-%m-%d").to_string().into());
            st.set_form_start(r.start.format("%H:%M").to_string().into());
            st.set_form_end(r.end.format("%H:%M").to_string().into());
            st.set_form_is_break(r.is_break());
            st.set_error("".into());
            st.set_form_visible(true);
        }
    });

    state.on_add_session({
        let (ctx, refresh, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        move |title, list, start, end| {
            let Some(w) = weak.upgrade() else { return };
            let st = w.global::<ReportsState>();
            let result = match (parse_date_time(&start), parse_date_time(&end)) {
                (Some(s), Some(e)) => ctx.insights.add_session(&title, &list, s, e, st.get_form_is_break()).map(|_| ()),
                _ => Err("Use a date like 2026-03-04 and times like 09:30.".to_string()),
            };
            finish_form(&st, result);
            refresh();
        }
    });

    state.on_save_session({
        let (ctx, refresh, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        move |id, title, list, start, end| {
            let Some(w) = weak.upgrade() else { return };
            let st = w.global::<ReportsState>();
            let result = match (parse_date_time(&start), parse_date_time(&end)) {
                (Some(s), Some(e)) => ctx.insights.update_session(&id, Some(&title), Some(&list), s, e),
                _ => Err("Use a date like 2026-03-04 and times like 09:30.".to_string()),
            };
            finish_form(&st, result);
            refresh();
        }
    });

    state.on_delete_session({
        let (ctx, refresh) = (ctx.clone(), refresh.clone());
        move |id| {
            ctx.insights.delete_session(&id);
            refresh();
        }
    });

    state.on_export_csv({
        let (ctx, view, weak) = (ctx.clone(), view.clone(), window.as_weak());
        move || {
            let Some(w) = weak.upgrade() else { return };
            let v = view.borrow();
            let data = v.report(&ctx, Local::now().date_naive());
            let rows: Vec<SessionRecord> = v.visible_sessions(&data).into_iter().cloned().collect();
            let name = format!("thulli-sessions-{}-to-{}.csv", data.from.format("%Y%m%d"), data.to.format("%Y%m%d"));
            save_export(&w, &name, sessions_csv(&rows).as_bytes());
        }
    });

    state.on_export_report({
        let (ctx, view, weak) = (ctx.clone(), view.clone(), window.as_weak());
        move || {
            let Some(w) = weak.upgrade() else { return };
            let data = view.borrow().report(&ctx, Local::now().date_naive());
            let name = format!("thulli-report-{}-to-{}.pdf", data.from.format("%Y%m%d"), data.to.format("%Y%m%d"));
            save_export(&w, &name, &simple_pdf("Thulli report", &data.text_lines()));
        }
    });

    refresh();
}

/// Closes the form on success, or shows the problem.
fn finish_form(state: &ReportsState<'_>, result: Result<(), String>) {
    match result {
        Ok(()) => {
            state.set_error("".into());
            state.set_form_visible(false);
        }
        Err(message) => state.set_error(message.into()),
    }
}
