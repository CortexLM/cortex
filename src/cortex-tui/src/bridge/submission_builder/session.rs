//! Session operation methods for SubmissionBuilder.

use cortex_protocol::Op;

use super::SubmissionBuilder;

impl SubmissionBuilder {
    /// Fork the session at a specific point.
    ///
    /// Creates a new branch in the conversation history.
    ///
    /// # Arguments
    ///
    /// * `message_id` - Optional ID of the message to fork from
    /// * `message_index` - Optional index in the conversation
    ///
    /// If both are `None`, forks from the current position.
    pub fn fork_session(message_id: Option<String>, message_index: Option<usize>) -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::ForkSession {
            fork_point_message_id: message_id,
            message_index,
        });
        builder
    }

    /// Fork the session from a specific message ID.
    pub fn fork_from_message(message_id: impl Into<String>) -> Self {
        Self::fork_session(Some(message_id.into()), None)
    }

    /// Fork the session from a specific message index.
    pub fn fork_from_index(index: usize) -> Self {
        Self::fork_session(None, Some(index))
    }

    /// Fork the session from the current position.
    pub fn fork_here() -> Self {
        Self::fork_session(None, None)
    }

    /// Switch to a different agent profile.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let switch = SubmissionBuilder::switch_agent("coder").build_expect();
    /// ```
    pub fn switch_agent(name: impl Into<String>) -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::SwitchAgent { name: name.into() });
        builder
    }

    /// Get the session timeline information.
    pub fn get_session_timeline() -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::GetSessionTimeline);
        builder
    }

    /// Share the current session.
    ///
    /// Makes the session available for sharing/export.
    pub fn share() -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::Share);
        builder
    }

    /// Unshare the current session.
    ///
    /// Removes sharing access for the session.
    pub fn unshare() -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::Unshare);
        builder
    }
}
