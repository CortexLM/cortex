//! Tool executor - dispatches tool calls to implementations.

use std::path::PathBuf;

use serde_json::Value;
use tracing::{debug, info};

use super::filesystem::{apply_patch, edit_file, list_dir, multi_edit, read_file, write_file};
use super::planning::{plan, questions, task, todo_read, todo_write};
use super::search::{glob, grep};
use super::shell::execute_shell;
use super::types::ToolResult;
use super::web::{fetch_url, web_search};

/// Tool executor - executes tools with proper sandboxing.
#[derive(Debug, Clone)]
pub struct ToolExecutor {
    cwd: PathBuf,
    timeout_secs: u64,
}

impl ToolExecutor {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            timeout_secs: 60,
        }
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Execute a tool by name.
    pub async fn execute(&self, name: &str, args: Value) -> ToolResult {
        info!(tool = %name, "Executing tool");
        debug!(args = ?args, "Tool arguments");

        match name {
            "Execute" | "local_shell" => execute_shell(&self.cwd, self.timeout_secs, args).await,
            "Read" | "read_file" => read_file(&self.cwd, args).await,
            "Create" | "write_file" => write_file(&self.cwd, args).await,
            "Edit" | "edit_file" => edit_file(&self.cwd, args).await,
            "LS" | "list_dir" => list_dir(&self.cwd, args).await,
            "Grep" | "grep" => grep(&self.cwd, args).await,
            "Glob" | "glob" => glob(&self.cwd, args).await,
            "FetchUrl" | "fetch_url" => fetch_url(args).await,
            "WebSearch" | "web_search" => web_search(args).await,
            "ApplyPatch" | "apply_patch" => apply_patch(&self.cwd, args).await,
            "TodoWrite" | "todo_write" => todo_write(args).await,
            "TodoRead" | "todo_read" => todo_read(args).await,
            "MultiEdit" | "multi_edit" => multi_edit(&self.cwd, args).await,
            "Task" => task(args).await,
            "Plan" => plan(args).await,
            "Questions" => questions(args).await,
            _ => ToolResult::error(format!("Unknown tool: {name}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_execute_list_dir() {
        let executor = ToolExecutor::new(std::env::current_dir().unwrap());
        let result = executor
            .execute("LS", json!({ "directory_path": "." }))
            .await;
        assert!(result.success);
        assert!(!result.output.is_empty());
    }
}
