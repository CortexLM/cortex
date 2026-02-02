//! TUI Backtracking System
//!
//! Allows navigation through conversation history and rollback to previous states.
//!
//! # Usage
//!
//! - Press `Esc` once to prime backtrack mode (500ms window)
//! - Press `Esc` again within 500ms to open the transcript overlay
//! - Use arrow keys (←/→ or ↑/↓) to navigate between user messages
//! - Press `Enter` to confirm rollback to selected state
//! - Press `Esc` again to cancel and close the overlay
//! - Press `f` to fork session from the selected message

use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Double-Esc detection window in milliseconds.
const DOUBLE_ESC_WINDOW_MS: u64 = 500;

/// Current mode of the backtracking system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BacktrackMode {
    /// Normal mode - backtracking not active.
    #[default]
    Normal,
    /// Primed mode - first Esc pressed, waiting for second.
    Primed,
    /// Overlay mode - transcript overlay is open for navigation.
    Overlay,
}

/// Actions that can result from handling backtrack events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BacktrackAction {
    /// No action needed.
    None,
    /// Show a hint message to the user.
    ShowHint(&'static str),
    /// Open the backtrack overlay.
    OpenOverlay,
    /// Close the overlay without action.
    CloseOverlay,
    /// Update the selection to the given index.
    UpdateSelection(usize),
    /// Rollback to the message with the given ID.
    RollbackTo(String),
    /// Fork session from the message with the given ID.
    ForkFrom(String),
}

/// Direction for navigation within the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Navigate to earlier message (left/up).
    Previous,
    /// Navigate to later message (right/down).
    Next,
}

/// State for the backtracking system.
#[derive(Debug)]
pub struct BacktrackState {
    /// Current backtrack mode.
    pub mode: BacktrackMode,

    /// Timestamp of the last Esc press (for double-Esc detection).
    last_esc: Option<Instant>,

    /// Session/thread ID of the base conversation.
    pub base_id: Option<String>,

    /// Index of the currently highlighted user message (1-indexed).
    pub nth_user_message: usize,

    /// Total number of user messages in the conversation.
    pub total_user_messages: usize,

    /// Pending rollback awaiting confirmation.
    pub pending_rollback: Option<PendingBacktrackRollback>,

    /// Snapshots of messages for display in the overlay.
    pub message_snapshots: Vec<MessageSnapshot>,
}

impl Default for BacktrackState {
    fn default() -> Self {
        Self::new()
    }
}

impl BacktrackState {
    /// Create a new backtrack state.
    pub fn new() -> Self {
        Self {
            mode: BacktrackMode::Normal,
            last_esc: None,
            base_id: None,
            nth_user_message: 0,
            total_user_messages: 0,
            pending_rollback: None,
            message_snapshots: Vec::new(),
        }
    }

    /// Reset the backtrack state.
    pub fn reset(&mut self) {
        self.mode = BacktrackMode::Normal;
        self.last_esc = None;
        self.pending_rollback = None;
        self.nth_user_message = 0;
    }

    /// Check if backtrack mode is primed (first Esc pressed).
    pub fn primed(&self) -> bool {
        self.mode == BacktrackMode::Primed
    }

    /// Check if the overlay is currently active.
    pub fn overlay_preview_active(&self) -> bool {
        self.mode == BacktrackMode::Overlay
    }

    /// Check if backtrack mode is active (primed or overlay).
    pub fn is_active(&self) -> bool {
        self.mode != BacktrackMode::Normal
    }

