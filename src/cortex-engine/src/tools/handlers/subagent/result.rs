//! Subagent execution result types.

use serde::{Deserialize, Serialize};

use super::types::SubagentSession;

/// Result of a subagent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResult {
    /// Whether the execution was successful.
    pub success: bool,
    /// Session information.
    pub session: SubagentSession,
    /// Final output/response from the subagent.
    pub output: String,
    /// Summary of what was accomplished.
    pub summary: Option<String>,
    /// Error message if failed.
    pub error: Option<String>,
    /// Detailed execution log.
    pub execution_log: Vec<ExecutionLogEntry>,
    /// Files that were modified.
    pub files_modified: Vec<FileChange>,
    /// Artifacts produced.
    pub artifacts: Vec<Artifact>,
    /// Token usage breakdown.
    pub token_usage: TokenUsageBreakdown,
    /// Can this session be continued?
    pub can_continue: bool,
    /// Suggested next steps.
    pub next_steps: Vec<String>,
}

impl SubagentResult {
    /// Create a successful result.
    pub fn success(session: SubagentSession, output: impl Into<String>) -> Self {
        Self {
            success: true,
            session,
            output: output.into(),
            summary: None,
            error: None,
            execution_log: Vec::new(),
            files_modified: Vec::new(),
            artifacts: Vec::new(),
            token_usage: TokenUsageBreakdown::default(),
            can_continue: false,
            next_steps: Vec::new(),
        }
    }

    /// Create a failure result.
    pub fn failure(session: SubagentSession, error: impl Into<String>) -> Self {
        Self {
            success: false,
            session,
            output: String::new(),
            summary: None,
            error: Some(error.into()),
            execution_log: Vec::new(),
            files_modified: Vec::new(),
            artifacts: Vec::new(),
            token_usage: TokenUsageBreakdown::default(),
            can_continue: false,
            next_steps: Vec::new(),
        }
    }

    /// Set the summary.
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Add execution log entry.
    pub fn with_log_entry(mut self, entry: ExecutionLogEntry) -> Self {
        self.execution_log.push(entry);
        self
    }

    /// Add file change.
    pub fn with_file_change(mut self, change: FileChange) -> Self {
        self.files_modified.push(change);
        self
    }

    /// Add artifact.
    pub fn with_artifact(mut self, artifact: Artifact) -> Self {
        self.artifacts.push(artifact);
        self
    }

    /// Set token usage.
    pub fn with_token_usage(mut self, usage: TokenUsageBreakdown) -> Self {
        self.token_usage = usage;
        self
    }

    /// Mark as continuable.
    pub fn with_continuation(mut self) -> Self {
        self.can_continue = true;
        self
    }

    /// Add next step suggestion.
    pub fn with_next_step(mut self, step: impl Into<String>) -> Self {
        self.next_steps.push(step.into());
        self
    }

    /// Format as a tool result output string.
    pub fn to_tool_output(&self) -> String {
        let mut output = String::new();

        // Header
        let status_icon = if self.success { "[OK]" } else { "[FAIL]" };
        output.push_str(&format!(
            "{} Subagent ({}) {}\n",
            status_icon,
            self.session.agent_type,
            if self.success { "completed" } else { "failed" }
        ));
        output.push_str(&format!("Session ID: {}\n", self.session.id));
        output.push('\n');

        // Summary or error
        if let Some(ref summary) = self.summary {
            output.push_str("## Summary\n");
            output.push_str(summary);
            output.push_str("\n\n");
        } else if let Some(ref error) = self.error {
            output.push_str("## Error\n");
            output.push_str(error);
            output.push_str("\n\n");
        }

        // Main output
        if !self.output.is_empty() {
            output.push_str("## Output\n");
            output.push_str(&self.output);
            output.push_str("\n\n");
        }

        // Statistics
        output.push_str("## Statistics\n");
        output.push_str(&format!("- Turns: {}\n", self.session.turns_completed));
        output.push_str(&format!("- Tool calls: {}\n", self.session.tool_calls_made));
        output.push_str(&format!("- Total tokens: {}\n", self.session.tokens_used));
        output.push_str(&format!(
            "- Input tokens: {}\n",
            self.token_usage.input_tokens
        ));
        output.push_str(&format!(
            "- Output tokens: {}\n",
            self.token_usage.output_tokens
        ));
        output.push('\n');

        // Files modified
        if !self.files_modified.is_empty() {
            output.push_str("## Files Modified\n");
            for file in &self.files_modified {
                output.push_str(&format!("- {} ({})\n", file.path, file.change_type));
            }
            output.push('\n');
        }

        // Artifacts
        if !self.artifacts.is_empty() {
            output.push_str("## Artifacts\n");
            for artifact in &self.artifacts {
                output.push_str(&format!(
                    "- {}: {}\n",
                    artifact.artifact_type, artifact.name
                ));
            }
            output.push('\n');
        }

        // Next steps
        if !self.next_steps.is_empty() {
            output.push_str("## Suggested Next Steps\n");
            for (i, step) in self.next_steps.iter().enumerate() {
                output.push_str(&format!("{}. {}\n", i + 1, step));
            }
            output.push('\n');
        }

        // Continuation info
        if self.can_continue {
            output.push_str(&format!(
                "\nHint: To continue this task, use session_id: \"{}\"\n",
                self.session.id
            ));
        }

        output
    }
}

