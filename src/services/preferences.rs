//! Reading and changing user preferences (product spec section 11).
//!
//! The service validates every change, keeps the stored values in range and
//! saves through the shared [`StoreHandle`]. Settings are addressed by the
//! same names the preferences screen uses (kebab-case or snake_case both work).
//!
//! Also here, because they are about the person's setup rather than screens:
//! the colour palettes behind light, dark and system mode with five accent
//! themes, the sound and timezone catalogues, the open-at-login switch (behind
//! the [`LoginItem`] trait so tests never touch the registry), the first-run
//! checklist, and tag management.

use std::fmt;

use crate::models::{
    AccentTheme, AppStateStore, ExpandedWindowMode, FocusDisplayMode, PanelSide, Preferences, Tag, ThemeChoice,
    WeekStart,
};
use crate::services::store_handle::StoreHandle;

/// Why a change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefError {
    /// The setting name is not one this service manages.
    UnknownSetting(String),
    /// The value is not allowed for that setting.
    InvalidValue { setting: String, value: String },
    /// The system refused the open-at-login change.
    Login(String),
    /// No tag has that id.
    NoSuchTag,
    /// A tag needs a name.
    EmptyTagName,
}

impl fmt::Display for PrefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrefError::UnknownSetting(s) => write!(f, "Unknown setting: {s}"),
            PrefError::InvalidValue { setting, value } => write!(f, "'{value}' is not valid for {setting}"),
            PrefError::Login(msg) => write!(f, "Could not change the start-up setting: {msg}"),
            PrefError::NoSuchTag => write!(f, "That tag no longer exists"),
            PrefError::EmptyTagName => write!(f, "Give the tag a name"),
        }
    }
}

impl std::error::Error for PrefError {}

fn norm(key: &str) -> String {
    key.trim().replace('-', "_")
}

fn invalid(setting: &str, value: &str) -> PrefError {
    PrefError::InvalidValue { setting: setting.to_string(), value: value.to_string() }
}

// ---------------------------------------------------------------------------
// Catalogues
// ---------------------------------------------------------------------------

/// Sounds Thulli can play. They are synthesised in code (see `alerts`).
pub const SOUND_CHOICES: &[(&str, &str)] = &[
    ("basic-ding", "Basic ding"),
    ("soft-chime", "Soft chime"),
    ("bright-bell", "Bright bell"),
    ("gentle-pulse", "Gentle pulse"),
    ("low-gong", "Low gong"),
    ("rising-tone", "Rising tone"),
    ("double-tap", "Double tap"),
    ("wood-tick", "Wood tick"),
];

/// Whether `id` names a known sound.
pub fn is_sound_id(id: &str) -> bool {
    SOUND_CHOICES.iter().any(|(s, _)| *s == id)
}

/// Interface languages offered. The choice is stored only; the interface is
/// currently English.
pub const LANGUAGE_CHOICES: &[(&str, &str)] = &[
    ("en", "English"),
    ("es", "Espanol"),
    ("fr", "Francais"),
    ("de", "Deutsch"),
    ("pt", "Portugues"),
    ("it", "Italiano"),
    ("hi", "Hindi"),
    ("ja", "Japanese"),
];

/// Timezone labels offered. An empty stored value means "system".
pub const TIMEZONE_CHOICES: &[&str] = &[
    "(GMT-08:00) America/Los_Angeles",
    "(GMT-07:00) America/Denver",
    "(GMT-06:00) America/Chicago",
    "(GMT-05:00) America/New_York",
    "(GMT-03:00) America/Sao_Paulo",
    "(GMT+00:00) Europe/London",
    "(GMT+01:00) Europe/Paris",
    "(GMT+01:00) Europe/Berlin",
    "(GMT+02:00) Africa/Cairo",
    "(GMT+03:00) Europe/Moscow",
    "(GMT+04:00) Asia/Dubai",
    "(GMT+05:30) Asia/Calcutta",
    "(GMT+07:00) Asia/Bangkok",
    "(GMT+08:00) Asia/Singapore",
    "(GMT+09:00) Asia/Tokyo",
    "(GMT+10:00) Australia/Sydney",
    "(GMT+12:00) Pacific/Auckland",
];

/// Label of the "use the computer's timezone" entry.
pub const SYSTEM_TIMEZONE_LABEL: &str = "System default";

/// Background presets for the focus view.
pub const FOCUS_BACKGROUNDS: &[(&str, &str)] =
    &[("calm", "Calm"), ("dusk", "Dusk"), ("forest", "Forest"), ("plain", "Plain")];

