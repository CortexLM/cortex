//! Tool Call Display Types
//!
//! Types for displaying tool calls in the UI, including status tracking
//! and formatted summaries for collapsed/expanded views.

use serde_json::Value;

// ============================================================
// CONTENT SEGMENT - For interleaved text/tool display
// ============================================================

/// A segment of content in the streaming timeline.
/// Used to interleave text and tool calls in display order.
#[derive(Debug, Clone)]
pub enum ContentSegment {
    /// A text segment from the assistant
    Text { content: String, sequence: u64 },
    /// A tool call reference (by ID)
    ToolCall { tool_call_id: String, sequence: u64 },
}

impl ContentSegment {
    /// Get the sequence number for ordering
    pub fn sequence(&self) -> u64 {
        match self {
            ContentSegment::Text { sequence, .. } => *sequence,
            ContentSegment::ToolCall { sequence, .. } => *sequence,
        }
    }
}

/// Status of a tool call execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolStatus {
    /// ○ waiting for approval
    #[default]
    Pending,
    /// ● executing
    Running,
    /// ● done (green)
    Completed,
    /// ● error (red)
    Failed,
}

/// Display state for a tool call in the UI
#[derive(Debug, Clone)]
pub struct ToolCallDisplay {
    /// Unique identifier for the tool call
    pub id: String,
    /// Name of the tool being called
    pub name: String,
    /// Arguments passed to the tool
    pub arguments: Value,
    /// Current execution status
    pub status: ToolStatus,
    /// Result of the tool call, if completed
    pub result: Option<ToolResultDisplay>,
    /// Whether the tool call view is collapsed
    pub collapsed: bool,
    /// Sequence number for ordering (arrival order)
    pub sequence: u64,
    /// Animation frame for spinner (updated by tick)
    pub spinner_frame: usize,
    /// Live output buffer (for streaming display, last 3 lines)
    pub live_output: Vec<String>,
}

impl ToolCallDisplay {
    /// Create a new tool call display with pending status
    pub fn new(id: String, name: String, arguments: Value, sequence: u64) -> Self {
        Self {
            id,
            name,
            arguments,
            status: ToolStatus::Pending,
            result: None,
            collapsed: true,
            sequence,
            spinner_frame: 0,
            live_output: Vec::new(),
        }
    }

    /// Toggle the collapsed state
    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    /// Update the tool call status
    pub fn set_status(&mut self, status: ToolStatus) {
        self.status = status;
    }

    /// Set the result of the tool call
    pub fn set_result(&mut self, result: ToolResultDisplay) {
        self.result = Some(result);
    }

    /// Append output line (keeps only last 3 lines for display)
    pub fn append_output(&mut self, line: String) {
        self.live_output.push(line);
        while self.live_output.len() > 3 {
            self.live_output.remove(0);
        }
    }

    /// Clear live output (called when tool completes)
    pub fn clear_live_output(&mut self) {
        self.live_output.clear();
    }

    /// Advance spinner frame
    pub fn tick_spinner(&mut self) {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
    }
}

/// Display state for a tool call result
#[derive(Debug, Clone)]
pub struct ToolResultDisplay {
    /// Full output from the tool
    pub output: String,
    /// Whether the tool call succeeded
    pub success: bool,
    /// Short summary for collapsed view
    pub summary: String,
}

/// Format tool arguments for collapsed view based on tool name
///
/// Returns a short summary of the tool call arguments suitable for display
/// in a collapsed view.
pub fn format_tool_summary(name: &str, args: &Value) -> String {
    match name.to_lowercase().as_str() {
        "read" | "edit" => {
            if let Some(path) = args.get("file_path").or_else(|| args.get("filePath"))
                && let Some(path_str) = path.as_str()
            {
                return shorten_path(path_str);
            }
            format_first_arg(args)
        }
        "execute" | "bash" => {
            if let Some(cmd) = args.get("command")
                && let Some(cmd_str) = cmd.as_str()
            {
                let truncated = truncate_str(cmd_str, 50);
                return format!("$ {truncated}");
            }
            format_first_arg(args)
        }
        "glob" => {
            if let Some(pattern) = args.get("pattern").or_else(|| args.get("patterns")) {
                match pattern {
                    Value::String(s) => return s.clone(),
                    Value::Array(arr) => {
                        let patterns: Vec<&str> =
                            arr.iter().filter_map(|v| v.as_str()).take(3).collect();
                        if patterns.len() < arr.len() {
                            return format!("{}, ...", patterns.join(", "));
                        }
                        return patterns.join(", ");
                    }
                    _ => {}
                }
            }
            format_first_arg(args)
        }
        "websearch" | "codesearch" => {
            if let Some(query) = args.get("query")
                && let Some(query_str) = query.as_str()
            {
                let truncated = truncate_str(query_str, 60);
                return format!("\"{truncated}\"");
            }
            format_first_arg(args)
        }
        _ => format_first_arg(args),
    }
}

