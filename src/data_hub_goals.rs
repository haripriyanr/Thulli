//! Goals on screen: builds the dashboard and detail models from the saved
//! state and handles the Goals callbacks. Every change goes through
//! `GoalService`, which saves it.

use std::collections::HashMap;
use std::rc::Rc;

use chrono::{Local, NaiveDate};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use thulli_core::models::{AppStateStore, Goal, GoalPriority, GoalStatus, ProgressMode, Task};
use thulli_core::services::goals::{self, GoalDraft, GoalFilter, GoalService, GoalSort, ReviewItem, ReviewReason};
use thulli_core::services::insights::{format_duration, to_local};
use thulli_core::services::tasks::parse_date_input;

use super::DataHub;
use crate::{AppData, GoalRow, GoalSessionRow, GoalTaskOption, GoalTaskRow, MainWindow, MilestoneRow, ReviewRow};

/// What the Goals screen is showing. Not saved.
pub(super) struct GoalUi {
    filter: GoalFilter,
    filter_id: String,
    sort: GoalSort,
    sort_id: String,
    search: String,
    /// The goal open in the detail view.
    open: Option<String>,
    link_search: String,
}

impl Default for GoalUi {
    fn default() -> Self {
        Self {
            filter: GoalFilter::InPlay,
            filter_id: "in_play".into(),
            sort: GoalSort::Priority,
            sort_id: "priority".into(),
            search: String::new(),
            open: None,
            link_search: String::new(),
        }
    }
}

/// "12", "12.5": no trailing zeros.
fn number_text(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.2}").trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn parse_number(text: &str) -> Result<f64, String> {
    let t = text.trim().replace(',', ".");
    if t.is_empty() {
        return Ok(0.0);
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or_else(|| "Enter a number, such as 12 or 7.5.".to_string())
}

fn date_input(d: Option<NaiveDate>) -> String {
    d.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()
}

/// A date for display, such as "9 Nov".
fn short_date(d: NaiveDate) -> String {
    d.format("%-d %b").to_string()
}

/// How a date stands against today, as facts the screen turns into a
/// sentence: `(kind, days, date text)`. Kinds: "" (no date), "today",
/// "tomorrow", "overdue" (days late), "future" and "date" (a finished item,
/// where urgency means nothing).
fn due_facts(date: Option<NaiveDate>, today: NaiveDate, open: bool) -> (&'static str, i32, String) {
    let Some(d) = date else { return ("", 0, String::new()) };
    if !open {
        return ("date", 0, short_date(d));
    }
    let days = (d - today).num_days();
    match days {
        0 => ("today", 0, short_date(d)),
        1 => ("tomorrow", 1, short_date(d)),
        n if n < 0 => ("overdue", (-n) as i32, short_date(d)),
        n => ("future", n as i32, short_date(d)),
    }
}

fn goal_row(store: &AppStateStore, g: &Goal, today: NaiveDate, focus: &HashMap<String, (u64, u64)>) -> GoalRow {
    let p = g.progress();
    let open = matches!(g.status, GoalStatus::Active | GoalStatus::Paused);
    let (due_kind, due_n, due_date) = due_facts(g.target_date, today, open);
    let (tasks_done, tasks_total) = goals::task_progress(store, &g.id);
    let (total_secs, week_secs) = focus.get(&g.id).copied().unwrap_or((0, 0));
    GoalRow {
        id: g.id.clone().into(),
        title: g.title.clone().into(),
        description: g.description.clone().into(),
        motivation: g.motivation.clone().into(),
        notes: g.notes.clone().into(),
        category: g.category.clone().into(),
        priority: g.priority.as_str().into(),
        status: g.status.as_str().into(),
        mode: if g.track_focus { "hours" } else { g.progress_mode.as_str() }.into(),
        percent: p.whole_percent(),
        percent_known: p.percent.is_some(),
        milestones_done: g.milestones.iter().filter(|m| m.is_done()).count() as i32,
        milestones_total: g.milestones.len() as i32,
        current_value: number_text(g.current_value).into(),
        target_value: if g.target_value > 0.0 { number_text(g.target_value) } else { String::new() }.into(),
        remaining_value: number_text(p.remaining).into(),
        unit: g.unit.clone().into(),
        due_kind: due_kind.into(),
        due_n,
        due_date: due_date.into(),
        overdue: due_kind == "overdue",
        next_milestone: g.next_milestone().map(|m| m.title.clone()).unwrap_or_default().into(),
        tasks_done: tasks_done as i32,
        tasks_total: tasks_total as i32,
        has_focus: total_secs > 0,
        total_focus_text: format_duration(total_secs).into(),
        week_focus_text: format_duration(week_secs).into(),
        target_date: date_input(g.target_date).into(),
    }
}

