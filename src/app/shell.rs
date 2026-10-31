//! Shell wiring: window controls, navigation notice, timer pill and the
//! UI-thread tick that drives the focus timer.
//!
//! Owned by the foundation. Screens react to the timer through
//! `AppContext::on_tick` rather than editing this file.

use std::time::Duration;

use slint::ComponentHandle;
use thulli_core::services::alerts::{plan_celebration, RodioPlayer, SoundPlayer};
use thulli_core::services::shortcuts::combo_from_key_event;
use thulli_core::services::timer::Moment;
use thulli_core::services::{TimerMode, TimerPhase};
use thulli_core::window::WindowManager;

use crate::app::context::AppContext;
use crate::app::notes;
use crate::{MainWindow, ShellState};

/// How often the UI timer polls the focus timer. The timer measures real
/// elapsed time, so this only affects how smoothly the clock updates.
const TICK_INTERVAL: Duration = Duration::from_millis(250);
/// How long the success overlay stays up before it hides itself.
const CELEBRATION_SHOW: Duration = Duration::from_millis(1600);
/// Time between pill pulses, and how many on/off steps one alert gets.
const PULSE_STEP: Duration = Duration::from_millis(350);
const PULSE_STEPS: u8 = 7;

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

    wire_shortcuts(window, ctx);
    wire_key_handler(window, ctx);
    wire_celebrations(window, ctx);
    wire_alerts(window, ctx);

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

/// Connects every shortcut action that has a counterpart in the app. The
/// preferences screen connects "preferences" and starts the global poll.
fn wire_shortcuts(window: &MainWindow, ctx: &AppContext) {
    let sc = &ctx.shortcuts;

    sc.on("show_app", show_window(window));
    sc.on("find_focus_timer", navigate(window, "focus"));
    sc.on("search", navigate(window, "tasks"));
    sc.on("new_task", navigate(window, "tasks"));
    sc.on("new_list", navigate(window, "lists"));
    sc.on("dashboard", navigate(window, "today"));
    sc.on("floating_notes", notes::request_toggle_floating_notes);
    sc.on("quick_capture", notes::request_toggle_quick_capture);

    {
        let to_notes = navigate(window, "notes");
        let c = ctx.clone();
        sc.on("add_notes", move || match c.timer.snapshot().task_id {
            Some(id) => notes::open_task_notes(&id),
            None => to_notes(),
        });
    }

    {
        let c = ctx.clone();
        sc.on("pause", move || {
            c.focus.toggle_pause_at(Moment::now());
        });
    }
    {
        let c = ctx.clone();
        sc.on("skip", move || {
            c.focus.skip_at(Moment::now());
        });
    }
    {
        let c = ctx.clone();
        sc.on("start_break", move || c.focus.start_break_at(None, Moment::now()));
    }
    {
        let c = ctx.clone();
        sc.on("next_task", move || {
            c.focus.start_first_at(Moment::now());
        });
    }
    {
        let c = ctx.clone();
        sc.on("finish", move || {
            let outcome = c.focus.complete_task_at(Moment::now());
            if outcome.completed_task.is_some() {
                c.celebrate(!outcome.on_time, false);
            }
        });
    }
}

/// Sends key presses that no text field used to the shortcut registry.
fn wire_key_handler(window: &MainWindow, ctx: &AppContext) {
    let c = ctx.clone();
    window.global::<ShellState>().on_app_key_pressed(move |text, ctrl, alt, shift, meta| {
        combo_from_key_event(text.as_str(), ctrl, alt, shift, meta)
            .is_some_and(|keys| c.shortcuts.handle_app_key(&keys))
    });
}

/// Shows the success overlay for finished tasks and breaks, then hides it.
fn wire_celebrations(window: &MainWindow, ctx: &AppContext) {
    let weak = window.as_weak();
    let c = ctx.clone();
    let sounds = RodioPlayer::new();
    ctx.on_celebration(move |over_estimate, from_break| {
        let prefs = c.preferences.snapshot();
        let Some(celebration) = plan_celebration(&prefs, over_estimate, from_break) else {
            return;
        };
        if let Some(sound) = &celebration.sound {
            sounds.play(sound);
        }
        if !celebration.animation {
            return;
        }
        if let Some(w) = weak.upgrade() {
            let shell = w.global::<ShellState>();
            shell.set_celebration_style(if over_estimate { "ripple" } else { "confetti" }.into());
            shell.set_celebration_playing(true);
        }
        let weak = weak.clone();
        slint::Timer::single_shot(CELEBRATION_SHOW, move || {
            if let Some(w) = weak.upgrade() {
                w.global::<ShellState>().set_celebration_playing(false);
            }
        });
    });
}

/// Flashes the window and pulses the timer pill when an alert fires.
fn wire_alerts(window: &MainWindow, ctx: &AppContext) {
    let weak = window.as_weak();
    ctx.on_alert(move |action| {
        if action.flash {
            WindowManager::flash_attention();
        }
        if action.animate_pill {
            pulse_pill(weak.clone(), PULSE_STEPS);
        }
    });
}

fn pulse_pill(weak: slint::Weak<MainWindow>, remaining: u8) {
    if let Some(w) = weak.upgrade() {
        w.global::<ShellState>().set_pill_alert(remaining % 2 == 1);
    }
    if remaining > 0 {
        slint::Timer::single_shot(PULSE_STEP, move || pulse_pill(weak, remaining - 1));
    }
}

/// A shortcut handler that opens a screen in the shell.
fn navigate(window: &MainWindow, screen: &'static str) -> impl Fn() + 'static {
    let weak = window.as_weak();
    move || {
        if let Some(w) = weak.upgrade() {
            w.global::<ShellState>().invoke_navigate(screen.into());
        }
    }
}

/// A shortcut handler that brings the window back from the background.
fn show_window(window: &MainWindow) -> impl Fn() + 'static {
    let weak = window.as_weak();
    move || {
        if let Some(w) = weak.upgrade() {
            w.window().set_minimized(false);
            let _ = w.show();
        }
    }
}
