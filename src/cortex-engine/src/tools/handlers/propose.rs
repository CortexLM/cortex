//! Propose tool handler.
//!
//! Presents an implementation plan for user review and approval before execution.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::ToolHandler;
use crate::error::{CortexError, Result};
use crate::tools::context::ToolContext;
use crate::tools::spec::{ToolMetadata, ToolResult};

/// Propose handler for presenting implementation plans.
pub struct ProposeHandler;

impl ProposeHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ProposeHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for ProposeHandler {
    fn name(&self) -> &str {
        "Propose"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let plan = arguments
            .get("plan")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CortexError::InvalidInput("Missing required parameter: plan".to_string())
            })?;

        let title = arguments
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Implementation Plan");

        let options = arguments.get("options").and_then(|v| v.as_array()).cloned();

        let propose_data = json!({
            "type": "propose",
            "title": title,
            "plan": plan,
            "options": options,
            "status": "pending_approval"
        });

        let mut output = format!("# {}\n\n", title);
        output.push_str(&format!("{}\n", plan));

        if let Some(opts) = &options {
            if !opts.is_empty() {
                output.push_str("\n## Options\n\n");
                for (i, opt) in opts.iter().enumerate() {
                    if let Some(opt_str) = opt.as_str() {
                        output.push_str(&format!("{}. {}\n", i + 1, opt_str));
                    }
                }
            }
        }

        output.push_str("\nPlease review and approve to proceed.\n");

        Ok(ToolResult::success(output).with_metadata(ToolMetadata {
            duration_ms: 0,
            exit_code: None,
            files_modified: vec![],
            data: Some(propose_data),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_propose_with_plan_only() {
        let handler = ProposeHandler::new();
        let context = ToolContext::new(PathBuf::from("."));

        let args = json!({
            "plan": "This is a test plan with implementation details."
        });

        let result = handler.execute(args, &context).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        assert!(tool_result.success);
        assert!(tool_result.output.contains("Implementation Plan"));
        assert!(tool_result.output.contains("This is a test plan"));
        assert!(tool_result.output.contains("Please review and approve"));

        let metadata = tool_result.metadata.unwrap();
        let data = metadata.data.unwrap();
        assert_eq!(data["type"], "propose");
        assert_eq!(data["status"], "pending_approval");
    }

    #[tokio::test]
    async fn test_propose_with_title() {
        let handler = ProposeHandler::new();
        let context = ToolContext::new(PathBuf::from("."));

        let args = json!({
            "plan": "Test plan content",
            "title": "Custom Title"
        });

        let result = handler.execute(args, &context).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        assert!(tool_result.output.contains("# Custom Title"));
    }

    #[tokio::test]
    async fn test_propose_with_options() {
        let handler = ProposeHandler::new();
        let context = ToolContext::new(PathBuf::from("."));

        let args = json!({
            "plan": "Test plan with multiple approaches",
            "options": [
                "Option A: Quick fix with minimal changes",
                "Option B: Full refactor for long-term maintainability"
            ]
        });

        let result = handler.execute(args, &context).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        assert!(tool_result.output.contains("## Options"));
        assert!(tool_result.output.contains("1. Option A: Quick fix"));
        assert!(tool_result.output.contains("2. Option B: Full refactor"));

        let metadata = tool_result.metadata.unwrap();
        let data = metadata.data.unwrap();
        let opts = data["options"].as_array().unwrap();
        assert_eq!(opts.len(), 2);
    }

    #[tokio::test]
    async fn test_propose_missing_plan_error() {
        let handler = ProposeHandler::new();
        let context = ToolContext::new(PathBuf::from("."));

        let args = json!({
            "title": "Missing Plan"
        });

        let result = handler.execute(args, &context).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, CortexError::InvalidInput(_)));
        assert!(err.to_string().contains("Missing required parameter: plan"));
    }

    #[tokio::test]
    async fn test_propose_empty_options_array() {
        let handler = ProposeHandler::new();
        let context = ToolContext::new(PathBuf::from("."));

        let args = json!({
            "plan": "Test plan",
            "options": []
        });

        let result = handler.execute(args, &context).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        assert!(!tool_result.output.contains("## Options"));
    }

    #[tokio::test]
    async fn test_propose_markdown_in_plan() {
        let handler = ProposeHandler::new();
        let context = ToolContext::new(PathBuf::from("."));

        let args = json!({
            "plan": "## Step 1\n\nImplement feature X\n\n## Step 2\n\nTest feature X"
        });

        let result = handler.execute(args, &context).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        assert!(tool_result.output.contains("## Step 1"));
        assert!(tool_result.output.contains("## Step 2"));
    }
}
