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

/// The five accent colour themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AccentTheme {
    #[default]
    Teal,
    Blue,
    Violet,
    Amber,
    Rose,
}

/// How the focus run is shown. `Tower` is a docked screen-edge panel and
/// `Pill` a tiny floating timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FocusDisplayMode {
    #[default]
    Expanded,
    Tower,
    Pill,
}

/// How the expanded focus view fills the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExpandedWindowMode {
    #[default]
    FullScreen,
    Maximized,
}

/// Which in-app suggestions are switched on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SuggestionToggles {
    pub plan_my_day: bool,
    pub welcome_back: bool,
    pub take_a_break: bool,
    pub wind_down: bool,
    pub prioritize: bool,
}

impl Default for SuggestionToggles {
    fn default() -> Self {
        Self { plan_my_day: true, welcome_back: true, take_a_break: true, wind_down: true, prioritize: true }
    }
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
            show_app: ShortcutBinding::new("Ctrl+Alt+Shift+T"),
            toggle_focus_mode: ShortcutBinding::new("Ctrl+Alt+Shift+M"),
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
    pub dashboard: ShortcutBinding,
    pub new_list: ShortcutBinding,
    pub find_pill: ShortcutBinding,
    pub preferences: ShortcutBinding,
    pub next_task: ShortcutBinding,
    /// Switches the focus run between tower and pill.
    pub switch_display: ShortcutBinding,
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
            dashboard: ShortcutBinding::new("Ctrl+Shift+,"),
            new_list: ShortcutBinding::new("Ctrl+Alt+L"),
            find_pill: ShortcutBinding::new("Ctrl+Alt+J"),
            preferences: ShortcutBinding::new("Ctrl+,"),
            next_task: ShortcutBinding::new("Ctrl+Alt+Right"),
            switch_display: ShortcutBinding::new("Ctrl+Alt+D"),
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
    pub language: String,
    pub accent_theme: AccentTheme,

    // General
    pub hide_estimates: bool,
    pub auto_parse_estimates: bool,
    pub open_at_login: bool,
    pub open_on_wake: bool,
    /// Wrap long task titles instead of cutting them off.
    pub show_full_title: bool,
    /// Set once the first-run checklist is finished or dismissed.
    pub first_run_done: bool,

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
    /// Start the next task when a break ends early.
    pub start_next_after_early_break: bool,

    // Focus run
    pub focus_display_mode: FocusDisplayMode,
    pub tower_side: PanelSide,
    /// Let the tower and pill be dragged anywhere.
    pub move_freely: bool,
    /// Tuck the tower against the screen edge when not in use.
    pub auto_hide: bool,
    pub expanded_window_mode: ExpandedWindowMode,
    /// A preset id, or the path of the user's own image.
    pub focus_background: String,

    // Alerts
    pub timed_alerts: bool,
    pub alert_interval_mins: u32,
    #[serde(alias = "sound_id")]
    pub alert_sound_id: String,
    pub alert_flash: bool,
    /// Minutes before a scheduled task at which its reminder fires.
    pub reminder_mins_before: u32,
    pub notification_sound_id: String,
    pub break_start_sound_id: String,
    pub task_start_sound_id: String,
    pub overtime_sound_id: String,
    pub overtime_alert: bool,
    pub reminder_sound_id: String,
    /// Remind about a break every N minutes of work; zero turns it off.
    pub break_reminder_mins: u32,
    pub break_reminder_sound_id: String,
    /// Animate the pill when an alert fires.
    pub pill_alert_animated: bool,

    // Celebration
    pub success_screen: bool,
    pub success_animation: bool,
    pub success_animation_on_break_end: bool,
    /// Artwork sets for tasks finished within and over their estimate.
    pub success_set_on_time: String,
    pub success_set_over_estimate: String,
    pub success_sound: bool,
    pub success_sound_id: String,

    // Compact timer panel
    pub compact_panel_screen: u32,
    pub compact_panel_side: PanelSide,

    // Shortcuts
    pub global_shortcuts: GlobalShortcuts,
    pub app_shortcuts: AppShortcuts,

    // Assistant suggestions
    pub suggestions: SuggestionToggles,

    // Distraction shield
    pub app_shield_active: bool,
    pub web_shield_active: bool,

    // Pomodoro cycle: long break length, focus rounds before it, and whether
    // the next phase starts on its own.
    pub pomodoro_long_break_mins: u32,
    pub pomodoro_rounds: u32,
    pub pomodoro_auto_advance: bool,
    /// Length of the Custom focus interval, in minutes.
    pub pomodoro_custom_mins: u32,
    /// Ambient sound while the timer runs: off, forest, lofi or rain.
    pub pomodoro_ambient: String,

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
            language: "en".to_string(),
            accent_theme: AccentTheme::Teal,

            hide_estimates: false,
            auto_parse_estimates: true,
            open_at_login: false,
            open_on_wake: false,
            show_full_title: false,
            first_run_done: false,

            daily_goal_mins: 360,
            count_weekends: false,

            default_session_mins: 50,
            sprint_enabled: false,
            work_sprint_mins: 60,
            break_time_mins: 15,
            default_break_mins: 10,
            pause_suggestions: false,
            scrolling_title: true,
            start_next_after_early_break: false,

            focus_display_mode: FocusDisplayMode::Expanded,
            tower_side: PanelSide::Right,
            move_freely: false,
            auto_hide: false,
            expanded_window_mode: ExpandedWindowMode::FullScreen,
            focus_background: "calm".to_string(),

            timed_alerts: true,
            alert_interval_mins: 30,
            alert_sound_id: "basic-ding".to_string(),
            alert_flash: false,
            reminder_mins_before: 5,
            notification_sound_id: "basic-ding".to_string(),
            break_start_sound_id: "soft-chime".to_string(),
            task_start_sound_id: "soft-chime".to_string(),
            overtime_sound_id: "basic-ding".to_string(),
            overtime_alert: true,
            reminder_sound_id: "basic-ding".to_string(),
            break_reminder_mins: 0,
            pomodoro_long_break_mins: 20,
            pomodoro_rounds: 4,
            pomodoro_auto_advance: false,
            pomodoro_custom_mins: 40,
            pomodoro_ambient: "forest".to_string(),
            break_reminder_sound_id: "soft-chime".to_string(),
            pill_alert_animated: true,

            success_screen: true,
            success_animation: true,
            success_animation_on_break_end: false,
            success_set_on_time: "sunrise".to_string(),
            success_set_over_estimate: "steady".to_string(),
            success_sound: true,
            success_sound_id: "soft-chime".to_string(),

            compact_panel_screen: 0,
            compact_panel_side: PanelSide::Right,

            global_shortcuts: GlobalShortcuts::default(),
            app_shortcuts: AppShortcuts::default(),

            suggestions: SuggestionToggles::default(),

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
