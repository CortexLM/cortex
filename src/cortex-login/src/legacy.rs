//! Legacy file-based credential storage (deprecated).
//!
//! This module provides backwards compatibility for credentials stored
//! in the old unencrypted JSON format. New credentials should use
//! keyring or encrypted file storage.

use anyhow::{Context, Result};
use secrecy::SecretString;
use std::path::Path;

use crate::types::{LegacyAuthData, SecureAuthData};
use crate::utils::set_file_permissions;

fn auth_file_path(cortex_home: &Path) -> std::path::PathBuf {
    cortex_home.join("auth.json")
}

/// Load authentication data from legacy unencrypted file.
pub fn load_from_file_legacy(cortex_home: &Path) -> Result<Option<SecureAuthData>> {
    let path = auth_file_path(cortex_home);

    if !path.exists() {
        return Ok(None);
    }

    tracing::warn!(
        "Loading from legacy unencrypted auth.json - please re-login to migrate to secure storage"
    );

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read auth file: {}", path.display()))?;

    let legacy: LegacyAuthData =
        serde_json::from_str(&content).context("Failed to parse legacy auth data")?;

    Ok(Some(SecureAuthData::from_components(
        legacy.mode,
        legacy.api_key.map(SecretString::from),
        legacy.access_token.map(SecretString::from),
        legacy.refresh_token.map(SecretString::from),
        legacy.expires_at,
        legacy.account_id,
    )))
}

/// Save authentication data to legacy unencrypted file (deprecated).
pub fn save_to_file_legacy(cortex_home: &Path, data: &SecureAuthData) -> Result<()> {
    use secrecy::ExposeSecret;

    tracing::warn!("Saving to legacy unencrypted auth.json - this is deprecated");

    let path = auth_file_path(cortex_home);

    // Create directory if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    let legacy = LegacyAuthData {
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

    let json = serde_json::to_string_pretty(&legacy).context("Failed to serialize auth data")?;

    std::fs::write(&path, json)
        .with_context(|| format!("Failed to write auth file: {}", path.display()))?;

    // Set restrictive permissions
    set_file_permissions(&path)?;

    Ok(())
}

/// Delete authentication data from legacy file.
pub fn delete_from_file_legacy(cortex_home: &Path) -> Result<bool> {
    let path = auth_file_path(cortex_home);

    if !path.exists() {
        return Ok(false);
    }

    std::fs::remove_file(&path)
        .with_context(|| format!("Failed to delete auth file: {}", path.display()))?;

    Ok(true)
}

/// Get the path to the legacy auth file.
pub fn get_legacy_auth_path(cortex_home: &Path) -> std::path::PathBuf {
    auth_file_path(cortex_home)
}
