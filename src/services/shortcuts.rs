//! Keyboard shortcuts: a registry of named actions with enable switches,
//! rebinding and conflict detection, for both system-wide (global) shortcuts
//! and shortcuts that work while Thulli has focus.
//!
//! # Connecting a handler
//!
//! Every shortcut is an *action* with a stable id (see [`SHORTCUT_DEFS`]).
//! Other parts of the app attach behaviour with [`ShortcutRegistry::on`]:
//!
//! ```ignore
//! registry.on("new_task", move || { /* open the composer */ });
//! ```
//!
//! The registry runs the handlers when the key combination is pressed, but
//! only while the action is enabled. Handlers live on the UI thread.
//!
//! Global shortcuts go through a [`HotkeyBackend`]; the real one registers
//! with the operating system (`global-hotkey`), tests use a mock.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::models::{Preferences, ShortcutBinding};
use crate::services::store_handle::StoreHandle;

/// Where a shortcut works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutScope {
    /// Anywhere on the computer, even when Thulli is in the background.
    Global,
    /// Only while Thulli has focus.
    App,
}

/// A named action that can have a shortcut.
#[derive(Debug, Clone, Copy)]
pub struct ShortcutDef {
    /// Stable id used with [`ShortcutRegistry::on`].
    pub id: &'static str,
    pub label: &'static str,
    pub scope: ShortcutScope,
}

const fn def(id: &'static str, label: &'static str, scope: ShortcutScope) -> ShortcutDef {
    ShortcutDef { id, label, scope }
}

use ShortcutScope::{App, Global};

/// Every action that can have a shortcut, in the order shown to the user.
pub const SHORTCUT_DEFS: &[ShortcutDef] = &[
    def("show_app", "Show Thulli", Global),
    def("toggle_focus_mode", "Switch focus mode", Global),
    def("find_focus_timer", "Find the focus timer", Global),
    def("floating_notes", "Floating notes", Global),
    def("quick_capture", "Quick capture", Global),
    def("new_task", "New task", App),
    def("start_break", "Start a break", App),
    def("pause", "Pause or resume", App),
    def("skip", "Skip", App),
    def("finish", "Finish", App),
    def("add_notes", "Add notes", App),
    def("search", "Search", App),
    def("dashboard", "Go to the dashboard", App),
    def("new_list", "New list", App),
    def("find_pill", "Find the timer pill", App),
    def("preferences", "Open preferences", App),
    def("next_task", "Next task", App),
    def("switch_display", "Switch tower and pill", App),
];

/// Looks up an action by id.
pub fn find_def(id: &str) -> Option<&'static ShortcutDef> {
    SHORTCUT_DEFS.iter().find(|d| d.id == id)
}

fn binding<'a>(p: &'a Preferences, id: &str) -> Option<&'a ShortcutBinding> {
    let g = &p.global_shortcuts;
    let a = &p.app_shortcuts;
    Some(match id {
        "show_app" => &g.show_app,
        "toggle_focus_mode" => &g.toggle_focus_mode,
        "find_focus_timer" => &g.find_focus_timer,
        "floating_notes" => &g.floating_notes,
        "quick_capture" => &g.quick_capture,
        "new_task" => &a.new_task,
        "start_break" => &a.start_break,
        "pause" => &a.pause,
        "skip" => &a.skip,
        "finish" => &a.finish,
        "add_notes" => &a.add_notes,
        "search" => &a.search,
        "dashboard" => &a.dashboard,
        "new_list" => &a.new_list,
        "find_pill" => &a.find_pill,
        "preferences" => &a.preferences,
        "next_task" => &a.next_task,
        "switch_display" => &a.switch_display,
        _ => return None,
    })
}

