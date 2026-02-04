//! Core EventLoop struct definition and main run loop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::actions::{ActionContext, ActionMapper};
use crate::app::AppState;
use crate::bridge::{SessionBridge, StreamController};
use crate::commands::{CommandExecutor, FormRegistry};
use crate::events::ToolEvent;
use crate::input::{ClickZoneRegistry, MouseHandler};
use crate::modal::ModalStack;
use crate::permissions::PermissionManager;
use crate::providers::ProviderManager;
use crate::runner::card_handler::CardHandler;
use crate::runner::terminal::CortexTerminal;
use crate::session::CortexSession;

use crate::capture::TuiCapture;
use cortex_core::EngineEvent;
use cortex_engine::streaming::StreamEvent;
use cortex_engine::tools::{ToolRegistry, UnifiedToolExecutor};

// ============================================================================
// ERROR MESSAGE HELPERS
// ============================================================================

/// Returns the error message as-is for debugging.
/// Previously simplified errors, but this hid useful information.
pub fn simplify_error_message(error: &str) -> String {
    // Return the full error message for better debugging
    // Log a debug hint based on error type
    let lower = error.to_lowercase();

    if lower.contains("401") || lower.contains("unauthorized") {
        tracing::debug!("Auth error hint: Run 'cortex login' to re-authenticate");
    } else if lower.contains("403") || lower.contains("forbidden") {
        tracing::debug!("Permission error hint: Check API key or subscription");
    }

    error.to_string()
}

// ============================================================================
// EVENT LOOP STRUCT
// ============================================================================

/// Pending tool call information.
#[derive(Debug, Clone)]
pub struct PendingToolCall {
    /// Tool call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Tool arguments.
    pub arguments: serde_json::Value,
}

/// Main event loop for the Cortex TUI application.
///
/// This struct coordinates the 120 FPS render loop with backend events,
/// handling user input, streaming responses, tool executions, and UI updates.
pub struct EventLoop {
    /// Application state containing all UI state.
    pub app_state: AppState,

    /// Session bridge for backend communication.
    pub(super) session_bridge: Option<SessionBridge>,

    /// Stream controller for managing streaming state.
    pub(super) stream_controller: StreamController,

    /// Action mapper for keybindings.
    pub(super) action_mapper: ActionMapper,

    /// Click zone registry for mouse interaction.
    pub(super) click_zones: ClickZoneRegistry,

    /// Mouse handler for click/drag/scroll.
    pub(super) mouse_handler: MouseHandler,

    /// Modal stack for overlay dialogs.
    pub(super) modal_stack: ModalStack,

    /// Command executor for slash commands.
    pub(super) command_executor: CommandExecutor,

    /// Form registry for command forms.
    pub(super) _form_registry: FormRegistry,

    /// Permission manager for tool approvals.
    pub(super) permission_manager: PermissionManager,

    /// Provider manager for AI providers.
    pub(super) provider_manager: Option<Arc<tokio::sync::RwLock<ProviderManager>>>,

    /// Subagent executor for running subagents.
    pub(super) _subagent_executor:
        Option<Arc<cortex_engine::tools::handlers::subagent::SubagentExecutor>>,

    /// Whether the event loop is running.
    pub(super) running: Arc<AtomicBool>,

    /// Background task handles.
    pub(super) background_tasks: Vec<JoinHandle<()>>,

    /// Last render timestamp for frame timing.
    pub(super) last_render: Instant,

    /// Minimum frame time for 120 FPS.
    pub(super) min_frame_time: Duration,

    /// Current Cortex session.
    pub(super) cortex_session: Option<CortexSession>,

    /// Tool registry for available tools.
    pub(super) tool_registry: Option<Arc<ToolRegistry>>,

    /// Whether streaming was cancelled.
    pub(super) streaming_cancelled: Arc<AtomicBool>,

    /// Whether stream done was received.
    pub(super) stream_done_received: bool,

    /// Receiver for streaming events.
    pub(super) streaming_rx: Option<mpsc::Receiver<StreamEvent>>,

    /// Handle to the streaming task.
    pub(super) streaming_task: Option<JoinHandle<()>>,

    /// Card handler for card-based UI.
    pub(super) card_handler: CardHandler,

    /// Unified tool executor.
    pub(super) unified_executor: Option<Arc<UnifiedToolExecutor>>,

    /// Pending assistant tool calls.
    pub(super) pending_assistant_tool_calls: Vec<PendingToolCall>,

    /// Running tool tasks.
    pub(super) running_tool_tasks: std::collections::HashMap<String, JoinHandle<()>>,

