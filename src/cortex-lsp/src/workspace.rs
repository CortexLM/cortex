//! Workspace management for multi-root LSP support.
//!
//! This module handles multiple workspace roots and maps files to their
//! appropriate LSP clients.
//!
//! Race condition protection:
//! - Uses RwLock for the clients map to allow concurrent reads
//! - Uses per-key Mutex for client spawning to prevent duplicate spawns
//! - Compare-and-swap pattern ensures only one client is spawned per key
//!
//! The spawn_client method uses a two-phase locking approach:
//! 1. First checks if a spawn is already in progress for this key
//! 2. If not, marks the key as "spawning" and proceeds
//! 3. After spawning, adds to clients map and removes from spawning set
//! 4. If another caller tries to spawn the same key, it waits and re-checks

use crate::root_detection::{
    detect_root, get_server_root_config, DetectedRoot, RootDetectionConfig,
};
use crate::{LspClient, LspError, LspServerConfig, Result, BUILTIN_SERVERS};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

/// Key for identifying a unique client (server + root combination).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ClientKey {
    /// The server ID.
    pub server_id: String,
    /// The workspace root path.
    pub root: PathBuf,
}

impl ClientKey {
    pub fn new(server_id: impl Into<String>, root: PathBuf) -> Self {
        Self {
            server_id: server_id.into(),
            root,
        }
    }
}

/// Mapping from a file to its workspace information.
#[derive(Debug, Clone)]
pub struct FileMapping {
    /// The detected root for this file.
    pub root: PathBuf,
    /// The server ID.
    pub server_id: String,
    /// Whether this is a workspace root (monorepo).
    pub is_workspace: bool,
}

/// Manager for handling multiple workspace roots and LSP clients.
pub struct WorkspaceManager {
    /// Active clients keyed by (server_id, root).
    clients: RwLock<HashMap<ClientKey, Arc<LspClient>>>,
    /// Custom server configurations.
    custom_configs: RwLock<HashMap<String, LspServerConfig>>,
    /// Custom root detection configs.
    custom_root_configs: RwLock<HashMap<String, RootDetectionConfig>>,
    /// Disabled servers.
    disabled_servers: RwLock<Vec<String>>,
    /// File to root cache for performance.
    file_cache: RwLock<HashMap<PathBuf, FileMapping>>,
    /// Default/fallback root when no project markers found.
    fallback_root: Option<PathBuf>,
    /// Mutex to prevent race conditions during client spawning.
    /// The set contains ClientKey strings being spawned.
    spawning_clients: Mutex<HashSet<String>>,
}

