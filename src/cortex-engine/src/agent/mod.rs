//! Cortex Agent - Main orchestrator for AI interactions.
//!
//! This module contains the core agent logic that:
//! - Orchestrates conversations with LLMs
//! - Manages tool execution and approval flows
//! - Handles context management and compaction
//! - Coordinates sandbox and security policies
//! - Manages session state and persistence

mod context;
mod core;
mod delegate;
mod executor;
pub mod generator;
mod handler;
mod orchestrator;
mod profile;
mod service;
mod state;
pub mod tools;
mod turn;

pub use context::AgentContext;
pub use core::CortexAgent;
pub use delegate::AgentDelegate;
pub use executor::{ExecutorConfig, ExecutorStats, ToolExecutor, ToolStats};
pub use generator::{AgentGenerator, AgentMode, GENERATE_PROMPT, GeneratedAgent};
pub use handler::MessageHandler;
pub use orchestrator::{
    Orchestrator, ToolCallResult, TurnContext, TurnResult as OrchestratorTurnResult,
};
pub use profile::{AgentProfile, ModelOverrides, ToolPermission};
pub use service::{DoomLoopDetector, ToolCallInfo};
pub use state::{AgentState, TurnState};
pub use turn::{Turn, TurnResult, TurnStatus};

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::tools::spec::ToolResult;

/// Agent configuration options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Model to use.
    pub model: String,
    /// Provider (openai, anthropic, etc).
    pub provider: String,
    /// Maximum tokens for context.
    pub max_context_tokens: u32,
    /// Maximum tokens for output.
    pub max_output_tokens: u32,
    /// Temperature for generation.
    pub temperature: Option<f32>,
    /// Maximum tool iterations per turn.
    pub max_tool_iterations: u32,
    /// Tool execution timeout.
    pub tool_timeout: Duration,
    /// Enable streaming responses.
    pub streaming: bool,
    /// Enable auto-approval for safe commands.
    pub auto_approve_safe: bool,
    /// Sandbox policy.
    pub sandbox_policy: SandboxPolicy,
    /// Working directory.
    pub working_directory: PathBuf,
    /// System prompt.
    pub system_prompt: Option<String>,
    /// Custom instructions.
    pub custom_instructions: Option<String>,
    /// Enable MCP servers.
    pub mcp_enabled: bool,
    /// MCP server configurations.
    pub mcp_servers: Vec<McpServerConfig>,
    /// Enable context compaction.
    pub auto_compact: bool,
    /// Compaction threshold (0.0 - 1.0).
    pub compaction_threshold: f32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            max_context_tokens: 128000,
            max_output_tokens: 16384,
            temperature: None,
            max_tool_iterations: 20,
            tool_timeout: Duration::from_secs(120),
            streaming: true,
            auto_approve_safe: false,
            sandbox_policy: SandboxPolicy::Prompt,
            working_directory: std::env::current_dir().unwrap_or_default(),
            system_prompt: None,
            custom_instructions: None,
            mcp_enabled: true,
            mcp_servers: Vec::new(),
            auto_compact: true,
            compaction_threshold: 0.8,
        }
    }
}

/// Sandbox policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPolicy {
    /// No sandbox - full access.
    None,
    /// Prompt for each action.
    Prompt,
    /// Auto-approve read operations.
    AutoApproveReads,
    /// Full sandbox mode.
    Full,
}

/// MCP server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Server name.
    pub name: String,
    /// Server command.
    pub command: String,
    /// Server arguments.
    pub args: Vec<String>,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Auto-start on agent init.
    pub auto_start: bool,
}

/// Agent event emitted during execution.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Turn started.
    TurnStarted { turn_id: u64, user_message: String },
    /// Model is thinking/generating.
    Thinking,
    /// Text delta from model.
    TextDelta { content: String },
    /// Reasoning/thinking content.
    ReasoningDelta { content: String },
    /// Tool call initiated.
    ToolCallStarted {
        id: String,
        name: String,
        arguments: String,
    },
    /// Tool call completed.
    ToolCallCompleted {
        id: String,
        name: String,
        result: ToolResult,
    },
    /// Tool call requires approval.
    ToolCallPending {
        id: String,
        name: String,
        arguments: String,
        risk_level: RiskLevel,
    },
    /// Tool call was approved.
    ToolCallApproved { id: String },
    /// Tool call was rejected.
    ToolCallRejected { id: String, reason: String },
    /// Turn completed.
    TurnCompleted {
        turn_id: u64,
        response: String,
        token_usage: TokenUsage,
    },
    /// Turn was interrupted.
    TurnInterrupted { turn_id: u64 },
    /// Subagent task spawned.
    TaskSpawned {
        id: String,
        description: String,
        subagent_type: String,
    },
    /// Subagent task progress.
    TaskProgress { id: String, message: String },
    /// Subagent task completed.
    TaskCompleted { id: String },
    /// Context was compacted.
    ContextCompacted {
        messages_removed: usize,
        tokens_saved: u32,
    },
    /// Doom loop detected.
    LoopDetected { tool_name: String, count: usize },
    /// Error occurred.
    Error { message: String, recoverable: bool },
    /// Session saved.
    SessionSaved { path: PathBuf },
    /// Shutdown initiated.
    ShutdownStarted,
    /// Shutdown complete.
    ShutdownComplete,
}

/// Risk level for tool calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Safe - no side effects.
    Safe,
    /// Low risk - reversible side effects.
    Low,
    /// Medium risk - significant but recoverable.
    Medium,
    /// High risk - potentially destructive.
    High,
    /// Critical - requires explicit confirmation.
    Critical,
}