fn binding_mut<'a>(p: &'a mut Preferences, id: &str) -> Option<&'a mut ShortcutBinding> {
    let g = &mut p.global_shortcuts;
    let a = &mut p.app_shortcuts;
    Some(match id {
        "show_app" => &mut g.show_app,
        "toggle_focus_mode" => &mut g.toggle_focus_mode,
        "find_focus_timer" => &mut g.find_focus_timer,
        "floating_notes" => &mut g.floating_notes,
        "quick_capture" => &mut g.quick_capture,
        "new_task" => &mut a.new_task,
        "start_break" => &mut a.start_break,
        "pause" => &mut a.pause,
        "skip" => &mut a.skip,
        "finish" => &mut a.finish,
        "add_notes" => &mut a.add_notes,
        "search" => &mut a.search,
        "dashboard" => &mut a.dashboard,
        "new_list" => &mut a.new_list,
        "find_pill" => &mut a.find_pill,
        "preferences" => &mut a.preferences,
        "next_task" => &mut a.next_task,
        "switch_display" => &mut a.switch_display,
        _ => return None,
    })
}

/// The shipped key combination for an action.
pub fn default_keys(id: &str) -> Option<String> {
    binding(&Preferences::default(), id).map(|b| b.keys.clone())
}

// ---------------------------------------------------------------------------
// Key combinations
// ---------------------------------------------------------------------------

/// Why a shortcut change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutError {
    UnknownAction(String),
    /// The text is not a usable combination.
    InvalidKeys(String),
    /// Another enabled shortcut of the same kind already uses these keys.
    Conflict { with_id: String, with_label: String },
}

impl fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShortcutError::UnknownAction(id) => write!(f, "Unknown shortcut: {id}"),
            ShortcutError::InvalidKeys(why) => write!(f, "{why}"),
            ShortcutError::Conflict { with_label, .. } => write!(f, "Already used by \"{with_label}\""),
        }
    }
}

impl std::error::Error for ShortcutError {}

/// A parsed key combination such as `Ctrl+Alt+T`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    /// Canonical key name: `A`, `7`, `F5`, `Space`, `Left`, `,` and so on.
    pub key: String,
}

fn canonical_key(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let lower = t.to_lowercase();
    let named = match lower.as_str() {
        "space" | "spacebar" => "Space",
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "esc" | "escape" => "Escape",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "insert" | "ins" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" | "pgup" => "PageUp",
        "pagedown" | "pgdn" => "PageDown",
        "up" | "arrowup" => "Up",
        "down" | "arrowdown" => "Down",
        "left" | "arrowleft" => "Left",
        "right" | "arrowright" => "Right",
        "plus" => "+",
        "comma" => ",",
        "period" | "dot" => ".",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_string());
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        if (1..=24).contains(&n) {
            return Some(format!("F{n}"));
        }
    }
    let mut chars = t.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if c.is_ascii_alphanumeric() {
        Some(c.to_ascii_uppercase().to_string())
    } else if ",./;'[]\\-=`+".contains(c) {
        Some(c.to_string())
    } else {
        None
    }
}

impl KeyCombo {
    /// Parses text like `Ctrl+Alt+Shift+T` (case-insensitive, any modifier
    /// order, common aliases accepted).
    pub fn parse(text: &str) -> Result<KeyCombo, ShortcutError> {
        let bad = |why: &str| ShortcutError::InvalidKeys(why.to_string());
        let text = text.trim();
        if text.is_empty() {
            return Err(bad("Press a key combination"));
        }
        // A trailing '+' is the plus key itself.
        let (mods_part, key_part) = if text.ends_with('+') {
            (text[..text.len() - 1].trim_end_matches('+'), "+".to_string())
        } else {
            match text.rsplit_once('+') {
                Some((m, k)) => (m, k.to_string()),
                None => ("", text.to_string()),
            }
        };
        let mut combo = KeyCombo { ctrl: false, alt: false, shift: false, meta: false, key: String::new() };
        for m in mods_part.split('+').map(str::trim).filter(|m| !m.is_empty()) {
            match m.to_lowercase().as_str() {
                "ctrl" | "control" => combo.ctrl = true,
                "alt" | "option" => combo.alt = true,
                "shift" => combo.shift = true,
                "meta" | "win" | "windows" | "cmd" | "command" | "super" => combo.meta = true,
                _ => return Err(bad("Unknown modifier key")),
            }
        }
        combo.key = canonical_key(&key_part).ok_or_else(|| bad("That key cannot be used"))?;
        Ok(combo)
    }

