//! Keyring-based credential storage.
//!
//! Provides secure credential storage using the system keyring:
//! - Windows: Credential Manager
//! - macOS: Keychain
//! - Linux: Secret Service (gnome-keyring, kwallet)

use anyhow::{Context, Result};
use secrecy::{ExposeSecret, SecretString};

use crate::constants::KEYRING_SERVICE;
use crate::types::{AuthData, AuthMode, SecureAuthData};

/// Keyring keys for different credential types.
const KEYRING_KEY_API: &str = "api_key";
const KEYRING_KEY_ACCESS: &str = "access_token";
const KEYRING_KEY_REFRESH: &str = "refresh_token";
const KEYRING_KEY_METADATA: &str = "metadata";

/// Windows Credential Manager has a BLOB size limit of 2560 bytes.
/// We use a smaller chunk size to be safe and account for encoding overhead.
/// See: https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentiala
#[cfg(target_os = "windows")]
const WINDOWS_CREDENTIAL_CHUNK_SIZE: usize = 2400;

/// Maximum number of chunks we support (allows up to ~24KB tokens which is more than enough)
#[cfg(target_os = "windows")]
const MAX_CREDENTIAL_CHUNKS: usize = 10;

fn get_keyring_entry(account: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, account).context("Failed to access keyring")
}

/// Windows-specific: Save a potentially large credential by splitting into chunks.
///
/// Windows Credential Manager has a hard limit of 2560 bytes per credential BLOB.
/// JWT tokens from OAuth can exceed this limit. This function splits large
/// credentials into multiple keyring entries and stores the chunk count in
/// a separate entry for reconstruction.
#[cfg(target_os = "windows")]
fn save_credential_chunked(base_key: &str, value: &str) -> Result<()> {
    let value_bytes = value.as_bytes();
    let total_len = value_bytes.len();

    // If it fits in a single chunk, store directly
    if total_len <= WINDOWS_CREDENTIAL_CHUNK_SIZE {
        tracing::debug!(
            key = base_key,
            size = total_len,
            "Storing credential directly (within size limit)"
        );
        get_keyring_entry(base_key)?
            .set_password(value)
            .with_context(|| format!("Failed to save {} to keyring", base_key))?;

        // Clean up any old chunk entries that might exist
        delete_credential_chunks(base_key)?;
        return Ok(());
    }

    // Calculate number of chunks needed
    let num_chunks =
        (total_len + WINDOWS_CREDENTIAL_CHUNK_SIZE - 1) / WINDOWS_CREDENTIAL_CHUNK_SIZE;

    if num_chunks > MAX_CREDENTIAL_CHUNKS {
        return Err(anyhow::anyhow!(
            "Credential too large: {} bytes requires {} chunks, max is {}",
            total_len,
            num_chunks,
            MAX_CREDENTIAL_CHUNKS
        ));
    }

    tracing::debug!(
        key = base_key,
        size = total_len,
        chunks = num_chunks,
        "Splitting large credential into chunks for Windows Credential Manager"
    );

    // Store each chunk
    for (i, chunk) in value_bytes
        .chunks(WINDOWS_CREDENTIAL_CHUNK_SIZE)
        .enumerate()
    {
        let chunk_key = format!("{}_chunk_{}", base_key, i);
        let chunk_str = String::from_utf8_lossy(chunk);

        get_keyring_entry(&chunk_key)?
            .set_password(&chunk_str)
            .with_context(|| format!("Failed to save chunk {} of {} to keyring", i, base_key))?;
    }

    // Store chunk metadata (count) in the base key
    let chunk_info = format!("__chunked__:{}", num_chunks);
    get_keyring_entry(base_key)?
        .set_password(&chunk_info)
        .with_context(|| format!("Failed to save chunk metadata for {} to keyring", base_key))?;

    // Clean up any extra old chunks that might exist from previous saves
    for i in num_chunks..MAX_CREDENTIAL_CHUNKS {
        let chunk_key = format!("{}_chunk_{}", base_key, i);
        if let Ok(entry) = get_keyring_entry(&chunk_key) {
            let _ = entry.delete_credential();
        }
    }

    Ok(())
}

