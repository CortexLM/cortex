//! Todo list tool handler for task management.

use std::sync::Arc;
use tokio::sync::RwLock;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ToolContext, ToolHandler, ToolResult};
use crate::error::Result;

/// A todo item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
    pub priority: TodoPriority,
}

/// Todo item status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// Todo item priority.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoPriority {
    High,
    Medium,
    Low,
}

/// Handler for todo_write tool.
pub struct TodoWriteHandler {
    todos: Arc<RwLock<Vec<TodoItem>>>,
}

#[derive(Debug, Deserialize)]
struct TodoWriteArgs {
    todos: Vec<TodoItemInput>,
}

#[derive(Debug, Deserialize)]
struct TodoItemInput {
    id: String,
    content: String,
    status: String,
    priority: String,
}

impl TodoWriteHandler {
    pub fn new() -> Self {
        Self {
            todos: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn with_shared_state(todos: Arc<RwLock<Vec<TodoItem>>>) -> Self {
        Self { todos }
    }
}

impl Default for TodoWriteHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for TodoWriteHandler {
    fn name(&self) -> &str {
        "TodoWrite"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let args: TodoWriteArgs = serde_json::from_value(arguments)?;

        let mut todos = self.todos.write().await;
        todos.clear();

        for item in args.todos {
            let status = match item.status.as_str() {
                "in_progress" => TodoStatus::InProgress,
                "completed" => TodoStatus::Completed,
                _ => TodoStatus::Pending,
            };

            let priority = match item.priority.as_str() {
                "high" => TodoPriority::High,
                "low" => TodoPriority::Low,
                _ => TodoPriority::Medium,
            };

            todos.push(TodoItem {
                id: item.id,
                content: item.content,
                status,
                priority,
            });
        }

        // Format output
        let mut output = String::from("TODO List Updated\n\n");
        for todo in todos.iter() {
            let status_icon = match todo.status {
                TodoStatus::Pending => "[ ]",
                TodoStatus::InProgress => "[~]",
                TodoStatus::Completed => "[x]",
            };
            let priority_icon = match todo.priority {
                TodoPriority::High => "!!!",
                TodoPriority::Medium => "!!",
                TodoPriority::Low => "!",
            };
            output.push_str(&format!(
                "{} {} {}: {}\n",
                status_icon, priority_icon, todo.id, todo.content
            ));
        }

        Ok(ToolResult::success(output))
    }
}

/// Handler for todo_read tool.
pub struct TodoReadHandler {
    todos: Arc<RwLock<Vec<TodoItem>>>,
}

impl TodoReadHandler {
    pub fn new() -> Self {
        Self {
            todos: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn with_shared_state(todos: Arc<RwLock<Vec<TodoItem>>>) -> Self {
        Self { todos }
    }
}

impl Default for TodoReadHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for TodoReadHandler {
    fn name(&self) -> &str {
        "TodoRead"
    }

    async fn execute(&self, _arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let todos = self.todos.read().await;

        if todos.is_empty() {
            return Ok(ToolResult::success("No todos"));
        }

        let mut output = String::new();
        for todo in todos.iter() {
            let status_icon = match todo.status {
                TodoStatus::Pending => "[ ]",
                TodoStatus::InProgress => "[~]",
                TodoStatus::Completed => "[x]",
            };
            let priority_icon = match todo.priority {
                TodoPriority::High => "!!!",
                TodoPriority::Medium => "!!",
                TodoPriority::Low => "!",
            };
            output.push_str(&format!(
                "{} {} {}: {}\n",
                status_icon, priority_icon, todo.id, todo.content
            ));
        }

        Ok(ToolResult::success(output))
    }
}
