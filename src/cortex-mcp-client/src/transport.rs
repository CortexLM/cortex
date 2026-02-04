//! MCP Client Transport Layer
//!
//! Provides stdio and HTTP/SSE transports for MCP client communication.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use cortex_common::create_default_client;
use cortex_mcp_types::{
    CallToolParams, CallToolResult, GetPromptParams, GetPromptResult, InitializeParams,
    InitializeResult, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, ListPromptsResult,
    ListResourcesResult, ListToolsResult, ReadResourceParams, ReadResourceResult, RequestId,
    methods,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

// ============================================================================
// Transport Trait
// ============================================================================

/// Transport layer for MCP client communication.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Initialize the connection.
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult>;

    /// Send initialized notification.
    async fn send_initialized(&self) -> Result<()>;

    /// List available tools.
    async fn list_tools(&self) -> Result<ListToolsResult>;

    /// Execute a tool.
    async fn call_tool(&self, params: CallToolParams) -> Result<CallToolResult>;

    /// List available resources.
    async fn list_resources(&self) -> Result<ListResourcesResult>;

    /// Read a resource.
    async fn read_resource(&self, params: ReadResourceParams) -> Result<ReadResourceResult>;

    /// List available prompts.
    async fn list_prompts(&self) -> Result<ListPromptsResult>;

    /// Get a prompt.
    async fn get_prompt(&self, params: GetPromptParams) -> Result<GetPromptResult>;

    /// Send a ping.
    async fn ping(&self) -> Result<()>;

    /// Close the connection.
    async fn close(&self) -> Result<()>;

    /// Check if the transport is connected.
    fn is_connected(&self) -> bool;
}

// ============================================================================
// Stdio Transport
// ============================================================================

/// Stdio transport using subprocess communication.
pub struct StdioTransport {
    /// Child process.
    process: Arc<Mutex<Option<Child>>>,
    /// Request ID counter.
    request_id: AtomicU64,
    /// Whether the transport is connected.
    connected: AtomicBool,
    /// Command to execute.
    command: String,
    /// Command arguments.
    args: Vec<String>,
    /// Working directory.
    cwd: Option<String>,
    /// Environment variables.
    env: HashMap<String, String>,
    /// Pending responses.
    pending_responses: Arc<RwLock<HashMap<String, tokio::sync::oneshot::Sender<JsonRpcResponse>>>>,
    /// Reconnection settings.
    reconnect_config: ReconnectConfig,
}

