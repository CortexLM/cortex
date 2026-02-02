//! Application event system for Cortex TUI.
//!
//! This module provides the complete event handling infrastructure for the TUI,
//! including all application-level events, event bus for distribution, and
//! the dispatcher that bridges engine events to application events.
//!
//! ## Architecture
//!
//! ```text
//! FrameEngine -> EngineEvent -> EventDispatcher -> AppEvent -> AppEventBus -> Handlers
//! ```
//!
//! The system separates concerns:
//! - `EngineEvent`: Low-level events from the frame engine (ticks, keys, mouse)
//! - `AppEvent`: Application-level events (session, message, tool, UI events)
//! - `AppEventBus`: Async channel-based event distribution
//! - `EventDispatcher`: Converts engine events to app events with action mapping

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{KeyEvent, MouseEvent};
use tokio::sync::mpsc;
use uuid::Uuid;

use cortex_core::{EngineEvent, widgets::Message};
use cortex_engine::tools::handlers::subagent::ProgressEvent;

use crate::actions::{ActionContext, ActionMapper, KeyAction};
use crate::app::{AppView, FocusTarget};

// ============================================================================
// TOOL & SUBAGENT EVENTS (for internal processing)
// ============================================================================

/// Events from tool executions.
#[derive(Debug, Clone)]
pub enum ToolEvent {
    /// Tool execution started.
    Started {
        id: String,
        name: String,
        started_at: std::time::Instant,
    },
    /// Tool produced output.
    Output { id: String, chunk: String },
    /// Tool completed successfully.
    Completed {
        id: String,
        name: String,
        output: String,
        success: bool,
        duration: std::time::Duration,
    },
    /// Tool execution failed.
    Failed {
        id: String,
        name: String,
        error: String,
        duration: std::time::Duration,
    },
    /// Subagent todo list was updated (from TodoWrite tool).
    /// Used to update the UI's SubagentTaskDisplay with real-time todo progress.
    TodoUpdated {
        /// Session ID of the subagent (e.g., "subagent_<tool_call_id>")
        session_id: String,
        /// Todo items: (content, status) where status is "pending", "in_progress", or "completed"
        todos: Vec<(String, String)>,
    },
    /// Agent generation completed successfully.
    AgentGenerated {
        /// Agent name/identifier
        name: String,
        /// Path where agent was saved
        path: String,
        /// Location type (project or global)
        location: String,
    },
    /// Agent generation failed.
    AgentGenerationFailed {
        /// Error message
        error: String,
    },
}

/// Events from subagent executions.
#[derive(Debug, Clone)]
pub enum SubagentEvent {
    /// Progress update from subagent.
    Progress(ProgressEvent),
    /// Subagent completed successfully.
    Completed {
        session_id: String,
        output: String,
        tool_call_id: String,
    },
    /// Subagent execution failed.
    Failed {
        session_id: String,
        error: String,
        tool_call_id: String,
    },
}

/// Target component for scroll events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollTarget {
    /// The main chat/message area.
    #[default]
    Chat,
    /// The sidebar (session list).
    Sidebar,
    /// The diff view in approvals.
    DiffView,
    /// The help browser.
    Help,
}

/// Application events that drive the TUI behavior.
///
/// These events are processed by the main application loop to update state,
/// trigger side effects, and handle user interactions.
#[derive(Debug, Clone)]
pub enum AppEvent {
    // === Engine events ===
    /// Frame tick for animations and periodic updates.
    Tick(u64),

    /// Key press event from the terminal.
    Key(KeyEvent),

    /// Mouse event from the terminal.
    Mouse(MouseEvent),

    /// Terminal resize event with new dimensions (width, height).
    Resize(u16, u16),

    // === Action events ===
    /// A mapped action from user input.
    /// These are higher-level semantic actions derived from key/mouse events.
    Action(KeyAction),

    // === Session events ===
    /// A new session was created.
    SessionCreated(Uuid),

    /// An existing session was loaded.
    SessionLoaded(Uuid),

    /// A session was deleted.
    SessionDeleted(Uuid),

    /// A session was renamed to the given title.
    SessionRenamed(Uuid, String),