/// Format a result summary based on tool name and output
///
/// Returns a short summary like "↳ Read 450 lines" or "↳ Error: file not found"
/// Uses '↳' prefix consistently across all tools for visual consistency.
pub fn format_result_summary(name: &str, output: &str, success: bool) -> String {
    if !success {
        // Extract first line of error, truncated
        let first_line = output.lines().next().unwrap_or("unknown error");
        let truncated = truncate_str(first_line, 50);
        return format!("↳ Error: {truncated}");
    }

    match name.to_lowercase().as_str() {
        "read" => {
            let line_count = output.lines().count();
            format!("↳ Read {line_count} lines")
        }
        "edit" | "multiedit" => "↳ Applied edit".to_string(),
        "create" => "↳ File created".to_string(),
        "execute" | "bash" => {
            let line_count = output.lines().count();
            if line_count == 0 {
                "↳ Completed".to_string()
            } else if line_count == 1 {
                format!("↳ {}", truncate_str(output.trim(), 60))
            } else {
                format!("↳ {line_count} lines of output")
            }
        }
        "glob" => {
            let file_count = output.lines().count();
            match file_count {
                0 => "↳ No matches".to_string(),
                1 => "↳ Found 1 file".to_string(),
                n => format!("↳ Found {n} files"),
            }
        }
        "grep" => {
            let match_count = output.lines().count();
            match match_count {
                0 => "↳ No matches".to_string(),
                1 => "↳ Found 1 match".to_string(),
                n => format!("↳ Found {n} matches"),
            }
        }
        "ls" => {
            let item_count = output.lines().count();
            match item_count {
                0 => "↳ Empty directory".to_string(),
                1 => "↳ Listed 1 item".to_string(),
                n => format!("↳ Listed {n} items"),
            }
        }
        "websearch" | "codesearch" | "fetchurl" => {
            let char_count = output.len();
            if char_count > 1000 {
                format!("↳ Retrieved ~{} chars", char_count)
            } else {
                "↳ Retrieved results".to_string()
            }
        }
        "write" => "↳ File written".to_string(),
        "todowrite" => "↳ Todos updated".to_string(),
        "task" => "↳ Task completed".to_string(),
        _ => {
            let line_count = output.lines().count();
            if line_count == 0 {
                "↳ Completed".to_string()
            } else {
                format!("↳ {line_count} items")
            }
        }
    }
}

/// Shorten a file path for display
fn shorten_path(path: &str) -> String {
    // Get the last 2-3 components of the path
    let components: Vec<&str> = path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();

    if components.len() <= 3 {
        return path.to_string();
    }

    // Take last 3 components
    let last_three = &components[components.len() - 3..];
    format!(".../{}", last_three.join("/"))
}

