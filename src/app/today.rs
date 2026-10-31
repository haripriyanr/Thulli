//! Wiring for the today screen (ui/screens/today.slint).
//!
//! Fills `TodayState` from the day plan, the focus run service and the task
//! services, and routes its callbacks back to them. The screen is refreshed on
//! every timer tick that moved time, and after each action.

use std::cell::RefCell;
use std::rc::Rc;

use chrono::{DateTime, Local, NaiveDate, Timelike, Utc};
use slint::{ComponentHandle, ModelRc, VecModel, Weak};
use thulli_core::models::{AppStateStore, Task, TaskSection, TaskStatus};
use thulli_core::services::timer::{FocusMode, FollowUp, Moment, ShortChoice, TimerPhase, TimerPurpose};
use thulli_core::services::today::format_duration;

use crate::app::context::AppContext;
use crate::app::convert::task_model;
use crate::{MainWindow, TaskModel, TimeBlockModel, TodayState};

/// What the screen shows that the services do not hold: the notice line and
/// the short-session prompt.
#[derive(Default)]
struct Transient {
    notice: String,
    short_prompt_secs: Option<u64>,
}

struct Binding {
    window: Weak<MainWindow>,
    ctx: AppContext,
    transient: RefCell<Transient>,
}

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let binding = Rc::new(Binding {
        window: window.as_weak(),
        ctx: ctx.clone(),
        transient: RefCell::new(Transient::default()),
    });
    wire_callbacks(window, &binding);
    refresh(window, &binding);

    let weak = Rc::downgrade(&binding);
    ctx.on_tick(move |outcome, _| {
        let Some(binding) = weak.upgrade() else { return };
        binding.ctx.focus.observe();
        if outcome.finished {
            if let FollowUp::SessionEnded { focused_secs } | FollowUp::SprintEnded { focused_secs } =
                binding.ctx.focus.handle_finished_at(Moment::now())
            {
                binding.transient.borrow_mut().notice = session_notice(focused_secs);
            }
        }
        if let Some(window) = binding.window.upgrade() {
            refresh(&window, &binding);
        }
    });
}

fn session_notice(focused_secs: u64) -> String {
    format!("Session finished: {} of focus", format_duration(focused_secs))
}

/// Runs `action` against the services, then re-renders the screen.
fn act(binding: &Binding, action: impl FnOnce(&Binding)) {
    action(binding);
    if let Some(window) = binding.window.upgrade() {
        refresh(&window, binding);
    }
}

