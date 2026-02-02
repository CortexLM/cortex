//! Hooks based on LLM prompts for contextual decisions.
//!
//! This module provides hooks that can evaluate decisions using an LLM
//! instead of executing shell commands. Useful for:
//! - Safety checks before executing commands
//! - Permission decisions
//! - Content filtering
//! - Dynamic behavior based on context

use crate::{HookContext, HookType};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A hook definition that uses an LLM prompt for decision making.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptHook {
    /// Hook identifier.
    pub id: String,
    /// Hook type.
    pub hook_type: HookType,
    /// The prompt template to send to the LLM.
    pub prompt: String,
    /// Optional model override (defaults to a fast model like haiku).
    #[serde(default)]
    pub model: Option<String>,
    /// Whether the hook is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl PromptHook {
    /// Create a new prompt hook.
    pub fn new(id: impl Into<String>, hook_type: HookType, prompt: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            hook_type,
            prompt: prompt.into(),
            model: None,
            enabled: true,
        }
    }

    /// Set the model to use for evaluation.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Build the complete prompt with context variables substituted.
    ///
    /// Variables like $TOOL_NAME, $TOOL_ARGS, $FILE_PATH, etc. are replaced
    /// with their values from the context.
    pub fn build_prompt(&self, context: &HookContext) -> String {
        let mut prompt = self.prompt.clone();

        // Substitute variables from context
        for (key, value) in context.as_env() {
            prompt = prompt.replace(&format!("${}", key), &value);
        }

        // Add response format instruction
        format!(
            "{}\n\n\
            Respond with a JSON object in this exact format:\n\
            {{\n  \
                \"decision\": \"allow\" | \"deny\" | \"modify\" | \"continue\",\n  \
                \"reason\": \"your explanation here\"\n\
            }}",
            prompt
        )
    }
}

/// Response from a prompt hook evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptHookResponse {
    /// The decision made by the LLM.
    pub decision: PromptDecision,
    /// Explanation for the decision.
    pub reason: String,
    /// Optional modified arguments (for "modify" decisions).
    #[serde(default)]
    pub modified_args: Option<serde_json::Value>,
}

/// Decision types for prompt hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptDecision {
    /// Allow the action to proceed.
    Allow,
    /// Deny/block the action.
    Deny,
    /// Allow with modifications.
    Modify,
    /// Continue with no specific decision.
    Continue,
}

impl PromptDecision {
    /// Check if this decision allows the action.
    pub fn is_allowed(&self) -> bool {
        matches!(
            self,
            PromptDecision::Allow | PromptDecision::Continue | PromptDecision::Modify
        )
    }

    /// Check if this decision blocks the action.
    pub fn is_denied(&self) -> bool {
        matches!(self, PromptDecision::Deny)
    }
}

/// Error type for prompt hook execution.
#[derive(Debug, thiserror::Error)]
pub enum PromptHookError {
    /// Failed to call the LLM.
    #[error("LLM error: {0}")]
    LlmError(String),
    /// Failed to parse the LLM response.
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
    /// Hook is disabled.
    #[error("Hook is disabled")]
    Disabled,
}

/// Trait for LLM clients that can be used with prompt hooks.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send a prompt to the LLM and get a response.
    async fn complete(&self, model: &str, prompt: &str) -> Result<String, String>;
}

/// Executor for prompt-based hooks.
pub struct PromptHookExecutor {
    /// LLM client for evaluations.
    llm_client: Arc<dyn LlmClient>,
    /// Default model for evaluations (should be a fast model).
    default_model: String,
}

impl PromptHookExecutor {
    /// Create a new prompt hook executor.
    pub fn new(llm_client: Arc<dyn LlmClient>) -> Self {
        Self {
            llm_client,
            default_model: "claude-3-haiku-20240307".to_string(),
        }
    }

