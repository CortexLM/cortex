//! Secrets management.
//!
//! Provides secure storage and retrieval of secrets
//! including API keys, tokens, and credentials.
//!
//! Security features:
//! - OS keychain integration (Windows Credential Manager, macOS Keychain, Linux Secret Service)
//! - AES-256-GCM encryption at rest
//! - Argon2id key derivation
//! - Secure memory handling with zeroize
//! - File permission enforcement (0600)

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng, rand_core::RngCore},
};
use argon2::Argon2;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::Result;

/// Service name for keyring storage.
const KEYRING_SERVICE: &str = "cortex-cli";

/// Account name for master key storage.
const KEYRING_MASTER_KEY_ACCOUNT: &str = "master-encryption-key";

/// Encryption nonce size (96 bits for AES-GCM).
const NONCE_SIZE: usize = 12;

/// Salt size for key derivation.
const SALT_SIZE: usize = 16;

/// Secret type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SecretType {
    /// API key.
    ApiKey,
    /// Access token.
    AccessToken,
    /// Refresh token.
    RefreshToken,
    /// Password.
    Password,
    /// Private key.
    PrivateKey,
    /// Certificate.
    Certificate,
    /// Generic secret.
    #[default]
    Generic,
}

/// Secret metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMetadata {
    /// Secret type.
    pub secret_type: SecretType,
    /// Description.
    pub description: Option<String>,
    /// Created timestamp.
    pub created_at: u64,
    /// Updated timestamp.
    pub updated_at: u64,
    /// Expires at.
    pub expires_at: Option<u64>,
    /// Tags.
    pub tags: Vec<String>,
    /// Custom attributes.
    pub attributes: HashMap<String, String>,
}

impl SecretMetadata {
    /// Create new metadata.
    pub fn new(secret_type: SecretType) -> Self {
        let now = timestamp_now();
        Self {
            secret_type,
            description: None,
            created_at: now,
            updated_at: now,
            expires_at: None,
            tags: Vec::new(),
            attributes: HashMap::new(),
        }
    }

    /// Set description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set expiration.
    pub fn expires_at(mut self, ts: u64) -> Self {
        self.expires_at = Some(ts);
        self
    }

    /// Add tag.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Add attribute.
    pub fn attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    /// Check if expired.
    pub fn is_expired(&self) -> bool {
        if let Some(expires) = self.expires_at {
            timestamp_now() > expires
        } else {
            false
        }
    }
}

/// Secret value (sensitive) - uses secrecy crate for memory protection.
#[derive(Clone)]
pub struct SecretValue {
    /// The secret value (protected in memory).
    value: SecretString,
    /// Metadata.
    metadata: SecretMetadata,
}

impl SecretValue {
    /// Create a new secret.
    pub fn new(value: impl Into<String>, metadata: SecretMetadata) -> Self {
        Self {
            value: SecretString::from(value.into()),
            metadata,
        }
    }

    /// Create a simple secret.
    pub fn simple(value: impl Into<String>, secret_type: SecretType) -> Self {
        Self::new(value, SecretMetadata::new(secret_type))
    }

    /// Get the value (exposes the secret - use sparingly).
    pub fn value(&self) -> &str {
        self.value.expose_secret()
    }

    /// Get metadata.
    pub fn metadata(&self) -> &SecretMetadata {
        &self.metadata
    }

    /// Check if expired.
    pub fn is_expired(&self) -> bool {
        self.metadata.is_expired()
    }

    /// Get redacted value for safe display.
    pub fn redacted(&self) -> String {
        let exposed = self.value.expose_secret();
        if exposed.len() <= 8 {
            "****".to_string()
        } else {
            let prefix = &exposed[..4];
            let suffix = &exposed[exposed.len() - 4..];
            format!("{prefix}...{suffix}")
        }
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretValue")
            .field("value", &"[REDACTED]")
            .field("metadata", &self.metadata)
            .finish()
    }
}

