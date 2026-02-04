//! MCP Server core implementation.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{RwLock, oneshot};
use tracing::{debug, error, info, warn};

use cortex_mcp_types::{
    CallToolParams, CallToolResult, CancelledNotification, GetPromptParams, Implementation,
    InitializeParams, InitializeResult, JsonRpcError, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, ListPromptsResult, ListResourcesResult, ListToolsResult, LogLevel, LogMessage,
    ReadResourceParams, ReadResourceResult, RequestId, ServerCapabilities, SetLogLevelParams, Tool,
    methods,
};

use crate::handlers::ToolHandler;
use crate::providers::{PromptProvider, ResourceProvider};

// Helper trait for pipe syntax
trait Pipe: Sized {
    fn pipe<F, R>(self, f: F) -> R
    where
        F: FnOnce(Self) -> R,
    {
        f(self)
    }
}

impl<T> Pipe for T {}

// ============================================================================
// MCP Server
// ============================================================================

/// MCP server state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    /// Server is not initialized.
    Uninitialized,
    /// Server is initializing.
    Initializing,
    /// Server is ready to handle requests.
    Ready,
    /// Server is shutting down.
    ShuttingDown,
    /// Server has stopped.
    Stopped,
}

/// MCP Server implementation.
pub struct McpServer {
    /// Server implementation info.
    pub(crate) info: Implementation,
    /// Server capabilities.
    pub(crate) capabilities: ServerCapabilities,
    /// Registered tool handlers.
    pub(crate) tools: RwLock<HashMap<String, Arc<dyn ToolHandler>>>,
    /// Resource provider.
    pub(crate) resource_provider: RwLock<Option<Arc<dyn ResourceProvider>>>,
    /// Prompt provider.
    pub(crate) prompt_provider: RwLock<Option<Arc<dyn PromptProvider>>>,
    /// Current log level.
    pub(crate) log_level: RwLock<LogLevel>,
    /// Server state.
    pub(crate) state: RwLock<ServerState>,
    /// Whether the server is running.
    pub(crate) running: AtomicBool,
    /// Request ID counter.
    pub(crate) request_id: AtomicU64,
    /// Pending requests for cancellation.
    pub(crate) pending_requests: RwLock<HashMap<String, oneshot::Sender<()>>>,
    /// Client info (set after initialization).
    pub(crate) client_info: RwLock<Option<Implementation>>,
    /// Protocol version negotiated.
    pub(crate) protocol_version: RwLock<Option<String>>,
    /// Optional instructions for clients.
    pub(crate) instructions: Option<String>,
}

impl McpServer {
    /// Create a new MCP server.
    pub fn new(info: Implementation, capabilities: ServerCapabilities) -> Self {
        Self {
            info,
            capabilities,
            tools: RwLock::new(HashMap::new()),
            resource_provider: RwLock::new(None),
            prompt_provider: RwLock::new(None),
            log_level: RwLock::new(LogLevel::Info),
            state: RwLock::new(ServerState::Uninitialized),
            running: AtomicBool::new(false),
            request_id: AtomicU64::new(1),
            pending_requests: RwLock::new(HashMap::new()),
            client_info: RwLock::new(None),
            protocol_version: RwLock::new(None),
            instructions: None,
        }
    }

    /// Get server info.
    pub fn info(&self) -> &Implementation {
        &self.info
    }

    /// Get server capabilities.
    pub fn capabilities(&self) -> &ServerCapabilities {
        &self.capabilities
    }

    /// Get current state.
    pub async fn state(&self) -> ServerState {
        *self.state.read().await
    }

    /// Check if server is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Register a tool handler.
    pub async fn register_tool(&self, handler: Arc<dyn ToolHandler>) {
        let tool = handler.tool();
        let name = tool.name.clone();
        self.tools.write().await.insert(name.clone(), handler);
        debug!(tool = %name, "Registered tool");
    }

