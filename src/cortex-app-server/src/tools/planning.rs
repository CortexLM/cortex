//! Planning and workflow tools (todos, plans, questions, tasks).

use serde_json::{Value, json};

use super::types::ToolResult;

/// Write/update a todo list.
pub async fn todo_write(args: Value) -> ToolResult {
    // Accept either string format or structured format
    let todos_str = if let Some(s) = args.get("todos").and_then(|v| v.as_str()) {
        // String format: "1. [completed] Task one\n2. [in_progress] Task two"
        s.to_string()
    } else if let Some(arr) = args.get("todos").and_then(|v| v.as_array()) {
        // Array format
        let mut output = String::new();
        for todo in arr {
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

            let status_marker = match status {
                "completed" => "[completed]",
                "in_progress" => "[in_progress]",
                _ => "[pending]",
            };
            let priority_marker = match priority {
                "high" => "[high]",
                "low" => "[low]",
                _ => "[medium]",
            };
            output.push_str(&format!(
                "{id}. {status_marker} {priority_marker} {content}\n"
            ));
        }
        output
    } else {
        return ToolResult::error("todos is required (string or array)");
    };

    ToolResult::success(format!("TODO List Updated\n\n{todos_str}"))
}

/// Read the current todo list.
pub async fn todo_read(_args: Value) -> ToolResult {
    ToolResult::success("No persistent todos (stateless execution)")
}

/// Task tool - spawn subagents (not available in container mode).
pub async fn task(_args: Value) -> ToolResult {
    // Task tool is for spawning subagents - not supported in container
    ToolResult::error("Task tool (subagents) not available in container mode")
}

/// Present an implementation plan for user approval.
pub async fn plan(args: Value) -> ToolResult {
    // Plan tool returns the plan data as JSON for frontend to display
    // The frontend will show accept/reject buttons
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Implementation Plan");
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tasks = args.get("tasks").cloned().unwrap_or(json!([]));
    let estimate = args
        .get("estimated_changes")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let plan_data = json!({
        "type": "plan",
        "title": title,
        "description": description,
        "tasks": tasks,
        "estimated_changes": estimate,
        "status": "pending_approval"
    });

    ToolResult {
        success: true,
        output: serde_json::to_string_pretty(&plan_data).unwrap_or_default(),
        error: None,
        metadata: Some(plan_data),
    }
}

/// Ask the user a series of questions.
pub async fn questions(args: Value) -> ToolResult {
    // Questions tool returns questions data for frontend to display as a form
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Questions");
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let questions = args.get("questions").cloned().unwrap_or(json!([]));

    let questions_data = json!({
        "type": "questions",
        "title": title,
        "description": description,
        "questions": questions,
        "status": "pending_answers"
    });

    ToolResult {
        success: true,
        output: serde_json::to_string_pretty(&questions_data).unwrap_or_default(),
        error: None,
        metadata: Some(questions_data),
    }
}