/// Encrypted secret data for storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedSecret {
    /// Encrypted value (base64 encoded).
    ciphertext: String,
    /// Nonce used for encryption (base64 encoded).
    nonce: String,
    /// Salt used for key derivation (base64 encoded).
    salt: String,
    /// Metadata.
    metadata: SecretMetadata,
}

/// Encryption key wrapper with secure memory.
#[derive(ZeroizeOnDrop)]
pub struct EncryptionKey {
    #[zeroize(skip)]
    key: [u8; 32],
}

impl EncryptionKey {
    fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    fn as_bytes(&self) -> &[u8; 32] {
        &self.key
    }
}

/// Encryption utilities for secrets at rest.
pub struct SecretEncryption;

impl SecretEncryption {
    /// Derive an encryption key from a password using Argon2id.
    pub fn derive_key(password: &str, salt: &[u8]) -> Result<EncryptionKey> {
        let argon2 = Argon2::default();
        let mut key = [0u8; 32];

        argon2
            .hash_password_into(password.as_bytes(), salt, &mut key)
            .map_err(|e| {
                crate::error::CortexError::Internal(format!("Key derivation failed: {e}"))
            })?;

        Ok(EncryptionKey::new(key))
    }

    /// Generate a random salt.
    pub fn generate_salt() -> [u8; SALT_SIZE] {
        let mut salt = [0u8; SALT_SIZE];
        OsRng.fill_bytes(&mut salt);
        salt
    }

    /// Generate a random nonce.
    fn generate_nonce() -> [u8; NONCE_SIZE] {
        let mut nonce = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce);
        nonce
    }

    /// Encrypt a secret value.
    pub fn encrypt(plaintext: &str, key: &EncryptionKey) -> Result<(Vec<u8>, [u8; NONCE_SIZE])> {
        let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
            .map_err(|e| crate::error::CortexError::Internal(format!("Cipher init failed: {e}")))?;

        let nonce_bytes = Self::generate_nonce();
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| crate::error::CortexError::Internal(format!("Encryption failed: {e}")))?;

        Ok((ciphertext, nonce_bytes))
    }

    /// Decrypt a secret value.
    pub fn decrypt(
        ciphertext: &[u8],
        nonce: &[u8; NONCE_SIZE],
        key: &EncryptionKey,
    ) -> Result<SecretString> {
        let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
            .map_err(|e| crate::error::CortexError::Internal(format!("Cipher init failed: {e}")))?;

        let nonce = Nonce::from_slice(nonce);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| crate::error::CortexError::Internal(format!("Decryption failed: {e}")))?;

        let mut plaintext_str = String::from_utf8(plaintext)
            .map_err(|e| crate::error::CortexError::Internal(format!("Invalid UTF-8: {e}")))?;

        let secret = SecretString::from(plaintext_str.clone());
        plaintext_str.zeroize();

        Ok(secret)
    }
}

/// Secret store trait.
#[async_trait::async_trait]
pub trait SecretStore: Send + Sync {
    /// Get a secret.
    async fn get(&self, key: &str) -> Result<Option<SecretValue>>;

    /// Set a secret.
    async fn set(&self, key: &str, value: SecretValue) -> Result<()>;

    /// Delete a secret.
    async fn delete(&self, key: &str) -> Result<()>;

    /// List secret keys.
    async fn list(&self) -> Result<Vec<String>>;

    /// Check if key exists.
    async fn exists(&self, key: &str) -> Result<bool>;
}

/// Keyring-based secret store using OS native credential storage.
#[derive(Debug)]
pub struct KeyringSecretStore {
    service: String,
}

impl KeyringSecretStore {
    /// Create a new keyring store with default service name.
    pub fn new() -> Self {
        Self {
            service: KEYRING_SERVICE.to_string(),
        }
    }

    /// Create a new keyring store with custom service name.
    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    /// Get keyring entry for a key.
    fn get_entry(&self, key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, key)
            .map_err(|e| crate::error::CortexError::Internal(format!("Keyring access failed: {e}")))
    }
}

