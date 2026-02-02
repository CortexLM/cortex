//! MCP Client - Connects to MCP servers and executes tools.
//!
//! Supports:
//! - Stdio transport (local processes)
//! - HTTP/SSE transport (remote servers)
//! - Tool listing and execution
//! - Resource reading
//! - Prompt retrieval

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};

use cortex_mcp_types::{
    CallToolParams, CallToolResult, InitializeParams, InitializeResult, JSONRPC_VERSION,
    JsonRpcRequest, JsonRpcResponse, ListResourcesResult, ListToolsResult, ReadResourceParams,
    ReadResourceResult, Resource, Tool, methods,
};

use cortex_common::create_default_client;

use super::{McpServerConfig, TransportType};

/// MCP client connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Not connected.
    Disconnected,
    /// Connection in progress.
    Connecting,
    /// Connected and initialized.
    Connected,
    /// Connection failed.
    Failed,
}

/// MCP client for connecting to a single server.
pub struct McpClient {
    /// Server configuration.
    config: McpServerConfig,
    /// Connection state.
    state: RwLock<ConnectionState>,
    /// Server capabilities (after initialization).
    server_info: RwLock<Option<InitializeResult>>,
    /// Request ID counter.
    request_id: AtomicI64,
    /// Stdio transport (process handle).
    stdio_process: Mutex<Option<StdioTransport>>,
    /// HTTP/SSE transport.
    http_client: reqwest::Client,
    /// Cached tools.
    cached_tools: RwLock<Vec<Tool>>,
    /// Cached resources.
    cached_resources: RwLock<Vec<Resource>>,
}

/// Stdio transport for local MCP servers.
struct StdioTransport {
    child: Child,
    // We'll use synchronous I/O for simplicity
}

impl McpClient {
    /// Create a new MCP client for the given server configuration.
    pub fn new(config: McpServerConfig) -> Self {
        let http_client = create_default_client().expect("HTTP client");
        Self {
            config,
            state: RwLock::new(ConnectionState::Disconnected),
            server_info: RwLock::new(None),
            request_id: AtomicI64::new(1),
            stdio_process: Mutex::new(None),
            http_client,
            cached_tools: RwLock::new(Vec::new()),
            cached_resources: RwLock::new(Vec::new()),
        }
    }

    /// Get the server name.
    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// Get the connection state.
    pub async fn state(&self) -> ConnectionState {
        *self.state.read().await
    }

    /// Check if connected.
    pub async fn is_connected(&self) -> bool {
        *self.state.read().await == ConnectionState::Connected
    }

    /// Get server info (after connection).
    pub async fn server_info(&self) -> Option<InitializeResult> {
        self.server_info.read().await.clone()
    }

    /// Get cached tools.
    pub async fn tools(&self) -> Vec<Tool> {
        self.cached_tools.read().await.clone()
    }

    /// Get cached resources.
    pub async fn resources(&self) -> Vec<Resource> {
        self.cached_resources.read().await.clone()
    }

    /// Connect to the MCP server.
    pub async fn connect(&self) -> Result<()> {
        {
            let mut state = self.state.write().await;
            if *state == ConnectionState::Connected {
                return Ok(());
            }
            *state = ConnectionState::Connecting;
        }

        let result = match self.config.transport {
            TransportType::Stdio => self.connect_stdio().await,
            TransportType::Sse | TransportType::Http => self.connect_http().await,
            TransportType::WebSocket => Err(anyhow!("WebSocket transport not yet implemented")),
        };

        match result {
            Ok(_) => {
                *self.state.write().await = ConnectionState::Connected;
                info!("Connected to MCP server: {}", self.config.name);

                // Refresh tools and resources
                let _ = self.refresh_tools().await;
                let _ = self.refresh_resources().await;

                Ok(())
            }
            Err(e) => {
                *self.state.write().await = ConnectionState::Failed;
                error!(
                    "Failed to connect to MCP server {}: {}",
                    self.config.name, e
                );
                Err(e)
            }
        }
    }