/// Celebration artwork sets drawn by Thulli.
pub const CELEBRATION_SETS: &[(&str, &str)] = &[
    ("sunrise", "Sunrise"),
    ("confetti", "Confetti"),
    ("steady", "Steady"),
    ("ripple", "Ripple"),
];

// ---------------------------------------------------------------------------
// Palettes
// ---------------------------------------------------------------------------

/// Concrete colours (0xRRGGBB) for the theme global.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette {
    pub dark: bool,
    pub bg_canvas: u32,
    pub bg_sidebar: u32,
    pub bg_panel: u32,
    pub bg_card: u32,
    pub bg_card_hover: u32,
    pub bg_card_selected: u32,
    pub border_subtle: u32,
    pub border_light: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_muted: u32,
    /// Primary accent (start of the accent gradient).
    pub accent_cyan: u32,
    /// Secondary accent (end of the accent gradient).
    pub accent_lime: u32,
}

impl ThemePalette {
    const DARK: ThemePalette = ThemePalette {
        dark: true,
        bg_canvas: 0x0D0F12,
        bg_sidebar: 0x14171D,
        bg_panel: 0x14171D,
        bg_card: 0x181C24,
        bg_card_hover: 0x222733,
        bg_card_selected: 0x252D3D,
        border_subtle: 0x262D3D,
        border_light: 0x343E54,
        text_primary: 0xFFFFFF,
        text_secondary: 0x9EABBE,
        text_muted: 0x5E697C,
        accent_cyan: 0x36C9C5,
        accent_lime: 0xB3C065,
    };

    const LIGHT: ThemePalette = ThemePalette {
        dark: false,
        bg_canvas: 0xF7F5F0,
        bg_sidebar: 0xEFEEE8,
        bg_panel: 0xFBFAF6,
        bg_card: 0xFFFFFF,
        bg_card_hover: 0xEAE9E2,
        bg_card_selected: 0xDCEBE7,
        border_subtle: 0xDDDCD4,
        border_light: 0xC8C9C0,
        text_primary: 0x242B2A,
        text_secondary: 0x586765,
        text_muted: 0x7D8985,
        accent_cyan: 0x36C9C5,
        accent_lime: 0xB3C065,
    };

    /// The gradient pair for an accent theme, as (primary, secondary).
    pub fn accent_pair(accent: AccentTheme) -> (u32, u32) {
        match accent {
            AccentTheme::Teal => (0x36C9C5, 0xB3C065),
            AccentTheme::Blue => (0x6F98E8, 0x7FD3F0),
            AccentTheme::Violet => (0x9B7BEF, 0xEF82EF),
            AccentTheme::Amber => (0xDB9D20, 0xF0D26A),
            AccentTheme::Rose => (0xEF6F8F, 0xF7A8B8),
        }
    }

    /// Picks the palette for a mode and accent. `system_dark` is only used
    /// when the mode is `System`.
    pub fn resolve(mode: ThemeChoice, accent: AccentTheme, system_dark: bool) -> ThemePalette {
        let dark = match mode {
            ThemeChoice::Dark => true,
            ThemeChoice::Light => false,
            ThemeChoice::System => system_dark,
        };
        let mut palette = if dark { Self::DARK } else { Self::LIGHT };
        let (a, b) = Self::accent_pair(accent);
        palette.accent_cyan = a;
        palette.accent_lime = b;
        palette
    }
}

/// Whether Windows is set to dark app mode. Falls back to dark when unknown.
pub fn system_prefers_dark() -> bool {
    #[cfg(windows)]
    {
        match win::read_hkcu_dword(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
            "AppsUseLightTheme",
        ) {
            Some(v) => v == 0,
            None => true,
        }
    }
    #[cfg(not(windows))]
    {
        true
    }
}

// ---------------------------------------------------------------------------
// Open at login
// ---------------------------------------------------------------------------

/// The system's "start with the computer" switch.
pub trait LoginItem {
    fn is_enabled(&self) -> bool;
    fn set_enabled(&self, enabled: bool) -> Result<(), String>;
}

/// Name of the per-user Run value.
pub const LOGIN_VALUE_NAME: &str = "Thulli";
/// Registry path (under HKEY_CURRENT_USER) of the Run list.
pub const RUN_KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

/// Real implementation: the `HKCU\...\Run` value only (never machine-wide).
pub struct RegistryLoginItem {
    exe_path: String,
}

impl RegistryLoginItem {
    /// Uses the path of the running program.
    pub fn for_current_exe() -> Self {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        Self { exe_path: exe }
    }

