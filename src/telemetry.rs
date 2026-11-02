//! Dev-phase observability for Thulli: FPS, process CPU/RAM, system RAM and GPU
//! utilisation are sampled periodically into `logs/telemetry.log`, and a panic
//! hook writes a dedicated `logs/crash-<timestamp>.log` (message plus Rust
//! backtrace) while also marking the crash in the telemetry log. Telemetry is on
//! by default during development; set `THULLI_TELEMETRY=0` to disable log writes
//! (the crash file itself is still produced so failures are never lost). Every
//! path in this module is best-effort: file and FFI failures are swallowed so
//! observability can never panic or block the UI thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const TELEMETRY_FILE: &str = "logs/telemetry.log";

/// True unless env `THULLI_TELEMETRY` is set to `"0"` (default ON — we are in dev).
pub fn enabled() -> bool {
    std::env::var("THULLI_TELEMETRY")
        .map(|v| v != "0")
        .unwrap_or(true)
}

fn timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Append one line to `logs/telemetry.log`. Silent no-op on any failure or when
/// telemetry is disabled via `THULLI_TELEMETRY=0`.
fn append_line(line: &str) {
    if !enabled() {
        return;
    }
    let _ = std::fs::create_dir_all("logs");
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(TELEMETRY_FILE)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Append a free-form event line to `logs/telemetry.log` (used for start/stop markers).
pub fn log_event(msg: &str) {
    append_line(&format!("{} event {}", timestamp(), msg));
}

/// Installs a panic hook: writes `logs/crash-<timestamp>.log` (message +
/// `std::backtrace`) AND appends a line to `logs/telemetry.log`, then chains
/// the original hook.
pub fn install_crash_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            write_crash_report(info);
        }));
        original(info);
    }));
}

fn write_crash_report(info: &std::panic::PanicHookInfo<'_>) {
    let payload = info.payload();
    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    };
    let location = match info.location() {
        Some(l) => format!("{}:{}", l.file(), l.line()),
        None => "unknown location".to_string(),
    };
    let backtrace = std::backtrace::Backtrace::force_capture();

    let now = chrono::Local::now();
    let file_stamp = now.format("%Y%m%d-%H%M%S-%3f");
    let contents = format!(
        "Thulli crash report\ntime: {}\nmessage: {}\nat: {}\n\nbacktrace:\n{}\n",
        now.to_rfc3339(),
        msg,
        location,
        backtrace
    );
    let _ = std::fs::create_dir_all("logs");
    let _ = std::fs::write(format!("logs/crash-{file_stamp}.log"), contents);

    append_line(&format!(
        "{} crash message=\"{}\" at {}",
        timestamp(),
        msg.replace('\n', " "),
        location
    ));
}

/// Frame counter fed by Slint's rendering notifier.
#[derive(Default)]
pub struct FpsCounter {
    /// Shared so the `'static` rendering-notifier closure can increment it.
    frames: Arc<AtomicU64>,
}

impl FpsCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach to `slint::Window` via `window.set_rendering_notifier(...)`,
    /// incrementing on `slint::RenderingState::AfterRendering` (other states
    /// are ignored). Returns the notifier error type directly.
    pub fn attach(&self, window: &slint::Window) -> Result<(), slint::SetRenderingNotifierError> {
        let frames = Arc::clone(&self.frames);
        window.set_rendering_notifier(move |state, _graphics| {
            if matches!(state, slint::RenderingState::AfterRendering) {
                frames.fetch_add(1, Ordering::Relaxed);
            }
        })
    }

    /// Atomically swap the counter to 0 and return `fps = frames / elapsed_secs`.
    pub fn take_fps(&self, elapsed_secs: f32) -> f32 {
        let frames = self.frames.swap(0, Ordering::Relaxed);
        if elapsed_secs > 0.0 {
            frames as f32 / elapsed_secs
        } else {
            0.0
        }
    }
}

/// Periodic sampler: process CPU%, process RAM, system RAM%, GPU util%.
pub struct Sampler {
    system: sysinfo::System,
    pid: sysinfo::Pid,
    #[cfg(target_os = "windows")]
    gpu: Option<GpuProbe>,
}

