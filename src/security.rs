use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use argon2::Argon2;
use rand::RngExt;
use std::fs;
use std::path::PathBuf;
use zeroize::Zeroize;

pub struct SecurityManager {
    key: [u8; 32],
}

impl SecurityManager {
    /// Initializes security by deriving a 256-bit AES master key from the machine identifier & salt
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        let machine_id = Self::get_machine_identifier();
        let salt = b"thulli_secure_salt_v1_2026";
        
        let mut key = [0u8; 32];
        let argon2 = Argon2::default();
        argon2
            .hash_password_into(machine_id.as_bytes(), salt, &mut key)
            .map_err(|e| format!("Argon2 key derivation error: {}", e))?;

        Ok(Self { key })
    }

    /// Retrieve Windows machine GUID, fallback to hostname/username
    fn get_machine_identifier() -> String {
        #[cfg(target_os = "windows")]
        {
            use std::process::Command;
            let output = Command::new("powershell")
                .args(["-NoProfile", "-Command", "(Get-ItemProperty -Path 'HKLM:\\SOFTWARE\\Microsoft\\Cryptography').MachineGuid"])
                .output();

            if let Ok(out) = output {
                let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !id.is_empty() {
                    return id;
                }
            }
        }

        format!("{}-thulli-master-id", whoami_fallback())
    }

    /// Encrypt plaintext bytes using AES-256-GCM with a random 12-byte nonce
    pub fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| format!("Cipher init error: {}", e))?;

        let mut nonce_bytes = [0u8; 12];
        rand::rng().fill(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, data)
            .map_err(|e| format!("Encryption error: {}", e))?;

        // Format: [12 bytes nonce] + [ciphertext + 16 bytes tag]
        let mut result = Vec::with_capacity(12 + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);

        Ok(result)
    }

    /// Decrypt data using AES-256-GCM
    pub fn decrypt(&self, encrypted_data: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        if encrypted_data.len() < 12 {
            return Err("Encrypted payload too short".into());
        }

        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| format!("Cipher init error: {}", e))?;

        let nonce = Nonce::from_slice(&encrypted_data[..12]);
        let ciphertext = &encrypted_data[12..];

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| format!("Decryption authentication error: {}", e))?;

        Ok(plaintext)
    }

    /// Save an encrypted JSON/TOML document to file
    pub fn save_encrypted_file(&self, path: &PathBuf, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let encrypted = self.encrypt(data)?;
        fs::write(path, encrypted)?;
        Ok(())
    }

    /// Read and decrypt file contents
    pub fn read_decrypted_file(&self, path: &PathBuf) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let encrypted = fs::read(path)?;
        self.decrypt(&encrypted)
    }
}

impl Drop for SecurityManager {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

fn whoami_fallback() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "default_user".to_string())
}