    /// Running subagents.
    pub(super) running_subagents: std::collections::HashMap<String, JoinHandle<()>>,

    /// Tool execution started flag.
    pub(super) tool_execution_started: bool,

    /// Channel for receiving tool execution events.
    pub(super) tool_event_tx: mpsc::Sender<ToolEvent>,
    pub(super) tool_event_rx: Option<mpsc::Receiver<ToolEvent>>,

    /// Whether the current streaming is a continuation after tool results.
    /// When true, tool calls should NOT be cleared on StreamEvent::Done.
    pub(super) is_continuation: bool,

    /// Undo stack for session message exchanges.
    /// Each entry is a pair of (user_message, assistant_message).
    pub(super) _undo_stack: Vec<Vec<cortex_core::widgets::Message>>,

    /// TUI capture manager for debugging (enabled via CORTEX_TUI_CAPTURE=1).
    pub(super) tui_capture: TuiCapture,
}

impl EventLoop {
    /// Creates a new EventLoop with the given application state.
    pub fn new(app_state: AppState) -> Self {
        // Create channel for tool execution events
        let (tool_event_tx, tool_event_rx) = mpsc::channel::<ToolEvent>(100);

        // Initialize TUI capture with terminal size from app state
        let (width, height) = app_state.terminal_size;
        let tui_capture = TuiCapture::new(width, height);

        Self {
            app_state,
            session_bridge: None,
            stream_controller: StreamController::new(),
            action_mapper: ActionMapper::default(),
            click_zones: ClickZoneRegistry::new(),
            mouse_handler: MouseHandler::new(),
            modal_stack: ModalStack::new(),
            command_executor: CommandExecutor::new(),
            _form_registry: FormRegistry::new(),
            permission_manager: PermissionManager::new(),
            provider_manager: None,
            _subagent_executor: None,
            running: Arc::new(AtomicBool::new(false)),
            background_tasks: Vec::new(),
            last_render: Instant::now(),
            min_frame_time: Duration::from_micros(8333), // ~120 FPS
            cortex_session: None,
            tool_registry: None,
            streaming_cancelled: Arc::new(AtomicBool::new(false)),
            stream_done_received: false,
            streaming_rx: None,
            streaming_task: None,
            card_handler: CardHandler::new(),
            unified_executor: None,
            pending_assistant_tool_calls: Vec::new(),
            running_tool_tasks: std::collections::HashMap::new(),
            running_subagents: std::collections::HashMap::new(),
            tool_execution_started: false,
            tool_event_tx,
            tool_event_rx: Some(tool_event_rx),
            is_continuation: false,
            _undo_stack: Vec::new(),
            tui_capture,
        }
    }

    /// Sets the provider manager.
    pub fn with_provider_manager(mut self, manager: ProviderManager) -> Self {
        self.provider_manager = Some(Arc::new(tokio::sync::RwLock::new(manager)));
        self
    }

    /// Sets the unified executor.
    pub fn with_unified_executor(mut self, executor: Arc<UnifiedToolExecutor>) -> Self {
        self.unified_executor = Some(executor);
        self
    }

    /// Sets the session bridge.
    pub fn with_session(mut self, bridge: SessionBridge) -> Self {
        self.session_bridge = Some(bridge);
        self
    }

    /// Sets the cortex session.
    pub fn with_cortex_session(mut self, session: CortexSession) -> Self {
        self.cortex_session = Some(session);
        self
    }

    /// Sets the tool registry for executing tools.
    pub fn with_tool_registry(mut self, registry: Arc<ToolRegistry>) -> Self {
        self.tool_registry = Some(registry);
        self
    }

