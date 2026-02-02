//! Subagent progress tracking and events.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::types::{SubagentStatus, SubagentType};

/// Progress event from a subagent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressEvent {
    /// Subagent has been spawned and is initializing.
    Started {
        session_id: String,
        agent_type: String,
        description: String,
    },

    /// Subagent status changed.
    StatusChanged {
        session_id: String,
        old_status: String,
        new_status: String,
    },

    /// Subagent started thinking/generating.
    Thinking {
        session_id: String,
        turn_number: u32,
    },

    /// Subagent generated text output.
    TextOutput {
        session_id: String,
        content: String,
        is_partial: bool,
    },

    /// Subagent is calling a tool.
    ToolCallStarted {
        session_id: String,
        tool_name: String,
        tool_id: String,
        arguments_preview: String,
    },

    /// Tool call completed.
    ToolCallCompleted {
        session_id: String,
        tool_name: String,
        tool_id: String,
        success: bool,
        output_preview: String,
        duration_ms: u64,
    },

    /// Tool call requires approval.
    ToolCallPending {
        session_id: String,
        tool_name: String,
        tool_id: String,
        arguments: String,
        risk_level: String,
    },

    /// File was modified by the subagent.
    FileModified {
        session_id: String,
        path: String,
        operation: String, // "created", "edited", "deleted"
    },

    /// Turn completed.
    TurnCompleted {
        session_id: String,
        turn_number: u32,
        tool_calls_count: u32,
        tokens_used: u64,
    },

    /// Subagent completed successfully.
    Completed {
        session_id: String,
        output: String,
        total_turns: u32,
        total_tool_calls: u32,
        total_tokens: u64,
        duration_ms: u64,
    },

    /// Subagent failed with error.
    Failed {
        session_id: String,
        error: String,
        recoverable: bool,
    },

    /// Subagent was cancelled.
    Cancelled { session_id: String, reason: String },

    /// Informational message.
    Info { session_id: String, message: String },

    /// Warning message.
    Warning { session_id: String, message: String },

    /// Todo list updated (from TodoWrite tool).
    TodoUpdated {
        session_id: String,
        /// Todo items: (content, status) where status is "pending", "in_progress", or "completed"
        todos: Vec<(String, String)>,
    },
}

impl ProgressEvent {
    /// Get the session ID from the event.
    pub fn session_id(&self) -> &str {
        match self {
            Self::Started { session_id, .. } => session_id,
            Self::StatusChanged { session_id, .. } => session_id,
            Self::Thinking { session_id, .. } => session_id,
            Self::TextOutput { session_id, .. } => session_id,
            Self::ToolCallStarted { session_id, .. } => session_id,
            Self::ToolCallCompleted { session_id, .. } => session_id,
            Self::ToolCallPending { session_id, .. } => session_id,
            Self::FileModified { session_id, .. } => session_id,
            Self::TurnCompleted { session_id, .. } => session_id,
            Self::Completed { session_id, .. } => session_id,
            Self::Failed { session_id, .. } => session_id,
            Self::Cancelled { session_id, .. } => session_id,
            Self::Info { session_id, .. } => session_id,
            Self::Warning { session_id, .. } => session_id,
            Self::TodoUpdated { session_id, .. } => session_id,
        }
    }

    /// Check if this is a terminal event.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled { .. }
        )
    }

    /// Convert to a user-friendly message.
    pub fn to_message(&self) -> String {
        match self {
            Self::Started {
                agent_type,
                description,
                ..
            } => {
                format!("Spawned {} subagent: {}", agent_type, description)
            }
            Self::StatusChanged { new_status, .. } => {
                format!("Status: {}", new_status)
            }
            Self::Thinking { turn_number, .. } => {
                format!("Thinking (turn {})", turn_number)
            }
            Self::TextOutput { content, .. } => {
                if content.len() > 100 {
                    format!("{}...", &content[..100])
                } else {
                    format!("{}", content)
                }
            }
            Self::ToolCallStarted { tool_name, .. } => {
                format!("Calling: {}", tool_name)
            }
            Self::ToolCallCompleted {
                tool_name,
                success,
                duration_ms,
                ..
            } => {
                let status = if *success { "✓" } else { "✗" };
                format!("{} {} ({}ms)", status, tool_name, duration_ms)
            }
            Self::ToolCallPending {
                tool_name,
                risk_level,
                ..
            } => {
                format!("⏸️ Awaiting approval: {} ({})", tool_name, risk_level)
            }
            Self::FileModified {
                path, operation, ..
            } => {
                format!("{} {}", operation, path)
            }
            Self::TurnCompleted {
                turn_number,
                tool_calls_count,
                ..
            } => {
                format!(
                    "Turn {} complete ({} tool calls)",
                    turn_number, tool_calls_count
                )
            }
            Self::Completed {
                total_turns,
                total_tool_calls,
                duration_ms,
                ..
            } => {
                format!(
                    "Completed: {} turns, {} tool calls, {}ms",
                    total_turns, total_tool_calls, duration_ms
                )
            }
            Self::Failed { error, .. } => {
                format!("Failed: {}", error)
            }
            Self::Cancelled { reason, .. } => {
                format!("Cancelled: {}", reason)
            }
            Self::Info { message, .. } => {
                format!("Info: {}", message)
            }
            Self::Warning { message, .. } => {
                format!("Warning: {}", message)
            }
            Self::TodoUpdated { todos, .. } => {
                let in_progress = todos
                    .iter()
                    .filter(|(_, status)| status == "in_progress")
                    .count();
                let completed = todos
                    .iter()
                    .filter(|(_, status)| status == "completed")
                    .count();
                format!(
                    "Todos: {} items ({} in progress, {} completed)",
                    todos.len(),
                    in_progress,
                    completed
                )
            }
        }
    }
}

