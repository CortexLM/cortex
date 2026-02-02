//! Stream state types and transitions.
//!
//! This module defines the [`StreamState`] enum that represents all possible
//! states in the streaming lifecycle.

use std::time::{Duration, Instant};

/// The current state of response streaming.
///
/// This enum represents all possible states in the streaming lifecycle,
/// from initial idle state through processing, streaming, and completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamState {
    /// No active streaming - the controller is idle and ready.
    Idle,

    /// Waiting for first token after user input.
    ///
    /// This state indicates that a request has been sent and we're
    /// waiting for the AI to begin responding.
    Processing,

    /// Actively receiving text tokens from the AI.
    ///
    /// Contains metadata about the current streaming session.
    Streaming {
        /// Number of tokens received so far.
        tokens_received: u32,
        /// When streaming started.
        started_at: Instant,
    },

    /// AI is in reasoning/thinking mode.
    ///
    /// Some models (like Claude with extended thinking) have a distinct
    /// reasoning phase before generating the response.
    Reasoning {
        /// When reasoning started.
        started_at: Instant,
    },

    /// A tool is being executed.
    ///
    /// The AI has requested a tool call and we're waiting for it to complete.
    ExecutingTool {
        /// Name of the tool being executed.
        tool_name: String,
        /// When tool execution started.
        started_at: Instant,
    },

    /// Waiting for user approval before executing a tool.
    ///
    /// Some tools require explicit user consent before execution.
    WaitingApproval {
        /// Name of the tool awaiting approval.
        tool_name: String,
    },

    /// Finishing up after receiving final message.
    ///
    /// This state handles cleanup and flushing any remaining buffered content.
    Finishing,

    /// Streaming completed successfully.
    ///
    /// Contains final metrics about the completed stream.
    Complete {
        /// Total number of tokens received.
        total_tokens: u32,
        /// Total duration of the streaming session.
        duration: Duration,
    },

    /// Stream was interrupted by user (e.g., Ctrl+C).
    Interrupted,

    /// An error occurred during streaming.
    Error(String),
}

impl StreamState {
    /// Returns `true` if the stream is actively processing or receiving data.
    ///
    /// This includes Processing, Streaming, Reasoning, and ExecutingTool states.
    /// Use this to determine if animations should be running.
    #[inline]
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Processing
                | Self::Streaming { .. }
                | Self::Reasoning { .. }
                | Self::ExecutingTool { .. }
        )
    }

    /// Returns `true` if the stream is in a terminal state.
    ///
    /// Terminal states are Idle, Complete, Interrupted, or Error.
    /// Once in a terminal state, the controller should be reset before reuse.
    #[inline]
    pub fn is_idle(&self) -> bool {
        matches!(
            self,
            Self::Idle | Self::Complete { .. } | Self::Interrupted | Self::Error(_)
        )
    }

    /// Returns `true` if waiting for user approval to execute a tool.
    #[inline]
    pub fn is_waiting_approval(&self) -> bool {
        matches!(self, Self::WaitingApproval { .. })
    }

    /// Returns `true` if currently in the streaming state.
    #[inline]
    pub fn is_streaming(&self) -> bool {
        matches!(self, Self::Streaming { .. })
    }

    /// Returns `true` if in the reasoning state.
    #[inline]
    pub fn is_reasoning(&self) -> bool {
        matches!(self, Self::Reasoning { .. })
    }

    /// Returns `true` if a tool is currently executing.
    #[inline]
    pub fn is_executing_tool(&self) -> bool {
        matches!(self, Self::ExecutingTool { .. })
    }

    /// Returns `true` if an error occurred.
    #[inline]
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }

    /// Returns the tool name if in a tool-related state.
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            Self::ExecutingTool { tool_name, .. } | Self::WaitingApproval { tool_name } => {
                Some(tool_name)
            }
            _ => None,
        }
    }

    /// Returns the error message if in error state.
    pub fn error_message(&self) -> Option<&str> {
        match self {
            Self::Error(msg) => Some(msg),
            _ => None,
        }
    }
}