impl StdioTransport {
    /// Create a new stdio transport.
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            process: Arc::new(Mutex::new(None)),
            request_id: AtomicU64::new(1),
            connected: AtomicBool::new(false),
            command: command.into(),
            args,
            cwd: None,
            env: HashMap::new(),
            pending_responses: Arc::new(RwLock::new(HashMap::new())),
            reconnect_config: ReconnectConfig::default(),
        }
    }

    /// Set working directory.
    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Add environment variable.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set reconnection configuration.
    pub fn with_reconnect(mut self, config: ReconnectConfig) -> Self {
        self.reconnect_config = config;
        self
    }

    /// Connect to the subprocess.
    async fn connect(&self) -> Result<()> {
        let mut process_guard = self.process.lock().await;

        if process_guard.is_some() {
            return Ok(());
        }

        debug!(command = %self.command, args = ?self.args, "Starting MCP subprocess");

        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if let Some(ref cwd) = self.cwd {
            cmd.current_dir(cwd);
        }

        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().context("Failed to spawn subprocess")?;

        // Start reading stdout in background
        let stdout = child.stdout.take().context("Failed to get stdout")?;
        let pending_responses = self.pending_responses.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // Try to parse as response
                if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
                    let id = response.id.to_string();
                    if let Some(sender) = pending_responses.write().await.remove(&id) {
                        let _ = sender.send(response);
                    }
                }
                // Try to parse as notification
                else if let Ok(notification) =
                    serde_json::from_str::<JsonRpcNotification>(trimmed)
                {
                    debug!(method = %notification.method, "Received notification");
                }
            }
        });

        // Start reading stderr in background
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    warn!(stderr = %line, "MCP subprocess stderr");
                }
            });
        }

        *process_guard = Some(child);
        self.connected.store(true, Ordering::SeqCst);

        info!(command = %self.command, "MCP subprocess connected");
        Ok(())
    }

    /// Send a request and wait for response.
    async fn send_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        // Ensure connected
        if !self.connected.load(Ordering::SeqCst) {
            self.connect().await?;
        }

        let request_id = request.id.to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Register pending response
        self.pending_responses
            .write()
            .await
            .insert(request_id.clone(), tx);

        // Send request
        let request_json = serde_json::to_string(&request)?;
        let mut process_guard = self.process.lock().await;

        if let Some(ref mut child) = *process_guard {
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(request_json.as_bytes()).await?;
                stdin.write_all(b"\n").await?;
                stdin.flush().await?;
            } else {
                return Err(anyhow!("Subprocess stdin not available"));
            }
        } else {
            return Err(anyhow!("Subprocess not running"));
        }

        drop(process_guard);

        // Wait for response with timeout
        // Use tokio::select! to properly cancel the pending request on timeout
        let timeout_future = tokio::time::sleep(Duration::from_secs(30));
        tokio::pin!(timeout_future);

        tokio::select! {
            response = rx => {
                response.context("Response channel closed")
            }
            _ = &mut timeout_future => {
                // On timeout, remove the pending request to prevent orphaned operations
                // This ensures the request handler won't try to send a response later
                self.pending_responses.write().await.remove(&request_id);
                Err(anyhow!("MCP tool request timed out after 30s. The in-flight request has been cancelled."))
            }
        }
    }

    /// Send a notification (no response expected).
    async fn send_notification(&self, notification: JsonRpcNotification) -> Result<()> {
        // Ensure connected
        if !self.connected.load(Ordering::SeqCst) {
            self.connect().await?;
        }

        let notification_json = serde_json::to_string(&notification)?;
        let mut process_guard = self.process.lock().await;

        if let Some(ref mut child) = *process_guard
            && let Some(ref mut stdin) = child.stdin
        {
            stdin.write_all(notification_json.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        }

        Ok(())
    }

    /// Generate next request ID.
    fn next_request_id(&self) -> RequestId {
        RequestId::Number(self.request_id.fetch_add(1, Ordering::SeqCst) as i64)
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::INITIALIZE)
            .with_params(serde_json::to_value(params)?);

        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("Initialize failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse initialize result")
    }

    async fn send_initialized(&self) -> Result<()> {
        let notification = JsonRpcNotification::new(methods::INITIALIZED);
        self.send_notification(notification).await
    }

    async fn list_tools(&self) -> Result<ListToolsResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::TOOLS_LIST);
        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("List tools failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse list tools result")
    }

    async fn call_tool(&self, params: CallToolParams) -> Result<CallToolResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::TOOLS_CALL)
            .with_params(serde_json::to_value(params)?);

        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("Call tool failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse call tool result")
    }

    async fn list_resources(&self) -> Result<ListResourcesResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::RESOURCES_LIST);
        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("List resources failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse list resources result")
    }

    async fn read_resource(&self, params: ReadResourceParams) -> Result<ReadResourceResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::RESOURCES_READ)
            .with_params(serde_json::to_value(params)?);

        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("Read resource failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse read resource result")
    }

    async fn list_prompts(&self) -> Result<ListPromptsResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::PROMPTS_LIST);
        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("List prompts failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse list prompts result")
    }

    async fn get_prompt(&self, params: GetPromptParams) -> Result<GetPromptResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::PROMPTS_GET)
            .with_params(serde_json::to_value(params)?);

        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("Get prompt failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse get prompt result")
    }

    async fn ping(&self) -> Result<()> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::PING);
        let response = self.send_request(request).await?;
        response
            .into_result()
            .map_err(|e| anyhow!("Ping failed: {}", e))?;
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        self.connected.store(false, Ordering::SeqCst);
        let mut process_guard = self.process.lock().await;

        if let Some(mut child) = process_guard.take() {
            child.kill().await.context("Failed to kill subprocess")?;
            info!("MCP subprocess terminated");
        }

        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

// ============================================================================
// HTTP/SSE Transport
// ============================================================================

/// HTTP/SSE transport for remote MCP servers.
pub struct HttpTransport {
    /// HTTP client.
    client: reqwest::Client,
    /// Base URL.
    base_url: url::Url,
    /// Request ID counter.
    request_id: AtomicU64,
    /// Whether the transport is connected.
    connected: AtomicBool,
    /// Reconnection settings.
    reconnect_config: ReconnectConfig,
    /// Custom headers.
    headers: HashMap<String, String>,
}

