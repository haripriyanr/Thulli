// Thulli desktop entry point.
//
// Frameless liquid-glass shell. Window move is handled by Slint's
// `WindowMoveArea`; minimize/maximize/close go through Slint's portable window
// API, so the same code works on Windows, macOS and Linux. Frameless edge
// resize and the fluid maximize/restore morph are driven programmatically
// here: Slint exposes no resize builtin, and native OS maximize snaps with a
// visible jump.

slint::include_modules!();

#[allow(dead_code)]
mod telemetry;
mod convert;
mod data_hub;
use data_hub::DataHub;

use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use std::sync::{Arc, Mutex};
#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicBool, Ordering};
use thulli_core::repository::{EncryptedStorageRepository, StorageRepository};
use thulli_core::services::{PreferencesService, StoreHandle};
use thulli_core::security::SecurityManager;

// Must match min-width / min-height in ui/app.slint (logical px).
const MIN_WIDTH: f32 = 360.0;
const MIN_HEIGHT: f32 = 480.0;

// Shared motion token for window state changes: one easing, one duration,
// and one atomic native bounds update per frame.
#[cfg(target_os = "windows")]
const MORPH_MS: f32 = 260.0;
#[cfg(target_os = "windows")]
const MORPH_TICK: Duration = Duration::from_millis(16);

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn current_rect(window: &slint::Window) -> Rect {
    let p = window.position();
    let s = window.size();
    Rect {
        x: p.x as f32,
        y: p.y as f32,
        w: s.width as f32,
        h: s.height as f32,
    }
}

struct ResizeState {
    edge: String,
    // Absolute pointer position (physical px) when the drag started.
    pointer: (f32, f32),
    // Window rect (physical px) when the drag started.
    start: Rect,
}

/// Eases the window rect from `from` to `to` over MORPH_MS (smooth ease-in-out),
/// applying position and size together so the compositor never presents an
/// intermediate half-updated rectangle.
///
/// The slot doubles as interruption state: starting a new morph drops the
/// previous timer (a dropped slint::Timer stops), and a finished morph takes
/// its own timer out of the slot — safe because Slint releases the
/// timer-list borrow before invoking a callback.
#[cfg(target_os = "windows")]
fn start_morph(
    weak: &slint::Weak<MainWindow>,
    slot: &Rc<RefCell<Option<slint::Timer>>>,
    from: Rect,
    to: Rect,
) {
    let timer = slint::Timer::default();
    let began = Instant::now();
    let slot_for_callback = slot.clone();
    let weak = weak.clone();
    timer.start(slint::TimerMode::Repeated, MORPH_TICK, move || {
        let Some(window) = weak.upgrade() else {
            slot_for_callback.borrow_mut().take();
            return;
        };
        let t = (began.elapsed().as_secs_f32() * 1000.0 / MORPH_MS).min(1.0);
        let eased = t * t * (3.0 - 2.0 * t);
        let r = Rect {
            x: from.x + (to.x - from.x) * eased,
            y: from.y + (to.y - from.y) * eased,
            w: from.w + (to.w - from.w) * eased,
            h: from.h + (to.h - from.h) * eased,
        };
        let wnd = window.window();
        if !set_window_rect(&wnd, r) {
            wnd.set_position(slint::PhysicalPosition::new(r.x.round() as i32, r.y.round() as i32));
            wnd.set_size(slint::PhysicalSize::new(
                r.w.round() as u32,
                r.h.round() as u32,
            ));
        }
        if t >= 1.0 {
            slot_for_callback.borrow_mut().take();
        }
    });
    *slot.borrow_mut() = Some(timer);
}

/// Updates the native window's position and size with one Win32 operation.
/// Two separate Slint calls can allow an intermediate frame to be presented,
/// which makes the maximize/restore morph look like alternating steps.
#[cfg(target_os = "windows")]
fn set_window_rect(window: &slint::Window, rect: Rect) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
    };

    let provider = window.window_handle();
    let Ok(handle) = provider.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };
    unsafe {
        SetWindowPos(
            handle.hwnd.get() as _,
            std::ptr::null_mut(),
            rect.x.round() as i32,
            rect.y.round() as i32,
            rect.w.round() as i32,
            rect.h.round() as i32,
            SWP_NOACTIVATE | SWP_NOZORDER,
        ) != 0
    }
}

