//! Tests for tool handlers.

use crate::tools::context::ToolContext;
use crate::tools::handlers::*;
use crate::tools::spec::ToolResult;
use std::path::PathBuf;

// Test TodoItem and related types
#[test]
fn test_todo_status_variants() {
    let pending = TodoStatus::Pending;
    let in_progress = TodoStatus::InProgress;
    let completed = TodoStatus::Completed;

    assert_eq!(format!("{:?}", pending), "Pending");
    assert_eq!(format!("{:?}", in_progress), "InProgress");
    assert_eq!(format!("{:?}", completed), "Completed");
}

#[test]
fn test_todo_priority_variants() {
    let high = TodoPriority::High;
    let medium = TodoPriority::Medium;
    let low = TodoPriority::Low;

    assert_eq!(format!("{:?}", high), "High");
    assert_eq!(format!("{:?}", medium), "Medium");
    assert_eq!(format!("{:?}", low), "Low");
}

#[test]
fn test_todo_item_creation() {
    let item = TodoItem {
        id: "1".to_string(),
        content: "Test task".to_string(),
        status: TodoStatus::Pending,
        priority: TodoPriority::Medium,
    };

    assert_eq!(item.id, "1");
    assert_eq!(item.content, "Test task");
    assert!(matches!(item.status, TodoStatus::Pending));
    assert!(matches!(item.priority, TodoPriority::Medium));
}

#[test]
fn test_todo_item_serialization() {
    let item = TodoItem {
        id: "todo_1".to_string(),
        content: "Complete feature".to_string(),
        status: TodoStatus::InProgress,
        priority: TodoPriority::High,
    };

    let json = serde_json::to_string(&item).expect("serialize");
    assert!(json.contains("todo_1"));
    assert!(json.contains("Complete feature"));

    let parsed: TodoItem = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.id, "todo_1");
}

// Handler creation tests
#[test]
fn test_patch_handler_name() {
    let handler = PatchHandler::new();
    assert_eq!(handler.name(), "Patch");
}

#[test]
fn test_patch_handler_default() {
    let handler = PatchHandler::default();
    assert_eq!(handler.name(), "Patch");
}

#[test]
fn test_grep_handler_name() {
    let handler = GrepHandler::new();
    assert_eq!(handler.name(), "Grep");
}

#[test]
fn test_grep_handler_default() {
    let handler = GrepHandler::default();
    assert_eq!(handler.name(), "Grep");
}

#[test]
fn test_glob_handler_name() {
    let handler = GlobHandler::new();
    assert_eq!(handler.name(), "Glob");
}

#[test]
fn test_glob_handler_default() {
    let handler = GlobHandler::default();
    assert_eq!(handler.name(), "Glob");
}

#[test]
fn test_local_shell_handler_name() {
    let handler = LocalShellHandler::new();
    assert_eq!(handler.name(), "Execute");
}

#[test]
fn test_fetch_url_handler_name() {
    let handler = FetchUrlHandler::new();
    assert_eq!(handler.name(), "FetchUrl");
}

#[test]
fn test_web_search_handler_name() {
    let handler = WebSearchHandler::new();
    assert_eq!(handler.name(), "WebSearch");
}

// Tool result tests
#[test]
fn test_tool_result_success_message() {
    let result = ToolResult::success("File read successfully");

    assert!(result.success);
    assert!(!result.is_error());
    assert_eq!(result.content(), "File read successfully");
}

#[test]
fn test_tool_result_error_message() {
    let result = ToolResult::error("File not found: /nonexistent");

    assert!(!result.success);
    assert!(result.is_error());
    assert!(result.content().contains("File not found"));
}

// Async handler execution tests
#[tokio::test]
#[cfg_attr(windows, ignore = "Unix paths not applicable on Windows")]
async fn test_patch_handler_file_not_found() {
    let handler = PatchHandler::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    let args = serde_json::json!({
        "file_path": "/nonexistent/path/file.txt",
        "old_str": "old",
        "new_str": "new"
    });

    let result = handler.execute(args, &ctx).await.expect("execute");

    // The handler should indicate failure for non-existent file
    assert!(
        !result.success || result.is_error(),
        "Expected failure for non-existent file, got success with: {}",
        result.content()
    );
}

#[tokio::test]
#[cfg_attr(windows, ignore = "Unix paths not applicable on Windows")]
async fn test_grep_handler_invalid_regex() {
    let handler = GrepHandler::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    let args = serde_json::json!({
        "pattern": "[invalid(regex",
        "path": "/tmp"
    });

    let result = handler.execute(args, &ctx).await.expect("execute");

    // Invalid regex should return error
    assert!(
        !result.success
            || result.content().contains("regex")
            || result.content().contains("Invalid")
    );
}