fn milestone_rows(store: &AppStateStore, g: &Goal, today: NaiveDate) -> Vec<MilestoneRow> {
    let last = g.milestones.len().saturating_sub(1);
    g.milestones
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let pinned: Vec<&Task> = goals::linked_tasks(store, &g.id).into_iter().filter(|t| t.milestone_id.as_deref() == Some(m.id.as_str())).collect();
            let done = pinned.iter().filter(|t| !t.is_pending()).count();
            let (due_kind, due_n, due_date) = due_facts(m.target_date, today, !m.is_done());
            MilestoneRow {
                id: m.id.clone().into(),
                title: m.title.clone().into(),
                description: m.description.clone().into(),
                due_kind: due_kind.into(),
                due_n,
                due_date: due_date.into(),
                date_input: date_input(m.target_date).into(),
                done: m.is_done(),
                overdue: due_kind == "overdue",
                tasks_done: done as i32,
                tasks_total: pinned.len() as i32,
                first: i == 0,
                last: i == last,
            }
        })
        .collect()
}

fn task_rows(store: &AppStateStore, g: &Goal) -> Vec<GoalTaskRow> {
    goals::linked_tasks(store, &g.id)
        .into_iter()
        .map(|t| GoalTaskRow {
            id: t.id.clone().into(),
            title: t.title.clone().into(),
            done: !t.is_pending(),
            space: store.lists.iter().find(|l| l.id == t.list_id).map(|l| l.name.clone()).unwrap_or_default().into(),
            due_date: t.deadline.map(short_date).unwrap_or_default().into(),
            milestone: t.milestone_id.as_deref().and_then(|id| g.milestone(id)).map(|m| m.title.clone()).unwrap_or_default().into(),
            milestone_id: t.milestone_id.clone().unwrap_or_default().into(),
        })
        .collect()
}

/// A review line as facts: `(kind, a, b, text)` for the screen to phrase.
fn review_facts(r: &ReviewReason) -> (&'static str, i32, i32, String) {
    match r {
        ReviewReason::Progressed { milestones, tasks } => ("progressed", *milestones as i32, *tasks as i32, String::new()),
        ReviewReason::Deadline { days } => ("deadline", (*days).clamp(-9999, 9999) as i32, 0, String::new()),
        ReviewReason::MilestoneDue { title, days } if *days == i64::MAX => ("next", 0, 0, title.clone()),
        ReviewReason::MilestoneDue { title, days } => ("milestone_due", (*days).clamp(-9999, 9999) as i32, 0, title.clone()),
        ReviewReason::Quiet { days } => ("quiet", (*days).clamp(0, 9999) as i32, 0, String::new()),
        ReviewReason::NextUp { milestone: Some(m) } => ("next", 0, 0, m.clone()),
        ReviewReason::NextUp { milestone: None } => ("add", 0, 0, String::new()),
    }
}

