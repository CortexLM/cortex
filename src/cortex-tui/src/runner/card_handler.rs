//! Card Handler - Integration module for the card system with the event loop.
//!
//! This module provides a unified interface for managing cards (modal dialogs)
//! and approval overlays within the main event loop. It handles the card stack,
//! processes user input, and collects actions to be executed by the event loop.
//!
//! ## Architecture
//!
//! ```text
//!                        +-----------------+
//!                        |  CardHandler    |
//!                        +--------+--------+
//!                                 |
//!         +-----------------------+-----------------------+
//!         |                       |                       |
//! +-------v-------+     +---------v---------+     +-------v-------+
//! |   CardStack   |     | ApprovalOverlay   |     | PendingActions|
//! | (Modal Cards) |     | (Priority Modal)  |     | (For EventLoop|
//! +---------------+     +-------------------+     +---------------+
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use cortex_tui::runner::CardHandler;
//! use cortex_tui::cards::SessionInfo;
//!
//! let mut handler = CardHandler::new();
//!
//! // Open a card
//! handler.open_sessions(sessions);
//!
//! // In the event loop
//! if handler.is_active() {
//!     if handler.handle_key(key_event) {
//!         // Key was consumed by the handler
//!     }
//!
//!     // Process any pending actions
//!     for action in handler.take_actions() {
//!         // Handle action...
//!     }
//! }
//!
//! // Render in the UI
//! handler.render(area, buf);
//! ```

use crate::cards::{
    CardAction, CardResult, CardStack, CommandsCard, HelpCard, McpCard, McpServerInfo, ModelInfo,
    ModelsCard, SessionInfo, SessionsCard,
};
use crate::widgets::{ApprovalDecision, ApprovalOverlay, ApprovalRequest};
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

// ============================================================
// CARD HANDLER
// ============================================================

/// Handler for managing cards and approval overlays within the event loop.
///
/// The CardHandler provides a unified interface for:
/// - Managing a stack of modal cards
/// - Handling approval overlays (which take priority over cards)
/// - Collecting and returning actions for the event loop to process
/// - Rendering the current card or overlay
pub struct CardHandler {
    /// Stack of active cards.
    card_stack: CardStack,

    /// Approval overlay (separate from cards because it has special rendering).
    /// Approval overlays take priority over regular cards.
    approval_overlay: Option<ApprovalOverlay>,

    /// Pending actions to be processed by the event loop.
    pending_actions: Vec<CardAction>,

    /// Pending approval decision to be processed by the event loop.
    /// Contains (request_id, decision) pairs.
    pending_approval: Option<(String, ApprovalDecision)>,
}

impl CardHandler {
    /// Creates a new CardHandler.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let handler = CardHandler::new();
    /// assert!(!handler.is_active());
    /// ```
    pub fn new() -> Self {
        Self {
            card_stack: CardStack::new(),
            approval_overlay: None,
            pending_actions: Vec::new(),
            pending_approval: None,
        }
    }

    // ========================================================================
    // CARD OPENING METHODS
    // ========================================================================

    /// Opens the sessions card with the given session list.
    ///
    /// # Arguments
    ///
    /// * `sessions` - List of sessions to display
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let sessions = vec![SessionInfo::new(...)];
    /// handler.open_sessions(sessions);
    /// ```
    pub fn open_sessions(&mut self, sessions: Vec<SessionInfo>) {
        let card = SessionsCard::new(sessions);
        self.card_stack.push(Box::new(card));
    }

    /// Opens the models card with the given model list.
    ///
    /// # Arguments
    ///
    /// * `models` - List of available models
    /// * `current` - Currently selected model ID (if any)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let models = vec![ModelInfo::new("claude-opus-4", "Claude Opus 4", "Anthropic")];
    /// handler.open_models(models, Some("claude-opus-4".to_string()));
    /// ```
    pub fn open_models(&mut self, models: Vec<ModelInfo>, current: Option<String>) {
        let card = ModelsCard::new(models, current);
        self.card_stack.push(Box::new(card));
    }

    /// Opens the providers card - REMOVED (single Cortex provider).
    /// This method is now a no-op.
    pub fn open_providers(&mut self, _current: Option<String>) {
        // Provider picker removed - Cortex is the only provider
    }

    /// Opens the MCP servers card with the given server list.
    ///
    /// # Arguments
    ///
    /// * `servers` - List of MCP servers to display
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let servers = vec![McpServerInfo::new("filesystem").with_status(McpStatus::Running)];
    /// handler.open_mcp(servers);
    /// ```
    pub fn open_mcp(&mut self, servers: Vec<McpServerInfo>) {
        let card = McpCard::new(servers);
        self.card_stack.push(Box::new(card));
    }

