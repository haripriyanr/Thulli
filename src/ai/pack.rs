//! The on-device model pack: one zip on Hugging Face holding the planner
//! (`llm/`), speech recognition (`asr/`) and speech (`tts/`) models.
//!
//! It is fetched the first time any model is needed, checked against the
//! SHA-256 that Hugging Face reports for the file, unpacked next to the app's
//! data and the zip is deleted. Downloads resume after an interruption.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

/// Repository and file of the model pack.
pub const PACK_REPO: &str = "konradjr007/thulli-models";
pub const PACK_FILE: &str = "thulli-models.zip";
/// Written after a successful unpack; its presence means the pack is ready.
const READY_MARKER: &str = "pack-ready.txt";

/// Progress of preparing the pack, for the status line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PackProgress {
    Downloading { done: u64, total: u64 },
    Checking,
    Unpacking { done: usize, total: usize },
}

impl PackProgress {
    /// Short wording for the status line.
    pub fn describe(&self) -> String {
        match self {
            PackProgress::Downloading { done, total } if *total > 0 => format!(
                "Downloading AI models: {:.0}% of {:.1} GB",
                *done as f64 * 100.0 / *total as f64,
                *total as f64 / 1e9
            ),
            PackProgress::Downloading { .. } => "Downloading AI models".to_string(),
            PackProgress::Checking => "Checking the download".to_string(),
            PackProgress::Unpacking { done, total } => format!("Unpacking AI models ({done}/{total})"),
        }
    }
}

/// Folder holding the unpacked models, in the per-user data area of each OS:
/// `%LOCALAPPDATA%\thulli\models` on Windows,
/// `~/Library/Application Support/thulli/models` on macOS, and
/// `$XDG_DATA_HOME/thulli/models` (or `~/.local/share/...`) elsewhere.
pub fn models_dir() -> PathBuf {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(target_os = "windows") {
        var("LOCALAPPDATA").or_else(|| var("APPDATA"))
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local").join("share")))
    };
    // Never the temp folder: the system may clear it and lose 2 GB of models.
    base.unwrap_or_else(|| PathBuf::from(".")).join("thulli").join("models")
}

/// True when the pack is unpacked and ready to load.
pub fn is_ready() -> bool {
    models_dir().join(READY_MARKER).is_file()
}

/// Only one download may run at a time.
static PREPARING: Mutex<()> = Mutex::new(());

/// Makes sure the pack is unpacked, downloading it when needed. Blocks for
/// the whole download, so call it from a worker thread.
pub fn ensure(on_progress: &mut dyn FnMut(PackProgress)) -> Result<PathBuf, String> {
    let _guard = PREPARING.lock().unwrap_or_else(|e| e.into_inner());
    let dir = models_dir();
    if is_ready() {
        return Ok(dir);
    }
    fs::create_dir_all(&dir).map_err(|e| format!("could not create the models folder: {e}"))?;
    let (size, sha) = remote_info()?;
    let zip_path = dir.join(PACK_FILE);
    download(&zip_path, size, on_progress)?;
    on_progress(PackProgress::Checking);
    let got = sha256_file(&zip_path).map_err(|e| format!("could not read the download: {e}"))?;
    if !got.eq_ignore_ascii_case(&sha) {
        let _ = fs::remove_file(&zip_path);
        return Err("the download was damaged; it will start again next time".into());
    }
    unpack(&zip_path, &dir, on_progress)?;
    let _ = fs::remove_file(&zip_path);
    fs::write(dir.join(READY_MARKER), sha).map_err(|e| format!("could not finish setup: {e}"))?;
    Ok(dir)
}

/// Removes every unpacked model, freeing the disk space.
pub fn remove() -> Result<(), String> {
    let _guard = PREPARING.lock().unwrap_or_else(|e| e.into_inner());
    let dir = models_dir();
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| format!("could not remove the models: {e}"))?;
    }
    Ok(())
}

fn file_url() -> String {
    format!("https://huggingface.co/{PACK_REPO}/resolve/main/{PACK_FILE}")
}

/// Size and SHA-256 of the pack, as Hugging Face records them.
fn remote_info() -> Result<(u64, String), String> {
    let url = format!("https://huggingface.co/api/models/{PACK_REPO}/tree/main");
    let list: serde_json::Value = ureq::get(&url)
        .call()
        .map_err(|e| format!("could not reach the model server: {e}"))?
        .into_json()
        .map_err(|e| format!("unexpected reply from the model server: {e}"))?;
    let entry = list
        .as_array()
        .and_then(|a| a.iter().find(|f| f["path"] == PACK_FILE))
        .ok_or("the model pack is not published yet")?;
    let size = entry["lfs"]["size"].as_u64().or_else(|| entry["size"].as_u64()).ok_or("the model pack has no size")?;
    let sha = entry["lfs"]["oid"].as_str().ok_or("the model pack has no checksum")?.to_string();
    Ok((size, sha))
}

/// Downloads to `<zip>.part`, resuming from what is already there.
fn download(zip_path: &Path, total: u64, on_progress: &mut dyn FnMut(PackProgress)) -> Result<(), String> {
    if zip_path.metadata().map(|m| m.len() == total).unwrap_or(false) {
        return Ok(());
    }
    let part = zip_path.with_extension("zip.part");
    let mut have = part.metadata().map(|m| m.len()).unwrap_or(0);
    if have > total {
        let _ = fs::remove_file(&part);
        have = 0;
    }
    if have < total {
        let mut req = ureq::get(&file_url());
        if have > 0 {
            req = req.set("Range", &format!("bytes={have}-"));
        }
        let resp = req.call().map_err(|e| format!("the download failed: {e}"))?;
        if have > 0 && resp.status() != 206 {
            have = 0; // the server sent the whole file
        }
        let mut out = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(have == 0)
            .open(&part)
            .map_err(|e| format!("could not save the download: {e}"))?;
        out.seek(SeekFrom::Start(have)).map_err(|e| e.to_string())?;
        let mut reader = resp.into_reader();
        let mut buf = vec![0u8; 1 << 20];
        let mut last = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("the download stopped: {e}"))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| format!("could not save the download: {e}"))?;
            have += n as u64;
            if have - last >= 8 << 20 || have == total {
                last = have;
                on_progress(PackProgress::Downloading { done: have, total });
            }
        }
        out.flush().map_err(|e| e.to_string())?;
    }
    if have != total {
        return Err("the download ended early; it will resume next time".into());
    }
    fs::rename(&part, zip_path).map_err(|e| format!("could not finish the download: {e}"))
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Unpacks every entry, refusing paths that would land outside `dir`.
fn unpack(zip_path: &Path, dir: &Path, on_progress: &mut dyn FnMut(PackProgress)) -> Result<(), String> {
    let file = File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("the model pack is not a valid zip: {e}"))?;
    let total = archive.len();
    for i in 0..total {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else { continue };
        let target = dir.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = File::create(&target).map_err(|e| format!("could not unpack: {e}"))?;
            io::copy(&mut entry, &mut out).map_err(|e| format!("could not unpack: {e}"))?;
        }
        on_progress(PackProgress::Unpacking { done: i + 1, total });
    }
    Ok(())
}