    /// Set the default model for evaluations.
    pub fn with_default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }

    /// Execute a prompt hook and get the decision.
    pub async fn execute(
        &self,
        hook: &PromptHook,
        context: &HookContext,
    ) -> Result<PromptHookResponse, PromptHookError> {
        if !hook.enabled {
            return Err(PromptHookError::Disabled);
        }

        let prompt = hook.build_prompt(context);
        let model = hook.model.as_ref().unwrap_or(&self.default_model);

        let response = self
            .llm_client
            .complete(model, &prompt)
            .await
            .map_err(PromptHookError::LlmError)?;

        // Try to parse the JSON response
        self.parse_response(&response)
    }

    /// Parse the LLM response into a structured response.
    fn parse_response(&self, response: &str) -> Result<PromptHookResponse, PromptHookError> {
        // Try to find JSON in the response
        let json_start = response.find('{');
        let json_end = response.rfind('}');

        if let (Some(start), Some(end)) = (json_start, json_end) {
            let json_str = &response[start..=end];
            serde_json::from_str(json_str)
                .map_err(|e| PromptHookError::InvalidResponse(format!("JSON parse error: {}", e)))
        } else {
            // If no JSON found, try to infer from keywords
            let lower = response.to_lowercase();
            let decision = if lower.contains("deny")
                || lower.contains("block")
                || lower.contains("unsafe")
            {
                PromptDecision::Deny
            } else if lower.contains("allow") || lower.contains("safe") || lower.contains("approve")
            {
                PromptDecision::Allow
            } else {
                PromptDecision::Continue
            };

            Ok(PromptHookResponse {
                decision,
                reason: response.to_string(),
                modified_args: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_hook_build() {
        let hook = PromptHook::new(
            "safety-check",
            HookType::PreToolUse,
            "Is the tool $TOOL_NAME with args $TOOL_ARGS safe to execute?",
        );

        let context = HookContext::new().with_tool("Execute", "rm -rf /tmp/test");

        let prompt = hook.build_prompt(&context);
        assert!(prompt.contains("Execute"));
        assert!(prompt.contains("rm -rf /tmp/test"));
        assert!(prompt.contains("decision"));
    }

    #[test]
    fn test_prompt_decision_is_allowed() {
        assert!(PromptDecision::Allow.is_allowed());
        assert!(PromptDecision::Continue.is_allowed());
        assert!(PromptDecision::Modify.is_allowed());
        assert!(!PromptDecision::Deny.is_allowed());
    }

    #[test]
    fn test_prompt_decision_is_denied() {
        assert!(PromptDecision::Deny.is_denied());
        assert!(!PromptDecision::Allow.is_denied());
    }

    // Mock LLM client for testing
    struct MockLlmClient {
        response: String,
    }

    #[async_trait]
    impl LlmClient for MockLlmClient {
        async fn complete(&self, _model: &str, _prompt: &str) -> Result<String, String> {
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn test_prompt_hook_executor() {
        let mock_client = Arc::new(MockLlmClient {
            response: r#"{"decision": "allow", "reason": "Command is safe"}"#.to_string(),
        });

        let executor = PromptHookExecutor::new(mock_client);
        let hook = PromptHook::new("test", HookType::PreToolUse, "Is this safe?");

        let context = HookContext::new();
        let result = executor.execute(&hook, &context).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.decision, PromptDecision::Allow);
        assert_eq!(response.reason, "Command is safe");
    }

    #[tokio::test]
    async fn test_prompt_hook_executor_deny() {
        let mock_client = Arc::new(MockLlmClient {
            response: r#"{"decision": "deny", "reason": "Command is dangerous"}"#.to_string(),
        });

        let executor = PromptHookExecutor::new(mock_client);
        let hook = PromptHook::new("test", HookType::PreToolUse, "Is this safe?");

        let context = HookContext::new();
        let result = executor.execute(&hook, &context).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.decision, PromptDecision::Deny);
    }

    #[tokio::test]
    async fn test_prompt_hook_executor_infer_from_keywords() {
        let mock_client = Arc::new(MockLlmClient {
            response: "This command looks unsafe and should be blocked.".to_string(),
        });

        let executor = PromptHookExecutor::new(mock_client);
        let hook = PromptHook::new("test", HookType::PreToolUse, "Is this safe?");

        let context = HookContext::new();
        let result = executor.execute(&hook, &context).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.decision, PromptDecision::Deny);
    }
}