    /// Handle an Esc key press with double-Esc detection.
    ///
    /// Returns the appropriate action based on current state and timing.
    pub fn handle_esc(&mut self) -> BacktrackAction {
        let now = Instant::now();

        match self.mode {
            BacktrackMode::Normal => {
                // First Esc - prime the backtrack mode
                self.mode = BacktrackMode::Primed;
                self.last_esc = Some(now);
                BacktrackAction::ShowHint("Press Esc again to rewind")
            }
            BacktrackMode::Primed => {
                // Check if within the double-Esc window
                if let Some(last) = self.last_esc
                    && now.duration_since(last) < Duration::from_millis(DOUBLE_ESC_WINDOW_MS)
                {
                    // Second Esc within window - open overlay
                    if self.total_user_messages > 0 {
                        self.mode = BacktrackMode::Overlay;
                        self.nth_user_message = self.total_user_messages;
                        return BacktrackAction::OpenOverlay;
                    } else {
                        // No messages to backtrack to
                        self.reset();
                        return BacktrackAction::ShowHint("No messages to rewind to");
                    }
                }
                // Timeout or no previous Esc - reset to normal
                self.reset();
                BacktrackAction::None
            }
            BacktrackMode::Overlay => {
                // Esc in overlay - close it
                self.reset();
                BacktrackAction::CloseOverlay
            }
        }
    }

    /// Check and reset primed state if the timeout has elapsed.
    ///
    /// Call this periodically (e.g., on tick) to auto-reset primed state.
    pub fn check_primed_timeout(&mut self) {
        if self.mode == BacktrackMode::Primed
            && let Some(last) = self.last_esc
            && Instant::now().duration_since(last) >= Duration::from_millis(DOUBLE_ESC_WINDOW_MS)
        {
            self.reset();
        }
    }

    /// Handle arrow key navigation in the overlay.
    pub fn handle_navigation(&mut self, direction: Direction) -> BacktrackAction {
        if self.mode != BacktrackMode::Overlay {
            return BacktrackAction::None;
        }

        match direction {
            Direction::Previous => self.select_previous(),
            Direction::Next => self.select_next(self.total_user_messages),
        }

        BacktrackAction::UpdateSelection(self.nth_user_message)
    }

    /// Handle Enter key to confirm rollback.
    pub fn handle_enter(&mut self) -> BacktrackAction {
        if self.mode != BacktrackMode::Overlay {
            return BacktrackAction::None;
        }

        // Get the selected message ID
        if let Some(snapshot) = self.get_selected_snapshot() {
            let message_id = snapshot.id.clone();
            self.reset();
            return BacktrackAction::RollbackTo(message_id);
        }

        BacktrackAction::None
    }

    /// Handle 'f' key to fork session from selected message.
    pub fn handle_fork(&mut self) -> BacktrackAction {
        if self.mode != BacktrackMode::Overlay {
            return BacktrackAction::None;
        }

        if let Some(snapshot) = self.get_selected_snapshot() {
            let message_id = snapshot.id.clone();
            self.reset();
            return BacktrackAction::ForkFrom(message_id);
        }

        BacktrackAction::None
    }

    /// Get the currently selected message snapshot.
    pub fn get_selected_snapshot(&self) -> Option<&MessageSnapshot> {
        if self.nth_user_message == 0 || self.message_snapshots.is_empty() {
            return None;
        }

        // nth_user_message is 1-indexed, find the nth user message
        self.message_snapshots
            .iter()
            .filter(|s| s.role == MessageRole::User)
            .nth(self.nth_user_message.saturating_sub(1))
    }

    /// Update message snapshots for display.
    pub fn update_snapshots(&mut self, snapshots: Vec<MessageSnapshot>) {
        self.total_user_messages = snapshots
            .iter()
            .filter(|s| s.role == MessageRole::User)
            .count();
        self.message_snapshots = snapshots;

        // Reset selection if out of bounds
        if self.nth_user_message > self.total_user_messages {
            self.nth_user_message = self.total_user_messages;
        }
    }

    /// Legacy method: Prime backtrack mode (first Esc press).
    pub fn prime(&mut self) {
        self.mode = BacktrackMode::Primed;
        self.last_esc = Some(Instant::now());
    }

    /// Legacy method: Activate the overlay preview (second Esc press).
    pub fn activate_overlay(&mut self, total_user_messages: usize) {
        if total_user_messages > 0 {
            self.mode = BacktrackMode::Overlay;
            self.total_user_messages = total_user_messages;
            self.nth_user_message = total_user_messages;
        }
    }

