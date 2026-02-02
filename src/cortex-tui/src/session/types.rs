//! Session data types for storage and serialization.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ============================================================
// SESSION METADATA
// ============================================================

/// Session metadata stored in `meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Unique session identifier.
    pub id: String,

    /// Session title (auto-generated or user-defined).
    #[serde(default)]
    pub title: Option<String>,

    /// Provider used for this session.
    pub provider: String,

    /// Model used for this session.
    pub model: String,

    /// Working directory when session was created.
    pub cwd: String,

    /// Creation timestamp (ISO 8601).
    pub created_at: DateTime<Utc>,

    /// Last update timestamp (ISO 8601).
    pub updated_at: DateTime<Utc>,

    /// Number of messages in the session.
    #[serde(default)]
    pub message_count: u32,

    /// Total tokens used.
    #[serde(default)]
    pub total_input_tokens: i64,

    /// Total output tokens used.
    #[serde(default)]
    pub total_output_tokens: i64,

    /// Whether this session is archived.
    #[serde(default)]
    pub archived: bool,

    /// Parent session ID if this was forked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,

    /// Git branch at session creation (if in a git repo).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
}

impl SessionMeta {
    /// Creates a new session metadata.
    pub fn new(provider: &str, model: &str) -> Self {
        let now = Utc::now();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string());

        Self {
            id: Uuid::new_v4().to_string(),
            title: None,
            provider: provider.to_string(),
            model: model.to_string(),
            cwd,
            created_at: now,
            updated_at: now,
            message_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            archived: false,
            forked_from: None,
            git_branch: Self::get_git_branch(),
        }
    }

    /// Gets the current git branch if in a git repository.
    fn get_git_branch() -> Option<String> {
        std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .ok()
            .and_then(|output| {
                if output.status.success() {
                    String::from_utf8(output.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                } else {
                    None
                }
            })
    }

    /// Updates the timestamp and message count.
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    /// Adds token usage.
    pub fn add_tokens(&mut self, input: i64, output: i64) {
        self.total_input_tokens += input;
        self.total_output_tokens += output;
    }

    /// Increments the message count.
    pub fn increment_messages(&mut self) {
        self.message_count += 1;
        self.touch();
    }

    /// Gets total tokens used.
    pub fn total_tokens(&self) -> i64 {
        self.total_input_tokens + self.total_output_tokens
    }

    /// Gets a display title (title or truncated first message).
    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .unwrap_or_else(|| format!("Session {}", &self.id[..8]))
    }

    /// Gets the short ID (first 8 characters).
    pub fn short_id(&self) -> &str {
        &self.id[..8.min(self.id.len())]
    }
}

// ============================================================
// STORED MESSAGE
// ============================================================

/// A message stored in the session history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    /// Unique message identifier.
    pub id: String,

    /// Message role: "user", "assistant", "system", or "tool".
    pub role: String,

    /// Message content.
    pub content: String,

    /// Timestamp (ISO 8601).
    pub timestamp: DateTime<Utc>,

    /// Token usage for this message (assistant messages only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenUsageInfo>,

    /// Tool calls made by this message (for assistant messages).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<StoredToolCall>,

    /// Tool call ID this message is responding to (for tool result messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    /// Whether this message includes reasoning/thinking.
    #[serde(default)]
    pub has_reasoning: bool,

    /// Reasoning content (if separate from main content).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

impl StoredMessage {
    /// Creates a new user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: "user".to_string(),
            content: content.into(),
            timestamp: Utc::now(),
            tokens: None,
            tool_calls: vec![],
            tool_call_id: None,
            has_reasoning: false,
            reasoning: None,
        }
    }

    /// Creates a new assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: "assistant".to_string(),
            content: content.into(),
            timestamp: Utc::now(),
            tokens: None,
            tool_calls: vec![],
            tool_call_id: None,
            has_reasoning: false,
            reasoning: None,
        }
    }

    /// Creates a new system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: "system".to_string(),
            content: content.into(),
            timestamp: Utc::now(),
            tokens: None,
            tool_calls: vec![],
            tool_call_id: None,
            has_reasoning: false,
            reasoning: None,
        }
    }

    /// Creates a new tool result message.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: "tool".to_string(),
            content: content.into(),
            timestamp: Utc::now(),
            tokens: None,
            tool_calls: vec![],
            tool_call_id: Some(tool_call_id.into()),
            has_reasoning: false,
            reasoning: None,
        }
    }

    /// Sets token usage.
    pub fn with_tokens(mut self, input: i64, output: i64) -> Self {
        self.tokens = Some(TokenUsageInfo {
            input_tokens: input,
            output_tokens: output,
        });
        self
    }

    /// Adds a tool call.
    pub fn with_tool_call(mut self, tool_call: StoredToolCall) -> Self {
        self.tool_calls.push(tool_call);
        self
    }

    /// Sets reasoning content.
    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.has_reasoning = true;
        self.reasoning = Some(reasoning.into());
        self
    }

    /// Returns true if this is a user message.
    pub fn is_user(&self) -> bool {
        self.role == "user"
    }

    /// Returns true if this is an assistant message.
    pub fn is_assistant(&self) -> bool {
        self.role == "assistant"
    }

    /// Gets the short ID.
    pub fn short_id(&self) -> &str {
        &self.id[..8.min(self.id.len())]
    }
}

