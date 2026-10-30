//! User preferences (product spec section 11).

use chrono::Local;
use serde::{Deserialize, Serialize};

/// The timezone string written by early builds; cleared on migration so the
/// system timezone is used instead.
const LEGACY_DEFAULT_TIMEZONE: &str = "(GMT+05:30) Asia/Calcutta";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PanelSide {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WeekStart {
    #[default]
    Monday,
    Sunday,
    Saturday,
}

/// A keyboard shortcut with an on/off switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutBinding {
    pub keys: String,
    pub enabled: bool,
}

impl ShortcutBinding {
    pub fn new(keys: impl Into<String>) -> Self {
        Self { keys: keys.into(), enabled: true }
    }
}

/// System-wide shortcuts that work while Thulli is in the background.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalShortcuts {
    pub show_app: ShortcutBinding,
    pub toggle_focus_mode: ShortcutBinding,
    pub find_focus_timer: ShortcutBinding,
    pub floating_notes: ShortcutBinding,
    pub quick_capture: ShortcutBinding,
}

impl Default for GlobalShortcuts {
    fn default() -> Self {
        Self {
            show_app: ShortcutBinding::new("Ctrl+Alt+Space"),
            toggle_focus_mode: ShortcutBinding::new("Ctrl+Alt+M"),
            find_focus_timer: ShortcutBinding::new("Ctrl+Alt+J"),
            floating_notes: ShortcutBinding::new("Alt+."),
            quick_capture: ShortcutBinding::new("Alt+,"),
        }
    }
}

/// Shortcuts that work while Thulli has focus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppShortcuts {
    pub new_task: ShortcutBinding,
    pub start_break: ShortcutBinding,
    pub pause: ShortcutBinding,
    pub skip: ShortcutBinding,
    pub finish: ShortcutBinding,
    pub add_notes: ShortcutBinding,
    pub search: ShortcutBinding,
}

impl Default for AppShortcuts {
    fn default() -> Self {
        Self {
            new_task: ShortcutBinding::new("Ctrl+Alt+T"),
            start_break: ShortcutBinding::new("Ctrl+Alt+B"),
            pause: ShortcutBinding::new("Ctrl+Alt+P"),
            skip: ShortcutBinding::new("Ctrl+Alt+S"),
            finish: ShortcutBinding::new("Ctrl+Alt+F"),
            add_notes: ShortcutBinding::new("Ctrl+Alt+N"),
            search: ShortcutBinding::new("Ctrl+F"),
        }
    }
}

/// Every user-adjustable setting. All fields default, so older files load.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    // Appearance and time
    pub theme: ThemeChoice,
    /// Empty means "use the system timezone".
    pub timezone: String,
    pub week_start: WeekStart,
    /// Interface zoom in percent (view commands: zoom in, zoom out, actual size).
    pub zoom_percent: u32,

    // General
    pub hide_estimates: bool,
    pub auto_parse_estimates: bool,
    pub open_at_login: bool,

    // Daily goal
    pub daily_goal_mins: u32,
    pub count_weekends: bool,

    // Sessions
    pub default_session_mins: u32,
    pub sprint_enabled: bool,
    pub work_sprint_mins: u32,
    pub break_time_mins: u32,
    pub default_break_mins: u32,
    pub pause_suggestions: bool,
    pub scrolling_title: bool,

    // Alerts
    pub timed_alerts: bool,
    pub alert_interval_mins: u32,
    #[serde(alias = "sound_id")]
    pub alert_sound_id: String,
    pub alert_flash: bool,
    /// Minutes before a scheduled task at which its reminder fires.
    pub reminder_mins_before: u32,

    // Celebration
    pub success_screen: bool,
    pub success_animation: bool,
    pub success_sound: bool,
    pub success_sound_id: String,

    // Compact timer panel
    pub compact_panel_screen: u32,
    pub compact_panel_side: PanelSide,

    // Shortcuts
    pub global_shortcuts: GlobalShortcuts,
    pub app_shortcuts: AppShortcuts,

    // Distraction shield
    pub app_shield_active: bool,
    pub web_shield_active: bool,

    // Assistant: user-chosen model file; never downloaded automatically.
    pub assistant_model_path: String,

    // Interface state
    pub quick_notes: String,
    pub active_mode: String,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            timezone: String::new(),
            week_start: WeekStart::Monday,
            zoom_percent: 100,

            hide_estimates: false,
            auto_parse_estimates: true,
            open_at_login: false,

            daily_goal_mins: 360,
            count_weekends: false,

            default_session_mins: 90,
            sprint_enabled: false,
            work_sprint_mins: 60,
            break_time_mins: 15,
            default_break_mins: 10,
            pause_suggestions: false,
            scrolling_title: true,

            timed_alerts: true,
            alert_interval_mins: 30,
            alert_sound_id: "basic-ding".to_string(),
            alert_flash: false,
            reminder_mins_before: 5,

            success_screen: true,
            success_animation: true,
            success_sound: true,
            success_sound_id: "soft-chime".to_string(),

            compact_panel_screen: 0,
            compact_panel_side: PanelSide::Right,

            global_shortcuts: GlobalShortcuts::default(),
            app_shortcuts: AppShortcuts::default(),

            app_shield_active: false,
            web_shield_active: false,

            assistant_model_path: String::new(),

            quick_notes: String::new(),
            active_mode: "dashboard".to_string(),
        }
    }
}

/// Shortest and longest focus session offered by the duration picker.
pub const SESSION_MIN_MINS: u32 = 10;
pub const SESSION_MAX_MINS: u32 = 180;
pub const SESSION_STEP_MINS: u32 = 5;

impl Preferences {
    /// Daily goal spread over the working days of a week.
    pub fn weekly_goal_mins(&self) -> u32 {
        self.daily_goal_mins * if self.count_weekends { 7 } else { 5 }
    }

    /// Clamps a requested session length to the picker's range and step.
    pub fn clamp_session_mins(mins: u32) -> u32 {
        let clamped = mins.clamp(SESSION_MIN_MINS, SESSION_MAX_MINS);
        (clamped / SESSION_STEP_MINS) * SESSION_STEP_MINS
    }

    /// Human-readable timezone: the chosen one, or the system offset.
    pub fn timezone_label(&self) -> String {
        if self.timezone.trim().is_empty() {
            format!("System time (UTC{})", Local::now().format("%:z"))
        } else {
            self.timezone.clone()
        }
    }

    /// Normalises values written by older builds.
    pub fn migrate(&mut self) {
        if self.timezone == LEGACY_DEFAULT_TIMEZONE {
            self.timezone.clear();
        }
        self.default_session_mins = Self::clamp_session_mins(self.default_session_mins);
    }
}
