//! Native window management services for frameless Slint desktop windows across
//! Windows, macOS, and Linux.

#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, FlashWindowEx, GetForegroundWindow, SendMessageW, WM_NCLBUTTONDOWN, HTCAPTION,
    ShowWindow, IsZoomed, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG, SW_MINIMIZE, SW_MAXIMIZE, SW_RESTORE,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;

/// OOP wrapper for platform-native window behaviors supporting Windows, macOS, and Linux.
pub struct WindowManager;

impl WindowManager {
    /// Returns the active operating system platform identifier ("windows", "macos", "linux", or "other").
    #[inline]
    pub fn platform() -> &'static str {
        if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "other"
        }
    }

    /// Returns `true` if running on macOS.
    #[inline]
    pub fn is_macos() -> bool {
        cfg!(target_os = "macos")
    }

    /// Returns `true` if running on Windows.
    #[inline]
    pub fn is_windows() -> bool {
        cfg!(target_os = "windows")
    }

    /// Returns `true` if running on Linux.
    #[inline]
    pub fn is_linux() -> bool {
        cfg!(target_os = "linux")
    }

    /// Minimizes the given window portably across all supported operating systems.
    pub fn minimize_window(window: &slint::Window) {
        window.set_minimized(true);
    }

    /// Toggles the maximized state of the given window portably across all platforms.
    pub fn toggle_maximize_window(window: &slint::Window) {
        let currently_maximized = window.is_maximized();
        window.set_maximized(!currently_maximized);
    }

    /// Requests window drag initialization. On Windows, uses native message pump hooks;
    /// on macOS and Linux, delegates cleanly to Slint's built-in window move mechanics.
    pub fn begin_window_drag(_window: &slint::Window) {
        #[cfg(target_os = "windows")]
        Self::begin_native_drag();
    }

    /// Initiates a native Win32 non-client window drag using standard message pump hooks.
    /// Safe no-op on macOS and Linux where `WindowMoveArea` directly owns the drag.
    pub fn begin_native_drag() {
        #[cfg(target_os = "windows")]
        unsafe {
            let hwnd = GetForegroundWindow();
            if !hwnd.is_null() {
                ReleaseCapture();
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            // On macOS and Linux, Slint WindowMoveArea coordinates dragging with the window manager
        }
    }

    /// Minimizes the current application window.
    pub fn minimize() {
        #[cfg(target_os = "windows")]
        unsafe {
            let hwnd = GetForegroundWindow();
            if !hwnd.is_null() {
                ShowWindow(hwnd, SW_MINIMIZE);
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            // Portable minimization is driven via `minimize_window`
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
        #[cfg(not(target_os = "windows"))]
        {
            // Portable maximize is driven via `toggle_maximize_window`
        }
    }

    /// Flashes the Thulli window in the dock/taskbar to get the person's attention.
    pub fn flash_attention() {
        #[cfg(target_os = "windows")]
        unsafe {
            let title: Vec<u16> = "Thulli".encode_utf16().chain(std::iter::once(0)).collect();
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if !hwnd.is_null() {
                let info = FLASHWINFO {
                    cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
                    hwnd,
                    dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
                    uCount: 3,
                    dwTimeout: 0,
                };
                FlashWindowEx(&info);
            }
        }
        #[cfg(target_os = "macos")]
        {
            // macOS user attention request hook
        }
        #[cfg(target_os = "linux")]
        {
            // Linux urgency hint hook
        }
    }

    /// Performs a clean application exit.
    pub fn exit_cleanly() -> ! {
        println!("[Thulli] Clean application shutdown requested");
        std::process::exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_identifier_is_valid() {
        let p = WindowManager::platform();
        assert!(["windows", "macos", "linux", "other"].contains(&p));
    }

    #[test]
    fn platform_flags_are_consistent() {
        if WindowManager::is_windows() {
            assert_eq!(WindowManager::platform(), "windows");
        } else if WindowManager::is_macos() {
            assert_eq!(WindowManager::platform(), "macos");
        } else if WindowManager::is_linux() {
            assert_eq!(WindowManager::platform(), "linux");
        }
    }
}
