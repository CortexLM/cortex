//! Session Bridge - Main bridge to cortex-core's Session API.
//!
//! This module provides `SessionBridge`, which wraps `cortex_engine::Session` and
//! `SessionHandle` to provide a clean async interface for cortex-tui.
//!
//! # Architecture
//!
//! The bridge pattern decouples the TUI from the core session management:
//!
//! ```text
//! ┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
//! │   cortex-tui    │────▶│  SessionBridge  │────▶│  cortex-core    │
//! │  (UI Layer)     │     │  (Adapter)      │     │  (Session)      │
//! └─────────────────┘     └─────────────────┘     └─────────────────┘
//!         │                       │                       │
//!         │   User Actions        │   Submissions         │
//!         │◀──────────────────────│◀──────────────────────│
//!         │   UI Events           │   Events              │
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_tui::bridge::SessionBridge;
//! use cortex_engine::Config;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = Config::default();
//!     let bridge = SessionBridge::new(config).await?;
//!     
//!     // Send a message
//!     bridge.send_message("Hello, world!".to_string()).await?;
//!     
//!     // Receive events
//!     while let Ok(event) = bridge.recv_event().await {
//!         println!("Received event: {:?}", event);
//!     }
//!     
//!     Ok(())
//! }
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use tokio::task::JoinHandle;

use cortex_engine::Config;
use cortex_engine::session::{Session, SessionHandle};
use cortex_protocol::{ConversationId, Event, Op, ReviewDecision, Submission, UserInput};

/// Bridge to cortex-core's Session API.
///
/// `SessionBridge` provides a clean async interface for the TUI to interact
/// with the underlying cortex-core session. It manages:
///
/// - Session lifecycle (creation, resumption, shutdown)
/// - Message sending and receiving
/// - Approval workflows
/// - Session operations (compact, undo, redo, etc.)
///
/// The bridge spawns the session's run loop as a background task and provides
/// methods to send submissions and receive events.
pub struct SessionBridge {
    /// Handle for communicating with the session.
    handle: SessionHandle,
    /// Background task running the session loop.
    session_task: Option<JoinHandle<cortex_engine::Result<()>>>,
    /// Session configuration (kept for reference).
    config: Config,
}

impl SessionBridge {
    // ========================================================================
    // Constructor Methods
    // ========================================================================

    /// Create a new session bridge with a fresh session.
    ///
    /// This method:
    /// 1. Creates a new `Session` and `SessionHandle` pair
    /// 2. Spawns the session's run loop as a background task
    /// 3. Returns the bridge ready for use
    ///
    /// # Arguments
    ///
    /// * `config` - The cortex-core configuration for the session
    ///
    /// # Errors
    ///
    /// Returns an error if session creation fails (e.g., invalid config,
    /// API key issues, etc.)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let config = Config::default();
    /// let bridge = SessionBridge::new(config).await?;
    /// ```
    pub async fn new(config: Config) -> Result<Self> {
        let (mut session, handle) =
            Session::new(config.clone()).context("Failed to create new session")?;

        let session_task = tokio::spawn(async move { session.run().await });

        Ok(Self {
            handle,
            session_task: Some(session_task),
            config,
        })
    }

    /// Resume an existing session from a conversation ID.
    ///
    /// This method:
    /// 1. Loads session state from the rollout file
    /// 2. Reconstructs the message history
    /// 3. Spawns the session's run loop as a background task
    ///
    /// # Arguments
    ///
    /// * `config` - The cortex-core configuration
    /// * `conversation_id` - The ID of the conversation to resume
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The conversation ID is not found
    /// - The rollout file is corrupted
    /// - Session creation fails
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let conversation_id = ConversationId::from_str("abc-123")?;
    /// let bridge = SessionBridge::resume(config, conversation_id).await?;
    /// ```
    pub async fn resume(config: Config, conversation_id: ConversationId) -> Result<Self> {
        let (mut session, handle) =
            Session::resume(config.clone(), conversation_id).context("Failed to resume session")?;

        let session_task = tokio::spawn(async move { session.run().await });

        Ok(Self {
            handle,
            session_task: Some(session_task),
            config,
        })
    }

    // ========================================================================
    // Message Sending Methods
    // ========================================================================