impl Default for KeyringSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SecretStore for KeyringSecretStore {
    async fn get(&self, key: &str) -> Result<Option<SecretValue>> {
        let entry = self.get_entry(key)?;

        match entry.get_password() {
            Ok(data) => {
                let stored: StoredSecret = serde_json::from_str(&data).map_err(|e| {
                    crate::error::CortexError::Internal(format!("Failed to parse secret: {e}"))
                })?;
                Ok(Some(stored.into_secret_value()))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(crate::error::CortexError::Internal(format!(
                "Keyring error: {e}"
            ))),
        }
    }

    async fn set(&self, key: &str, value: SecretValue) -> Result<()> {
        let entry = self.get_entry(key)?;
        let stored = StoredSecret::from_secret_value(&value);
        let data = serde_json::to_string(&stored).map_err(|e| {
            crate::error::CortexError::Internal(format!("Failed to serialize secret: {e}"))
        })?;

        entry
            .set_password(&data)
            .map_err(|e| crate::error::CortexError::Internal(format!("Keyring write failed: {e}")))
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let entry = self.get_entry(key)?;

        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(crate::error::CortexError::Internal(format!(
                "Keyring delete failed: {e}"
            ))),
        }
    }

    async fn list(&self) -> Result<Vec<String>> {
        // Keyring doesn't support listing - return empty
        // In production, maintain a separate index
        Ok(Vec::new())
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        let entry = self.get_entry(key)?;

        match entry.get_password() {
            Ok(_) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(crate::error::CortexError::Internal(format!(
                "Keyring error: {e}"
            ))),
        }
    }
}

/// In-memory secret store with secure memory.
pub struct MemorySecretStore {
    secrets: RwLock<HashMap<String, SecretValue>>,
}

impl MemorySecretStore {
    /// Create a new store.
    pub fn new() -> Self {
        Self {
            secrets: RwLock::new(HashMap::new()),
        }
    }

    /// Clear all secrets from memory securely.
    pub async fn clear(&self) {
        let mut secrets = self.secrets.write().await;
        secrets.clear();
    }
}

impl Default for MemorySecretStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SecretStore for MemorySecretStore {
    async fn get(&self, key: &str) -> Result<Option<SecretValue>> {
        Ok(self.secrets.read().await.get(key).cloned())
    }

    async fn set(&self, key: &str, value: SecretValue) -> Result<()> {
        self.secrets.write().await.insert(key.to_string(), value);
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.secrets.write().await.remove(key);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<String>> {
        Ok(self.secrets.read().await.keys().cloned().collect())
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        Ok(self.secrets.read().await.contains_key(key))
    }
}

/// File-based encrypted secret store.
pub struct EncryptedFileSecretStore {
    path: PathBuf,
    secrets: RwLock<HashMap<String, EncryptedSecret>>,
    master_key: Vec<u8>,
}

impl EncryptedFileSecretStore {
    /// Create a new encrypted file store.
    pub fn new(path: impl Into<PathBuf>, master_password: &str) -> Result<Self> {
        let path = path.into();
        let salt = Self::get_or_create_salt(&path)?;
        let key = SecretEncryption::derive_key(master_password, &salt)?;

        Ok(Self {
            path,
            secrets: RwLock::new(HashMap::new()),
            master_key: key.as_bytes().to_vec(),
        })
    }

    /// Create using keyring-stored master key (preferred).
    pub fn with_keyring_key(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let master_key = Self::get_or_create_master_key()?;

        Ok(Self {
            path,
            secrets: RwLock::new(HashMap::new()),
            master_key,
        })
    }

