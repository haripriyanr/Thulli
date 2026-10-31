//! Native window management services for frameless Slint desktop windows.

#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, FlashWindowEx, GetForegroundWindow, SendMessageW, WM_NCLBUTTONDOWN, HTCAPTION,
    ShowWindow, IsZoomed, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG, SW_MINIMIZE, SW_MAXIMIZE, SW_RESTORE,
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

    /// Flashes the Thulli window in the taskbar to get the person's attention.
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
    }

    /// Performs a clean application exit.
    pub fn exit_cleanly() -> ! {
        println!("[Thulli] Clean application shutdown requested");
        std::process::exit(0);
    }
}
