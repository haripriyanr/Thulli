//! Wiring for the shield screen (ui/screens/shield.slint).
//!
//! Binds the rule callbacks and runs the enforcement loop. Rules and the
//! enforcement itself live in `ShieldService`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use thulli_core::services::shield::WebStatus;

use crate::app::context::AppContext;
use crate::app::convert::{blocked_app_model, blocked_site_model};
use crate::{MainWindow, ShieldState};

/// How often the shield checks the timer. App checks are throttled further
/// inside the service.
const SYNC_INTERVAL: Duration = Duration::from_secs(1);

thread_local! {
    /// Keeps the enforcement timer alive for the life of the UI thread.
    static SYNC_TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

fn reload(window: &MainWindow, ctx: &AppContext) {
    let state = window.global::<ShieldState>();
    let apps: Vec<_> = ctx.shield.apps().iter().map(blocked_app_model).collect();
    let sites: Vec<_> = ctx.shield.sites().iter().map(blocked_site_model).collect();
    state.set_apps(ModelRc::from(Rc::new(VecModel::from(apps))));
    state.set_sites(ModelRc::from(Rc::new(VecModel::from(sites))));
    state.set_apps_enabled(ctx.shield.apps_enabled());
    state.set_sites_enabled(ctx.shield.sites_enabled());
}

/// Shows the administrator note whenever website blocking is on (or being
/// set up) but the hosts file cannot be changed.
fn refresh_note(window: &MainWindow, ctx: &AppContext) {
    let state = window.global::<ShieldState>();
    let note = if ctx.shield.sites_enabled() || !ctx.shield.sites().is_empty() {
        ctx.shield.web_availability_message()
    } else {
        String::new()
    };
    state.set_sites_note(note.into());
}

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    // Remove anything left behind by a previous run that ended mid-session.
    ctx.shield.cleanup_stale();

    reload(window, ctx);
    refresh_note(window, ctx);

    let state = window.global::<ShieldState>();

    {
        let (w, c) = (window.as_weak(), ctx.clone());
        state.on_add_app(move |name, process| {
            let Some(w) = w.upgrade() else { return };
            let st = w.global::<ShieldState>();
            match c.shield.add_app(name.as_str(), process.as_str()) {
                Ok(_) => st.set_app_error(SharedString::new()),
                Err(e) => st.set_app_error(e.to_string().into()),
            }
            reload(&w, &c);
        });
    }
    {
        let (w, c) = (window.as_weak(), ctx.clone());
        state.on_toggle_app(move |id| {
            c.shield.toggle_app(id.as_str());
            if let Some(w) = w.upgrade() {
                reload(&w, &c);
            }
        });
    }
    {
        let (w, c) = (window.as_weak(), ctx.clone());
        state.on_remove_app(move |id| {
            c.shield.remove_app(id.as_str());
            if let Some(w) = w.upgrade() {
                reload(&w, &c);
            }
        });
    }
    {
        let (w, c) = (window.as_weak(), ctx.clone());
        state.on_add_site(move |input| {
            let Some(w) = w.upgrade() else { return };
            let st = w.global::<ShieldState>();
            match c.shield.add_site(input.as_str()) {
                Ok(_) => st.set_site_error(SharedString::new()),
                Err(e) => st.set_site_error(e.to_string().into()),
            }
            reload(&w, &c);
            refresh_note(&w, &c);
        });
    }
    {
        let (w, c) = (window.as_weak(), ctx.clone());
        state.on_toggle_site(move |id| {
            c.shield.toggle_site(id.as_str());
            if let Some(w) = w.upgrade() {
                reload(&w, &c);
            }
        });
    }
    {
        let (w, c) = (window.as_weak(), ctx.clone());
        state.on_remove_site(move |id| {
            c.shield.remove_site(id.as_str());
            if let Some(w) = w.upgrade() {
                reload(&w, &c);
            }
        });
    }
    {
        let (w, c) = (window.as_weak(), ctx.clone());
        state.on_set_apps_enabled(move |on| {
            c.shield.set_apps_enabled(on);
            if let Some(w) = w.upgrade() {
                reload(&w, &c);
            }
        });
    }
    {
        let (w, c) = (window.as_weak(), ctx.clone());
        state.on_set_sites_enabled(move |on| {
            c.shield.set_sites_enabled(on);
            if let Some(w) = w.upgrade() {
                reload(&w, &c);
                refresh_note(&w, &c);
            }
        });
    }
    {
        let (w, c) = (window.as_weak(), ctx.clone());
        state.on_refresh_running(move || {
            if let Some(w) = w.upgrade() {
                let names: Vec<SharedString> = c.shield.running_apps().into_iter().map(Into::into).collect();
                w.global::<ShieldState>().set_running(ModelRc::from(Rc::new(VecModel::from(names))));
            }
        });
    }

    // Enforcement loop: independent of timer ticks so pausing or stopping a
    // session lifts website blocking straight away.
    let (w, c) = (window.as_weak(), ctx.clone());
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, SYNC_INTERVAL, move || {
        let report = c.shield.sync(&c.timer.snapshot(), Instant::now());
        let Some(w) = w.upgrade() else { return };
        let st = w.global::<ShieldState>();
        if !report.closed.is_empty() {
            st.set_status(format!("Closed {} to protect your focus.", report.closed.join(", ")).into());
        } else if !thulli_core::services::shield::is_focus_running(&c.timer.snapshot()) {
            st.set_status(SharedString::new());
        }
        if matches!(report.web, WebStatus::NeedsAdmin | WebStatus::Failed(_)) {
            st.set_sites_note(report.web.message().into());
        }
    });
    SYNC_TIMER.with(|t| *t.borrow_mut() = Some(timer));
}
