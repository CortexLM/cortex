//! Rich message part types for structured message content.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::conversation_id::ConversationId;

use super::tokens::TokenUsage;

// ============================================================
// Message Parts
// ============================================================

/// Rich message part types supporting various content kinds.
/// Provides comprehensive message representation for structured content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePart {
    /// Plain text content.
    Text {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        synthetic: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        ignored: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },

    /// Reasoning/thinking content from the model.
    Reasoning {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },

    /// Tool invocation and result.
    Tool {
        call_id: String,
        name: String,
        input: serde_json::Value,
        state: ToolState,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },

    /// File attachment or reference.
    File {
        path: PathBuf,
        mime_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<FilePartSource>,
    },

    /// Git snapshot reference.
    Snapshot {
        snapshot_id: String,
        message: String,
    },

    /// Code patch/diff.
    Patch {
        file_path: PathBuf,
        diff: String,
        additions: u32,
        deletions: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
    },

    /// Agent switch indicator.
    Agent {
        agent_id: String,
        agent_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<TextRange>,
    },

    /// Retry attempt information.
    Retry {
        attempt: u32,
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<MessagePartError>,
    },

    /// Context compaction marker.
    Compaction {
        original_tokens: u64,
        compacted_tokens: u64,
        summary: String,
        #[serde(default)]
        auto: bool,
    },

    /// Step start marker for multi-step operations.
    StepStart {
        step_id: String,
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        snapshot: Option<String>,
    },

    /// Step completion with metrics.
    StepFinish {
        step_id: String,
        tokens: TokenUsage,
        #[serde(skip_serializing_if = "Option::is_none")]
        cost: Option<f64>,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        snapshot: Option<String>,
    },

    /// Subtask delegation.
    Subtask {
        task_id: String,
        description: String,
        status: SubtaskStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
}

// ============================================================
// Tool State
// ============================================================

/// Tool execution state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolState {
    /// Tool call parsed but not yet executed.
    Pending {
        #[serde(skip_serializing_if = "Option::is_none")]
        raw: Option<String>,
    },

    /// Tool is currently executing.
    Running {
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },

    /// Tool completed successfully.
    Completed {
        title: String,
        metadata: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<FileAttachment>>,
    },

    /// Tool execution failed.
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },
}

/// Subtask execution status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubtaskStatus {
    /// Not yet started.
    Pending,
    /// Currently executing.
    Running,
    /// Successfully completed.
    Completed,
    /// Execution failed.
    Failed,
    /// Cancelled by user or system.
    Cancelled,
}

// ============================================================
// Timing
// ============================================================

/// Timing information for message parts.
/// Uses Unix timestamps (milliseconds since epoch) for portability.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PartTiming {
    /// When the part started (Unix timestamp in milliseconds).
    pub start: i64,
    /// When the part completed (Unix timestamp in milliseconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<i64>,
    /// When the part was compacted (Unix timestamp in milliseconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compacted: Option<i64>,
}

impl PartTiming {
    /// Create a new timing starting now.
    pub fn now() -> Self {
        Self {
            start: Utc::now().timestamp_millis(),
            end: None,
            compacted: None,
        }
    }

    /// Create from explicit start time.
    pub fn from_start(start: DateTime<Utc>) -> Self {
        Self {
            start: start.timestamp_millis(),
            end: None,
            compacted: None,
        }
    }

    /// Create a completed timing.
    pub fn completed(start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self {
            start: start.timestamp_millis(),
            end: Some(end.timestamp_millis()),
            compacted: None,
        }
    }

    /// Mark as completed now.
    pub fn complete(&mut self) {
        self.end = Some(Utc::now().timestamp_millis());
    }

    /// Mark as compacted now.
    pub fn compact(&mut self) {
        self.compacted = Some(Utc::now().timestamp_millis());
    }

    /// Duration in milliseconds.
    ///
    /// Returns `None` if `end` is not set, or if timestamps are corrupted
    /// (e.g., `end < start` or arithmetic overflow).
    pub fn duration_ms(&self) -> Option<u64> {
        self.end.and_then(|end| {
            end.checked_sub(self.start)
                .and_then(|d| if d >= 0 { Some(d as u64) } else { None })
        })
    }