    /// Runs the main event loop.
    ///
    /// This method initializes the FrameEngine to poll keyboard, mouse, and
    /// terminal events, then dispatches them to `handle_engine_event()` for
    /// processing. It also polls streaming events from the LLM backend.
    pub async fn run(&mut self, terminal: &mut CortexTerminal) -> Result<()> {
        use cortex_core::frame_engine::FrameEngine;

        self.running.store(true, Ordering::SeqCst);

        // Create channel for receiving events from FrameEngine
        let (action_tx, mut action_rx) = tokio::sync::mpsc::channel::<EngineEvent>(256);

        // Create and spawn the FrameEngine
        let running = self.running.clone();
        let mut frame_engine = FrameEngine::new(action_tx, running);

        let engine_handle = tokio::spawn(async move {
            if let Err(e) = frame_engine.run().await {
                tracing::error!("FrameEngine error: {}", e);
            }
        });

        // Initial render
        self.render(terminal)?;

        // Check token expiration and handle accordingly
        let session_expired = if let Ok(Some(auth)) = cortex_login::load_from_keyring() {
            if auth.is_expired() {
                // Session is already expired
                true
            } else if auth.expires_soon(86400) {
                // Session expires within 24 hours but is still valid
                if let Some(remaining) = auth.time_until_expiry() {
                    if remaining > 0 {
                        let hours = remaining / 3600;
                        let minutes = (remaining % 3600) / 60;
                        if hours > 0 {
                            self.app_state.toasts.warning(format!(
                                "Session expires in {} hours. Run /login to refresh.",
                                hours
                            ));
                        } else if minutes > 0 {
                            self.app_state.toasts.warning(format!(
                                "Session expires in {} minutes. Run /login to refresh.",
                                minutes
                            ));
                        }
                        false // Session still valid
                    } else {
                        // remaining <= 0 means expired
                        true
                    }
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        // If session is expired, automatically redirect to login page
        if session_expired {
            self.app_state
                .toasts
                .warning("Session expired. Redirecting to login...");
            self.start_login_flow().await;
        }

        // Main event loop: poll BOTH FrameEngine events AND streaming events
        loop {
            // Check exit conditions
            if !self.running.load(Ordering::SeqCst) || self.app_state.should_quit() {
                break;
            }

            // Use tokio::select! to poll multiple event sources concurrently
            tokio::select! {
                // Branch 1: FrameEngine events (keyboard, mouse, ticks, resize)
                Some(event) = action_rx.recv() => {
                    // Handle quit event
                    if matches!(event, EngineEvent::Quit) {
                        self.app_state.set_quit();
                        break;
                    }

                    // Dispatch event to handler
                    if let Err(e) = self.handle_engine_event(event, terminal).await {
                        tracing::error!("Error handling engine event: {}", e);
                    }
                }

                // Branch 2: Streaming events from LLM (Delta, Done, Error, ToolCall)
                Some(stream_event) = async {
                    match self.streaming_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.handle_stream_event(stream_event).await;
                    // Re-render after streaming update to show new content
                    if let Err(e) = self.render(terminal) {
                        tracing::error!("Error rendering after stream event: {}", e);
                    }
                }

                // Branch 3: Tool execution events (Started, Output, Completed, Failed)
                Some(tool_event) = async {
                    match self.tool_event_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.handle_tool_event(tool_event).await;
                    // Re-render after tool event to show status update
                    if let Err(e) = self.render(terminal) {
                        tracing::error!("Error rendering after tool event: {}", e);
                    }
                }
            }
        }

        // Cleanup: signal engine to stop and wait for it
        self.running.store(false, Ordering::SeqCst);
        let _ = engine_handle.await;

        // Cleanup: abort all running background tasks to prevent orphaned threads
        // This ensures clean process termination on shutdown
        self.cleanup_background_tasks().await;

        // Export TUI captures if enabled
        if self.tui_capture.is_enabled() {
            // Take ownership of capture manager for export
            let capture = std::mem::take(&mut self.tui_capture);
            if let Err(e) = capture.export().await {
                tracing::warn!("Failed to export TUI capture: {}", e);
            }
        }

        Ok(())
    }

    /// Syncs the permission mode between app state and permission manager.
    pub(super) fn sync_permission_mode(&mut self) {
        self.permission_manager.mode = match self.app_state.permission_mode {
            crate::permissions::PermissionMode::Yolo => crate::permissions::PermissionMode::Yolo,
            crate::permissions::PermissionMode::Low => crate::permissions::PermissionMode::Low,
            crate::permissions::PermissionMode::Medium => {
                crate::permissions::PermissionMode::Medium
            }
            crate::permissions::PermissionMode::High => crate::permissions::PermissionMode::High,
        };
    }

    /// Updates the session's model metadata and persists it.
    /// This ensures the model is saved with the session for /load to display correctly.
    pub(super) fn update_session_model(&mut self, model_id: &str) {
        if let Some(ref mut session) = self.cortex_session {
            session.meta.model = model_id.to_string();
            if let Err(e) = session.save() {
                tracing::error!("Failed to save session model: {}", e);
            }
        }
    }

    /// Cancels the current streaming operation.
    ///
    /// This method properly cleans up:
    /// - Sets the cancellation flag to signal the streaming task to stop
    /// - Aborts the streaming task if it doesn't respond to the flag
    /// - Closes the streaming channel to prevent resource leaks
    /// - Resets the stream controller state
    pub(super) fn cancel_streaming(&mut self) {
        if self.streaming_task.is_some() {
            // Signal cancellation first (gives the task a chance to clean up gracefully)
            self.streaming_cancelled.store(true, Ordering::SeqCst);
            self.stream_controller.interrupt();
            self.app_state.stop_streaming();
            self.add_system_message("Streaming cancelled.");

            // Close the channel first to unblock any pending sends
            // This ensures the HTTP connection is released when the stream is dropped
            self.streaming_rx = None;

            // Abort the task if it hasn't already stopped
            // This ensures any hanging network operations are terminated
            if let Some(task) = self.streaming_task.take() {
                task.abort();
            }

            // Also abort any running tool tasks to prevent orphaned operations
            for (_, task) in self.running_tool_tasks.drain() {
                task.abort();
            }

            // Abort any running subagent tasks
            for (_, task) in self.running_subagents.drain() {
                task.abort();
            }

            // Reset ESC timer after cancellation to prevent accidental quit
            self.app_state.reset_esc();

            self.stream_controller.reset();
        }
    }

    /// Cleanup all running background tasks during shutdown.
    ///
    /// This ensures that worker threads are properly joined or aborted
    /// before the process exits, preventing orphaned threads and ensuring
    /// clean termination.
    pub(super) async fn cleanup_background_tasks(&mut self) {
        use tokio::time::{Duration, timeout};

        tracing::debug!(
            "Cleaning up background tasks: {} tool tasks, {} subagents, {} streaming",
            self.running_tool_tasks.len(),
            self.running_subagents.len(),
            if self.streaming_task.is_some() { 1 } else { 0 }
        );

        // Signal cancellation for streaming
        self.streaming_cancelled.store(true, Ordering::SeqCst);

        // Abort streaming task
        if let Some(task) = self.streaming_task.take() {
            task.abort();
        }
        self.streaming_rx = None;

        // Abort all running tool tasks with a timeout
        let tool_tasks: Vec<_> = self.running_tool_tasks.drain().collect();
        for (id, task) in tool_tasks {
            tracing::debug!("Aborting tool task: {}", id);
            task.abort();
            // Give a brief moment for cleanup
            let _ = timeout(Duration::from_millis(100), async {
                let _ = task.await;
            })
            .await;
        }

        // Abort all running subagent tasks
        let subagent_tasks: Vec<_> = self.running_subagents.drain().collect();
        for (id, task) in subagent_tasks {
            tracing::debug!("Aborting subagent task: {}", id);
            task.abort();
            let _ = timeout(Duration::from_millis(100), async {
                let _ = task.await;
            })
            .await;
        }

        // Abort general background tasks
        for task in self.background_tasks.drain(..) {
            task.abort();
        }

        tracing::debug!("Background task cleanup complete");
    }

    /// Adds a system message to the chat display.
    pub(super) fn add_system_message(&mut self, content: &str) {
        let message = cortex_core::widgets::Message::system(content);
        self.app_state.add_message(message);
    }

    /// Returns the current action context based on app state.
    pub(super) fn get_action_context(&self) -> ActionContext {
        use crate::app::FocusTarget;

        if self.app_state.pending_approval.is_some() {
            ActionContext::Approval
        } else {
            match self.app_state.focus {
                FocusTarget::Input => ActionContext::Input,
                FocusTarget::Chat => ActionContext::Chat,
                FocusTarget::Sidebar => ActionContext::Sidebar,
                // Modals handle their own key events, so treat as approval context
                FocusTarget::Modal => ActionContext::Approval,
            }
        }
    }

    /// Returns whether the event loop is currently running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Signals the event loop to stop.
    ///
    /// This sets the running flag to false, causing the main loop to exit
    /// on the next iteration.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Returns a reference to the application state.
    pub fn app_state(&self) -> &AppState {
        &self.app_state
    }

    /// Returns a mutable reference to the application state.
    pub fn app_state_mut(&mut self) -> &mut AppState {
        &mut self.app_state
    }

    /// Returns a reference to the stream controller.
    pub fn stream_controller(&self) -> &StreamController {
        &self.stream_controller
    }

    /// Returns a mutable reference to the stream controller.
    pub fn stream_controller_mut(&mut self) -> &mut StreamController {
        &mut self.stream_controller
    }

    /// Returns a reference to the session bridge if attached.
    pub fn session_bridge(&self) -> Option<&SessionBridge> {
        self.session_bridge.as_ref()
    }

    /// Attaches a session bridge to the event loop.
    ///
    /// This can be called after construction to attach a session.
    pub fn set_session_bridge(&mut self, bridge: SessionBridge) {
        self.session_bridge = Some(bridge);
    }

    /// Removes and returns the session bridge.
    pub fn take_session_bridge(&mut self) -> Option<SessionBridge> {
        self.session_bridge.take()
    }

    /// Returns a reference to the click zone registry.
    pub fn click_zones(&self) -> &ClickZoneRegistry {
        &self.click_zones
    }

    /// Returns a mutable reference to the click zone registry.
    pub fn click_zones_mut(&mut self) -> &mut ClickZoneRegistry {
        &mut self.click_zones
    }

    /// Returns a reference to the mouse handler.
    pub fn mouse_handler(&self) -> &MouseHandler {
        &self.mouse_handler
    }

    /// Returns a mutable reference to the mouse handler.
    pub fn mouse_handler_mut(&mut self) -> &mut MouseHandler {
        &mut self.mouse_handler
    }

    /// Get a reference to the card handler.
    pub fn card_handler(&self) -> &CardHandler {
        &self.card_handler
    }

    /// Get a mutable reference to the card handler.
    pub fn card_handler_mut(&mut self) -> &mut CardHandler {
        &mut self.card_handler
    }

    /// Opens a modal onto the modal stack.
    pub fn open_modal(&mut self, modal: Box<dyn crate::modal::Modal>) {
        self.modal_stack.push(modal);
    }
}

// ============================================================================
// APP STATE EXTENSION
// ============================================================================

impl AppState {
    /// Returns whether the application should quit.
    ///
    /// This is used by the event loop to determine when to exit.
    pub fn should_quit(&self) -> bool {
        !self.running
    }

    /// Signals that the application should quit.
    pub fn set_quit(&mut self) {
        self.running = false;
    }
}

// ============================================================================
// BROWSER HELPER
// ============================================================================

/// Opens a URL in the default browser.
///
/// This function attempts to open a URL in the system's default browser.
/// It validates the URL for security (only http/https allowed) and fails silently
/// if the browser cannot be opened (common in headless environments).
///
/// # Security
/// - Only allows HTTP and HTTPS URLs
/// - Rejects URLs with embedded credentials
/// - Validates against shell metacharacters for defense in depth
pub fn open_browser_url(url: &str) -> Result<()> {
    // Parse and validate the URL
    let parsed_url = url::Url::parse(url).map_err(|e| anyhow::anyhow!("Invalid URL: {}", e))?;

    // Only allow HTTP and HTTPS URLs
    match parsed_url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(anyhow::anyhow!(
                "Refusing to open URL with scheme '{}': only http and https are allowed",
                scheme
            ));
        }
    }

    // Reject URLs with embedded credentials
    if !parsed_url.username().is_empty() || parsed_url.password().is_some() {
        return Err(anyhow::anyhow!(
            "Refusing to open URL with embedded credentials"
        ));
    }

    // Validate there are no shell metacharacters (defense in depth)
    const DANGEROUS_CHARS: &[char] = &[
        '`', '$', '|', ';', '&', '<', '>', '(', ')', '{', '}', '[', ']', '!', '\n', '\r',
    ];
    if url.chars().any(|c| DANGEROUS_CHARS.contains(&c)) {
        return Err(anyhow::anyhow!(
            "URL contains potentially dangerous characters"
        ));
    }

    let safe_url = parsed_url.as_str();

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("--")
            .arg(safe_url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to open browser: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(safe_url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to open browser: {}", e))?;
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", safe_url])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to open browser: {}", e))?;
    }