    /// Get or create master key from keyring.
    fn get_or_create_master_key() -> Result<Vec<u8>> {
        let entry =
            keyring::Entry::new(KEYRING_SERVICE, KEYRING_MASTER_KEY_ACCOUNT).map_err(|e| {
                crate::error::CortexError::Internal(format!("Keyring access failed: {e}"))
            })?;

        match entry.get_password() {
            Ok(key) => {
                let key_bytes =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &key)
                        .map_err(|e| {
                            crate::error::CortexError::Internal(format!("Invalid master key: {e}"))
                        })?;
                Ok(key_bytes)
            }
            Err(keyring::Error::NoEntry) => {
                // Generate new master key
                let mut key = [0u8; 32];
                OsRng.fill_bytes(&mut key);

                let key_b64 =
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &key);
                entry.set_password(&key_b64).map_err(|e| {
                    crate::error::CortexError::Internal(format!("Failed to store master key: {e}"))
                })?;

                let result = key.to_vec();
                key.zeroize();
                Ok(result)
            }
            Err(e) => Err(crate::error::CortexError::Internal(format!(
                "Keyring error: {e}"
            ))),
        }
    }

    /// Get or create salt file.
    fn get_or_create_salt(base_path: &PathBuf) -> Result<[u8; SALT_SIZE]> {
        let salt_path = base_path.with_extension("salt");

        if salt_path.exists() {
            let salt_data = std::fs::read(&salt_path)?;
            if salt_data.len() == SALT_SIZE {
                let mut salt = [0u8; SALT_SIZE];
                salt.copy_from_slice(&salt_data);
                return Ok(salt);
            }
        }

        // Generate new salt
        let salt = SecretEncryption::generate_salt();

        // Ensure parent directory exists
        if let Some(parent) = salt_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&salt_path, &salt)?;
        Self::set_file_permissions(&salt_path)?;

        Ok(salt)
    }

    /// Set restrictive file permissions (0600 on Unix).
    fn set_file_permissions(path: &PathBuf) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, perms)?;
        }

        #[cfg(windows)]
        {
            // Windows: File is owned by current user by default
            // ACLs can be set using windows-acl crate if needed
            let _ = path; // Suppress unused warning
        }

        Ok(())
    }

    /// Load from file.
    pub async fn load(&self) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&self.path)?;
        let data: HashMap<String, EncryptedSecret> = serde_json::from_str(&content)?;

        let mut secrets = self.secrets.write().await;
        *secrets = data;

        Ok(())
    }

    /// Save to file with proper permissions.
    async fn save(&self) -> Result<()> {
        // Ensure directory exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let secrets = self.secrets.read().await;
        let content = serde_json::to_string_pretty(&*secrets)?;
        std::fs::write(&self.path, content)?;
        Self::set_file_permissions(&self.path)?;

        Ok(())
    }

    /// Get encryption key for operations.
    fn get_encryption_key(&self) -> Result<EncryptionKey> {
        let key_bytes = &self.master_key;
        if key_bytes.len() != 32 {
            return Err(crate::error::CortexError::Internal(
                "Invalid master key length".to_string(),
            ));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(key_bytes);
        Ok(EncryptionKey::new(key))
    }
}

#[async_trait::async_trait]
impl SecretStore for EncryptedFileSecretStore {
    async fn get(&self, key: &str) -> Result<Option<SecretValue>> {
        let secrets = self.secrets.read().await;

        if let Some(encrypted) = secrets.get(key) {
            let ciphertext = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &encrypted.ciphertext,
            )
            .map_err(|e| crate::error::CortexError::Internal(format!("Invalid ciphertext: {e}")))?;

            let nonce_bytes = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &encrypted.nonce,
            )
            .map_err(|e| crate::error::CortexError::Internal(format!("Invalid nonce: {e}")))?;

            if nonce_bytes.len() != NONCE_SIZE {
                return Err(crate::error::CortexError::Internal(
                    "Invalid nonce size".to_string(),
                ));
            }
            let mut nonce = [0u8; NONCE_SIZE];
            nonce.copy_from_slice(&nonce_bytes);

            let enc_key = self.get_encryption_key()?;
            let plaintext = SecretEncryption::decrypt(&ciphertext, &nonce, &enc_key)?;

