//! Wiring for the preferences screen (ui/screens/preferences.slint).
//!
//! Connects the screen's `PreferencesState` to the preferences service, and
//! also starts the things settings control: the colour palette on the `Theme`
//! global, open-at-login, the alert engine (on each focus tick and on a
//! schedule check), and system-wide plus in-app keyboard shortcuts.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use chrono::Utc;
use slint::{Color, ComponentHandle, ModelRc, VecModel};
use thulli_core::models::{AccentTheme, Preferences, ThemeChoice};
use thulli_core::services::alerts::{
    perform, plan, plan_celebration, AlertEngine, AlertEvent, RodioPlayer, SoundPlayer, SystemNotifier, ToastNotifier,
};
use thulli_core::services::preferences::{
    system_prefers_dark, LoginItem, PreferencesService, RegistryLoginItem, ThemePalette, CELEBRATION_SETS,
    FOCUS_BACKGROUNDS, LANGUAGE_CHOICES, SOUND_CHOICES, SYSTEM_TIMEZONE_LABEL,
};
use thulli_core::services::shortcuts::{combo_from_key_event, ShortcutRegistry, ShortcutScope, ShortcutStatus};
use thulli_core::services::{TickOutcome, TimerSnapshot};

use crate::app::context::AppContext;
use crate::{MainWindow, PreferencesState, ShellState, ShortcutModel, Theme};

/// How often the schedule is checked for reminders and due tasks.
const SCHEDULE_CHECK: Duration = Duration::from_secs(5);
/// How often global shortcut presses are read from the system.
const HOTKEY_POLL: Duration = Duration::from_millis(250);
/// How long the celebration preview runs before it can be replayed.
const CELEBRATION_PREVIEW: Duration = Duration::from_millis(1500);

/// Settings that name a sound.
const SOUND_KEYS: &[&str] = &[
    "alert_sound",
    "overtime_sound",
    "reminder_sound",
    "break_reminder_sound",
    "break_start_sound",
    "task_start_sound",
    "notification_sound",
    "success_sound_id",
];

/// Everything the callbacks need. Cheap to clone; UI thread only.
#[derive(Clone)]
struct PrefsWiring {
    ctx: AppContext,
    window: slint::Weak<MainWindow>,
    login: Rc<dyn LoginItem>,
    sounds: Rc<dyn SoundPlayer>,
    notifier: Rc<dyn SystemNotifier>,
    shortcuts: Rc<ShortcutRegistry>,
    engine: Rc<RefCell<AlertEngine>>,
}

/// Connects the preferences screen and starts the alert and shortcut timers.
pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let shortcuts = ctx.shortcuts.clone();
    let wiring = PrefsWiring {
        ctx: ctx.clone(),
        window: window.as_weak(),
        login: Rc::new(RegistryLoginItem::for_current_exe()),
        sounds: Rc::new(RodioPlayer::new()),
        notifier: Rc::new(ToastNotifier),
        shortcuts: shortcuts.clone(),
        engine: Rc::new(RefCell::new(AlertEngine::new())),
    };

    wiring.ctx.preferences.sync_open_at_login(wiring.login.as_ref());

    // "Open preferences" works from anywhere once bound to a shortcut.
    {
        let weak = window.as_weak();
        shortcuts.on("preferences", move || {
            if let Some(w) = weak.upgrade() {
                w.global::<ShellState>().invoke_navigate("preferences".into());
            }
        });
    }

    shortcuts.sync_global();
    wiring.report_unavailable();
    wire_callbacks(window, &wiring);
    wire_alerts(&wiring);
    wiring.refresh();
    start_timers(&wiring);
}

