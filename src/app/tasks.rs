//! Wiring for the board (ui/screens/tasks.slint): sidebar, global search,
//! the four columns, the inline editor and undo / redo.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use chrono::Local;
use slint::{ComponentHandle, ModelRc};
use thulli_core::models::{Task, TaskSection, TaskStatus};
use thulli_core::services::tasks::search::{HitKind, SearchHit};
use thulli_core::services::tasks::{parse_date_input, search, BoardScope, DateFilter, TaskEdit};

use crate::app::context::AppContext;
use crate::app::convert::{list_model, model_of, task_model};
use crate::{BoardColumnModel, ListModel, MainWindow, SearchHitModel, ShellState, TaskModel, TasksState};

type Refresh = Rc<dyn Fn()>;

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let collapsed: Rc<RefCell<HashSet<String>>> = Rc::default();
    let refresh: Refresh = {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        let collapsed = collapsed.clone();
        Rc::new(move || {
            if let Some(w) = weak.upgrade() {
                fill(&w, &ctx, &collapsed.borrow());
            }
        })
    };

    let state = window.global::<TasksState>();

    {
        let r = refresh.clone();
        state.on_request_refresh(move || r());
    }

    {
        let (r, weak) = (refresh.clone(), window.as_weak());
        state.on_set_filter(move |filter| {
            if let Some(w) = weak.upgrade() {
                let st = w.global::<TasksState>();
                st.set_filter(filter);
                st.set_editing_id("".into());
                st.set_search_text("".into());
                st.set_search_hits(model_of(Vec::<SearchHitModel>::new()));
            }
            r();
        });
    }

    {
        let (ctx, weak) = (ctx.clone(), window.as_weak());
        state.on_search(move |text| {
            if let Some(w) = weak.upgrade() {
                let hits: Vec<SearchHitModel> =
                    ctx.handle.read(|st| search::run(st, &text)).into_iter().map(hit_model).collect();
                w.global::<TasksState>().set_search_hits(model_of(hits));
            }
        });
    }

    {
        let (ctx, r, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        state.on_search_pick(move |encoded| {
            let Some(w) = weak.upgrade() else { return };
            let st = w.global::<TasksState>();
            let query = st.get_search_text().to_string();
            st.set_search_text("".into());
            st.set_search_hits(model_of(Vec::<SearchHitModel>::new()));

            if let Some(id) = encoded.strip_prefix("task:") {
                st.set_filter("all".into());
                open_editor(&st, &ctx, id);
            } else if let Some(id) = encoded.strip_prefix("list:") {
                st.set_filter(id.into());
            } else {
                match encoded.strip_prefix("action:").unwrap_or_default() {
                    "add-task-with-text" => {
                        ctx.tasks.add_task_recorded(&ctx.history, &query, "", TaskSection::Backlog);
                    }
                    "add-list" => {
                        w.global::<ShellState>().invoke_navigate("lists".into());
                        let lists = w.global::<crate::ListsState>();
                        lists.set_editor_id("".into());
                        lists.set_editor_name("".into());
                        lists.set_editor_color(0);
                        lists.set_editor_open(true);
                    }
                    "go-insights" => w.global::<ShellState>().invoke_navigate("insights".into()),
                    _ => {}
                }
            }
            r();
        });
    }

    {
        let (ctx, r, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        state.on_add_task(move |section, text| {
            if let Some(w) = weak.upgrade() {
                let filter = w.global::<TasksState>().get_filter().to_string();
                let list_id = match BoardScope::parse(&filter) {
                    BoardScope::List(id) => id,
                    _ => String::new(),
                };
                let section = TaskSection::parse(&section).unwrap_or(TaskSection::Today);
                ctx.tasks.add_task_recorded(&ctx.history, &text, &list_id, section);
            }
            r();
        });
    }

    {
        let (ctx, r) = (ctx.clone(), refresh.clone());
        state.on_toggle_complete(move |id| {
            ctx.tasks.toggle_complete(&ctx.history, &id);
            r();
        });
    }

    {
        let (ctx, r, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        state.on_move_task(move |id, section| {
            if let Some(section) = TaskSection::parse(&section) {
                ctx.tasks.move_to_section_recorded(&ctx.history, &id, section);
            }
            close_editor(&weak);
            r();
        });
    }

    {
        let (ctx, r) = (ctx.clone(), refresh.clone());
        state.on_move_next(move |id| {
            let current = ctx.handle.read(|st| st.task(&id).map(|t| t.section));
            let next = match current {
                Some(TaskSection::Backlog) => Some(TaskSection::ThisWeek),
                Some(TaskSection::ThisWeek) => Some(TaskSection::Today),
                _ => None,
            };
            if let Some(next) = next {
                ctx.tasks.move_to_section_recorded(&ctx.history, &id, next);
            }
            r();
        });
    }

    {
        let (ctx, r, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        state.on_set_status(move |id, status| {
            if let Some(status) = TaskStatus::parse(&status) {
                ctx.tasks.set_status_recorded(&ctx.history, &id, status);
            }
            close_editor(&weak);
            r();
        });
    }

    {
        let (ctx, r) = (ctx.clone(), refresh.clone());
        state.on_start_focus(move |id| {
            if let Some(estimate) = ctx.tasks.start_focus(&id) {
                ctx.timer.start_task_count_up(&id, estimate);
            }
            r();
        });
    }

    {
        let (ctx, weak) = (ctx.clone(), window.as_weak());
        state.on_edit_task(move |id| {
            if let Some(w) = weak.upgrade() {
                open_editor(&w.global::<TasksState>(), &ctx, &id);
            }
        });
    }

    {
        let (r, weak) = (refresh.clone(), window.as_weak());
        state.on_close_editor(move || {
            close_editor(&weak);
            r();
        });
    }

    {
        let (ctx, r, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        state.on_save_task(move |id, title, notes, estimate, do_date, deadline| {
            let Some(w) = weak.upgrade() else { return };
            let today = Local::now().date_naive();

            let estimate = estimate.trim();
            let estimate_mins = if estimate.is_empty() {
                0
            } else {
                match estimate.parse::<u32>() {
                    Ok(mins) => mins,
                    Err(_) => return notify(&w, "Estimate needs a whole number of minutes."),
                }
            };
            let do_date = match parse_date_input(&do_date, today) {
                Ok(date) => date,
                Err(message) => return notify(&w, &message),
            };
            let deadline = match parse_date_input(&deadline, today) {
                Ok(date) => date,
                Err(message) => return notify(&w, &message),
            };

            let tag_ids = ctx.handle.read(|st| st.task(&id).map(|t| t.tags.clone()).unwrap_or_default());
            let edit = TaskEdit {
                title: title.to_string(),
                notes: notes.to_string(),
                estimate_mins,
                tag_ids,
                do_date,
                deadline,
            };
            if ctx.tasks.update_task(&ctx.history, &id, edit) {
                close_editor(&weak);
            } else {
                notify(&w, "Give the task a name.");
            }
            r();
        });
    }

    {
        let (ctx, r, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        state.on_delete_task(move |id| {
            ctx.tasks.delete_recorded(&ctx.history, &id);
            close_editor(&weak);
            r();
        });
    }

    {
        let (r, collapsed) = (refresh.clone(), collapsed.clone());
        state.on_toggle_column(move |section| {
            let section = section.to_string();
            {
                let mut set = collapsed.borrow_mut();
                if !set.remove(&section) {
                    set.insert(section);
                }
            }
            r();
        });
    }

    {
        let (ctx, r, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        state.on_undo(move || {
            ctx.history.undo();
            close_editor(&weak);
            r();
        });
    }

    {
        let (ctx, r, weak) = (ctx.clone(), refresh.clone(), window.as_weak());
        state.on_redo(move || {
            ctx.history.redo();
            close_editor(&weak);
            r();
        });
    }

    {
        let weak = window.as_weak();
        state.on_manage_lists(move || {
            if let Some(w) = weak.upgrade() {
                w.global::<ShellState>().invoke_navigate("lists".into());
            }
        });
    }

    refresh();
}

/// Pushes the board state into the Slint global.
fn fill(window: &MainWindow, ctx: &AppContext, collapsed: &HashSet<String>) {
    let state = window.global::<TasksState>();
    let scope = BoardScope::parse(&state.get_filter());
    let board = ctx.tasks.board(&scope, DateFilter::Any);
    let open = ctx.tasks.open_count();

    let (title, lists, all_tasks, columns, column_tasks) = ctx.handle.read(|st| {
        let title = match &scope {
            BoardScope::All => "All tasks".to_string(),
            BoardScope::Logbook => "Logbook".to_string(),
            BoardScope::Waiting => "Waiting".to_string(),
            BoardScope::List(id) => st.lists.iter().find(|l| &l.id == id).map(|l| l.name.clone()).unwrap_or_default(),
        };
        let lists: Vec<ListModel> =
            st.lists.iter().filter(|l| !l.is_archived).map(|l| list_model(st, l)).collect();

        let all_tasks: Vec<TaskModel> = if scope.is_flat() {
            board.flat.iter().map(|t| task_model(st, t)).collect()
        } else {
            board.columns.iter().flat_map(|c| c.tasks.iter()).map(|t| task_model(st, t)).collect()
        };
        let columns: Vec<BoardColumnModel> = board
            .columns
            .iter()
            .map(|c| BoardColumnModel {
                section: c.section.into(),
                title: c.title.into(),
                done_count: c.done as i32,
                total_count: c.total as i32,
                collapsed: collapsed.contains(c.section),
            })
            .collect();
        let column_tasks: Vec<ModelRc<TaskModel>> = board
            .columns
            .iter()
            .map(|c| model_of(c.tasks.iter().map(|t| task_model(st, t)).collect()))
            .collect();
        (title, lists, all_tasks, columns, column_tasks)
    });

    let progress: Vec<f32> = board
        .columns
        .iter()
        .map(|c| if c.total == 0 { 0.0 } else { c.done as f32 / c.total as f32 })
        .collect();

    state.set_scope_title(title.into());
    state.set_tasks(model_of(all_tasks));
    state.set_lists(model_of(lists));
    state.set_all_count(open as i32);
    state.set_columns(model_of(columns));
    state.set_column_progress(model_of(progress));
    state.set_column_tasks(model_of(column_tasks));
    state.set_can_undo(ctx.history.can_undo());
    state.set_can_redo(ctx.history.can_redo());
}

/// Fills the editor fields from a task and opens it.
fn open_editor(st: &TasksState, ctx: &AppContext, id: &str) {
    let fields = ctx.handle.read(|s| s.task(id).map(editor_fields));
    if let Some((title, notes, estimate, do_date, deadline)) = fields {
        st.set_editing_id(id.into());
        st.set_editor_title(title.into());
        st.set_editor_notes(notes.into());
        st.set_editor_estimate(estimate.into());
        st.set_editor_do_date(do_date.into());
        st.set_editor_deadline(deadline.into());
    }
}

fn editor_fields(task: &Task) -> (String, String, String, String, String) {
    let estimate = if task.estimate_mins == 0 { String::new() } else { task.estimate_mins.to_string() };
    let date = |d: Option<chrono::NaiveDate>| d.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
    (task.title.clone(), task.notes.clone(), estimate, date(task.do_date), date(task.deadline))
}

fn close_editor(weak: &slint::Weak<MainWindow>) {
    if let Some(w) = weak.upgrade() {
        w.global::<TasksState>().set_editing_id("".into());
    }
}

fn notify(window: &MainWindow, message: &str) {
    window.global::<ShellState>().set_notice(message.into());
}

fn hit_model(hit: SearchHit) -> SearchHitModel {
    let kind = match hit.kind {
        HitKind::Task => "task",
        HitKind::List => "list",
        HitKind::Action => "action",
    };
    SearchHitModel {
        encoded_id: hit.encoded_id().into(),
        kind: kind.into(),
        title: hit.title.into(),
        subtitle: hit.subtitle.into(),
    }
}
