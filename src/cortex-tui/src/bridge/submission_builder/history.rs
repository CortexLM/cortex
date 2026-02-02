//! History operation methods for SubmissionBuilder.

use cortex_protocol::Op;

use super::SubmissionBuilder;

impl SubmissionBuilder {
    /// Add an entry to persistent history.
    ///
    /// # Arguments
    ///
    /// * `text` - The text to add to history
    pub fn add_to_history(text: impl Into<String>) -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::AddToHistory { text: text.into() });
        builder
    }

    /// Request a history entry.
    ///
    /// # Arguments
    ///
    /// * `offset` - The offset in history
    /// * `log_id` - The log ID
    pub fn get_history_entry(offset: usize, log_id: u64) -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::GetHistoryEntryRequest { offset, log_id });
        builder
    }

    /// List available custom prompts.
    pub fn list_custom_prompts() -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::ListCustomPrompts);
        builder
    }

    /// Execute a user shell command (!cmd).
    ///
    /// This runs a command in the user's shell, not through the agent.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // User typed "!ls -la"
    /// let cmd = SubmissionBuilder::run_shell_command("ls -la").build_expect();
    /// ```
    pub fn run_shell_command(command: impl Into<String>) -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::RunUserShellCommand {
            command: command.into(),
        });
        builder
    }
}