/// Truncate a string to a maximum length, adding ellipsis if needed
fn truncate_str(s: &str, max_len: usize) -> String {
    let s = s.trim();
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Format the first argument value for display
fn format_first_arg(args: &Value) -> String {
    if let Some(obj) = args.as_object()
        && let Some((_, first_value)) = obj.iter().next()
    {
        return match first_value {
            Value::String(s) => truncate_str(s, 50),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Array(arr) => format!("[{} items]", arr.len()),
            Value::Object(obj) => format!("{{{} fields}}", obj.len()),
            Value::Null => "null".to_string(),
        };
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_status_default() {
        let status = ToolStatus::default();
        assert_eq!(status, ToolStatus::Pending);
    }

    #[test]
    fn test_tool_call_display_new() {
        let display = ToolCallDisplay::new(
            "test-id".to_string(),
            "read".to_string(),
            json!({"file_path": "/tmp/test.rs"}),
            0,
        );

        assert_eq!(display.id, "test-id");
        assert_eq!(display.name, "read");
        assert_eq!(display.status, ToolStatus::Pending);
        assert!(display.result.is_none());
        assert!(display.collapsed);
        assert_eq!(display.sequence, 0);
    }

    #[test]
    fn test_toggle_collapsed() {
        let mut display =
            ToolCallDisplay::new("test-id".to_string(), "read".to_string(), json!({}), 0);

        assert!(display.collapsed);
        display.toggle_collapsed();
        assert!(!display.collapsed);
        display.toggle_collapsed();
        assert!(display.collapsed);
    }

    #[test]
    fn test_set_status() {
        let mut display =
            ToolCallDisplay::new("test-id".to_string(), "read".to_string(), json!({}), 0);

        display.set_status(ToolStatus::Running);
        assert_eq!(display.status, ToolStatus::Running);

        display.set_status(ToolStatus::Completed);
        assert_eq!(display.status, ToolStatus::Completed);
    }

    #[test]
    fn test_set_result() {
        let mut display =
            ToolCallDisplay::new("test-id".to_string(), "read".to_string(), json!({}), 0);

        let result = ToolResultDisplay {
            output: "file contents".to_string(),
            success: true,
            summary: "Read 10 lines".to_string(),
        };

        display.set_result(result);
        assert!(display.result.is_some());
        assert!(display.result.as_ref().unwrap().success);
    }

    #[test]
    fn test_format_tool_summary_read() {
        let args = json!({"file_path": "/home/user/projects/myapp/src/main.rs"});
        let summary = format_tool_summary("read", &args);
        assert!(summary.contains("main.rs"));
    }

    #[test]
    fn test_format_tool_summary_bash() {
        let args = json!({"command": "cargo build --release"});
        let summary = format_tool_summary("bash", &args);
        assert_eq!(summary, "$ cargo build --release");
    }

    #[test]
    fn test_format_tool_summary_websearch() {
        let args = json!({"query": "rust async programming"});
        let summary = format_tool_summary("websearch", &args);
        assert_eq!(summary, "\"rust async programming\"");
    }

    #[test]
    fn test_format_tool_summary_glob() {
        let args = json!({"pattern": "**/*.rs"});
        let summary = format_tool_summary("glob", &args);
        assert_eq!(summary, "**/*.rs");
    }

    #[test]
    fn test_format_result_summary_read() {
        let output = "line1\nline2\nline3\nline4\nline5";
        let summary = format_result_summary("read", output, true);
        assert_eq!(summary, "↳ Read 5 lines");
    }

    #[test]
    fn test_format_result_summary_error() {
        let output = "file not found";
        let summary = format_result_summary("read", output, false);
        assert_eq!(summary, "↳ Error: file not found");
    }

    #[test]
    fn test_format_result_summary_glob() {
        let output = "file1.rs\nfile2.rs\nfile3.rs";
        let summary = format_result_summary("glob", output, true);
        assert_eq!(summary, "↳ Found 3 files");
    }

    #[test]
    fn test_format_result_summary_ls() {
        let output = "file1.rs\nfile2.rs\ndir1";
        let summary = format_result_summary("ls", output, true);
        assert_eq!(summary, "↳ Listed 3 items");
    }

    #[test]
    fn test_format_result_summary_grep() {
        let output = "match1\nmatch2";
        let summary = format_result_summary("grep", output, true);
        assert_eq!(summary, "↳ Found 2 matches");
    }

    #[test]
    fn test_format_result_summary_execute_multiline() {
        let output = "line1\nline2\nline3";
        let summary = format_result_summary("execute", output, true);
        assert_eq!(summary, "↳ 3 lines of output");
    }

    #[test]
    fn test_format_result_summary_default() {
        let output = "item1\nitem2\nitem3\nitem4";
        let summary = format_result_summary("unknown_tool", output, true);
        assert_eq!(summary, "↳ 4 items");
    }

    #[test]
    fn test_shorten_path() {
        let path = "/home/user/projects/myapp/src/views/main.rs";
        let shortened = shorten_path(path);
        assert_eq!(shortened, ".../src/views/main.rs");
    }

    #[test]
    fn test_shorten_path_short() {
        let path = "src/main.rs";
        let shortened = shorten_path(path);
        assert_eq!(shortened, "src/main.rs");
    }

    #[test]
    fn test_truncate_str() {
        let s = "this is a very long string that needs truncation";
        let truncated = truncate_str(s, 20);
        assert_eq!(truncated, "this is a very lo...");
    }

    #[test]
    fn test_truncate_str_short() {
        let s = "short";
        let truncated = truncate_str(s, 20);
        assert_eq!(truncated, "short");
    }
}
