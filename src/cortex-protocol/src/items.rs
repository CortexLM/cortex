//! Turn item types for unified event handling.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::user_input::UserInput;

/// Items that can occur during a turn.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnItem {
    /// User message.
    UserMessage(UserMessageItem),

    /// Agent/assistant message.
    AgentMessage(AgentMessageItem),

    /// Tool call.
    ToolCall(ToolCallItem),

    /// Tool result.
    ToolResult(ToolResultItem),

    /// Web search.
    WebSearch(WebSearchItem),

    /// Reasoning/thinking.
    Reasoning(ReasoningItem),
}

/// User message item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UserMessageItem {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub content: Vec<UserInput>,
}

impl UserMessageItem {
    pub fn new(content: &[UserInput]) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            content: content.to_vec(),
        }
    }

    pub fn with_parent(content: &[UserInput], parent_id: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: Some(parent_id.into()),
            content: content.to_vec(),
        }
    }
}

/// Agent message item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentMessageItem {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub content: String,
}

impl AgentMessageItem {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            content: content.into(),
        }
    }

    pub fn with_parent(content: impl Into<String>, parent_id: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: Some(parent_id.into()),
            content: content.into(),
        }
    }
}

/// Tool call item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolCallItem {
    pub id: String,
    pub call_id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Tool result item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolResultItem {
    pub id: String,
    pub call_id: String,
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
}

/// Web search item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchItem {
    pub id: String,
    pub query: String,
}

/// Reasoning item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReasoningItem {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub is_summary: bool,
}

impl TurnItem {
    /// Get the ID of this item.
    pub fn id(&self) -> &str {
        match self {
            Self::UserMessage(item) => &item.id,
            Self::AgentMessage(item) => &item.id,
            Self::ToolCall(item) => &item.id,
            Self::ToolResult(item) => &item.id,
            Self::WebSearch(item) => &item.id,
            Self::Reasoning(item) => &item.id,
        }
    }

    /// Check if this is a user message.
    pub fn is_user_message(&self) -> bool {
        matches!(self, Self::UserMessage(_))
    }

    /// Check if this is an agent message.
    pub fn is_agent_message(&self) -> bool {
        matches!(self, Self::AgentMessage(_))
    }

    /// Check if this is a tool call.
    pub fn is_tool_call(&self) -> bool {
        matches!(self, Self::ToolCall(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_turn_item_id() {
        let item = TurnItem::AgentMessage(AgentMessageItem::new("Hello"));
        assert!(!item.id().is_empty());
        assert!(item.is_agent_message());
    }

    #[test]
    fn test_user_message_item() {
        let inputs = vec![UserInput::text("Test message")];
        let item = UserMessageItem::new(&inputs);
        assert_eq!(item.content.len(), 1);
    }
}
