//! Plan tool handler.
//!
//! Presents comprehensive implementation plans with multi-agent analysis for user approval.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ToolHandler;
use crate::error::Result;
use crate::tools::context::ToolContext;
use crate::tools::spec::{ToolMetadata, ToolResult};

/// Plan task item with detailed analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_status")]
    pub status: PlanTaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtasks: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
}

fn default_status() -> PlanTaskStatus {
    PlanTaskStatus::Pending
}

/// Plan task status.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanTaskStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
}

/// Agent analysis report.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct AgentAnalysis {
    pub agent: String,
    pub role: String,
    pub findings: Vec<String>,
    pub recommendations: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority_items: Option<Vec<String>>,
}

/// Plan handler for presenting implementation plans.
pub struct PlanHandler;

impl PlanHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PlanHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for PlanHandler {
    fn name(&self) -> &str {
        "Plan"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        // Extract todos parameter (required by spec)
        let todos_str = arguments
            .get("todos")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Validate and parse todos
        let todos_lines: Vec<&str> = todos_str.lines().collect();

        // Validate: max 50 items
        if todos_lines.len() > 50 {
            return Err(crate::error::CortexError::ToolExecution {
                tool: "Plan".to_string(),
                message: format!("Too many items: {} (max 50)", todos_lines.len()),
            });
        }

        // Validate: each item max 500 chars
        for (idx, line) in todos_lines.iter().enumerate() {
            if line.len() > 500 {
                return Err(crate::error::CortexError::ToolExecution {
                    tool: "Plan".to_string(),
                    message: format!("Item {} exceeds 500 chars: {} chars", idx + 1, line.len()),
                });
            }
        }

        // Parse todos into structured format
        let mut parsed_todos = Vec::new();
        for line in todos_lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            parsed_todos.push(trimmed.to_string());
        }

        // Create plan data structure aligned with spec
        let plan_data = json!({
            "type": "plan",
            "todos": todos_str,
            "item_count": parsed_todos.len(),
            "status": "active"
        });

        // Format output as readable markdown
        let mut output = String::from("# Task Plan\n\n");
        output.push_str("## Tasks\n\n");

        for line in &parsed_todos {
            output.push_str(&format!("{}\n", line));
        }

        Ok(ToolResult::success(output).with_metadata(ToolMetadata {
            duration_ms: 0,
            exit_code: None,
            files_modified: vec![],
            data: Some(plan_data),
        }))
    }
}