fn wire_callbacks(window: &MainWindow, p: &PrefsWiring) {
    let st = window.global::<PreferencesState>();
    {
        let p = p.clone();
        st.on_set_bool(move |key, value| p.set_bool(key.as_str(), value));
    }
    {
        let p = p.clone();
        st.on_set_int(move |key, value| p.set_int(key.as_str(), value));
    }
    {
        let p = p.clone();
        st.on_set_text(move |key, value| p.set_text(key.as_str(), value.as_str()));
    }
    {
        let p = p.clone();
        st.on_cycle_choice(move |key| p.cycle(key.as_str()));
    }
    {
        let p = p.clone();
        st.on_preview_sound(move |key| p.preview_sound(key.as_str()));
    }
    {
        let p = p.clone();
        st.on_set_shortcut(move |id, _keys, enabled| p.set_shortcut(id.as_str(), enabled));
    }
    {
        let p = p.clone();
        st.on_captured_shortcut(move |id, text, ctrl, alt, shift, meta| {
            p.capture_shortcut(id.as_str(), text.as_str(), ctrl, alt, shift, meta)
        });
    }
    {
        let p = p.clone();
        st.on_reset_shortcut(move |id| p.reset_shortcut(id.as_str()));
    }
    {
        let p = p.clone();
        st.on_preview_celebration(move || p.preview_celebration());
    }
    {
        let p = p.clone();
        st.on_pick_model_file(move || p.pick_model_file());
    }
}

/// Alerts on every focus tick, and a schedule check on a timer.
fn wire_alerts(p: &PrefsWiring) {
    let tick = p.clone();
    p.ctx.on_tick(move |outcome: &TickOutcome, snap: &TimerSnapshot| {
        let prefs = tick.ctx.preferences.snapshot();
        let events = tick.engine.borrow_mut().on_tick(&prefs, snap, outcome);
        for event in &events {
            tick.announce(event, &prefs);
        }
    });
}

/// Keeps the timers alive for the whole run of the app.
fn start_timers(p: &PrefsWiring) {
    let hotkeys = slint::Timer::default();
    let poll = p.clone();
    hotkeys.start(slint::TimerMode::Repeated, HOTKEY_POLL, move || {
        poll.shortcuts.poll_global();
    });

    let schedule = slint::Timer::default();
    let check = p.clone();
    schedule.start(slint::TimerMode::Repeated, SCHEDULE_CHECK, move || check.check_schedule());

    // Timers stop when dropped; these two must run until the window closes.
    std::mem::forget(hotkeys);
    std::mem::forget(schedule);
}

impl PrefsWiring {
    fn refresh(&self) {
        let Some(w) = self.window.upgrade() else { return };
        let st = w.global::<PreferencesState>();
        let p = self.ctx.preferences.snapshot();

        st.set_theme_label(label("theme", &p).into());
        st.set_accent_label(label("accent_theme", &p).into());
        st.set_week_label(label("week_start", &p).into());
        st.set_language_label(label("language", &p).into());
        st.set_timezone_label(label("timezone", &p).into());

        st.set_hide_estimates(p.hide_estimates);
        st.set_auto_parse_estimates(p.auto_parse_estimates);
        st.set_open_at_login(p.open_at_login);
        st.set_open_on_wake(p.open_on_wake);
        st.set_show_full_title(p.show_full_title);

        st.set_default_session_mins(p.default_session_mins as i32);
        st.set_sprint_enabled(p.sprint_enabled);
        st.set_work_sprint_mins(p.work_sprint_mins as i32);
        st.set_break_time_mins(p.break_time_mins as i32);
        st.set_default_break_mins(p.default_break_mins as i32);
        st.set_pause_suggestions(p.pause_suggestions);
        st.set_start_next_after_early_break(p.start_next_after_early_break);

        st.set_focus_mode_label(label("focus_display_mode", &p).into());
        st.set_tower_side_label(label("tower_side", &p).into());
        st.set_move_freely(p.move_freely);
        st.set_auto_hide(p.auto_hide);
        st.set_window_mode_label(label("expanded_window_mode", &p).into());
        st.set_background_label(label("focus_background", &p).into());

        st.set_timed_alerts(p.timed_alerts);
        st.set_alert_interval_mins(p.alert_interval_mins as i32);
        st.set_alert_flash(p.alert_flash);
        st.set_overtime_alert(p.overtime_alert);
        st.set_reminder_mins_before(p.reminder_mins_before as i32);
        st.set_break_reminder_mins(p.break_reminder_mins as i32);
        st.set_pill_alert_animated(p.pill_alert_animated);
        st.set_alert_sound_label(label("alert_sound", &p).into());
        st.set_overtime_sound_label(label("overtime_sound", &p).into());
        st.set_reminder_sound_label(label("reminder_sound", &p).into());
        st.set_break_reminder_sound_label(label("break_reminder_sound", &p).into());
        st.set_break_start_sound_label(label("break_start_sound", &p).into());
        st.set_task_start_sound_label(label("task_start_sound", &p).into());
        st.set_notification_sound_label(label("notification_sound", &p).into());

        st.set_success_screen(p.success_screen);
        st.set_success_animation(p.success_animation);
        st.set_success_animation_on_break_end(p.success_animation_on_break_end);
        st.set_success_sound(p.success_sound);
        st.set_success_sound_label(label("success_sound_id", &p).into());
        st.set_set_on_time_label(label("success_set_on_time", &p).into());
        st.set_set_over_label(label("success_set_over_estimate", &p).into());

        st.set_compact_side_label(label("compact_panel_side", &p).into());

        st.set_suggest_plan_my_day(p.suggestions.plan_my_day);
        st.set_suggest_welcome_back(p.suggestions.welcome_back);
        st.set_suggest_take_a_break(p.suggestions.take_a_break);
        st.set_suggest_wind_down(p.suggestions.wind_down);
        st.set_suggest_prioritize(p.suggestions.prioritize);

        st.set_shortcuts(ModelRc::new(VecModel::from(self.shortcut_models())));
        st.set_model_label(model_label(&p.assistant_model_path).into());
        apply_palette(&w, &self.ctx.preferences.palette(system_prefers_dark()));
    }