    /// Register multiple tool handlers.
    pub async fn register_tools(&self, handlers: Vec<Arc<dyn ToolHandler>>) {
        for handler in handlers {
            self.register_tool(handler).await;
        }
    }

    /// Set the resource provider.
    pub async fn set_resource_provider(&self, provider: Arc<dyn ResourceProvider>) {
        *self.resource_provider.write().await = Some(provider);
    }

    /// Set the prompt provider.
    pub async fn set_prompt_provider(&self, provider: Arc<dyn PromptProvider>) {
        *self.prompt_provider.write().await = Some(provider);
    }

    /// Get all registered tools.
    pub async fn tools(&self) -> Vec<Tool> {
        self.tools.read().await.values().map(|h| h.tool()).collect()
    }

    /// Get current log level.
    pub async fn log_level(&self) -> LogLevel {
        *self.log_level.read().await
    }

    /// Set log level.
    pub async fn set_log_level(&self, level: LogLevel) {
        *self.log_level.write().await = level;
    }

    /// Generate a new request ID.
    pub fn next_request_id(&self) -> RequestId {
        RequestId::Number(self.request_id.fetch_add(1, Ordering::SeqCst) as i64)
    }

    // ========================================================================
    // Request Handlers
    // ========================================================================

    /// Handle a JSON-RPC request.
    pub async fn handle_request(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        debug!(method = %request.method, id = %request.id, "Handling request");

        let result = match request.method.as_str() {
            methods::INITIALIZE => self.handle_initialize(request.params).await,
            methods::PING => Ok(json!({})),
            methods::TOOLS_LIST => self.handle_list_tools().await,
            methods::TOOLS_CALL => self.handle_call_tool(request.params).await,
            methods::RESOURCES_LIST => self.handle_list_resources().await,
            methods::RESOURCES_READ => self.handle_read_resource(request.params).await,
            methods::PROMPTS_LIST => self.handle_list_prompts().await,
            methods::PROMPTS_GET => self.handle_get_prompt(request.params).await,
            methods::LOGGING_SET_LEVEL => self.handle_set_log_level(request.params).await,
            methods::ROOTS_LIST => self.handle_list_roots().await,
            _ => Err(JsonRpcError::method_not_found(&request.method)),
        };

        match result {
            Ok(value) => JsonRpcResponse::success(request.id, value),
            Err(error) => JsonRpcResponse::error(request.id, error),
        }
    }

    /// Handle a JSON-RPC notification.
    pub async fn handle_notification(&self, notification: JsonRpcNotification) {
        debug!(method = %notification.method, "Handling notification");

        match notification.method.as_str() {
            methods::INITIALIZED => {
                *self.state.write().await = ServerState::Ready;
                info!("Server initialized and ready");
            }
            methods::CANCELLED => {
                if let Some(params) = notification.params
                    && let Ok(cancelled) = serde_json::from_value::<CancelledNotification>(params)
                {
                    self.handle_cancellation(cancelled).await;
                }
            }
            methods::ROOTS_LIST_CHANGED => {
                debug!("Roots list changed notification received");
            }
            _ => {
                warn!(method = %notification.method, "Unknown notification");
            }
        }
    }

    async fn handle_initialize(&self, params: Option<Value>) -> Result<Value, JsonRpcError> {
        // Atomic check-and-transition: hold write lock during entire state check and modification
        // to prevent TOCTOU race conditions where multiple concurrent initialize requests
        // could both pass the uninitialized check before either sets the state
        {
            let mut state_guard = self.state.write().await;
            if *state_guard != ServerState::Uninitialized {
                return Err(JsonRpcError::invalid_request("Server already initialized"));
            }
            *state_guard = ServerState::Initializing;
        }

        // Parse params
        let init_params: InitializeParams = params
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| JsonRpcError::invalid_params(format!("Invalid params: {e}")))?
            .unwrap_or_default();

        // Store client info
        *self.client_info.write().await = Some(init_params.client_info.clone());
        *self.protocol_version.write().await = Some(init_params.protocol_version.clone());