    /// Opens the MCP servers card directly in Add Server mode.
    ///
    /// # Arguments
    ///
    /// * `servers` - List of MCP servers (for reference)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let servers = vec![McpServerInfo::new("filesystem").with_status(McpStatus::Running)];
    /// handler.open_mcp_add_mode(servers);
    /// ```
    pub fn open_mcp_add_mode(&mut self, servers: Vec<McpServerInfo>) {
        let card = McpCard::new_add_mode(servers);
        self.card_stack.push(Box::new(card));
    }

    /// Opens the help card.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// handler.open_help();
    /// ```
    pub fn open_help(&mut self) {
        let card = HelpCard::new();
        self.card_stack.push(Box::new(card));
    }

    /// Opens the commands card (command palette).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// handler.open_commands();
    /// ```
    pub fn open_commands(&mut self) {
        let card = CommandsCard::new();
        self.card_stack.push(Box::new(card));
    }

    // ========================================================================
    // APPROVAL HANDLING
    // ========================================================================

    /// Requests approval for an action.
    ///
    /// Creates or enqueues an approval overlay. If an overlay is already active,
    /// the request is added to the queue.
    ///
    /// # Arguments
    ///
    /// * `request` - The approval request to display
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let request = ApprovalRequest::Exec {
    ///     id: "cmd-1".to_string(),
    ///     command: vec!["git".into(), "add".into(), ".".into()],
    ///     reason: Some("Stage changes".to_string()),
    /// };
    /// handler.request_approval(request);
    /// ```
    pub fn request_approval(&mut self, request: ApprovalRequest) {
        if let Some(ref mut overlay) = self.approval_overlay {
            overlay.enqueue(request);
        } else {
            self.approval_overlay = Some(ApprovalOverlay::new(request));
        }
    }

    /// Checks if there is a pending approval request.
    ///
    /// # Returns
    ///
    /// `true` if an approval overlay is currently active.
    pub fn has_pending_approval(&self) -> bool {
        self.approval_overlay.is_some()
    }

    /// Gets a reference to the current approval request (if any).
    ///
    /// # Returns
    ///
    /// A reference to the current `ApprovalRequest`, or `None` if no approval is pending.
    pub fn current_approval(&self) -> Option<&ApprovalRequest> {
        self.approval_overlay
            .as_ref()
            .and_then(|o| o.current_request())
    }

    // ========================================================================
    // STATE QUERIES
    // ========================================================================

    /// Checks if any card or approval overlay is active.
    ///
    /// # Returns
    ///
    /// `true` if either the approval overlay is active or the card stack is not empty.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// if handler.is_active() {
    ///     // Render the handler
    ///     handler.render(area, buf);
    /// }
    /// ```
    pub fn is_active(&self) -> bool {
        self.approval_overlay.is_some() || self.card_stack.is_active()
    }

    /// Checks if a card (not approval overlay) is currently active.
    ///
    /// # Returns
    ///
    /// `true` if the card stack is not empty.
    pub fn has_active_card(&self) -> bool {
        self.card_stack.is_active()
    }

    /// Gets the title of the current card (if any).
    ///
    /// # Returns
    ///
    /// The title of the current card, or `None` if no card is active.
    pub fn current_card_title(&self) -> Option<&str> {
        self.card_stack.current().map(|c| c.title())
    }

    // ========================================================================
    // INPUT HANDLING
    // ========================================================================

    /// Handles a key event.
    ///
    /// The approval overlay takes priority over cards. If a key event is consumed
    /// by either the overlay or the card stack, this method returns `true`.
    ///
    /// # Arguments
    ///
    /// * `key` - The key event to handle
    ///
    /// # Returns
    ///
    /// `true` if the event was consumed by the handler, `false` otherwise.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// if handler.handle_key(key_event) {
    ///     // Event was consumed, don't propagate
    /// } else {
    ///     // Event was not consumed, handle normally
    /// }
    /// ```
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Approval overlay takes priority
        if let Some(ref mut overlay) = self.approval_overlay {
            if let Some((id, decision)) = overlay.handle_key(key) {
                // Store the decision for the event loop
                self.pending_approval = Some((id, decision));

                // Check if the overlay is complete (no more requests in queue)
                if overlay.current_request().is_none() {
                    self.approval_overlay = None;
                }
            }
            return true; // Always consume key when approval is active
        }

        // Then handle card stack
        if self.card_stack.is_active() {
            let result = self.card_stack.handle_key(key);

            match result {
                CardResult::Action(action) => {
                    self.pending_actions.push(action);
                }
                CardResult::Continue | CardResult::Close | CardResult::Replace(_) => {
                    // Already handled by CardStack
                }
            }

            return true; // Always consume key when card is active
        }

        false
    }

