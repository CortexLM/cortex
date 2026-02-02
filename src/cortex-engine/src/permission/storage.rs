//! Permission storage for Cortex CLI.
//!
//! Handles persistence of permissions to `~/.cortex/permissions.json`
//! and manages session-scoped permissions in memory.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::types::{Permission, PermissionResponse, PermissionScope};
use crate::error::{CortexError, Result};

/// Storage format for persisted permissions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionStore {
    /// Version of the permission store format.
    pub version: u32,
    /// Persisted permissions (scope = Always).
    pub permissions: Vec<Permission>,
}

impl PermissionStore {
    /// Current storage version.
    pub const CURRENT_VERSION: u32 = 1;

    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            permissions: Vec::new(),
        }
    }

    /// Add a permission.
    pub fn add(&mut self, permission: Permission) {
        // Remove any existing permission for the same tool/pattern
        self.permissions
            .retain(|p| p.tool != permission.tool || p.pattern != permission.pattern);
        self.permissions.push(permission);
    }

    /// Remove a permission.
    pub fn remove(&mut self, tool: &str, pattern: &str) {
        self.permissions
            .retain(|p| p.tool != tool || p.pattern != pattern);
    }

    /// Find a permission by tool and pattern.
    pub fn find(&self, tool: &str, pattern: &str) -> Option<&Permission> {
        self.permissions
            .iter()
            .find(|p| p.tool == tool && p.pattern == pattern)
    }

    /// Get all permissions for a tool.
    pub fn for_tool(&self, tool: &str) -> Vec<&Permission> {
        self.permissions.iter().filter(|p| p.tool == tool).collect()
    }
}

/// Permission storage manager.
pub struct PermissionStorage {
    /// Path to the permission store file.
    store_path: PathBuf,
    /// Persisted permissions.
    persistent: Arc<RwLock<PermissionStore>>,
    /// Session-scoped permissions (in-memory only).
    session: Arc<RwLock<HashMap<String, Permission>>>,
}