/// Entry in the execution log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionLogEntry {
    /// Timestamp (relative to start).
    pub timestamp_ms: u64,
    /// Entry type.
    pub entry_type: LogEntryType,
    /// Message.
    pub message: String,
    /// Additional data.
    pub data: Option<serde_json::Value>,
}

impl ExecutionLogEntry {
    /// Create a new log entry.
    pub fn new(timestamp_ms: u64, entry_type: LogEntryType, message: impl Into<String>) -> Self {
        Self {
            timestamp_ms,
            entry_type,
            message: message.into(),
            data: None,
        }
    }

    /// Add data to the entry.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// Type of log entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogEntryType {
    /// Status change.
    Status,
    /// Tool call.
    ToolCall,
    /// Model output.
    Output,
    /// Error.
    Error,
    /// Warning.
    Warning,
    /// Info.
    Info,
}

/// Record of a file change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    /// File path.
    pub path: String,
    /// Type of change.
    pub change_type: FileChangeType,
    /// Lines added.
    pub lines_added: Option<u32>,
    /// Lines removed.
    pub lines_removed: Option<u32>,
    /// Brief description of change.
    pub description: Option<String>,
}

impl FileChange {
    /// Create a new file change record.
    pub fn new(path: impl Into<String>, change_type: FileChangeType) -> Self {
        Self {
            path: path.into(),
            change_type,
            lines_added: None,
            lines_removed: None,
            description: None,
        }
    }

    /// Set line counts.
    pub fn with_lines(mut self, added: u32, removed: u32) -> Self {
        self.lines_added = Some(added);
        self.lines_removed = Some(removed);
        self
    }

    /// Set description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// Type of file change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeType {
    /// File was created.
    Created,
    /// File was modified.
    Modified,
    /// File was deleted.
    Deleted,
    /// File was renamed.
    Renamed,
}

impl std::fmt::Display for FileChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Modified => write!(f, "modified"),
            Self::Deleted => write!(f, "deleted"),
            Self::Renamed => write!(f, "renamed"),
        }
    }
}

/// Artifact produced by a subagent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Artifact type.
    pub artifact_type: String,
    /// Artifact name.
    pub name: String,
    /// Content (inline) or path.
    pub content: ArtifactContent,
    /// MIME type.
    pub mime_type: Option<String>,
    /// Size in bytes.
    pub size: Option<u64>,
}

impl Artifact {
    /// Create an inline artifact.
    pub fn inline(
        artifact_type: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            artifact_type: artifact_type.into(),
            name: name.into(),
            content: ArtifactContent::Inline(content.into()),
            mime_type: None,
            size: None,
        }
    }

    /// Create a file reference artifact.
    pub fn file_ref(
        artifact_type: impl Into<String>,
        name: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            artifact_type: artifact_type.into(),
            name: name.into(),
            content: ArtifactContent::FilePath(path.into()),
            mime_type: None,
            size: None,
        }
    }

    /// Set MIME type.
    pub fn with_mime_type(mut self, mime: impl Into<String>) -> Self {
        self.mime_type = Some(mime.into());
        self
    }

    /// Set size.
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }
}

/// Content of an artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArtifactContent {
    /// Inline content.
    Inline(String),
    /// Reference to a file.
    FilePath(String),
    /// Base64 encoded binary.
    Base64(String),
}

/// Token usage breakdown.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsageBreakdown {
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Cached tokens.
    pub cached_tokens: u64,
    /// Reasoning tokens (for o1/o3).
    pub reasoning_tokens: u64,
    /// Per-turn breakdown.
    pub per_turn: Vec<TurnTokenUsage>,
}

impl TokenUsageBreakdown {
    /// Get total tokens.
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Add usage from a turn.
    pub fn add_turn(&mut self, input: u64, output: u64, cached: u64, reasoning: u64) {
        self.input_tokens += input;
        self.output_tokens += output;
        self.cached_tokens += cached;
        self.reasoning_tokens += reasoning;
        self.per_turn.push(TurnTokenUsage {
            turn: self.per_turn.len() as u32 + 1,
            input,
            output,
            cached,
            reasoning,
        });
    }
}

/// Token usage for a single turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnTokenUsage {
    /// Turn number.
    pub turn: u32,
    /// Input tokens.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Cached tokens.
    pub cached: u64,
    /// Reasoning tokens.
    pub reasoning: u64,
}