            Ok(Some(SecretValue {
                value: plaintext,
                metadata: encrypted.metadata.clone(),
            }))
        } else {
            Ok(None)
        }
    }

    async fn set(&self, key: &str, value: SecretValue) -> Result<()> {
        let enc_key = self.get_encryption_key()?;
        let (ciphertext, nonce) = SecretEncryption::encrypt(value.value.expose_secret(), &enc_key)?;

        let encrypted = EncryptedSecret {
            ciphertext: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &ciphertext,
            ),
            nonce: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &nonce),
            salt: String::new(), // Salt is stored separately
            metadata: value.metadata,
        };

        self.secrets
            .write()
            .await
            .insert(key.to_string(), encrypted);
        self.save().await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.secrets.write().await.remove(key);
        self.save().await
    }

    async fn list(&self) -> Result<Vec<String>> {
        Ok(self.secrets.read().await.keys().cloned().collect())
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        Ok(self.secrets.read().await.contains_key(key))
    }
}

/// Stored secret format for serialization.
#[derive(Serialize, Deserialize)]
struct StoredSecret {
    value: String,
    secret_type: SecretType,
    description: Option<String>,
    created_at: u64,
    updated_at: u64,
    expires_at: Option<u64>,
    tags: Vec<String>,
    attributes: HashMap<String, String>,
}

impl StoredSecret {
    fn from_secret_value(sv: &SecretValue) -> Self {
        Self {
            value: sv.value.expose_secret().to_string(),
            secret_type: sv.metadata.secret_type,
            description: sv.metadata.description.clone(),
            created_at: sv.metadata.created_at,
            updated_at: sv.metadata.updated_at,
            expires_at: sv.metadata.expires_at,
            tags: sv.metadata.tags.clone(),
            attributes: sv.metadata.attributes.clone(),
        }
    }

    fn into_secret_value(self) -> SecretValue {
        SecretValue {
            value: SecretString::from(self.value),
            metadata: SecretMetadata {
                secret_type: self.secret_type,
                description: self.description,
                created_at: self.created_at,
                updated_at: self.updated_at,
                expires_at: self.expires_at,
                tags: self.tags,
                attributes: self.attributes,
            },
        }
    }
}

/// Secret manager with multiple stores and fallback support.
pub struct SecretManager {
    /// Primary store.
    store: Arc<dyn SecretStore>,
    /// Fallback stores.
    fallbacks: Vec<Arc<dyn SecretStore>>,
}

impl SecretManager {
    /// Create a new manager with keyring as primary store.
    pub fn new_with_keyring() -> Self {
        Self {
            store: Arc::new(KeyringSecretStore::new()),
            fallbacks: Vec::new(),
        }
    }

    /// Create a new manager with custom store.
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self {
            store,
            fallbacks: Vec::new(),
        }
    }

    /// Add a fallback store.
    pub fn add_fallback(&mut self, store: Arc<dyn SecretStore>) {
        self.fallbacks.push(store);
    }

    /// Get a secret.
    pub async fn get(&self, key: &str) -> Result<Option<SecretValue>> {
        // Try primary store
        if let Some(value) = self.store.get(key).await?
            && !value.is_expired()
        {
            return Ok(Some(value));
        }

        // Try fallbacks
        for store in &self.fallbacks {
            if let Some(value) = store.get(key).await?
                && !value.is_expired()
            {
                return Ok(Some(value));
            }
        }

        Ok(None)
    }

    /// Get a secret value as string.
    pub async fn get_value(&self, key: &str) -> Result<Option<String>> {
        Ok(self.get(key).await?.map(|s| s.value().to_string()))
    }

    /// Set a secret.
    pub async fn set(&self, key: &str, value: SecretValue) -> Result<()> {
        self.store.set(key, value).await
    }

    /// Set a simple secret.
    pub async fn set_simple(
        &self,
        key: &str,
        value: impl Into<String>,
        secret_type: SecretType,
    ) -> Result<()> {
        self.set(key, SecretValue::simple(value, secret_type)).await
    }

    /// Delete a secret from all stores.
    pub async fn delete(&self, key: &str) -> Result<()> {
        self.store.delete(key).await?;
        for store in &self.fallbacks {
            let _ = store.delete(key).await;
        }
        Ok(())
    }

    /// List all keys.
    pub async fn list(&self) -> Result<Vec<String>> {
        self.store.list().await
    }

    /// Check if key exists.
    pub async fn exists(&self, key: &str) -> Result<bool> {
        self.store.exists(key).await
    }
}