/// Length of one full drift of the background orbs, in seconds.
const BACKGROUND_LOOP_SECS: f32 = 40.0;
/// Interval between background updates (about 20 per second).
const BACKGROUND_FRAME: Duration = Duration::from_millis(50);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    telemetry::install_crash_hook();
    let security = Arc::new(SecurityManager::init()?);
    let repository: Arc<dyn StorageRepository> = Arc::new(EncryptedStorageRepository::new_default(security)?);
    let report = repository.load_with_report();
    let state = Arc::new(Mutex::new(report.store));
    let handle = StoreHandle::new(state.clone(), repository.clone());
    let preferences = Arc::new(PreferencesService::new(handle.clone()));
    let window = MainWindow::new()?;
    let shell = window.global::<ShellState>();

    // Restore the saved theme before the first frame. The system option stays
    // selected until the user explicitly chooses light or dark.
    let theme_choice = match preferences.snapshot().theme {
        thulli_core::models::ThemeChoice::System => "system",
        thulli_core::models::ThemeChoice::Light => "light",
        thulli_core::models::ThemeChoice::Dark => "dark",
    };
    window.global::<Theme>().set_theme_choice(theme_choice.into());
    window.global::<ShellState>().set_theme_choice(theme_choice.into());
    window.global::<ShellState>().set_is_macos(thulli_core::window::WindowManager::is_macos());
    {
        let preferences = preferences.clone();
        shell.on_theme_choice_changed(move |choice| {
            let _ = preferences.set_text("theme", choice.as_str());
        });
    }

    // --- Saved data: tasks, notes, insights, settings and the Pomodoro timer
    let hub = DataHub::new(state.clone(), repository.clone());
    hub.attach(&window);

    // One tick per second. Each tick advances the timer and refreshes the window.
    let _pomodoro_ticker = slint::Timer::default();
    {
        let hub = hub.clone();
        let weak = window.as_weak();
        _pomodoro_ticker.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
            if let Some(w) = weak.upgrade() {
                hub.tick_pomodoro(&w);
            }
        });
    }

    // Background drift: the orbs complete one loop every BACKGROUND_LOOP_SECS,
    // updated BACKGROUND_FRAME times a second.
    let _background_ticker = slint::Timer::default();
    {
        let weak = window.as_weak();
        let started = Instant::now();
        _background_ticker.start(slint::TimerMode::Repeated, BACKGROUND_FRAME, move || {
            if let Some(w) = weak.upgrade() {
                let phase = (started.elapsed().as_secs_f32() / BACKGROUND_LOOP_SECS).fract();
                w.global::<AppData>().set_background_phase(phase);
            }
        });
    }

    // Keep sampling and file I/O off the UI thread. The sampler is only
    // enabled for developer builds; release builds install crash reporting
    // below without collecting performance data.
    #[cfg(debug_assertions)]
    {
        let fps = telemetry::FpsCounter::new();
        let _ = fps.attach(&window.window());
        let telemetry_running = Arc::new(AtomicBool::new(true));
        let sampler_running = telemetry_running.clone();
        std::thread::Builder::new()
            .name("thulli-telemetry".into())
            .spawn(move || {
                let mut sampler = telemetry::Sampler::new();
                telemetry::log_event("started");
                let mut last = Instant::now();
                loop {
                    std::thread::sleep(Duration::from_secs(5));
                    if !sampler_running.load(Ordering::Relaxed) {
                        break;
                    }
                    let now = Instant::now();
                    let elapsed = now.duration_since(last).as_secs_f32();
                    last = now;
                    let _ = sampler.log(fps.take_fps(elapsed));
                }
                telemetry::log_event("stopped");
            })
            .ok();
        let _telemetry_watchdog = slint::Timer::default();
        let weak = window.as_weak();
        let running = telemetry_running;
        _telemetry_watchdog.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
            if weak.upgrade().is_none() {
                running.store(false, Ordering::Relaxed);
            }
        });
    }

    #[cfg(target_os = "windows")]
    {
        schedule_corner_update(window.as_weak());
    }

    {
        let weak = window.as_weak();
        shell.on_window_minimize(move || {
            if let Some(w) = weak.upgrade() {
                w.window().set_minimized(true);
            }
        });
    }

    // --- Fluid maximize / restore ----------------------------------------
    // Windows: the window rect itself eases to the monitor work area. The
    // OS is asked to maximize for one instruction only — long enough to
    // read back the correct per-monitor work-area rect (taskbars, docks),
    // then handed the state back — both calls land in a single event-loop
    // turn, so no frame is painted between them. From there the morph owns
    // the rect; the OS never sees a maximize, which keeps its restore
    // bookkeeping from fighting our animation.
    // Other platforms: native set_maximized (OS owns the transition).
    let morph_slot: Rc<RefCell<Option<slint::Timer>>> = Rc::new(RefCell::new(None));
    let restore_rect: Rc<RefCell<Option<Rect>>> = Rc::new(RefCell::new(None));
    let default_rect = current_rect(&window.window());

    {
        let weak = window.as_weak();
        let morph_slot = morph_slot.clone();
        let restore_rect = restore_rect.clone();
        shell.on_window_maximize(move || {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let wnd = w.window();
            let shell = w.global::<ShellState>();
            let maximizing = !shell.get_is_maximized();

            // Interrupt any in-flight morph; it freezes where it is and the
            // new one continues from that rect.
            morph_slot.borrow_mut().take();

            #[cfg(not(target_os = "windows"))]
            {
                let _ = (&restore_rect, &default_rect);
                wnd.set_maximized(maximizing);
                shell.set_is_maximized(maximizing);
            }

            #[cfg(target_os = "windows")]
            if maximizing {
                let from = current_rect(&wnd);
                *restore_rect.borrow_mut() = Some(from);
                shell.set_is_maximized(true);
                if let Some(to) = monitor_work_area(&wnd) {
                    start_morph(&weak, &morph_slot, from, to);
                } else {
                    // Fall back to the native path if Windows cannot report
                    // the monitor work area for this handle.
                    wnd.set_maximized(true);
                }
            } else {
                if wnd.is_maximized() {
                    // Maximized externally (snap, Win+Up): let the OS restore
                    // first, then ease from wherever it lands.
                    wnd.set_maximized(false);
                }
                let from = current_rect(&wnd);
                let to = restore_rect.borrow_mut().take().unwrap_or(default_rect);
                shell.set_is_maximized(false);
                start_morph(&weak, &morph_slot, from, to);
            }
        });
    }

    shell.on_window_close(|| {
        let _ = slint::quit_event_loop();
    });

    // --- Frameless edge resize -------------------------------------------
    // Slint has no resize builtin, so the .slint edge strips report
    // window-relative pointer positions and Rust applies the rect math:
    // physical px throughout, deltas from the drag-start snapshot, clamped
    // to the same minimums as ui/app.slint (scaled by DPI).
    let resize_state: Rc<RefCell<Option<ResizeState>>> = Rc::new(RefCell::new(None));

    {
        let weak = window.as_weak();
        let state = resize_state.clone();
        shell.on_window_resize_start(move |edge, px, py| {
            if let Some(w) = weak.upgrade() {
                let wnd = w.window();
                let sf = wnd.scale_factor();
                let p = wnd.position();
                *state.borrow_mut() = Some(ResizeState {
                    edge: edge.to_string(),
                    pointer: (p.x as f32 + px * sf, p.y as f32 + py * sf),
                    start: current_rect(&wnd),
                });
            }
        });
    }

    {
        let weak = window.as_weak();
        let state = resize_state.clone();
        shell.on_window_resize_move(move |px, py| {
            let Some(current) = state.borrow().as_ref().map(|s| {
                (
                    s.edge.clone(),
                    s.pointer,
                    s.start,
                )
            }) else {
                return;
            };
            let (edge, pointer0, start) = current;

            let Some(w) = weak.upgrade() else {
                return;
            };
            let wnd = w.window();
            let sf = wnd.scale_factor();
            let p = wnd.position();
            let pointer = (p.x as f32 + px * sf, p.y as f32 + py * sf);
            let (dx, dy) = (pointer.0 - pointer0.0, pointer.1 - pointer0.1);

            let min_w = (MIN_WIDTH * sf).round();
            let min_h = (MIN_HEIGHT * sf).round();

            let mut r = start;
            let left = edge.contains("left");
            let right = edge.contains("right");
            let top = edge.contains("top");
            let bottom = edge.contains("bottom");

            if right {
                r.w = start.w + dx;
            }
            if left {
                r.w = start.w - dx;
                r.x = start.x + dx;
            }
            if bottom {
                r.h = start.h + dy;
            }
            if top {
                r.h = start.h - dy;
                r.y = start.y + dy;
            }
            if r.w < min_w {
                if left {
                    r.x = start.x + start.w - min_w;
                }
                r.w = min_w;
            }
            if r.h < min_h {
                if top {
                    r.y = start.y + start.h - min_h;
                }
                r.h = min_h;
            }

            wnd.set_size(slint::PhysicalSize::new(r.w.round() as u32, r.h.round() as u32));
            if left || top {
                wnd.set_position(slint::PhysicalPosition::new(
                    r.x.round() as i32,
                    r.y.round() as i32,
                ));
            }
        });
    }

    {
        let state = resize_state.clone();
        shell.on_window_resize_end(move || {
            state.borrow_mut().take();
        });
    }

    // Keep the restore glyph in sync with the OS. On Windows the morph
    // owns maximize state (the OS flag stays false), so only *adopt* an
    // OS-side maximize (snap, Win+Up) — never overwrite ours. Elsewhere
    // native set_maximized is authoritative, so sync both ways.
    let _sync_timer = slint::Timer::default();
    {
        let weak = window.as_weak();
        _sync_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(250),
            move || {
                if let Some(w) = weak.upgrade() {
                    let maximized = w.window().is_maximized();
                    let shell = w.global::<ShellState>();
                    #[cfg(target_os = "windows")]
                    if maximized && !shell.get_is_maximized() {
                        shell.set_is_maximized(true);
                    }
                    #[cfg(not(target_os = "windows"))]
                    if shell.get_is_maximized() != maximized {
                        shell.set_is_maximized(maximized);
                    }
                }
            },
        );
    }

    Ok(window.run()?)
}

