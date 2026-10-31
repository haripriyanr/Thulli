//! Wiring for the lists screen (ui/screens/lists.slint): list rows, the
//! create / edit panel, duplicate, archive and the archive view.

use slint::ComponentHandle;
use thulli_core::services::lists::IconChange;

use crate::app::context::AppContext;
use crate::app::convert::{list_model, model_of, task_model};
use crate::{ListModel, ListsState, MainWindow, ShellState, TaskModel, TasksState};

/// Colours offered in the editor, in the order of the Slint palette.
const PALETTE: [&str; 6] = ["#3CC9C4", "#6F98E8", "#B3C065", "#EF82EF", "#DB9D20", "#6FDBAB"];

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let refresh = {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        move || {
            if let Some(w) = weak.upgrade() {
                fill(&w, &ctx);
            }
        }
    };

    let state = window.global::<ListsState>();

    {
        let r = refresh.clone();
        state.on_request_refresh(move || r());
    }

    {
        let (ctx, r, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        state.on_save_list(move |id, name, color| {
            let Some(w) = weak.upgrade() else { return };
            let color = PALETTE.get(color.max(0) as usize).copied().unwrap_or(PALETTE[0]);
            let result = if id.is_empty() {
                ctx.lists.create_list_with_icon(&name, color, "").map(|_| ())
            } else {
                ctx.lists.edit_list(&id, &name, color, IconChange::Keep)
            };
            match result {
                Ok(()) => w.global::<ListsState>().set_editor_open(false),
                Err(message) => w.global::<ShellState>().set_notice(message.into()),
            }
            r();
        });
    }

    {
        let (ctx, weak) = (ctx.clone(), window.as_weak());
        state.on_edit_list(move |id| {
            let Some(w) = weak.upgrade() else { return };
            let found = ctx.handle.read(|st| st.lists.iter().find(|l| l.id == id.as_str()).map(|l| (l.name.clone(), l.color.clone())));
            if let Some((name, color)) = found {
                let lists = w.global::<ListsState>();
                let index = PALETTE.iter().position(|c| c.eq_ignore_ascii_case(&color)).unwrap_or(0);
                lists.set_editor_id(id);
                lists.set_editor_name(name.into());
                lists.set_editor_color(index as i32);
                lists.set_editor_open(true);
            }
        });
    }

    {
        let (ctx, r) = (ctx.clone(), refresh.clone());
        state.on_duplicate_list(move |id| {
            ctx.lists.duplicate_list(&id);
            r();
        });
    }

    {
        let (ctx, r) = (ctx.clone(), refresh.clone());
        state.on_archive_list(move |id| {
            ctx.lists.archive_list(&id);
            r();
        });
    }

    {
        let (ctx, r) = (ctx.clone(), refresh.clone());
        state.on_restore_list(move |id| {
            ctx.lists.restore_list(&id);
            r();
        });
    }

    {
        let weak = window.as_weak();
        state.on_open_list(move |id| {
            let Some(w) = weak.upgrade() else { return };
            // The board shows the list named by its filter.
            w.global::<TasksState>().set_filter(id);
            w.global::<ShellState>().invoke_navigate("tasks".into());
        });
    }

    {
        let (ctx, r) = (ctx.clone(), refresh.clone());
        state.on_reopen_task(move |id| {
            ctx.lists.reopen_task(&id);
            r();
        });
    }

    refresh();
}

/// Pushes active lists, archived lists and finished tasks into the Slint global.
fn fill(window: &MainWindow, ctx: &AppContext) {
    let state = window.global::<ListsState>();
    let finished = ctx.lists.archived_done_tasks();

    let (active, archived, done): (Vec<ListModel>, Vec<ListModel>, Vec<TaskModel>) = ctx.handle.read(|st| {
        let active = st.lists.iter().filter(|l| !l.is_archived).map(|l| list_model(st, l)).collect();
        let archived = st.lists.iter().filter(|l| l.is_archived).map(|l| list_model(st, l)).collect();
        let done = finished.iter().map(|t| task_model(st, t)).collect();
        (active, archived, done)
    });

    state.set_lists(model_of(active));
    state.set_archived_lists(model_of(archived));
    state.set_archived_tasks(model_of(done));
}
