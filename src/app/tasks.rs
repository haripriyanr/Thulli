//! Wiring for the tasks screen (ui/screens/tasks.slint).
//!
//! Foundation stub: fills the task and list models once at launch. Bind the
//! `TasksState` callbacks and keep the models fresh as the screen is built out.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::context::AppContext;
use crate::app::convert::{list_model, task_model};
use crate::{MainWindow, TasksState};

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let state = window.global::<TasksState>();
    ctx.handle.read(|store| {
        let tasks: Vec<_> = store.tasks.iter().map(|t| task_model(store, t)).collect();
        let lists: Vec<_> = store.lists.iter().filter(|l| !l.is_archived).map(|l| list_model(store, l)).collect();
        let open = store.tasks.iter().filter(|t| !t.is_step() && t.is_pending()).count();
        state.set_tasks(ModelRc::from(Rc::new(VecModel::from(tasks))));
        state.set_lists(ModelRc::from(Rc::new(VecModel::from(lists))));
        state.set_all_count(open as i32);
    });
}
