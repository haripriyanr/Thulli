//! Distraction shield rules and enforcement while a focus session runs.
//!
//! * App blocking: while the timer reports a running focus session, matching
//!   processes are asked to close, then terminated if they linger.
//! * Website blocking: a clearly marked Thulli section of the system hosts
//!   file is added when a session starts and always removed when it ends, or
//!   at the next launch if the app went away mid-session.
//!
//! Process and hosts access sit behind the [`ProcessControl`] and [`HostsFile`]
//! traits so tests never touch real processes or the real hosts file.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::models::{BlockedApp, BlockedSite};
use crate::services::store_handle::StoreHandle;
use crate::services::timer::{TimerPhase, TimerPurpose, TimerSnapshot};

/// How often running processes are checked during a session.
pub const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// How long an app gets to close by itself before it is terminated.
pub const CLOSE_GRACE: Duration = Duration::from_secs(4);

/// First line of the Thulli section in the hosts file.
pub const HOSTS_BEGIN: &str = "# >>> Thulli website blocking (managed section, do not edit) >>>";
/// Last line of the Thulli section in the hosts file.
pub const HOSTS_END: &str = "# <<< Thulli website blocking <<<";

/// Plain-language note shown when the hosts file cannot be changed.
pub const NEEDS_ADMIN_MESSAGE: &str =
    "Website blocking needs Thulli to be run as administrator. App blocking still works without it.";

/// Programs that are never closed and cannot be added as rules: Thulli itself,
/// core system processes and common shells.
const PROTECTED: &[&str] = &[
    "thulli", "system", "system idle process", "registry", "smss", "csrss", "wininit", "winlogon", "services",
    "lsass", "lsm", "svchost", "dwm", "explorer", "fontdrvhost", "sihost", "taskhostw", "ctfmon", "conhost",
    "runtimebroker", "searchhost", "startmenuexperiencehost", "shellexperiencehost", "textinputhost",
    "securityhealthservice", "msmpeng", "spoolsv", "wudfhost", "audiodg", "taskmgr", "cmd", "powershell", "pwsh",
    "windowsterminal", "wt", "bash", "sh", "zsh", "fish", "wsl", "wslhost", "mintty", "nu", "git-bash", "login",
    "init", "systemd", "launchd", "kernel_task", "finder", "windowserver", "loginwindow",
];

/// Lower-cases a process name and drops a trailing `.exe`.
pub fn normalize_process(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    lower.strip_suffix(".exe").map(str::to_string).unwrap_or(lower)
}

/// True for Thulli itself, system processes and the user's shell.
pub fn is_protected(process_name: &str) -> bool {
    let norm = normalize_process(process_name);
    if PROTECTED.contains(&norm.as_str()) {
        return true;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(stem) = exe.file_stem().and_then(|s| s.to_str()) {
            if normalize_process(stem) == norm {
                return true;
            }
        }
    }
    false
}

/// True while a focus session (not a break or planning) is counting.
pub fn is_focus_running(snapshot: &TimerSnapshot) -> bool {
    snapshot.phase == TimerPhase::Running
        && matches!(snapshot.purpose, TimerPurpose::Session | TimerPurpose::Sprint | TimerPurpose::Task)
}

/// Why a rule could not be added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShieldError {
    Empty,
    InvalidApp,
    InvalidSite,
    /// The program is Thulli, part of the system, or a shell.
    Protected(String),
    Duplicate,
}

impl fmt::Display for ShieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShieldError::Empty => write!(f, "Type a name first."),
            ShieldError::InvalidApp => write!(f, "That does not look like a program name."),
            ShieldError::InvalidSite => write!(f, "That does not look like a website address."),
            ShieldError::Protected(n) => write!(f, "{n} is needed to keep your computer and Thulli running, so it cannot be blocked."),
            ShieldError::Duplicate => write!(f, "That one is already in your list."),
        }
    }
}

impl std::error::Error for ShieldError {}