    /// Whether the combination is safe to use as a shortcut: it must not hijack
    /// ordinary typing, so it needs Ctrl, Alt or Win unless it is a function key.
    pub fn validate(&self) -> Result<(), ShortcutError> {
        let is_fkey = self.key.starts_with('F') && self.key.len() > 1 && self.key[1..].chars().all(|c| c.is_ascii_digit());
        if self.ctrl || self.alt || self.meta || is_fkey {
            Ok(())
        } else {
            Err(ShortcutError::InvalidKeys("Include Ctrl, Alt or Win so typing is not interrupted".to_string()))
        }
    }

    /// Text form, modifiers in a fixed order, e.g. `Ctrl+Alt+T`.
    pub fn display(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.meta {
            parts.push("Win");
        }
        let mut s = parts.join("+");
        if !s.is_empty() {
            s.push('+');
        }
        s.push_str(&self.key);
        s
    }

    /// The operating-system key code for the key, if one exists.
    pub fn code(&self) -> Option<Code> {
        let k = self.key.as_str();
        let code = match k {
            "A" => Code::KeyA, "B" => Code::KeyB, "C" => Code::KeyC, "D" => Code::KeyD, "E" => Code::KeyE,
            "F" => Code::KeyF, "G" => Code::KeyG, "H" => Code::KeyH, "I" => Code::KeyI, "J" => Code::KeyJ,
            "K" => Code::KeyK, "L" => Code::KeyL, "M" => Code::KeyM, "N" => Code::KeyN, "O" => Code::KeyO,
            "P" => Code::KeyP, "Q" => Code::KeyQ, "R" => Code::KeyR, "S" => Code::KeyS, "T" => Code::KeyT,
            "U" => Code::KeyU, "V" => Code::KeyV, "W" => Code::KeyW, "X" => Code::KeyX, "Y" => Code::KeyY,
            "Z" => Code::KeyZ,
            "0" => Code::Digit0, "1" => Code::Digit1, "2" => Code::Digit2, "3" => Code::Digit3,
            "4" => Code::Digit4, "5" => Code::Digit5, "6" => Code::Digit6, "7" => Code::Digit7,
            "8" => Code::Digit8, "9" => Code::Digit9,
            "F1" => Code::F1, "F2" => Code::F2, "F3" => Code::F3, "F4" => Code::F4, "F5" => Code::F5,
            "F6" => Code::F6, "F7" => Code::F7, "F8" => Code::F8, "F9" => Code::F9, "F10" => Code::F10,
            "F11" => Code::F11, "F12" => Code::F12, "F13" => Code::F13, "F14" => Code::F14,
            "F15" => Code::F15, "F16" => Code::F16, "F17" => Code::F17, "F18" => Code::F18,
            "F19" => Code::F19, "F20" => Code::F20, "F21" => Code::F21, "F22" => Code::F22,
            "F23" => Code::F23, "F24" => Code::F24,
            "Space" => Code::Space, "Enter" => Code::Enter, "Tab" => Code::Tab, "Escape" => Code::Escape,
            "Backspace" => Code::Backspace, "Delete" => Code::Delete, "Insert" => Code::Insert,
            "Home" => Code::Home, "End" => Code::End, "PageUp" => Code::PageUp, "PageDown" => Code::PageDown,
            "Up" => Code::ArrowUp, "Down" => Code::ArrowDown, "Left" => Code::ArrowLeft, "Right" => Code::ArrowRight,
            "," => Code::Comma, "." => Code::Period, "/" => Code::Slash, ";" => Code::Semicolon,
            "'" => Code::Quote, "[" => Code::BracketLeft, "]" => Code::BracketRight,
            "\\" => Code::Backslash, "-" => Code::Minus, "=" | "+" => Code::Equal, "`" => Code::Backquote,
            _ => return None,
        };
        Some(code)
    }

    /// The operating-system hotkey for this combination.
    pub fn to_hotkey(&self) -> Option<HotKey> {
        let mut mods = Modifiers::empty();
        if self.ctrl {
            mods |= Modifiers::CONTROL;
        }
        if self.alt {
            mods |= Modifiers::ALT;
        }
        if self.shift {
            mods |= Modifiers::SHIFT;
        }
        if self.meta {
            mods |= Modifiers::SUPER;
        }
        Some(HotKey::new((!mods.is_empty()).then_some(mods), self.code()?))
    }
}

