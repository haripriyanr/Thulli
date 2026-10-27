//! # Thulli Desktop Application Entry Point
//!
//! Wires the Slint GUI event loop to OOP backend services (TaskService,
//! ListService, TimerService, WindowManager) and encrypted persistence.

slint::include_modules!();

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use slint::{Model, VecModel};

use thulli_core::repository::{EncryptedStorageRepository, StorageRepository};
use thulli_core::security::SecurityManager;
use thulli_core::services::{ListService, TaskService, TimerService};
use thulli_core::window::WindowManager;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("[Thulli] Booting security vault and application services...");

    // 1. Initialize OOP Security Vault and Repository
    let security_mgr = Arc::new(SecurityManager::init()?);
    let repository: Arc<dyn StorageRepository> = Arc::new(EncryptedStorageRepository::new_default(security_mgr)?);
    let app_store = Arc::new(Mutex::new(repository.load()));

    // 2. Initialize OOP Domain Services
    let task_service = Arc::new(TaskService::new(app_store.clone(), repository.clone()));
    let list_service = Arc::new(ListService::new(app_store.clone(), repository.clone()));
    let timer_service = Arc::new(TimerService::new(25)); // 25-minute Pomodoro default

    // 3. Create Slint Main Window
    let main_window = MainWindow::new()?;

    // 4. Initial State Synchronization (Store -> Slint UI)
    {
        let store = app_store.lock().unwrap();

        // Populate lists
        let ui_lists: Vec<ListModel> = store.lists.iter().map(|l| {
            let pending = store.count_pending_for_list(&l.id) as i32;
            ListModel {
                id: l.id.clone().into(),
                name: l.name.clone().into(),
                color: l.color.clone().into(),
                pending_count: pending,
                is_archived: l.is_archived,
            }
        }).collect();
        main_window.set_lists(Rc::new(VecModel::from(ui_lists)).into());

        // Initialize active list
        if let Some(primary) = store.get_primary_list() {
            main_window.set_selected_list_id(primary.id.clone().into());
            main_window.set_selected_list_name(primary.name.clone().into());
        }

        // Populate tasks
        let ui_tasks: Vec<TaskModel> = store.tasks.iter().map(|t| {
            TaskModel {
                id: t.id.clone().into(),
                list_id: t.list_id.clone().into(),
                title: t.title.clone().into(),
                estimate_mins: t.estimate_mins as i32,
                completed: t.completed,
                board: t.board.clone().into(),
            }
        }).collect();
        main_window.set_tasks(Rc::new(VecModel::from(ui_tasks)).into());

        // Populate blocked apps
        let ui_blocked: Vec<BlockedAppModel> = store.blocked_apps.iter().map(|a| {
            BlockedAppModel {
                id: a.id.clone().into(),
                name: a.name.clone().into(),
                process_name: a.process_name.clone().into(),
                active: a.active,
            }
        }).collect();
        main_window.set_blocked_apps(Rc::new(VecModel::from(ui_blocked)).into());

        main_window.set_scratchpad_text(store.preferences.quick_notes.clone().into());
        main_window.set_active_mode(store.preferences.active_mode.clone().into());
    }

    // 5. Window Management Event Wiring
    main_window.on_window_drag_start(move || {
        WindowManager::begin_native_drag();
    });

    main_window.on_window_minimize(move || {
        WindowManager::minimize();
    });

    main_window.on_window_maximize(move || {
        WindowManager::toggle_maximize();
    });

    main_window.on_window_close(move || {
        WindowManager::exit_cleanly();
    });

    // 6. Focus Timer Event Wiring
    let ts_toggle = timer_service.clone();
    let w_timer = main_window.as_weak();
    main_window.on_toggle_timer(move || {
        let is_running = ts_toggle.toggle();
        if let Some(w) = w_timer.upgrade() {
            w.set_timer_running(is_running);
        }
    });

    let ts_reset = timer_service.clone();
    let w_reset = main_window.as_weak();
    main_window.on_reset_timer(move || {
        ts_reset.reset();
        if let Some(w) = w_reset.upgrade() {
            w.set_timer_running(false);
            w.set_timer_display(ts_reset.formatted_display().into());
        }
    });

    // 7. List Management Event Wiring
    let ls_create = list_service.clone();
    let w_create_list = main_window.as_weak();
    main_window.on_create_list(move |name, color| {
        let new_list = ls_create.create_list(name.as_str(), color.as_str());

        if let Some(w) = w_create_list.upgrade() {
            let current = w.get_lists();
            let mut list_vec: Vec<ListModel> = current.iter().collect();
            list_vec.push(ListModel {
                id: new_list.id.clone().into(),
                name: new_list.name.clone().into(),
                color: new_list.color.clone().into(),
                pending_count: 0,
                is_archived: false,
            });
            w.set_lists(Rc::new(VecModel::from(list_vec)).into());
            w.set_selected_list_id(new_list.id.into());
            w.set_selected_list_name(name);
            w.set_active_nav("board".into());
        }
    });

    let ls_delete = list_service.clone();
    let store_dl = app_store.clone();
    let w_dl = main_window.as_weak();
    main_window.on_delete_list(move |list_id| {
        ls_delete.delete_list(list_id.as_str());

        if let Some(w) = w_dl.upgrade() {
            let current_lists = w.get_lists();
            let mut list_vec: Vec<ListModel> = current_lists.iter().collect();
            list_vec.retain(|l| l.id != list_id);
            w.set_lists(Rc::new(VecModel::from(list_vec)).into());

            let current_tasks = w.get_tasks();
            let mut task_vec: Vec<TaskModel> = current_tasks.iter().collect();
            task_vec.retain(|t| t.list_id != list_id);
            w.set_tasks(Rc::new(VecModel::from(task_vec)).into());

            let st = store_dl.lock().unwrap();
            if let Some(primary) = st.get_primary_list() {
                w.set_selected_list_id(primary.id.clone().into());
                w.set_selected_list_name(primary.name.clone().into());
            } else {
                w.set_selected_list_id("".into());
                w.set_selected_list_name("All Lists".into());
                w.set_active_nav("all_lists".into());
            }
        }
    });

    let store_sl = app_store.clone();
    let w_sl = main_window.as_weak();
    main_window.on_select_list(move |list_id| {
        let st = store_sl.lock().unwrap();
        if let Some(target) = st.lists.iter().find(|l| l.id == list_id.as_str()) {
            if let Some(w) = w_sl.upgrade() {
                w.set_selected_list_id(target.id.clone().into());
                w.set_selected_list_name(target.name.clone().into());
                w.set_active_nav("board".into());
            }
        }
    });

    // 8. Task Management Event Wiring
    let ts_add = task_service.clone();
    let w_add = main_window.as_weak();
    main_window.on_add_task(move |raw_input, list_id, default_board| {
        if let Some(created) = ts_add.add_smart_task(raw_input.as_str(), list_id.as_str(), default_board.as_str()) {
            if let Some(w) = w_add.upgrade() {
                if w.get_selected_list_id().is_empty() {
                    w.set_selected_list_id(created.list_id.clone().into());
                }

                let current_tasks = w.get_tasks();
                let mut task_vec: Vec<TaskModel> = current_tasks.iter().collect();
                task_vec.push(TaskModel {
                    id: created.id.clone().into(),
                    list_id: created.list_id.clone().into(),
                    title: created.title.clone().into(),
                    estimate_mins: created.estimate_mins as i32,
                    completed: created.completed,
                    board: created.board.clone().into(),
                });
                w.set_tasks(Rc::new(VecModel::from(task_vec)).into());

                let current_lists = w.get_lists();
                let mut list_vec: Vec<ListModel> = current_lists.iter().collect();
                for l in &mut list_vec {
                    if l.id == created.list_id.as_str() {
                        l.pending_count += 1;
                    }
                }
                w.set_lists(Rc::new(VecModel::from(list_vec)).into());
            }
        }
    });

    let ts_delete = task_service.clone();
    let store_dt = app_store.clone();
    let w_dt = main_window.as_weak();
    main_window.on_delete_task(move |task_id| {
        let mut affected_list = String::new();
        let mut was_pending = false;

        {
            let st = store_dt.lock().unwrap();
            if let Some(t) = st.tasks.iter().find(|t| t.id == task_id.as_str()) {
                affected_list = t.list_id.clone();
                was_pending = !t.completed;
            }
        }

        if ts_delete.delete_task(task_id.as_str()) {
            if let Some(w) = w_dt.upgrade() {
                let current_tasks = w.get_tasks();
                let mut task_vec: Vec<TaskModel> = current_tasks.iter().collect();
                task_vec.retain(|t| t.id != task_id);
                w.set_tasks(Rc::new(VecModel::from(task_vec)).into());

                if was_pending && !affected_list.is_empty() {
                    let current_lists = w.get_lists();
                    let mut list_vec: Vec<ListModel> = current_lists.iter().collect();
                    for l in &mut list_vec {
                        if l.id.as_str() == affected_list.as_str() && l.pending_count > 0 {
                            l.pending_count -= 1;
                        }
                    }
                    w.set_lists(Rc::new(VecModel::from(list_vec)).into());
                }
            }
        }
    });

    let ts_toggle_task = task_service.clone();
    let store_tt = app_store.clone();
    let w_tt = main_window.as_weak();
    main_window.on_toggle_task(move |task_id| {
        if let Some(completed_state) = ts_toggle_task.toggle_task(task_id.as_str()) {
            let mut list_id = String::new();
            {
                let st = store_tt.lock().unwrap();
                if let Some(t) = st.tasks.iter().find(|t| t.id == task_id.as_str()) {
                    list_id = t.list_id.clone();
                }
            }

            if let Some(w) = w_tt.upgrade() {
                let current_tasks = w.get_tasks();
                let mut task_vec: Vec<TaskModel> = current_tasks.iter().collect();
                for t in &mut task_vec {
                    if t.id == task_id {
                        t.completed = completed_state;
                        t.board = if completed_state { "done".into() } else { "today".into() };
                    }
                }
                w.set_tasks(Rc::new(VecModel::from(task_vec)).into());

                let current_lists = w.get_lists();
                let mut list_vec: Vec<ListModel> = current_lists.iter().collect();
                for l in &mut list_vec {
                    if l.id.as_str() == list_id.as_str() {
                        if completed_state && l.pending_count > 0 {
                            l.pending_count -= 1;
                        } else if !completed_state {
                            l.pending_count += 1;
                        }
                    }
                }
                w.set_lists(Rc::new(VecModel::from(list_vec)).into());
            }
        }
    });

    // 9. App Shield & Scratchpad Event Wiring
    let store_ba = app_store.clone();
    let repo_ba = repository.clone();
    let w_ba = main_window.as_weak();
    main_window.on_toggle_blocked_app(move |app_id| {
        let mut new_active = false;
        {
            let mut st = store_ba.lock().unwrap();
            for a in &mut st.blocked_apps {
                if a.id == app_id.as_str() {
                    new_active = a.toggle_active();
                }
            }
            let _ = repo_ba.save(&st);
        }

        if let Some(w) = w_ba.upgrade() {
            let current = w.get_blocked_apps();
            let mut app_vec: Vec<BlockedAppModel> = current.iter().collect();
            for item in &mut app_vec {
                if item.id == app_id {
                    item.active = new_active;
                }
            }
            w.set_blocked_apps(Rc::new(VecModel::from(app_vec)).into());
        }
    });

    let store_note = app_store.clone();
    let repo_note = repository.clone();
    main_window.on_save_scratchpad(move |notes| {
        let mut st = store_note.lock().unwrap();
        st.preferences.quick_notes = notes.to_string();
        let _ = repo_note.save(&st);
    });

    let store_mode = app_store.clone();
    let repo_mode = repository.clone();
    main_window.on_switch_app_mode(move |mode| {
        let mut st = store_mode.lock().unwrap();
        st.preferences.active_mode = mode.to_string();
        let _ = repo_mode.save(&st);
    });

    println!("[Thulli] Launching user interface...");
    main_window.run()?;
    Ok(())
}
