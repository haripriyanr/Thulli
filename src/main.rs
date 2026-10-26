slint::include_modules!();

use slint::{Model, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let main_window = MainWindow::new()?;

    // Shared state
    let is_running = Arc::new(AtomicBool::new(false));
    let remaining_seconds = Arc::new(std::sync::atomic::AtomicU32::new(25 * 60));

    // Handle timer toggle
    let timer_running_clone = is_running.clone();
    let window_weak = main_window.as_weak();
    main_window.on_toggle_timer(move || {
        let currently_running = timer_running_clone.load(Ordering::SeqCst);
        let next_state = !currently_running;
        timer_running_clone.store(next_state, Ordering::SeqCst);

        if let Some(w) = window_weak.upgrade() {
            w.set_timer_running(next_state);
        }
    });

    // Handle timer reset
    let timer_running_reset = is_running.clone();
    let seconds_reset = remaining_seconds.clone();
    let window_weak_reset = main_window.as_weak();
    main_window.on_reset_timer(move || {
        timer_running_reset.store(false, Ordering::SeqCst);
        seconds_reset.store(25 * 60, Ordering::SeqCst);
        if let Some(w) = window_weak_reset.upgrade() {
            w.set_timer_running(false);
            w.set_timer_display("25:00".into());
        }
    });

    // Handle Add Task
    let window_weak_task = main_window.as_weak();
    main_window.on_add_task(move |title| {
        if let Some(w) = window_weak_task.upgrade() {
            let current_tasks = w.get_tasks();
            let mut list: Vec<TaskItem> = current_tasks.iter().collect();
            let next_id = list.len() as i32 + 1;
            list.push(TaskItem {
                id: next_id,
                title,
                completed: false,
                estimate_mins: 25,
            });
            w.set_tasks(Rc::new(VecModel::from(list)).into());
        }
    });

    // Handle Task Completion Toggle
    let window_weak_toggle = main_window.as_weak();
    main_window.on_toggle_task_completed(move |task_id| {
        if let Some(w) = window_weak_toggle.upgrade() {
            let current_tasks = w.get_tasks();
            let mut list: Vec<TaskItem> = current_tasks.iter().collect();
            for task in &mut list {
                if task.id == task_id {
                    task.completed = !task.completed;
                }
            }
            w.set_tasks(Rc::new(VecModel::from(list)).into());
        }
    });

    // Handle Mode Switching
    main_window.on_switch_mode(move |mode| {
        println!("[Thulli] Switched to mode: {}", mode);
    });

    // Handle Blocker App Shield Toggle
    let window_weak_blocker = main_window.as_weak();
    main_window.on_toggle_blocker_app(move |app_id| {
        if let Some(w) = window_weak_blocker.upgrade() {
            let current_apps = w.get_blocked_apps();
            let mut list: Vec<BlockedApp> = current_apps.iter().collect();
            for app in &mut list {
                if app.id == app_id {
                    app.active = !app.active;
                    println!("[Thulli Blocker] App {} ({}) active: {}", app.name, app.process_name, app.active);
                }
            }
            w.set_blocked_apps(Rc::new(VecModel::from(list)).into());
        }
    });

    // Background timer thread
    let timer_running_bg = is_running.clone();
    let seconds_bg = remaining_seconds.clone();
    let window_weak_bg = main_window.as_weak();

    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            if timer_running_bg.load(Ordering::SeqCst) {
                let sec = seconds_bg.load(Ordering::SeqCst);
                if sec > 0 {
                    let next = sec - 1;
                    seconds_bg.store(next, Ordering::SeqCst);
                    let display = format!("{:02}:{:02}", next / 60, next % 60);

                    let w_weak = window_weak_bg.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_weak.upgrade() {
                            w.set_timer_display(display.into());
                        }
                    });
                } else {
                    timer_running_bg.store(false, Ordering::SeqCst);
                    let w_weak = window_weak_bg.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_weak.upgrade() {
                            w.set_timer_running(false);
                        }
                    });
                }
            }
        }
    });

    println!("[Thulli] Launching Slint GUI...");
    main_window.run()?;

    Ok(())
}
