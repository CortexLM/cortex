//! Encrypted file-based credential storage.
//!
//! Uses AES-256-GCM for encryption with a machine-derived key.
//! This provides secure storage when keyring is unavailable.

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng, rand_core::RngCore},
};
use anyhow::{Context, Result};
use secrecy::{ExposeSecret, SecretString};
use std::path::Path;
use zeroize::Zeroize;

use crate::types::{SecureAuthData, StoredSecureAuth};
use crate::utils::set_file_permissions;

const ENCRYPTED_AUTH_FILE: &str = "auth.enc";
const NONCE_SIZE: usize = 12;
const KEY_SIZE: usize = 32;

/// Generate encryption key from machine-specific entropy.
/// This removes the circular dependency on keyring for encrypted file mode.
fn get_machine_derived_key() -> Result<[u8; KEY_SIZE]> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();

    // Machine ID (Linux) or fallback
    #[cfg(target_os = "linux")]
    {
        if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
            hasher.update(id.trim().as_bytes());
        } else if let Ok(id) = std::fs::read_to_string("/var/lib/dbus/machine-id") {
            hasher.update(id.trim().as_bytes());
        }
    }

    // Hostname
    if let Ok(hostname) = hostname::get() {
        hasher.update(hostname.as_encoded_bytes());
    }

    // User ID (Unix) or username (Windows)
    #[cfg(unix)]
    {
        hasher.update(unsafe { libc::getuid() }.to_le_bytes());
    }
    #[cfg(windows)]
    {
        if let Ok(user) = std::env::var("USERNAME") {
            hasher.update(user.as_bytes());
        }
    }

    // Home directory path as additional entropy
    if let Some(home) = dirs::home_dir() {
        hasher.update(home.to_string_lossy().as_bytes());
    }

    // Application-specific salt
    hasher.update(b"cortex-cli-credential-encryption-v1-machine-key");

    let result = hasher.finalize();
    let mut key = [0u8; KEY_SIZE];
    key.copy_from_slice(&result);
    Ok(key)
}

/// Get encryption key - uses machine-derived key (no keyring dependency).
fn get_encryption_key() -> Result<[u8; KEY_SIZE]> {
    get_machine_derived_key()
}

fn encrypted_auth_path(cortex_home: &Path) -> std::path::PathBuf {
    cortex_home.join(ENCRYPTED_AUTH_FILE)
}

/// Load authentication data from encrypted file.
pub fn load_from_encrypted_file(cortex_home: &Path) -> Result<Option<SecureAuthData>> {
    let path = encrypted_auth_path(cortex_home);

    if !path.exists() {
        return Ok(None);
    }

    let encrypted_data = std::fs::read(&path)
        .with_context(|| format!("Failed to read encrypted auth file: {}", path.display()))?;

    if encrypted_data.len() < NONCE_SIZE {
        return Err(anyhow::anyhow!("Invalid encrypted auth file"));
    }

    // Extract nonce and ciphertext
    let nonce_bytes = &encrypted_data[..NONCE_SIZE];
    let ciphertext = &encrypted_data[NONCE_SIZE..];

    // Get encryption key
    let key = get_encryption_key()?;

    // Decrypt
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow::anyhow!("Cipher init failed: {e}"))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("Decryption failed: {e}"))?;

    let mut json_str = String::from_utf8(plaintext).context("Invalid UTF-8 in decrypted data")?;

    // Parse the stored data
    let stored: StoredSecureAuth =
        serde_json::from_str(&json_str).context("Failed to parse decrypted auth data")?;

    // Clear the plaintext from memory
    json_str.zeroize();

    Ok(Some(SecureAuthData::from_components(
        stored.mode,
        stored.api_key.map(SecretString::from),
        stored.access_token.map(SecretString::from),
        stored.refresh_token.map(SecretString::from),
        stored.expires_at,
        stored.account_id,
    )))
}

/// Save authentication data to encrypted file.
pub fn save_to_encrypted_file(cortex_home: &Path, data: &SecureAuthData) -> Result<()> {
    let path = encrypted_auth_path(cortex_home);

    // Create directory if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Convert to storable format
    let stored = StoredSecureAuth {
        mode: data.mode,
        api_key: data.api_key.as_ref().map(|s| s.expose_secret().to_string()),
        access_token: data
            .access_token
            .as_ref()
            .map(|s| s.expose_secret().to_string()),
        refresh_token: data
            .refresh_token
            .as_ref()
            .map(|s| s.expose_secret().to_string()),
        expires_at: data.expires_at,
        account_id: data.account_id.clone(),
    };

    let mut json = serde_json::to_string(&stored).context("Failed to serialize auth data")?;

    // Get encryption key
    let key = get_encryption_key()?;

    // Generate nonce
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Encrypt
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow::anyhow!("Cipher init failed: {e}"))?;

    let ciphertext = cipher
        .encrypt(nonce, json.as_bytes())
        .map_err(|e| anyhow::anyhow!("Encryption failed: {e}"))?;

    // Clear plaintext
    json.zeroize();

    // Write nonce + ciphertext
    let mut output = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);

    std::fs::write(&path, &output)
        .with_context(|| format!("Failed to write encrypted auth file: {}", path.display()))?;

    // Set restrictive permissions
    set_file_permissions(&path)?;

    Ok(())
}

/// Delete authentication data from encrypted file.
pub fn delete_from_encrypted_file(cortex_home: &Path) -> Result<bool> {
    let path = encrypted_auth_path(cortex_home);

    if !path.exists() {
        return Ok(false);
    }

    // Overwrite with random data before deleting (secure deletion)
    if let Ok(metadata) = std::fs::metadata(&path) {
        let size = metadata.len() as usize;
        let mut random_data = vec![0u8; size];
        OsRng.fill_bytes(&mut random_data);
        let _ = std::fs::write(&path, &random_data);
    }

    std::fs::remove_file(&path)
        .with_context(|| format!("Failed to delete encrypted auth file: {}", path.display()))?;

    Ok(true)
}