/// Progress tracker for subagent execution.
pub struct SubagentProgress {
    /// Session ID.
    session_id: String,
    /// Agent type.
    agent_type: SubagentType,
    /// Description.
    #[allow(dead_code)]
    description: String,
    /// Start time.
    start_time: Instant,
    /// Current status.
    status: SubagentStatus,
    /// Current turn number.
    current_turn: u32,
    /// Total tool calls.
    total_tool_calls: u32,
    /// Total tokens.
    total_tokens: u64,
    /// Event sender.
    event_tx: mpsc::UnboundedSender<ProgressEvent>,
    /// Files modified.
    files_modified: Vec<String>,
}

impl SubagentProgress {
    /// Create a new progress tracker.
    pub fn new(
        session_id: impl Into<String>,
        agent_type: SubagentType,
        description: impl Into<String>,
        event_tx: mpsc::UnboundedSender<ProgressEvent>,
    ) -> Self {
        let session_id = session_id.into();
        let description = description.into();

        // Send started event
        let _ = event_tx.send(ProgressEvent::Started {
            session_id: session_id.clone(),
            agent_type: agent_type.name().to_string(),
            description: description.clone(),
        });

        Self {
            session_id,
            agent_type,
            description,
            start_time: Instant::now(),
            status: SubagentStatus::Initializing,
            current_turn: 0,
            total_tool_calls: 0,
            total_tokens: 0,
            event_tx,
            files_modified: Vec::new(),
        }
    }

    /// Get the session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Get elapsed time.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Set status.
    pub fn set_status(&mut self, status: SubagentStatus) {
        let old_status = self.status;
        self.status = status;

        let _ = self.event_tx.send(ProgressEvent::StatusChanged {
            session_id: self.session_id.clone(),
            old_status: old_status.to_string(),
            new_status: status.to_string(),
        });
    }

    /// Record thinking start.
    pub fn start_thinking(&mut self) {
        self.current_turn += 1;
        let _ = self.event_tx.send(ProgressEvent::Thinking {
            session_id: self.session_id.clone(),
            turn_number: self.current_turn,
        });
    }

    /// Record text output.
    pub fn text_output(&self, content: impl Into<String>, is_partial: bool) {
        let _ = self.event_tx.send(ProgressEvent::TextOutput {
            session_id: self.session_id.clone(),
            content: content.into(),
            is_partial,
        });
    }

    /// Record tool call started.
    pub fn tool_call_started(
        &mut self,
        tool_name: impl Into<String>,
        tool_id: impl Into<String>,
        arguments: &serde_json::Value,
    ) {
        let args_preview = serde_json::to_string(arguments)
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect();

        let _ = self.event_tx.send(ProgressEvent::ToolCallStarted {
            session_id: self.session_id.clone(),
            tool_name: tool_name.into(),
            tool_id: tool_id.into(),
            arguments_preview: args_preview,
        });
    }

    /// Record tool call completed.
    pub fn tool_call_completed(
        &mut self,
        tool_name: impl Into<String>,
        tool_id: impl Into<String>,
        success: bool,
        output: &str,
        duration: Duration,
    ) {
        self.total_tool_calls += 1;

        let output_preview: String = output.chars().take(200).collect();

        let _ = self.event_tx.send(ProgressEvent::ToolCallCompleted {
            session_id: self.session_id.clone(),
            tool_name: tool_name.into(),
            tool_id: tool_id.into(),
            success,
            output_preview,
            duration_ms: duration.as_millis() as u64,
        });
    }

    /// Record tool call pending approval.
    pub fn tool_call_pending(
        &self,
        tool_name: impl Into<String>,
        tool_id: impl Into<String>,
        arguments: &serde_json::Value,
        risk_level: impl Into<String>,
    ) {
        let _ = self.event_tx.send(ProgressEvent::ToolCallPending {
            session_id: self.session_id.clone(),
            tool_name: tool_name.into(),
            tool_id: tool_id.into(),
            arguments: serde_json::to_string(arguments).unwrap_or_default(),
            risk_level: risk_level.into(),
        });
    }

    /// Record file modification.
    pub fn file_modified(&mut self, path: impl Into<String>, operation: impl Into<String>) {
        let path = path.into();
        if !self.files_modified.contains(&path) {
            self.files_modified.push(path.clone());
        }

        let _ = self.event_tx.send(ProgressEvent::FileModified {
            session_id: self.session_id.clone(),
            path,
            operation: operation.into(),
        });
    }

