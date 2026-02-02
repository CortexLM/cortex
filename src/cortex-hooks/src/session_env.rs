//! Session environment file management.
//!
//! This module provides persistent environment variables per session,
//! allowing hooks to share state and communicate through environment variables.
//!
//! The env file is stored in the system cache directory and can be sourced
//! by hooks or read programmatically.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;

/// Manages environment variables that persist within a session.
///
/// Variables set here will be available to all subsequent hooks
/// within the same session.
pub struct SessionEnvFile {
    /// Path to the env file.
    path: PathBuf,
    /// Cached environment variables.
    vars: HashMap<String, String>,
}

impl SessionEnvFile {
    /// Create a new session env file manager.
    pub async fn new(session_id: &str) -> Result<Self, std::io::Error> {
        let path = Self::env_file_path(session_id)?;

        // Create the directory structure if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).await?;
            }
        }

        // Create the file if it doesn't exist
        if !path.exists() {
            fs::write(&path, "").await?;
        }

        let mut env_file = Self {
            path,
            vars: HashMap::new(),
        };

        // Load existing variables
        env_file.load().await?;

        Ok(env_file)
    }

    /// Get the path to the env file for a session.
    fn env_file_path(session_id: &str) -> Result<PathBuf, std::io::Error> {
        let cache_dir = dirs::cache_dir().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "Cache directory not found")
        })?;

        // Sanitize session_id to be safe for file paths
        let safe_session_id: String = session_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();

        Ok(cache_dir
            .join("cortex")
            .join("sessions")
            .join(safe_session_id)
            .join("env"))
    }

    /// Load variables from the env file.
    pub async fn load(&mut self) -> Result<(), std::io::Error> {
        let content = fs::read_to_string(&self.path).await?;
        self.vars.clear();

        for line in content.lines() {
            let line = line.trim();

            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Parse export statements and simple assignments
            let line = line.strip_prefix("export ").unwrap_or(line);

            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();

                // Remove quotes if present
                let value = value
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .unwrap_or(value);
                let value = value
                    .strip_prefix('\'')
                    .and_then(|v| v.strip_suffix('\''))
                    .unwrap_or(value);

                self.vars.insert(key.to_string(), value.to_string());
            }
        }

        Ok(())
    }

    /// Set an environment variable.
    pub async fn set(&mut self, key: &str, value: &str) -> Result<(), std::io::Error> {
        self.vars.insert(key.to_string(), value.to_string());
        self.persist().await
    }

    /// Get an environment variable.
    pub fn get(&self, key: &str) -> Option<&String> {
        self.vars.get(key)
    }

    /// Remove an environment variable.
    pub async fn remove(&mut self, key: &str) -> Result<Option<String>, std::io::Error> {
        let removed = self.vars.remove(key);
        if removed.is_some() {
            self.persist().await?;
        }
        Ok(removed)
    }

    /// Clear all environment variables.
    pub async fn clear(&mut self) -> Result<(), std::io::Error> {
        self.vars.clear();
        self.persist().await
    }

    /// Persist the variables to the env file.
    async fn persist(&self) -> Result<(), std::io::Error> {
        let content: String = self
            .vars
            .iter()
            .map(|(k, v)| {
                // Quote values that contain spaces or special characters
                if v.contains(' ') || v.contains('$') || v.contains('`') {
                    format!("export {}=\"{}\"\n", k, v.replace('"', "\\\""))
                } else {
                    format!("export {}={}\n", k, v)
                }
            })
            .collect();

        fs::write(&self.path, content).await
    }

    /// Get all variables as a HashMap.
    pub fn as_env(&self) -> HashMap<String, String> {
        self.vars.clone()
    }

    /// Get the path to the env file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Check if a variable exists.
    pub fn contains(&self, key: &str) -> bool {
        self.vars.contains_key(key)
    }

    /// Get the number of variables.
    pub fn len(&self) -> usize {
        self.vars.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }
}

/// Delete the session env file.
pub async fn delete_session_env(session_id: &str) -> Result<(), std::io::Error> {
    let path = SessionEnvFile::env_file_path(session_id)?;
    if path.exists() {
        fs::remove_file(&path).await?;
    }
    // Also try to clean up the session directory if empty
    if let Some(parent) = path.parent() {
        let _ = fs::remove_dir(parent).await; // Ignore errors (directory might not be empty)
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Counter for unique session IDs in tests
    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_session_id() -> String {
        let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        format!("test-session-{}-{}", std::process::id(), count)
    }

    #[tokio::test]
    async fn test_session_env_set_and_get() {
        let session_id = unique_session_id();
        let mut env = SessionEnvFile::new(&session_id).await.unwrap();

        env.set("MY_VAR", "my_value").await.unwrap();
        assert_eq!(env.get("MY_VAR"), Some(&"my_value".to_string()));

        // Clean up
        delete_session_env(&session_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_session_env_persistence() {
        let session_id = unique_session_id();

        // Create and set variable
        {
            let mut env = SessionEnvFile::new(&session_id).await.unwrap();
            env.set("PERSISTENT_VAR", "persistent_value").await.unwrap();
        }

        // Load in new instance
        {
            let env = SessionEnvFile::new(&session_id).await.unwrap();
            assert_eq!(
                env.get("PERSISTENT_VAR"),
                Some(&"persistent_value".to_string())
            );
        }

        // Clean up
        delete_session_env(&session_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_session_env_remove() {
        let session_id = unique_session_id();
        let mut env = SessionEnvFile::new(&session_id).await.unwrap();

        env.set("TO_REMOVE", "value").await.unwrap();
        assert!(env.contains("TO_REMOVE"));

        let removed = env.remove("TO_REMOVE").await.unwrap();
        assert_eq!(removed, Some("value".to_string()));
        assert!(!env.contains("TO_REMOVE"));

        // Clean up
        delete_session_env(&session_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_session_env_clear() {
        let session_id = unique_session_id();
        let mut env = SessionEnvFile::new(&session_id).await.unwrap();

        env.set("VAR1", "value1").await.unwrap();
        env.set("VAR2", "value2").await.unwrap();
        assert_eq!(env.len(), 2);

        env.clear().await.unwrap();
        assert!(env.is_empty());

        // Clean up
        delete_session_env(&session_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_session_env_special_characters() {
        let session_id = unique_session_id();
        let mut env = SessionEnvFile::new(&session_id).await.unwrap();

        // Value with spaces
        env.set("SPACED", "value with spaces").await.unwrap();

        // Reload and verify
        let env2 = SessionEnvFile::new(&session_id).await.unwrap();
        assert_eq!(env2.get("SPACED"), Some(&"value with spaces".to_string()));

        // Clean up
        delete_session_env(&session_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_session_env_as_env() {
        let session_id = unique_session_id();
        let mut env = SessionEnvFile::new(&session_id).await.unwrap();

        env.set("KEY1", "value1").await.unwrap();
        env.set("KEY2", "value2").await.unwrap();

        let vars = env.as_env();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars.get("KEY1"), Some(&"value1".to_string()));
        assert_eq!(vars.get("KEY2"), Some(&"value2".to_string()));

        // Clean up
        delete_session_env(&session_id).await.unwrap();
    }
}