impl Sampler {
    pub fn new() -> Self {
        let pid = sysinfo::Pid::from_u32(std::process::id());
        let mut system = sysinfo::System::new();
        // Prime one baseline sample so the first log() has a delta to diff against.
        system.refresh_memory();
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(std::slice::from_ref(&pid)), false);
        #[cfg(target_os = "windows")]
        let gpu = GpuProbe::new();
        Self {
            system,
            pid,
            #[cfg(target_os = "windows")]
            gpu,
        }
    }

    /// Sample now and append one line to `logs/telemetry.log`, e.g.
    /// `2026-11-02T14:33:05Z fps=59.8 cpu=3.2% ram=96MB sys_ram=41.1% gpu=1.4%`
    /// Returns the line as String too (for optional console echo in dev).
    pub fn log(&mut self, fps: f32) -> String {
        self.system.refresh_memory();
        self.system.refresh_processes(
            sysinfo::ProcessesToUpdate::Some(std::slice::from_ref(&self.pid)),
            false,
        );

        let (cpu_pct, ram_mb) = match self.system.process(self.pid) {
            Some(p) => (p.cpu_usage(), p.memory() / (1024 * 1024)),
            None => (0.0, 0),
        };

        let total = self.system.total_memory();
        let available = self.system.available_memory();
        let sys_ram_pct = if total > 0 {
            (total.saturating_sub(available) as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        let gpu_str = match self.gpu_sample() {
            Some(v) => format!("{v:.1}%"),
            None => "n/a".to_string(),
        };

        let line = format!(
            "{} fps={fps:.1} cpu={cpu_pct:.1}% ram={ram_mb}MB sys_ram={sys_ram_pct:.1}% gpu={gpu_str}",
            timestamp()
        );
        append_line(&line);
        line
    }

    /// GPU utilisation sample from the Windows PDH counter, or `None` when the
    /// platform has no probe so the log line shows `gpu=n/a`.
    #[cfg(target_os = "windows")]
    fn gpu_sample(&mut self) -> Option<f64> {
        self.gpu.as_mut()?.sample()
    }

    #[cfg(not(target_os = "windows"))]
    fn gpu_sample(&mut self) -> Option<f64> {
        None
    }
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

/// Windows GPU utilisation probe over the PDH `\GPU Engine(*)\Utilization
/// Percentage` counter, filtered to instances belonging to our process id.
/// The query stays open across samples so rate counters have a time baseline;
/// handles are stored as `usize` so `Sampler` stays simple to hold.
#[cfg(target_os = "windows")]
struct GpuProbe {
    query: usize,
    counter: usize,
    pid_token: String,
    buffer: Vec<windows_sys::Win32::System::Performance::PDH_FMT_COUNTERVALUE_ITEM_W>,
}

#[cfg(target_os = "windows")]
impl GpuProbe {
    fn new() -> Option<Self> {
        use windows_sys::Win32::System::Performance::{
            PdhAddEnglishCounterW, PdhCloseQuery, PdhOpenQueryW, PDH_HCOUNTER, PDH_HQUERY,
        };
        unsafe {
            let mut query: PDH_HQUERY = std::ptr::null_mut();
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                return None;
            }
            let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage\0".encode_utf16().collect();
            let mut counter: PDH_HCOUNTER = std::ptr::null_mut();
            if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) != 0 {
                let _ = PdhCloseQuery(query);
                return None;
            }
            Some(Self {
                query: query as usize,
                counter: counter as usize,
                pid_token: format!("pid_{}_", std::process::id()),
                buffer: Vec::new(),
            })
        }
    }

    /// Collect once and sum the double values of our process' GPU engine
    /// instances, clamped to 0..100. Returns `None` on any PDH failure.
    fn sample(&mut self) -> Option<f64> {
        use windows_sys::Win32::System::Performance::{
            PDH_CSTATUS_NEW_DATA, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
            PdhCollectQueryData, PdhGetFormattedCounterArrayW, PDH_FMT_COUNTERVALUE_ITEM_W,
        };
        unsafe {
            let query = self.query as PDH_HQUERY;
            let counter = self.counter as PDH_HCOUNTER;

            let status = PdhCollectQueryData(query);
            if status != 0 && status != PDH_CSTATUS_NEW_DATA {
                return None;
            }

            // Size the array: a call with no buffer returns PDH_MORE_DATA and
            // the required byte count (0 when there are no instances yet).
            let mut size_bytes: u32 = 0;
            let mut item_count: u32 = 0;
            let status = PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size_bytes,
                &mut item_count,
                std::ptr::null_mut(),
            );
            let _ = item_count; // advisory only; the full read below re-queries it
            if status != PDH_MORE_DATA && status != 0 {
                return None;
            }
            if size_bytes == 0 {
                // No GPU engine instances yet (e.g. nothing has rendered).
                return Some(0.0);
            }

            let item_size = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            let capacity = (size_bytes as usize).div_ceil(item_size).max(1);
            self.buffer
                .resize(capacity, Default::default());

            // Retry a bounded number of times in case instances appear between
            // the sizing call and the read.
            for _ in 0..3 {
                let mut write_bytes = (self.buffer.len() * item_size) as u32;
                let mut write_count: u32 = 0;
                let status = PdhGetFormattedCounterArrayW(
                    counter,
                    PDH_FMT_DOUBLE,
                    &mut write_bytes,
                    &mut write_count,
                    self.buffer.as_mut_ptr(),
                );
                if status == PDH_MORE_DATA {
                    let needed = (write_bytes as usize).div_ceil(item_size).max(1);
                    self.buffer.resize(needed, Default::default());
                    continue;
                }
                if status != 0 {
                    return None;
                }
                let mut total = 0.0f64;
                let usable = (write_count as usize).min(self.buffer.len());
                for item in &self.buffer[..usable] {
                    let valid =
                        item.FmtValue.CStatus == 0 || item.FmtValue.CStatus == PDH_CSTATUS_NEW_DATA;
                    if !valid {
                        continue;
                    }
                    let name = wide_to_string(item.szName);
                    if name.contains(&self.pid_token) {
                        total += item.FmtValue.Anonymous.doubleValue;
                    }
                }
                return Some(total.clamp(0.0, 100.0));
            }
            None
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for GpuProbe {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Performance::{PdhCloseQuery, PDH_HQUERY};
        unsafe {
            let _ = PdhCloseQuery(self.query as PDH_HQUERY);
        }
    }
}

/// Convert a null-terminated wide string (PDH instance name) to `String`.
/// Bounded so a corrupt pointer cannot run away.
#[cfg(target_os = "windows")]
unsafe fn wide_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while len < 1024 && unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) })
}