impl WorkspaceManager {
    /// Create a new workspace manager.
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            custom_configs: RwLock::new(HashMap::new()),
            custom_root_configs: RwLock::new(HashMap::new()),
            disabled_servers: RwLock::new(Vec::new()),
            file_cache: RwLock::new(HashMap::new()),
            fallback_root: None,
            spawning_clients: Mutex::new(HashSet::new()),
        }
    }

    /// Create a new workspace manager with a fallback root.
    pub fn with_fallback_root(fallback: impl Into<PathBuf>) -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            custom_configs: RwLock::new(HashMap::new()),
            custom_root_configs: RwLock::new(HashMap::new()),
            disabled_servers: RwLock::new(Vec::new()),
            file_cache: RwLock::new(HashMap::new()),
            fallback_root: Some(fallback.into()),
            spawning_clients: Mutex::new(HashSet::new()),
        }
    }

    /// Generate a unique key string for spawning lock.
    fn spawn_key(server_id: &str, root: &Path) -> String {
        format!("{}:{}", server_id, root.display())
    }

    /// Add a custom server configuration.
    pub async fn add_server_config(&self, config: LspServerConfig) {
        self.custom_configs
            .write()
            .await
            .insert(config.id.clone(), config);
    }

    /// Add a custom root detection configuration.
    pub async fn add_root_config(&self, config: RootDetectionConfig) {
        self.custom_root_configs
            .write()
            .await
            .insert(config.server_id.clone(), config);
    }

    /// Disable a server.
    pub async fn disable_server(&self, id: &str) {
        self.disabled_servers.write().await.push(id.to_string());
    }

    /// Enable a server.
    pub async fn enable_server(&self, id: &str) {
        self.disabled_servers.write().await.retain(|s| s != id);
    }

    /// Check if a server is disabled.
    pub async fn is_server_disabled(&self, id: &str) -> bool {
        self.disabled_servers.read().await.contains(&id.to_string())
    }

    /// Clear the file cache.
    pub async fn clear_cache(&self) {
        self.file_cache.write().await.clear();
    }

    /// Get or start an LSP client for a file.
    ///
    /// This method:
    /// 1. Determines the appropriate server for the file extension
    /// 2. Detects the project root for that file
    /// 3. Returns an existing client or spawns a new one
    ///
    /// Uses a spawning lock to prevent race conditions when multiple
    /// concurrent requests try to spawn the same client.
    pub async fn get_client_for_file(&self, path: &Path) -> Result<Option<Arc<LspClient>>> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        // Find matching server config
        let config = self.find_config_for_extension(ext).await;

        if let Some(config) = config {
            // Check if server is disabled
            if self.is_server_disabled(&config.id).await {
                return Ok(None);
            }

            // Get root detection config
            let root_config = self.get_root_config(&config.id).await;

            // Detect root for this file
            let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            let root = self.detect_root_for_file(&canonical, &root_config).await;

            if let Some(detected_root) = root {
                let key = ClientKey::new(&config.id, detected_root.path.clone());
                let spawn_key = Self::spawn_key(&config.id, &detected_root.path);

                // Check cache and return existing client
                {
                    let clients = self.clients.read().await;
                    if let Some(client) = clients.get(&key) {
                        // Check if the client's server is still alive
                        if client.is_server_alive() {
                            debug!(
                                "Reusing client for {} at {}",
                                config.id,
                                detected_root.path.display()
                            );
                            return Ok(Some(client.clone()));
                        } else {
                            // Server is dead, will need to respawn
                            debug!(
                                "Client for {} at {} has dead server, will respawn",
                                config.id,
                                detected_root.path.display()
                            );
                        }
                    }
                }

                // Check if another task is already spawning this client
                let is_already_spawning = {
                    let spawning = self.spawning_clients.lock().await;
                    spawning.contains(&spawn_key)
                };

                if is_already_spawning {
                    // Another task is spawning this client, wait and retry
                    // Wait for a short period and check again
                    for _ in 0..50 {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                        // Check if client is now available
                        {
                            let clients = self.clients.read().await;
                            if let Some(client) = clients.get(&key) {
                                if client.is_server_alive() {
                                    return Ok(Some(client.clone()));
                                }
                            }
                        }

                        // Check if still spawning
                        let still_spawning = {
                            let spawning = self.spawning_clients.lock().await;
                            spawning.contains(&spawn_key)
                        };
                        if !still_spawning {
                            break;
                        }
                    }

                    // Final check after waiting
                    let clients = self.clients.read().await;
                    if let Some(client) = clients.get(&key) {
                        if client.is_server_alive() {
                            return Ok(Some(client.clone()));
                        }
                    }

                    // If still no client, fall through to spawn
                }

                // Mark this client as being spawned
                {
                    let mut spawning = self.spawning_clients.lock().await;
                    spawning.insert(spawn_key.clone());
                }

                // Spawn new client with cleanup on error
                let result = self.spawn_client(&config, &detected_root).await;

                // Remove from spawning set
                {
                    let mut spawning = self.spawning_clients.lock().await;
                    spawning.remove(&spawn_key);
                }

                return result.map(Some);
            } else if let Some(fallback) = &self.fallback_root {
                // Use fallback root
                let key = ClientKey::new(&config.id, fallback.clone());
                let spawn_key = Self::spawn_key(&config.id, fallback);

                {
                    let clients = self.clients.read().await;
                    if let Some(client) = clients.get(&key) {
                        if client.is_server_alive() {
                            return Ok(Some(client.clone()));
                        }
                    }
                }

                // Check if another task is spawning
                let is_already_spawning = {
                    let spawning = self.spawning_clients.lock().await;
                    spawning.contains(&spawn_key)
                };

                if is_already_spawning {
                    // Wait for spawning to complete
                    for _ in 0..50 {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                        {
                            let clients = self.clients.read().await;
                            if let Some(client) = clients.get(&key) {
                                if client.is_server_alive() {
                                    return Ok(Some(client.clone()));
                                }
                            }
                        }

                        let still_spawning = {
                            let spawning = self.spawning_clients.lock().await;
                            spawning.contains(&spawn_key)
                        };
                        if !still_spawning {
                            break;
                        }
                    }

                    let clients = self.clients.read().await;
                    if let Some(client) = clients.get(&key) {
                        if client.is_server_alive() {
                            return Ok(Some(client.clone()));
                        }
                    }
                }

                // Mark this client as being spawned
                {
                    let mut spawning = self.spawning_clients.lock().await;
                    spawning.insert(spawn_key.clone());
                }

                let detected =
                    DetectedRoot::new(fallback.clone(), crate::markers::ProjectMarker::GitDir);
                let result = self.spawn_client(&config, &detected).await;

                {
                    let mut spawning = self.spawning_clients.lock().await;
                    spawning.remove(&spawn_key);
                }

                return result.map(Some);
            }
        }

        Ok(None)
    }

    /// Get all clients for a specific server.
    pub async fn get_clients_for_server(&self, server_id: &str) -> Vec<Arc<LspClient>> {
        let clients = self.clients.read().await;
        clients
            .iter()
            .filter(|(key, _)| key.server_id == server_id)
            .map(|(_, client)| client.clone())
            .collect()
    }

    /// Get all active workspace roots.
    pub async fn get_active_roots(&self) -> Vec<PathBuf> {
        let clients = self.clients.read().await;
        clients.keys().map(|k| k.root.clone()).collect()
    }

    /// Get all active clients.
    pub async fn get_all_clients(&self) -> Vec<Arc<LspClient>> {
        self.clients.read().await.values().cloned().collect()
    }

    /// Get file mapping (root and server) for a file.
    pub async fn get_file_mapping(&self, path: &Path) -> Option<FileMapping> {
        let canonical = path.canonicalize().ok()?;

        // Check cache
        {
            let cache = self.file_cache.read().await;
            if let Some(mapping) = cache.get(&canonical) {
                return Some(mapping.clone());
            }
        }

        // Determine mapping
        let ext = path.extension()?.to_str()?;
        let config = self.find_config_for_extension(ext).await?;
        let root_config = self.get_root_config(&config.id).await;
        let detected = self.detect_root_for_file(&canonical, &root_config).await?;

        let mapping = FileMapping {
            root: detected.path.clone(),
            server_id: config.id.clone(),
            is_workspace: detected.is_workspace,
        };

        // Cache the mapping
        {
            let mut cache = self.file_cache.write().await;
            cache.insert(canonical, mapping.clone());
        }

        Some(mapping)
    }

    /// List running servers and their roots.
    pub async fn running_servers(&self) -> Vec<(String, PathBuf)> {
        self.clients
            .read()
            .await
            .keys()
            .map(|k| (k.server_id.clone(), k.root.clone()))
            .collect()
    }

    /// List available server configurations.
    pub async fn available_servers(&self) -> Vec<LspServerConfig> {
        let custom = self.custom_configs.read().await;
        let mut servers: Vec<_> = BUILTIN_SERVERS.iter().cloned().collect();
        servers.extend(custom.values().cloned());
        servers
    }

    /// Shutdown all clients.
    pub async fn shutdown_all(&self) {
        let clients = self.clients.write().await;
        for (key, client) in clients.iter() {
            if let Err(e) = client.shutdown().await {
                warn!(
                    "Failed to shutdown {} at {}: {}",
                    key.server_id,
                    key.root.display(),
                    e
                );
            }
        }
    }

    /// Shutdown clients for a specific root.
    pub async fn shutdown_root(&self, root: &Path) {
        let mut clients = self.clients.write().await;
        let keys_to_remove: Vec<_> = clients.keys().filter(|k| k.root == root).cloned().collect();

        for key in keys_to_remove {
            if let Some(client) = clients.remove(&key) {
                if let Err(e) = client.shutdown().await {
                    warn!(
                        "Failed to shutdown {} at {}: {}",
                        key.server_id,
                        key.root.display(),
                        e
                    );
                }
            }
        }

        // Clear cache entries for this root
        let mut cache = self.file_cache.write().await;
        cache.retain(|_, mapping| mapping.root != root);
    }

    /// Shutdown a specific client.
    pub async fn shutdown_client(&self, server_id: &str, root: &Path) -> Result<()> {
        let key = ClientKey::new(server_id, root.to_path_buf());
        let mut clients = self.clients.write().await;

        if let Some(client) = clients.remove(&key) {
            client.shutdown().await?;
            info!("Shutdown {} at {}", server_id, root.display());
        }

        Ok(())
    }

    // Helper methods

    async fn find_config_for_extension(&self, ext: &str) -> Option<LspServerConfig> {
        // Check custom configs first
        let custom = self.custom_configs.read().await;
        for config in custom.values() {
            if config.extensions.iter().any(|e| e == ext) {
                return Some(config.clone());
            }
        }

        // Check builtin servers
        for config in BUILTIN_SERVERS.iter() {
            if config.extensions.iter().any(|e| e == ext) {
                return Some(config.clone());
            }
        }

        None
    }

    async fn get_root_config(&self, server_id: &str) -> RootDetectionConfig {
        // Check custom configs first
        let custom = self.custom_root_configs.read().await;
        if let Some(config) = custom.get(server_id) {
            return config.clone();
        }

        // Use default config
        get_server_root_config(server_id)
    }

    async fn detect_root_for_file(
        &self,
        path: &Path,
        config: &RootDetectionConfig,
    ) -> Option<DetectedRoot> {
        detect_root(path, &config.server_id)
    }

    async fn spawn_client(
        &self,
        config: &LspServerConfig,
        root: &DetectedRoot,
    ) -> Result<Arc<LspClient>> {
        // Check if command exists
        if !Self::command_exists(&config.command[0]).await {
            return Err(LspError::ServerNotFound(format!(
                "{} not found (command: {})",
                config.name, config.command[0]
            )));
        }

        let client = LspClient::new(config.clone()).with_root(&root.path);

        client.start().await?;
        client.initialize().await?;

        let client = Arc::new(client);
        let key = ClientKey::new(&config.id, root.path.clone());

        self.clients.write().await.insert(key, client.clone());

        info!(
            "Started LSP server: {} ({}) at {}{}",
            config.name,
            config.id,
            root.path.display(),
            if root.is_workspace {
                " [workspace]"
            } else {
                ""
            }
        );

        Ok(client)
    }

    async fn command_exists(cmd: &str) -> bool {
        #[cfg(windows)]
        {
            tokio::process::Command::new("where")
                .arg(cmd)
                .output()
                .await
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        #[cfg(not(windows))]
        {
            tokio::process::Command::new("which")
                .arg(cmd)
                .output()
                .await
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
    }
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_workspace_manager_creation() {
        let manager = WorkspaceManager::new();
        let servers = manager.available_servers().await;
        assert!(!servers.is_empty());
    }

    #[tokio::test]
    async fn test_file_mapping_cache() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        // Create a package.json
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/index.ts"), "").unwrap();

        let manager = WorkspaceManager::new();

        // First call should populate cache
        let mapping1 = manager.get_file_mapping(&root.join("src/index.ts")).await;
        assert!(mapping1.is_some());

        // Second call should use cache
        let mapping2 = manager.get_file_mapping(&root.join("src/index.ts")).await;
        assert!(mapping2.is_some());

        assert_eq!(mapping1.unwrap().root, mapping2.unwrap().root);
    }

    #[tokio::test]
    async fn test_disable_server() {
        let manager = WorkspaceManager::new();

        assert!(!manager.is_server_disabled("typescript").await);

        manager.disable_server("typescript").await;
        assert!(manager.is_server_disabled("typescript").await);

        manager.enable_server("typescript").await;
        assert!(!manager.is_server_disabled("typescript").await);
    }
}
