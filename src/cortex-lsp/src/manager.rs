//! LSP manager for handling multiple language servers.
//!
//! This module provides two modes of operation:
//! - Single root mode (legacy): Uses a fixed root for all clients.
//! - Multi-root mode: Uses per-file root detection for monorepo support.

use crate::root_detection::detect_root;
use crate::workspace::WorkspaceManager;
use crate::{Diagnostic, LspClient, LspError, LspServerConfig, Result, BUILTIN_SERVERS};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Manager for multiple LSP clients.
///
/// Supports both single-root (legacy) and multi-root (monorepo) modes.
pub struct LspManager {
    /// Legacy single root (used when workspace_manager is None).
    root: PathBuf,
    /// Legacy clients keyed by server ID.
    clients: RwLock<HashMap<String, Arc<LspClient>>>,
    /// Custom server configurations.
    custom_configs: RwLock<HashMap<String, LspServerConfig>>,
    /// Disabled servers.
    disabled_servers: RwLock<Vec<String>>,
    /// Multi-root workspace manager (optional).
    workspace_manager: Option<WorkspaceManager>,
    /// Whether to use multi-root mode.
    multi_root_enabled: bool,
}

impl LspManager {
    /// Create a new LSP manager with a single root (legacy mode).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            clients: RwLock::new(HashMap::new()),
            custom_configs: RwLock::new(HashMap::new()),
            disabled_servers: RwLock::new(Vec::new()),
            workspace_manager: None,
            multi_root_enabled: false,
        }
    }

    /// Create a new LSP manager with multi-root support.
    pub fn with_multi_root(fallback_root: impl Into<PathBuf>) -> Self {
        let fallback = fallback_root.into();
        Self {
            root: fallback.clone(),
            clients: RwLock::new(HashMap::new()),
            custom_configs: RwLock::new(HashMap::new()),
            disabled_servers: RwLock::new(Vec::new()),
            workspace_manager: Some(WorkspaceManager::with_fallback_root(fallback)),
            multi_root_enabled: true,
        }
    }

    /// Enable multi-root mode on an existing manager.
    pub fn enable_multi_root(&mut self) {
        if self.workspace_manager.is_none() {
            self.workspace_manager = Some(WorkspaceManager::with_fallback_root(&self.root));
        }
        self.multi_root_enabled = true;
    }

    /// Disable multi-root mode.
    pub fn disable_multi_root(&mut self) {
        self.multi_root_enabled = false;
    }

    /// Check if multi-root mode is enabled.
    pub fn is_multi_root_enabled(&self) -> bool {
        self.multi_root_enabled && self.workspace_manager.is_some()
    }

    /// Get the workspace manager (if in multi-root mode).
    pub fn workspace_manager(&self) -> Option<&WorkspaceManager> {
        if self.multi_root_enabled {
            self.workspace_manager.as_ref()
        } else {
            None
        }
    }

    /// Add a custom server configuration.
    pub async fn add_server_config(&self, config: LspServerConfig) {
        self.custom_configs
            .write()
            .await
            .insert(config.id.clone(), config);
    }

    /// Disable a server.
    pub async fn disable_server(&self, id: &str) {
        self.disabled_servers.write().await.push(id.to_string());
    }

    /// Enable a server.
    pub async fn enable_server(&self, id: &str) {
        self.disabled_servers.write().await.retain(|s| s != id);
    }

    /// Get or start an LSP client for a file.
    ///
    /// In multi-root mode, this will detect the appropriate root for the file
    /// and return/create a client for that specific root.
    pub async fn get_client_for_file(&self, path: &Path) -> Result<Option<Arc<LspClient>>> {
        // Use multi-root mode if enabled
        if self.multi_root_enabled {
            if let Some(workspace) = &self.workspace_manager {
                return workspace.get_client_for_file(path).await;
            }
        }

        // Legacy single-root mode
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        // Find matching server config
        let config = self.find_config_for_extension(ext).await;

        if let Some(config) = config {
            let disabled = self.disabled_servers.read().await;
            if disabled.contains(&config.id) {
                return Ok(None);
            }
            drop(disabled);

            return self.get_or_start_client(&config).await.map(Some);
        }

        Ok(None)
    }

    /// Get or start an LSP client for a file with automatic root detection.
    ///
    /// This always uses root detection, regardless of multi_root_enabled setting.
    pub async fn get_client_for_file_with_root_detection(
        &self,
        path: &Path,
    ) -> Result<Option<Arc<LspClient>>> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        // Find matching server config
        let config = self.find_config_for_extension(ext).await;

        if let Some(config) = config {
            let disabled = self.disabled_servers.read().await;
            if disabled.contains(&config.id) {
                return Ok(None);
            }
            drop(disabled);

            // Detect root for this file
            let detected_root = detect_root(path, &config.id);
            let root = detected_root
                .map(|r| r.path)
                .unwrap_or_else(|| self.root.clone());

            debug!(
                "Detected root {} for file {} (server: {})",
                root.display(),
                path.display(),
                config.id
            );

            return self
                .get_or_start_client_at_root(&config, &root)
                .await
                .map(Some);
        }

        Ok(None)
    }

    /// Get the detected root for a file and server.
    pub fn get_root_for_file(&self, path: &Path, server_id: &str) -> PathBuf {
        detect_root(path, server_id)
            .map(|r| r.path)
            .unwrap_or_else(|| self.root.clone())
    }

    /// Get or start an LSP client for a specific server.
    pub async fn get_client(&self, server_id: &str) -> Result<Arc<LspClient>> {
        let config = self
            .find_config_by_id(server_id)
            .await
            .ok_or_else(|| LspError::ServerNotFound(server_id.to_string()))?;

        self.get_or_start_client(&config).await
    }

    /// Get diagnostics for a file.
    pub async fn get_diagnostics(&self, path: &Path) -> Vec<Diagnostic> {
        if let Ok(Some(client)) = self.get_client_for_file(path).await {
            client.get_diagnostics(path).await
        } else {
            Vec::new()
        }
    }

    /// Get all diagnostics from all servers.
    pub async fn all_diagnostics(&self) -> HashMap<PathBuf, Vec<Diagnostic>> {
        let mut all = HashMap::new();
        let clients = self.clients.read().await;

        for client in clients.values() {
            let diags = client.all_diagnostics().await;
            for (path, d) in diags {
                all.entry(path).or_insert_with(Vec::new).extend(d);
            }
        }

        all
    }

    /// Open a file in the appropriate LSP server.
    pub async fn did_open(&self, path: &Path, content: &str) -> Result<()> {
        if let Some(client) = self.get_client_for_file(path).await? {
            let lang_id = self.get_language_id(path).unwrap_or("plaintext");
            client.did_open(path, lang_id, content).await?;
        }
        Ok(())
    }

    /// Close a file in the appropriate LSP server.
    pub async fn did_close(&self, path: &Path) -> Result<()> {
        if let Some(client) = self.get_client_for_file(path).await? {
            client.did_close(path).await?;
        }
        Ok(())
    }

    /// Get hover information for a position.
    pub async fn hover(&self, path: &Path, line: u32, column: u32) -> Result<Option<String>> {
        if let Some(client) = self.get_client_for_file(path).await? {
            if let Some(hover) = client.hover(path, line, column).await? {
                let text = match hover.contents {
                    lsp_types::HoverContents::Scalar(content) => Self::markup_to_string(content),
                    lsp_types::HoverContents::Array(contents) => contents
                        .into_iter()
                        .map(Self::markup_to_string)
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                    lsp_types::HoverContents::Markup(markup) => markup.value,
                };
                return Ok(Some(text));
            }
        }
        Ok(None)
    }

    /// Shutdown all LSP servers.
    pub async fn shutdown_all(&self) {
        let clients = self.clients.write().await;
        for (id, client) in clients.iter() {
            if let Err(e) = client.shutdown().await {
                warn!("Failed to shutdown LSP server {}: {}", id, e);
            }
        }
    }

    /// List running servers.
    pub async fn running_servers(&self) -> Vec<String> {
        self.clients.read().await.keys().cloned().collect()
    }

    /// List available servers.
    pub async fn available_servers(&self) -> Vec<LspServerConfig> {
        let custom = self.custom_configs.read().await;
        let mut servers: Vec<_> = BUILTIN_SERVERS.iter().cloned().collect();
        servers.extend(custom.values().cloned());
        servers
    }

    async fn get_or_start_client(&self, config: &LspServerConfig) -> Result<Arc<LspClient>> {
        // Check if already running
        {
            let clients = self.clients.read().await;
            if let Some(client) = clients.get(&config.id) {
                return Ok(client.clone());
            }
        }

        // Start new client
        let client = LspClient::new(config.clone()).with_root(&self.root);

        // Check if command exists
        if !Self::command_exists(&config.command[0]).await {
            return Err(LspError::ServerNotFound(format!(
                "{} not found (command: {})",
                config.name, config.command[0]
            )));
        }

        client.start().await?;
        client.initialize().await?;

        let client = Arc::new(client);
        self.clients
            .write()
            .await
            .insert(config.id.clone(), client.clone());

        info!("Started LSP server: {} ({})", config.name, config.id);
        Ok(client)
    }

    /// Get or start a client at a specific root (for multi-root support).
    async fn get_or_start_client_at_root(
        &self,
        config: &LspServerConfig,
        root: &Path,
    ) -> Result<Arc<LspClient>> {
        // Generate a unique key for this server+root combination
        let key = format!("{}:{}", config.id, root.display());

        // Check if already running
        {
            let clients = self.clients.read().await;
            if let Some(client) = clients.get(&key) {
                return Ok(client.clone());
            }
        }

        // Start new client at the specified root
        let client = LspClient::new(config.clone()).with_root(root);

        // Check if command exists
        if !Self::command_exists(&config.command[0]).await {
            return Err(LspError::ServerNotFound(format!(
                "{} not found (command: {})",
                config.name, config.command[0]
            )));
        }

        client.start().await?;
        client.initialize().await?;

        let client = Arc::new(client);
        self.clients.write().await.insert(key, client.clone());

        info!(
            "Started LSP server: {} ({}) at {}",
            config.name,
            config.id,
            root.display()
        );
        Ok(client)
    }

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

    async fn find_config_by_id(&self, id: &str) -> Option<LspServerConfig> {
        // Check custom configs first
        let custom = self.custom_configs.read().await;
        if let Some(config) = custom.get(id) {
            return Some(config.clone());
        }

        // Check builtin servers
        BUILTIN_SERVERS.iter().find(|c| c.id == id).cloned()
    }

    fn get_language_id(&self, path: &Path) -> Option<&'static str> {
        let ext = path.extension()?.to_str()?;
        match ext {
            "ts" | "tsx" => Some("typescript"),
            "js" | "jsx" | "mjs" | "cjs" => Some("javascript"),
            "rs" => Some("rust"),
            "py" | "pyi" => Some("python"),
            "go" => Some("go"),
            "c" | "h" => Some("c"),
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Some("cpp"),
            "java" => Some("java"),
            "lua" => Some("lua"),
            "yaml" | "yml" => Some("yaml"),
            "json" | "jsonc" => Some("json"),
            "html" | "htm" => Some("html"),
            "css" => Some("css"),
            "scss" => Some("scss"),
            "less" => Some("less"),
            "sh" | "bash" => Some("shellscript"),
            "zig" => Some("zig"),
            _ => None,
        }
    }

    fn markup_to_string(content: lsp_types::MarkedString) -> String {
        match content {
            lsp_types::MarkedString::String(s) => s,
            lsp_types::MarkedString::LanguageString(ls) => {
                format!("```{}\n{}\n```", ls.language, ls.value)
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lsp_manager_creation() {
        let temp_dir = std::env::temp_dir();
        let manager = LspManager::new(&temp_dir);
        let servers = manager.available_servers().await;
        assert!(!servers.is_empty());
    }

    #[tokio::test]
    async fn test_find_config_for_extension() {
        let temp_dir = std::env::temp_dir();
        let manager = LspManager::new(&temp_dir);
        let config = manager.find_config_for_extension("rs").await;
        assert!(config.is_some());
        assert_eq!(config.unwrap().id, "rust");
    }
}