    /// The value written to the Run key: the quoted program path.
    pub fn command_line(&self) -> String {
        format!("\"{}\"", self.exe_path)
    }
}

impl LoginItem for RegistryLoginItem {
    fn is_enabled(&self) -> bool {
        #[cfg(windows)]
        {
            win::run_value_exists(RUN_KEY_PATH, LOGIN_VALUE_NAME)
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        #[cfg(windows)]
        {
            if enabled {
                win::set_run_value(RUN_KEY_PATH, LOGIN_VALUE_NAME, &self.command_line())
            } else {
                win::delete_run_value(RUN_KEY_PATH, LOGIN_VALUE_NAME)
            }
        }
        #[cfg(not(windows))]
        {
            let _ = enabled;
            Err("Open at login is only available on Windows".to_string())
        }
    }
}

#[cfg(windows)]
mod win {
    //! Minimal registry access for the current user.
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn open(path: &str, access: u32) -> Option<HKEY> {
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `path` is NUL-terminated and `key` is a valid out pointer.
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide(path).as_ptr(), 0, access, &mut key) };
        (rc == ERROR_SUCCESS).then_some(key)
    }

    pub fn read_hkcu_dword(path: &str, name: &str) -> Option<u32> {
        let key = open(path, KEY_QUERY_VALUE)?;
        let mut data: u32 = 0;
        let mut size: u32 = 4;
        // SAFETY: the out buffer is a u32 and `size` says so.
        let rc = unsafe {
            RegQueryValueExW(
                key,
                wide(name).as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                (&mut data as *mut u32).cast::<u8>(),
                &mut size,
            )
        };
        unsafe { RegCloseKey(key) };
        (rc == ERROR_SUCCESS).then_some(data)
    }

    pub fn run_value_exists(path: &str, name: &str) -> bool {
        let Some(key) = open(path, KEY_QUERY_VALUE) else { return false };
        // SAFETY: a null data pointer only asks whether the value exists.
        let rc = unsafe {
            RegQueryValueExW(
                key,
                wide(name).as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        unsafe { RegCloseKey(key) };
        rc == ERROR_SUCCESS
    }

    pub fn set_run_value(path: &str, name: &str, value: &str) -> Result<(), String> {
        let key = open(path, KEY_SET_VALUE).ok_or_else(|| "could not open the start-up list".to_string())?;
        let data = wide(value);
        // SAFETY: `data` outlives the call and the byte length matches it.
        let rc = unsafe {
            RegSetValueExW(key, wide(name).as_ptr(), 0, REG_SZ, data.as_ptr().cast::<u8>(), (data.len() * 2) as u32)
        };
        unsafe { RegCloseKey(key) };
        if rc == ERROR_SUCCESS { Ok(()) } else { Err(format!("error code {rc}")) }
    }

    pub fn delete_run_value(path: &str, name: &str) -> Result<(), String> {
        let Some(key) = open(path, KEY_SET_VALUE) else { return Ok(()) };
        // SAFETY: `name` is NUL-terminated.
        let rc = unsafe { RegDeleteValueW(key, wide(name).as_ptr()) };
        unsafe { RegCloseKey(key) };
        // A missing value is already the state we want.
        if rc == ERROR_SUCCESS || rc == 2 { Ok(()) } else { Err(format!("error code {rc}")) }
    }
}

// ---------------------------------------------------------------------------
// First-run checklist
// ---------------------------------------------------------------------------

/// One step of the first-run checklist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecklistItem {
    pub id: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub done: bool,
}

/// Short coaching tips shown under the checklist, one at a time.
pub const COACH_TIPS: &[&str] = &[
    "Type 'Write report 45m' and the estimate is picked up for you.",
    "Start with one small task. Momentum does the rest.",
    "Short breaks keep long days sustainable.",
    "Plan your day first thing, then protect the plan.",
    "Turn on the shield when you need quiet.",
];

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Reading and changing user preferences.
pub struct PreferencesService {
    handle: StoreHandle,
}

impl PreferencesService {
    pub fn new(handle: StoreHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }

    /// A copy of every setting.
    pub fn snapshot(&self) -> Preferences {
        self.handle.read(|s| s.preferences.clone())
    }