    /// Patterns in variable names that indicate sensitive data (case-insensitive).
    /// These will be excluded from the environment passed to MCP server processes.
    const SENSITIVE_PATTERNS: &'static [&'static str] = &[
        "KEY",        // API_KEY, SSH_KEY, etc.
        "SECRET",     // AWS_SECRET, etc.
        "TOKEN",      // AUTH_TOKEN, etc. (except CORTEX_TOKEN which MCP servers may need)
        "PASSWORD",   // DB_PASSWORD, etc.
        "CREDENTIAL", // GOOGLE_CREDENTIALS, etc.
        "PRIVATE",    // PRIVATE_KEY, etc.
    ];

    /// Environment variables that are explicitly allowed even if they match sensitive patterns.
    /// These are needed for MCP servers to function properly.
    const ALLOWED_ENV_VARS: &'static [&'static str] = &[
        "CORTEX_TOKEN", // MCP servers may need this to communicate
        "PATH",         // Essential for finding executables
        "HOME",         // Many tools need this
        "USER",         // User information
        "SHELL",        // Shell information
    ];

    /// Connect using stdio transport.
    async fn connect_stdio(&self) -> Result<()> {
        let mut cmd = Command::new(&self.config.command);
        cmd.args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Build a filtered environment to prevent leaking sensitive data to MCP servers.
        // Start with a clean environment and selectively add safe variables.
        cmd.env_clear();

        // Add filtered environment variables from the parent process
        for (key, value) in std::env::vars() {
            // Check if explicitly allowed
            if Self::ALLOWED_ENV_VARS.iter().any(|&allowed| key == allowed) {
                cmd.env(&key, &value);
                continue;
            }

            // Check if matches sensitive patterns (case-insensitive)
            let key_upper = key.to_uppercase();
            let is_sensitive = Self::SENSITIVE_PATTERNS
                .iter()
                .any(|pattern| key_upper.contains(pattern));

            if !is_sensitive {
                cmd.env(&key, &value);
            }
        }

        // Add explicitly configured environment variables (these override filtered ones)
        // Note: Config-specified env vars are trusted since they come from user configuration
        for (key, value) in &self.config.env {
            cmd.env(key, value);
        }

        // Set working directory
        if let Some(ref cwd) = self.config.cwd {
            cmd.current_dir(cwd);
        }

        let child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {}", self.config.command))?;

        *self.stdio_process.lock().await = Some(StdioTransport { child });

        // Send initialize request
        self.initialize().await?;

        Ok(())
    }

    /// Connect using HTTP/SSE transport.
    async fn connect_http(&self) -> Result<()> {
        let _url = self
            .config
            .sse_url
            .as_ref()
            .ok_or_else(|| anyhow!("SSE URL not configured for HTTP transport"))?;

        // Test connection by sending initialize
        self.initialize().await?;

        Ok(())
    }

    /// Send initialize request.
    async fn initialize(&self) -> Result<InitializeResult> {
        let params = InitializeParams::default();

        let response: InitializeResult = self
            .request(methods::INITIALIZE, Some(serde_json::to_value(&params)?))
            .await?;

        *self.server_info.write().await = Some(response.clone());

        // Send initialized notification
        self.notify(methods::INITIALIZED, None).await?;

        Ok(response)
    }

    /// Disconnect from the server.
    pub async fn disconnect(&self) -> Result<()> {
        let mut process = self.stdio_process.lock().await;
        if let Some(mut transport) = process.take() {
            let _ = transport.child.kill();
        }

        *self.state.write().await = ConnectionState::Disconnected;
        *self.server_info.write().await = None;
        self.cached_tools.write().await.clear();
        self.cached_resources.write().await.clear();

        info!("Disconnected from MCP server: {}", self.config.name);
        Ok(())
    }

    /// Refresh the list of available tools.
    pub async fn refresh_tools(&self) -> Result<Vec<Tool>> {
        let result: ListToolsResult = self.request(methods::TOOLS_LIST, None).await?;
        *self.cached_tools.write().await = result.tools.clone();
        Ok(result.tools)
    }

    /// Refresh the list of available resources.
    pub async fn refresh_resources(&self) -> Result<Vec<Resource>> {
        let result: ListResourcesResult = self.request(methods::RESOURCES_LIST, None).await?;
        *self.cached_resources.write().await = result.resources.clone();
        Ok(result.resources)
    }

    /// Call a tool.
    pub async fn call_tool(&self, name: &str, arguments: Option<Value>) -> Result<CallToolResult> {
        let params = CallToolParams {
            name: name.to_string(),
            arguments,
        };

        self.request(methods::TOOLS_CALL, Some(serde_json::to_value(&params)?))
            .await
    }

    /// Read a resource.
    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult> {
        let params = ReadResourceParams {
            uri: uri.to_string(),
        };

        self.request(
            methods::RESOURCES_READ,
            Some(serde_json::to_value(&params)?),
        )
        .await
    }

    /// Send a JSON-RPC request and wait for response.
    async fn request<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<T> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);

        let request = JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: id.into(),
            method: method.to_string(),
            params,
        };

        let response = match self.config.transport {
            TransportType::Stdio => self.send_stdio_request(&request).await?,
            TransportType::Sse | TransportType::Http => self.send_http_request(&request).await?,
            TransportType::WebSocket => {
                return Err(anyhow!("WebSocket transport not yet implemented"));
            }
        };

        if let Some(error) = response.error {
            return Err(anyhow!(
                "MCP error: {} (code {})",
                error.message,
                error.code
            ));
        }

        let result = response
            .result
            .ok_or_else(|| anyhow!("No result in response"))?;
        serde_json::from_value(result).context("Failed to parse MCP response")
    }

    /// Send a JSON-RPC notification (no response expected).
    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let notification = json!({
            "jsonrpc": JSONRPC_VERSION,
            "method": method,
            "params": params
        });

        match self.config.transport {
            TransportType::Stdio => self.send_stdio_notification(&notification).await,
            TransportType::Sse | TransportType::Http => {
                self.send_http_notification(&notification).await
            }
            TransportType::WebSocket => Err(anyhow!("WebSocket transport not yet implemented")),
        }
    }

    /// Send a request via stdio transport.
    async fn send_stdio_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let mut process = self.stdio_process.lock().await;
        let transport = process.as_mut().ok_or_else(|| anyhow!("Not connected"))?;

        let stdin = transport
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("No stdin"))?;
        let stdout = transport
            .child
            .stdout
            .as_mut()
            .ok_or_else(|| anyhow!("No stdout"))?;

        // Write request
        let request_json = serde_json::to_string(request)?;
        writeln!(stdin, "{}", request_json)?;
        stdin.flush()?;

        // Read response
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line)?;

        serde_json::from_str(&line).context("Failed to parse JSON-RPC response")
    }

    /// Send a notification via stdio transport.
    async fn send_stdio_notification(&self, notification: &Value) -> Result<()> {
        let mut process = self.stdio_process.lock().await;
        let transport = process.as_mut().ok_or_else(|| anyhow!("Not connected"))?;

        let stdin = transport
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("No stdin"))?;

        let json = serde_json::to_string(notification)?;
        writeln!(stdin, "{}", json)?;
        stdin.flush()?;

        Ok(())
    }

    /// Send a request via HTTP transport.
    async fn send_http_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        let url = self
            .config
            .sse_post_url
            .as_ref()
            .or(self.config.sse_url.as_ref())
            .ok_or_else(|| anyhow!("No HTTP URL configured"))?;

        let response = self
            .http_client
            .post(url)
            .json(request)
            .send()
            .await
            .context("HTTP request failed")?;

        if !response.status().is_success() {
            return Err(anyhow!("HTTP error: {}", response.status()));
        }

        response
            .json()
            .await
            .context("Failed to parse JSON-RPC response")
    }

    /// Send a notification via HTTP transport.
    async fn send_http_notification(&self, notification: &Value) -> Result<()> {
        let url = self
            .config
            .sse_post_url
            .as_ref()
            .or(self.config.sse_url.as_ref())
            .ok_or_else(|| anyhow!("No HTTP URL configured"))?;

        let response = self
            .http_client
            .post(url)
            .json(notification)
            .send()
            .await
            .context("HTTP notification failed")?;

        if !response.status().is_success() {
            warn!("HTTP notification returned error: {}", response.status());
        }

        Ok(())
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Kill child process if running
        if let Ok(mut process) = self.stdio_process.try_lock() {
            if let Some(mut transport) = process.take() {
                let _ = transport.child.kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_state() {
        assert_ne!(ConnectionState::Connected, ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_client_creation() {
        let config = McpServerConfig::new("test", "echo");
        let client = McpClient::new(config);

        assert_eq!(client.name(), "test");
        assert_eq!(client.state().await, ConnectionState::Disconnected);
    }
}