    /// A session was exported to the given path.
    SessionExported(PathBuf),

    // === Message events ===
    /// User sent a message.
    MessageSent(String),

    /// A complete message was received from the assistant.
    MessageReceived(Message),

    /// Streaming response has started.
    StreamingStarted,

    /// A chunk of streaming content was received.
    StreamingChunk(String),

    /// Streaming response completed successfully.
    StreamingCompleted,

    /// Streaming response encountered an error.
    StreamingError(String),

    // === Tool events ===
    /// A tool execution has started.
    ToolStarted {
        /// Name of the tool being executed
        name: String,
        /// Arguments passed to the tool
        args: serde_json::Value,
    },

    /// Tool execution progress update.
    ToolProgress {
        /// Name of the tool
        name: String,
        /// Current status message
        status: String,
    },

    /// Tool execution completed successfully.
    ToolCompleted {
        /// Name of the tool
        name: String,
        /// Result of the tool execution
        result: String,
    },

    /// Tool execution failed with an error.
    ToolError {
        /// Name of the tool
        name: String,
        /// Error message
        error: String,
    },

    /// Tool requires user approval before execution.
    ToolApprovalRequired {
        /// Name of the tool requiring approval
        name: String,
        /// Arguments that will be passed to the tool
        args: serde_json::Value,
        /// Optional diff preview for file operations
        diff: Option<String>,
    },

    /// User approved a tool execution.
    ToolApproved(String),

    /// User rejected a tool execution.
    ToolRejected(String),

    // === Context events ===
    /// A file was added to the conversation context.
    FileAdded(PathBuf),

    /// A folder was added to the conversation context.
    FolderAdded(PathBuf),

    /// All context was cleared from the conversation.
    ContextCleared,

    // === Model events ===
    /// The active model was changed.
    ModelChanged(String),

    // ProviderChanged removed: provider is now always "cortex"

    // === UI events ===
    /// The active view was changed.
    ViewChanged(AppView),

    /// The focus target was changed.
    FocusChanged(FocusTarget),

    /// The sidebar visibility was toggled.
    SidebarToggled(bool),

    /// The active theme was changed.
    ThemeChanged(String),

    /// Scroll position changed for a target component.
    ScrollChanged {
        /// The component that was scrolled
        target: ScrollTarget,
        /// New scroll position (0-based line offset)
        position: usize,
    },

    // === System events ===
    /// An error occurred that should be displayed to the user.
    Error(String),

    /// A warning message for the user.
    Warning(String),

    /// An informational message for the user.
    Info(String),

    /// Request to quit the application.
    Quit,
}

impl AppEvent {
    /// Returns true if this is a tick event.
    #[inline]
    pub fn is_tick(&self) -> bool {
        matches!(self, Self::Tick(_))
    }

    /// Returns true if this is a quit event.
    #[inline]
    pub fn is_quit(&self) -> bool {
        matches!(self, Self::Quit)
    }

    /// Returns true if this is an error, warning, or info event.
    #[inline]
    pub fn is_notification(&self) -> bool {
        matches!(self, Self::Error(_) | Self::Warning(_) | Self::Info(_))
    }

    /// Returns true if this is a streaming-related event.
    #[inline]
    pub fn is_streaming(&self) -> bool {
        matches!(
            self,
            Self::StreamingStarted
                | Self::StreamingChunk(_)
                | Self::StreamingCompleted
                | Self::StreamingError(_)
        )
    }

    /// Returns true if this is a tool-related event.
    #[inline]
    pub fn is_tool(&self) -> bool {
        matches!(
            self,
            Self::ToolStarted { .. }
                | Self::ToolProgress { .. }
                | Self::ToolCompleted { .. }
                | Self::ToolError { .. }
                | Self::ToolApprovalRequired { .. }
                | Self::ToolApproved(_)
                | Self::ToolRejected(_)
        )
    }

    /// Returns true if this is a session-related event.
    #[inline]
    pub fn is_session(&self) -> bool {
        matches!(
            self,
            Self::SessionCreated(_)
                | Self::SessionLoaded(_)
                | Self::SessionDeleted(_)
                | Self::SessionRenamed(_, _)
                | Self::SessionExported(_)
        )
    }
}