    /// Switches a yes/no setting.
    pub fn set_bool(&self, key: &str, value: bool) -> Result<(), PrefError> {
        let key = norm(&key);
        self.handle.mutate(|s| {
            let p = &mut s.preferences;
            match key.as_str() {
                "hide_estimates" => p.hide_estimates = value,
                "pomodoro_auto_advance" => p.pomodoro_auto_advance = value,
                "pomodoro_breaks" => p.pomodoro_breaks = value,
                "auto_parse_estimates" => p.auto_parse_estimates = value,
                "open_at_login" => p.open_at_login = value,
                "open_on_wake" => p.open_on_wake = value,
                "show_full_title" => p.show_full_title = value,
                "count_weekends" => p.count_weekends = value,
                "sprint_enabled" => p.sprint_enabled = value,
                "pause_suggestions" => p.pause_suggestions = value,
                "scrolling_title" => p.scrolling_title = value,
                "start_next_after_early_break" => p.start_next_after_early_break = value,
                "move_freely" => p.move_freely = value,
                "auto_hide" => p.auto_hide = value,
                "timed_alerts" => p.timed_alerts = value,
                "alert_flash" => p.alert_flash = value,
                "overtime_alert" => p.overtime_alert = value,
                "pill_alert_animated" => p.pill_alert_animated = value,
                "success_screen" => p.success_screen = value,
                "success_animation" => p.success_animation = value,
                "success_sound" => p.success_sound = value,
                "success_animation_on_break_end" => p.success_animation_on_break_end = value,
                "suggest_plan_my_day" => p.suggestions.plan_my_day = value,
                "suggest_welcome_back" => p.suggestions.welcome_back = value,
                "suggest_take_a_break" => p.suggestions.take_a_break = value,
                "suggest_wind_down" => p.suggestions.wind_down = value,
                "suggest_prioritize" => p.suggestions.prioritize = value,
                "first_run_done" => p.first_run_done = value,
                _ => return Err(PrefError::UnknownSetting(key.clone())),
            }
            Ok(())
        })
    }

    /// Changes a number. Values are clamped to the range the setting allows.
    pub fn set_int(&self, key: &str, value: i32) -> Result<(), PrefError> {
        let key = norm(key);
        let v = value.max(0) as u32;
        self.handle.mutate(|s| {
            let p = &mut s.preferences;
            match key.as_str() {
                "daily_goal_mins" => p.daily_goal_mins = v.clamp(15, 960),
                "default_session_mins" => p.default_session_mins = Preferences::clamp_session_mins(v),
                "work_sprint_mins" => p.work_sprint_mins = v.clamp(5, 120),
                "break_time_mins" => p.break_time_mins = v.clamp(1, 60),
                "default_break_mins" => p.default_break_mins = v.clamp(1, 60),
                "alert_interval_mins" => p.alert_interval_mins = v.clamp(1, 180),
                "reminder_mins_before" => p.reminder_mins_before = v.clamp(0, 120),
                "break_reminder_mins" => p.break_reminder_mins = v.min(240),
                "zoom_percent" => p.zoom_percent = v.clamp(50, 200),
                "pomodoro_long_break_mins" => p.pomodoro_long_break_mins = v.clamp(1, 60),
                "pomodoro_rounds" => p.pomodoro_rounds = v.clamp(1, 8),
                "pomodoro_custom_mins" => p.pomodoro_custom_mins = v.clamp(5, 180),
                _ => return Err(PrefError::UnknownSetting(key.clone())),
            }
            Ok(())
        })
    }

    /// Saves the ambient sound choice. Unknown choices are ignored.
    pub fn set_pomodoro_ambient(&self, id: &str) {
        if matches!(id, "off" | "forest" | "lofi" | "rain") {
            let id = id.to_string();
            self.handle.mutate(|s| s.preferences.pomodoro_ambient = id);
        }
    }

