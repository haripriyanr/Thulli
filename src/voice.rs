//! Voice input through Windows' own voice typing. Win+H opens the system
//! dictation panel, which types into the focused text field using the
//! system microphone and the user's installed speech languages. Pressing it
//! again closes the panel, so the mic button works as a toggle.

/// Opens or closes Windows voice typing for the focused field.
#[cfg(target_os = "windows")]
pub fn toggle_voice_typing() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_LWIN,
    };
    const VK_H: u16 = 0x48;
    let key = |vk: u16, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 },
        },
    };
    let inputs = [key(VK_LWIN, false), key(VK_H, false), key(VK_H, true), key(VK_LWIN, true)];
    unsafe {
        SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(not(target_os = "windows"))]
pub fn toggle_voice_typing() {}
