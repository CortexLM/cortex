//! MCP Server Configuration Storage
//!
//! This module provides persistent storage for MCP server configurations
//! in the local Cortex data directory (using the cross-platform `AppDirs`).
//!
//! Configuration files are stored as JSON files in the `mcps` directory:
//! - Linux: `~/.local/share/cortex/mcps/`
//! - macOS: `~/.local/share/cortex/mcps/`
//! - Windows: `%APPDATA%\cortex\mcps\`

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use cortex_common::AppDirs;
use serde::{Deserialize, Serialize};

/// MCP transport type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum McpTransport {
    /// Stdio transport (local subprocess)
    #[default]
    Stdio,
    /// HTTP transport (remote server)
    Http,
}

/// Stored MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMcpServer {
    /// Server name/identifier
    pub name: String,
    /// Whether the server is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Transport type
    #[serde(default)]
    pub transport: McpTransport,
    /// Command to execute (for stdio transport)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Command arguments (for stdio transport)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Environment variables to set when launching
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// Server URL (for HTTP transport)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// API key environment variable name (for HTTP transport)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env_var: Option<String>,
    /// Working directory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Auto-start on application launch
    #[serde(default = "default_auto_start")]
    pub auto_start: bool,
}

fn default_enabled() -> bool {
    true
}

fn default_auto_start() -> bool {
    true
}

impl StoredMcpServer {
    /// Create a new stdio MCP server configuration
    pub fn new_stdio(name: String, command: String, args: Vec<String>) -> Self {
        Self {
            name,
            enabled: true,
            transport: McpTransport::Stdio,
            command: Some(command),
            args,
            env: HashMap::new(),
            url: None,
            api_key_env_var: None,
            cwd: None,
            auto_start: true,
        }
    }

    /// Create a new HTTP MCP server configuration
    pub fn new_http(name: String, url: String) -> Self {
        Self {
            name,
            enabled: true,
            transport: McpTransport::Http,
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            url: Some(url),
            api_key_env_var: None,
            cwd: None,
            auto_start: true,
        }
    }

    /// Set environment variables
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Set API key environment variable name
    pub fn with_api_key_env_var(mut self, env_var: String) -> Self {
        self.api_key_env_var = Some(env_var);
        self
    }
}

/// MCP storage manager
///
/// Handles reading and writing MCP server configurations to the local data directory.
pub struct McpStorage {
    /// Directory where MCP configs are stored
    mcps_dir: PathBuf,
}

impl McpStorage {
    /// Create a new MCP storage manager
    pub fn new() -> Result<Self> {
        let app_dirs = AppDirs::new()
            .ok_or_else(|| anyhow::anyhow!("Failed to determine application directories"))?;
        let mcps_dir = app_dirs.mcps_dir();
        Ok(Self { mcps_dir })
    }

    /// Create storage with a custom directory (for testing)
    pub fn with_dir(mcps_dir: PathBuf) -> Self {
        Self { mcps_dir }
    }