/// Turns user input such as `https://www.Example.com/path` into `example.com`.
pub fn normalize_domain(input: &str) -> Result<String, ShieldError> {
    let mut s = input.trim().to_lowercase();
    if s.is_empty() {
        return Err(ShieldError::Empty);
    }
    if let Some(i) = s.find("://") {
        s = s[i + 3..].to_string();
    }
    if let Some(i) = s.find(['/', '?', '#']) {
        s.truncate(i);
    }
    if let Some(i) = s.rfind('@') {
        s = s[i + 1..].to_string();
    }
    if let Some(i) = s.find(':') {
        s.truncate(i);
    }
    let s = s.trim_matches('.').strip_prefix("www.").map(str::to_string).unwrap_or_else(|| s.trim_matches('.').to_string());
    let valid = s.contains('.')
        && s.len() <= 253
        && s.split('.').all(|l| {
            !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    if valid { Ok(s) } else { Err(ShieldError::InvalidSite) }
}

// ---------------------------------------------------------------------------
// Process access
// ---------------------------------------------------------------------------

/// A running process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    /// Executable name such as `chrome.exe`.
    pub name: String,
}

/// Lists and closes processes.
pub trait ProcessControl: Send + Sync {
    fn list(&self) -> Vec<ProcInfo>;
    /// Asks the process to close its windows. Returns false when there was
    /// nothing to ask (no windows) or the request failed.
    fn close_graceful(&self, pid: u32) -> bool;
    /// Ends the process. Returns whether it was stopped.
    fn terminate(&self, pid: u32) -> bool;
}

/// Real process access built on `sysinfo`, with Win32 window-close on Windows.
pub struct SystemProcesses {
    system: Mutex<sysinfo::System>,
}

impl SystemProcesses {
    pub fn new() -> Self {
        Self { system: Mutex::new(sysinfo::System::new()) }
    }
}

impl Default for SystemProcesses {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessControl for SystemProcesses {
    fn list(&self) -> Vec<ProcInfo> {
        let mut sys = self.system.lock().unwrap_or_else(|p| p.into_inner());
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        sys.processes()
            .iter()
            .map(|(pid, p)| ProcInfo { pid: pid.as_u32(), name: p.name().to_string_lossy().into_owned() })
            .collect()
    }

    fn close_graceful(&self, pid: u32) -> bool {
        post_close_to_windows(pid)
    }

    fn terminate(&self, pid: u32) -> bool {
        let sys = self.system.lock().unwrap_or_else(|p| p.into_inner());
        sys.process(sysinfo::Pid::from_u32(pid)).map(|p| p.kill()).unwrap_or(false)
    }
}

#[cfg(windows)]
fn post_close_to_windows(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};

    struct Target {
        pid: u32,
        posted: bool,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: `lparam` is the address of the `Target` owned by the caller
        // of `EnumWindows`, which outlives the enumeration.
        let target = unsafe { &mut *(lparam as *mut Target) };
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if owner == target.pid && unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) } != 0 {
            target.posted = true;
        }
        1
    }

    let mut target = Target { pid, posted: false };
    // SAFETY: the callback only touches `target`, which lives across the call.
    unsafe { EnumWindows(Some(visit), &mut target as *mut Target as LPARAM) };
    target.posted
}

#[cfg(not(windows))]
fn post_close_to_windows(_pid: u32) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Hosts file access
// ---------------------------------------------------------------------------

/// Reads and writes the system hosts file.
pub trait HostsFile: Send + Sync {
    fn read(&self) -> io::Result<String>;
    fn write(&self, contents: &str) -> io::Result<()>;
    /// Saves a copy of the original contents before Thulli changes the file.
    fn backup(&self, contents: &str) -> io::Result<()>;
    /// Whether the file can be changed right now (checked without changing it).
    fn can_write(&self) -> bool;
}

/// Hosts file at a path on disk.
pub struct FileHostsFile {
    path: PathBuf,
}

impl FileHostsFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The operating system's hosts file.
    pub fn system() -> Self {
        #[cfg(windows)]
        let path = {
            let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
            root.join("System32").join("drivers").join("etc").join("hosts")
        };
        #[cfg(not(windows))]
        let path = PathBuf::from("/etc/hosts");
        Self::new(path)
    }

    fn backup_path(&self) -> PathBuf {
        let mut name = self.path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        name.push(".thulli-backup");
        self.path.with_file_name(name)
    }
}

