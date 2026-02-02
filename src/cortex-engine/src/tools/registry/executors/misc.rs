//! Miscellaneous tool executors (todo, task, web fetch).

use serde_json::Value;

use crate::agent::tools::WebFetchTool;
use crate::error::Result;
use crate::tools::registry::ToolRegistry;
use crate::tools::spec::{ToolHandler, ToolResult};

impl ToolRegistry {
    pub(crate) async fn execute_fetch_url(&self, args: Value) -> Result<ToolResult> {
        let tool = WebFetchTool::new();
        let context = crate::tools::ToolContext::new(std::env::current_dir().unwrap_or_default());
        tool.execute(args, &context).await
    }

    pub(crate) async fn execute_todo_write(&self, args: Value) -> Result<ToolResult> {
        let todos = args
            .get("todos")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();

        let mut output = String::from("TODO List Updated\n\n");
        for todo in &todos {
            let id = todo.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let content = todo.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let status = todo
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending");
            let priority = todo
                .get("priority")
                .and_then(|v| v.as_str())
                .unwrap_or("medium");

            let status_icon = match status {
                "in_progress" => "[~]",
                "completed" => "[x]",
                _ => "[ ]",
            };
            let priority_icon = match priority {
                "high" => "!!!",
                "low" => "!",
                _ => "!!",
            };

            output.push_str(&format!("{status_icon} {priority_icon} {id}: {content}\n"));
        }

        Ok(ToolResult::success(output))
    }

    pub(crate) async fn execute_todo_read(&self, _args: Value) -> Result<ToolResult> {
        Ok(ToolResult::success("No todos stored (stateless execution)"))
    }

    pub(crate) async fn execute_task(&self, args: Value) -> Result<ToolResult> {
        // Use the SimpleTaskHandler for registry-based execution
        // Full subagent execution happens through the orchestrator
        let handler = crate::tools::handlers::SimpleTaskHandler::new();
        let context =
            crate::tools::context::ToolContext::new(std::env::current_dir().unwrap_or_default());
        handler.execute(args, &context).await
    }

    pub(crate) async fn execute_list_subagents(&self, args: Value) -> Result<ToolResult> {
        let handler = crate::tools::handlers::ListSubagentsHandler::new();
        let context =
            crate::tools::context::ToolContext::new(std::env::current_dir().unwrap_or_default());
        handler.execute(args, &context).await
    }
}