    /// Get start time as DateTime.
    pub fn start_time(&self) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp_millis(self.start)
    }

    /// Get end time as DateTime.
    pub fn end_time(&self) -> Option<DateTime<Utc>> {
        self.end.and_then(DateTime::from_timestamp_millis)
    }

    /// Get compacted time as DateTime.
    pub fn compacted_time(&self) -> Option<DateTime<Utc>> {
        self.compacted.and_then(DateTime::from_timestamp_millis)
    }
}

// ============================================================
// File & Source Types
// ============================================================

/// File attachment for tool results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileAttachment {
    pub path: PathBuf,
    pub mime_type: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// Source information for file parts.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FilePartSource {
    /// Direct file reference.
    File { path: String, text: TextRange },
    /// Symbol reference (function, class, etc.).
    Symbol {
        path: String,
        name: String,
        kind: i32,
        range: LineRange,
        text: TextRange,
    },
}

/// Text range within content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TextRange {
    pub value: String,
    pub start: i64,
    pub end: i64,
}

/// Line range in a file.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LineRange {
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
}

/// Error information for message parts.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MessagePartError {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default)]
    pub is_retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

// ============================================================
// Message With Parts
// ============================================================

/// A message with its associated parts.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MessageWithParts {
    /// Message ID.
    pub id: String,
    /// Session ID.
    pub session_id: ConversationId,
    /// Message role.
    pub role: MessageRole,
    /// Message parts.
    pub parts: Vec<IndexedPart>,
    /// Creation time (Unix timestamp in milliseconds).
    pub created_at: i64,
    /// Completion time (Unix timestamp in milliseconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    /// Parent message ID (for assistant messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Model used (for assistant messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Provider used (for assistant messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Agent name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Token usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenUsage>,
    /// Cost in USD.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// Error if the message failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<MessagePartError>,
    /// Finish reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// Message role.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

/// A message part with index and timing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IndexedPart {
    /// Part ID.
    pub id: String,
    /// Index within the message.
    pub index: usize,
    /// The part content.
    pub part: MessagePart,
    /// Timing information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<PartTiming>,
}

impl IndexedPart {
    /// Create a new indexed part.
    pub fn new(id: String, index: usize, part: MessagePart) -> Self {
        Self {
            id,
            index,
            part,
            timing: Some(PartTiming::now()),
        }
    }

    /// Create a new indexed part with custom timing.
    pub fn with_timing(id: String, index: usize, part: MessagePart, timing: PartTiming) -> Self {
        Self {
            id,
            index,
            part,
            timing: Some(timing),
        }
    }
}

impl MessageWithParts {
    /// Create a new user message.
    pub fn user(id: String, session_id: ConversationId) -> Self {
        Self {
            id,
            session_id,
            role: MessageRole::User,
            parts: Vec::new(),
            created_at: Utc::now().timestamp_millis(),
            completed_at: None,
            parent_id: None,
            model_id: None,
            provider_id: None,
            agent: None,
            tokens: None,
            cost: None,
            error: None,
            finish_reason: None,
        }
    }

    /// Create a new assistant message.
    pub fn assistant(
        id: String,
        session_id: ConversationId,
        parent_id: String,
        model_id: String,
        provider_id: String,
    ) -> Self {
        Self {
            id,
            session_id,
            role: MessageRole::Assistant,
            parts: Vec::new(),
            created_at: Utc::now().timestamp_millis(),
            completed_at: None,
            parent_id: Some(parent_id),
            model_id: Some(model_id),
            provider_id: Some(provider_id),
            agent: None,
            tokens: None,
            cost: None,
            error: None,
            finish_reason: None,
        }
    }

    /// Add a part to the message.
    pub fn add_part(&mut self, id: String, part: MessagePart) {
        let index = self.parts.len();
        self.parts.push(IndexedPart::new(id, index, part));
    }

    /// Add a text part.
    pub fn add_text(&mut self, id: String, content: String) {
        self.add_part(
            id,
            MessagePart::Text {
                content,
                synthetic: None,
                ignored: None,
                metadata: None,
            },
        );
    }