    /// Changes a choice or text setting. Unknown choices are refused.
    pub fn set_text(&self, key: &str, value: &str) -> Result<(), PrefError> {
        let key = norm(key);
        let value = value.trim();
        self.handle.mutate(|s| {
            let p = &mut s.preferences;
            let bad = || invalid(&key, value);
            match key.as_str() {
                "theme" => {
                    p.theme = match value {
                        "system" => ThemeChoice::System,
                        "dark" => ThemeChoice::Dark,
                        "light" => ThemeChoice::Light,
                        _ => return Err(bad()),
                    }
                }
                "accent_theme" => {
                    p.accent_theme = match value {
                        "teal" => AccentTheme::Teal,
                        "blue" => AccentTheme::Blue,
                        "violet" => AccentTheme::Violet,
                        "amber" => AccentTheme::Amber,
                        "rose" => AccentTheme::Rose,
                        _ => return Err(bad()),
                    }
                }
                "week_start" => {
                    p.week_start = match value {
                        "monday" => WeekStart::Monday,
                        "sunday" => WeekStart::Sunday,
                        "saturday" => WeekStart::Saturday,
                        _ => return Err(bad()),
                    }
                }
                "language" => {
                    if !LANGUAGE_CHOICES.iter().any(|(id, _)| *id == value) {
                        return Err(bad());
                    }
                    p.language = value.to_string();
                }
                "timezone" => {
                    p.timezone = if value == SYSTEM_TIMEZONE_LABEL { String::new() } else { value.to_string() };
                }
                "focus_display_mode" => {
                    p.focus_display_mode = match value {
                        "expanded" => FocusDisplayMode::Expanded,
                        "tower" => FocusDisplayMode::Tower,
                        "pill" => FocusDisplayMode::Pill,
                        _ => return Err(bad()),
                    }
                }
                "tower_side" | "compact_panel_side" => {
                    let side = match value {
                        "left" => PanelSide::Left,
                        "right" => PanelSide::Right,
                        _ => return Err(bad()),
                    };
                    if key == "tower_side" {
                        p.tower_side = side;
                    } else {
                        p.compact_panel_side = side;
                    }
                }
                "expanded_window_mode" => {
                    p.expanded_window_mode = match value {
                        "full_screen" => ExpandedWindowMode::FullScreen,
                        "maximized" => ExpandedWindowMode::Maximized,
                        _ => return Err(bad()),
                    }
                }
                "focus_background" => {
                    if value.is_empty() {
                        return Err(bad());
                    }
                    p.focus_background = value.to_string();
                }
                "alert_sound" | "notification_sound" | "break_start_sound" | "task_start_sound"
                | "overtime_sound" | "reminder_sound" | "break_reminder_sound" | "success_sound_id" => {
                    if !is_sound_id(value) {
                        return Err(bad());
                    }
                    let id = value.to_string();
                    match key.as_str() {
                        "alert_sound" => p.alert_sound_id = id,
                        "notification_sound" => p.notification_sound_id = id,
                        "break_start_sound" => p.break_start_sound_id = id,
                        "task_start_sound" => p.task_start_sound_id = id,
                        "overtime_sound" => p.overtime_sound_id = id,
                        "reminder_sound" => p.reminder_sound_id = id,
                        "break_reminder_sound" => p.break_reminder_sound_id = id,
                        _ => p.success_sound_id = id,
                    }
                }
                "success_set_on_time" | "success_set_over_estimate" => {
                    if !CELEBRATION_SETS.iter().any(|(id, _)| *id == value) {
                        return Err(bad());
                    }
                    if key == "success_set_on_time" {
                        p.success_set_on_time = value.to_string();
                    } else {
                        p.success_set_over_estimate = value.to_string();
                    }
                }
                "assistant_model_path" => p.assistant_model_path = value.to_string(),
                "ai_model" => {
                    // Ids look like "vendor/model"; anything else is refused.
                    if value.is_empty() || value.len() > 120 || value.chars().any(char::is_whitespace) {
                        return Err(bad());
                    }
                    p.ai_model = value.to_string();
                }
                _ => return Err(PrefError::UnknownSetting(key.clone())),
            }
            Ok(())
        })
    }

    /// Turns open-at-login on or off. The saved setting only changes if the
    /// system accepted the change.
    pub fn set_open_at_login(&self, login: &dyn LoginItem, enabled: bool) -> Result<(), PrefError> {
        login.set_enabled(enabled).map_err(PrefError::Login)?;
        self.set_bool("open_at_login", enabled)
    }

    /// Makes the system match the saved setting (call at launch). The system
    /// value wins if the person changed it outside the app.
    pub fn sync_open_at_login(&self, login: &dyn LoginItem) {
        let actual = login.is_enabled();
        let saved = self.handle.read(|s| s.preferences.open_at_login);
        if actual != saved {
            let _ = self.set_bool("open_at_login", actual);
        }
    }

    /// The palette for the saved mode and accent.
    pub fn palette(&self, system_dark: bool) -> ThemePalette {
        self.handle.read(|s| ThemePalette::resolve(s.preferences.theme, s.preferences.accent_theme, system_dark))
    }

    /// Timezone choices as display labels, system entry first.
    pub fn timezone_labels() -> Vec<String> {
        std::iter::once(SYSTEM_TIMEZONE_LABEL.to_string())
            .chain(TIMEZONE_CHOICES.iter().map(|s| s.to_string()))
            .collect()
    }

