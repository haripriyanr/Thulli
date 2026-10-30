//! Repository pattern abstractions and encrypted persistence implementations.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;

use crate::models::AppStateStore;
use crate::security::SecurityManager;

/// Reported when stored data could not be read and a fresh start was needed.
#[derive(Debug, Clone)]
pub struct RecoveryNotice {
    /// Technical cause, for logs.
    pub reason: String,
    /// Where the unreadable file was kept, when it could be kept.
    pub backup_path: Option<PathBuf>,
}

impl RecoveryNotice {
    /// Plain-language message suitable for showing to the user.
    pub fn user_message(&self) -> String {
        match &self.backup_path {
            Some(path) => format!(
                "Your saved data could not be opened, so Thulli started fresh. The original file was kept at {}.",
                path.display()
            ),
            None => "Your saved data could not be opened, and a copy could not be made. Thulli started fresh without changing the file on disk.".to_string(),
        }
    }
}

/// Result of loading: the state plus a notice when recovery was needed.
#[derive(Debug, Clone)]
pub struct LoadReport {
    pub store: AppStateStore,
    pub recovery: Option<RecoveryNotice>,
}

/// Abstract repository interface for persisting application state.
pub trait StorageRepository: Send + Sync {
    /// Loads the stored state, reporting any recovery that was needed.
    fn load_with_report(&self) -> LoadReport;

    /// Loads the stored application state or initializes defaults if not present.
    fn load(&self) -> AppStateStore {
        self.load_with_report().store
    }

    /// Persists the application state securely.
    fn save(&self, store: &AppStateStore) -> Result<(), Box<dyn std::error::Error>>;
}

