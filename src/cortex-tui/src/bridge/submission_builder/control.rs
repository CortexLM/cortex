//! Control operation methods for SubmissionBuilder.

use cortex_protocol::Op;

use super::SubmissionBuilder;

impl SubmissionBuilder {
    /// Create an interrupt submission.
    ///
    /// Sends an interrupt signal to abort the current agent task.
    /// The agent will stop processing and return control to the user.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // User pressed Ctrl+C
    /// let interrupt = SubmissionBuilder::interrupt().build_expect();
    /// sender.send(interrupt).await?;
    /// ```
    pub fn interrupt() -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::Interrupt);
        builder
    }

    /// Create a shutdown submission.
    ///
    /// Gracefully shuts down the agent session. This should be used
    /// when the user wants to exit the application.
    pub fn shutdown() -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::Shutdown);
        builder
    }

    /// Create a compact (context compression) submission.
    ///
    /// Requests the agent to compress the conversation context
    /// to reduce token usage while preserving important information.
    pub fn compact() -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::Compact);
        builder
    }

    /// Create an undo submission.
    ///
    /// Undoes the last agent turn, restoring the previous state.
    pub fn undo() -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::Undo);
        builder
    }

    /// Create a redo submission.
    ///
    /// Redoes the last undone turn.
    pub fn redo() -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::Redo);
        builder
    }
}