// ============================================================================
// ENGINE EVENT CONVERSION
// ============================================================================

impl AppEvent {
    /// Converts an EngineEvent to an AppEvent.
    ///
    /// Returns `None` for engine events that don't have a direct app-level
    /// equivalent (like `EngineEvent::Error` which is handled separately).
    pub fn from_engine_event(event: EngineEvent) -> Option<Self> {
        match event {
            EngineEvent::Tick(frame) => Some(AppEvent::Tick(frame)),
            EngineEvent::Key(key) => Some(AppEvent::Key(key)),
            EngineEvent::Mouse(mouse) => Some(AppEvent::Mouse(mouse)),
            EngineEvent::Resize(w, h) => Some(AppEvent::Resize(w, h)),
            EngineEvent::Quit => Some(AppEvent::Quit),
            EngineEvent::Error(_) => None, // Errors are logged, not converted
            EngineEvent::Paste(_) => None, // Paste events are handled directly in event_loop
            // Suspend/Resume are Unix-specific signals handled directly in event_loop
            EngineEvent::Suspend => None,
            EngineEvent::Resume => None,
        }
    }
}

// ============================================================================
// EVENT HANDLER TRAIT
// ============================================================================

/// Trait for components that can handle application events.
///
/// Implementing this trait allows a component to participate in event handling.
/// The return value indicates whether the event was consumed (true) or should
/// continue propagating to other handlers (false).
///
/// # Example
///
/// ```rust,ignore
/// impl EventHandler for ChatView {
///     fn handle_event(&mut self, event: &AppEvent) -> bool {
///         match event {
///             AppEvent::MessageReceived(msg) => {
///                 self.messages.push(msg.clone());
///                 true // consumed
///             }
///             AppEvent::Key(key) if self.has_focus => {
///                 self.handle_key(key);
///                 true
///             }
///             _ => false // not consumed, pass to next handler
///         }
///     }
/// }
/// ```
pub trait EventHandler {
    /// Handle an event, returning true if it was consumed.
    ///
    /// # Arguments
    ///
    /// * `event` - The event to handle
    ///
    /// # Returns
    ///
    /// * `true` - The event was consumed and should not propagate further
    /// * `false` - The event was not handled and should continue propagating
    fn handle_event(&mut self, event: &AppEvent) -> bool;
}

// ============================================================================
// APP EVENT SENDER
// ============================================================================

/// A clonable sender for dispatching application events.
///
/// This is a lightweight wrapper around `mpsc::Sender<AppEvent>` that provides
/// both async and blocking send methods for flexibility in different contexts.
#[derive(Clone)]
pub struct AppEventSender {
    tx: mpsc::Sender<AppEvent>,
}

impl AppEventSender {
    /// Creates a new event sender from an mpsc sender.
    pub fn new(tx: mpsc::Sender<AppEvent>) -> Self {
        Self { tx }
    }

    /// Sends an event asynchronously.
    ///
    /// # Errors
    ///
    /// Returns an error if the receiver has been dropped.
    pub async fn send(&self, event: AppEvent) -> Result<()> {
        self.tx
            .send(event)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send event: {}", e))
    }

    /// Sends an event, blocking the current thread.
    ///
    /// This is useful for sending events from synchronous code or callbacks.
    ///
    /// # Errors
    ///
    /// Returns an error if the receiver has been dropped.
    pub fn send_blocking(&self, event: AppEvent) -> Result<()> {
        self.tx
            .blocking_send(event)
            .map_err(|e| anyhow::anyhow!("Failed to send event (blocking): {}", e))
    }

    /// Attempts to send an event without waiting.
    ///
    /// Returns `Ok(())` if the event was sent, or an error if the channel is full
    /// or the receiver has been dropped.
    pub fn try_send(&self, event: AppEvent) -> Result<()> {
        self.tx
            .try_send(event)
            .map_err(|e| anyhow::anyhow!("Failed to try_send event: {}", e))
    }

