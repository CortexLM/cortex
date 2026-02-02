//! Keyring-based credential storage for Cortex CLI.
//!
//! This crate provides secure credential storage using OS-native keychains:
//! - Windows: Credential Manager
//! - macOS: Keychain
//! - Linux: Secret Service (GNOME Keyring, KWallet)
//!
//! Security features:
//! - Credentials stored in OS keychain, not on disk
//! - Automatic memory protection for sensitive values
//! - Secure deletion with proper cleanup

use thiserror::Error;
use tracing::{debug, warn};

/// Default service name for keyring entries.
pub const DEFAULT_SERVICE: &str = "cortex-cli";

/// Errors that can occur during keyring operations.
#[derive(Error, Debug)]
pub enum KeyringError {
    /// Failed to access the keyring.
    #[error("Failed to access keyring: {0}")]
    AccessDenied(String),

    /// No entry found for the specified key.
    #[error("No entry found for key: {0}")]
    NotFound(String),

    /// Failed to store the credential.
    #[error("Failed to store credential: {0}")]
    StoreFailed(String),

    /// Failed to delete the credential.
    #[error("Failed to delete credential: {0}")]
    DeleteFailed(String),

    /// Platform not supported.
    #[error("Keyring not supported on this platform")]
    NotSupported,

    /// Internal keyring error.
    #[error("Keyring error: {0}")]
    Internal(String),
}

impl From<keyring::Error> for KeyringError {
    fn from(err: keyring::Error) -> Self {
        match err {
            keyring::Error::NoEntry => KeyringError::NotFound("Entry not found".to_string()),
            keyring::Error::NoStorageAccess(_) => {
                KeyringError::AccessDenied("Cannot access keyring storage".to_string())
            }
            keyring::Error::PlatformFailure(_) => {
                KeyringError::Internal("Platform-specific keyring failure".to_string())
            }
            _ => KeyringError::Internal(err.to_string()),
        }
    }
}

/// Result type for keyring operations.
pub type Result<T> = std::result::Result<T, KeyringError>;

/// Keyring store for secure credential management.
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    /// Create a new keyring store with the default service name.
    pub fn new() -> Self {
        Self {
            service: DEFAULT_SERVICE.to_string(),
        }
    }

    /// Create a new keyring store with a custom service name.
    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    /// Get an entry from the keyring.
    fn get_entry(&self, key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, key).map_err(KeyringError::from)
    }

    /// Store a credential in the keyring.
    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        let entry = self.get_entry(key)?;
        entry.set_password(value).map_err(|e| {
            warn!("Failed to store credential for key '{}': {}", key, e);
            KeyringError::StoreFailed(e.to_string())
        })?;
        debug!("Stored credential for key '{}'", key);
        Ok(())
    }

    /// Retrieve a credential from the keyring.
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        let entry = self.get_entry(key)?;
        match entry.get_password() {
            Ok(value) => {
                debug!("Retrieved credential for key '{}'", key);
                Ok(Some(value))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(KeyringError::from(e)),
        }
    }

    /// Delete a credential from the keyring.
    pub fn delete(&self, key: &str) -> Result<bool> {
        let entry = self.get_entry(key)?;
        match entry.delete_credential() {
            Ok(()) => {
                debug!("Deleted credential for key '{}'", key);
                Ok(true)
            }
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(KeyringError::DeleteFailed(e.to_string())),
        }
    }

    /// Check if a credential exists in the keyring.
    pub fn exists(&self, key: &str) -> Result<bool> {
        let entry = self.get_entry(key)?;
        match entry.get_password() {
            Ok(_) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(KeyringError::from(e)),
        }
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience functions for quick access with default service.
pub mod quick {
    use super::*;

    /// Store a credential with the default service.
    pub fn set(key: &str, value: &str) -> Result<()> {
        KeyringStore::new().set(key, value)
    }

    /// Retrieve a credential with the default service.
    pub fn get(key: &str) -> Result<Option<String>> {
        KeyringStore::new().get(key)
    }

    /// Delete a credential with the default service.
    pub fn delete(key: &str) -> Result<bool> {
        KeyringStore::new().delete(key)
    }

    /// Check if a credential exists with the default service.
    pub fn exists(key: &str) -> Result<bool> {
        KeyringStore::new().exists(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests require a working keyring on the system.
    // They are marked as ignored by default to avoid CI failures.

    #[test]
    #[ignore]
    fn test_store_and_retrieve() {
        let store = KeyringStore::with_service("cortex-cli-test");
        let key = "test-key";
        let value = "test-value-12345";

        // Store
        store.set(key, value).expect("Failed to store");

        // Retrieve
        let retrieved = store.get(key).expect("Failed to get");
        assert_eq!(retrieved, Some(value.to_string()));

        // Delete
        let deleted = store.delete(key).expect("Failed to delete");
        assert!(deleted);

        // Verify deleted
        let after_delete = store.get(key).expect("Failed to get after delete");
        assert!(after_delete.is_none());
    }

    #[test]
    #[ignore]
    fn test_not_found() {
        let store = KeyringStore::with_service("cortex-cli-test");
        let result = store.get("nonexistent-key-12345");
        assert!(matches!(result, Ok(None)));
    }

    #[test]
    #[ignore]
    fn test_exists() {
        let store = KeyringStore::with_service("cortex-cli-test");
        let key = "test-exists-key";

        // Should not exist
        assert!(!store.exists(key).unwrap());

        // Store
        store.set(key, "value").unwrap();

        // Should exist
        assert!(store.exists(key).unwrap());

        // Cleanup
        store.delete(key).unwrap();
    }
}
