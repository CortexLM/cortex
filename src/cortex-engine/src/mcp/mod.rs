//! MCP (Model Context Protocol) support.
//!
//! This module provides:
//! - MCP client for connecting to servers (stdio, HTTP/SSE)
//! - Connection manager for multiple servers
//! - OAuth 2.0 authentication support for remote servers
//! - Tool execution and resource reading

pub mod client;
pub mod manager;
pub mod oauth;
pub mod oauth_callback;
pub mod registry;

// OAuth exports - these are actively used by cortex-cli
pub use oauth::{
    AuthStatus, OAUTH_CALLBACK_PATH, OAUTH_CALLBACK_PORT, OAuthClientInfo, OAuthClientMetadata,
    OAuthConfig, OAuthEntry, OAuthFlow, OAuthServerMetadata, OAuthStorage, OAuthTokens, Pkce,
    get_auth_status, has_stored_tokens, remove_auth,
};
pub use oauth_callback::{
    CallbackResult, OAuthCallbackServer, ensure_valid_tokens, run_oauth_flow,
};

// Client exports
pub use client::{ConnectionState, McpClient};
pub use manager::{McpConnectionManager, create_qualified_name, parse_qualified_name};

// Registry exports
pub use registry::{
    DEFAULT_CACHE_TTL, HttpConfig, McpRegistryClient, REGISTRY_URL, RegistryInstallConfig,
    RegistryServer, StdioConfig,
};

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// MCP configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    /// Enabled servers.
    pub servers: Vec<McpServerConfig>,
    /// Default timeout for requests.
    pub timeout: Duration,
    /// Auto-start servers on init.
    pub auto_start: bool,
    /// Maximum concurrent requests per server.
    pub max_concurrent: usize,
    /// Enable caching.
    pub cache_enabled: bool,
    /// Cache TTL.
    pub cache_ttl: Duration,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            timeout: Duration::from_secs(30),
            auto_start: true,
            max_concurrent: 10,
            cache_enabled: true,
            cache_ttl: Duration::from_secs(300),
        }
    }
}

/// MCP server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Server name/identifier.
    pub name: String,
    /// Server command to execute.
    pub command: String,
    /// Command arguments.
    pub args: Vec<String>,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// Transport type.
    pub transport: TransportType,
    /// Server capabilities.
    pub capabilities: Option<Vec<String>>,
    /// Auto-start this server.
    pub auto_start: bool,
    /// Restart on failure.
    pub restart_on_failure: bool,
    /// Maximum restart attempts.
    pub max_restarts: u32,
    /// SSE endpoint URL (for SSE transport).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sse_url: Option<String>,
    /// HTTP POST URL for SSE transport (defaults to sse_url if not specified).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sse_post_url: Option<String>,
}

impl McpServerConfig {
    /// Create a new server config.
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            transport: TransportType::Stdio,
            capabilities: None,
            auto_start: true,
            restart_on_failure: true,
            max_restarts: 3,
            sse_url: None,
            sse_post_url: None,
        }
    }

    /// Create a new SSE server config.
    pub fn new_sse(name: impl Into<String>, sse_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            transport: TransportType::Sse,
            capabilities: None,
            auto_start: true,
            restart_on_failure: true,
            max_restarts: 3,
            sse_url: Some(sse_url.into()),
            sse_post_url: None,
        }
    }

    /// Add an argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args
            .extend(args.into_iter().map(std::convert::Into::into));
        self
    }

    /// Add environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set working directory.
    pub fn cwd(mut self, path: impl Into<PathBuf>) -> Self {
        self.cwd = Some(path.into());
        self
    }

    /// Set transport type.
    pub fn transport(mut self, transport: TransportType) -> Self {
        self.transport = transport;
        self
    }

    /// Set SSE URL (also sets transport to SSE).
    pub fn sse_url(mut self, url: impl Into<String>) -> Self {
        self.sse_url = Some(url.into());
        self.transport = TransportType::Sse;
        self
    }

    /// Set SSE POST URL.
    pub fn sse_post_url(mut self, url: impl Into<String>) -> Self {
        self.sse_post_url = Some(url.into());
        self
    }
}

/// Transport type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportType {
    /// Standard I/O transport.
    Stdio,
    /// HTTP/HTTPS transport.
    Http,
    /// WebSocket transport.
    WebSocket,
    /// Server-Sent Events transport.
    Sse,
}
