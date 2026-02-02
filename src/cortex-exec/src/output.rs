//! Output formatting for headless execution.

use std::io::{self, Write};

/// Output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Plain text output.
    #[default]
    Text,
    /// JSON output.
    Json,
    /// JSON Lines output (one JSON object per line).
    JsonLines,
    /// Quiet mode (minimal output).
    Quiet,
}

/// Output writer.
pub struct OutputWriter {
    format: OutputFormat,
}

impl OutputWriter {
    /// Create a new output writer.
    pub fn new(format: OutputFormat) -> Self {
        Self { format }
    }

    /// Write an info message.
    pub fn write_info(&self, message: &str) {
        match self.format {
            OutputFormat::Text => {
                eprintln!("[INFO] {message}");
            }
            OutputFormat::Json | OutputFormat::JsonLines => {
                self.write_json_event("info", message);
            }
            OutputFormat::Quiet => {}
        }
    }

    /// Write a success message.
    pub fn write_success(&self, message: &str) {
        match self.format {
            OutputFormat::Text => {
                eprintln!("[OK] {message}");
            }
            OutputFormat::Json | OutputFormat::JsonLines => {
                self.write_json_event("success", message);
            }
            OutputFormat::Quiet => {}
        }
    }

    /// Write an error message.
    pub fn write_error(&self, message: &str) {
        match self.format {
            OutputFormat::Text => {
                eprintln!("[ERROR] {message}");
            }
            OutputFormat::Json | OutputFormat::JsonLines => {
                self.write_json_event("error", message);
            }
            OutputFormat::Quiet => {
                eprintln!("{message}");
            }
        }
    }

    /// Write the final response.
    pub fn write_response(&self, response: &str) {
        match self.format {
            OutputFormat::Text | OutputFormat::Quiet => {
                println!("{response}");
            }
            OutputFormat::Json => {
                let output = serde_json::json!({
                    "type": "response",
                    "content": response
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&output).unwrap_or_default()
                );
            }
            OutputFormat::JsonLines => {
                let output = serde_json::json!({
                    "type": "response",
                    "content": response
                });
                println!("{}", serde_json::to_string(&output).unwrap_or_default());
            }
        }
    }

    /// Write a tool call event.
    pub fn write_tool_call(&self, tool: &str, args: &str) {
        match self.format {
            OutputFormat::Text => {
                eprintln!("[TOOL] {tool} {args}");
            }
            OutputFormat::Json | OutputFormat::JsonLines => {
                let output = serde_json::json!({
                    "type": "tool_call",
                    "tool": tool,
                    "arguments": args
                });
                if self.format == OutputFormat::JsonLines {
                    println!("{}", serde_json::to_string(&output).unwrap_or_default());
                }
            }
            OutputFormat::Quiet => {}
        }
    }

    /// Write a tool result event.
    pub fn write_tool_result(&self, tool: &str, result: &str, success: bool) {
        match self.format {
            OutputFormat::Text => {
                let status = if success { "OK" } else { "FAIL" };
                eprintln!("[{}] {} -> {}", status, tool, truncate(result, 100));
            }
            OutputFormat::Json | OutputFormat::JsonLines => {
                let output = serde_json::json!({
                    "type": "tool_result",
                    "tool": tool,
                    "result": result,
                    "success": success
                });
                if self.format == OutputFormat::JsonLines {
                    println!("{}", serde_json::to_string(&output).unwrap_or_default());
                }
            }
            OutputFormat::Quiet => {}
        }
    }

    /// Write streaming content delta.
    pub fn write_delta(&self, delta: &str) {
        match self.format {
            OutputFormat::Text => {
                print!("{delta}");
                io::stdout().flush().ok();
            }
            OutputFormat::JsonLines => {
                let output = serde_json::json!({
                    "type": "delta",
                    "content": delta
                });
                println!("{}", serde_json::to_string(&output).unwrap_or_default());
            }
            OutputFormat::Json | OutputFormat::Quiet => {}
        }
    }

    fn write_json_event(&self, event_type: &str, message: &str) {
        let output = serde_json::json!({
            "type": event_type,
            "message": message
        });
        if self.format == OutputFormat::JsonLines {
            eprintln!("{}", serde_json::to_string(&output).unwrap_or_default());
        }
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

impl Default for OutputWriter {
    fn default() -> Self {
        Self::new(OutputFormat::Text)
    }
}