    /// Move selection to previous user message.
    pub fn select_previous(&mut self) {
        if self.nth_user_message > 1 {
            self.nth_user_message -= 1;
        }
    }

    /// Move selection to next user message.
    pub fn select_next(&mut self, total_user_messages: usize) {
        if self.nth_user_message < total_user_messages {
            self.nth_user_message += 1;
        }
    }

    /// Set pending rollback.
    pub fn set_pending_rollback(&mut self, rollback: PendingBacktrackRollback) {
        self.pending_rollback = Some(rollback);
    }

    /// Clear pending rollback.
    pub fn clear_pending(&mut self) {
        self.pending_rollback = None;
    }

    /// Check if a rollback is pending.
    pub fn has_pending_rollback(&self) -> bool {
        self.pending_rollback.is_some()
    }

    /// Create a fork request from the currently selected message.
    pub fn create_fork_request(&self) -> Option<ForkRequest> {
        self.get_selected_snapshot().map(|snapshot| ForkRequest {
            message_id: snapshot.id.clone(),
            timestamp: snapshot.timestamp,
            new_session_id: Uuid::new_v4().to_string(),
        })
    }
}

/// Pending rollback operation.
#[derive(Debug, Clone)]
pub struct PendingBacktrackRollback {
    /// Number of turns to roll back.
    pub num_turns: u32,

    /// Selection that triggered the rollback.
    pub selection: BacktrackSelection,
}

/// Selection of a user message for backtracking.
#[derive(Debug, Clone)]
pub struct BacktrackSelection {
    /// Index of the selected user message (1-indexed).
    pub nth_user_message: usize,

    /// Prefill text for the composer (the selected message).
    pub prefill: String,

    /// Text elements from the selected message.
    pub text_elements: Vec<TextElement>,

    /// Local image paths from the selected message.
    pub local_image_paths: Vec<PathBuf>,
}

impl BacktrackSelection {
    /// Create a new backtrack selection.
    pub fn new(nth_user_message: usize, prefill: impl Into<String>) -> Self {
        Self {
            nth_user_message,
            prefill: prefill.into(),
            text_elements: Vec::new(),
            local_image_paths: Vec::new(),
        }
    }

    /// Add a text element.
    pub fn with_text_element(mut self, element: TextElement) -> Self {
        self.text_elements.push(element);
        self
    }

    /// Add an image path.
    pub fn with_image(mut self, path: PathBuf) -> Self {
        self.local_image_paths.push(path);
        self
    }
}

/// A text element from a message.
#[derive(Debug, Clone)]
pub struct TextElement {
    /// The text content.
    pub content: String,

    /// Element type.
    pub element_type: TextElementType,
}

/// Type of text element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextElementType {
    /// Plain text.
    Text,

    /// Code block.
    Code,

    /// Inline code.
    InlineCode,

    /// Link.
    Link,
}

impl TextElement {
    /// Create a plain text element.
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            element_type: TextElementType::Text,
        }
    }

    /// Create a code element.
    pub fn code(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            element_type: TextElementType::Code,
        }
    }
}

/// Count user messages in a transcript.
pub fn count_user_messages<T>(messages: &[T], is_user: impl Fn(&T) -> bool) -> usize {
    messages.iter().filter(|m| is_user(m)).count()
}

/// Get the Nth user message from a transcript.
pub fn get_nth_user_message<T>(
    messages: &[T],
    n: usize,
    is_user: impl Fn(&T) -> bool,
) -> Option<&T> {
    messages
        .iter()
        .filter(|m| is_user(m))
        .nth(n.saturating_sub(1))
}

/// Calculate the number of turns to roll back.
pub fn calculate_rollback_turns(total_user_messages: usize, selected_nth: usize) -> u32 {
    let num_turns = total_user_messages.saturating_sub(selected_nth);
    u32::try_from(num_turns).unwrap_or(u32::MAX)
}