/// Builds a combination text from a key press as the UI reports it (the key's
/// text plus modifier state). Returns `None` for a press of only a modifier
/// key, so a recorder keeps waiting for the real key.
pub fn combo_from_key_event(text: &str, ctrl: bool, alt: bool, shift: bool, meta: bool) -> Option<String> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let key: String = match c {
        // Modifier keys on their own.
        '\u{10}'..='\u{18}' => return None,
        '\u{1b}' => "Escape".into(),
        '\u{8}' => "Backspace".into(),
        '\t' => "Tab".into(),
        '\n' | '\r' => "Enter".into(),
        '\u{7f}' => "Delete".into(),
        ' ' => "Space".into(),
        '\u{F700}' => "Up".into(),
        '\u{F701}' => "Down".into(),
        '\u{F702}' => "Left".into(),
        '\u{F703}' => "Right".into(),
        '\u{F704}'..='\u{F71B}' => format!("F{}", c as u32 - 0xF704 + 1),
        '\u{F727}' => "Insert".into(),
        '\u{F729}' => "Home".into(),
        '\u{F72B}' => "End".into(),
        '\u{F72C}' => "PageUp".into(),
        '\u{F72D}' => "PageDown".into(),
        // Ctrl+letter sometimes arrives as a control character.
        '\u{1}'..='\u{1a}' => ((b'A' + (c as u8 - 1)) as char).to_string(),
        other => {
            let base = if shift { unshift(other) } else { other };
            canonical_key(&base.to_string())?
        }
    };
    let combo = KeyCombo { ctrl, alt, shift, meta, key };
    Some(combo.display())
}

/// Undoes Shift on a US keyboard so `Shift+,` is stored as `,` with Shift.
fn unshift(c: char) -> char {
    match c {
        '<' => ',', '>' => '.', '?' => '/', ':' => ';', '"' => '\'', '{' => '[', '}' => ']', '|' => '\\',
        '_' => '-', '+' => '=', '~' => '`', '!' => '1', '@' => '2', '#' => '3', '$' => '4', '%' => '5',
        '^' => '6', '&' => '7', '*' => '8', '(' => '9', ')' => '0',
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// System-wide hotkey registration.
pub trait HotkeyBackend {
    /// Starts listening for `combo`, reporting presses under `action_id`.
    fn register(&mut self, action_id: &str, combo: &KeyCombo) -> Result<(), String>;
    fn unregister(&mut self, action_id: &str);
    /// Action ids pressed since the last call.
    fn poll(&mut self) -> Vec<String>;
}

/// Registers with the operating system through `global-hotkey`. Create it on
/// the thread that runs the window event loop.
pub struct GlobalHotkeyBackend {
    manager: GlobalHotKeyManager,
    registered: HashMap<String, HotKey>,
}

impl GlobalHotkeyBackend {
    pub fn new() -> Result<Self, String> {
        let manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
        Ok(Self { manager, registered: HashMap::new() })
    }
}

impl HotkeyBackend for GlobalHotkeyBackend {
    fn register(&mut self, action_id: &str, combo: &KeyCombo) -> Result<(), String> {
        self.unregister(action_id);
        let hotkey = combo.to_hotkey().ok_or_else(|| "That key is not supported system-wide".to_string())?;
        self.manager.register(hotkey).map_err(|_| "Already in use by another program".to_string())?;
        self.registered.insert(action_id.to_string(), hotkey);
        Ok(())
    }

    fn unregister(&mut self, action_id: &str) {
        if let Some(hotkey) = self.registered.remove(action_id) {
            let _ = self.manager.unregister(hotkey);
        }
    }

    fn poll(&mut self) -> Vec<String> {
        let mut pressed = Vec::new();
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if let Some((action, _)) = self.registered.iter().find(|(_, hk)| hk.id() == event.id) {
                pressed.push(action.clone());
            }
        }
        pressed
    }
}

/// Does nothing; used when the system refuses a hotkey manager.
pub struct NoHotkeys;

impl HotkeyBackend for NoHotkeys {
    fn register(&mut self, _: &str, _: &KeyCombo) -> Result<(), String> {
        Err("System-wide shortcuts are not available".to_string())
    }
    fn unregister(&mut self, _: &str) {}
    fn poll(&mut self) -> Vec<String> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// How a shortcut stands right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutStatus {
    Active,
    Off,
    /// A global shortcut the system would not register (reason inside).
    Unavailable(String),
}

/// One row for the preferences screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub scope: ShortcutScope,
    pub keys: String,
    pub enabled: bool,
    pub status: ShortcutStatus,
}