    /// Returns true if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// Returns the current capacity of the channel.
    pub fn capacity(&self) -> usize {
        self.tx.capacity()
    }
}

impl std::fmt::Debug for AppEventSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppEventSender")
            .field("closed", &self.tx.is_closed())
            .field("capacity", &self.tx.capacity())
            .finish()
    }
}

// ============================================================================
// APP EVENT BUS
// ============================================================================

/// Manages event distribution via async channels.
///
/// The `AppEventBus` provides a publish-subscribe mechanism for dispatching
/// events throughout the application. It owns both the sender and receiver,
/// providing methods to get sender clones for distribution.
///
/// # Example
///
/// ```rust,ignore
/// let mut bus = AppEventBus::new(256);
/// let sender = bus.sender();
///
/// // Send from another task
/// tokio::spawn(async move {
///     sender.send(AppEvent::Info("Hello".to_string())).await.unwrap();
/// });
///
/// // Receive in main loop
/// while let Some(event) = bus.recv().await {
///     match event {
///         AppEvent::Quit => break,
///         _ => handle_event(event),
///     }
/// }
/// ```
pub struct AppEventBus {
    tx: mpsc::Sender<AppEvent>,
    rx: mpsc::Receiver<AppEvent>,
}

impl AppEventBus {
    /// Creates a new event bus with the specified channel capacity.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of events that can be buffered
    pub fn new(capacity: usize) -> Self {
        let (tx, rx) = mpsc::channel(capacity);
        Self { tx, rx }
    }

    /// Returns a new sender that can be used to dispatch events.
    ///
    /// The sender can be cloned and shared across tasks.
    pub fn sender(&self) -> AppEventSender {
        AppEventSender::new(self.tx.clone())
    }

    /// Returns a reference to the underlying mpsc sender.
    ///
    /// Use `sender()` for most cases; this is provided for compatibility
    /// with APIs that need the raw sender type.
    pub fn raw_sender(&self) -> &mpsc::Sender<AppEvent> {
        &self.tx
    }

    /// Receives the next event asynchronously.
    ///
    /// Returns `None` if all senders have been dropped.
    pub async fn recv(&mut self) -> Option<AppEvent> {
        self.rx.recv().await
    }

    /// Attempts to receive an event without blocking.
    ///
    /// Returns `None` if no event is available or all senders are dropped.
    pub fn try_recv(&mut self) -> Option<AppEvent> {
        self.rx.try_recv().ok()
    }

    /// Closes the receiving half of the channel.
    ///
    /// This prevents any further messages from being sent on the channel
    /// while still allowing any buffered messages to be received.
    pub fn close(&mut self) {
        self.rx.close();
    }
}

impl std::fmt::Debug for AppEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppEventBus")
            .field("capacity", &"<channel>")
            .finish()
    }
}

impl Default for AppEventBus {
    fn default() -> Self {
        Self::new(256)
    }
}

// ============================================================================
// EVENT DISPATCHER
// ============================================================================

/// Dispatches engine events to application events with action mapping.
///
/// The `EventDispatcher` serves as the bridge between low-level engine events
/// (ticks, raw key presses) and high-level application events (actions, commands).
/// It uses an `ActionMapper` to translate key events into semantic actions based
/// on the current application context.
///
/// # Example
///
/// ```rust,ignore
/// let action_mapper = ActionMapper::new();
/// let event_sender = bus.sender();
/// let dispatcher = EventDispatcher::new(action_mapper, event_sender);
///
/// // In the main loop
/// while let Some(engine_event) = engine_rx.recv().await {
///     let context = ActionContext {
///         view: current_view,
///         focus: current_focus,
///         is_input_focused: input.has_focus(),
///         has_selection: sidebar.has_selection(),
///     };
///     dispatcher.dispatch(engine_event, context).await?;
/// }
/// ```
pub struct EventDispatcher {
    action_mapper: ActionMapper,
    event_sender: AppEventSender,
}

impl EventDispatcher {
    /// Creates a new event dispatcher.
    ///
    /// # Arguments
    ///
    /// * `action_mapper` - Mapper for converting key events to actions
    /// * `event_sender` - Sender for dispatching converted events
    pub fn new(action_mapper: ActionMapper, event_sender: AppEventSender) -> Self {
        Self {
            action_mapper,
            event_sender,
        }
    }