/// Keyboard hint for backtrack mode.
pub fn backtrack_hint(primed: bool, overlay_active: bool) -> &'static str {
    if overlay_active {
        "←/→: navigate | Enter: confirm | f: fork | Esc: cancel"
    } else if primed {
        "Press Esc again to open backtrack view"
    } else {
        ""
    }
}

// ============================================================================
// MESSAGE SNAPSHOT
// ============================================================================

/// A snapshot of a message for display in the backtrack overlay.
#[derive(Debug, Clone)]
pub struct MessageSnapshot {
    /// Unique identifier for the message.
    pub id: String,
    /// Role of the message sender.
    pub role: MessageRole,
    /// Content of the message (may be truncated for display).
    pub content: String,
    /// Timestamp of the message.
    pub timestamp: DateTime<Utc>,
    /// Tool calls made in this message (for assistant messages).
    pub tool_calls: Vec<String>,
}

impl MessageSnapshot {
    /// Create a new message snapshot.
    pub fn new(id: impl Into<String>, role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            role,
            content: content.into(),
            timestamp: Utc::now(),
            tool_calls: Vec::new(),
        }
    }

    /// Set the timestamp.
    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.timestamp = timestamp;
        self
    }

    /// Add tool calls.
    pub fn with_tool_calls(mut self, tool_calls: Vec<String>) -> Self {
        self.tool_calls = tool_calls;
        self
    }
}

/// Role of a message sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    /// User message.
    User,
    /// Assistant message.
    Assistant,
    /// System message.
    System,
}

// ============================================================================
// FORK REQUEST
// ============================================================================

/// A request to fork the session from a specific message.
#[derive(Debug, Clone)]
pub struct ForkRequest {
    /// ID of the message to fork from.
    pub message_id: String,
    /// Timestamp of the message.
    pub timestamp: DateTime<Utc>,
    /// ID for the new forked session.
    pub new_session_id: String,
}

