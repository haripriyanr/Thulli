//! Wiring for the matrix screen (ui/screens/matrix.slint): the four quadrants,
//! the unsorted rail and the "prioritize one by one" review.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use slint::ComponentHandle;
use thulli_core::models::{MatrixQuadrant, Task};
use thulli_core::services::tasks::matrix;

use crate::app::context::AppContext;
use crate::app::convert::{model_of, task_model};
use crate::{MainWindow, MatrixState, TaskModel};

type Queue = Rc<RefCell<VecDeque<String>>>;

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let queue: Queue = Rc::default();

    let refresh = {
        let (ctx, queue, weak) = (ctx.clone(), queue.clone(), window.as_weak());
        move || {
            if let Some(w) = weak.upgrade() {
                fill(&w, &ctx, &queue);
            }
        }
    };

    let state = window.global::<MatrixState>();

    {
        let r = refresh.clone();
        state.on_request_refresh(move || r());
    }

    {
        let (ctx, r, queue) = (ctx.clone(), refresh.clone(), queue.clone());
        state.on_place_task(move |id, quadrant| {
            let quadrant = if quadrant.is_empty() { None } else { MatrixQuadrant::parse(&quadrant) };
            ctx.tasks.set_quadrant(&ctx.history, &id, quadrant);
            // A placed task leaves the review queue; the next one takes its place.
            queue.borrow_mut().retain(|queued| queued != id.as_str());
            r();
        });
    }

    {
        let (ctx, r, queue) = (ctx.clone(), refresh.clone(), queue.clone());
        state.on_review_quadrant(move |quadrant| {
            if let Some(quadrant) = MatrixQuadrant::parse(&quadrant) {
                let view = ctx.handle.read(|st| matrix::build(st, ""));
                *queue.borrow_mut() = matrix::quadrant_queue(&view, quadrant).into();
            }
            r();
        });
    }

    {
        let (ctx, r, queue) = (ctx.clone(), refresh.clone(), queue.clone());
        state.on_start_review(move || {
            let view = ctx.handle.read(|st| matrix::build(st, ""));
            *queue.borrow_mut() = matrix::unsorted_queue(&view).into();
            r();
        });
    }

    {
        let (r, queue) = (refresh.clone(), queue.clone());
        state.on_next_review_task(move || {
            // Skipping sends the current task to the back of the queue.
            let mut q = queue.borrow_mut();
            if let Some(front) = q.pop_front() {
                q.push_back(front);
            }
            drop(q);
            r();
        });
    }

    {
        let (r, queue) = (refresh.clone(), queue.clone());
        state.on_close_review(move || {
            queue.borrow_mut().clear();
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

/// Rebuilds the quadrants, the rail and the review card from the current state.
fn fill(window: &MainWindow, ctx: &AppContext, queue: &Queue) {
    let state = window.global::<MatrixState>();
    let view = ctx.handle.read(|st| matrix::build(st, ""));

    let to_models = |tasks: &[Task]| -> Vec<TaskModel> { ctx.handle.read(|st| tasks.iter().map(|t| task_model(st, t)).collect()) };

    state.set_do_asap(model_of(to_models(&view.do_asap)));
    state.set_do_later_today(model_of(to_models(&view.do_later_today)));
    state.set_do_mid_week(model_of(to_models(&view.do_mid_week)));
    state.set_leave_till_end(model_of(to_models(&view.leave_till_end)));
    state.set_unsorted_today(model_of(to_models(&view.unsorted_today)));
    state.set_unsorted_this_week(model_of(to_models(&view.unsorted_this_week)));
    state.set_unsorted_backlog(model_of(to_models(&view.unsorted_backlog)));
    state.set_unsorted_count(view.unsorted_count() as i32);
    state.set_can_undo(ctx.history.can_undo());
    state.set_can_redo(ctx.history.can_redo());

    update_review(window, ctx, queue);
}

/// Shows the first queued task that is still unsorted, or closes the card.
fn update_review(window: &MainWindow, ctx: &AppContext, queue: &Queue) {
    let state = window.global::<MatrixState>();
    let mut q = queue.borrow_mut();
    loop {
        let Some(front) = q.front().cloned() else {
            state.set_review_open(false);
            return;
        };
        let task = ctx.handle.read(|st| {
            st.task(&front)
                .filter(|t| t.is_pending() && t.matrix_quadrant.is_none() && !t.is_step())
                .map(|t| task_model(st, t))
        });
        match task {
            Some(model) => {
                state.set_review_task(model);
                state.set_review_remaining(q.len() as i32);
                state.set_review_open(true);
                return;
            }
            None => {
                q.pop_front();
            }
        }
    }
}
