//! Repository pattern abstractions and encrypted persistence implementations.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use crate::models::AppStateStore;
use crate::security::SecurityManager;

/// Abstract repository interface for persisting application state.
pub trait StorageRepository: Send + Sync {
    /// Loads the stored application state or initializes defaults if not present.
    fn load(&self) -> AppStateStore;

    /// Persists the application state securely.
    fn save(&self, store: &AppStateStore) -> Result<(), Box<dyn std::error::Error>>;
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
        std::fs::create_dir_all(&dir)?;

        let mut file_path = dir;
        file_path.push("thulli_data.enc");

        Ok(Self {
            security,
            file_path,
        })
    }

    /// Custom path constructor (useful for testing and isolated sandboxes)
    pub fn with_custom_path(security: Arc<SecurityManager>, path: impl AsRef<Path>) -> Self {
        Self {
            security,
            file_path: path.as_ref().to_path_buf(),
        }
    }
}

impl StorageRepository for EncryptedStorageRepository {
    fn load(&self) -> AppStateStore {
        if self.file_path.exists() {
            match self.security.read_decrypted_file(&self.file_path) {
                Ok(bytes) => match serde_json::from_slice::<AppStateStore>(&bytes) {
                    Ok(store) => {
                        println!("[Storage] Loaded encrypted vault ({} lists, {} tasks)", store.lists.len(), store.tasks.len());
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

    fn save(&self, store: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
        let serialized = serde_json::to_vec_pretty(store)?;
        self.security.save_encrypted_file(&self.file_path, &serialized)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Task;

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
        let temp_dir = std::env::temp_dir().join(format!("thulli_test_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("test_vault.enc");

        let repo = EncryptedStorageRepository::with_custom_path(sec, &test_file);

        let mut store = repo.load();
        let list_id = store.lists[0].id.clone();

        store.add_task(Task::new(list_id, "Ship Thulli Modular MVP", 45, "today"));
        repo.save(&store).unwrap();

        let loaded = repo.load();
        assert_eq!(loaded.lists.len(), 1);
        assert_eq!(loaded.tasks.len(), 1);
        assert_eq!(loaded.tasks[0].title, "Ship Thulli Modular MVP");
        assert_eq!(loaded.tasks[0].estimate_mins, 45);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
