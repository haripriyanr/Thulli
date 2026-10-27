slint::include_modules!();

mod parser;
mod security;
mod storage;

use parser::parse_task_title;
use security::SecurityManager;
use storage::{EncryptedStorage, TaskList as StoredList, Task as StoredTask};

use slint::{Model, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;
use chrono::Utc;

#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, SendMessageW, WM_NCLBUTTONDOWN, HTCAPTION,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("[Thulli] Booting security vault...");
    let security_mgr = Arc::new(SecurityManager::init()?);
    let storage = Arc::new(EncryptedStorage::new(security_mgr)?);
    let app_store = Arc::new(Mutex::new(storage.load()));

    let main_window = MainWindow::new()?;

    // Sync initial state from encrypted storage to Slint UI
    {
        let store = app_store.lock().unwrap();

        // Populate lists
        let ui_lists: Vec<ListModel> = store.lists.iter().map(|l| {
            let pending = store.tasks.iter().filter(|t| t.list_id == l.id && !t.completed).count() as i32;
            ListModel {
                id: l.id.clone().into(),
                name: l.name.clone().into(),
                color: l.color.clone().into(),
                pending_count: pending,
                is_archived: l.is_archived,
            }
        }).collect();
        main_window.set_lists(Rc::new(VecModel::from(ui_lists)).into());

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

    // Native Window Dragging on Windows (WM_NCLBUTTONDOWN + HTCAPTION)
    main_window.on_window_drag_start(move || {
        #[cfg(target_os = "windows")]
        unsafe {
            let hwnd = GetForegroundWindow();
            if !hwnd.is_null() {
                ReleaseCapture();
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
            }
        }
    });

    // Window controls
    main_window.on_window_minimize(move || {
        println!("[Thulli] Minimize requested");
    });

    main_window.on_window_maximize(move || {
        println!("[Thulli] Maximize requested");
    });

    main_window.on_window_close(move || {
        println!("[Thulli] Clean shutdown requested");
        std::process::exit(0);
    });

    // Timer controls
    let is_running = Arc::new(AtomicBool::new(false));
    let remaining_seconds = Arc::new(AtomicU32::new(25 * 60));

    let t_toggle = is_running.clone();
    let w_timer = main_window.as_weak();
    main_window.on_toggle_timer(move || {
        let cur = t_toggle.load(Ordering::SeqCst);
        let next = !cur;
        t_toggle.store(next, Ordering::SeqCst);
        if let Some(w) = w_timer.upgrade() {
            w.set_timer_running(next);
        }
    });

    let t_reset_flag = is_running.clone();
    let t_reset_sec = remaining_seconds.clone();
    let w_reset = main_window.as_weak();
    main_window.on_reset_timer(move || {
        t_reset_flag.store(false, Ordering::SeqCst);
        t_reset_sec.store(25 * 60, Ordering::SeqCst);
        if let Some(w) = w_reset.upgrade() {
            w.set_timer_running(false);
            w.set_timer_display("25:00".into());
        }
    });

    // Create New List Callback
    let store_cl = app_store.clone();
    let storage_cl = storage.clone();
    let w_cl = main_window.as_weak();
    main_window.on_create_list(move |name, color| {
        let new_id = Uuid::new_v4().to_string();
        let new_list = StoredList {
            id: new_id.clone(),
            name: name.to_string(),
            color: color.to_string(),
            is_archived: false,
            sort_order: 0,
            created_at: Utc::now(),
        };

        let mut st = store_cl.lock().unwrap();
        st.lists.push(new_list);
        let _ = storage_cl.save(&st);

        // Update UI
        if let Some(w) = w_cl.upgrade() {
            let current = w.get_lists();
            let mut list_vec: Vec<ListModel> = current.iter().collect();
            list_vec.push(ListModel {
                id: new_id.into(),
                name: name.clone(),
                color: color.into(),
                pending_count: 0,
                is_archived: false,
            });
            w.set_lists(Rc::new(VecModel::from(list_vec)).into());
        }
        println!("[Thulli] Created list '{}' securely in encrypted store", name);
    });

    // Add Task with Blitzit Smart Title & Duration Parser
    let store_add = app_store.clone();
    let storage_add = storage.clone();
    let w_add = main_window.as_weak();
    main_window.on_add_task(move |raw_input, list_id, default_board| {
        let parsed = parse_task_title(raw_input.as_str(), default_board.as_str());
        let new_task_id = Uuid::new_v4().to_string();

        let new_task = StoredTask {
            id: new_task_id.clone(),
            list_id: list_id.to_string(),
            title: parsed.cleaned_title.clone(),
            estimate_mins: parsed.estimate_mins,
            time_taken_secs: 0,
            completed: false,
            completed_at: None,
            board: parsed.target_board.clone(),
            sort_order: 0,
            created_at: Utc::now(),
        };

        let mut st = store_add.lock().unwrap();
        st.tasks.push(new_task);
        let _ = storage_add.save(&st);

        if let Some(w) = w_add.upgrade() {
            let current = w.get_tasks();
            let mut task_vec: Vec<TaskModel> = current.iter().collect();
            task_vec.push(TaskModel {
                id: new_task_id.into(),
                list_id: list_id.clone(),
                title: parsed.cleaned_title.clone().into(),
                estimate_mins: parsed.estimate_mins as i32,
                completed: false,
                board: parsed.target_board.clone().into(),
            });
            w.set_tasks(Rc::new(VecModel::from(task_vec)).into());
        }
        println!("[Thulli] Added task '{}' with estimate {}m to board '{}'", parsed.cleaned_title, parsed.estimate_mins, parsed.target_board);
    });

    // Toggle Task Completed
    let store_tt = app_store.clone();
    let storage_tt = storage.clone();
    let w_tt = main_window.as_weak();
    main_window.on_toggle_task(move |task_id| {
        let mut st = store_tt.lock().unwrap();
        let mut completed_state = false;
        for t in &mut st.tasks {
            if t.id == task_id.as_str() {
                t.completed = !t.completed;
                completed_state = t.completed;
                t.completed_at = if t.completed { Some(Utc::now()) } else { None };
                if t.completed {
                    t.board = "done".to_string();
                }
            }
        }
        let _ = storage_tt.save(&st);

        if let Some(w) = w_tt.upgrade() {
            let current = w.get_tasks();
            let mut tasks_vec: Vec<TaskModel> = current.iter().collect();
            for item in &mut tasks_vec {
                if item.id == task_id {
                    item.completed = completed_state;
                    if completed_state {
                        item.board = "done".into();
                    }
                }
            }
            w.set_tasks(Rc::new(VecModel::from(tasks_vec)).into());
        }
    });

    // Toggle Blocked App
    let store_ba = app_store.clone();
    let storage_ba = storage.clone();
    let w_ba = main_window.as_weak();
    main_window.on_toggle_blocked_app(move |app_id| {
        let mut st = store_ba.lock().unwrap();
        let mut new_active = false;
        for a in &mut st.blocked_apps {
            if a.id == app_id.as_str() {
                a.active = !a.active;
                new_active = a.active;
            }
        }
        let _ = storage_ba.save(&st);

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
        println!("[Thulli Shield] Toggled app {} active state to {}", app_id, new_active);
    });

    // Switch App Mode
    let store_mode = app_store.clone();
    let storage_mode = storage.clone();
    let w_mode = main_window.as_weak();
    main_window.on_switch_app_mode(move |mode| {
        println!("[Thulli] Switching presentation mode: {}", mode);
        let mut st = store_mode.lock().unwrap();
        st.preferences.active_mode = mode.to_string();
        let _ = storage_mode.save(&st);
        if let Some(w) = w_mode.upgrade() {
            w.set_active_mode(mode);
        }
    });

    // Background timer tick thread
    let bg_running = is_running.clone();
    let bg_sec = remaining_seconds.clone();
    let w_bg = main_window.as_weak();

    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            if bg_running.load(Ordering::SeqCst) {
                let sec = bg_sec.load(Ordering::SeqCst);
                if sec > 0 {
                    let next = sec - 1;
                    bg_sec.store(next, Ordering::SeqCst);
                    let display = format!("{:02}:{:02}", next / 60, next % 60);

                    let w_w = w_bg.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_w.upgrade() {
                            w.set_timer_display(display.into());
                        }
                    });
                } else {
                    bg_running.store(false, Ordering::SeqCst);
                    let w_w = w_bg.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_w.upgrade() {
                            w.set_timer_running(false);
                        }
                    });
                }
            }
        }
    });

    println!("[Thulli] Launching Custom Frameless Window with Native Dragging & Full Kanban...");
    main_window.run()?;
    Ok(())
}