impl PermissionStorage {
    /// Create a new permission storage with default path.
    pub fn new() -> Self {
        let store_path = Self::default_store_path();
        Self {
            store_path,
            persistent: Arc::new(RwLock::new(PermissionStore::new())),
            session: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create with a custom store path.
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            store_path: path.into(),
            persistent: Arc::new(RwLock::new(PermissionStore::new())),
            session: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get the default store path (~/.cortex/permissions.json).
    pub fn default_store_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cortex")
            .join("permissions.json")
    }

    /// Load permissions from disk.
    pub async fn load(&self) -> Result<()> {
        if !self.store_path.exists() {
            return Ok(());
        }

        let content = tokio::fs::read_to_string(&self.store_path).await?;
        let store: PermissionStore = serde_json::from_str(&content)
            .map_err(|e| CortexError::Config(format!("Failed to parse permissions file: {}", e)))?;

        *self.persistent.write().await = store;
        Ok(())
    }

    /// Save permissions to disk.
    pub async fn save(&self) -> Result<()> {
        // Ensure directory exists
        if let Some(parent) = self.store_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let store = self.persistent.read().await;
        let content = serde_json::to_string_pretty(&*store).map_err(|e| {
            CortexError::Serialization(format!("Failed to serialize permissions: {}", e))
        })?;

        tokio::fs::write(&self.store_path, content).await?;
        Ok(())
    }

    /// Grant a permission.
    pub async fn grant(&self, permission: Permission) -> Result<()> {
        let key = Self::make_key(&permission.tool, &permission.pattern);

        match permission.scope {
            PermissionScope::Once => {
                // Once permissions are not stored
            }
            PermissionScope::Session => {
                self.session.write().await.insert(key, permission);
            }
            PermissionScope::Always => {
                self.persistent.write().await.add(permission);
                self.save().await?;
            }
        }

        Ok(())
    }

    /// Deny a permission (creates a deny entry).
    pub async fn deny(&self, tool: &str, pattern: &str, scope: PermissionScope) -> Result<()> {
        let permission = Permission::new(tool, pattern, PermissionResponse::Deny, scope);
        self.grant(permission).await
    }

    /// Revoke a permission.
    pub async fn revoke(&self, tool: &str, pattern: &str) -> Result<()> {
        let key = Self::make_key(tool, pattern);

        // Remove from session
        self.session.write().await.remove(&key);

        // Remove from persistent
        self.persistent.write().await.remove(tool, pattern);
        self.save().await?;

        Ok(())
    }

    /// Check for a stored permission.
    pub async fn check(&self, tool: &str, pattern: &str) -> Option<Permission> {
        let key = Self::make_key(tool, pattern);

        // Check session first (takes precedence)
        if let Some(perm) = self.session.read().await.get(&key) {
            return Some(perm.clone());
        }

        // Check persistent
        if let Some(perm) = self.persistent.read().await.find(tool, pattern) {
            return Some(perm.clone());
        }

        None
    }

    /// List all permissions.
    pub async fn list(&self) -> Vec<Permission> {
        let mut permissions = Vec::new();

        // Add session permissions
        for perm in self.session.read().await.values() {
            permissions.push(perm.clone());
        }

        // Add persistent permissions
        for perm in &self.persistent.read().await.permissions {
            permissions.push(perm.clone());
        }

        permissions
    }

    /// List permissions for a specific tool.
    pub async fn list_for_tool(&self, tool: &str) -> Vec<Permission> {
        let mut permissions = Vec::new();

        // Add session permissions
        for perm in self.session.read().await.values() {
            if perm.tool == tool {
                permissions.push(perm.clone());
            }
        }

        // Add persistent permissions
        for perm in self.persistent.read().await.for_tool(tool) {
            permissions.push(perm.clone());
        }

        permissions
    }

    /// Clear all session permissions.
    pub async fn clear_session(&self) {
        self.session.write().await.clear();
    }

    /// Clear all permissions (both session and persistent).
    pub async fn clear_all(&self) -> Result<()> {
        self.session.write().await.clear();
        *self.persistent.write().await = PermissionStore::new();
        self.save().await
    }

    /// Get the store path.
    pub fn store_path(&self) -> &PathBuf {
        &self.store_path
    }

    /// Create a key from tool and pattern.
    fn make_key(tool: &str, pattern: &str) -> String {
        format!("{}:{}", tool, pattern)
    }
}

impl Default for PermissionStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for PermissionStorage {
    fn clone(&self) -> Self {
        Self {
            store_path: self.store_path.clone(),
            persistent: Arc::clone(&self.persistent),
            session: Arc::clone(&self.session),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_permission_store() {
        let mut store = PermissionStore::new();

        let perm = Permission::new(
            "bash",
            "git diff*",
            PermissionResponse::Allow,
            PermissionScope::Always,
        );
        store.add(perm);

        assert_eq!(store.permissions.len(), 1);
        assert!(store.find("bash", "git diff*").is_some());
    }

    #[tokio::test]
    async fn test_storage_session() {
        let storage = PermissionStorage::new();

        let perm = Permission::new(
            "bash",
            "test*",
            PermissionResponse::Allow,
            PermissionScope::Session,
        );
        storage.grant(perm).await.unwrap();

        let found = storage.check("bash", "test*").await;
        assert!(found.is_some());
        assert!(found.unwrap().allows());
    }

    #[tokio::test]
    async fn test_storage_persistent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("permissions.json");
        let storage = PermissionStorage::with_path(&path);

        let perm = Permission::new(
            "edit",
            "*.rs",
            PermissionResponse::Allow,
            PermissionScope::Always,
        );
        storage.grant(perm).await.unwrap();

        // Verify file was created
        assert!(path.exists());

        // Load in new storage instance
        let storage2 = PermissionStorage::with_path(&path);
        storage2.load().await.unwrap();

        let found = storage2.check("edit", "*.rs").await;
        assert!(found.is_some());
    }

    #[tokio::test]
    async fn test_storage_revoke() {
        let storage = PermissionStorage::new();

        let perm = Permission::new(
            "bash",
            "test*",
            PermissionResponse::Allow,
            PermissionScope::Session,
        );
        storage.grant(perm).await.unwrap();

        assert!(storage.check("bash", "test*").await.is_some());

        storage.revoke("bash", "test*").await.unwrap();

        assert!(storage.check("bash", "test*").await.is_none());
    }
}