impl ForkRequest {
    /// Create a new fork request.
    pub fn new(message_id: impl Into<String>) -> Self {
        Self {
            message_id: message_id.into(),
            timestamp: Utc::now(),
            new_session_id: Uuid::new_v4().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_backtrack_state_basic() {
        let mut state = BacktrackState::new();

        assert!(!state.is_active());
        assert_eq!(state.mode, BacktrackMode::Normal);

        state.prime();
        assert!(state.is_active());
        assert!(state.primed());

        state.total_user_messages = 5;
        state.activate_overlay(5);
        assert!(state.overlay_preview_active());
        assert_eq!(state.nth_user_message, 5);

        state.select_previous();
        assert_eq!(state.nth_user_message, 4);

        state.select_next(5);
        assert_eq!(state.nth_user_message, 5);

        state.reset();
        assert!(!state.is_active());
    }

    #[test]
    fn test_backtrack_double_esc() {
        let mut state = BacktrackState::new();
        state.total_user_messages = 5;

        // First Esc - should prime
        let action = state.handle_esc();
        assert!(matches!(action, BacktrackAction::ShowHint(_)));
        assert_eq!(state.mode, BacktrackMode::Primed);

        // Second Esc quickly - should open overlay
        let action = state.handle_esc();
        assert!(matches!(action, BacktrackAction::OpenOverlay));
        assert_eq!(state.mode, BacktrackMode::Overlay);
    }

    #[test]
    fn test_backtrack_esc_timeout() {
        let mut state = BacktrackState::new();
        state.total_user_messages = 5;

        // First Esc - should prime
        state.handle_esc();
        assert_eq!(state.mode, BacktrackMode::Primed);

        // Wait for timeout (600ms > 500ms window)
        thread::sleep(Duration::from_millis(600));

        // Simulate the check_primed_timeout that would be called on tick
        state.check_primed_timeout();

        // After timeout, should be back to Normal
        assert_eq!(state.mode, BacktrackMode::Normal);

        // Next Esc should prime again
        let action = state.handle_esc();
        assert!(matches!(action, BacktrackAction::ShowHint(_)));
        assert_eq!(state.mode, BacktrackMode::Primed);
    }

    #[test]
    fn test_backtrack_navigation() {
        let mut state = BacktrackState::new();
        state.total_user_messages = 5;
        state.mode = BacktrackMode::Overlay;
        state.nth_user_message = 5;

        // Navigate previous
        let action = state.handle_navigation(Direction::Previous);
        assert!(matches!(action, BacktrackAction::UpdateSelection(4)));
        assert_eq!(state.nth_user_message, 4);

        // Navigate next
        let action = state.handle_navigation(Direction::Next);
        assert!(matches!(action, BacktrackAction::UpdateSelection(5)));
        assert_eq!(state.nth_user_message, 5);

        // Can't go past the end
        state.handle_navigation(Direction::Next);
        assert_eq!(state.nth_user_message, 5);
    }

    #[test]
    fn test_backtrack_close_overlay() {
        let mut state = BacktrackState::new();
        state.mode = BacktrackMode::Overlay;
        state.total_user_messages = 5;

        let action = state.handle_esc();
        assert!(matches!(action, BacktrackAction::CloseOverlay));
        assert_eq!(state.mode, BacktrackMode::Normal);
    }

    #[test]
    fn test_backtrack_enter_rollback() {
        let mut state = BacktrackState::new();
        state.mode = BacktrackMode::Overlay;
        state.nth_user_message = 2;
        state.message_snapshots = vec![
            MessageSnapshot::new("msg1", MessageRole::User, "First message"),
            MessageSnapshot::new("msg2", MessageRole::Assistant, "Response"),
            MessageSnapshot::new("msg3", MessageRole::User, "Second message"),
        ];

        let action = state.handle_enter();
        assert!(matches!(action, BacktrackAction::RollbackTo(id) if id == "msg3"));
        assert_eq!(state.mode, BacktrackMode::Normal);
    }

    #[test]
    fn test_backtrack_fork() {
        let mut state = BacktrackState::new();
        state.mode = BacktrackMode::Overlay;
        state.nth_user_message = 1;
        state.message_snapshots = vec![
            MessageSnapshot::new("msg1", MessageRole::User, "First message"),
            MessageSnapshot::new("msg2", MessageRole::Assistant, "Response"),
        ];

        let action = state.handle_fork();
        assert!(matches!(action, BacktrackAction::ForkFrom(id) if id == "msg1"));
        assert_eq!(state.mode, BacktrackMode::Normal);
    }

    #[test]
    fn test_message_snapshot() {
        let snapshot = MessageSnapshot::new("test-id", MessageRole::User, "Hello world");
        assert_eq!(snapshot.id, "test-id");
        assert_eq!(snapshot.role, MessageRole::User);
        assert_eq!(snapshot.content, "Hello world");
    }

    #[test]
    fn test_fork_request() {
        let request = ForkRequest::new("msg-123");
        assert_eq!(request.message_id, "msg-123");
        assert!(!request.new_session_id.is_empty());
    }

    #[test]
    fn test_calculate_rollback_turns() {
        assert_eq!(calculate_rollback_turns(10, 10), 0);
        assert_eq!(calculate_rollback_turns(10, 8), 2);
        assert_eq!(calculate_rollback_turns(10, 1), 9);
    }

    #[test]
    fn test_count_user_messages() {
        let messages = vec![true, false, true, false, true];
        assert_eq!(count_user_messages(&messages, |&m| m), 3);
    }

    #[test]
    fn test_backtrack_hint() {
        assert_eq!(backtrack_hint(false, false), "");
        assert_eq!(
            backtrack_hint(true, false),
            "Press Esc again to open backtrack view"
        );
        assert!(backtrack_hint(false, true).contains("navigate"));
        assert!(backtrack_hint(false, true).contains("fork"));
    }
}
