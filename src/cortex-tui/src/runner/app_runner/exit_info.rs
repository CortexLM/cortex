//! Exit information types for the application.

use cortex_protocol::ConversationId;

// ============================================================================
// Exit Information
// ============================================================================

/// The reason the application exited.
///
/// This enum captures the different ways the TUI can terminate, which is
/// useful for downstream handling (e.g., displaying appropriate messages,
/// setting exit codes, or cleaning up resources).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExitReason {
    /// User quit normally (e.g., via quit command or Ctrl+Q).
    #[default]
    Normal,
    /// User interrupted the application (e.g., via Ctrl+C).
    Interrupted,
    /// An error occurred during execution.
    Error,
    /// The session ended (e.g., backend disconnected).
    SessionEnded,
}

impl std::fmt::Display for ExitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExitReason::Normal => write!(f, "normal exit"),
            ExitReason::Interrupted => write!(f, "interrupted"),
            ExitReason::Error => write!(f, "error"),
            ExitReason::SessionEnded => write!(f, "session ended"),
        }
    }
}

/// Information about how the application exited.
///
/// This struct is returned by `AppRunner::run()` and contains details about
/// the exit, including the conversation ID (for session resumption) and
/// the reason for termination.
///
/// # Example
///
/// ```rust,ignore
/// let exit_info = runner.run().await?;
/// if let Some(id) = exit_info.conversation_id {
///     println!("Session ID: {} - can be resumed later", id);
/// }
/// match exit_info.exit_reason {
///     ExitReason::Normal => println!("Goodbye!"),
///     ExitReason::Interrupted => println!("Session interrupted"),
///     ExitReason::Error => eprintln!("An error occurred"),
///     ExitReason::SessionEnded => println!("Session ended"),
/// }
/// ```
#[derive(Debug, Clone)]
pub struct AppExitInfo {
    /// The conversation ID (if a session was active).
    ///
    /// This can be used to resume the session later using
    /// `AppRunner::with_conversation_id()`.
    pub conversation_id: Option<ConversationId>,
    /// The reason for exit.
    pub exit_reason: ExitReason,
    /// Message to print after exit (e.g., logout confirmation).
    pub exit_message: Option<String>,
}

impl Default for AppExitInfo {
    fn default() -> Self {
        Self {
            conversation_id: None,
            exit_reason: ExitReason::Normal,
            exit_message: None,
        }
    }
}

impl AppExitInfo {
    /// Create a new exit info with a conversation ID.
    pub fn with_conversation_id(mut self, id: ConversationId) -> Self {
        self.conversation_id = Some(id);
        self
    }

    /// Create a new exit info with an exit reason.
    pub fn with_exit_reason(mut self, reason: ExitReason) -> Self {
        self.exit_reason = reason;
        self
    }

    /// Create a new exit info with an exit message.
    pub fn with_exit_message(mut self, message: impl Into<String>) -> Self {
        self.exit_message = Some(message.into());
        self
    }

    /// Returns true if the exit was due to an error.
    pub fn is_error(&self) -> bool {
        self.exit_reason == ExitReason::Error
    }

    /// Returns true if the exit was normal.
    pub fn is_normal(&self) -> bool {
        self.exit_reason == ExitReason::Normal
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exit_reason_display() {
        assert_eq!(ExitReason::Normal.to_string(), "normal exit");
        assert_eq!(ExitReason::Interrupted.to_string(), "interrupted");
        assert_eq!(ExitReason::Error.to_string(), "error");
        assert_eq!(ExitReason::SessionEnded.to_string(), "session ended");
    }

    #[test]
    fn test_exit_reason_default() {
        assert_eq!(ExitReason::default(), ExitReason::Normal);
    }

    #[test]
    fn test_app_exit_info_default() {
        let info = AppExitInfo::default();
        assert!(info.conversation_id.is_none());
        assert_eq!(info.exit_reason, ExitReason::Normal);
        assert!(info.is_normal());
        assert!(!info.is_error());
    }

    #[test]
    fn test_app_exit_info_builder() {
        let info = AppExitInfo::default().with_exit_reason(ExitReason::Error);

        assert!(info.is_error());
        assert!(!info.is_normal());
    }
}