// ============================================================
// TOKEN USAGE
// ============================================================

/// Token usage information.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct TokenUsageInfo {
    /// Input tokens used.
    pub input_tokens: i64,
    /// Output tokens generated.
    pub output_tokens: i64,
}

impl TokenUsageInfo {
    /// Creates new token usage info.
    pub fn new(input: i64, output: i64) -> Self {
        Self {
            input_tokens: input,
            output_tokens: output,
        }
    }

    /// Gets total tokens.
    pub fn total(&self) -> i64 {
        self.input_tokens + self.output_tokens
    }
}

// ============================================================
// TOOL CALL
// ============================================================

/// A tool call stored in a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToolCall {
    /// Tool call ID.
    pub id: String,

    /// Tool name.
    pub name: String,

    /// Tool input arguments (JSON).
    pub input: serde_json::Value,

    /// Tool output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,

    /// Whether the tool call succeeded.
    pub success: bool,

    /// Duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl StoredToolCall {
    /// Creates a new tool call.
    pub fn new(id: impl Into<String>, name: impl Into<String>, input: serde_json::Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            input,
            output: None,
            success: false,
            duration_ms: None,
        }
    }

    /// Sets the output.
    pub fn with_output(mut self, output: impl Into<String>, success: bool) -> Self {
        self.output = Some(output.into());
        self.success = success;
        self
    }

    /// Sets the duration.
    pub fn with_duration(mut self, ms: u64) -> Self {
        self.duration_ms = Some(ms);
        self
    }
}

// ============================================================
// SESSION SUMMARY
// ============================================================

/// A summary of a session for listing.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// Session ID.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Model used.
    pub model: String,
    /// Provider used.
    pub provider: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Message count.
    pub message_count: u32,
    /// Whether archived.
    pub archived: bool,
}

impl From<&SessionMeta> for SessionSummary {
    fn from(meta: &SessionMeta) -> Self {
        Self {
            id: meta.id.clone(),
            title: meta.display_title(),
            model: meta.model.clone(),
            provider: meta.provider.clone(),
            created_at: meta.created_at,
            updated_at: meta.updated_at,
            message_count: meta.message_count,
            archived: meta.archived,
        }
    }
}

impl SessionSummary {
    /// Gets the short ID.
    pub fn short_id(&self) -> &str {
        &self.id[..8.min(self.id.len())]
    }

    /// Formats the time for display.
    pub fn format_time(&self) -> String {
        let now = Utc::now();
        let diff = now.signed_duration_since(self.updated_at);

        if diff.num_minutes() < 1 {
            "just now".to_string()
        } else if diff.num_hours() < 1 {
            format!("{}m ago", diff.num_minutes())
        } else if diff.num_days() < 1 {
            format!("{}h ago", diff.num_hours())
        } else if diff.num_days() < 7 {
            format!("{}d ago", diff.num_days())
        } else {
            self.updated_at.format("%b %d").to_string()
        }
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_meta_new() {
        let meta = SessionMeta::new("cortex", "anthropic/claude-opus-4-20250514");
        assert!(!meta.id.is_empty());
        assert_eq!(meta.provider, "cortex");
        assert!(meta.model.contains("claude-opus"));
        assert!(!meta.archived);
    }

    #[test]
    fn test_stored_message_user() {
        let msg = StoredMessage::user("Hello!");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "Hello!");
        assert!(msg.is_user());
        assert!(!msg.is_assistant());
    }

    #[test]
    fn test_stored_message_assistant() {
        let msg = StoredMessage::assistant("Hi there!").with_tokens(100, 50);
        assert_eq!(msg.role, "assistant");
        assert!(msg.tokens.is_some());
        assert_eq!(msg.tokens.unwrap().total(), 150);
    }

    #[test]
    fn test_token_usage() {
        let usage = TokenUsageInfo::new(1000, 500);
        assert_eq!(usage.total(), 1500);
    }

    #[test]
    fn test_session_summary_format_time() {
        let mut meta = SessionMeta::new("test", "test-model");
        meta.updated_at = Utc::now();
        let summary = SessionSummary::from(&meta);
        assert_eq!(summary.format_time(), "just now");
    }
}