    fn say(&self, text: &str) {
        if let Some(w) = self.window.upgrade() {
            w.global::<PreferencesState>().set_message(text.into());
        }
    }

    /// Shows the outcome of a change: the error, or a cleared message.
    fn finish<E: std::fmt::Display>(&self, result: Result<(), E>) {
        match result {
            Ok(()) => self.say(""),
            Err(e) => self.say(&e.to_string()),
        }
        self.refresh();
    }

    fn set_bool(&self, key: &str, value: bool) {
        let result = if key == "open_at_login" {
            self.ctx.preferences.set_open_at_login(self.login.as_ref(), value)
        } else {
            self.ctx.preferences.set_bool(key, value)
        };
        self.finish(result);
    }

    fn set_int(&self, key: &str, value: i32) {
        self.finish(self.ctx.preferences.set_int(key, value));
    }

    fn set_text(&self, key: &str, value: &str) {
        self.finish(self.ctx.preferences.set_text(key, value));
    }

    /// Moves a choice setting to its next option and plays a sound choice.
    fn cycle(&self, key: &str) {
        let prefs = self.ctx.preferences.snapshot();
        let Some(next) = next_choice(key, &prefs) else { return };
        let result = self.ctx.preferences.set_text(key, &next);
        if result.is_ok() && SOUND_KEYS.contains(&key) {
            self.sounds.play(&next);
        }
        self.finish(result);
    }

    fn preview_sound(&self, key: &str) {
        let prefs = self.ctx.preferences.snapshot();
        if SOUND_KEYS.contains(&key) {
            self.sounds.play(&current_id(key, &prefs));
        }
    }

    fn set_shortcut(&self, id: &str, enabled: bool) {
        let result = self.shortcuts.set_enabled(id, enabled);
        self.after_shortcut(result.map(|_| ()));
    }

    /// Records the keys pressed while a shortcut is waiting for new keys.
    fn capture_shortcut(&self, id: &str, text: &str, ctrl: bool, alt: bool, shift: bool, meta: bool) {
        let Some(combo) = combo_from_key_event(text, ctrl, alt, shift, meta) else {
            // Only a modifier was pressed; keep waiting for the real key.
            return;
        };
        if let Some(w) = self.window.upgrade() {
            w.global::<PreferencesState>().set_capturing_id("".into());
        }
        if combo == "Escape" {
            self.say("");
            self.refresh();
            return;
        }
        let result = self.shortcuts.rebind(id, &combo);
        self.after_shortcut(result.map(|_| ()));
    }

    fn reset_shortcut(&self, id: &str) {
        let result = self.shortcuts.reset(id);
        self.after_shortcut(result.map(|_| ()));
    }

    fn after_shortcut<E: std::fmt::Display>(&self, result: Result<(), E>) {
        match result {
            Ok(()) => self.report_unavailable(),
            Err(e) => self.say(&e.to_string()),
        }
        self.refresh();
    }