    /// Dispatches an engine event as application event(s).
    ///
    /// For key events, this performs action mapping based on the provided context.
    /// The mapped action is sent as an `AppEvent::Action`, while the raw key event
    /// is also forwarded for components that need it.
    ///
    /// # Arguments
    ///
    /// * `engine_event` - The engine event to dispatch
    /// * `context` - Current application context for action mapping
    ///
    /// # Errors
    ///
    /// Returns an error if sending events fails.
    pub async fn dispatch(&self, engine_event: EngineEvent, context: ActionContext) -> Result<()> {
        // Convert engine event to app event
        if let Some(app_event) = AppEvent::from_engine_event(engine_event.clone()) {
            // Send the base event
            self.event_sender.send(app_event.clone()).await?;

            // For key events, also dispatch the mapped action
            if let EngineEvent::Key(key_event) = engine_event {
                let action = self.action_mapper.get_action(key_event, context);
                if action != KeyAction::None {
                    self.event_sender.send(AppEvent::Action(action)).await?;
                }
            }
        }

        Ok(())
    }

    /// Dispatches an engine event synchronously (blocking).
    ///
    /// This is useful for dispatching events from synchronous code.
    ///
    /// # Arguments
    ///
    /// * `engine_event` - The engine event to dispatch
    /// * `context` - Current application context for action mapping
    ///
    /// # Errors
    ///
    /// Returns an error if sending events fails.
    pub fn dispatch_blocking(
        &self,
        engine_event: EngineEvent,
        context: ActionContext,
    ) -> Result<()> {
        if let Some(app_event) = AppEvent::from_engine_event(engine_event.clone()) {
            self.event_sender.send_blocking(app_event)?;

            if let EngineEvent::Key(key_event) = engine_event {
                let action = self.action_mapper.get_action(key_event, context);
                if action != KeyAction::None {
                    self.event_sender.send_blocking(AppEvent::Action(action))?;
                }
            }
        }

        Ok(())
    }

    /// Sends a custom application event directly.
    ///
    /// This bypasses engine event conversion and sends the event as-is.
    ///
    /// # Arguments
    ///
    /// * `event` - The application event to send
    ///
    /// # Errors
    ///
    /// Returns an error if sending fails.
    pub async fn send(&self, event: AppEvent) -> Result<()> {
        self.event_sender.send(event).await
    }

    /// Gets a reference to the action mapper.
    pub fn action_mapper(&self) -> &ActionMapper {
        &self.action_mapper
    }

    /// Gets a mutable reference to the action mapper.
    pub fn action_mapper_mut(&mut self) -> &mut ActionMapper {
        &mut self.action_mapper
    }

    /// Gets a clone of the event sender.
    pub fn sender(&self) -> AppEventSender {
        self.event_sender.clone()
    }
}

