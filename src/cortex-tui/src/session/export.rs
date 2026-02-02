//! Session export functionality.
//!
//! Supports exporting sessions to various formats:
//! - Markdown
//! - JSON
//! - Plain text

use anyhow::Result;
use chrono::Local;

use super::manager::CortexSession;
use super::types::StoredMessage;

// ============================================================
// EXPORT FORMAT
// ============================================================

/// Export format options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// Markdown format with syntax highlighting.
    Markdown,
    /// JSON format (full data).
    Json,
    /// Plain text format.
    Text,
}

impl ExportFormat {
    /// Gets the file extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            ExportFormat::Markdown => "md",
            ExportFormat::Json => "json",
            ExportFormat::Text => "txt",
        }
    }

    /// Gets the display name for this format.
    pub fn name(&self) -> &'static str {
        match self {
            ExportFormat::Markdown => "Markdown",
            ExportFormat::Json => "JSON",
            ExportFormat::Text => "Text",
        }
    }

    /// Parses a format from string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "md" | "markdown" => Some(ExportFormat::Markdown),
            "json" => Some(ExportFormat::Json),
            "txt" | "text" => Some(ExportFormat::Text),
            _ => None,
        }
    }
}

// ============================================================
// EXPORT FUNCTION
// ============================================================

/// Exports a session to the specified format.
pub fn export_session(session: &CortexSession, format: ExportFormat) -> Result<String> {
    match format {
        ExportFormat::Markdown => export_markdown(session),
        ExportFormat::Json => export_json(session),
        ExportFormat::Text => export_text(session),
    }
}

/// Exports a session to Markdown format.
fn export_markdown(session: &CortexSession) -> Result<String> {
    let mut output = String::new();

    // Header
    output.push_str(&format!("# {}\n\n", session.title()));
    output.push_str(&format!(
        "**Model:** {} / {}\n\n",
        session.meta.provider, session.meta.model
    ));
    output.push_str(&format!(
        "**Created:** {}\n\n",
        session
            .meta
            .created_at
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
    ));
    output.push_str(&format!(
        "**Messages:** {} | **Tokens:** {}\n\n",
        session.message_count(),
        session.format_tokens()
    ));
    output.push_str("---\n\n");

    // Messages
    for message in session.messages() {
        output.push_str(&format_message_markdown(message));
        output.push('\n');
    }

    Ok(output)
}

/// Escape nested code blocks in content by using different fence styles.
/// If content contains ```, use ~~~~ as the outer fence.
fn escape_nested_code_blocks(content: &str) -> (String, &'static str) {
    // Count the maximum number of consecutive backticks in the content
    let max_backticks = content.split("```").count().saturating_sub(1);

    if max_backticks > 0 || content.contains("```") {
        // Use tildes as outer fence when content has backticks
        ("~~~~".to_string(), "~~~~")
    } else {
        ("```".to_string(), "```")
    }
}

/// Formats a single message as Markdown.
fn format_message_markdown(message: &StoredMessage) -> String {
    let mut output = String::new();

    // Role header
    let role_icon = match message.role.as_str() {
        "user" => ">",
        "assistant" => "",
        "system" => "[System]",
        _ => "",
    };

    if message.role == "user" {
        output.push_str(&format!("### {} User\n\n", role_icon));
    } else if message.role == "assistant" {
        output.push_str("### Assistant\n\n");
    } else if message.role == "system" {
        output.push_str(&format!("### {}\n\n", role_icon));
    }

    // Reasoning (if present)
    if let Some(reasoning) = &message.reasoning {
        output.push_str("<details>\n<summary>Thinking</summary>\n\n");
        output.push_str(reasoning);
        output.push_str("\n\n</details>\n\n");
    }

    // Content
    output.push_str(&message.content);
    output.push_str("\n\n");

    // Tool calls
    for tool_call in &message.tool_calls {
        output.push_str(&format!("**Tool:** `{}`\n", tool_call.name));
        output.push_str("```json\n");
        output.push_str(&serde_json::to_string_pretty(&tool_call.input).unwrap_or_default());
        output.push_str("\n```\n");
        if let Some(result) = &tool_call.output {
            let status = if tool_call.success {
                "Success"
            } else {
                "Error"
            };
            output.push_str(&format!("**Result ({}):**\n", status));

            // Use appropriate fence style based on content
            let (fence_open, fence_close) = escape_nested_code_blocks(result);
            output.push_str(&fence_open);
            output.push('\n');

            // Truncate long outputs
            if result.len() > 1000 {
                output.push_str(&result[..1000]);
                output.push_str("\n... (truncated)");
            } else {
                output.push_str(result);
            }
            output.push('\n');
            output.push_str(fence_close);
            output.push('\n');
        }
        output.push('\n');
    }

    // Token usage
    if let Some(tokens) = &message.tokens {
        output.push_str(&format!(
            "*Tokens: {} in, {} out*\n\n",
            tokens.input_tokens, tokens.output_tokens
        ));
    }

    output
}

