//! High-level credential storage operations.
//!
//! Provides unified interface for loading, saving, and deleting credentials
//! across different storage backends (keyring, encrypted file, legacy file).

use aes_gcm::aead::{OsRng, rand_core::RngCore};
use anyhow::Result;
use std::path::Path;

use crate::encrypted::{
    delete_from_encrypted_file, load_from_encrypted_file, save_to_encrypted_file,
};
use crate::keyring::{delete_from_keyring, load_from_keyring, save_to_keyring};
use crate::legacy::{
    delete_from_file_legacy, get_legacy_auth_path, load_from_file_legacy, save_to_file_legacy,
};
use crate::types::{CredentialsStoreMode, SecureAuthData};

/// Load authentication data from storage.
pub fn load_auth(cortex_home: &Path, mode: CredentialsStoreMode) -> Result<Option<SecureAuthData>> {
    match mode {
        CredentialsStoreMode::Keyring => load_from_keyring(),
        CredentialsStoreMode::EncryptedFile => load_from_encrypted_file(cortex_home),
        CredentialsStoreMode::File => load_from_file_legacy(cortex_home),
    }
}

/// Save authentication data to storage.
pub fn save_auth(
    cortex_home: &Path,
    data: &SecureAuthData,
    mode: CredentialsStoreMode,
) -> Result<()> {
    match mode {
        CredentialsStoreMode::Keyring => save_to_keyring(data),
        CredentialsStoreMode::EncryptedFile => save_to_encrypted_file(cortex_home, data),
        CredentialsStoreMode::File => save_to_file_legacy(cortex_home, data),
    }
}

/// Delete authentication data from storage.
pub fn delete_auth(cortex_home: &Path, mode: CredentialsStoreMode) -> Result<bool> {
    match mode {
        CredentialsStoreMode::Keyring => delete_from_keyring(),
        CredentialsStoreMode::EncryptedFile => delete_from_encrypted_file(cortex_home),
        CredentialsStoreMode::File => delete_from_file_legacy(cortex_home),
    }
}

/// Login with API key (stores securely).
pub fn login_with_api_key(
    cortex_home: &Path,
    api_key: &str,
    mode: CredentialsStoreMode,
) -> Result<()> {
    let data = SecureAuthData::with_api_key(api_key.to_string());
    save_auth(cortex_home, &data, mode)?;
    Ok(())
}

/// Logout and remove stored credentials.
pub fn logout(cortex_home: &Path, mode: CredentialsStoreMode) -> Result<bool> {
    delete_auth(cortex_home, mode)
}

/// Logout and remove stored credentials from all storage locations.
///
/// This function clears credentials from all possible storage locations:
/// - System keyring
/// - Encrypted file
/// - Legacy file
///
/// This should be used when credentials were saved with `save_auth_with_fallback`,
/// as the fallback mechanism may have stored credentials in encrypted file
/// when keyring was unavailable.
///
/// Returns true if any credentials were deleted.
pub fn logout_with_fallback(cortex_home: &Path) -> Result<bool> {
    let mut deleted = false;

    // Try to delete from keyring
    match delete_from_keyring() {
        Ok(true) => {
            tracing::debug!("Deleted credentials from keyring");
            deleted = true;
        }
        Ok(false) => {
            tracing::debug!("No credentials in keyring to delete");
        }
        Err(e) => {
            tracing::debug!(error = %e, "Failed to delete from keyring (may not exist)");
        }
    }

    // Try to delete from encrypted file
    match delete_from_encrypted_file(cortex_home) {
        Ok(true) => {
            tracing::debug!("Deleted credentials from encrypted file");
            deleted = true;
        }
        Ok(false) => {
            tracing::debug!("No encrypted credentials file to delete");
        }
        Err(e) => {
            tracing::debug!(error = %e, "Failed to delete encrypted file (may not exist)");
        }
    }

    // Try to delete from legacy file
    match delete_from_file_legacy(cortex_home) {
        Ok(true) => {
            tracing::info!("Deleted credentials from legacy file");
            deleted = true;
        }
        Ok(false) => {
            tracing::debug!("No legacy credentials file to delete");
        }
        Err(e) => {
            tracing::debug!(error = %e, "Failed to delete legacy file (may not exist)");
        }
    }

    Ok(deleted)
}