fn review_rows(items: &[ReviewItem]) -> ModelRc<ReviewRow> {
    let rows: Vec<ReviewRow> = items
        .iter()
        .map(|i| {
            let (kind, a, b, s) = review_facts(&i.reason);
            ReviewRow { id: i.goal_id.clone().into(), title: i.title.clone().into(), kind: kind.into(), a, b, s: s.into() }
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

impl DataHub {
    /// Rebuilds the dashboard and, when a goal is open, its detail view.
    pub(super) fn refresh_goals(&self, window: &MainWindow) {
        let data = window.global::<AppData>();
        let today = Local::now().date_naive();
        // Goals that count focus hours follow the logged time.
        self.goals.sync_focus_hours();
        let ui = self.goal_ui.borrow();
        self.handle.read(|store| {
            let focus = goals::focus_summary(store, today);
            let rows: Vec<GoalRow> =
                goals::list_goals(store, ui.filter, ui.sort, &ui.search).into_iter().map(|g| goal_row(store, g, today, &focus)).collect();
            data.set_goals(ModelRc::new(VecModel::from(rows)));
            data.set_goals_none_yet(store.goals.is_empty());
            let counts = goals::counts(store, today);
            data.set_goals_active(counts.active as i32);
            data.set_goals_paused(counts.paused as i32);
            data.set_goals_completed(counts.completed as i32);
            data.set_goals_overdue(counts.overdue as i32);
            data.set_goals_filter(ui.filter_id.clone().into());
            data.set_goals_sort(ui.sort_id.clone().into());
            if data.get_review_open() {
                let r = goals::weekly_review(store, today);
                data.set_review_range(format!("{} – {}", r.from.format("%-d %b"), r.to.format("%-d %b")).into());
                data.set_review_milestones(r.milestones_done as i32);
                data.set_review_tasks(r.tasks_done as i32);
                data.set_review_focus(format_duration(r.focus_secs).into());
                data.set_review_progressed(review_rows(&r.progressed));
                data.set_review_upcoming(review_rows(&r.upcoming));
                data.set_review_attention(review_rows(&r.attention));
                data.set_review_priorities(review_rows(&r.priorities));
            }

            let open = ui.open.as_deref().and_then(|id| store.goals.iter().find(|g| g.id == id));
            match open {
                Some(g) => {
                    data.set_goal(goal_row(store, g, today, &focus));
                    data.set_goal_milestones(ModelRc::new(VecModel::from(milestone_rows(store, g, today))));
                    data.set_goal_tasks(ModelRc::new(VecModel::from(task_rows(store, g))));
                    let sessions: Vec<GoalSessionRow> = goals::recent_sessions(store, &g.id, 6)
                        .into_iter()
                        .map(|s| GoalSessionRow {
                            when: s.start.format("%a %-d %b, %H:%M").to_string().into(),
                            duration: format_duration(s.secs).into(),
                            task: s.task_title.into(),
                        })
                        .collect();
                    data.set_goal_sessions(ModelRc::new(VecModel::from(sessions)));
                    let history: Vec<SharedString> = g
                        .history
                        .iter()
                        .rev()
                        .take(8)
                        .map(|e| SharedString::from(format!("{} · {}", to_local(e.at).format("%-d %b %H:%M"), e.event)))
                        .collect();
                    data.set_goal_history(ModelRc::new(VecModel::from(history)));
                    let options: Vec<GoalTaskOption> = GoalService::linkable_tasks(store, &ui.link_search, 8)
                        .into_iter()
                        .map(|t| GoalTaskOption {
                            id: t.id.clone().into(),
                            title: t.title.clone().into(),
                            space: store.lists.iter().find(|l| l.id == t.list_id).map(|l| l.name.clone()).unwrap_or_default().into(),
                        })
                        .collect();
                    data.set_goal_link_options(ModelRc::new(VecModel::from(options)));
                    data.set_goal_open(true);
                }
                None => data.set_goal_open(false),
            }
        });
    }

    pub(super) fn wire_goals(self: &Rc<Self>, window: &MainWindow) {
        let data = window.global::<AppData>();

        // Runs a goal action, shows its error (if any), and refreshes.
        let act = |hub: &Rc<Self>, weak: &slint::Weak<MainWindow>, result: Result<(), String>| -> bool {
            let ok = result.is_ok();
            if let Some(w) = weak.upgrade() {
                w.global::<AppData>().set_goals_error(result.err().unwrap_or_default().into());
                hub.refresh_goals(&w);
            }
            ok
        };

        macro_rules! handler {
            ($on:ident, |$hub:ident, $w:ident $(, $a:ident)*| $body:expr) => {{
                let $hub = self.clone();
                let weak = window.as_weak();
                data.$on(move |$($a),*| {
                    let $w = &weak;
                    let _ = &$w;
                    $body
                });
            }};
        }

        handler!(on_goals_filter_changed, |hub, weak, id| {
            {
                let mut ui = hub.goal_ui.borrow_mut();
                ui.filter = GoalFilter::parse(&id);
                ui.filter_id = id.to_string();
            }
            act(&hub, weak, Ok(()));
        });
        handler!(on_goals_sort_changed, |hub, weak, id| {
            {
                let mut ui = hub.goal_ui.borrow_mut();
                ui.sort = GoalSort::parse(&id);
                ui.sort_id = id.to_string();
            }
            act(&hub, weak, Ok(()));
        });
        handler!(on_goals_search_changed, |hub, weak, text| {
            hub.goal_ui.borrow_mut().search = text.to_string();
            act(&hub, weak, Ok(()));
        });
        handler!(on_goal_opened, |hub, weak, id| {
            {
                let mut ui = hub.goal_ui.borrow_mut();
                ui.open = Some(id.to_string());
                ui.link_search.clear();
            }
            act(&hub, weak, Ok(()));
        });
        handler!(on_review_toggled, |hub, weak, _open| {
            act(&hub, weak, Ok(()));
        });
        handler!(on_goal_closed, |hub, weak| {
            hub.goal_ui.borrow_mut().open = None;
            act(&hub, weak, Ok(()));
        });
        handler!(on_goal_link_search_changed, |hub, weak, text| {
            hub.goal_ui.borrow_mut().link_search = text.to_string();
            act(&hub, weak, Ok(()));
        });

        // Create or edit.
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_goal_save(move |id, title, description, motivation, category, priority, mode, date, current, target, unit, notes| {
                let today = Local::now().date_naive();
                let built = (|| -> Result<GoalDraft, String> {
                    let mut d = GoalDraft::new(title.to_string());
                    d.description = description.to_string();
                    d.motivation = motivation.to_string();
                    d.category = category.to_string();
                    d.notes = notes.to_string();
                    d.priority = GoalPriority::parse(&priority).unwrap_or_default();
                    // "hours" is a measured goal that counts logged focus time.
                    d.track_focus = mode.as_str() == "hours";
                    d.mode = if d.track_focus { ProgressMode::Measured } else { ProgressMode::parse(&mode).unwrap_or_default() };
                    d.target_date = parse_date_input(&date, today)?;
                    d.unit = unit.to_string();
                    if d.track_focus {
                        d.target_value = parse_number(&target)?;
                    } else if d.mode == ProgressMode::Measured {
                        d.current_value = parse_number(&current)?;
                        d.target_value = parse_number(&target)?;
                    }
                    Ok(d)
                })();
                let result = built.and_then(|d| {
                    if id.is_empty() {
                        hub.goals.create(&d).map(|g| {
                            hub.goal_ui.borrow_mut().open = Some(g.id);
                        })
                    } else {
                        hub.goals.update(&id, &d)
                    }
                });
                if act(&hub, &weak, result) {
                    if let Some(w) = weak.upgrade() {
                        let data = w.global::<AppData>();
                        data.set_goal_saved_seq(data.get_goal_saved_seq() + 1);
                    }
                }
            });
        }

        handler!(on_goal_set_status, |hub, weak, id, status| {
            let result = match GoalStatusArg::parse(&status) {
                Some(s) => hub.goals.set_status(&id, s),
                None => Err("Unknown status.".to_string()),
            };
            act(&hub, weak, result);
        });
        handler!(on_goal_delete, |hub, weak, id| {
            let result = hub.goals.delete(&id);
            if result.is_ok() {
                hub.goal_ui.borrow_mut().open = None;
            }
            act(&hub, weak, result);
            if let Some(w) = weak.upgrade() {
                hub.refresh_all(&w);
            }
        });
        handler!(on_goal_set_value, |hub, weak, id, value| {
            let result = parse_number(&value).and_then(|v| hub.goals.set_current_value(&id, v));
            act(&hub, weak, result);
        });

        // Milestones.
        handler!(on_milestone_add, |hub, weak, goal, title, date, description| {
            let today = Local::now().date_naive();
            let result = parse_date_input(&date, today).and_then(|d| hub.goals.add_milestone(&goal, &title, d, &description).map(|_| ()));
            act(&hub, weak, result);
        });
        handler!(on_milestone_edit, |hub, weak, goal, milestone, title, date, description| {
            let today = Local::now().date_naive();
            let result = parse_date_input(&date, today).and_then(|d| hub.goals.edit_milestone(&goal, &milestone, &title, d, &description));
            act(&hub, weak, result);
        });
        handler!(on_milestone_toggle, |hub, weak, goal, milestone| {
            let done = hub.handle.read(|s| {
                s.goals.iter().find(|g| g.id == goal.as_str()).and_then(|g| g.milestone(&milestone)).is_some_and(|m| m.is_done())
            });
            let result = hub.goals.set_milestone_done(&goal, &milestone, !done);
            act(&hub, weak, result);
        });
        handler!(on_milestone_move, |hub, weak, goal, milestone, delta| {
            let result = hub.goals.move_milestone(&goal, &milestone, delta);
            act(&hub, weak, result);
        });
        handler!(on_milestone_delete, |hub, weak, goal, milestone| {
            let result = hub.goals.delete_milestone(&goal, &milestone);
            act(&hub, weak, result);
            if let Some(w) = weak.upgrade() {
                hub.refresh_all(&w);
            }
        });

        // Task links.
        handler!(on_goal_link_task, |hub, weak, goal, task, milestone| {
            let pin = (!milestone.is_empty()).then(|| milestone.to_string());
            let result = hub.goals.link_task(&goal, &task, pin.as_deref());
            act(&hub, weak, result);
            if let Some(w) = weak.upgrade() {
                hub.refresh_all(&w);
            }
        });
        handler!(on_goal_unlink_task, |hub, weak, task| {
            let result = hub.goals.unlink_task(&task);
            act(&hub, weak, result);
            if let Some(w) = weak.upgrade() {
                hub.refresh_all(&w);
            }
        });
        // "next" cycles a task through the goal's milestones, then back to none.
        handler!(on_goal_pin_task, |hub, weak, task, milestone| {
            let target: Option<String> = if milestone.as_str() == "next" {
                hub.handle.read(|s| {
                    let t = s.task(&task)?;
                    let g = s.goals.iter().find(|g| Some(&g.id) == t.goal_id.as_ref())?;
                    let pos = t.milestone_id.as_deref().and_then(|id| g.milestones.iter().position(|m| m.id == id));
                    match pos {
                        None => g.milestones.first().map(|m| m.id.clone()),
                        Some(i) => g.milestones.get(i + 1).map(|m| m.id.clone()),
                    }
                })
            } else {
                (!milestone.is_empty()).then(|| milestone.to_string())
            };
            let result = hub.goals.pin_task(&task, target.as_deref());
            act(&hub, weak, result);
        });
    }
}

/// Status ids sent by the screen.
struct GoalStatusArg;
impl GoalStatusArg {
    fn parse(value: &str) -> Option<GoalStatus> {
        GoalStatus::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_without_trailing_zeros() {
        assert_eq!(number_text(12.0), "12");
        assert_eq!(number_text(7.5), "7.5");
        assert_eq!(number_text(0.25), "0.25");
        assert_eq!(number_text(2.499), "2.5");
    }

    #[test]
    fn typed_numbers_accept_commas_and_reject_text() {
        assert_eq!(parse_number(" 7,5 "), Ok(7.5));
        assert_eq!(parse_number(""), Ok(0.0));
        assert!(parse_number("abc").is_err());
        assert!(parse_number("NaN").is_err());
        assert!(parse_number("inf").is_err());
    }

    #[test]
    fn dates_become_facts_not_english_sentences() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let day = |n: i64| Some(today + chrono::Duration::days(n));
        assert_eq!(due_facts(day(0), today, true), ("today", 0, "8 Nov".to_string()));
        assert_eq!(due_facts(day(1), today, true), ("tomorrow", 1, "9 Nov".to_string()));
        assert_eq!(due_facts(day(-1), today, true), ("overdue", 1, "7 Nov".to_string()));
        assert_eq!(due_facts(day(-3), today, true), ("overdue", 3, "5 Nov".to_string()));
        assert_eq!(due_facts(day(10), today, true), ("future", 10, "18 Nov".to_string()));
        // A finished item is never "overdue".
        assert_eq!(due_facts(day(-3), today, false).0, "date");
        assert_eq!(due_facts(None, today, true), ("", 0, String::new()));
    }

    #[test]
    fn review_reasons_become_facts() {
        assert_eq!(review_facts(&ReviewReason::Progressed { milestones: 1, tasks: 3 }), ("progressed", 1, 3, String::new()));
        assert_eq!(review_facts(&ReviewReason::Deadline { days: -4 }), ("deadline", -4, 0, String::new()));
        assert_eq!(review_facts(&ReviewReason::MilestoneDue { title: "Draft".into(), days: 2 }), ("milestone_due", 2, 0, "Draft".to_string()));
        assert_eq!(review_facts(&ReviewReason::MilestoneDue { title: "Draft".into(), days: i64::MAX }), ("next", 0, 0, "Draft".to_string()));
        assert_eq!(review_facts(&ReviewReason::Quiet { days: 20 }), ("quiet", 20, 0, String::new()));
        assert_eq!(review_facts(&ReviewReason::NextUp { milestone: None }).0, "add");
    }

    #[test]
    fn goal_rows_carry_counts_and_measured_values() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let mut store = AppStateStore::default();
        let mut g = Goal::new("Read");
        g.progress_mode = ProgressMode::Measured;
        g.target_value = 20.0;
        g.current_value = 12.0;
        g.unit = "books".into();
        g.target_date = Some(today - chrono::Duration::days(2));
        store.goals.push(g.clone());
        let row = goal_row(&store, &g, today, &HashMap::new());
        assert_eq!((row.current_value.as_str(), row.target_value.as_str(), row.remaining_value.as_str()), ("12", "20", "8"));
        assert_eq!((row.due_kind.as_str(), row.due_n, row.overdue), ("overdue", 2, true));
        assert_eq!(row.percent, 60);
        assert!(row.percent_known && !row.has_focus);
    }
}