impl std::fmt::Debug for EventDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventDispatcher")
            .field("action_mapper", &self.action_mapper)
            .field("event_sender", &self.event_sender)
            .finish()
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn make_key_event(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn test_scroll_target_default() {
        assert_eq!(ScrollTarget::default(), ScrollTarget::Chat);
    }

    #[test]
    fn test_app_event_is_tick() {
        assert!(AppEvent::Tick(42).is_tick());
        assert!(!AppEvent::Quit.is_tick());
    }

    #[test]
    fn test_app_event_is_quit() {
        assert!(AppEvent::Quit.is_quit());
        assert!(!AppEvent::Tick(0).is_quit());
    }

    #[test]
    fn test_app_event_is_notification() {
        assert!(AppEvent::Error("test".to_string()).is_notification());
        assert!(AppEvent::Warning("test".to_string()).is_notification());
        assert!(AppEvent::Info("test".to_string()).is_notification());
        assert!(!AppEvent::Quit.is_notification());
    }

    #[test]
    fn test_app_event_is_streaming() {
        assert!(AppEvent::StreamingStarted.is_streaming());
        assert!(AppEvent::StreamingChunk("chunk".to_string()).is_streaming());
        assert!(AppEvent::StreamingCompleted.is_streaming());
        assert!(AppEvent::StreamingError("error".to_string()).is_streaming());
        assert!(!AppEvent::Quit.is_streaming());
    }

    #[test]
    fn test_app_event_is_tool() {
        assert!(
            AppEvent::ToolStarted {
                name: "test".to_string(),
                args: serde_json::Value::Null,
            }
            .is_tool()
        );
        assert!(AppEvent::ToolApproved("test".to_string()).is_tool());
        assert!(AppEvent::ToolRejected("test".to_string()).is_tool());
        assert!(!AppEvent::Quit.is_tool());
    }

    #[test]
    fn test_app_event_is_session() {
        assert!(AppEvent::SessionCreated(Uuid::new_v4()).is_session());
        assert!(AppEvent::SessionLoaded(Uuid::new_v4()).is_session());
        assert!(AppEvent::SessionDeleted(Uuid::new_v4()).is_session());
        assert!(!AppEvent::Quit.is_session());
    }

    #[test]
    fn test_engine_event_conversion() {
        // Tick converts
        let tick = AppEvent::from_engine_event(EngineEvent::Tick(42));
        assert!(matches!(tick, Some(AppEvent::Tick(42))));

        // Key converts
        let key_event = make_key_event(KeyCode::Enter);
        let key = AppEvent::from_engine_event(EngineEvent::Key(key_event));
        assert!(matches!(key, Some(AppEvent::Key(_))));

        // Resize converts
        let resize = AppEvent::from_engine_event(EngineEvent::Resize(80, 24));
        assert!(matches!(resize, Some(AppEvent::Resize(80, 24))));

        // Quit converts
        let quit = AppEvent::from_engine_event(EngineEvent::Quit);
        assert!(matches!(quit, Some(AppEvent::Quit)));

        // Error does not convert
        let error = AppEvent::from_engine_event(EngineEvent::Error("test".to_string()));
        assert!(error.is_none());
    }

    #[tokio::test]
    async fn test_event_bus_send_recv() {
        let mut bus = AppEventBus::new(10);
        let sender = bus.sender();

        sender.send(AppEvent::Quit).await.unwrap();

        let event = bus.recv().await;
        assert!(matches!(event, Some(AppEvent::Quit)));
    }

    #[tokio::test]
    async fn test_event_bus_try_recv() {
        let mut bus = AppEventBus::new(10);
        let sender = bus.sender();

        // Empty bus returns None
        assert!(bus.try_recv().is_none());

        sender.send(AppEvent::Tick(1)).await.unwrap();

        // Now it should return the event
        let event = bus.try_recv();
        assert!(matches!(event, Some(AppEvent::Tick(1))));

        // Empty again
        assert!(bus.try_recv().is_none());
    }

    #[test]
    fn test_event_sender_blocking() {
        let (tx, mut rx) = mpsc::channel(10);
        let sender = AppEventSender::new(tx);

        sender
            .send_blocking(AppEvent::Info("test".to_string()))
            .unwrap();

        let event = rx.try_recv().unwrap();
        assert!(matches!(event, AppEvent::Info(_)));
    }

    #[test]
    fn test_event_sender_try_send() {
        let (tx, _rx) = mpsc::channel(10);
        let sender = AppEventSender::new(tx);

        let result = sender.try_send(AppEvent::Quit);
        assert!(result.is_ok());
    }

    #[test]
    fn test_event_bus_default() {
        let bus = AppEventBus::default();
        assert!(!bus.tx.is_closed());
    }

    #[test]
    fn test_event_sender_debug() {
        let (tx, _rx) = mpsc::channel::<AppEvent>(10);
        let sender = AppEventSender::new(tx);
        let debug_str = format!("{:?}", sender);
        assert!(debug_str.contains("AppEventSender"));
    }

    #[test]
    fn test_event_bus_debug() {
        let bus = AppEventBus::new(10);
        let debug_str = format!("{:?}", bus);
        assert!(debug_str.contains("AppEventBus"));
    }
}