    // ========================================================================
    // ACTION RETRIEVAL
    // ========================================================================

    /// Takes pending actions to be processed by the event loop.
    ///
    /// This clears the internal action queue and returns all pending actions.
    ///
    /// # Returns
    ///
    /// A vector of `CardAction` values to be processed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// for action in handler.take_actions() {
    ///     match action {
    ///         CardAction::SelectModel(id) => { /* handle model selection */ }
    ///         CardAction::SelectSession(path) => { /* handle session selection */ }
    ///         // ...
    ///     }
    /// }
    /// ```
    pub fn take_actions(&mut self) -> Vec<CardAction> {
        std::mem::take(&mut self.pending_actions)
    }

    /// Takes the pending approval decision (if any).
    ///
    /// # Returns
    ///
    /// A tuple of `(request_id, decision)` if a decision was made, `None` otherwise.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// if let Some((id, decision)) = handler.take_approval_decision() {
    ///     match decision {
    ///         ApprovalDecision::Approved => { /* execute the action */ }
    ///         ApprovalDecision::ApprovedForSession => { /* execute and remember */ }
    ///         ApprovalDecision::Rejected => { /* cancel */ }
    ///     }
    /// }
    /// ```
    pub fn take_approval_decision(&mut self) -> Option<(String, ApprovalDecision)> {
        self.pending_approval.take()
    }

    // ========================================================================
    // RENDERING
    // ========================================================================

    /// Renders the current card or approval overlay.
    ///
    /// The approval overlay takes priority over cards if both are active.
    ///
    /// # Arguments
    ///
    /// * `area` - The area to render into
    /// * `buf` - The buffer to render to
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// if handler.is_active() {
    ///     handler.render(modal_area, buf);
    /// }
    /// ```
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        // Approval overlay takes priority
        if let Some(ref overlay) = self.approval_overlay {
            overlay.render(area, buf);
            return;
        }

        // Otherwise render the card stack
        if self.card_stack.is_active() {
            self.card_stack.render(area, buf);
        }
    }

    // ========================================================================
    // KEY HINTS
    // ========================================================================

    /// Gets key hints for the current state.
    ///
    /// Returns hints appropriate for the current active element (approval overlay
    /// or top card).
    ///
    /// # Returns
    ///
    /// A vector of `(key, description)` tuples.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let hints = handler.key_hints();
    /// for (key, desc) in hints {
    ///     println!("{}: {}", key, desc);
    /// }
    /// ```
    pub fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        // Approval overlay takes priority
        if self.approval_overlay.is_some() {
            // Approval overlay hints
            return vec![
                ("\u{2191}\u{2193}", "select"),
                ("Enter", "confirm"),
                ("Esc", "reject"),
            ];
        }

        // Card hints
        if let Some(card) = self.card_stack.current() {
            return card.key_hints();
        }

        Vec::new()
    }

    // ========================================================================
    // CARD MANAGEMENT
    // ========================================================================

    /// Closes the current card.
    ///
    /// If there's an approval overlay, it is dismissed (treated as rejection).
    /// Otherwise, the top card is popped from the stack.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// handler.close();
    /// ```
    pub fn close(&mut self) {
        // Approval overlay takes priority
        if self.approval_overlay.is_some() {
            self.approval_overlay = None;
            return;
        }

        // Pop the top card
        self.card_stack.pop();
    }

    /// Closes all cards and dismisses any approval overlay.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// handler.close_all();
    /// assert!(!handler.is_active());
    /// ```
    pub fn close_all(&mut self) {
        self.approval_overlay = None;
        while self.card_stack.pop().is_some() {}
    }

    /// Checks if at least one card is in the stack.
    ///
    /// Does not include the approval overlay.
    ///
    /// # Returns
    ///
    /// `true` if the card stack has at least one card.
    ///
    /// Note: CardStack doesn't expose a `len()` method, so we can only
    /// check if cards are present, not count them exactly.
    pub fn has_cards(&self) -> bool {
        self.card_stack.is_active()
    }

    /// Gets the desired height for the current card or overlay.
    ///
    /// # Arguments
    ///
    /// * `max_height` - Maximum available height
    /// * `width` - Available width
    ///
    /// # Returns
    ///
    /// The desired height for rendering.
    pub fn desired_height(&self, max_height: u16, width: u16) -> u16 {
        // Approval overlay has fixed height
        if self.approval_overlay.is_some() {
            return 10.min(max_height); // Fixed height for approval
        }

        // Card desired height
        if let Some(card) = self.card_stack.current() {
            return card.desired_height(max_height, width);
        }

        0
    }
}

