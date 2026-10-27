use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::security::SecurityManager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub estimate_mins: u32,
    pub time_taken_secs: u64,
    pub completed: bool,
    pub completed_at: Option<DateTime<Utc>>,
    pub board: String, // "today", "week", "backlog", "done"
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskList {
    pub id: String,
    pub name: String,
    pub color: String,
    pub is_archived: bool,
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedApp {
    pub id: String,
    pub name: String,
    pub process_name: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThulliPreferences {
    pub timezone: String,
    pub pomodoro_enabled: bool,
    pub work_sprint_mins: u32,
    pub break_time_mins: u32,
    pub default_break_mins: u32,
    pub scrolling_title: bool,
    pub timed_alerts: bool,
    pub alert_interval_mins: u32,
    pub sound_id: String,
    pub app_blocker_active: bool,
    pub web_blocker_active: bool,
    pub quick_notes: String,
    pub active_mode: String,
}

impl Default for ThulliPreferences {
    fn default() -> Self {
        Self {
            timezone: "(GMT+05:30) Asia/Calcutta".to_string(),
            pomodoro_enabled: false,
            work_sprint_mins: 60,
            break_time_mins: 15,
            default_break_mins: 10,
            scrolling_title: true,
            timed_alerts: true,
            alert_interval_mins: 30,
            sound_id: "basic-ding".to_string(),
            app_blocker_active: false,
            web_blocker_active: false,
            quick_notes: "⚡ Thulli Scratchpad\n- Keep focus ideas here without leaving your flow.".to_string(),
            active_mode: "dashboard".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStateStore {
    pub lists: Vec<TaskList>,
    pub tasks: Vec<Task>,
    pub blocked_apps: Vec<BlockedApp>,
    pub preferences: ThulliPreferences,
}

impl Default for AppStateStore {
    fn default() -> Self {
        let personal_list_id = Uuid::new_v4().to_string();
        Self {
            lists: vec![
                TaskList {
                    id: personal_list_id.clone(),
                    name: "Personal".to_string(),
                    color: "#3CC9C4".to_string(),
                    is_archived: false,
                    sort_order: 0,
                    created_at: Utc::now(),
                },
                TaskList {
                    id: Uuid::new_v4().to_string(),
                    name: "Work Sprint".to_string(),
                    color: "#EF82EF".to_string(),
                    is_archived: false,
                    sort_order: 1,
                    created_at: Utc::now(),
                },
            ],
            tasks: vec![
                Task {
                    id: Uuid::new_v4().to_string(),
                    list_id: personal_list_id.clone(),
                    title: "Deploy high-performance Rust core".to_string(),
                    estimate_mins: 45,
                    time_taken_secs: 0,
                    completed: false,
                    completed_at: None,
                    board: "today".to_string(),
                    sort_order: 0,
                    created_at: Utc::now(),
                },
                Task {
                    id: Uuid::new_v4().to_string(),
                    list_id: personal_list_id.clone(),
                    title: "Calibrate Locu app blocker rules".to_string(),
                    estimate_mins: 15,
                    time_taken_secs: 900,
                    completed: true,
                    completed_at: Some(Utc::now()),
                    board: "done".to_string(),
                    sort_order: 1,
                    created_at: Utc::now(),
                },
                Task {
                    id: Uuid::new_v4().to_string(),
                    list_id: personal_list_id,
                    title: "Configure BlitzTower screen snapping".to_string(),
                    estimate_mins: 30,
                    time_taken_secs: 0,
                    completed: false,
                    completed_at: None,
                    board: "week".to_string(),
                    sort_order: 2,
                    created_at: Utc::now(),
                },
            ],
            blocked_apps: vec![
                BlockedApp {
                    id: Uuid::new_v4().to_string(),
                    name: "Discord".to_string(),
                    process_name: "Discord.exe".to_string(),
                    active: true,
                },
                BlockedApp {
                    id: Uuid::new_v4().to_string(),
                    name: "Steam".to_string(),
                    process_name: "steam.exe".to_string(),
                    active: true,
                },
                BlockedApp {
                    id: Uuid::new_v4().to_string(),
                    name: "Spotify".to_string(),
                    process_name: "Spotify.exe".to_string(),
                    active: false,
                },
            ],
            preferences: ThulliPreferences::default(),
        }
    }
}

pub struct EncryptedStorage {
    security: Arc<SecurityManager>,
    file_path: std::path::PathBuf,
}

impl EncryptedStorage {
    pub fn new(security: Arc<SecurityManager>) -> Result<Self, Box<dyn std::error::Error>> {
        let app_data = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        let mut dir = std::path::PathBuf::from(app_data);
        dir.push("thulli");
        std::fs::create_dir_all(&dir)?;

        let mut file_path = dir;
        file_path.push("thulli_data.enc");

        Ok(Self {
            security,
            file_path,
        })
    }

    pub fn load(&self) -> AppStateStore {
        if self.file_path.exists() {
            match self.security.read_decrypted_file(&self.file_path) {
                Ok(bytes) => match serde_json::from_slice::<AppStateStore>(&bytes) {
                    Ok(store) => {
                        println!("[Storage] Successfully loaded & decrypted Thulli vault.");
                        return store;
                    }
                    Err(e) => println!("[Storage] Deserialization error: {}, reinitializing default", e),
                },
                Err(e) => println!("[Storage] Decryption error: {}, reinitializing default", e),
            }
        }

        let default_store = AppStateStore::default();
        let _ = self.save(&default_store);
        default_store
    }

    pub fn save(&self, store: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
        let serialized = serde_json::to_vec_pretty(store)?;
        self.security.save_encrypted_file(&self.file_path, &serialized)?;
        Ok(())
    }
}