        info!(
            client = %init_params.client_info.name,
            version = %init_params.client_info.version,
            protocol = %init_params.protocol_version,
            "Client connected"
        );

        // Build result
        let result = InitializeResult {
            protocol_version: cortex_mcp_types::PROTOCOL_VERSION.to_string(),
            capabilities: self.capabilities.clone(),
            server_info: self.info.clone(),
            instructions: self.instructions.clone(),
        };

        serde_json::to_value(result).map_err(|e| JsonRpcError::internal_error(e.to_string()))
    }

    async fn handle_list_tools(&self) -> Result<Value, JsonRpcError> {
        let tools = self.tools().await;
        let result = ListToolsResult::new(tools);
        serde_json::to_value(result).map_err(|e| JsonRpcError::internal_error(e.to_string()))
    }

    async fn handle_call_tool(&self, params: Option<Value>) -> Result<Value, JsonRpcError> {
        let call_params: CallToolParams = params
            .ok_or_else(|| JsonRpcError::invalid_params("Missing params"))?
            .pipe(serde_json::from_value)
            .map_err(|e| JsonRpcError::invalid_params(format!("Invalid params: {e}")))?;

        debug!(tool = %call_params.name, "Calling tool");

        // Get handler
        let handlers = self.tools.read().await;
        let handler = handlers.get(&call_params.name).cloned().ok_or_else(|| {
            JsonRpcError::invalid_params(format!("Unknown tool: {}", call_params.name))
        })?;
        drop(handlers);

        // Execute tool
        let arguments = call_params.arguments.unwrap_or(json!({}));
        let result = handler.execute(arguments).await;

        match result {
            Ok(call_result) => serde_json::to_value(call_result)
                .map_err(|e| JsonRpcError::internal_error(e.to_string())),
            Err(e) => {
                let error_result = CallToolResult::error(e.to_string());
                serde_json::to_value(error_result)
                    .map_err(|e| JsonRpcError::internal_error(e.to_string()))
            }
        }
    }

    async fn handle_list_resources(&self) -> Result<Value, JsonRpcError> {
        let provider = self.resource_provider.read().await;
        let resources = match provider.as_ref() {
            Some(p) => p
                .list()
                .await
                .map_err(|e| JsonRpcError::internal_error(e.to_string()))?,
            None => Vec::new(),
        };
        let result = ListResourcesResult::new(resources);
        serde_json::to_value(result).map_err(|e| JsonRpcError::internal_error(e.to_string()))
    }

    async fn handle_read_resource(&self, params: Option<Value>) -> Result<Value, JsonRpcError> {
        let read_params: ReadResourceParams = params
            .ok_or_else(|| JsonRpcError::invalid_params("Missing params"))?
            .pipe(serde_json::from_value)
            .map_err(|e| JsonRpcError::invalid_params(format!("Invalid params: {e}")))?;

        let provider = self.resource_provider.read().await;
        let content = match provider.as_ref() {
            Some(p) => p
                .read(&read_params.uri)
                .await
                .map_err(|e| JsonRpcError::invalid_params(e.to_string()))?,
            None => {
                return Err(JsonRpcError::invalid_params(format!(
                    "Resource not found: {}",
                    read_params.uri
                )));
            }
        };

        let result = ReadResourceResult {
            contents: vec![content],
        };
        serde_json::to_value(result).map_err(|e| JsonRpcError::internal_error(e.to_string()))
    }

    async fn handle_list_prompts(&self) -> Result<Value, JsonRpcError> {
        let provider = self.prompt_provider.read().await;
        let prompts = match provider.as_ref() {
            Some(p) => p
                .list()
                .await
                .map_err(|e| JsonRpcError::internal_error(e.to_string()))?,
            None => Vec::new(),
        };
        let result = ListPromptsResult::new(prompts);
        serde_json::to_value(result).map_err(|e| JsonRpcError::internal_error(e.to_string()))
    }

    async fn handle_get_prompt(&self, params: Option<Value>) -> Result<Value, JsonRpcError> {
        let get_params: GetPromptParams = params
            .ok_or_else(|| JsonRpcError::invalid_params("Missing params"))?
            .pipe(serde_json::from_value)
            .map_err(|e| JsonRpcError::invalid_params(format!("Invalid params: {e}")))?;

        let provider = self.prompt_provider.read().await;
        let result = match provider.as_ref() {
            Some(p) => p
                .get(&get_params.name, get_params.arguments)
                .await
                .map_err(|e| JsonRpcError::invalid_params(e.to_string()))?,
            None => {
                return Err(JsonRpcError::invalid_params(format!(
                    "Prompt not found: {}",
                    get_params.name
                )));
            }
        };

        serde_json::to_value(result).map_err(|e| JsonRpcError::internal_error(e.to_string()))
    }

    async fn handle_set_log_level(&self, params: Option<Value>) -> Result<Value, JsonRpcError> {
        let level_params: SetLogLevelParams = params
            .ok_or_else(|| JsonRpcError::invalid_params("Missing params"))?
            .pipe(serde_json::from_value)
            .map_err(|e| JsonRpcError::invalid_params(format!("Invalid params: {e}")))?;

        *self.log_level.write().await = level_params.level;
        debug!(level = %level_params.level, "Log level changed");

        Ok(json!({}))
    }

    async fn handle_list_roots(&self) -> Result<Value, JsonRpcError> {
        // Default implementation returns empty roots
        Ok(json!({ "roots": [] }))
    }

    async fn handle_cancellation(&self, cancelled: CancelledNotification) {
        let request_id = cancelled.request_id.to_string();
        if let Some(sender) = self.pending_requests.write().await.remove(&request_id) {
            let _ = sender.send(());
            debug!(request_id = %request_id, "Request cancelled");
        }
    }

    // ========================================================================
    // Transport: Stdio
    // ========================================================================

    /// Run the server with stdio transport.
    pub async fn run_stdio(self: Arc<Self>) -> Result<()> {
        info!(server = %self.info.name, "Starting MCP server with stdio transport");
        self.running.store(true, Ordering::SeqCst);

        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let mut reader = BufReader::new(stdin);
        let mut stdout = stdout;

        let mut line = String::new();

        while self.running.load(Ordering::SeqCst) {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    // EOF
                    debug!("EOF received, shutting down");
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    // Try to parse as request first
                    if let Ok(request) = serde_json::from_str::<JsonRpcRequest>(trimmed) {
                        let response = self.handle_request(request).await;
                        let response_json = serde_json::to_string(&response)
                            .context("Failed to serialize response")?;
                        stdout
                            .write_all(response_json.as_bytes())
                            .await
                            .context("Failed to write response")?;
                        stdout.write_all(b"\n").await?;
                        stdout.flush().await?;
                    }
                    // Try notification
                    else if let Ok(notification) =
                        serde_json::from_str::<JsonRpcNotification>(trimmed)
                    {
                        self.handle_notification(notification).await;
                    }
                    // Invalid message
                    else {
                        warn!(line = %trimmed, "Invalid JSON-RPC message");
                        let error_response = JsonRpcResponse::error(
                            RequestId::Number(0),
                            JsonRpcError::parse_error("Invalid JSON"),
                        );
                        let error_json = serde_json::to_string(&error_response)?;
                        stdout.write_all(error_json.as_bytes()).await?;
                        stdout.write_all(b"\n").await?;
                        stdout.flush().await?;
                    }
                }
                Err(e) => {
                    error!(error = %e, "Error reading from stdin");
                    break;
                }
            }
        }

        *self.state.write().await = ServerState::Stopped;
        self.running.store(false, Ordering::SeqCst);
        info!("MCP server stopped");

        Ok(())
    }

    /// Run the server with blocking stdio (for synchronous contexts).
    pub fn run_stdio_blocking(self: Arc<Self>) -> Result<()> {
        info!(server = %self.info.name, "Starting MCP server with blocking stdio transport");
        self.running.store(true, Ordering::SeqCst);

        let stdin = std::io::stdin();
        let stdout = std::io::stdout();

        let handle = stdin.lock();
        let mut stdout_handle = stdout.lock();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to create runtime")?;

        for line in handle.lines() {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            let line = line.context("Failed to read line")?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Try to parse as request first
            if let Ok(request) = serde_json::from_str::<JsonRpcRequest>(trimmed) {
                let response = rt.block_on(self.handle_request(request));
                let response_json =
                    serde_json::to_string(&response).context("Failed to serialize response")?;
                writeln!(stdout_handle, "{response_json}").context("Failed to write response")?;
                stdout_handle.flush()?;
            }
            // Try notification
            else if let Ok(notification) = serde_json::from_str::<JsonRpcNotification>(trimmed) {
                rt.block_on(self.handle_notification(notification));
            }
            // Invalid message
            else {
                warn!(line = %trimmed, "Invalid JSON-RPC message");
                let error_response = JsonRpcResponse::error(
                    RequestId::Number(0),
                    JsonRpcError::parse_error("Invalid JSON"),
                );
                let error_json = serde_json::to_string(&error_response)?;
                writeln!(stdout_handle, "{error_json}")?;
                stdout_handle.flush()?;
            }
        }

        rt.block_on(async {
            *self.state.write().await = ServerState::Stopped;
        });
        self.running.store(false, Ordering::SeqCst);
        info!("MCP server stopped");

        Ok(())
    }

    // ========================================================================
    // Transport: HTTP
    // ========================================================================

    /// Run the server with HTTP transport on the given address.
    #[cfg(feature = "http")]
    pub async fn run_http(self: Arc<Self>, addr: std::net::SocketAddr) -> Result<()> {
        use axum::{Json, Router, extract::State, http::StatusCode, routing::post};

        info!(server = %self.info.name, addr = %addr, "Starting MCP server with HTTP transport");
        self.running.store(true, Ordering::SeqCst);

        async fn handle_json_rpc(
            State(server): State<Arc<McpServer>>,
            Json(request): Json<JsonRpcRequest>,
        ) -> (StatusCode, Json<JsonRpcResponse>) {
            let response = server.handle_request(request).await;
            (StatusCode::OK, Json(response))
        }

        let app = Router::new()
            .route("/", post(handle_json_rpc))
            .with_state(self.clone());

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        *self.state.write().await = ServerState::Stopped;
        self.running.store(false, Ordering::SeqCst);

        Ok(())
    }

    /// Stop the server.
    pub async fn stop(&self) {
        info!("Stopping MCP server");
        *self.state.write().await = ServerState::ShuttingDown;
        self.running.store(false, Ordering::SeqCst);
    }

    // ========================================================================
    // Notification Sending
    // ========================================================================

    /// Send a log message notification.
    pub fn create_log_notification(
        &self,
        level: LogLevel,
        message: impl Into<String>,
    ) -> JsonRpcNotification {
        let log_message = LogMessage::new(level, json!({ "message": message.into() }));
        JsonRpcNotification::new(methods::LOG_MESSAGE)
            .with_params(serde_json::to_value(log_message).unwrap_or(json!({})))
    }

    /// Create a tools list changed notification.
    pub fn create_tools_changed_notification(&self) -> JsonRpcNotification {
        JsonRpcNotification::new(methods::TOOLS_LIST_CHANGED)
    }

    /// Create a resources list changed notification.
    pub fn create_resources_changed_notification(&self) -> JsonRpcNotification {
        JsonRpcNotification::new(methods::RESOURCES_LIST_CHANGED)
    }

    /// Create a prompts list changed notification.
    pub fn create_prompts_changed_notification(&self) -> JsonRpcNotification {
        JsonRpcNotification::new(methods::PROMPTS_LIST_CHANGED)
    }
}