impl HttpTransport {
    /// Create a new HTTP transport.
    pub fn new(base_url: url::Url) -> Self {
        Self {
            client: create_default_client().expect("Failed to create HTTP client"),
            base_url,
            request_id: AtomicU64::new(1),
            connected: AtomicBool::new(false),
            reconnect_config: ReconnectConfig::default(),
            headers: HashMap::new(),
        }
    }

    /// Add custom header.
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set reconnection configuration.
    pub fn with_reconnect(mut self, config: ReconnectConfig) -> Self {
        self.reconnect_config = config;
        self
    }

    /// Send a JSON-RPC request over HTTP.
    async fn send_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        let mut req = self.client.post(self.base_url.clone()).json(&request);

        // Add custom headers
        for (key, value) in &self.headers {
            req = req.header(key, value);
        }

        let response = req.send().await.context("HTTP request failed")?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "HTTP request failed with status: {}",
                response.status()
            ));
        }

        let json_response: JsonRpcResponse =
            response.json().await.context("Failed to parse response")?;
        Ok(json_response)
    }

    /// Generate next request ID.
    fn next_request_id(&self) -> RequestId {
        RequestId::Number(self.request_id.fetch_add(1, Ordering::SeqCst) as i64)
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::INITIALIZE)
            .with_params(serde_json::to_value(params)?);

        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("Initialize failed: {}", e))?;

        self.connected.store(true, Ordering::SeqCst);
        serde_json::from_value(result).context("Failed to parse initialize result")
    }

    async fn send_initialized(&self) -> Result<()> {
        // HTTP transport doesn't need to send initialized notification
        Ok(())
    }

    async fn list_tools(&self) -> Result<ListToolsResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::TOOLS_LIST);
        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("List tools failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse list tools result")
    }

    async fn call_tool(&self, params: CallToolParams) -> Result<CallToolResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::TOOLS_CALL)
            .with_params(serde_json::to_value(params)?);

        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("Call tool failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse call tool result")
    }

    async fn list_resources(&self) -> Result<ListResourcesResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::RESOURCES_LIST);
        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("List resources failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse list resources result")
    }

    async fn read_resource(&self, params: ReadResourceParams) -> Result<ReadResourceResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::RESOURCES_READ)
            .with_params(serde_json::to_value(params)?);

        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("Read resource failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse read resource result")
    }

    async fn list_prompts(&self) -> Result<ListPromptsResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::PROMPTS_LIST);
        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("List prompts failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse list prompts result")
    }

    async fn get_prompt(&self, params: GetPromptParams) -> Result<GetPromptResult> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::PROMPTS_GET)
            .with_params(serde_json::to_value(params)?);

        let response = self.send_request(request).await?;
        let result = response
            .into_result()
            .map_err(|e| anyhow!("Get prompt failed: {}", e))?;

        serde_json::from_value(result).context("Failed to parse get prompt result")
    }

    async fn ping(&self) -> Result<()> {
        let request = JsonRpcRequest::new(self.next_request_id(), methods::PING);
        let response = self.send_request(request).await?;
        response
            .into_result()
            .map_err(|e| anyhow!("Ping failed: {}", e))?;
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        self.connected.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

// ============================================================================
// Reconnection Configuration
// ============================================================================

/// Reconnection configuration.
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// Whether reconnection is enabled.
    pub enabled: bool,
    /// Maximum number of reconnection attempts.
    pub max_attempts: u32,
    /// Initial delay between reconnection attempts.
    pub initial_delay: Duration,
    /// Maximum delay between reconnection attempts.
    pub max_delay: Duration,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 5,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl ReconnectConfig {
    /// Create a new reconnection config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Disable reconnection.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Set maximum attempts.
    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    /// Set initial delay.
    pub fn with_initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    /// Set maximum delay.
    pub fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconnect_config() {
        let config = ReconnectConfig::new()
            .with_max_attempts(10)
            .with_initial_delay(Duration::from_millis(500))
            .with_max_delay(Duration::from_secs(60));

        assert!(config.enabled);
        assert_eq!(config.max_attempts, 10);
        assert_eq!(config.initial_delay, Duration::from_millis(500));
        assert_eq!(config.max_delay, Duration::from_secs(60));
    }

    #[test]
    fn test_disabled_reconnect() {
        let config = ReconnectConfig::disabled();
        assert!(!config.enabled);
    }
}