type Handler = Rc<dyn Fn()>;

/// The in-app shortcut registry. Lives on the UI thread.
pub struct ShortcutRegistry {
    handle: StoreHandle,
    backend: RefCell<Box<dyn HotkeyBackend>>,
    handlers: RefCell<HashMap<String, Vec<Handler>>>,
    unavailable: RefCell<HashMap<String, String>>,
}

impl ShortcutRegistry {
    pub fn new(handle: StoreHandle, backend: Box<dyn HotkeyBackend>) -> Self {
        Self {
            handle,
            backend: RefCell::new(backend),
            handlers: RefCell::new(HashMap::new()),
            unavailable: RefCell::new(HashMap::new()),
        }
    }

    /// Connects a handler to an action. Several handlers may share an action.
    /// Unknown ids are ignored (and reported in debug builds).
    pub fn on(&self, action_id: &str, handler: impl Fn() + 'static) {
        debug_assert!(find_def(action_id).is_some(), "unknown shortcut action {action_id}");
        self.handlers.borrow_mut().entry(action_id.to_string()).or_default().push(Rc::new(handler));
    }

    /// Runs the action's handlers if it is enabled. Returns whether any ran.
    pub fn trigger(&self, action_id: &str) -> bool {
        let enabled = self.handle.read(|s| binding(&s.preferences, action_id).is_some_and(|b| b.enabled));
        if !enabled {
            return false;
        }
        let list: Vec<Handler> = self.handlers.borrow().get(action_id).cloned().unwrap_or_default();
        for h in &list {
            h();
        }
        !list.is_empty()
    }

    /// Handles a key press inside the app: runs the enabled in-app shortcut
    /// bound to those keys, if any. Returns whether the press was used.
    pub fn handle_app_key(&self, keys: &str) -> bool {
        let Ok(pressed) = KeyCombo::parse(keys) else { return false };
        let target = self.handle.read(|s| {
            SHORTCUT_DEFS.iter().filter(|d| d.scope == ShortcutScope::App).find_map(|d| {
                let b = binding(&s.preferences, d.id)?;
                (b.enabled && KeyCombo::parse(&b.keys).ok().as_ref() == Some(&pressed)).then_some(d.id)
            })
        });
        match target {
            Some(id) => self.trigger(id),
            None => false,
        }
    }

    /// Runs handlers for global shortcuts pressed since the last call.
    /// Call this a few times per second from the UI thread.
    pub fn poll_global(&self) -> usize {
        let pressed = self.backend.borrow_mut().poll();
        pressed.iter().filter(|id| self.trigger(id)).count()
    }