    /// "30h" style label of the weekly goal, or "6h 30m".
    pub fn weekly_goal_label(&self) -> String {
        let mins = self.handle.read(|s| s.preferences.weekly_goal_mins());
        format_hours_minutes(mins)
    }

    /// Resets every setting to its default, except what the person made
    /// elsewhere (quick notes, shield state, first-run flag).
    pub fn reset_to_defaults(&self) {
        self.handle.mutate(|s| {
            let keep = (
                s.preferences.quick_notes.clone(),
                s.preferences.app_shield_active,
                s.preferences.web_shield_active,
                s.preferences.first_run_done,
                s.preferences.active_mode.clone(),
            );
            s.preferences = Preferences::default();
            s.preferences.quick_notes = keep.0;
            s.preferences.app_shield_active = keep.1;
            s.preferences.web_shield_active = keep.2;
            s.preferences.first_run_done = keep.3;
            s.preferences.active_mode = keep.4;
        });
    }

    // ----- first-run checklist -----

    /// The checklist with each step's progress read from the person's data.
    pub fn checklist(&self) -> Vec<ChecklistItem> {
        self.handle.read(|s| {
            vec![
                ChecklistItem {
                    id: "task",
                    label: "Add your first task",
                    hint: "Open Tasks and type a title.",
                    done: !s.tasks.is_empty(),
                },
                ChecklistItem {
                    id: "focus",
                    label: "Run a focus session",
                    hint: "Pick a task and press play.",
                    done: !s.focus_sessions.is_empty(),
                },
                ChecklistItem {
                    id: "note",
                    label: "Jot down a note",
                    hint: "Notes keep ideas out of your head.",
                    done: !s.notes.is_empty(),
                },
                ChecklistItem {
                    id: "shield",
                    label: "Choose what to quiet",
                    hint: "Add an app or site to the shield.",
                    done: !s.blocked_apps.is_empty() || !s.blocked_sites.is_empty(),
                },
                ChecklistItem {
                    id: "look",
                    label: "Make it yours",
                    hint: "Pick a colour mode and accent.",
                    done: s.preferences.theme != ThemeChoice::System || s.preferences.accent_theme != AccentTheme::Teal,
                },
            ]
        })
    }

    /// True while the checklist should be shown.
    pub fn checklist_visible(&self) -> bool {
        let done = self.handle.read(|s| s.preferences.first_run_done);
        !done && !self.checklist().iter().all(|i| i.done)
    }

    /// Hides the checklist for good.
    pub fn dismiss_checklist(&self) {
        let _ = self.set_bool("first_run_done", true);
    }

    /// The tip for a given index (wraps around).
    pub fn coach_tip(index: usize) -> &'static str {
        COACH_TIPS[index % COACH_TIPS.len()]
    }

    // ----- tags -----

    /// Creates a tag; the name must be unique ignoring case.
    pub fn add_tag(&self, name: &str, emoji: &str, color: &str) -> Result<String, PrefError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(PrefError::EmptyTagName);
        }
        self.handle.mutate(|s| {
            if let Some(existing) = s.tags.iter().find(|t| t.name.eq_ignore_ascii_case(name)) {
                return Ok(existing.id.clone());
            }
            let mut tag = Tag::new(name, normalise_color(color));
            if !emoji.trim().is_empty() {
                tag = tag.with_emoji(emoji.trim());
            }
            let id = tag.id.clone();
            s.tags.push(tag);
            Ok(id)
        })
    }

    /// Renames or recolours a tag.
    pub fn edit_tag(&self, id: &str, name: &str, emoji: &str, color: &str) -> Result<(), PrefError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(PrefError::EmptyTagName);
        }
        self.handle.mutate(|s| {
            let tag = s.tags.iter_mut().find(|t| t.id == id).ok_or(PrefError::NoSuchTag)?;
            tag.name = name.to_string();
            tag.emoji = (!emoji.trim().is_empty()).then(|| emoji.trim().to_string());
            tag.color = normalise_color(color);
            Ok(())
        })
    }

    /// Removes a tag and takes it off every task.
    pub fn remove_tag(&self, id: &str) -> Result<(), PrefError> {
        self.handle.mutate(|s| remove_tag_from(s, id))
    }
}

fn remove_tag_from(s: &mut AppStateStore, id: &str) -> Result<(), PrefError> {
    let before = s.tags.len();
    s.tags.retain(|t| t.id != id);
    if s.tags.len() == before {
        return Err(PrefError::NoSuchTag);
    }
    for task in &mut s.tasks {
        task.tags.retain(|t| t != id);
    }
    Ok(())
}