impl RiskLevel {
    /// Check if auto-approval is possible.
    pub fn can_auto_approve(&self, policy: SandboxPolicy) -> bool {
        match policy {
            SandboxPolicy::None => true,
            SandboxPolicy::AutoApproveReads => matches!(self, Self::Safe | Self::Low),
            SandboxPolicy::Prompt | SandboxPolicy::Full => matches!(self, Self::Safe),
        }
    }
}

/// Token usage statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input tokens.
    pub input_tokens: u32,
    /// Output tokens.
    pub output_tokens: u32,
    /// Total tokens.
    pub total_tokens: u32,
    /// Cached tokens.
    pub cached_tokens: u32,
    /// Reasoning tokens (for o1/o3 models).
    pub reasoning_tokens: u32,
}

/// Agent metrics.
#[derive(Debug, Clone, Default)]
pub struct AgentMetrics {
    /// Total turns.
    pub total_turns: u64,
    /// Total tool calls.
    pub total_tool_calls: u64,
    /// Total tokens used.
    pub total_tokens: u64,
    /// Average turn duration.
    pub avg_turn_duration_ms: f64,
    /// Errors encountered.
    pub error_count: u64,
    /// Compactions performed.
    pub compaction_count: u64,
    /// Start time (not serialized).
    pub start_time: Option<Instant>,
}

impl serde::Serialize for AgentMetrics {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("AgentMetrics", 7)?;
        s.serialize_field("total_turns", &self.total_turns)?;
        s.serialize_field("total_tool_calls", &self.total_tool_calls)?;
        s.serialize_field("total_tokens", &self.total_tokens)?;
        s.serialize_field("avg_turn_duration_ms", &self.avg_turn_duration_ms)?;
        s.serialize_field("error_count", &self.error_count)?;
        s.serialize_field("compaction_count", &self.compaction_count)?;
        s.serialize_field(
            "uptime_ms",
            &self.start_time.map(|t| t.elapsed().as_millis()),
        )?;
        s.end()
    }
}

impl AgentMetrics {
    /// Record a turn completion.
    pub fn record_turn(&mut self, duration: Duration, tokens: u32) {
        self.total_turns += 1;
        self.total_tokens += tokens as u64;

        // Update rolling average
        let duration_ms = duration.as_secs_f64() * 1000.0;
        let prev_avg = self.avg_turn_duration_ms;
        let n = self.total_turns as f64;
        self.avg_turn_duration_ms = prev_avg + (duration_ms - prev_avg) / n;
    }

    /// Record a tool call.
    pub fn record_tool_call(&mut self) {
        self.total_tool_calls += 1;
    }

    /// Record an error.
    pub fn record_error(&mut self) {
        self.error_count += 1;
    }

    /// Record a compaction.
    pub fn record_compaction(&mut self) {
        self.compaction_count += 1;
    }

    /// Get uptime.
    pub fn uptime(&self) -> Option<Duration> {
        self.start_time.map(|t| t.elapsed())
    }
}

/// Pending approval for a tool call.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    /// Tool call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments.
    pub arguments: serde_json::Value,
    /// Risk level.
    pub risk_level: RiskLevel,
    /// Timestamp.
    pub timestamp: Instant,
    /// Response channel.
    pub response_tx: mpsc::Sender<ApprovalResponse>,
}

/// Approval response.
#[derive(Debug, Clone)]
pub enum ApprovalResponse {
    /// Approved.
    Approve,
    /// Approved with modifications.
    ApproveModified(serde_json::Value),
    /// Rejected.
    Reject(String),
    /// Always approve this tool.
    AlwaysApprove,
    /// Abort the turn.
    Abort,
}

/// Conversation turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    /// Turn ID.
    pub id: u64,
    /// User message.
    pub user_message: String,
    /// User message items (for multi-part).
    pub user_items: Vec<UserInputItem>,
    /// Assistant response.
    pub assistant_response: Option<String>,
    /// Tool calls made.
    pub tool_calls: Vec<ToolCallRecord>,
    /// Token usage.
    pub token_usage: TokenUsage,
    /// Duration.
    pub duration_ms: u64,
    /// Status.
    pub status: TurnStatus,
    /// Timestamp.
    pub timestamp: String,
}

/// User input item.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserInputItem {
    /// Text input.
    Text { content: String },
    /// File attachment.
    File {
        path: String,
        content: Option<String>,
    },
    /// Image attachment.
    Image { url: String },
    /// URL reference.
    Url { url: String },
}

/// Tool call record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    /// Call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments.
    pub arguments: serde_json::Value,
    /// Result.
    pub result: Option<ToolResultRecord>,
    /// Duration.
    pub duration_ms: u64,
    /// Was approved.
    pub approved: bool,
}

/// Tool result record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecord {
    /// Success status.
    pub success: bool,
    /// Output.
    pub output: String,
    /// Error message if failed.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert_eq!(config.model, "gpt-4o");
        assert_eq!(config.max_tool_iterations, 20);
        assert!(config.streaming);
    }

    #[test]
    fn test_risk_level_auto_approve() {
        assert!(RiskLevel::Safe.can_auto_approve(SandboxPolicy::Full));
        assert!(!RiskLevel::High.can_auto_approve(SandboxPolicy::Full));
        assert!(RiskLevel::High.can_auto_approve(SandboxPolicy::None));
    }

    #[test]
    fn test_agent_metrics() {
        let mut metrics = AgentMetrics::default();
        metrics.record_turn(Duration::from_millis(100), 500);
        metrics.record_turn(Duration::from_millis(200), 600);

        assert_eq!(metrics.total_turns, 2);
        assert_eq!(metrics.total_tokens, 1100);
        assert!(metrics.avg_turn_duration_ms > 100.0);
    }
}