/// Exports a session to JSON format.
fn export_json(session: &CortexSession) -> Result<String> {
    #[derive(serde::Serialize)]
    struct ExportedSession<'a> {
        meta: &'a super::types::SessionMeta,
        messages: &'a [StoredMessage],
    }

    let exported = ExportedSession {
        meta: &session.meta,
        messages: session.messages(),
    };

    Ok(serde_json::to_string_pretty(&exported)?)
}

/// Exports a session to plain text format.
fn export_text(session: &CortexSession) -> Result<String> {
    let mut output = String::new();

    // Header
    output.push_str(&format!("{}\n", session.title()));
    output.push_str(&format!(
        "Model: {} / {}\n",
        session.meta.provider, session.meta.model
    ));
    output.push_str(&format!(
        "Created: {}\n",
        session
            .meta
            .created_at
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
    ));
    output.push_str(&format!(
        "Messages: {} | Tokens: {}\n",
        session.message_count(),
        session.format_tokens()
    ));
    output.push_str(&"=".repeat(60));
    output.push_str("\n\n");

    // Messages
    for message in session.messages() {
        output.push_str(&format_message_text(message));
        output.push('\n');
    }

    Ok(output)
}

/// Formats a single message as plain text.
fn format_message_text(message: &StoredMessage) -> String {
    let mut output = String::new();

    // Role
    let role = match message.role.as_str() {
        "user" => "> User",
        "assistant" => "Assistant",
        "system" => "[System]",
        _ => &message.role,
    };

    output.push_str(&format!("{}\n", role));
    output.push_str(&"-".repeat(40));
    output.push('\n');

    // Content
    output.push_str(&message.content);
    output.push_str("\n\n");

    // Tool calls (simplified)
    for tool_call in &message.tool_calls {
        let status = if tool_call.success { "OK" } else { "ERROR" };
        output.push_str(&format!("[Tool: {} - {}]\n", tool_call.name, status));
    }

    output
}

// ============================================================
// FILE EXPORT
// ============================================================

/// Exports a session to a file.
pub fn export_to_file(
    session: &CortexSession,
    format: ExportFormat,
    path: &std::path::Path,
) -> Result<()> {
    let content = export_session(session, format)?;
    std::fs::write(path, content)?;
    Ok(())
}

/// Generates a default filename for export.
pub fn default_export_filename(session: &CortexSession, format: ExportFormat) -> String {
    let title = session
        .meta
        .title
        .clone()
        .unwrap_or_else(|| session.meta.short_id().to_string());

    // Sanitize title for filename
    let safe_title: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    let timestamp = session.meta.created_at.format("%Y%m%d");

    format!("cortex_{}_{}.{}", safe_title, timestamp, format.extension())
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::storage::SessionStorage;
    use cortex_engine::client::TokenUsage;
    use tempfile::TempDir;

    fn create_test_session_with_messages() -> (CortexSession, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let storage = SessionStorage::with_dir(temp_dir.path().to_path_buf());
        let mut session =
            CortexSession::with_storage("cortex", "anthropic/claude-opus-4", storage).unwrap();

        session.add_user_message("What is 2 + 2?");
        session.add_assistant_message(
            "2 + 2 = 4",
            TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
            },
        );

        (session, temp_dir)
    }

    #[test]
    fn test_export_markdown() {
        let (session, _temp) = create_test_session_with_messages();
        let md = export_session(&session, ExportFormat::Markdown).unwrap();

        assert!(md.contains("# "));
        assert!(md.contains("cortex"));
        assert!(md.contains("What is 2 + 2?"));
        assert!(md.contains("2 + 2 = 4"));
    }

    #[test]
    fn test_export_json() {
        let (session, _temp) = create_test_session_with_messages();
        let json = export_session(&session, ExportFormat::Json).unwrap();

        // Should be valid JSON
        let _: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(json.contains("\"provider\""));
        assert!(json.contains("\"messages\""));
    }

    #[test]
    fn test_export_text() {
        let (session, _temp) = create_test_session_with_messages();
        let text = export_session(&session, ExportFormat::Text).unwrap();

        assert!(text.contains("> User"));
        assert!(text.contains("Assistant"));
        assert!(text.contains("What is 2 + 2?"));
    }

    #[test]
    fn test_format_extension() {
        assert_eq!(ExportFormat::Markdown.extension(), "md");
        assert_eq!(ExportFormat::Json.extension(), "json");
        assert_eq!(ExportFormat::Text.extension(), "txt");
    }

    #[test]
    fn test_format_from_str() {
        assert_eq!(ExportFormat::parse("md"), Some(ExportFormat::Markdown));
        assert_eq!(
            ExportFormat::parse("markdown"),
            Some(ExportFormat::Markdown)
        );
        assert_eq!(ExportFormat::parse("json"), Some(ExportFormat::Json));
        assert_eq!(ExportFormat::parse("txt"), Some(ExportFormat::Text));
        assert_eq!(ExportFormat::parse("unknown"), None);
    }
}