    Ok(())
}

impl EventLoop {
    /// Loads MCP server configurations from storage.
    pub fn load_mcp_servers(&mut self) {
        match crate::mcp_storage::McpStorage::new() {
            Ok(storage) => match storage.list_servers() {
                Ok(stored_servers) => {
                    for stored in stored_servers {
                        // Convert StoredMcpServer to McpServerInfo for display
                        // Note: All servers start as Stopped regardless of enabled flag
                        // They need to be explicitly started to become Running
                        let server_info = crate::modal::mcp_manager::McpServerInfo {
                            name: stored.name.clone(),
                            status: crate::modal::mcp_manager::McpStatus::Stopped,
                            tool_count: 0,
                            error: None,
                            requires_auth: stored.api_key_env_var.is_some(),
                        };

                        // Only add if not already present
                        if !self
                            .app_state
                            .mcp_servers
                            .iter()
                            .any(|s| s.name == stored.name)
                        {
                            self.app_state.mcp_servers.push(server_info);
                        }
                    }
                    if !self.app_state.mcp_servers.is_empty() {
                        tracing::info!(
                            "Loaded {} MCP server(s) from storage",
                            self.app_state.mcp_servers.len()
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to load MCP servers from storage: {}", e);
                }
            },
            Err(e) => {
                tracing::warn!("Failed to initialize MCP storage: {}", e);
            }
        }
    }
}