    /// Add a reasoning part.
    pub fn add_reasoning(&mut self, id: String, content: String) {
        self.add_part(
            id,
            MessagePart::Reasoning {
                content,
                signature: None,
                metadata: None,
            },
        );
    }

    /// Add a tool call part.
    pub fn add_tool_call(
        &mut self,
        id: String,
        call_id: String,
        name: String,
        input: serde_json::Value,
    ) {
        self.add_part(
            id,
            MessagePart::Tool {
                call_id,
                name,
                input,
                state: ToolState::Pending { raw: None },
                output: None,
                error: None,
                metadata: None,
            },
        );
    }

    /// Get a part by index.
    pub fn get_part(&self, index: usize) -> Option<&IndexedPart> {
        self.parts.get(index)
    }

    /// Get a mutable part by index.
    pub fn get_part_mut(&mut self, index: usize) -> Option<&mut IndexedPart> {
        self.parts.get_mut(index)
    }

    /// Find a part by ID.
    pub fn find_part(&self, id: &str) -> Option<&IndexedPart> {
        self.parts.iter().find(|p| p.id == id)
    }

    /// Find a mutable part by ID.
    pub fn find_part_mut(&mut self, id: &str) -> Option<&mut IndexedPart> {
        self.parts.iter_mut().find(|p| p.id == id)
    }

    /// Update a tool state by call ID.
    pub fn update_tool_state(&mut self, call_id: &str, new_state: ToolState) -> bool {
        for part in &mut self.parts {
            if let MessagePart::Tool {
                call_id: cid,
                state,
                ..
            } = &mut part.part
            {
                if cid == call_id {
                    *state = new_state;
                    return true;
                }
            }
        }
        false
    }

    /// Complete a tool call.
    pub fn complete_tool(
        &mut self,
        call_id: &str,
        output: String,
        title: String,
        metadata: serde_json::Value,
    ) -> bool {
        for part in &mut self.parts {
            if let MessagePart::Tool {
                call_id: cid,
                state,
                output: out,
                ..
            } = &mut part.part
            {
                if cid == call_id {
                    *state = ToolState::Completed {
                        title,
                        metadata,
                        attachments: None,
                    };
                    *out = Some(output);
                    if let Some(timing) = &mut part.timing {
                        timing.complete();
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Mark tool call as error.
    pub fn error_tool(&mut self, call_id: &str, error_msg: String) -> bool {
        for part in &mut self.parts {
            if let MessagePart::Tool {
                call_id: cid,
                state,
                error,
                ..
            } = &mut part.part
            {
                if cid == call_id {
                    *state = ToolState::Error {
                        message: error_msg.clone(),
                        metadata: None,
                    };
                    *error = Some(error_msg);
                    if let Some(timing) = &mut part.timing {
                        timing.complete();
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Complete the message.
    pub fn complete(&mut self, tokens: TokenUsage, cost: Option<f64>, finish_reason: String) {
        self.completed_at = Some(Utc::now().timestamp_millis());
        self.tokens = Some(tokens);
        self.cost = cost;
        self.finish_reason = Some(finish_reason);
    }

    /// Get all text content concatenated.
    pub fn get_text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match &p.part {
                MessagePart::Text {
                    content, ignored, ..
                } if !ignored.unwrap_or(false) => Some(content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Check if this message has any tool calls.
    pub fn has_tool_calls(&self) -> bool {
        self.parts
            .iter()
            .any(|p| matches!(p.part, MessagePart::Tool { .. }))
    }

    /// Get all tool parts.
    pub fn get_tool_parts(&self) -> Vec<&IndexedPart> {
        self.parts
            .iter()
            .filter(|p| matches!(p.part, MessagePart::Tool { .. }))
            .collect()
    }
}

// ============================================================
// Part Delta
// ============================================================

/// Delta content for streaming part updates.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PartDelta {
    /// Text content delta.
    Text { content: String },
    /// Reasoning content delta.
    Reasoning { content: String },
    /// Tool output delta.
    ToolOutput { output: String },
}