    /// Ensure the MCP storage directory exists
    pub fn ensure_dir(&self) -> Result<()> {
        if !self.mcps_dir.exists() {
            std::fs::create_dir_all(&self.mcps_dir).with_context(|| {
                format!(
                    "Failed to create MCP storage directory: {}",
                    self.mcps_dir.display()
                )
            })?;

            // Set proper permissions on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&self.mcps_dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(())
    }

    /// Get the path to a server's config file
    fn server_path(&self, name: &str) -> PathBuf {
        self.mcps_dir.join(format!("{}.json", name))
    }

    /// Save an MCP server configuration
    pub fn save_server(&self, server: &StoredMcpServer) -> Result<()> {
        self.ensure_dir()?;

        let path = self.server_path(&server.name);
        let content = serde_json::to_string_pretty(server)
            .with_context(|| format!("Failed to serialize MCP server config: {}", server.name))?;

        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write MCP server config: {}", path.display()))?;

        tracing::info!(
            "Saved MCP server config: {} -> {}",
            server.name,
            path.display()
        );
        Ok(())
    }

    /// Load an MCP server configuration by name
    pub fn load_server(&self, name: &str) -> Result<Option<StoredMcpServer>> {
        let path = self.server_path(name);
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read MCP server config: {}", path.display()))?;

        let server: StoredMcpServer = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse MCP server config: {}", path.display()))?;

        Ok(Some(server))
    }

    /// Remove an MCP server configuration
    pub fn remove_server(&self, name: &str) -> Result<bool> {
        let path = self.server_path(name);
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| {
                format!("Failed to remove MCP server config: {}", path.display())
            })?;
            tracing::info!("Removed MCP server config: {}", name);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// List all stored MCP server configurations
    pub fn list_servers(&self) -> Result<Vec<StoredMcpServer>> {
        self.ensure_dir()?;

        let mut servers = Vec::new();

        for entry in std::fs::read_dir(&self.mcps_dir).with_context(|| {
            format!(
                "Failed to read MCP storage directory: {}",
                self.mcps_dir.display()
            )
        })? {
            let entry = entry?;
            let path = entry.path();

            // Only process .json files
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                match std::fs::read_to_string(&path) {
                    Ok(content) => match serde_json::from_str::<StoredMcpServer>(&content) {
                        Ok(server) => servers.push(server),
                        Err(e) => {
                            tracing::warn!("Failed to parse MCP config {}: {}", path.display(), e);
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Failed to read MCP config {}: {}", path.display(), e);
                    }
                }
            }
        }

        // Sort by name for consistent ordering
        servers.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(servers)
    }

    /// Check if a server with the given name exists
    pub fn server_exists(&self, name: &str) -> bool {
        self.server_path(name).exists()
    }

    /// Get the storage directory path
    pub fn storage_dir(&self) -> &PathBuf {
        &self.mcps_dir
    }
}

impl Default for McpStorage {
    fn default() -> Self {
        Self::new().expect("Failed to initialize MCP storage")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_storage() -> (McpStorage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let storage = McpStorage::with_dir(temp_dir.path().join("mcps"));
        (storage, temp_dir)
    }

    #[test]
    fn test_save_and_load_stdio_server() {
        let (storage, _tmp) = test_storage();

        let server = StoredMcpServer::new_stdio(
            "test-server".to_string(),
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
            ],
        );

        storage.save_server(&server).unwrap();

        let loaded = storage.load_server("test-server").unwrap();
        assert!(loaded.is_some());

        let loaded = loaded.unwrap();
        assert_eq!(loaded.name, "test-server");
        assert_eq!(loaded.command, Some("npx".to_string()));
        assert_eq!(loaded.args.len(), 2);
        assert!(loaded.enabled);
        assert_eq!(loaded.transport, McpTransport::Stdio);
    }

    #[test]
    fn test_save_and_load_http_server() {
        let (storage, _tmp) = test_storage();

        let server = StoredMcpServer::new_http(
            "api-server".to_string(),
            "https://api.example.com/mcp".to_string(),
        )
        .with_api_key_env_var("MY_API_KEY".to_string());

        storage.save_server(&server).unwrap();

        let loaded = storage.load_server("api-server").unwrap().unwrap();
        assert_eq!(loaded.name, "api-server");
        assert_eq!(loaded.url, Some("https://api.example.com/mcp".to_string()));
        assert_eq!(loaded.api_key_env_var, Some("MY_API_KEY".to_string()));
        assert_eq!(loaded.transport, McpTransport::Http);
    }

    #[test]
    fn test_list_servers() {
        let (storage, _tmp) = test_storage();

        storage
            .save_server(&StoredMcpServer::new_stdio(
                "server-b".to_string(),
                "cmd".to_string(),
                vec![],
            ))
            .unwrap();

        storage
            .save_server(&StoredMcpServer::new_stdio(
                "server-a".to_string(),
                "cmd".to_string(),
                vec![],
            ))
            .unwrap();

        let servers = storage.list_servers().unwrap();
        assert_eq!(servers.len(), 2);
        // Should be sorted alphabetically
        assert_eq!(servers[0].name, "server-a");
        assert_eq!(servers[1].name, "server-b");
    }

    #[test]
    fn test_remove_server() {
        let (storage, _tmp) = test_storage();

        storage
            .save_server(&StoredMcpServer::new_stdio(
                "to-remove".to_string(),
                "cmd".to_string(),
                vec![],
            ))
            .unwrap();

        assert!(storage.server_exists("to-remove"));
        assert!(storage.remove_server("to-remove").unwrap());
        assert!(!storage.server_exists("to-remove"));
        assert!(!storage.remove_server("to-remove").unwrap()); // Already removed
    }

    #[test]
    fn test_load_nonexistent_server() {
        let (storage, _tmp) = test_storage();
        let result = storage.load_server("nonexistent").unwrap();
        assert!(result.is_none());
    }
}
