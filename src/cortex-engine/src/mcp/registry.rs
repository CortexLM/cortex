//! MCP Remote Registry Client
//!
//! Provides access to MCP servers registered at registry.cortex.foundation/mcp
//! with local caching and TTL support.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use cortex_common::create_default_client;

/// Default registry URL
pub const REGISTRY_URL: &str = "https://registry.cortex.foundation/mcp";

/// Default cache TTL (1 hour)
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Cache directory name
const CACHE_DIR_NAME: &str = "mcp-registry";
const CACHE_FILE_NAME: &str = "registry-cache.json";

/// Information about an MCP server in the registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryServer {
    /// Server name/identifier
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Server vendor/author
    #[serde(default)]
    pub vendor: Option<String>,
    /// Homepage URL
    #[serde(default)]
    pub homepage: Option<String>,
    /// Server category (e.g., "database", "api", "filesystem")
    #[serde(default)]
    pub category: Option<String>,
    /// Tags for search
    #[serde(default)]
    pub tags: Vec<String>,
    /// Installation configurations by transport type
    pub install: RegistryInstallConfig,
}

/// Installation configuration for a registry server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryInstallConfig {
    /// Stdio transport configuration
    #[serde(default)]
    pub stdio: Option<StdioConfig>,
    /// HTTP/SSE transport configuration
    #[serde(default)]
    pub http: Option<HttpConfig>,
}

/// Stdio transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StdioConfig {
    /// Command to run
    pub command: String,
    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// Required environment variables (names only, values set by user)
    #[serde(default)]
    pub required_env: Vec<String>,
}

/// HTTP transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpConfig {
    /// SSE endpoint URL template
    pub url: String,
    /// Whether authentication is required
    #[serde(default)]
    pub requires_auth: bool,
}

/// Registry response from the remote API
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegistryResponse {
    /// List of available servers
    servers: Vec<RegistryServer>,
    /// Registry version
    #[serde(default)]
    #[allow(dead_code)]
    version: Option<String>,
}

/// Cached registry data
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedRegistry {
    /// Cached servers
    servers: Vec<RegistryServer>,
    /// When the cache was last updated (Unix timestamp)
    last_updated: u64,
    /// Registry URL used
    registry_url: String,
}

impl CachedRegistry {
    fn is_expired(&self, ttl: Duration) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now.saturating_sub(self.last_updated) > ttl.as_secs()
    }
}

/// MCP Registry Client for fetching servers from the remote registry
pub struct McpRegistryClient {
    /// HTTP client for requests
    http_client: reqwest::Client,
    /// Registry URL
    registry_url: String,
    /// Cache TTL
    cache_ttl: Duration,
    /// In-memory cache
    cache: Arc<RwLock<Option<(Vec<RegistryServer>, Instant)>>>,
    /// Cache directory for persistence
    cache_dir: Option<PathBuf>,
}

impl McpRegistryClient {
    /// Create a new registry client with default settings
    pub fn new() -> Result<Self> {
        Self::with_config(REGISTRY_URL, DEFAULT_CACHE_TTL, None)
    }

    /// Create a registry client with custom configuration
    pub fn with_config(
        registry_url: impl Into<String>,
        cache_ttl: Duration,
        cache_dir: Option<PathBuf>,
    ) -> Result<Self> {
        let http_client = create_default_client()
            .map_err(|e| anyhow::anyhow!(e))
            .context("Failed to create HTTP client")?;

        Ok(Self {
            http_client,
            registry_url: registry_url.into(),
            cache_ttl,
            cache: Arc::new(RwLock::new(None)),
            cache_dir,
        })
    }

    /// Set the cache directory for persistent caching
    pub fn with_cache_dir(mut self, dir: PathBuf) -> Self {
        self.cache_dir = Some(dir);
        self
    }