fn wire_callbacks(window: &MainWindow, binding: &Rc<Binding>) {
    let state = window.global::<TodayState>();
    let now = Moment::now;

    // ---- plan ----
    let b = binding.clone();
    state.on_plan_day(move || act(&b, |b| b.ctx.focus.begin_planning_at(now())));

    let b = binding.clone();
    state.on_skip_planning(move || {
        act(&b, |b| b.ctx.today.skip_planning(b.ctx.today.today()));
    });

    let b = binding.clone();
    state.on_finish_planning(move || {
        act(&b, |b| {
            let secs = b.ctx.focus.finish_planning_at(now());
            b.ctx.today.finish_planning(b.ctx.today.today(), secs);
        });
    });

    let b = binding.clone();
    state.on_reopen_day(move || act(&b, |b| b.ctx.today.reopen_day(b.ctx.today.today())));

    let b = binding.clone();
    state.on_wrap_up(move |brain_dump| {
        act(&b, |b| b.ctx.today.wrap_up(b.ctx.today.today(), brain_dump.as_str()));
    });

    // ---- session ----
    let b = binding.clone();
    state.on_start_session(move |mins| {
        act(&b, |b| b.ctx.focus.start_session_at(mins.max(0) as u32, now()));
    });

    let b = binding.clone();
    state.on_toggle_session(move || {
        act(&b, |b| {
            b.ctx.focus.toggle_pause_at(now());
        });
    });

    let b = binding.clone();
    state.on_finish_session(move || {
        act(&b, |b| {
            let m = now();
            match b.ctx.focus.short_session_secs_at(m) {
                Some(secs) => b.transient.borrow_mut().short_prompt_secs = Some(secs),
                None => finish_session(b, ShortChoice::Save, m),
            }
        });
    });

    let b = binding.clone();
    state.on_short_session_choice(move |choice| {
        act(&b, |b| {
            let short = if choice.as_str() == "delete" { ShortChoice::Delete } else { ShortChoice::Save };
            finish_session(b, short, now());
        });
    });

    let b = binding.clone();
    state.on_start_break(move || act(&b, |b| b.ctx.focus.start_break_at(None, now())));

    let b = binding.clone();
    state.on_end_break(move || act(&b, |b| b.ctx.focus.end_break_at(now())));

    let b = binding.clone();
    state.on_continue_next(move || {
        act(&b, |b| {
            b.ctx.focus.continue_after_break_at(now());
        });
    });

    let b = binding.clone();
    state.on_start_now(move || {
        act(&b, |b| {
            b.ctx.focus.start_first_at(now());
        });
    });

    // The focus screen is reached through the shell's navigation.
    state.on_open_focus(|| {});

    // ---- tasks ----
    let b = binding.clone();
    state.on_start_task(move |id| act(&b, |b| {
        b.ctx.focus.start_task_at(id.as_str(), now());
    }));

    let b = binding.clone();
    state.on_choose_task(move |id| act(&b, |b| {
        b.ctx.focus.choose_task_at(id.as_str(), now());
    }));

    let b = binding.clone();
    state.on_complete_task(move |id| act(&b, |b| {
        let active = b.ctx.focus.timer().snapshot().task_id;
        if active.as_deref() == Some(id.as_str()) {
            b.ctx.focus.complete_task_at(now());
        } else {
            b.ctx.tasks.set_status(id.as_str(), TaskStatus::Completed);
        }
    }));

    let b = binding.clone();
    state.on_add_to_today(move |id| act(&b, |b| {
        b.ctx.today.add_to_today(id.as_str());
    }));

    let b = binding.clone();
    state.on_remove_from_today(move |id| act(&b, |b| {
        b.ctx.today.remove_from_today(id.as_str());
    }));

    let b = binding.clone();
    state.on_add_task(move |raw| act(&b, |b| {
        let list_id = b.ctx.handle.read(|s| s.lists.iter().find(|l| !l.is_archived).map(|l| l.id.clone()));
        if let Some(list_id) = list_id {
            b.ctx.tasks.add_smart_task(raw.as_str(), &list_id, TaskSection::Today);
        }
    }));

    // ---- settings ----
    let b = binding.clone();
    state.on_set_session_minutes(move |mins| act(&b, |b| {
        let mins = thulli_core::models::Preferences::clamp_session_mins(mins.max(0) as u32);
        b.ctx.handle.mutate(|s| s.preferences.default_session_mins = mins);
    }));

    let b = binding.clone();
    state.on_set_goal(move |mins| act(&b, |b| b.ctx.today.set_daily_goal_mins(mins.max(0) as u32)));
}

/// Finishes the current run, records the notice and clears the prompt.
fn finish_session(b: &Binding, short: ShortChoice, m: Moment) {
    let outcome = b.ctx.focus.finish_at(m, short);
    let mut transient = b.transient.borrow_mut();
    transient.short_prompt_secs = None;
    if !outcome.discarded && outcome.focused_secs > 0 {
        transient.notice = session_notice(outcome.focused_secs);
    }
}