impl HostsFile for FileHostsFile {
    fn read(&self) -> io::Result<String> {
        let bytes = std::fs::read(&self.path)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn write(&self, contents: &str) -> io::Result<()> {
        std::fs::write(&self.path, contents)
    }

    fn backup(&self, contents: &str) -> io::Result<()> {
        std::fs::write(self.backup_path(), contents)
    }

    fn can_write(&self) -> bool {
        std::fs::OpenOptions::new().append(true).open(&self.path).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Hosts section editing (pure)
// ---------------------------------------------------------------------------

fn is_begin(line: &str) -> bool {
    line.trim() == HOSTS_BEGIN
}

fn is_end(line: &str) -> bool {
    line.trim() == HOSTS_END
}

fn looks_like_entry(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("0.0.0.0 ") || t.starts_with(":: ")
}

/// True when the text contains the Thulli section marker.
pub fn has_section(text: &str) -> bool {
    text.lines().any(is_begin)
}

/// Removes the Thulli section, leaving every other line exactly as it was.
///
/// If the end marker is missing (damaged file), only the begin line and the
/// block of Thulli-style entries right after it are removed.
pub fn strip_section(text: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < lines.len() {
        if is_begin(lines[i]) {
            let end = (i + 1..lines.len()).find(|&j| is_end(lines[j]));
            match end {
                Some(j) => i = j + 1,
                None => {
                    i += 1;
                    while i < lines.len() && looks_like_entry(lines[i]) {
                        i += 1;
                    }
                }
            }
        } else {
            out.push_str(lines[i]);
            i += 1;
        }
    }
    out
}

/// Returns `text` with a fresh Thulli section blocking `domains` (the old one,
/// if any, is replaced).
pub fn with_section(text: &str, domains: &[String]) -> String {
    let mut out = strip_section(text);
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    out.push_str(HOSTS_BEGIN);
    out.push_str(eol);
    for d in domains {
        for host in [d.clone(), format!("www.{d}")] {
            out.push_str(&format!("0.0.0.0 {host}{eol}:: {host}{eol}"));
        }
    }
    out.push_str(HOSTS_END);
    out.push_str(eol);
    out
}

// ---------------------------------------------------------------------------
// Website blocker
// ---------------------------------------------------------------------------

/// State of website blocking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebStatus {
    /// Nothing is blocked.
    Inactive,
    /// This many sites are blocked right now.
    Active(usize),
    /// The hosts file is not writable without administrator access.
    NeedsAdmin,
    /// Something else went wrong; the text is a plain-language reason.
    Failed(String),
}

impl WebStatus {
    /// Message for the screen, empty when there is nothing to say.
    pub fn message(&self) -> String {
        match self {
            WebStatus::NeedsAdmin => NEEDS_ADMIN_MESSAGE.to_string(),
            WebStatus::Failed(m) => m.clone(),
            _ => String::new(),
        }
    }
}

fn status_for_error(e: &io::Error) -> WebStatus {
    if e.kind() == io::ErrorKind::PermissionDenied {
        WebStatus::NeedsAdmin
    } else {
        WebStatus::Failed("Website blocking could not be changed right now. App blocking still works.".to_string())
    }
}

/// Adds and removes the Thulli section of the hosts file.
pub struct WebBlocker {
    hosts: Arc<dyn HostsFile>,
}

impl WebBlocker {
    pub fn new(hosts: Arc<dyn HostsFile>) -> Self {
        Self { hosts }
    }

    pub fn can_write(&self) -> bool {
        self.hosts.can_write()
    }

    /// Writes the section for `domains`, backing the file up first.
    pub fn apply(&self, domains: &[String]) -> WebStatus {
        if domains.is_empty() {
            return match self.remove() {
                WebStatus::Failed(m) => WebStatus::Failed(m),
                WebStatus::NeedsAdmin => WebStatus::NeedsAdmin,
                _ => WebStatus::Inactive,
            };
        }
        let original = match self.hosts.read() {
            Ok(t) => t,
            Err(e) => return status_for_error(&e),
        };
        if !has_section(&original) {
            if let Err(e) = self.hosts.backup(&original) {
                return status_for_error(&e);
            }
        }
        let updated = with_section(&original, domains);
        if updated == original {
            return WebStatus::Active(domains.len());
        }
        match self.hosts.write(&updated) {
            Ok(()) => WebStatus::Active(domains.len()),
            Err(e) => status_for_error(&e),
        }
    }

    /// Removes the section if present. Does not write when it is absent.
    pub fn remove(&self) -> WebStatus {
        let original = match self.hosts.read() {
            Ok(t) => t,
            Err(e) => return status_for_error(&e),
        };
        if !has_section(&original) {
            return WebStatus::Inactive;
        }
        match self.hosts.write(&strip_section(&original)) {
            Ok(()) => WebStatus::Inactive,
            Err(e) => status_for_error(&e),
        }
    }
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// What one `sync` call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Display names of programs closed or asked to close in this call.
    pub closed: Vec<String>,
    pub web: WebStatus,
}

#[derive(Default)]
struct Runtime {
    close_requested: HashMap<u32, Instant>,
    last_poll: Option<Instant>,
    web_applied: Option<Vec<String>>,
    web_attempted: Option<Vec<String>>,
    web_status: Option<WebStatus>,
}

/// Distraction shield rules and enforcement while a session runs.
pub struct ShieldService {
    handle: StoreHandle,
    procs: Arc<dyn ProcessControl>,
    web: WebBlocker,
    runtime: Mutex<Runtime>,
}

impl ShieldService {
    pub fn new(handle: StoreHandle) -> Self {
        Self::with_backends(handle, Arc::new(SystemProcesses::new()), Arc::new(FileHostsFile::system()))
    }

    pub fn with_backends(handle: StoreHandle, procs: Arc<dyn ProcessControl>, hosts: Arc<dyn HostsFile>) -> Self {
        Self { handle, procs, web: WebBlocker::new(hosts), runtime: Mutex::new(Runtime::default()) }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }

    fn rt(&self) -> std::sync::MutexGuard<'_, Runtime> {
        self.runtime.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ----- rules -----

    pub fn apps(&self) -> Vec<BlockedApp> {
        self.handle.read(|s| s.blocked_apps.clone())
    }

    pub fn sites(&self) -> Vec<BlockedSite> {
        self.handle.read(|s| s.blocked_sites.clone())
    }

    pub fn apps_enabled(&self) -> bool {
        self.handle.read(|s| s.preferences.app_shield_active)
    }

    pub fn sites_enabled(&self) -> bool {
        self.handle.read(|s| s.preferences.web_shield_active)
    }

    pub fn set_apps_enabled(&self, on: bool) {
        self.handle.mutate(|s| s.preferences.app_shield_active = on);
    }

    pub fn set_sites_enabled(&self, on: bool) {
        self.handle.mutate(|s| s.preferences.web_shield_active = on);
    }

    /// Adds an app rule. `process` may be given with or without `.exe`.
    pub fn add_app(&self, name: &str, process: &str) -> Result<BlockedApp, ShieldError> {
        let process = process.trim();
        if process.is_empty() {
            return Err(ShieldError::Empty);
        }
        if process.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
            return Err(ShieldError::InvalidApp);
        }
        if is_protected(process) {
            return Err(ShieldError::Protected(display_name(process)));
        }
        let stored = if process.contains('.') { process.to_string() } else { format!("{process}.exe") };
        let norm = normalize_process(&stored);
        let name = if name.trim().is_empty() { display_name(&stored) } else { name.trim().to_string() };
        self.handle.mutate(|s| {
            if s.blocked_apps.iter().any(|a| normalize_process(&a.process_name) == norm) {
                return Err(ShieldError::Duplicate);
            }
            let app = BlockedApp::new(name, stored);
            s.blocked_apps.push(app.clone());
            Ok(app)
        })
    }

    pub fn toggle_app(&self, id: &str) {
        self.handle.mutate(|s| {
            if let Some(a) = s.blocked_apps.iter_mut().find(|a| a.id == id) {
                a.toggle_active();
            }
        });
    }

    pub fn remove_app(&self, id: &str) {
        self.handle.mutate(|s| s.blocked_apps.retain(|a| a.id != id));
    }

    pub fn add_site(&self, input: &str) -> Result<BlockedSite, ShieldError> {
        let domain = normalize_domain(input)?;
        if domain == "localhost" || domain.ends_with(".localhost") {
            return Err(ShieldError::InvalidSite);
        }
        self.handle.mutate(|s| {
            if s.blocked_sites.iter().any(|x| x.domain == domain) {
                return Err(ShieldError::Duplicate);
            }
            let site = BlockedSite::new(domain);
            s.blocked_sites.push(site.clone());
            Ok(site)
        })
    }

    pub fn toggle_site(&self, id: &str) {
        self.handle.mutate(|s| {
            if let Some(x) = s.blocked_sites.iter_mut().find(|x| x.id == id) {
                x.toggle_active();
            }
        });
    }

    pub fn remove_site(&self, id: &str) {
        self.handle.mutate(|s| s.blocked_sites.retain(|x| x.id != id));
    }

    /// Distinct, sorted names of running programs that may be blocked.
    pub fn running_apps(&self) -> Vec<String> {
        let me = std::process::id();
        let mut seen = HashSet::new();
        let mut names: Vec<String> = self
            .procs
            .list()
            .into_iter()
            .filter(|p| p.pid != me && !is_protected(&p.name))
            .map(|p| p.name)
            .filter(|n| seen.insert(normalize_process(n)))
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        names
    }

    /// Whether website blocking can be used right now, as a screen message
    /// (empty when it can).
    pub fn web_availability_message(&self) -> String {
        if self.web.can_write() { String::new() } else { NEEDS_ADMIN_MESSAGE.to_string() }
    }

    // ----- enforcement -----

    /// Applies the shield for the current timer state. Call every second or
    /// two; app checks are throttled internally.
    pub fn sync(&self, snapshot: &TimerSnapshot, now: Instant) -> SyncReport {
        let (apps_on, sites_on, app_rules, domains) = self.handle.read(|s| {
            (
                s.preferences.app_shield_active,
                s.preferences.web_shield_active,
                s.blocked_apps.iter().filter(|a| a.active).map(|a| normalize_process(&a.process_name)).collect::<Vec<_>>(),
                s.blocked_sites.iter().filter(|x| x.active).map(|x| x.domain.clone()).collect::<Vec<_>>(),
            )
        });
        let mut closed = Vec::new();
        let mut rt = self.rt();

        if !is_focus_running(snapshot) {
            rt.close_requested.clear();
            rt.last_poll = None;
            rt.web_attempted = None;
            if rt.web_applied.is_some() {
                let st = self.web.remove();
                if !matches!(st, WebStatus::Failed(_) | WebStatus::NeedsAdmin) {
                    rt.web_applied = None;
                }
                rt.web_status = Some(st);
            } else if matches!(rt.web_status, Some(WebStatus::Active(_))) {
                rt.web_status = Some(WebStatus::Inactive);
            }
            return SyncReport { closed, web: rt.web_status.clone().unwrap_or(WebStatus::Inactive) };
        }

        if apps_on && !app_rules.is_empty() && rt.last_poll.map_or(true, |t| now.duration_since(t) >= POLL_INTERVAL) {
            rt.last_poll = Some(now);
            closed = self.enforce_apps(&mut rt, &app_rules, now);
        }

        let want: Vec<String> = if sites_on { domains } else { Vec::new() };
        if want.is_empty() {
            if rt.web_applied.is_some() {
                let st = self.web.remove();
                if !matches!(st, WebStatus::Failed(_) | WebStatus::NeedsAdmin) {
                    rt.web_applied = None;
                }
                rt.web_status = Some(st);
            }
        } else if rt.web_applied.as_ref() != Some(&want) && rt.web_attempted.as_ref() != Some(&want) {
            rt.web_attempted = Some(want.clone());
            let st = self.web.apply(&want);
            if matches!(st, WebStatus::Active(_)) {
                rt.web_applied = Some(want);
            }
            rt.web_status = Some(st);
        }
        SyncReport { closed, web: rt.web_status.clone().unwrap_or(WebStatus::Inactive) }
    }

    fn enforce_apps(&self, rt: &mut Runtime, rules: &[String], now: Instant) -> Vec<String> {
        let me = std::process::id();
        let procs = self.procs.list();
        let live: HashSet<u32> = procs.iter().map(|p| p.pid).collect();
        rt.close_requested.retain(|pid, _| live.contains(pid));
        let mut closed = Vec::new();
        for p in procs {
            if p.pid == me || is_protected(&p.name) || !rules.contains(&normalize_process(&p.name)) {
                continue;
            }
            match rt.close_requested.get(&p.pid) {
                None => {
                    if self.procs.close_graceful(p.pid) {
                        rt.close_requested.insert(p.pid, now);
                    } else {
                        self.procs.terminate(p.pid);
                    }
                    closed.push(display_name(&p.name));
                }
                Some(&asked) if now.duration_since(asked) >= CLOSE_GRACE => {
                    self.procs.terminate(p.pid);
                    rt.close_requested.remove(&p.pid);
                }
                Some(_) => {}
            }
        }
        closed.sort();
        closed.dedup();
        closed
    }

    /// Removes any leftover Thulli section, such as after a crash. Call at launch.
    pub fn cleanup_stale(&self) -> WebStatus {
        let st = self.web.remove();
        let mut rt = self.rt();
        rt.web_applied = None;
        rt.web_attempted = None;
        rt.web_status = None;
        st
    }

    /// Lifts website blocking now (used when the app closes).
    pub fn shutdown(&self) {
        let mut rt = self.rt();
        if rt.web_applied.take().is_some() {
            self.web.remove();
        }
    }
}

impl Drop for ShieldService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// `chrome.exe` -> `Chrome`.
pub fn display_name(process: &str) -> String {
    let base = process.trim();
    let base = base.strip_suffix(".exe").or_else(|| base.strip_suffix(".EXE")).unwrap_or(base);
    let mut chars = base.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AppStateStore;
    use crate::repository::{LoadReport, StorageRepository};
    use crate::services::timer::TimerMode;

    struct MemoryRepo;
    impl StorageRepository for MemoryRepo {
        fn load_with_report(&self) -> LoadReport {
            LoadReport { store: AppStateStore::default(), recovery: None }
        }
        fn save(&self, _: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockProcs {
        procs: Mutex<Vec<ProcInfo>>,
        graceful_ok: Mutex<bool>,
        asked: Mutex<Vec<u32>>,
        killed: Mutex<Vec<u32>>,
    }
    impl ProcessControl for MockProcs {
        fn list(&self) -> Vec<ProcInfo> {
            self.procs.lock().unwrap().clone()
        }
        fn close_graceful(&self, pid: u32) -> bool {
            self.asked.lock().unwrap().push(pid);
            *self.graceful_ok.lock().unwrap()
        }
        fn terminate(&self, pid: u32) -> bool {
            self.killed.lock().unwrap().push(pid);
            self.procs.lock().unwrap().retain(|p| p.pid != pid);
            true
        }
    }

    struct MockHosts {
        text: Mutex<String>,
        writable: bool,
        backups: Mutex<Vec<String>>,
        writes: Mutex<usize>,
    }
    impl MockHosts {
        fn new(text: &str, writable: bool) -> Arc<Self> {
            Arc::new(Self { text: Mutex::new(text.into()), writable, backups: Mutex::new(vec![]), writes: Mutex::new(0) })
        }
    }
    impl HostsFile for MockHosts {
        fn read(&self) -> io::Result<String> {
            Ok(self.text.lock().unwrap().clone())
        }
        fn write(&self, c: &str) -> io::Result<()> {
            if !self.writable {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            *self.text.lock().unwrap() = c.into();
            *self.writes.lock().unwrap() += 1;
            Ok(())
        }
        fn backup(&self, c: &str) -> io::Result<()> {
            self.backups.lock().unwrap().push(c.into());
            Ok(())
        }
        fn can_write(&self) -> bool {
            self.writable
        }
    }

    fn proc(pid: u32, name: &str) -> ProcInfo {
        ProcInfo { pid, name: name.into() }
    }

    fn snap(phase: TimerPhase, purpose: TimerPurpose) -> TimerSnapshot {
        TimerSnapshot {
            mode: TimerMode::Countdown,
            purpose,
            phase,
            planned_secs: 1500,
            estimate_secs: 0,
            elapsed_secs: 0,
            task_id: None,
        }
    }

    fn running() -> TimerSnapshot {
        snap(TimerPhase::Running, TimerPurpose::Session)
    }

    fn service(procs: Arc<MockProcs>, hosts: Arc<MockHosts>) -> ShieldService {
        let store = Arc::new(Mutex::new(AppStateStore::default()));
        let repo: Arc<dyn StorageRepository> = Arc::new(MemoryRepo);
        ShieldService::with_backends(StoreHandle::new(store, repo), procs, hosts)
    }

    const HOSTS: &str = "# my hosts\r\n127.0.0.1 localhost\r\n10.0.0.5 nas\r\n";

    #[test]
    fn section_round_trips_without_touching_other_lines() {
        let with = with_section(HOSTS, &["example.com".to_string()]);
        assert!(with.starts_with(HOSTS));
        assert!(with.contains("0.0.0.0 example.com\r\n"));
        assert!(with.contains("0.0.0.0 www.example.com"));
        assert_eq!(strip_section(&with), HOSTS);
        // Replacing is idempotent.
        assert_eq!(with_section(&with, &["example.com".to_string()]), with);
    }

    #[test]
    fn strip_handles_missing_end_marker_conservatively() {
        let text = format!("a\n{HOSTS_BEGIN}\n0.0.0.0 x.com\n:: x.com\nkeep me\n");
        assert_eq!(strip_section(&text), "a\nkeep me\n");
    }

    #[test]
    fn strip_without_section_is_identity() {
        assert_eq!(strip_section("no newline"), "no newline");
        assert_eq!(strip_section(HOSTS), HOSTS);
    }

    #[test]
    fn domains_are_normalised_and_validated() {
        assert_eq!(normalize_domain("https://www.Example.com/a?b=1").unwrap(), "example.com");
        assert_eq!(normalize_domain("news.site.org:8080").unwrap(), "news.site.org");
        assert_eq!(normalize_domain(""), Err(ShieldError::Empty));
        assert_eq!(normalize_domain("nodots"), Err(ShieldError::InvalidSite));
        assert_eq!(normalize_domain("bad host.com"), Err(ShieldError::InvalidSite));
        assert_eq!(normalize_domain("a..com"), Err(ShieldError::InvalidSite));
    }

    #[test]
    fn protected_programs_cannot_be_added() {
        let svc = service(Arc::new(MockProcs::default()), MockHosts::new("", true));
        for p in ["explorer.exe", "Thulli", "powershell.exe", "svchost", "cmd.exe"] {
            assert!(matches!(svc.add_app("", p), Err(ShieldError::Protected(_))), "{p}");
        }
        assert!(svc.apps().is_empty());
    }

    #[test]
    fn add_app_normalises_and_rejects_duplicates() {
        let svc = service(Arc::new(MockProcs::default()), MockHosts::new("", true));
        let a = svc.add_app("", "chrome").unwrap();
        assert_eq!(a.process_name, "chrome.exe");
        assert_eq!(a.name, "Chrome");
        assert_eq!(svc.add_app("Browser", "CHROME.EXE").unwrap_err(), ShieldError::Duplicate);
        assert_eq!(svc.add_app("", "a\\b.exe").unwrap_err(), ShieldError::InvalidApp);
        svc.toggle_app(&a.id);
        assert!(!svc.apps()[0].active);
        svc.remove_app(&a.id);
        assert!(svc.apps().is_empty());
    }

    #[test]
    fn add_site_rules() {
        let svc = service(Arc::new(MockProcs::default()), MockHosts::new("", true));
        svc.add_site("https://www.reddit.com/r/rust").unwrap();
        assert_eq!(svc.sites()[0].domain, "reddit.com");
        assert_eq!(svc.add_site("reddit.com").unwrap_err(), ShieldError::Duplicate);
        assert_eq!(svc.add_site("localhost").unwrap_err(), ShieldError::InvalidSite);
    }

    #[test]
    fn focus_running_only_for_work_sessions() {
        assert!(is_focus_running(&running()));
        assert!(is_focus_running(&snap(TimerPhase::Running, TimerPurpose::Sprint)));
        assert!(!is_focus_running(&snap(TimerPhase::Paused, TimerPurpose::Session)));
        assert!(!is_focus_running(&snap(TimerPhase::Running, TimerPurpose::Break)));
        assert!(!is_focus_running(&snap(TimerPhase::Idle, TimerPurpose::Session)));
    }

    #[test]
    fn apps_are_left_alone_when_no_session_or_disabled() {
        let procs = Arc::new(MockProcs::default());
        procs.procs.lock().unwrap().push(proc(10, "chrome.exe"));
        let svc = service(procs.clone(), MockHosts::new("", true));
        svc.add_app("", "chrome.exe").unwrap();
        svc.set_apps_enabled(true);
        let t = Instant::now();
        svc.sync(&snap(TimerPhase::Idle, TimerPurpose::Session), t);
        assert!(procs.asked.lock().unwrap().is_empty());
        svc.set_apps_enabled(false);
        svc.sync(&running(), t);
        assert!(procs.asked.lock().unwrap().is_empty());
    }

    #[test]
    fn graceful_close_then_terminate_after_grace() {
        let procs = Arc::new(MockProcs::default());
        *procs.graceful_ok.lock().unwrap() = true;
        procs.procs.lock().unwrap().extend([proc(10, "Chrome.exe"), proc(11, "notes.exe")]);
        let svc = service(procs.clone(), MockHosts::new("", true));
        svc.add_app("", "chrome").unwrap();
        svc.set_apps_enabled(true);

        let t0 = Instant::now();
        let r = svc.sync(&running(), t0);
        assert_eq!(r.closed, vec!["Chrome".to_string()]);
        assert_eq!(*procs.asked.lock().unwrap(), vec![10]);
        assert!(procs.killed.lock().unwrap().is_empty());

        // Within the poll interval nothing happens.
        svc.sync(&running(), t0 + Duration::from_secs(1));
        assert_eq!(procs.asked.lock().unwrap().len(), 1);

        // Still alive after the grace period: terminated.
        svc.sync(&running(), t0 + CLOSE_GRACE + POLL_INTERVAL);
        assert_eq!(*procs.killed.lock().unwrap(), vec![10]);
        // The unrelated app was never touched.
        assert!(!procs.asked.lock().unwrap().contains(&11));
    }

    #[test]
    fn no_window_means_immediate_terminate() {
        let procs = Arc::new(MockProcs::default());
        procs.procs.lock().unwrap().push(proc(10, "helper.exe"));
        let svc = service(procs.clone(), MockHosts::new("", true));
        svc.add_app("", "helper").unwrap();
        svc.set_apps_enabled(true);
        svc.sync(&running(), Instant::now());
        assert_eq!(*procs.killed.lock().unwrap(), vec![10]);
    }

    #[test]
    fn protected_process_is_never_closed_even_if_a_rule_exists() {
        let procs = Arc::new(MockProcs::default());
        procs.procs.lock().unwrap().push(proc(4, "explorer.exe"));
        let svc = service(procs.clone(), MockHosts::new("", true));
        // Bypass add_app validation, as a hand-edited data file could.
        svc.handle().mutate(|s| s.blocked_apps.push(BlockedApp::new("Explorer", "explorer.exe")));
        svc.set_apps_enabled(true);
        svc.sync(&running(), Instant::now());
        assert!(procs.asked.lock().unwrap().is_empty());
        assert!(procs.killed.lock().unwrap().is_empty());
    }

    #[test]
    fn running_apps_excludes_protected_and_duplicates() {
        let procs = Arc::new(MockProcs::default());
        procs.procs.lock().unwrap().extend([
            proc(1, "explorer.exe"),
            proc(2, "Chrome.exe"),
            proc(3, "chrome.exe"),
            proc(4, "Slack.exe"),
            proc(5, "pwsh.exe"),
        ]);
        let svc = service(procs, MockHosts::new("", true));
        assert_eq!(svc.running_apps(), vec!["Chrome.exe".to_string(), "Slack.exe".to_string()]);
    }

    #[test]
    fn hosts_added_during_session_and_removed_after() {
        let hosts = MockHosts::new(HOSTS, true);
        let svc = service(Arc::new(MockProcs::default()), hosts.clone());
        svc.add_site("example.com").unwrap();
        svc.set_sites_enabled(true);

        let r = svc.sync(&running(), Instant::now());
        assert_eq!(r.web, WebStatus::Active(1));
        assert!(hosts.text.lock().unwrap().contains("0.0.0.0 example.com"));
        assert_eq!(*hosts.backups.lock().unwrap(), vec![HOSTS.to_string()]);

        // Second sync does not rewrite.
        svc.sync(&running(), Instant::now());
        assert_eq!(*hosts.writes.lock().unwrap(), 1);

        // Pausing lifts the block and restores the file exactly.
        let r = svc.sync(&snap(TimerPhase::Paused, TimerPurpose::Session), Instant::now());
        assert_eq!(r.web, WebStatus::Inactive);
        assert_eq!(*hosts.text.lock().unwrap(), HOSTS);
    }

    #[test]
    fn hosts_follow_rule_changes_mid_session() {
        let hosts = MockHosts::new("", true);
        let svc = service(Arc::new(MockProcs::default()), hosts.clone());
        svc.add_site("a.com").unwrap();
        svc.set_sites_enabled(true);
        svc.sync(&running(), Instant::now());
        svc.add_site("b.com").unwrap();
        svc.sync(&running(), Instant::now());
        assert!(hosts.text.lock().unwrap().contains("0.0.0.0 b.com"));
        svc.set_sites_enabled(false);
        svc.sync(&running(), Instant::now());
        assert!(!has_section(&hosts.text.lock().unwrap()));
    }

    #[test]
    fn unwritable_hosts_reports_admin_message_and_apps_still_work() {
        let hosts = MockHosts::new(HOSTS, false);
        let procs = Arc::new(MockProcs::default());
        procs.procs.lock().unwrap().push(proc(10, "game.exe"));
        let svc = service(procs.clone(), hosts.clone());
        svc.add_site("example.com").unwrap();
        svc.add_app("", "game").unwrap();
        svc.set_sites_enabled(true);
        svc.set_apps_enabled(true);

        let r = svc.sync(&running(), Instant::now());
        assert_eq!(r.web, WebStatus::NeedsAdmin);
        assert!(r.web.message().contains("administrator"));
        assert_eq!(r.closed, vec!["Game".to_string()]);
        assert_eq!(*hosts.text.lock().unwrap(), HOSTS);
        assert!(!svc.web_availability_message().is_empty());
        // No retry storm: the write is not attempted again for the same rules.
        svc.sync(&running(), Instant::now());
        assert_eq!(*hosts.writes.lock().unwrap(), 0);
    }

    #[test]
    fn stale_section_is_cleaned_at_startup() {
        let stale = with_section(HOSTS, &["example.com".to_string()]);
        let hosts = MockHosts::new(&stale, true);
        let svc = service(Arc::new(MockProcs::default()), hosts.clone());
        assert_eq!(svc.cleanup_stale(), WebStatus::Inactive);
        assert_eq!(*hosts.text.lock().unwrap(), HOSTS);
        // Nothing to clean: no write.
        let before = *hosts.writes.lock().unwrap();
        svc.cleanup_stale();
        assert_eq!(*hosts.writes.lock().unwrap(), before);
    }

    #[test]
    fn dropping_the_service_lifts_an_active_block() {
        let hosts = MockHosts::new(HOSTS, true);
        {
            let svc = service(Arc::new(MockProcs::default()), hosts.clone());
            svc.add_site("example.com").unwrap();
            svc.set_sites_enabled(true);
            svc.sync(&running(), Instant::now());
            assert!(has_section(&hosts.text.lock().unwrap()));
        }
        assert_eq!(*hosts.text.lock().unwrap(), HOSTS);
    }

    #[test]
    fn file_hosts_round_trip_uses_a_temp_file_only() {
        let dir = std::env::temp_dir().join(format!("thulli-shield-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hosts");
        std::fs::write(&path, HOSTS).unwrap();
        let blocker = WebBlocker::new(Arc::new(FileHostsFile::new(&path)));

        assert_eq!(blocker.apply(&["example.com".to_string()]), WebStatus::Active(1));
        assert!(std::fs::read_to_string(&path).unwrap().contains(HOSTS_BEGIN));
        assert_eq!(std::fs::read_to_string(dir.join("hosts.thulli-backup")).unwrap(), HOSTS);
        assert_eq!(blocker.remove(), WebStatus::Inactive);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), HOSTS);
        std::fs::remove_dir_all(&dir).ok();
    }
}