    /// Set the cache TTL
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Fetch servers from the registry (uses cache if valid)
    pub async fn fetch_servers(&self) -> Result<Vec<RegistryServer>> {
        // Check in-memory cache first
        {
            let cache = self.cache.read().await;
            if let Some((servers, last_updated)) = cache.as_ref() {
                if last_updated.elapsed() < self.cache_ttl {
                    debug!("Using in-memory cache for registry servers");
                    return Ok(servers.clone());
                }
            }
        }

        // Check persistent cache
        if let Some(servers) = self.load_from_disk_cache().await {
            // Update in-memory cache
            let mut cache = self.cache.write().await;
            *cache = Some((servers.clone(), Instant::now()));
            return Ok(servers);
        }

        // Fetch from remote
        self.fetch_from_remote().await
    }

    /// Force refresh from remote registry
    pub async fn refresh(&self) -> Result<Vec<RegistryServer>> {
        self.fetch_from_remote().await
    }

    /// Fetch servers from the remote registry
    async fn fetch_from_remote(&self) -> Result<Vec<RegistryServer>> {
        info!("Fetching MCP servers from registry: {}", self.registry_url);

        let response = self
            .http_client
            .get(&self.registry_url)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to fetch registry")?;

        if !response.status().is_success() {
            // Return fallback servers if registry is unavailable
            warn!(
                "Registry returned error status: {}, using fallback servers",
                response.status()
            );
            return Ok(Self::fallback_servers());
        }

        let registry: RegistryResponse = response
            .json()
            .await
            .context("Failed to parse registry response")?;

        // Update caches
        let servers = registry.servers;

        // Update in-memory cache
        {
            let mut cache = self.cache.write().await;
            *cache = Some((servers.clone(), Instant::now()));
        }

        // Save to disk cache
        self.save_to_disk_cache(&servers).await;

        info!("Fetched {} servers from registry", servers.len());
        Ok(servers)
    }

    /// Load servers from disk cache
    async fn load_from_disk_cache(&self) -> Option<Vec<RegistryServer>> {
        let cache_dir = self.cache_dir.as_ref()?;
        let cache_file = cache_dir.join(CACHE_DIR_NAME).join(CACHE_FILE_NAME);

        if !cache_file.exists() {
            return None;
        }

        let content = tokio::fs::read_to_string(&cache_file).await.ok()?;
        let cached: CachedRegistry = serde_json::from_str(&content).ok()?;

        // Check if cache is expired
        if cached.is_expired(self.cache_ttl) {
            debug!("Disk cache expired");
            return None;
        }

        // Check if registry URL matches
        if cached.registry_url != self.registry_url {
            debug!("Disk cache URL mismatch");
            return None;
        }

        debug!("Using disk cache for registry servers");
        Some(cached.servers)
    }

    /// Save servers to disk cache
    async fn save_to_disk_cache(&self, servers: &[RegistryServer]) {
        let Some(cache_dir) = self.cache_dir.as_ref() else {
            return;
        };

        let cache_path = cache_dir.join(CACHE_DIR_NAME);
        if let Err(e) = tokio::fs::create_dir_all(&cache_path).await {
            warn!("Failed to create cache directory: {}", e);
            return;
        }

        let cache_file = cache_path.join(CACHE_FILE_NAME);
        let cached = CachedRegistry {
            servers: servers.to_vec(),
            last_updated: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            registry_url: self.registry_url.clone(),
        };

        match serde_json::to_string_pretty(&cached) {
            Ok(json) => {
                if let Err(e) = tokio::fs::write(&cache_file, json).await {
                    warn!("Failed to write cache file: {}", e);
                }
            }
            Err(e) => {
                warn!("Failed to serialize cache: {}", e);
            }
        }
    }