/// Windows-specific: Load a potentially chunked credential.
#[cfg(target_os = "windows")]
fn load_credential_chunked(base_key: &str) -> Result<Option<String>> {
    let entry = get_keyring_entry(base_key)?;

    match entry.get_password() {
        Ok(value) => {
            // Check if this is chunk metadata
            if let Some(chunk_count_str) = value.strip_prefix("__chunked__:") {
                let num_chunks: usize = chunk_count_str
                    .parse()
                    .with_context(|| format!("Invalid chunk count for {}", base_key))?;

                tracing::debug!(
                    key = base_key,
                    chunks = num_chunks,
                    "Loading chunked credential from Windows Credential Manager"
                );

                // Reconstruct from chunks
                let mut full_value = String::new();
                for i in 0..num_chunks {
                    let chunk_key = format!("{}_chunk_{}", base_key, i);
                    let chunk_entry = get_keyring_entry(&chunk_key)?;
                    let chunk = chunk_entry
                        .get_password()
                        .with_context(|| format!("Failed to load chunk {} of {}", i, base_key))?;
                    full_value.push_str(&chunk);
                }

                Ok(Some(full_value))
            } else {
                // Regular non-chunked value
                Ok(Some(value))
            }
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("Keyring error loading {}: {}", base_key, e)),
    }
}

/// Windows-specific: Delete a credential and all its chunks.
#[cfg(target_os = "windows")]
fn delete_credential_chunked(base_key: &str) -> Result<bool> {
    let mut deleted = false;

    // Delete all possible chunks
    deleted |= delete_credential_chunks(base_key)?;

    // Delete the base entry
    if let Ok(entry) = get_keyring_entry(base_key) {
        match entry.delete_credential() {
            Ok(()) => deleted = true,
            Err(keyring::Error::NoEntry) => {}
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Keyring error deleting {}: {}",
                    base_key,
                    e
                ));
            }
        }
    }

    Ok(deleted)
}

/// Windows-specific: Delete all chunk entries for a credential.
#[cfg(target_os = "windows")]
fn delete_credential_chunks(base_key: &str) -> Result<bool> {
    let mut deleted = false;

    for i in 0..MAX_CREDENTIAL_CHUNKS {
        let chunk_key = format!("{}_chunk_{}", base_key, i);
        if let Ok(entry) = get_keyring_entry(&chunk_key) {
            match entry.delete_credential() {
                Ok(()) => deleted = true,
                Err(keyring::Error::NoEntry) => {}
                Err(e) => {
                    tracing::debug!(
                        key = chunk_key,
                        error = %e,
                        "Failed to delete chunk (may not exist)"
                    );
                }
            }
        }
    }

    Ok(deleted)
}

/// Non-Windows: Save credential directly (no size limit issues).
#[cfg(not(target_os = "windows"))]
fn save_credential_chunked(base_key: &str, value: &str) -> Result<()> {
    get_keyring_entry(base_key)?
        .set_password(value)
        .with_context(|| format!("Failed to save {} to keyring", base_key))
}

/// Non-Windows: Load credential directly.
#[cfg(not(target_os = "windows"))]
fn load_credential_chunked(base_key: &str) -> Result<Option<String>> {
    let entry = get_keyring_entry(base_key)?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("Keyring error loading {}: {}", base_key, e)),
    }
}

/// Non-Windows: Delete credential directly.
#[cfg(not(target_os = "windows"))]
fn delete_credential_chunked(base_key: &str) -> Result<bool> {
    if let Ok(entry) = get_keyring_entry(base_key) {
        match entry.delete_credential() {
            Ok(()) => return Ok(true),
            Err(keyring::Error::NoEntry) => return Ok(false),
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Keyring error deleting {}: {}",
                    base_key,
                    e
                ));
            }
        }
    }
    Ok(false)
}