/// Builder for SubagentResult.
pub struct SubagentResultBuilder {
    result: SubagentResult,
}

impl SubagentResultBuilder {
    /// Create a new builder.
    pub fn new(session: SubagentSession) -> Self {
        Self {
            result: SubagentResult {
                success: false,
                session,
                output: String::new(),
                summary: None,
                error: None,
                execution_log: Vec::new(),
                files_modified: Vec::new(),
                artifacts: Vec::new(),
                token_usage: TokenUsageBreakdown::default(),
                can_continue: false,
                next_steps: Vec::new(),
            },
        }
    }

    /// Set success status.
    pub fn success(mut self, success: bool) -> Self {
        self.result.success = success;
        self
    }

    /// Set output.
    pub fn output(mut self, output: impl Into<String>) -> Self {
        self.result.output = output.into();
        self
    }

    /// Set summary.
    #[allow(dead_code)]
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.result.summary = Some(summary.into());
        self
    }

    /// Set error.
    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.result.error = Some(error.into());
        self
    }

    /// Add log entry.
    #[allow(dead_code)]
    pub fn log(mut self, entry: ExecutionLogEntry) -> Self {
        self.result.execution_log.push(entry);
        self
    }

    /// Add file change.
    pub fn file_changed(mut self, change: FileChange) -> Self {
        self.result.files_modified.push(change);
        self
    }

    /// Add artifact.
    #[allow(dead_code)]
    pub fn artifact(mut self, artifact: Artifact) -> Self {
        self.result.artifacts.push(artifact);
        self
    }

    /// Set token usage.
    pub fn tokens(mut self, usage: TokenUsageBreakdown) -> Self {
        self.result.token_usage = usage;
        self
    }

    /// Enable continuation.
    pub fn continuable(mut self) -> Self {
        self.result.can_continue = true;
        self
    }

    /// Add next step.
    #[allow(dead_code)]
    pub fn next_step(mut self, step: impl Into<String>) -> Self {
        self.result.next_steps.push(step.into());
        self
    }

    /// Build the result.
    pub fn build(self) -> SubagentResult {
        self.result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::handlers::subagent::types::SubagentType;
    use std::path::PathBuf;

    fn make_session() -> SubagentSession {
        SubagentSession::new(
            "test-session",
            None,
            SubagentType::Code,
            "Test task",
            PathBuf::from("/project"),
        )
    }

    #[test]
    fn test_subagent_result_success() {
        let session = make_session();
        let result = SubagentResult::success(session, "Task completed successfully")
            .with_summary("Implemented the feature")
            .with_continuation();

        assert!(result.success);
        assert!(result.can_continue);
        assert_eq!(result.summary.as_deref(), Some("Implemented the feature"));
    }

    #[test]
    fn test_subagent_result_failure() {
        let session = make_session();
        let result = SubagentResult::failure(session, "Connection timeout");

        assert!(!result.success);
        assert_eq!(result.error.as_deref(), Some("Connection timeout"));
    }

    #[test]
    fn test_file_change() {
        let change = FileChange::new("src/main.rs", FileChangeType::Modified)
            .with_lines(10, 5)
            .with_description("Added new function");

        assert_eq!(change.path, "src/main.rs");
        assert_eq!(change.lines_added, Some(10));
        assert_eq!(change.lines_removed, Some(5));
    }

    #[test]
    fn test_token_usage_breakdown() {
        let mut usage = TokenUsageBreakdown::default();
        usage.add_turn(100, 50, 20, 0);
        usage.add_turn(150, 75, 30, 0);

        assert_eq!(usage.input_tokens, 250);
        assert_eq!(usage.output_tokens, 125);
        assert_eq!(usage.total(), 375);
        assert_eq!(usage.per_turn.len(), 2);
    }

    #[test]
    fn test_result_builder() {
        let session = make_session();
        let result = SubagentResultBuilder::new(session)
            .success(true)
            .output("Done")
            .summary("Task complete")
            .continuable()
            .next_step("Review the changes")
            .build();

        assert!(result.success);
        assert!(result.can_continue);
        assert_eq!(result.next_steps.len(), 1);
    }

    #[test]
    fn test_to_tool_output() {
        let mut session = make_session();
        session.turns_completed = 3;
        session.tool_calls_made = 7;
        session.tokens_used = 1500;

        let result = SubagentResult::success(session, "Feature implemented")
            .with_summary("Added user authentication module")
            .with_file_change(FileChange::new("src/auth.rs", FileChangeType::Created))
            .with_continuation();

        let output = result.to_tool_output();
        assert!(output.contains("[OK]"));
        assert!(output.contains("code"));
        assert!(output.contains("src/auth.rs"));
        assert!(output.contains("test-session"));
    }
}