/// Save authentication data with automatic fallback.
///
/// Tries keyring first, falls back to encrypted file if keyring fails.
/// Returns the storage mode that was actually used.
///
/// **Important**: Before saving to a new storage location, this function clears
/// credentials from ALL other storage locations to prevent stale data from being
/// loaded on subsequent authentication checks.
///
/// # Windows Note
/// On Windows, the Credential Manager has a 2560-byte limit per credential.
/// Large OAuth tokens are automatically split into chunks. If chunking still
/// fails (e.g., due to permissions), the function falls back to encrypted file storage.
pub fn save_auth_with_fallback(
    cortex_home: &Path,
    data: &SecureAuthData,
) -> Result<CredentialsStoreMode> {
    tracing::debug!(
        mode = ?data.mode,
        cortex_home = %cortex_home.display(),
        "Attempting to save credentials with fallback support"
    );

    // IMPORTANT: Clear ALL existing credentials from all storage locations before saving.
    // This prevents stale/expired tokens from being loaded from other storage locations
    // on subsequent authentication checks.
    tracing::debug!("Clearing existing credentials from all storage locations before saving");
    clear_all_auth_storage(cortex_home);

    // Try keyring first
    match save_to_keyring(data) {
        Ok(()) => {
            tracing::info!("Credentials saved to system keyring successfully");
            return Ok(CredentialsStoreMode::Keyring);
        }
        Err(e) => {
            // Log detailed error for debugging
            tracing::warn!(
                error = %e,
                error_debug = ?e,
                "Keyring save failed, will attempt encrypted file fallback"
            );

            // On Windows, provide more specific guidance
            #[cfg(target_os = "windows")]
            tracing::info!(
                "Windows Credential Manager may have failed due to permissions or size limits. \
                 Trying encrypted file storage as fallback."
            );
        }
    }

    // Fallback to encrypted file
    tracing::debug!(
        path = %cortex_home.display(),
        "Attempting encrypted file storage fallback"
    );

    match save_to_encrypted_file(cortex_home, data) {
        Ok(()) => {
            tracing::info!(
                path = %cortex_home.display(),
                "Credentials saved to encrypted file (keyring unavailable)"
            );
            return Ok(CredentialsStoreMode::EncryptedFile);
        }
        Err(e) => {
            tracing::error!(
                error = %e,
                error_debug = ?e,
                path = %cortex_home.display(),
                "Failed to save to encrypted file"
            );
        }
    }

    // Both failed - provide helpful error message
    let error_msg = format!(
        "Failed to save credentials. Both keyring and encrypted file storage failed.\n\
         \n\
         Possible solutions:\n\
         1. Set CORTEX_API_KEY environment variable instead\n\
         2. On Windows: Run as administrator or check Credential Manager permissions\n\
         3. On Linux: Install and start a secret service (gnome-keyring, kwallet)\n\
         4. Ensure {} directory exists and is writable\n\
         \n\
         For CI/CD environments, use: export CORTEX_API_KEY=your-api-key\n\
         \n\
         Run with --log-level trace for detailed error information.",
        cortex_home.display()
    );

    tracing::error!("{}", error_msg);
    Err(anyhow::anyhow!("{}", error_msg))
}

/// Clear credentials from all storage locations (keyring, encrypted file, legacy file).
///
/// This is used internally before saving new credentials to ensure no stale data
/// remains in other storage locations.
///
/// Errors are logged but not propagated since this is a best-effort cleanup.
fn clear_all_auth_storage(cortex_home: &Path) {
    // Clear keyring (ignore errors - it might not exist or be unavailable)
    if let Err(e) = delete_from_keyring() {
        tracing::debug!(error = %e, "Failed to clear keyring (may not exist)");
    }

    // Clear encrypted file (ignore errors - it might not exist)
    if let Err(e) = delete_from_encrypted_file(cortex_home) {
        tracing::debug!(error = %e, "Failed to clear encrypted file (may not exist)");
    }

    // Clear legacy file (ignore errors - it might not exist)
    if let Err(e) = delete_from_file_legacy(cortex_home) {
        tracing::debug!(error = %e, "Failed to clear legacy file (may not exist)");
    }
}

/// Load authentication data with automatic fallback.
///
/// Tries keyring first, then encrypted file, then legacy file.
pub fn load_auth_with_fallback(cortex_home: &Path) -> Result<Option<SecureAuthData>> {
    // Try keyring first (both new and legacy service names)
    match load_from_keyring() {
        Ok(Some(auth)) => {
            tracing::debug!("Loaded credentials from keyring");
            return Ok(Some(auth));
        }
        Ok(None) => {
            tracing::debug!("No credentials in keyring");
        }
        Err(e) => {
            tracing::debug!(error = %e, "Keyring access failed, trying fallbacks");
        }
    }

    // Try encrypted file
    match load_from_encrypted_file(cortex_home) {
        Ok(Some(auth)) => {
            tracing::debug!("Loaded credentials from encrypted file");
            return Ok(Some(auth));
        }
        Ok(None) => {}
        Err(e) => {
            tracing::debug!(error = %e, "Encrypted file load failed");
        }
    }

    // Try legacy file (for migration)
    match load_from_file_legacy(cortex_home) {
        Ok(Some(auth)) => {
            tracing::info!(
                "Found legacy credentials, consider re-logging to migrate to secure storage"
            );
            return Ok(Some(auth));
        }
        Ok(None) => {}
        Err(e) => {
            tracing::debug!(error = %e, "Legacy file load failed");
        }
    }

    Ok(None)
}

/// Migrate from legacy storage to secure storage.
pub fn migrate_to_secure_storage(cortex_home: &Path) -> Result<bool> {
    // Try to load from legacy file
    if let Ok(Some(data)) = load_from_file_legacy(cortex_home) {
        // Save to keyring (primary secure storage)
        save_to_keyring(&data)?;

        // Delete legacy file (secure deletion)
        let legacy_path = get_legacy_auth_path(cortex_home);
        if legacy_path.exists() {
            // Overwrite with random data
            if let Ok(metadata) = std::fs::metadata(&legacy_path) {
                let size = metadata.len() as usize;
                let mut random_data = vec![0u8; size];
                OsRng.fill_bytes(&mut random_data);
                let _ = std::fs::write(&legacy_path, &random_data);
            }
            std::fs::remove_file(&legacy_path)?;
        }

        tracing::info!("Migrated credentials from legacy file to secure keyring storage");
        return Ok(true);
    }

    Ok(false)
}

/// Check if valid authentication exists (without loading secrets).
///
/// This checks all storage locations (keyring, encrypted file, legacy file)
/// using the same fallback mechanism as `load_auth_with_fallback`.
/// Useful for UI status checks.
pub fn has_valid_auth() -> bool {
    // Get default cortex home for fallback loading
    let cortex_home = match dirs::home_dir() {
        Some(home) => home.join(".cortex"),
        None => return false,
    };

    match load_auth_with_fallback(&cortex_home) {
        Ok(Some(auth)) => !auth.is_expired() && auth.get_token().is_some(),
        _ => false,
    }
}