/// Accepts `#RRGGBB` or `RRGGBB`; anything else becomes a neutral blue.
fn normalise_color(color: &str) -> String {
    let c = color.trim().trim_start_matches('#');
    if c.len() == 6 && c.chars().all(|ch| ch.is_ascii_hexdigit()) {
        format!("#{}", c.to_uppercase())
    } else {
        "#6F98E8".to_string()
    }
}

/// "6h", "6h 30m" or "45m".
pub fn format_hours_minutes(mins: u32) -> String {
    match (mins / 60, mins % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Task, TaskSection};
    use crate::repository::{LoadReport, StorageRepository};
    use std::cell::Cell;
    use std::sync::{Arc, Mutex};

    struct MemoryRepo;
    impl StorageRepository for MemoryRepo {
        fn load_with_report(&self) -> LoadReport {
            LoadReport { store: AppStateStore::default(), recovery: None }
        }
        fn save(&self, _store: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
    }

    fn service() -> PreferencesService {
        let handle = StoreHandle::new(Arc::new(Mutex::new(AppStateStore::with_space())), Arc::new(MemoryRepo));
        PreferencesService::new(handle)
    }

    struct MockLogin {
        on: Cell<bool>,
        fail: bool,
    }
    impl LoginItem for MockLogin {
        fn is_enabled(&self) -> bool {
            self.on.get()
        }
        fn set_enabled(&self, enabled: bool) -> Result<(), String> {
            if self.fail {
                return Err("denied".into());
            }
            self.on.set(enabled);
            Ok(())
        }
    }

    #[test]
    fn bool_and_kebab_keys() {
        let svc = service();
        svc.set_bool("hide-estimates", true).unwrap();
        svc.set_bool("suggest-wind-down", false).unwrap();
        let p = svc.snapshot();
        assert!(p.hide_estimates);
        assert!(!p.suggestions.wind_down);
        assert!(matches!(svc.set_bool("nope", true), Err(PrefError::UnknownSetting(_))));
    }

    #[test]
    fn ints_are_clamped() {
        let svc = service();
        svc.set_int("daily-goal-mins", 5).unwrap();
        assert_eq!(svc.snapshot().daily_goal_mins, 15);
        svc.set_int("default-session-mins", 97).unwrap();
        assert_eq!(svc.snapshot().default_session_mins, 95);
        svc.set_int("break-reminder-mins", -4).unwrap();
        assert_eq!(svc.snapshot().break_reminder_mins, 0);
        svc.set_int("alert-interval-mins", 9999).unwrap();
        assert_eq!(svc.snapshot().alert_interval_mins, 180);
    }

    #[test]
    fn text_choices_are_validated() {
        let svc = service();
        svc.set_text("theme", "light").unwrap();
        svc.set_text("accent-theme", "rose").unwrap();
        assert_eq!(svc.snapshot().theme, ThemeChoice::Light);
        assert_eq!(svc.snapshot().accent_theme, AccentTheme::Rose);
        assert!(svc.set_text("theme", "neon").is_err());
        assert!(svc.set_text("alert-sound", "kazoo").is_err());
        svc.set_text("alert-sound", "low-gong").unwrap();
        assert_eq!(svc.snapshot().alert_sound_id, "low-gong");
        svc.set_text("success-sound-id", "wood-tick").unwrap();
        assert_eq!(svc.snapshot().success_sound_id, "wood-tick");
    }

    #[test]
    fn language_is_stored_only() {
        let svc = service();
        svc.set_text("language", "fr").unwrap();
        assert_eq!(svc.snapshot().language, "fr");
        assert!(svc.set_text("language", "xx").is_err());
    }

    #[test]
    fn timezone_system_entry_clears_value() {
        let svc = service();
        svc.set_text("timezone", "(GMT+09:00) Asia/Tokyo").unwrap();
        assert_eq!(svc.snapshot().timezone, "(GMT+09:00) Asia/Tokyo");
        svc.set_text("timezone", SYSTEM_TIMEZONE_LABEL).unwrap();
        assert!(svc.snapshot().timezone.is_empty());
        assert_eq!(PreferencesService::timezone_labels()[0], SYSTEM_TIMEZONE_LABEL);
    }

    #[test]
    fn palettes_follow_mode_and_accent() {
        let dark = ThemePalette::resolve(ThemeChoice::Dark, AccentTheme::Teal, false);
        assert!(dark.dark);
        assert_eq!(dark.bg_canvas, 0x0D0F12);
        let light = ThemePalette::resolve(ThemeChoice::Light, AccentTheme::Teal, true);
        assert!(!light.dark);
        assert_ne!(light.bg_canvas, dark.bg_canvas);
        let sys_dark = ThemePalette::resolve(ThemeChoice::System, AccentTheme::Blue, true);
        let sys_light = ThemePalette::resolve(ThemeChoice::System, AccentTheme::Blue, false);
        assert!(sys_dark.dark && !sys_light.dark);
        assert_eq!(sys_dark.accent_cyan, ThemePalette::accent_pair(AccentTheme::Blue).0);
        let accents = [AccentTheme::Teal, AccentTheme::Blue, AccentTheme::Violet, AccentTheme::Amber, AccentTheme::Rose];
        let mut seen: Vec<u32> = accents.iter().map(|a| ThemePalette::accent_pair(*a).0).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 5);
    }

    #[test]
    fn open_at_login_uses_trait_and_only_saves_on_success() {
        let svc = service();
        let ok = MockLogin { on: Cell::new(false), fail: false };
        svc.set_open_at_login(&ok, true).unwrap();
        assert!(ok.on.get());
        assert!(svc.snapshot().open_at_login);

        let bad = MockLogin { on: Cell::new(false), fail: true };
        assert!(matches!(svc.set_open_at_login(&bad, false), Err(PrefError::Login(_))));
        assert!(svc.snapshot().open_at_login, "setting unchanged after failure");
    }

    #[test]
    fn sync_open_at_login_follows_system() {
        let svc = service();
        let login = MockLogin { on: Cell::new(true), fail: false };
        svc.sync_open_at_login(&login);
        assert!(svc.snapshot().open_at_login);
    }

    #[test]
    fn run_value_is_quoted_path() {
        let item = RegistryLoginItem { exe_path: "C:\\Apps\\Thulli\\thulli.exe".into() };
        assert_eq!(item.command_line(), "\"C:\\Apps\\Thulli\\thulli.exe\"");
    }

    #[test]
    fn checklist_progress_and_dismiss() {
        let svc = service();
        assert!(svc.checklist_visible());
        assert!(svc.checklist().iter().all(|i| !i.done));
        svc.handle().mutate(|s| {
            let list_id = s.lists[0].id.clone();
            s.add_task(Task::new(list_id, "Write", 30, TaskSection::Today));
        });
        svc.set_text("accent-theme", "violet").unwrap();
        let done: Vec<_> = svc.checklist().into_iter().filter(|i| i.done).map(|i| i.id).collect();
        assert_eq!(done, vec!["task", "look"]);
        svc.dismiss_checklist();
        assert!(!svc.checklist_visible());
    }

    #[test]
    fn tags_add_edit_remove() {
        let svc = service();
        let id = svc.add_tag("Deep work", "", "3cc9c4").unwrap();
        assert_eq!(svc.add_tag("deep WORK", "", "#000000").unwrap(), id, "no duplicates");
        svc.handle().mutate(|s| {
            let list_id = s.lists[0].id.clone();
            let mut t = Task::new(list_id, "Write", 30, TaskSection::Today);
            t.tags.push(id.clone());
            s.add_task(t);
        });
        svc.edit_tag(&id, "Focus", "", "#abc").unwrap();
        svc.handle().read(|s| {
            assert_eq!(s.tags[0].name, "Focus");
            assert_eq!(s.tags[0].color, "#6F98E8");
        });
        assert_eq!(svc.add_tag("  ", "", ""), Err(PrefError::EmptyTagName));
        svc.remove_tag(&id).unwrap();
        svc.handle().read(|s| {
            assert!(s.tags.is_empty());
            assert!(s.tasks[0].tags.is_empty());
        });
        assert_eq!(svc.remove_tag(&id), Err(PrefError::NoSuchTag));
    }

    #[test]
    fn hours_label_and_reset() {
        assert_eq!(format_hours_minutes(390), "6h 30m");
        assert_eq!(format_hours_minutes(120), "2h");
        assert_eq!(format_hours_minutes(45), "45m");
        let svc = service();
        svc.set_bool("hide-estimates", true).unwrap();
        svc.set_bool("first-run-done", true).unwrap();
        svc.reset_to_defaults();
        assert!(!svc.snapshot().hide_estimates);
        assert!(svc.snapshot().first_run_done);
        assert_eq!(svc.weekly_goal_label(), "30h");
    }
}
