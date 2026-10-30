//! Wiring for the lists screen (ui/screens/lists.slint).
//!
//! Foundation stub: fills the list models once at launch. Bind the
//! `ListsState` callbacks and keep the models fresh as the screen is built out.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::context::AppContext;
use crate::app::convert::list_model;
use crate::{ListsState, MainWindow};

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let state = window.global::<ListsState>();
    ctx.handle.read(|store| {
        let (archived, active): (Vec<_>, Vec<_>) = store.lists.iter().partition(|l| l.is_archived);
        let active: Vec<_> = active.into_iter().map(|l| list_model(store, l)).collect();
        let archived: Vec<_> = archived.into_iter().map(|l| list_model(store, l)).collect();
        state.set_lists(ModelRc::from(Rc::new(VecModel::from(active))));
        state.set_archived_lists(ModelRc::from(Rc::new(VecModel::from(archived))));
    });
}
