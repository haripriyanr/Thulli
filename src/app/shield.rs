//! Wiring for the shield screen (ui/screens/shield.slint).
//!
//! Foundation stub: fills the rule models once at launch. Bind the
//! `ShieldState` callbacks and keep the models fresh as the screen is built out.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::context::AppContext;
use crate::app::convert::{blocked_app_model, blocked_site_model};
use crate::{MainWindow, ShieldState};

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let state = window.global::<ShieldState>();
    ctx.handle.read(|store| {
        let apps: Vec<_> = store.blocked_apps.iter().map(blocked_app_model).collect();
        let sites: Vec<_> = store.blocked_sites.iter().map(blocked_site_model).collect();
        state.set_apps(ModelRc::from(Rc::new(VecModel::from(apps))));
        state.set_sites(ModelRc::from(Rc::new(VecModel::from(sites))));
        state.set_apps_enabled(store.preferences.app_shield_active);
        state.set_sites_enabled(store.preferences.web_shield_active);
    });
}