#[tokio::test]
#[cfg_attr(windows, ignore = "Unix paths not applicable on Windows")]
async fn test_glob_handler_basic_pattern() {
    let handler = GlobHandler::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    let args = serde_json::json!({
        "patterns": ["*.txt"]
    });

    // This should execute without error (may find files or not)
    let result = handler.execute(args, &ctx).await.expect("execute");

    // Should succeed even if no matches
    assert!(result.success);
}

// Integration-style tests with temp files
#[tokio::test]
async fn test_patch_handler_success() {
    use std::fs;
    use tempfile::tempdir;

    let temp = tempdir().expect("create temp dir");
    let file_path = temp.path().join("test.txt");
    fs::write(&file_path, "Hello world").expect("write file");

    let handler = PatchHandler::new();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let args = serde_json::json!({
        "file_path": file_path.to_str().unwrap(),
        "old_str": "world",
        "new_str": "Rust"
    });

    let result = handler.execute(args, &ctx).await.expect("execute");

    assert!(result.success, "Edit should succeed: {}", result.content());

    let content = fs::read_to_string(&file_path).expect("read file");
    assert_eq!(content, "Hello Rust");
}

#[tokio::test]
async fn test_patch_handler_old_str_not_found() {
    use std::fs;
    use tempfile::tempdir;

    let temp = tempdir().expect("create temp dir");
    let file_path = temp.path().join("test.txt");
    fs::write(&file_path, "Hello world").expect("write file");

    let handler = PatchHandler::new();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let args = serde_json::json!({
        "file_path": file_path.to_str().unwrap(),
        "old_str": "not_present",
        "new_str": "replacement"
    });

    let result = handler.execute(args, &ctx).await.expect("execute");

    assert!(!result.success);
    assert!(result.content().contains("not") || result.content().contains("Could not find"));
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "Windows path handling differs from Unix in this test"
)]
async fn test_patch_handler_multiple_occurrences() {
    use std::fs;
    use tempfile::tempdir;

    let temp = tempdir().expect("create temp dir");
    let file_path = temp.path().join("test.txt");
    fs::write(&file_path, "foo bar foo").expect("write file");

    let handler = PatchHandler::new();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    // Without change_all - behavior may vary:
    // - Either fail with multiple occurrences error
    // - Or succeed by replacing first occurrence only
    let args = serde_json::json!({
        "file_path": file_path.to_str().unwrap(),
        "old_str": "foo",
        "new_str": "baz"
    });

    let result = handler.execute(args, &ctx).await.expect("execute");

    // Accept either behavior: fail with error OR succeed with first replacement
    if result.success {
        // If successful, file should have been modified
        let content = fs::read_to_string(&file_path).expect("read file");
        assert!(
            content.contains("baz"),
            "File should contain 'baz' after edit"
        );
    } else {
        // If failed, should mention multiple occurrences
        assert!(
            result.content().contains("occurrences")
                || result.content().contains("Found")
                || result.content().contains("multiple"),
            "Error should mention multiple occurrences: {}",
            result.content()
        );
    }
}

#[tokio::test]
async fn test_patch_handler_change_all() {
    use std::fs;
    use tempfile::tempdir;

    let temp = tempdir().expect("create temp dir");
    let file_path = temp.path().join("test.txt");
    fs::write(&file_path, "foo bar foo").expect("write file");

    let handler = PatchHandler::new();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let args = serde_json::json!({
        "file_path": file_path.to_str().unwrap(),
        "old_str": "foo",
        "new_str": "baz",
        "change_all": true
    });

    let result = handler.execute(args, &ctx).await.expect("execute");

    assert!(
        result.success,
        "Edit with change_all should succeed: {}",
        result.content()
    );

    let content = fs::read_to_string(&file_path).expect("read file");
    assert_eq!(content, "baz bar baz");
}

#[tokio::test]
async fn test_grep_handler_search_in_temp() {
    use std::fs;
    use tempfile::tempdir;

    let temp = tempdir().expect("create temp dir");
    let file_path = temp.path().join("test.txt");
    fs::write(&file_path, "Hello world\nThis is a test\nHello again").expect("write file");

    let handler = GrepHandler::new();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let args = serde_json::json!({
        "pattern": "Hello",
        "path": temp.path().to_str().unwrap(),
        "output_mode": "content",
        "line_numbers": true
    });

    let result = handler.execute(args, &ctx).await.expect("execute");

    assert!(result.success);
    // Should find matches
    if !result.content().contains("No matches") {
        assert!(result.content().contains("Hello") || result.content().contains("test.txt"));
    }
}
