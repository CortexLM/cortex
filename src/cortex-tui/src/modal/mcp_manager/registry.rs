//! MCP Registry
//!
//! Provides access to MCP server configurations from both local fallbacks
//! and the remote registry at registry.cortex.foundation/mcp

use cortex_engine::mcp::RegistryServer;

/// Registry entry for display in the TUI
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    /// Server name
    pub name: String,
    /// Description
    pub description: String,
    /// Category
    pub category: Option<String>,
    /// Vendor
    pub _vendor: Option<String>,
    /// Tags for search
    pub tags: Vec<String>,
    /// Whether stdio transport is available
    pub _has_stdio: bool,
    /// Whether HTTP transport is available
    pub _has_http: bool,
    /// Required environment variables
    pub required_env: Vec<String>,
    /// Source (local or remote)
    pub _source: RegistrySource,
}

/// Source of the registry entry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrySource {
    /// Local fallback
    Local,
    /// Remote registry
    Remote,
}

impl From<RegistryServer> for RegistryEntry {
    fn from(server: RegistryServer) -> Self {
        let required_env = server
            .install
            .stdio
            .as_ref()
            .map(|s| s.required_env.clone())
            .unwrap_or_default();

        Self {
            name: server.name,
            description: server.description,
            category: server.category,
            _vendor: server.vendor,
            tags: server.tags,
            _has_stdio: server.install.stdio.is_some(),
            _has_http: server.install.http.is_some(),
            required_env,
            _source: RegistrySource::Remote,
        }
    }
}

/// Get list of available servers from local fallback registry
/// Returns (name, description) tuples for backwards compatibility
pub fn get_registry_servers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("filesystem", "File system operations and management"),
        ("github", "GitHub API integration for repos and issues"),
        ("postgres", "PostgreSQL database queries and management"),
        ("sqlite", "SQLite database operations"),
        ("brave-search", "Brave Search API for web searches"),
        ("google-maps", "Google Maps API integration"),
        ("slack", "Slack workspace integration"),
        ("memory", "Persistent memory storage"),
        ("fetch", "HTTP fetch operations"),
        ("puppeteer", "Browser automation with Puppeteer"),
        ("sequential-thinking", "Step-by-step reasoning"),
        ("time", "Time and timezone utilities"),
    ]
}

/// Get the command and args for a local registry server
pub fn get_registry_server_config(name: &str) -> (String, Vec<String>) {
    match name {
        "filesystem" => (
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                ".".to_string(),
            ],
        ),
        "github" => (
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-github".to_string(),
            ],
        ),
        "postgres" => (
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-postgres".to_string(),
            ],
        ),
        "sqlite" => (
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-sqlite".to_string(),
            ],
        ),
        "brave-search" => (
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-brave-search".to_string(),
            ],
        ),
        "google-maps" => (
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-google-maps".to_string(),
            ],
        ),
        "slack" => (
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-slack".to_string(),
            ],
        ),
        "memory" => (
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-memory".to_string(),
            ],
        ),
        "fetch" => ("uvx".to_string(), vec!["mcp-server-fetch".to_string()]),
        "puppeteer" => (
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-puppeteer".to_string(),
            ],
        ),
        "sequential-thinking" => (
            "npx".to_string(),
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-sequential-thinking".to_string(),
            ],
        ),
        "time" => ("uvx".to_string(), vec!["mcp-server-time".to_string()]),
        _ => ("npx".to_string(), vec!["-y".to_string(), name.to_string()]),
    }
}

/// Get registry entries as RegistryEntry structs
pub fn get_local_registry_entries() -> Vec<RegistryEntry> {
    get_registry_servers()
        .into_iter()
        .map(|(name, description)| {
            // Determine required env vars based on server
            let required_env = match name {
                "github" => vec!["GITHUB_TOKEN".to_string()],
                "brave-search" => vec!["BRAVE_API_KEY".to_string()],
                "google-maps" => vec!["GOOGLE_MAPS_API_KEY".to_string()],
                "slack" => vec!["SLACK_TOKEN".to_string()],
                "postgres" => vec!["DATABASE_URL".to_string()],
                _ => vec![],
            };

            RegistryEntry {
                name: name.to_string(),
                description: description.to_string(),
                category: None,
                _vendor: None,
                tags: vec![],
                _has_stdio: true,
                _has_http: false,
                required_env,
                _source: RegistrySource::Local,
            }
        })
        .collect()
}

/// Convert a RegistryServer from remote to command/args for installation
pub fn _get_remote_server_config(server: &RegistryServer) -> Option<(String, Vec<String>)> {
    server
        .install
        .stdio
        .as_ref()
        .map(|stdio| (stdio.command.clone(), stdio.args.clone()))
}

// Re-export types for convenience (allow unused imports since they're for API exposure)
#[allow(unused_imports)]
pub use cortex_engine::mcp::{
    HttpConfig as RemoteHttpConfig, McpRegistryClient, RegistryInstallConfig,
    RegistryServer as RemoteRegistryServer, StdioConfig as RemoteStdioConfig,
};