    /// Makes the system's registrations match the saved global shortcuts.
    pub fn sync_global(&self) {
        let wanted: Vec<(&'static str, bool, String)> = self.handle.read(|s| {
            SHORTCUT_DEFS
                .iter()
                .filter(|d| d.scope == ShortcutScope::Global)
                .filter_map(|d| binding(&s.preferences, d.id).map(|b| (d.id, b.enabled, b.keys.clone())))
                .collect()
        });
        let mut backend = self.backend.borrow_mut();
        let mut unavailable = self.unavailable.borrow_mut();
        unavailable.clear();
        for (id, enabled, keys) in wanted {
            backend.unregister(id);
            if !enabled {
                continue;
            }
            let result = KeyCombo::parse(&keys).map_err(|e| e.to_string()).and_then(|c| backend.register(id, &c));
            if let Err(reason) = result {
                unavailable.insert(id.to_string(), reason);
            }
        }
    }

    /// Every shortcut with its current state.
    pub fn list(&self) -> Vec<ShortcutInfo> {
        let unavailable = self.unavailable.borrow();
        self.handle.read(|s| {
            SHORTCUT_DEFS
                .iter()
                .map(|d| {
                    let b = binding(&s.preferences, d.id).expect("every def has a binding");
                    let status = if !b.enabled {
                        ShortcutStatus::Off
                    } else if let Some(reason) = unavailable.get(d.id) {
                        ShortcutStatus::Unavailable(reason.clone())
                    } else {
                        ShortcutStatus::Active
                    };
                    ShortcutInfo { id: d.id, label: d.label, scope: d.scope, keys: b.keys.clone(), enabled: b.enabled, status }
                })
                .collect()
        })
    }

    /// The other enabled shortcut of the same scope using `combo`, if any.
    pub fn find_conflict(&self, action_id: &str, combo: &KeyCombo) -> Option<(&'static str, &'static str)> {
        let scope = find_def(action_id)?.scope;
        self.handle.read(|s| {
            SHORTCUT_DEFS.iter().filter(|d| d.scope == scope && d.id != action_id).find_map(|d| {
                let b = binding(&s.preferences, d.id)?;
                (b.enabled && KeyCombo::parse(&b.keys).ok().as_ref() == Some(combo)).then_some((d.id, d.label))
            })
        })
    }

    /// Turns a shortcut on or off. Turning on is refused if its keys clash.
    pub fn set_enabled(&self, action_id: &str, enabled: bool) -> Result<(), ShortcutError> {
        let def = find_def(action_id).ok_or_else(|| ShortcutError::UnknownAction(action_id.to_string()))?;
        if enabled {
            let keys = self.handle.read(|s| binding(&s.preferences, action_id).map(|b| b.keys.clone())).unwrap_or_default();
            let combo = KeyCombo::parse(&keys)?;
            if let Some((id, label)) = self.find_conflict(action_id, &combo) {
                return Err(ShortcutError::Conflict { with_id: id.to_string(), with_label: label.to_string() });
            }
        }
        self.handle.mutate(|s| {
            if let Some(b) = binding_mut(&mut s.preferences, action_id) {
                b.enabled = enabled;
            }
        });
        if def.scope == ShortcutScope::Global {
            self.sync_global();
        }
        Ok(())
    }

    /// Changes the keys of a shortcut. Returns the normalised text. Refused
    /// when the keys are unusable or clash with another enabled shortcut of
    /// the same kind.
    pub fn rebind(&self, action_id: &str, keys: &str) -> Result<String, ShortcutError> {
        let def = find_def(action_id).ok_or_else(|| ShortcutError::UnknownAction(action_id.to_string()))?;
        let combo = KeyCombo::parse(keys)?;
        combo.validate()?;
        if let Some((id, label)) = self.find_conflict(action_id, &combo) {
            return Err(ShortcutError::Conflict { with_id: id.to_string(), with_label: label.to_string() });
        }
        let text = combo.display();
        self.handle.mutate(|s| {
            if let Some(b) = binding_mut(&mut s.preferences, action_id) {
                b.keys = text.clone();
            }
        });
        if def.scope == ShortcutScope::Global {
            self.sync_global();
        }
        Ok(text)
    }

    /// Puts a shortcut back to its shipped keys (still refused on a clash).
    pub fn reset(&self, action_id: &str) -> Result<String, ShortcutError> {
        let keys = default_keys(action_id).ok_or_else(|| ShortcutError::UnknownAction(action_id.to_string()))?;
        self.rebind(action_id, &keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AppStateStore;
    use crate::repository::{LoadReport, StorageRepository};
    use std::cell::Cell;
    use std::sync::{Arc, Mutex};

    struct MemoryRepo;
    impl StorageRepository for MemoryRepo {
        fn load_with_report(&self) -> LoadReport {
            LoadReport { store: AppStateStore::default(), recovery: None }
        }
        fn save(&self, _s: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockState {
        registered: HashMap<String, String>,
        refuse: Vec<String>,
        pressed: Vec<String>,
    }

    struct MockBackend(Rc<RefCell<MockState>>);
    impl HotkeyBackend for MockBackend {
        fn register(&mut self, id: &str, combo: &KeyCombo) -> Result<(), String> {
            let mut st = self.0.borrow_mut();
            if st.refuse.contains(&combo.display()) {
                return Err("Already in use by another program".into());
            }
            st.registered.insert(id.to_string(), combo.display());
            Ok(())
        }
        fn unregister(&mut self, id: &str) {
            self.0.borrow_mut().registered.remove(id);
        }
        fn poll(&mut self) -> Vec<String> {
            std::mem::take(&mut self.0.borrow_mut().pressed)
        }
    }

    fn registry() -> (ShortcutRegistry, Rc<RefCell<MockState>>) {
        let handle = StoreHandle::new(Arc::new(Mutex::new(AppStateStore::default())), Arc::new(MemoryRepo));
        let state = Rc::new(RefCell::new(MockState::default()));
        (ShortcutRegistry::new(handle, Box::new(MockBackend(state.clone()))), state)
    }

    #[test]
    fn every_action_has_a_binding_and_defaults_parse() {
        for d in SHORTCUT_DEFS {
            let keys = default_keys(d.id).unwrap_or_else(|| panic!("no binding for {}", d.id));
            let combo = KeyCombo::parse(&keys).unwrap_or_else(|e| panic!("{}: {e}", d.id));
            combo.validate().unwrap();
            assert!(combo.code().is_some(), "{} has no key code", d.id);
            assert_eq!(combo.display(), keys, "{} default is not canonical", d.id);
        }
        assert_eq!(SHORTCUT_DEFS.iter().filter(|d| d.scope == Global).count(), 5);
    }

    #[test]
    fn default_combos_are_unique_within_each_scope() {
        let mut seen: HashMap<(bool, String), &str> = HashMap::new();
        for d in SHORTCUT_DEFS {
            let keys = default_keys(d.id).unwrap();
            let combo = KeyCombo::parse(&keys).unwrap().display();
            let key = (d.scope == Global, combo.clone());
            if let Some(other) = seen.insert(key, d.id) {
                panic!("{} and {} share the default combo {combo}", other, d.id);
            }
        }
    }

    #[test]
    fn parsing_is_flexible_and_canonical() {
        assert_eq!(KeyCombo::parse("alt+ctrl+t").unwrap().display(), "Ctrl+Alt+T");
        assert_eq!(KeyCombo::parse("Control + Shift + Comma").unwrap().display(), "Ctrl+Shift+,");
        assert_eq!(KeyCombo::parse("Ctrl++").unwrap().display(), "Ctrl++");
        assert_eq!(KeyCombo::parse("Alt+.").unwrap().display(), "Alt+.");
        assert_eq!(KeyCombo::parse("cmd+f5").unwrap().display(), "Win+F5");
        assert!(KeyCombo::parse("").is_err());
        assert!(KeyCombo::parse("Ctrl+Banana").is_err());
        assert!(KeyCombo::parse("Hyper+A").is_err());
    }

    #[test]
    fn plain_keys_are_not_valid_shortcuts() {
        assert!(KeyCombo::parse("T").unwrap().validate().is_err());
        assert!(KeyCombo::parse("Shift+T").unwrap().validate().is_err());
        assert!(KeyCombo::parse("F8").unwrap().validate().is_ok());
        assert!(KeyCombo::parse("Alt+T").unwrap().validate().is_ok());
    }

    #[test]
    fn key_events_become_combo_text() {
        assert_eq!(combo_from_key_event("t", true, true, false, false).as_deref(), Some("Ctrl+Alt+T"));
        assert_eq!(combo_from_key_event(" ", true, true, false, false).as_deref(), Some("Ctrl+Alt+Space"));
        assert_eq!(combo_from_key_event("<", true, false, true, false).as_deref(), Some("Ctrl+Shift+,"));
        assert_eq!(combo_from_key_event("\u{F703}", true, true, false, false).as_deref(), Some("Ctrl+Alt+Right"));
        assert_eq!(combo_from_key_event("\u{F708}", false, false, false, false).as_deref(), Some("F5"));
        assert_eq!(combo_from_key_event("\u{11}", true, false, false, false), None, "modifier only");
        assert_eq!(combo_from_key_event("", false, false, false, false), None);
    }

    #[test]
    fn rebind_normalises_and_saves() {
        let (reg, _) = registry();
        assert_eq!(reg.rebind("new_task", "ctrl+shift+n").unwrap(), "Ctrl+Shift+N");
        let row = reg.list().into_iter().find(|r| r.id == "new_task").unwrap();
        assert_eq!(row.keys, "Ctrl+Shift+N");
        assert!(matches!(reg.rebind("new_task", "N"), Err(ShortcutError::InvalidKeys(_))));
        assert!(matches!(reg.rebind("nope", "Ctrl+N"), Err(ShortcutError::UnknownAction(_))));
        assert_eq!(reg.reset("new_task").unwrap(), "Ctrl+Alt+T");
    }

    #[test]
    fn conflicts_are_detected_per_scope_among_enabled() {
        let (reg, _) = registry();
        // app vs app
        match reg.rebind("pause", "Ctrl+Alt+T") {
            Err(ShortcutError::Conflict { with_id, .. }) => assert_eq!(with_id, "new_task"),
            other => panic!("expected conflict, got {other:?}"),
        }
        // a disabled shortcut frees its keys
        reg.set_enabled("new_task", false).unwrap();
        assert!(reg.rebind("pause", "Ctrl+Alt+T").is_ok());
        // ...and cannot be turned back on while clashing
        assert!(matches!(reg.set_enabled("new_task", true), Err(ShortcutError::Conflict { .. })));
        // global vs app do not clash: find_focus_timer and find_pill share Ctrl+Alt+J by default
        assert_eq!(default_keys("find_focus_timer"), default_keys("find_pill"));
        assert!(reg.rebind("find_pill", "Ctrl+Alt+J").is_ok());
        // global vs global
        assert!(matches!(reg.rebind("quick_capture", "Alt+."), Err(ShortcutError::Conflict { .. })));
    }

    #[test]
    fn handlers_run_only_when_enabled() {
        let (reg, _) = registry();
        let count = Rc::new(Cell::new(0));
        let c = count.clone();
        reg.on("new_task", move || c.set(c.get() + 1));
        assert!(reg.trigger("new_task"));
        assert!(reg.handle_app_key("Ctrl+Alt+T"));
        assert!(reg.handle_app_key("ctrl+alt+t"));
        assert_eq!(count.get(), 3);
        assert!(!reg.handle_app_key("Ctrl+Alt+Q"));
        reg.set_enabled("new_task", false).unwrap();
        assert!(!reg.handle_app_key("Ctrl+Alt+T"));
        assert!(!reg.trigger("new_task"));
        assert_eq!(count.get(), 3);
    }

    #[test]
    fn app_keys_ignore_global_actions() {
        let (reg, _) = registry();
        let hit = Rc::new(Cell::new(false));
        let h = hit.clone();
        reg.on("show_app", move || h.set(true));
        assert!(!reg.handle_app_key("Ctrl+Alt+Shift+T"));
        assert!(!hit.get());
    }

    #[test]
    fn global_shortcuts_register_and_dispatch() {
        let (reg, state) = registry();
        reg.sync_global();
        {
            let st = state.borrow();
            assert_eq!(st.registered.len(), 5);
            assert_eq!(st.registered["show_app"], "Ctrl+Alt+Shift+T");
        }
        let hit = Rc::new(Cell::new(0));
        let h = hit.clone();
        reg.on("quick_capture", move || h.set(h.get() + 1));
        state.borrow_mut().pressed.push("quick_capture".into());
        assert_eq!(reg.poll_global(), 1);
        assert_eq!(hit.get(), 1);

        reg.set_enabled("quick_capture", false).unwrap();
        assert!(!state.borrow().registered.contains_key("quick_capture"));
        reg.rebind("show_app", "Ctrl+Alt+H").unwrap();
        assert_eq!(state.borrow().registered["show_app"], "Ctrl+Alt+H");
    }

    #[test]
    fn unavailable_global_shortcut_is_reported() {
        let (reg, state) = registry();
        state.borrow_mut().refuse.push("Ctrl+Alt+Shift+T".into());
        reg.sync_global();
        let row = reg.list().into_iter().find(|r| r.id == "show_app").unwrap();
        assert!(matches!(row.status, ShortcutStatus::Unavailable(_)));
        let other = reg.list().into_iter().find(|r| r.id == "quick_capture").unwrap();
        assert_eq!(other.status, ShortcutStatus::Active);
    }

    #[test]
    fn hotkeys_build_for_default_combos() {
        for d in SHORTCUT_DEFS {
            let combo = KeyCombo::parse(&default_keys(d.id).unwrap()).unwrap();
            assert!(combo.to_hotkey().is_some());
        }
    }
}