    /// Search servers by name, description, or tags
    pub async fn search(&self, query: &str) -> Result<Vec<RegistryServer>> {
        let servers = self.fetch_servers().await?;
        let query_lower = query.to_lowercase();

        Ok(servers
            .into_iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&query_lower)
                    || s.description.to_lowercase().contains(&query_lower)
                    || s.tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&query_lower))
                    || s.category
                        .as_ref()
                        .map(|c| c.to_lowercase().contains(&query_lower))
                        .unwrap_or(false)
            })
            .collect())
    }

    /// Get servers by category
    pub async fn get_by_category(&self, category: &str) -> Result<Vec<RegistryServer>> {
        let servers = self.fetch_servers().await?;
        let category_lower = category.to_lowercase();

        Ok(servers
            .into_iter()
            .filter(|s| {
                s.category
                    .as_ref()
                    .map(|c| c.to_lowercase() == category_lower)
                    .unwrap_or(false)
            })
            .collect())
    }

    /// Get a specific server by name
    pub async fn get_server(&self, name: &str) -> Result<Option<RegistryServer>> {
        let servers = self.fetch_servers().await?;
        Ok(servers.into_iter().find(|s| s.name == name))
    }

    /// Fallback servers when registry is unavailable
    fn fallback_servers() -> Vec<RegistryServer> {
        vec![
            RegistryServer {
                name: "filesystem".to_string(),
                description: "File system operations and management".to_string(),
                vendor: Some("Anthropic".to_string()),
                homepage: Some("https://github.com/modelcontextprotocol/servers".to_string()),
                category: Some("filesystem".to_string()),
                tags: vec!["files".to_string(), "directories".to_string()],
                install: RegistryInstallConfig {
                    stdio: Some(StdioConfig {
                        command: "npx".to_string(),
                        args: vec![
                            "-y".to_string(),
                            "@modelcontextprotocol/server-filesystem".to_string(),
                            ".".to_string(),
                        ],
                        required_env: vec![],
                    }),
                    http: None,
                },
            },
            RegistryServer {
                name: "github".to_string(),
                description: "GitHub API integration for repos and issues".to_string(),
                vendor: Some("Anthropic".to_string()),
                homepage: Some("https://github.com/modelcontextprotocol/servers".to_string()),
                category: Some("api".to_string()),
                tags: vec!["github".to_string(), "git".to_string(), "repos".to_string()],
                install: RegistryInstallConfig {
                    stdio: Some(StdioConfig {
                        command: "npx".to_string(),
                        args: vec![
                            "-y".to_string(),
                            "@modelcontextprotocol/server-github".to_string(),
                        ],
                        required_env: vec!["GITHUB_TOKEN".to_string()],
                    }),
                    http: None,
                },
            },
            RegistryServer {
                name: "postgres".to_string(),
                description: "PostgreSQL database queries and management".to_string(),
                vendor: Some("Anthropic".to_string()),
                homepage: Some("https://github.com/modelcontextprotocol/servers".to_string()),
                category: Some("database".to_string()),
                tags: vec![
                    "database".to_string(),
                    "sql".to_string(),
                    "postgres".to_string(),
                ],
                install: RegistryInstallConfig {
                    stdio: Some(StdioConfig {
                        command: "npx".to_string(),
                        args: vec![
                            "-y".to_string(),
                            "@modelcontextprotocol/server-postgres".to_string(),
                        ],
                        required_env: vec!["DATABASE_URL".to_string()],
                    }),
                    http: None,
                },
            },
            RegistryServer {
                name: "sqlite".to_string(),
                description: "SQLite database operations".to_string(),
                vendor: Some("Anthropic".to_string()),
                homepage: Some("https://github.com/modelcontextprotocol/servers".to_string()),
                category: Some("database".to_string()),
                tags: vec![
                    "database".to_string(),
                    "sql".to_string(),
                    "sqlite".to_string(),
                ],
                install: RegistryInstallConfig {
                    stdio: Some(StdioConfig {
                        command: "npx".to_string(),
                        args: vec![
                            "-y".to_string(),
                            "@modelcontextprotocol/server-sqlite".to_string(),
                        ],
                        required_env: vec![],
                    }),
                    http: None,
                },
            },
            RegistryServer {
                name: "brave-search".to_string(),
                description: "Brave Search API for web searches".to_string(),
                vendor: Some("Anthropic".to_string()),
                homepage: Some("https://github.com/modelcontextprotocol/servers".to_string()),
                category: Some("search".to_string()),
                tags: vec!["search".to_string(), "web".to_string()],
                install: RegistryInstallConfig {
                    stdio: Some(StdioConfig {
                        command: "npx".to_string(),
                        args: vec![
                            "-y".to_string(),
                            "@modelcontextprotocol/server-brave-search".to_string(),
                        ],
                        required_env: vec!["BRAVE_API_KEY".to_string()],
                    }),
                    http: None,
                },
            },
            RegistryServer {
                name: "memory".to_string(),
                description: "Persistent memory storage".to_string(),
                vendor: Some("Anthropic".to_string()),
                homepage: Some("https://github.com/modelcontextprotocol/servers".to_string()),
                category: Some("utility".to_string()),
                tags: vec!["memory".to_string(), "storage".to_string()],
                install: RegistryInstallConfig {
                    stdio: Some(StdioConfig {
                        command: "npx".to_string(),
                        args: vec![
                            "-y".to_string(),
                            "@modelcontextprotocol/server-memory".to_string(),
                        ],
                        required_env: vec![],
                    }),
                    http: None,
                },
            },
            RegistryServer {
                name: "fetch".to_string(),
                description: "HTTP fetch operations".to_string(),
                vendor: Some("Community".to_string()),
                homepage: None,
                category: Some("utility".to_string()),
                tags: vec!["http".to_string(), "fetch".to_string(), "web".to_string()],
                install: RegistryInstallConfig {
                    stdio: Some(StdioConfig {
                        command: "uvx".to_string(),
                        args: vec!["mcp-server-fetch".to_string()],
                        required_env: vec![],
                    }),
                    http: None,
                },
            },
            RegistryServer {
                name: "time".to_string(),
                description: "Time and timezone utilities".to_string(),
                vendor: Some("Community".to_string()),
                homepage: None,
                category: Some("utility".to_string()),
                tags: vec!["time".to_string(), "timezone".to_string()],
                install: RegistryInstallConfig {
                    stdio: Some(StdioConfig {
                        command: "uvx".to_string(),
                        args: vec!["mcp-server-time".to_string()],
                        required_env: vec![],
                    }),
                    http: None,
                },
            },
        ]
    }

    /// Clear all caches
    pub async fn clear_cache(&self) {
        // Clear in-memory cache
        {
            let mut cache = self.cache.write().await;
            *cache = None;
        }

        // Clear disk cache
        if let Some(cache_dir) = self.cache_dir.as_ref() {
            let cache_file = cache_dir.join(CACHE_DIR_NAME).join(CACHE_FILE_NAME);
            let _ = tokio::fs::remove_file(&cache_file).await;
        }
    }
}

impl Default for McpRegistryClient {
    fn default() -> Self {
        Self::new().expect("Failed to create default registry client")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_servers() {
        let servers = McpRegistryClient::fallback_servers();
        assert!(!servers.is_empty());

        // Check filesystem server exists
        let fs_server = servers.iter().find(|s| s.name == "filesystem");
        assert!(fs_server.is_some());

        // Check it has stdio config
        let fs = fs_server.unwrap();
        assert!(fs.install.stdio.is_some());
    }

    #[test]
    fn test_cache_expiry() {
        let cached = CachedRegistry {
            servers: vec![],
            last_updated: 0, // Unix epoch
            registry_url: "test".to_string(),
        };

        // Should be expired
        assert!(cached.is_expired(Duration::from_secs(60)));
    }

    #[tokio::test]
    async fn test_registry_client_creation() {
        let client = McpRegistryClient::new();
        assert!(client.is_ok());
    }
}