    /// Names shortcuts the system would not register.
    fn report_unavailable(&self) {
        let names: Vec<&str> = self
            .shortcuts
            .list()
            .iter()
            .filter(|s| matches!(s.status, ShortcutStatus::Unavailable(_)))
            .map(|s| s.label)
            .collect();
        if !names.is_empty() {
            self.say(&format!(
                "These shortcuts could not be registered, probably because another program uses the keys: {}.",
                names.join(", ")
            ));
        }
    }

    fn preview_celebration(&self) {
        let Some(w) = self.window.upgrade() else { return };
        let prefs = self.ctx.preferences.snapshot();
        if let Some(celebration) = plan_celebration(&prefs, false, false) {
            if let Some(sound) = &celebration.sound {
                self.sounds.play(sound);
            }
        }
        let style = match prefs.success_set_on_time.as_str() {
            "steady" | "ripple" => "ripple",
            _ => "confetti",
        };
        let st = w.global::<PreferencesState>();
        st.set_celebration_style(style.into());
        st.set_celebrating(true);

        let weak = self.window.clone();
        slint::Timer::single_shot(CELEBRATION_PREVIEW, move || {
            if let Some(w) = weak.upgrade() {
                w.global::<PreferencesState>().set_celebrating(false);
            }
        });
    }

    /// Carries out an alert. Window flash and pill animation are left to the
    /// shell, which owns the window.
    fn announce(&self, event: &AlertEvent, prefs: &Preferences) {
        let action = plan(event, prefs);
        perform(&action, self.sounds.as_ref(), self.notifier.as_ref());
        self.ctx.notify_alert(&action);
    }

    /// Lets the person choose the local assistant model (a .gguf file).
    fn pick_model_file(&self) {
        let picked = rfd::FileDialog::new()
            .set_title("Choose the assistant model")
            .add_filter("Model file", &["gguf"])
            .pick_file();
        if let Some(path) = picked {
            let text = path.to_string_lossy().into_owned();
            self.finish(self.ctx.preferences.set_text("assistant_model_path", &text));
        }
    }

    fn check_schedule(&self) {
        let tasks = self.ctx.handle.read(|s| s.tasks.clone());
        let prefs = self.ctx.preferences.snapshot();
        let events = self.engine.borrow_mut().check_schedule(Utc::now(), &tasks, &prefs);
        for event in &events {
            self.announce(event, &prefs);
        }
    }