    /// Send a user text message to the session.
    ///
    /// This creates a `Submission` with `Op::UserInput` and sends it through
    /// the submission channel. The session will process the message and
    /// generate appropriate events.
    ///
    /// # Arguments
    ///
    /// * `text` - The user's message text
    ///
    /// # Errors
    ///
    /// Returns an error if the submission channel is closed (session terminated).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// bridge.send_message("Explain this code".to_string()).await?;
    /// ```
    pub async fn send_message(&self, text: String) -> Result<()> {
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::UserInput {
                items: vec![UserInput::Text { text }],
            },
        };
        self.handle
            .submission_tx
            .send(submission)
            .await
            .context("Failed to send message: submission channel closed")?;
        Ok(())
    }

    /// Send approval decision for a tool execution request.
    ///
    /// When the session requests approval for a potentially dangerous operation
    /// (like shell commands), this method sends the user's decision back.
    ///
    /// # Arguments
    ///
    /// * `call_id` - The ID of the tool call requiring approval
    /// * `decision` - The user's approval decision
    ///
    /// # Errors
    ///
    /// Returns an error if the submission channel is closed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Approve a command
    /// bridge.send_approval(call_id, ReviewDecision::Approved).await?;
    ///
    /// // Deny and ask agent to try something else
    /// bridge.send_approval(call_id, ReviewDecision::Denied).await?;
    ///
    /// // Deny and stop the current task
    /// bridge.send_approval(call_id, ReviewDecision::Abort).await?;
    /// ```
    pub async fn send_approval(&self, call_id: String, decision: ReviewDecision) -> Result<()> {
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::ExecApproval {
                id: call_id,
                decision,
            },
        };
        self.handle
            .submission_tx
            .send(submission)
            .await
            .context("Failed to send approval: submission channel closed")?;
        Ok(())
    }

    /// Interrupt the current operation.
    ///
    /// This sets the cancellation flag and sends an interrupt submission.
    /// The session will abort any ongoing operation and emit a `TurnAborted` event.
    ///
    /// # Errors
    ///
    /// Returns an error if the submission channel is closed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // User pressed Ctrl+C
    /// bridge.interrupt().await?;
    /// ```
    pub async fn interrupt(&self) -> Result<()> {
        self.handle.cancelled.store(true, Ordering::SeqCst);
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::Interrupt,
        };
        self.handle
            .submission_tx
            .send(submission)
            .await
            .context("Failed to send interrupt: submission channel closed")?;
        Ok(())
    }

    /// Request graceful shutdown of the session.
    ///
    /// This sends a shutdown submission and the session will clean up
    /// resources and emit a `ShutdownComplete` event.
    ///
    /// # Errors
    ///
    /// Returns an error if the submission channel is closed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// bridge.shutdown().await?;
    /// bridge.wait().await?; // Wait for session to fully terminate
    /// ```
    pub async fn shutdown(&self) -> Result<()> {
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::Shutdown,
        };
        self.handle
            .submission_tx
            .send(submission)
            .await
            .context("Failed to send shutdown: submission channel closed")?;
        Ok(())
    }

    /// Compact the conversation context.
    ///
    /// When the context window is getting full, this triggers summarization
    /// of older messages to reduce token usage while preserving important context.
    ///
    /// # Errors
    ///
    /// Returns an error if the submission channel is closed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Context is getting large, compact it
    /// bridge.compact().await?;
    /// ```
    pub async fn compact(&self) -> Result<()> {
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::Compact,
        };
        self.handle
            .submission_tx
            .send(submission)
            .await
            .context("Failed to send compact: submission channel closed")?;
        Ok(())
    }

    /// Undo the last action/turn.
    ///
    /// This reverts file changes made in the last turn and removes the
    /// corresponding messages from the conversation history.
    ///
    /// # Errors
    ///
    /// Returns an error if the submission channel is closed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // User wants to undo the last agent action
    /// bridge.undo().await?;
    /// ```
    pub async fn undo(&self) -> Result<()> {
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::Undo,
        };
        self.handle
            .submission_tx
            .send(submission)
            .await
            .context("Failed to send undo: submission channel closed")?;
        Ok(())
    }

    /// Redo the last undone action.
    ///
    /// This re-applies file changes and messages that were previously undone.
    ///
    /// # Errors
    ///
    /// Returns an error if the submission channel is closed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // User changed their mind, redo the undone action
    /// bridge.redo().await?;
    /// ```
    pub async fn redo(&self) -> Result<()> {
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::Redo,
        };
        self.handle
            .submission_tx
            .send(submission)
            .await
            .context("Failed to send redo: submission channel closed")?;
        Ok(())
    }

    /// Switch the active model.
    ///
    /// This changes the model used for subsequent turns. The change takes
    /// effect immediately for the next user message.
    ///
    /// # Arguments
    ///
    /// * `model` - The model identifier (e.g., "claude-sonnet-4-20250514", "gpt-4o")
    ///
    /// # Errors
    ///
    /// Returns an error if the submission channel is closed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Switch to a different model
    /// bridge.switch_model("gpt-4o".to_string()).await?;
    /// ```
    pub async fn switch_model(&self, model: String) -> Result<()> {
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::OverrideTurnContext {
                cwd: None,
                approval_policy: None,
                sandbox_policy: None,
                model: Some(model),
                effort: None,
                summary: None,
            },
        };
        self.handle
            .submission_tx
            .send(submission)
            .await
            .context("Failed to send model switch: submission channel closed")?;
        Ok(())
    }

    /// Reload MCP (Model Context Protocol) servers.
    ///
    /// This reinitializes all configured MCP servers, picking up any
    /// configuration changes.
    ///
    /// # Errors
    ///
    /// Returns an error if the submission channel is closed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Configuration changed, reload MCP servers
    /// bridge.reload_mcp_servers().await?;
    /// ```
    pub async fn reload_mcp_servers(&self) -> Result<()> {
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::ReloadMcpServers,
        };
        self.handle
            .submission_tx
            .send(submission)
            .await
            .context("Failed to send MCP reload: submission channel closed")?;
        Ok(())
    }

    // ========================================================================
    // Event Receiving Methods
    // ========================================================================

    /// Get the event receiver for polling cortex-core events.
    ///
    /// This returns a reference to the underlying `async_channel::Receiver`,
    /// which can be used with `select!` or other async patterns.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let rx = bridge.event_receiver();
    /// tokio::select! {
    ///     event = rx.recv() => { /* handle event */ }
    ///     _ = some_other_future => { /* handle other */ }
    /// }
    /// ```
    pub fn event_receiver(&self) -> &async_channel::Receiver<Event> {
        &self.handle.event_rx
    }

    /// Try to receive an event without blocking.
    ///
    /// Returns `Some(event)` if an event is immediately available,
    /// or `None` if the channel is empty.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Non-blocking poll for events
    /// while let Some(event) = bridge.try_recv_event() {
    ///     process_event(event);
    /// }
    /// ```
    pub fn try_recv_event(&self) -> Option<Event> {
        self.handle.event_rx.try_recv().ok()
    }

    /// Receive an event, blocking until one is available.
    ///
    /// This method will wait indefinitely for an event. Use with timeout
    /// or `select!` if you need non-blocking behavior.
    ///
    /// # Errors
    ///
    /// Returns an error if the event channel is closed (session terminated).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// loop {
    ///     match bridge.recv_event().await {
    ///         Ok(event) => process_event(event),
    ///         Err(_) => break, // Session ended
    ///     }
    /// }
    /// ```
    pub async fn recv_event(&self) -> Result<Event> {
        self.handle
            .event_rx
            .recv()
            .await
            .map_err(|e| anyhow::anyhow!("Event channel closed: {}", e))
    }

    // ========================================================================
    // Accessor Methods
    // ========================================================================

    /// Get the conversation ID for this session.
    ///
    /// The conversation ID uniquely identifies this session and is used
    /// for persistence and resumption.
    pub fn conversation_id(&self) -> &ConversationId {
        &self.handle.conversation_id
    }

    /// Get the session configuration.
    ///
    /// Returns a reference to the config used to create this session.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Check if the session has been cancelled.
    ///
    /// Returns `true` if `interrupt()` was called or the session was
    /// otherwise cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.handle.cancelled.load(Ordering::SeqCst)
    }

    /// Get access to the cancellation flag.
    ///
    /// This can be used to signal cancellation from multiple places
    /// or to check cancellation status without going through the bridge.
    pub fn cancelled_flag(&self) -> &Arc<AtomicBool> {
        &self.handle.cancelled
    }

    /// Get the underlying session handle.
    ///
    /// This provides direct access to the `SessionHandle` for advanced
    /// use cases that need lower-level control.
    pub fn handle(&self) -> &SessionHandle {
        &self.handle
    }

    // ========================================================================
    // Lifecycle Methods
    // ========================================================================

    /// Wait for the session task to complete.
    ///
    /// This should be called after `shutdown()` to ensure the session
    /// has fully terminated and cleaned up resources.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The session task panicked
    /// - The session returned an error
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// bridge.shutdown().await?;
    /// bridge.wait().await?; // Ensure clean termination
    /// ```
    pub async fn wait(&mut self) -> Result<()> {
        if let Some(task) = self.session_task.take() {
            task.await
                .context("Session task panicked")?
                .context("Session returned error")?;
        }
        Ok(())
    }

    /// Check if the session task is still running.
    ///
    /// Returns `true` if the background session task has not yet completed.
    pub fn is_running(&self) -> bool {
        self.session_task
            .as_ref()
            .map(|t| !t.is_finished())
            .unwrap_or(false)
    }

    /// Reset the cancellation flag.
    ///
    /// This should be called before starting a new operation if the
    /// session was previously interrupted.
    pub fn reset_cancelled(&self) {
        self.handle.cancelled.store(false, Ordering::SeqCst);
    }
}

impl Drop for SessionBridge {
    fn drop(&mut self) {
        // Signal shutdown via the cancellation flag
        self.handle.cancelled.store(true, Ordering::SeqCst);

        // Note: We don't await the session task here since Drop is synchronous.
        // The session will detect the cancellation flag and exit gracefully.
        // For clean shutdown, call shutdown() and wait() before dropping.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_submission_creation() {
        // Test that we can create valid submissions
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::UserInput {
                items: vec![UserInput::Text {
                    text: "test".to_string(),
                }],
            },
        };
        assert!(!submission.id.is_empty());
    }

    #[test]
    fn test_review_decision_variants() {
        // Ensure all review decision variants are available
        let _ = ReviewDecision::Approved;
        let _ = ReviewDecision::ApprovedForSession;
        let _ = ReviewDecision::Denied;
        let _ = ReviewDecision::Abort;
    }
}