impl Default for CardHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::McpStatus;
    use chrono::Utc;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::path::PathBuf;

    fn create_test_sessions() -> Vec<SessionInfo> {
        vec![
            SessionInfo::new(
                PathBuf::from("/sessions/session1"),
                "First Session",
                "claude-opus-4",
                Utc::now(),
                5,
            ),
            SessionInfo::new(
                PathBuf::from("/sessions/session2"),
                "Second Session",
                "gpt-4",
                Utc::now(),
                10,
            ),
        ]
    }

    fn create_test_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo::new("claude-opus-4", "Claude Opus 4", "Anthropic")
                .with_context_length(200_000),
            ModelInfo::new("gpt-4o", "GPT-4o", "OpenAI").with_context_length(128_000),
        ]
    }

    fn create_test_mcp_servers() -> Vec<McpServerInfo> {
        vec![
            McpServerInfo::new("filesystem")
                .with_status(McpStatus::Running)
                .with_tool_count(3),
            McpServerInfo::new("github").with_status(McpStatus::Stopped),
        ]
    }

    #[test]
    fn test_new_handler_is_inactive() {
        let handler = CardHandler::new();
        assert!(!handler.is_active());
        assert!(!handler.has_pending_approval());
        assert!(!handler.has_active_card());
    }

    #[test]
    fn test_default_is_same_as_new() {
        let handler1 = CardHandler::new();
        let handler2 = CardHandler::default();
        assert_eq!(handler1.is_active(), handler2.is_active());
    }

    #[test]
    fn test_open_sessions() {
        let mut handler = CardHandler::new();
        handler.open_sessions(create_test_sessions());

        assert!(handler.is_active());
        assert!(handler.has_active_card());
        assert_eq!(handler.current_card_title(), Some("Sessions"));
    }

    #[test]
    fn test_open_models() {
        let mut handler = CardHandler::new();
        handler.open_models(create_test_models(), Some("claude-opus-4".to_string()));

        assert!(handler.is_active());
        assert_eq!(handler.current_card_title(), Some("Models"));
    }

    #[test]
    fn test_open_providers() {
        // Provider picker removed - Cortex is the only provider
        // This test now verifies the no-op behavior
        let mut handler = CardHandler::new();
        handler.open_providers(Some("cortex".to_string()));

        // Should NOT be active since providers card was removed
        assert!(!handler.is_active());
    }

    #[test]
    fn test_open_mcp() {
        let mut handler = CardHandler::new();
        handler.open_mcp(create_test_mcp_servers());

        assert!(handler.is_active());
        assert_eq!(handler.current_card_title(), Some("MCP Servers"));
    }

    #[test]
    fn test_open_help() {
        let mut handler = CardHandler::new();
        handler.open_help();

        assert!(handler.is_active());
        assert_eq!(handler.current_card_title(), Some("Help"));
    }

    #[test]
    fn test_open_commands() {
        let mut handler = CardHandler::new();
        handler.open_commands();

        assert!(handler.is_active());
        assert_eq!(handler.current_card_title(), Some("Commands"));
    }

    #[test]
    fn test_request_approval() {
        let mut handler = CardHandler::new();

        let request = ApprovalRequest::Exec {
            id: "cmd-1".to_string(),
            command: vec!["git".into(), "status".into()],
            reason: Some("Check status".to_string()),
        };

        handler.request_approval(request);

        assert!(handler.is_active());
        assert!(handler.has_pending_approval());
        assert!(handler.current_approval().is_some());
    }

    #[test]
    fn test_approval_takes_priority_over_cards() {
        let mut handler = CardHandler::new();

        // Open a card first
        handler.open_help();
        assert_eq!(handler.current_card_title(), Some("Help"));

        // Then request approval
        let request = ApprovalRequest::Exec {
            id: "cmd-1".to_string(),
            command: vec!["git".into(), "add".into(), ".".into()],
            reason: None,
        };
        handler.request_approval(request);

        // Approval should take priority
        assert!(handler.has_pending_approval());

        // Key hints should be for approval
        let hints = handler.key_hints();
        assert!(hints.iter().any(|(_, desc)| *desc == "reject"));
    }

    #[test]
    fn test_handle_key_consumes_when_active() {
        let mut handler = CardHandler::new();
        handler.open_help();

        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let consumed = handler.handle_key(key);

        assert!(consumed);
    }

    #[test]
    fn test_handle_key_does_not_consume_when_inactive() {
        let mut handler = CardHandler::new();

        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let consumed = handler.handle_key(key);

        assert!(!consumed);
    }

    #[test]
    fn test_close_card() {
        let mut handler = CardHandler::new();
        handler.open_help();
        assert!(handler.is_active());

        handler.close();
        assert!(!handler.is_active());
    }

    #[test]
    fn test_close_dismisses_approval() {
        let mut handler = CardHandler::new();

        let request = ApprovalRequest::Exec {
            id: "cmd-1".to_string(),
            command: vec!["test".into()],
            reason: None,
        };
        handler.request_approval(request);
        assert!(handler.has_pending_approval());

        handler.close();
        assert!(!handler.has_pending_approval());
    }

    #[test]
    fn test_close_all() {
        let mut handler = CardHandler::new();

        // Open multiple cards
        handler.open_help();
        handler.open_models(create_test_models(), None);

        // Request approval
        let request = ApprovalRequest::Exec {
            id: "cmd-1".to_string(),
            command: vec!["test".into()],
            reason: None,
        };
        handler.request_approval(request);

        assert!(handler.is_active());

        handler.close_all();
        assert!(!handler.is_active());
        assert!(!handler.has_pending_approval());
    }

    #[test]
    fn test_take_actions() {
        let mut handler = CardHandler::new();
        handler.open_models(create_test_models(), None);

        // Simulate pressing Enter to select a model
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        handler.handle_key(key);

        // Should have a pending action
        let actions = handler.take_actions();
        assert!(!actions.is_empty());
        assert!(matches!(actions[0], CardAction::SelectModel(_)));

        // Second call should return empty
        let actions2 = handler.take_actions();
        assert!(actions2.is_empty());
    }

    #[test]
    fn test_take_approval_decision() {
        let mut handler = CardHandler::new();

        let request = ApprovalRequest::Exec {
            id: "cmd-1".to_string(),
            command: vec!["test".into()],
            reason: None,
        };
        handler.request_approval(request);

        // Simulate pressing Enter to approve
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        handler.handle_key(key);

        // Should have a pending decision
        let decision = handler.take_approval_decision();
        assert!(decision.is_some());
        let (id, dec) = decision.unwrap();
        assert_eq!(id, "cmd-1");
        assert!(matches!(dec, ApprovalDecision::Approved));

        // Second call should return None
        let decision2 = handler.take_approval_decision();
        assert!(decision2.is_none());
    }

    #[test]
    fn test_key_hints_empty_when_inactive() {
        let handler = CardHandler::new();
        let hints = handler.key_hints();
        assert!(hints.is_empty());
    }

    #[test]
    fn test_key_hints_from_card() {
        let mut handler = CardHandler::new();
        handler.open_help();

        let hints = handler.key_hints();
        assert!(!hints.is_empty());
    }

    #[test]
    fn test_desired_height() {
        let mut handler = CardHandler::new();
        assert_eq!(handler.desired_height(20, 80), 0);

        handler.open_help();
        let height = handler.desired_height(20, 80);
        assert!(height > 0);
        assert!(height <= 20);
    }

    #[test]
    fn test_desired_height_approval() {
        let mut handler = CardHandler::new();

        let request = ApprovalRequest::Exec {
            id: "cmd-1".to_string(),
            command: vec!["test".into()],
            reason: None,
        };
        handler.request_approval(request);

        let height = handler.desired_height(20, 80);
        assert_eq!(height, 10); // Fixed approval height
    }

    #[test]
    fn test_escape_closes_card() {
        let mut handler = CardHandler::new();
        handler.open_help();
        assert!(handler.is_active());

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        handler.handle_key(key);

        // Help card should be closed after Esc
        assert!(!handler.is_active());
    }

    #[test]
    fn test_card_stacking() {
        let mut handler = CardHandler::new();

        // Open first card
        handler.open_help();
        assert_eq!(handler.current_card_title(), Some("Help"));

        // Open second card
        handler.open_models(create_test_models(), None);
        assert_eq!(handler.current_card_title(), Some("Models"));

        // Close top card
        handler.close();
        assert_eq!(handler.current_card_title(), Some("Help"));

        // Close last card
        handler.close();
        assert!(!handler.is_active());
    }
}