    fn shortcut_models(&self) -> Vec<ShortcutModel> {
        self.shortcuts
            .list()
            .into_iter()
            .map(|s| ShortcutModel {
                id: s.id.into(),
                label: s.label.into(),
                keys: s.keys.into(),
                enabled: s.enabled,
                is_global: s.scope == ShortcutScope::Global,
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Choice catalogue: each choice setting has ids (what the service accepts)
// and labels (what the person sees).
// ---------------------------------------------------------------------------

fn options(key: &str) -> Vec<(String, String)> {
    if key == "timezone" {
        return PreferencesService::timezone_labels().into_iter().map(|l| (l.clone(), l)).collect();
    }
    let pairs: Vec<(&str, &str)> = match key {
        "theme" => vec![("system", "System"), ("dark", "Dark"), ("light", "Light")],
        "accent_theme" => vec![
            ("teal", "Teal"),
            ("blue", "Blue"),
            ("violet", "Violet"),
            ("amber", "Amber"),
            ("rose", "Rose"),
        ],
        "week_start" => vec![("monday", "Monday"), ("sunday", "Sunday"), ("saturday", "Saturday")],
        "language" => LANGUAGE_CHOICES.to_vec(),
        "focus_display_mode" => vec![("expanded", "Expanded"), ("tower", "Tower"), ("pill", "Pill")],
        "tower_side" | "compact_panel_side" => vec![("left", "Left"), ("right", "Right")],
        "expanded_window_mode" => vec![("full_screen", "Full screen"), ("maximized", "Maximized")],
        "focus_background" => FOCUS_BACKGROUNDS.to_vec(),
        "success_set_on_time" | "success_set_over_estimate" => CELEBRATION_SETS.to_vec(),
        k if SOUND_KEYS.contains(&k) => SOUND_CHOICES.to_vec(),
        _ => Vec::new(),
    };
    pairs.into_iter().map(|(id, label)| (id.to_string(), label.to_string())).collect()
}

/// The stored value of a choice setting, in the id form `set_text` accepts.
fn current_id(key: &str, p: &Preferences) -> String {
    let text = match key {
        "theme" => match p.theme {
            ThemeChoice::System => "system",
            ThemeChoice::Dark => "dark",
            ThemeChoice::Light => "light",
        },
        "accent_theme" => match p.accent_theme {
            AccentTheme::Teal => "teal",
            AccentTheme::Blue => "blue",
            AccentTheme::Violet => "violet",
            AccentTheme::Amber => "amber",
            AccentTheme::Rose => "rose",
        },
        "week_start" => match p.week_start {
            thulli_core::models::WeekStart::Monday => "monday",
            thulli_core::models::WeekStart::Sunday => "sunday",
            thulli_core::models::WeekStart::Saturday => "saturday",
        },
        "language" => return p.language.clone(),
        "timezone" => {
            return if p.timezone.is_empty() { SYSTEM_TIMEZONE_LABEL.to_string() } else { p.timezone.clone() };
        }
        "focus_display_mode" => match p.focus_display_mode {
            thulli_core::models::FocusDisplayMode::Expanded => "expanded",
            thulli_core::models::FocusDisplayMode::Tower => "tower",
            thulli_core::models::FocusDisplayMode::Pill => "pill",
        },
        "tower_side" => side_id(p.tower_side),
        "compact_panel_side" => side_id(p.compact_panel_side),
        "expanded_window_mode" => match p.expanded_window_mode {
            thulli_core::models::ExpandedWindowMode::FullScreen => "full_screen",
            thulli_core::models::ExpandedWindowMode::Maximized => "maximized",
        },
        "focus_background" => return p.focus_background.clone(),
        "alert_sound" => return p.alert_sound_id.clone(),
        "overtime_sound" => return p.overtime_sound_id.clone(),
        "reminder_sound" => return p.reminder_sound_id.clone(),
        "break_reminder_sound" => return p.break_reminder_sound_id.clone(),
        "break_start_sound" => return p.break_start_sound_id.clone(),
        "task_start_sound" => return p.task_start_sound_id.clone(),
        "notification_sound" => return p.notification_sound_id.clone(),
        "success_sound_id" => return p.success_sound_id.clone(),
        "success_set_on_time" => return p.success_set_on_time.clone(),
        "success_set_over_estimate" => return p.success_set_over_estimate.clone(),
        _ => "",
    };
    text.to_string()
}

fn side_id(side: thulli_core::models::PanelSide) -> &'static str {
    match side {
        thulli_core::models::PanelSide::Left => "left",
        thulli_core::models::PanelSide::Right => "right",
    }
}

fn label(key: &str, p: &Preferences) -> String {
    let current = current_id(key, p);
    options(key).into_iter().find(|(id, _)| *id == current).map(|(_, l)| l).unwrap_or(current)
}

fn next_choice(key: &str, p: &Preferences) -> Option<String> {
    let choices = options(key);
    if choices.is_empty() {
        return None;
    }
    let current = current_id(key, p);
    let index = choices.iter().position(|(id, _)| *id == current).unwrap_or(choices.len() - 1);
    Some(choices[(index + 1) % choices.len()].0.clone())
}

/// Pushes a palette onto the `Theme` global. Colours are 0xRRGGBB.
fn apply_palette(w: &MainWindow, palette: &ThemePalette) {
    fn rgb(v: u32) -> Color {
        Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }
    let t = w.global::<Theme>();
    t.set_bg_canvas(rgb(palette.bg_canvas));
    t.set_bg_sidebar(rgb(palette.bg_sidebar));
    t.set_bg_panel(rgb(palette.bg_panel));
    t.set_bg_card(rgb(palette.bg_card));
    t.set_bg_card_hover(rgb(palette.bg_card_hover));
    t.set_bg_card_selected(rgb(palette.bg_card_selected));
    t.set_border_subtle(rgb(palette.border_subtle));
    t.set_border_light(rgb(palette.border_light));
    t.set_text_primary(rgb(palette.text_primary));
    t.set_text_secondary(rgb(palette.text_secondary));
    t.set_text_muted(rgb(palette.text_muted));
    t.set_accent_cyan(rgb(palette.accent_cyan));
    t.set_accent_lime(rgb(palette.accent_lime));
}

/// The model file's name for the preferences row, or a plain prompt.
fn model_label(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return "Not chosen".to_string();
    }
    std::path::Path::new(trimmed)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| trimmed.to_string())
}