/// Load authentication data from the system keyring.
///
/// On Windows, this function handles credentials that may have been split into
/// multiple chunks due to the 2560-byte BLOB size limit in Windows Credential Manager.
pub fn load_from_keyring() -> Result<Option<SecureAuthData>> {
    // Load metadata first (metadata is always small enough to fit in a single entry)
    let metadata_entry = get_keyring_entry(KEYRING_KEY_METADATA)?;

    let metadata: AuthData = match metadata_entry.get_password() {
        Ok(json) => serde_json::from_str(&json).context("Failed to parse auth metadata")?,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(e) => return Err(anyhow::anyhow!("Keyring error: {e}")),
    };

    // Load actual secrets based on metadata (using chunked loading for Windows compatibility)
    let api_key = if metadata.has_api_key {
        match load_credential_chunked(KEYRING_KEY_API) {
            Ok(Some(key)) => Some(SecretString::from(key)),
            Ok(None) | Err(_) => None,
        }
    } else {
        None
    };

    let access_token = if metadata.has_access_token {
        match load_credential_chunked(KEYRING_KEY_ACCESS) {
            Ok(Some(token)) => Some(SecretString::from(token)),
            Ok(None) | Err(_) => None,
        }
    } else {
        None
    };

    let refresh_token = if metadata.has_refresh_token {
        match load_credential_chunked(KEYRING_KEY_REFRESH) {
            Ok(Some(token)) => Some(SecretString::from(token)),
            Ok(None) | Err(_) => None,
        }
    } else {
        None
    };

    Ok(Some(SecureAuthData::from_components(
        metadata.mode,
        api_key,
        access_token,
        refresh_token,
        metadata.expires_at,
        metadata.account_id,
    )))
}

/// Save authentication data to the system keyring.
///
/// On Windows, this function automatically splits large credentials (like OAuth JWT tokens)
/// into multiple chunks to work around the 2560-byte BLOB size limit in Windows Credential Manager.
pub fn save_to_keyring(data: &SecureAuthData) -> Result<()> {
    tracing::debug!(
        mode = ?data.mode,
        has_api_key = data.api_key.is_some(),
        has_access_token = data.access_token.is_some(),
        has_refresh_token = data.refresh_token.is_some(),
        "Attempting to save credentials to system keyring"
    );

    // Save secrets (using chunked saving for Windows compatibility)
    if let Some(ref api_key) = data.api_key {
        let key_len = api_key.expose_secret().len();
        tracing::debug!(key = KEYRING_KEY_API, size = key_len, "Saving API key");
        save_credential_chunked(KEYRING_KEY_API, api_key.expose_secret())
            .context("Failed to save API key to keyring")?;
        tracing::debug!(key = KEYRING_KEY_API, "API key saved successfully");
    }

    if let Some(ref access_token) = data.access_token {
        let token_len = access_token.expose_secret().len();
        tracing::debug!(
            key = KEYRING_KEY_ACCESS,
            size = token_len,
            "Saving access token"
        );
        save_credential_chunked(KEYRING_KEY_ACCESS, access_token.expose_secret())
            .context("Failed to save access token to keyring")?;
        tracing::debug!(key = KEYRING_KEY_ACCESS, "Access token saved successfully");
    }

    if let Some(ref refresh_token) = data.refresh_token {
        let token_len = refresh_token.expose_secret().len();
        tracing::debug!(
            key = KEYRING_KEY_REFRESH,
            size = token_len,
            "Saving refresh token"
        );
        save_credential_chunked(KEYRING_KEY_REFRESH, refresh_token.expose_secret())
            .context("Failed to save refresh token to keyring")?;
        tracing::debug!(
            key = KEYRING_KEY_REFRESH,
            "Refresh token saved successfully"
        );
    }

    // Save metadata (always small enough to fit in a single entry)
    let metadata = data.to_metadata();
    let metadata_json = serde_json::to_string(&metadata).context("Failed to serialize metadata")?;
    tracing::debug!(
        key = KEYRING_KEY_METADATA,
        size = metadata_json.len(),
        "Saving metadata"
    );

    get_keyring_entry(KEYRING_KEY_METADATA)?
        .set_password(&metadata_json)
        .context("Failed to save metadata to keyring")?;

    tracing::debug!("All credentials saved successfully to keyring");
    Ok(())
}

/// Delete authentication data from the system keyring.
///
/// On Windows, this function also cleans up any chunk entries that may have been
/// created for large credentials.
pub fn delete_from_keyring() -> Result<bool> {
    let mut deleted = false;

    // Delete all keyring entries (using chunked deletion for Windows compatibility)
    // This ensures any chunk entries are also cleaned up
    for key in &[KEYRING_KEY_API, KEYRING_KEY_ACCESS, KEYRING_KEY_REFRESH] {
        deleted |= delete_credential_chunked(key)?;
    }

    // Delete metadata (always a single entry)
    if let Ok(entry) = get_keyring_entry(KEYRING_KEY_METADATA) {
        match entry.delete_credential() {
            Ok(()) => deleted = true,
            Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(anyhow::anyhow!("Keyring error: {e}")),
        }
    }

    Ok(deleted)
}