#[cfg(target_os = "windows")]
fn schedule_corner_update(weak: slint::Weak<MainWindow>) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        if let Some(window) = weak.upgrade() {
            if !set_rounded_window_corners(&window.window()) {
                schedule_corner_update(weak);
            }
        }
    });
}

#[cfg(target_os = "windows")]
fn set_rounded_window_corners(window: &slint::Window) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };

    let provider = window.window_handle();
    let Ok(handle) = provider.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };
    // The shell draws its own rounded frame, so the system must not round
    // the window too: two different radii leave a visible corner artifact.
    let preference = DWMWCP_DONOTROUND;
    unsafe {
        DwmSetWindowAttribute(
            handle.hwnd.get() as _,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&preference as *const i32).cast(),
            std::mem::size_of_val(&preference) as u32,
        ) == 0
    }
}

/// Gets the physical work-area rectangle of the monitor containing `window`.
/// This is stable immediately, unlike reading the window rectangle directly
/// after requesting maximize (Windows updates that asynchronously).
#[cfg(target_os = "windows")]
fn monitor_work_area(window: &slint::Window) -> Option<Rect> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    };

    let provider = window.window_handle();
    let handle = provider.window_handle().ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    let hwnd = handle.hwnd.get() as _;
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    let area = info.rcWork;
    Some(Rect {
        x: area.left as f32,
        y: area.top as f32,
        w: (area.right - area.left) as f32,
        h: (area.bottom - area.top) as f32,
    })
}
