//! Wiring for the focus screen (ui/screens/focus.slint).
//!
//! Shows the active task (or session or break) from the focus run service and
//! routes the expanded view's actions back to it. Display mode and background
//! are screen state kept on the Slint side.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel, Weak};
use thulli_core::services::timer::{FocusMode, Moment, ShortChoice};

use crate::app::context::AppContext;
use crate::app::convert::task_model;
use crate::{FocusQueueItemModel, FocusState, MainWindow, TaskModel};

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let weak = window.as_weak();
    wire_callbacks(window, ctx, weak.clone());
    refresh(window, ctx);

    let ctx_tick = ctx.clone();
    ctx.on_tick(move |_, _| {
        if let Some(window) = weak.upgrade() {
            refresh(&window, &ctx_tick);
        }
    });
}

/// Runs `action`, then re-renders the screen.
fn act(window: &Weak<MainWindow>, ctx: &AppContext, action: impl FnOnce(&AppContext)) {
    action(ctx);
    if let Some(window) = window.upgrade() {
        refresh(&window, ctx);
    }
}

fn wire_callbacks(window: &MainWindow, ctx: &AppContext, weak: Weak<MainWindow>) {
    let state = window.global::<FocusState>();
    let now = Moment::now;

    let (c, w) = (ctx.clone(), weak.clone());
    state.on_done(move || act(&w, &c, |c| {
        c.focus.complete_task_at(now());
    }));

    let (c, w) = (ctx.clone(), weak.clone());
    state.on_start_break(move || act(&w, &c, |c| c.focus.start_break_at(None, now())));

    let (c, w) = (ctx.clone(), weak.clone());
    state.on_toggle_pause(move || act(&w, &c, |c| {
        c.focus.toggle_pause_at(now());
    }));

    let (c, w) = (ctx.clone(), weak.clone());
    state.on_skip(move || act(&w, &c, |c| {
        c.focus.skip_at(now());
    }));

    let (c, w) = (ctx.clone(), weak.clone());
    state.on_add_step(move |title| act(&w, &c, |c| {
        if let Some(parent) = c.focus.timer().snapshot().task_id {
            c.focus.add_step(&parent, title.as_str());
        }
    }));

    let (c, w) = (ctx.clone(), weak.clone());
    state.on_select_task(move |id| act(&w, &c, |c| {
        c.focus.start_task_at(id.as_str(), now());
    }));

    let (c, w) = (ctx.clone(), weak.clone());
    state.on_add_queue_task(move |id| act(&w, &c, |c| {
        c.today.add_to_today(id.as_str());
    }));

    let (c, w) = (ctx.clone(), weak.clone());
    state.on_close_session(move || act(&w, &c, |c| {
        c.focus.finish_at(now(), ShortChoice::Save);
    }));

    // Expanded, tower and pill modes are screen state on the Slint side.
    // Only the expanded view is built in this slice.
    state.on_set_display_mode(|_mode| {});
    state.on_set_background(|_name| {});
    state.on_pick_background_image(|| {});
    state.on_add_note(|| {});
}

/// Pushes the active run's state and the day queue into the focus screen.
pub fn refresh(window: &MainWindow, ctx: &AppContext) {
    let state = window.global::<FocusState>();
    let view = ctx.focus.view();
    let queue = ctx.focus.queue();

    state.set_task_id(view.task_id.clone().unwrap_or_default().into());
    state.set_task_title(view.task_title.clone().into());
    state.set_task_notes(view.task_notes.clone().into());
    state.set_clock(view.clock.clone().into());
    state.set_estimate_label(view.estimate_label.clone().into());
    state.set_progress(view.progress);
    state.set_overtime(view.overtime);
    state.set_on_break(view.mode == FocusMode::Break);
    state.set_paused(view.paused);

    let steps: Vec<TaskModel> = match &view.task_id {
        Some(id) => ctx.handle.read(|s| s.steps_of(id).into_iter().map(|t| task_model(s, t)).collect()),
        None => Vec::new(),
    };
    state.set_steps(ModelRc::from(Rc::new(VecModel::from(steps))));

    let items: Vec<FocusQueueItemModel> = queue
        .entries
        .iter()
        .map(|e| FocusQueueItemModel {
            task_id: e.task_id.clone().into(),
            title: e.title.clone().into(),
            done: e.done,
            active: e.active,
        })
        .collect();
    state.set_queue(ModelRc::from(Rc::new(VecModel::from(items))));
    state.set_queue_done(queue.done as i32);
    state.set_queue_total(queue.total as i32);
}
