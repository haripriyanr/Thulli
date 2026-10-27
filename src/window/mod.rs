//! Native window management services for frameless Slint desktop windows.

#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, SendMessageW, WM_NCLBUTTONDOWN, HTCAPTION,
    ShowWindow, IsZoomed, SW_MINIMIZE, SW_MAXIMIZE, SW_RESTORE,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;

/// OOP wrapper for platform-native window behaviors.
pub struct WindowManager;

impl WindowManager {
    /// Initiates a native Win32 non-client window drag using standard message pump hooks.
    pub fn begin_native_drag() {
        #[cfg(target_os = "windows")]
        unsafe {
            let hwnd = GetForegroundWindow();
            if !hwnd.is_null() {
                ReleaseCapture();
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
            }
        }
    }

    /// Minimizes the current foreground window to the taskbar.
    pub fn minimize() {
        #[cfg(target_os = "windows")]
        unsafe {
            let hwnd = GetForegroundWindow();
            if !hwnd.is_null() {
                ShowWindow(hwnd, SW_MINIMIZE);
            }
        }
    }

    /// Toggles between maximized and restored window states.
    pub fn toggle_maximize() {
        #[cfg(target_os = "windows")]
        unsafe {
            let hwnd = GetForegroundWindow();
            if !hwnd.is_null() {
                if IsZoomed(hwnd) != 0 {
                    ShowWindow(hwnd, SW_RESTORE);
                } else {
                    ShowWindow(hwnd, SW_MAXIMIZE);
                }
            }
        }
    }

    /// Performs a clean application exit.
    pub fn exit_cleanly() -> ! {
        println!("[Thulli] Clean application shutdown requested");
        std::process::exit(0);
    }
}