/// Writes `bytes` to `path` atomically: temp file in the same folder, flushed,
/// then renamed over the destination.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut tmp_name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp_path = path.with_file_name(tmp_name);

    let result = (|| {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

/// Encrypted local-first repository implementing authenticated AES-256-GCM storage.
pub struct EncryptedStorageRepository {
    security: Arc<SecurityManager>,
    file_path: PathBuf,
}

impl EncryptedStorageRepository {
    /// Factory constructor discovering %APPDATA%/thulli/thulli_data.enc
    pub fn new_default(security: Arc<SecurityManager>) -> Result<Self, Box<dyn std::error::Error>> {
        let app_data = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        let mut dir = PathBuf::from(app_data);
        dir.push("thulli");
        fs::create_dir_all(&dir)?;

        let mut file_path = dir;
        file_path.push("thulli_data.enc");

        Ok(Self { security, file_path })
    }

    /// Custom path constructor (useful for testing and isolated sandboxes)
    pub fn with_custom_path(security: Arc<SecurityManager>, path: impl AsRef<Path>) -> Self {
        Self { security, file_path: path.as_ref().to_path_buf() }
    }

    /// Location of the data file.
    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    fn read_store(&self) -> Result<AppStateStore, String> {
        let bytes = self.security.read_decrypted_file(&self.file_path).map_err(|e| format!("could not decrypt: {e}"))?;
        AppStateStore::from_json(&bytes).map_err(|e| format!("could not parse: {e}"))
    }

    /// Keeps the unreadable file as `<name>.<timestamp>.bak`. Returns its new path.
    fn keep_unreadable_copy(&self) -> Option<PathBuf> {
        let mut name = self.file_path.file_name()?.to_os_string();
        name.push(format!(".{}.bak", Utc::now().format("%Y%m%dT%H%M%S%3f")));
        let backup = self.file_path.with_file_name(name);
        // Rename keeps the original bytes untouched; copy is the fallback.
        if fs::rename(&self.file_path, &backup).is_ok() || fs::copy(&self.file_path, &backup).is_ok() {
            Some(backup)
        } else {
            None
        }
    }
}

impl StorageRepository for EncryptedStorageRepository {
    fn load_with_report(&self) -> LoadReport {
        let mut recovery = None;

        if self.file_path.exists() {
            match self.read_store() {
                Ok(store) => {
                    println!("[Storage] Loaded data ({} lists, {} tasks)", store.lists.len(), store.tasks.len());
                    return LoadReport { store, recovery: None };
                }
                Err(reason) => {
                    eprintln!("[Storage] {reason}");
                    let backup_path = self.keep_unreadable_copy();
                    match &backup_path {
                        Some(p) => eprintln!("[Storage] Kept unreadable file at {}", p.display()),
                        None => eprintln!("[Storage] Could not keep a copy of the unreadable file"),
                    }
                    recovery = Some(RecoveryNotice { reason, backup_path });
                }
            }
        }

        let default_store = AppStateStore::default();
        // If the unreadable file could not be preserved, leave it on disk untouched.
        let safe_to_write = recovery.as_ref().is_none_or(|r| r.backup_path.is_some());
        if safe_to_write {
            if let Err(e) = self.save(&default_store) {
                eprintln!("[Storage] Could not write initial data: {e}");
            }
        }
        LoadReport { store: default_store, recovery }
    }

    fn save(&self, store: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
        let serialized = serde_json::to_vec_pretty(store)?;
        let encrypted = self.security.encrypt(&serialized)?;
        write_atomic(&self.file_path, &encrypted)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Task, TaskSection, TaskStatus, CURRENT_SCHEMA_VERSION};

    fn temp_vault(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("thulli_test_{tag}_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test_vault.enc");
        (dir, file)
    }

    #[test]
    fn test_default_store_has_personal_list() {
        let store = AppStateStore::default();
        assert_eq!(store.lists.len(), 1);
        assert_eq!(store.lists[0].name, "Personal");
        assert!(!store.lists[0].id.is_empty());
    }

    #[test]
    fn test_encrypted_round_trip() {
        let sec = Arc::new(SecurityManager::init().unwrap());
        let (dir, file) = temp_vault("roundtrip");
        let repo = EncryptedStorageRepository::with_custom_path(sec, &file);

        let mut store = repo.load();
        let list_id = store.lists[0].id.clone();
        store.add_task(Task::new(list_id, "Ship Thulli Modular MVP", 45, TaskSection::Today));
        repo.save(&store).unwrap();

        let loaded = repo.load();
        assert_eq!(loaded.lists.len(), 1);
        assert_eq!(loaded.tasks.len(), 1);
        assert_eq!(loaded.tasks[0].title, "Ship Thulli Modular MVP");
        assert_eq!(loaded.tasks[0].estimate_mins, 45);
        assert_eq!(loaded.schema_version, CURRENT_SCHEMA_VERSION);

        // Atomic write leaves no temp file behind.
        assert!(!dir.join("test_vault.enc.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// JSON exactly as the first builds wrote it: no schema_version, `board`
    /// and `completed` on tasks, the old preferences block.
    const OLD_FORMAT_JSON: &str = r##"{
        "lists": [{"id":"l1","name":"Personal","color":"#6FDBAB","is_archived":false,"sort_order":0,"created_at":"2026-01-01T00:00:00Z"}],
        "tasks": [
            {"id":"t1","list_id":"l1","title":"Today task","estimate_mins":25,"time_taken_secs":120,"completed":false,"completed_at":null,"board":"today","sort_order":0,"created_at":"2026-01-01T00:00:00Z"},
            {"id":"t2","list_id":"l1","title":"Week task","estimate_mins":30,"time_taken_secs":0,"completed":false,"completed_at":null,"board":"week","sort_order":1,"created_at":"2026-01-01T00:00:00Z"},
            {"id":"t3","list_id":"l1","title":"Backlog task","estimate_mins":45,"time_taken_secs":0,"completed":false,"completed_at":null,"board":"backlog","sort_order":2,"created_at":"2026-01-01T00:00:00Z"},
            {"id":"t4","list_id":"l1","title":"Finished task","estimate_mins":10,"time_taken_secs":600,"completed":true,"completed_at":"2026-01-02T10:00:00Z","board":"done","sort_order":3,"created_at":"2026-01-01T00:00:00Z"}
        ],
        "blocked_apps": [{"id":"b1","name":"Chat","process_name":"chat.exe","active":true}],
        "preferences": {"timezone":"(GMT+05:30) Asia/Calcutta","sprint_enabled":true,"work_sprint_mins":50,"break_time_mins":15,"default_break_mins":10,"scrolling_title":true,"timed_alerts":true,"alert_interval_mins":30,"sound_id":"basic-ding","app_shield_active":true,"web_shield_active":false,"quick_notes":"my notes","active_mode":"dashboard"}
    }"##;

    #[test]
    fn test_old_format_json_migrates() {
        let store = AppStateStore::from_json(OLD_FORMAT_JSON.as_bytes()).unwrap();
        assert_eq!(store.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(store.tasks.len(), 4);

        assert_eq!(store.tasks[0].section, TaskSection::Today);
        assert_eq!(store.tasks[1].section, TaskSection::ThisWeek);
        assert_eq!(store.tasks[2].section, TaskSection::Backlog);
        assert!(store.tasks.iter().take(3).all(|t| t.status == TaskStatus::Open));
        assert_eq!(store.tasks[3].status, TaskStatus::Completed);
        assert!(store.tasks[3].completed_at.is_some());
        assert_eq!(store.tasks[0].time_taken_secs, 120);

        // Preferences keep user values, gain defaults, and drop the hardcoded timezone.
        let p = &store.preferences;
        assert!(p.sprint_enabled && p.app_shield_active);
        assert_eq!(p.work_sprint_mins, 50);
        assert_eq!(p.alert_sound_id, "basic-ding");
        assert_eq!(p.quick_notes, "my notes");
        assert!(p.timezone.is_empty());
        assert_eq!(p.default_session_mins, 90);
        assert!(p.auto_parse_estimates);

        assert_eq!(store.blocked_apps.len(), 1);
        assert!(store.notes.is_empty() && store.blocked_sites.is_empty());

        // Re-saving writes the new layout only, and it round-trips.
        let json = serde_json::to_string(&store).unwrap();
        assert!(!json.contains("\"board\""));
        let again = AppStateStore::from_json(json.as_bytes()).unwrap();
        assert_eq!(again.tasks[3].status, TaskStatus::Completed);
        assert_eq!(again.tasks[1].section, TaskSection::ThisWeek);
    }

    #[test]
    fn test_old_format_file_loads_through_repository() {
        let sec = Arc::new(SecurityManager::init().unwrap());
        let (dir, file) = temp_vault("oldfile");
        let encrypted = sec.encrypt(OLD_FORMAT_JSON.as_bytes()).unwrap();
        fs::write(&file, encrypted).unwrap();

        let repo = EncryptedStorageRepository::with_custom_path(sec, &file);
        let report = repo.load_with_report();
        assert!(report.recovery.is_none());
        assert_eq!(report.store.tasks.len(), 4);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_unreadable_file_is_backed_up_not_overwritten() {
        let sec = Arc::new(SecurityManager::init().unwrap());
        let (dir, file) = temp_vault("backup");
        let garbage = b"definitely not an encrypted vault".to_vec();
        fs::write(&file, &garbage).unwrap();

        let repo = EncryptedStorageRepository::with_custom_path(sec, &file);
        let report = repo.load_with_report();

        let notice = report.recovery.expect("failure must be reported");
        assert!(!notice.user_message().is_empty());
        let backup = notice.backup_path.expect("backup must exist");
        assert!(backup.to_string_lossy().ends_with(".bak"));
        assert_eq!(fs::read(&backup).unwrap(), garbage, "backup keeps the original bytes");

        // Fresh state was written, and it now loads cleanly.
        assert_eq!(report.store.lists.len(), 1);
        let second = repo.load_with_report();
        assert!(second.recovery.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_valid_encryption_but_bad_json_is_backed_up() {
        let sec = Arc::new(SecurityManager::init().unwrap());
        let (dir, file) = temp_vault("badjson");
        fs::write(&file, sec.encrypt(b"{ not json").unwrap()).unwrap();

        let repo = EncryptedStorageRepository::with_custom_path(sec, &file);
        let report = repo.load_with_report();
        assert!(report.recovery.is_some_and(|r| r.backup_path.is_some()));
        let baks = fs::read_dir(&dir).unwrap().filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().ends_with(".bak")).count();
        assert_eq!(baks, 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