/// Pushes the current state of the day, the timer and the goal into the screen.
fn refresh(window: &MainWindow, b: &Binding) {
    let ctx = &b.ctx;
    let state = window.global::<TodayState>();
    let date: NaiveDate = ctx.today.today();
    let focus = &ctx.focus;
    let snap = ctx.timer.snapshot();
    let view = focus.view();
    let planning = focus.is_planning();
    let live: Option<(String, DateTime<Utc>)> = focus.live_block();

    // Day phase and header
    state.set_phase(ctx.today.phase(date, planning).as_str().into());
    state.set_date_label(Local::now().format("%A, %-d %B").to_string().into());
    let clock = Local::now();
    state.set_now_mins((clock.hour() * 60 + clock.minute()) as i32);

    // Tasks
    let priority = ctx.today.priority_tasks();
    let candidates = ctx.today.candidate_tasks();
    state.set_tasks(task_rows(ctx, &priority));
    state.set_candidates(task_rows(ctx, &candidates));
    state.set_picker(task_rows(ctx, &candidates));

    // Run state
    let run_mode = if planning {
        "planning"
    } else {
        match snap.purpose {
            _ if snap.phase == TimerPhase::Idle => "idle",
            TimerPurpose::Task => "task",
            TimerPurpose::Session => "session",
            TimerPurpose::Sprint => "sprint",
            TimerPurpose::Break => "break",
            TimerPurpose::Planning => "planning",
        }
    };
    state.set_run_mode(run_mode.into());
    state.set_run_clock(snap.formatted().into());
    state.set_planning_clock(if planning { snap.formatted().into() } else { "00:00".into() });
    state.set_run_title(view.task_title.clone().into());
    state.set_run_label(
        if view.paused {
            "Paused".to_string()
        } else if !view.estimate_label.is_empty() {
            view.estimate_label.clone()
        } else if snap.planned_secs > 0 {
            format!("{} of {}", format_duration(snap.elapsed_secs), format_duration(snap.planned_secs))
        } else {
            String::new()
        }
        .into(),
    );
    let run_progress = if view.mode == FocusMode::Task {
        view.progress
    } else if snap.planned_secs > 0 {
        (snap.elapsed_secs as f32 / snap.planned_secs as f32).min(1.0)
    } else {
        0.0
    };
    state.set_run_progress(run_progress);
    state.set_run_paused(view.paused);
    state.set_run_overtime(view.overtime);
    state.set_chosen_task_title(
        focus
            .pending_task()
            .and_then(|id| ctx.handle.read(|s| s.task(&id).map(|t| t.title.clone())))
            .unwrap_or_default()
            .into(),
    );
    state.set_can_start_now(focus.can_start_now());
    state.set_awaiting_next(focus.awaiting_next());
    state.set_session_minutes(ctx.handle.read(|s| s.preferences.default_session_mins) as i32);

    let transient = b.transient.borrow();
    state.set_notice(transient.notice.clone().into());
    state.set_short_prompt_secs(transient.short_prompt_secs.map_or(-1, |s| s as i32));
    drop(transient);

    // Daily goal
    let summary = ctx.today.goal_summary(date, live.as_ref());
    state.set_goal_progress(summary.progress);
    state.set_goal_mins((summary.goal_secs / 60) as i32);
    state.set_goal_label(format!("{} of {}", format_duration(summary.worked_secs), format_duration(summary.goal_secs)).into());
    state.set_goal_worked(format_duration(summary.worked_secs).into());
    state.set_goal_left(format_duration(summary.left_secs).into());
    state.set_goal_focus(format_duration(summary.focus_secs).into());
    state.set_goal_meetings(format_duration(summary.meeting_secs).into());
    state.set_goal_other(format_duration(summary.other_secs).into());
    state.set_wrapped_worked(format_duration(summary.worked_secs).into());

    // Timeline
    let blocks: Vec<TimeBlockModel> = ctx
        .today
        .timeline(date, live.as_ref())
        .into_iter()
        .map(|block| TimeBlockModel {
            id: block.id.into(),
            kind: block.kind.as_str().into(),
            label: block.label.into(),
            start_mins: block.start_mins,
            length_mins: block.length_mins,
        })
        .collect();
    state.set_timeline(ModelRc::from(Rc::new(VecModel::from(blocks))));
}

/// Converts tasks to rows, keeping the order given.
fn task_rows(ctx: &AppContext, tasks: &[Task]) -> ModelRc<TaskModel> {
    let rows: Vec<TaskModel> = ctx.handle.read(|s: &AppStateStore| tasks.iter().map(|t| task_model(s, t)).collect());
    ModelRc::from(Rc::new(VecModel::from(rows)))
}
