//! Shell wiring: window controls, navigation notice, timer pill and the
//! UI-thread tick that drives the focus timer.
//!
//! Owned by the foundation. Screens react to the timer through
//! `AppContext::on_tick` rather than editing this file.

use std::time::Duration;

use slint::ComponentHandle;
use thulli_core::services::{TimerMode, TimerPhase};
use thulli_core::window::WindowManager;

use crate::app::context::AppContext;
use crate::{MainWindow, ShellState};

/// How often the UI timer polls the focus timer. The timer measures real
/// elapsed time, so this only affects how smoothly the clock updates.
const TICK_INTERVAL: Duration = Duration::from_millis(250);

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let shell = window.global::<ShellState>();

    shell.on_window_drag_start(WindowManager::begin_native_drag);
    shell.on_window_minimize(WindowManager::minimize);
    shell.on_window_maximize(WindowManager::toggle_maximize);
    shell.on_window_close(|| WindowManager::exit_cleanly());

    if let Some(notice) = &ctx.startup_notice {
        shell.set_notice(notice.as_str().into());
    }

    {
        let ctx = ctx.clone();
        let weak = window.as_weak();
        shell.on_timer_toggle(move || {
            ctx.timer.toggle();
            if let Some(w) = weak.upgrade() {
                refresh_pill(&w, &ctx);
            }
        });
    }

    {
        let weak = window.as_weak();
        shell.on_timer_open(move || {
            if let Some(w) = weak.upgrade() {
                w.global::<ShellState>().invoke_navigate("focus".into());
            }
        });
    }

    let ctx_tick = ctx.clone();
    let weak = window.as_weak();
    ctx.tick_timer.start(slint::TimerMode::Repeated, TICK_INTERVAL, move || {
        let outcome = ctx_tick.timer.tick();
        if let Some(w) = weak.upgrade() {
            refresh_pill(&w, &ctx_tick);
        }
        if outcome.advanced_secs > 0 || outcome.finished {
            let snapshot = ctx_tick.timer.snapshot();
            ctx_tick.notify_tick(&outcome, &snapshot);
        }
    });
}

/// Pushes the current timer state into the shell's timer pill.
pub fn refresh_pill(window: &MainWindow, ctx: &AppContext) {
    let snap = ctx.timer.snapshot();
    let shell = window.global::<ShellState>();
    shell.set_timer_visible(snap.phase != TimerPhase::Idle);
    shell.set_timer_running(snap.phase == TimerPhase::Running);
    shell.set_timer_display(snap.formatted().into());

    let label = match (&snap.task_id, snap.mode) {
        (Some(id), _) => ctx.handle.read(|s| s.task(id).map(|t| t.title.clone())).unwrap_or_default(),
        (None, TimerMode::CountUp) => "Planning".to_string(),
        (None, TimerMode::Countdown) => String::new(),
    };
    shell.set_timer_label(label.into());
}
