//! Prompt types for MCP protocol.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::content::Content;

/// MCP prompt definition.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    /// Unique name for the prompt.
    pub name: String,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Prompt arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
}

impl Prompt {
    /// Create a new prompt.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            arguments: None,
        }
    }

    /// Add a description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add arguments.
    pub fn with_arguments(mut self, arguments: Vec<PromptArgument>) -> Self {
        self.arguments = Some(arguments);
        self
    }

    /// Add a single argument.
    pub fn argument(mut self, arg: PromptArgument) -> Self {
        self.arguments.get_or_insert_with(Vec::new).push(arg);
        self
    }
}

/// Prompt argument definition.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PromptArgument {
    /// Argument name.
    pub name: String,
    /// Argument description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the argument is required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

impl PromptArgument {
    /// Create a new argument.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            required: None,
        }
    }

    /// Create a required argument.
    pub fn required(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            required: Some(true),
        }
    }

    /// Add a description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Mark as required.
    pub fn set_required(mut self, required: bool) -> Self {
        self.required = Some(required);
        self
    }
}

/// List prompts request parameters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListPromptsParams {
    /// Pagination cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// List prompts result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListPromptsResult {
    /// Available prompts.
    pub prompts: Vec<Prompt>,
    /// Next page cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl ListPromptsResult {
    /// Create a new result.
    pub fn new(prompts: Vec<Prompt>) -> Self {
        Self {
            prompts,
            next_cursor: None,
        }
    }
}

/// Get prompt request parameters.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetPromptParams {
    /// Prompt name.
    pub name: String,
    /// Prompt arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<HashMap<String, String>>,
}

impl GetPromptParams {
    /// Create new params.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arguments: None,
        }
    }

    /// Add arguments.
    pub fn with_arguments(mut self, args: HashMap<String, String>) -> Self {
        self.arguments = Some(args);
        self
    }
}

/// Get prompt result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetPromptResult {
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Prompt messages.
    pub messages: Vec<PromptMessage>,
}

impl GetPromptResult {
    /// Create a new result.
    pub fn new(messages: Vec<PromptMessage>) -> Self {
        Self {
            description: None,
            messages,
        }
    }

    /// Add a description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Prompt message.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PromptMessage {
    /// Message role.
    pub role: Role,
    /// Message content.
    pub content: Content,
}

impl PromptMessage {
    /// Create a user message.
    pub fn user(content: Content) -> Self {
        Self {
            role: Role::User,
            content,
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: Content) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }
}

/// Message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// User role.
    User,
    /// Assistant role.
    Assistant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_creation() {
        let prompt = Prompt::new("code_review")
            .with_description("Review code changes")
            .argument(PromptArgument::required("code").with_description("The code to review"));

        assert_eq!(prompt.name, "code_review");
        assert!(prompt.description.is_some());
        assert_eq!(prompt.arguments.as_ref().map(|a| a.len()), Some(1));
    }
}