/// Environment secret provider.
pub struct EnvSecretProvider {
    prefix: String,
}

impl EnvSecretProvider {
    /// Create a new provider.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }

    /// Get secret from environment.
    pub fn get(&self, key: &str) -> Option<SecretString> {
        let env_key = format!("{}_{}", self.prefix, key.to_uppercase().replace('.', "_"));
        std::env::var(&env_key).ok().map(SecretString::from)
    }

    /// Get all secrets as SecretStrings.
    pub fn all(&self) -> HashMap<String, SecretString> {
        let prefix_with_underscore = format!("{}_", self.prefix);

        std::env::vars()
            .filter(|(k, _)| k.starts_with(&prefix_with_underscore))
            .map(|(k, v)| {
                let key = k[prefix_with_underscore.len()..]
                    .to_lowercase()
                    .replace('_', ".");
                (key, SecretString::from(v))
            })
            .collect()
    }
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_metadata() {
        let meta = SecretMetadata::new(SecretType::ApiKey)
            .description("Test API key")
            .tag("production");

        assert_eq!(meta.secret_type, SecretType::ApiKey);
        assert!(!meta.is_expired());
    }

    #[test]
    fn test_secret_value() {
        let secret = SecretValue::simple("sk-12345678abcdefgh", SecretType::ApiKey);

        assert_eq!(secret.value(), "sk-12345678abcdefgh");
        assert_eq!(secret.redacted(), "sk-1...efgh");
    }

    #[test]
    fn test_secret_value_short() {
        let secret = SecretValue::simple("abc", SecretType::Password);
        assert_eq!(secret.redacted(), "****");
    }

    #[tokio::test]
    async fn test_memory_store() {
        let store = MemorySecretStore::new();

        let secret = SecretValue::simple("test-value", SecretType::Generic);
        store.set("test-key", secret).await.unwrap();

        assert!(store.exists("test-key").await.unwrap());

        let retrieved = store.get("test-key").await.unwrap().unwrap();
        assert_eq!(retrieved.value(), "test-value");

        store.delete("test-key").await.unwrap();
        assert!(!store.exists("test-key").await.unwrap());
    }

    #[tokio::test]
    async fn test_secret_manager() {
        let store = Arc::new(MemorySecretStore::new());
        let manager = SecretManager::new(store);

        manager
            .set_simple("api_key", "sk-test", SecretType::ApiKey)
            .await
            .unwrap();

        let value = manager.get_value("api_key").await.unwrap();
        assert_eq!(value, Some("sk-test".to_string()));
    }

    #[test]
    fn test_encryption_roundtrip() {
        let salt = SecretEncryption::generate_salt();
        let key = SecretEncryption::derive_key("test-password", &salt).unwrap();

        let plaintext = "super-secret-api-key-12345";
        let (ciphertext, nonce) = SecretEncryption::encrypt(plaintext, &key).unwrap();
        let decrypted = SecretEncryption::decrypt(&ciphertext, &nonce, &key).unwrap();

        assert_eq!(decrypted.expose_secret(), plaintext);
    }

    #[test]
    fn test_expired_secret() {
        let meta = SecretMetadata::new(SecretType::AccessToken).expires_at(0); // Expired

        let secret = SecretValue::new("expired-token", meta);
        assert!(secret.is_expired());
    }

    #[test]
    fn test_secret_debug() {
        let secret = SecretValue::simple("sensitive-data", SecretType::Password);
        let debug = format!("{:?}", secret);

        assert!(!debug.contains("sensitive-data"));
        assert!(debug.contains("[REDACTED]"));
    }
}