    /// Record turn completed.
    pub fn turn_completed(&mut self, tool_calls_count: u32, tokens: u64) {
        self.total_tokens += tokens;

        let _ = self.event_tx.send(ProgressEvent::TurnCompleted {
            session_id: self.session_id.clone(),
            turn_number: self.current_turn,
            tool_calls_count,
            tokens_used: tokens,
        });
    }

    /// Record completion.
    pub fn complete(&mut self, output: impl Into<String>) {
        self.status = SubagentStatus::Completed;

        let _ = self.event_tx.send(ProgressEvent::Completed {
            session_id: self.session_id.clone(),
            output: output.into(),
            total_turns: self.current_turn,
            total_tool_calls: self.total_tool_calls,
            total_tokens: self.total_tokens,
            duration_ms: self.elapsed().as_millis() as u64,
        });
    }

    /// Record failure.
    pub fn fail(&mut self, error: impl Into<String>, recoverable: bool) {
        self.status = SubagentStatus::Failed;

        let _ = self.event_tx.send(ProgressEvent::Failed {
            session_id: self.session_id.clone(),
            error: error.into(),
            recoverable,
        });
    }

    /// Record cancellation.
    pub fn cancel(&mut self, reason: impl Into<String>) {
        self.status = SubagentStatus::Cancelled;

        let _ = self.event_tx.send(ProgressEvent::Cancelled {
            session_id: self.session_id.clone(),
            reason: reason.into(),
        });
    }

    /// Send info message.
    pub fn info(&self, message: impl Into<String>) {
        let _ = self.event_tx.send(ProgressEvent::Info {
            session_id: self.session_id.clone(),
            message: message.into(),
        });
    }

    /// Send warning message.
    pub fn warning(&self, message: impl Into<String>) {
        let _ = self.event_tx.send(ProgressEvent::Warning {
            session_id: self.session_id.clone(),
            message: message.into(),
        });
    }

    /// Update todo list (from TodoWrite tool).
    pub fn update_todos(&self, todos: Vec<(String, String)>) {
        let _ = self.event_tx.send(ProgressEvent::TodoUpdated {
            session_id: self.session_id.clone(),
            todos,
        });
    }

    /// Get summary statistics.
    pub fn stats(&self) -> ProgressStats {
        ProgressStats {
            session_id: self.session_id.clone(),
            agent_type: self.agent_type.clone(),
            status: self.status,
            turns: self.current_turn,
            tool_calls: self.total_tool_calls,
            tokens: self.total_tokens,
            duration: self.elapsed(),
            files_modified: self.files_modified.len(),
        }
    }
}

/// Summary statistics for a subagent execution.
#[derive(Debug, Clone)]
pub struct ProgressStats {
    /// Session ID.
    pub session_id: String,
    /// Agent type.
    pub agent_type: SubagentType,
    /// Current status.
    pub status: SubagentStatus,
    /// Number of turns.
    pub turns: u32,
    /// Number of tool calls.
    pub tool_calls: u32,
    /// Tokens used.
    pub tokens: u64,
    /// Total duration.
    pub duration: Duration,
    /// Files modified count.
    pub files_modified: usize,
}

impl std::fmt::Display for ProgressStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}) - {} turns, {} tool calls, {} tokens, {}ms, {} files modified",
            self.session_id,
            self.agent_type,
            self.turns,
            self.tool_calls,
            self.tokens,
            self.duration.as_millis(),
            self.files_modified
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_event_session_id() {
        let event = ProgressEvent::Started {
            session_id: "test-123".to_string(),
            agent_type: "code".to_string(),
            description: "Test task".to_string(),
        };
        assert_eq!(event.session_id(), "test-123");
    }

    #[test]
    fn test_progress_event_is_terminal() {
        let completed = ProgressEvent::Completed {
            session_id: "test".to_string(),
            output: "Done".to_string(),
            total_turns: 1,
            total_tool_calls: 2,
            total_tokens: 100,
            duration_ms: 1000,
        };
        assert!(completed.is_terminal());

        let thinking = ProgressEvent::Thinking {
            session_id: "test".to_string(),
            turn_number: 1,
        };
        assert!(!thinking.is_terminal());
    }

    #[tokio::test]
    async fn test_subagent_progress() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut progress = SubagentProgress::new("session-1", SubagentType::Code, "Test", tx);

        progress.set_status(SubagentStatus::Running);
        progress.start_thinking();
        progress.tool_call_completed(
            "Read",
            "call-1",
            true,
            "file content",
            Duration::from_millis(50),
        );
        progress.complete("Task completed");

        // Verify events were sent
        let mut event_count = 0;
        while let Ok(event) = rx.try_recv() {
            event_count += 1;
            if event.is_terminal() {
                break;
            }
        }
        assert!(event_count >= 4); // Started, StatusChanged, Thinking, ToolCallCompleted, Completed
    }
}
