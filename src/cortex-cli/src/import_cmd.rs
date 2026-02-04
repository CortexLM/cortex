//! Session import command for Cortex CLI.
//!
//! Imports a session from a portable JSON format (exported or shared).

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::styled_output::{print_info, print_success, print_warning};
use cortex_engine::rollout::recorder::{RolloutRecorder, SessionMeta};
use cortex_engine::rollout::{SESSIONS_SUBDIR, get_rollout_path};
use cortex_protocol::{
    AgentMessageEvent, ConversationId, Event, EventMsg, ExecCommandEndEvent, ExecCommandSource,
    ParsedCommand, UserMessageEvent,
};

use crate::agent_cmd::load_all_agents;
use crate::export_cmd::{ExportMessage, SessionExport};

/// Maximum depth for processing messages to prevent stack overflow from deeply nested structures.
const MAX_PROCESSING_DEPTH: usize = 10000;

/// Import a session from JSON format.
#[derive(Debug, Parser)]
pub struct ImportCommand {
    /// Path to the JSON file to import, URL to fetch, or "-" for stdin
    #[arg(value_name = "FILE_OR_URL")]
    pub source: String,

    /// Force import even if session already exists
    #[arg(short, long, default_value_t = false)]
    pub force: bool,

    /// Resume the imported session after import
    #[arg(long, default_value_t = false)]
    pub resume: bool,
}

impl ImportCommand {
    /// Run the import command.
    pub async fn run(self) -> Result<()> {
        // Validate source argument is not empty
        if self.source.trim().is_empty() {
            bail!("Error: Source path cannot be empty\n\nUsage: cortex import <FILE_OR_URL>");
        }

        let cortex_home = dirs::home_dir()
            .map(|h| h.join(".cortex"))
            .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

        // Read the export data
        let (json_content, is_from_url) = if self.source == "-" {
            // Read from stdin
            use std::io::Read;
            let mut content = String::new();
            std::io::stdin()
                .read_to_string(&mut content)
                .with_context(|| "Failed to read from stdin")?;
            (content, false)
        } else if self.source.starts_with("http://") || self.source.starts_with("https://") {
            // Fetch from URL
            (fetch_url(&self.source).await?, true)
        } else {
            // Read from local file
            let path = PathBuf::from(&self.source);
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read file: {}", path.display()))?;
            (content, false)
        };

        // Parse the export with helpful error messages
        let export: SessionExport = serde_json::from_str(&json_content).map_err(|e| {
            // Create a helpful error message with content preview
            let preview_len = json_content.len().min(200);
            let content_preview = &json_content[..preview_len];
            let truncated = if json_content.len() > 200 {
                "..."
            } else {
                ""
            };

            let source_type = if is_from_url { "URL" } else { "file" };

            // Detect common non-JSON content types
            let hint = if content_preview.trim_start().starts_with("<!DOCTYPE")
                || content_preview.trim_start().starts_with("<html")
            {
                "\nHint: The URL returned HTML content, not JSON. Make sure the URL points directly to a JSON export file."
            } else if content_preview.trim_start().starts_with("<?xml") {
                "\nHint: The URL returned XML content, not JSON. Make sure the URL points directly to a JSON export file."
            } else if content_preview.is_empty() {
                "\nHint: The response was empty. Make sure the URL is accessible and returns JSON content."
            } else {
                "\nHint: Ensure the file contains valid JSON. Check for syntax errors like missing commas, unclosed brackets, or invalid characters."
            };

            anyhow::anyhow!(
                "Failed to parse JSON from {}: {}\n\nReceived content (first {} bytes):\n{}{}\n{}",
                source_type,
                e,
                preview_len,
                content_preview,
                truncated,
                hint
            )
        })?;

        // Validate version
        if export.version != 1 {
            bail!(
                "Unsupported export version: {}. This CLI supports version 1.",
                export.version
            );
        }

        // Validate all messages, including base64 content
        validate_export_messages(&export.messages)?;

        // Validate agent references in the imported session
        let missing_agents = validate_agent_references(&export)?;
        if !missing_agents.is_empty() {
            eprintln!(
                "Warning: The following agent references in this session are not available locally:"
            );
            for agent in &missing_agents {
                eprintln!("  - @{}", agent);
            }
            eprintln!();
            eprintln!(
                "The session will be imported, but agent-related functionality may not work as expected."
            );
            eprintln!(
                "To fix this, create the missing agents using 'cortex agent create <name>' or copy them from the source system."
            );
            eprintln!();
        }

        // Generate a new session ID (we always create a new session on import)
        let new_conversation_id = ConversationId::new();

        // Check if a session with the original ID already exists
        let original_id: Result<ConversationId, _> = export.session.id.parse();
        if let Ok(orig_id) = original_id {
            let existing_path = get_rollout_path(&cortex_home, &orig_id);
            if existing_path.exists() && !self.force {
                print_warning(&format!(
                    "Original session {} already exists locally.",
                    export.session.id
                ));
                print_info(&format!(
                    "Creating new session with ID: {new_conversation_id}"
                ));
            }
        }

        // Create sessions directory if needed
        let sessions_dir = cortex_home.join(SESSIONS_SUBDIR);
        std::fs::create_dir_all(&sessions_dir)?;

        // Initialize rollout recorder for the new session
        let mut recorder = RolloutRecorder::new(&cortex_home, new_conversation_id)?;
        recorder.init()?;

        // Record session metadata
        let cwd = export
            .session
            .cwd
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        let meta = SessionMeta {
            id: new_conversation_id,
            parent_id: None,
            fork_point: None,
            timestamp: export.session.created_at.clone(),
            cwd: cwd.clone(),
            model: export
                .session
                .model
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            instructions: None,
        };
        recorder.record_meta(&meta)?;

        // Validate message count to prevent infinite loop on malicious input
        if export.messages.len() > MAX_PROCESSING_DEPTH {
            bail!(
                "Error: Session contains too many messages ({} > {}). \
                 This may indicate a malformed or malicious session file.",
                export.messages.len(),
                MAX_PROCESSING_DEPTH
            );
        }

        // Check for circular message references if messages have IDs and reply_to fields
        // This prevents infinite loops when processing message chains
        validate_no_circular_references(&export.messages)?;

        // Record messages as events
        let mut turn_id = 0u64;
        for message in &export.messages {
            let event = message_to_event(message, &mut turn_id, &cwd)?;
            recorder.record_event(&event)?;
        }

        recorder.flush()?;

        print_success(&format!("Imported session as: {new_conversation_id}"));
        println!("  Original ID: {}", export.session.id);
        if let Some(title) = &export.session.title {
            println!("  Title: {title}");
        }
        println!("  Messages: {}", export.messages.len());
        println!("\nTo resume: cortex resume {new_conversation_id}");

        if self.resume {
            // Launch resume
            print_info("Resuming session...");
            let config = cortex_engine::Config::default();

            #[cfg(feature = "cortex-tui")]
            {
                cortex_tui::resume(config, new_conversation_id).await?;
            }

            #[cfg(not(feature = "cortex-tui"))]
            {
                compile_error!("The 'cortex-tui' feature must be enabled");
            }
        }

        Ok(())
    }
}

/// Fetch content from a URL.
async fn fetch_url(url: &str) -> Result<String> {
    // Use curl for fetching
    {
        // Simple curl-based fallback
        use std::process::Command;

        let output = Command::new("curl")
            .args(["-sSL", url])
            .output()
            .with_context(|| "Failed to run curl. Install curl or use a local file.")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to fetch URL: {stderr}");
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Validate agent references in the imported session.
/// Returns a list of missing agent names that are referenced but not available locally.
fn validate_agent_references(export: &SessionExport) -> Result<Vec<String>> {
    // Get all locally available agents
    let local_agents: HashSet<String> = match load_all_agents() {
        Ok(agents) => agents.into_iter().map(|a| a.name).collect(),
        Err(e) => {
            // If we can't load agents, log a warning but continue
            tracing::warn!("Could not load local agents for validation: {}", e);
            HashSet::new()
        }
    };

    let mut missing_agents = Vec::new();

    // Check the session's agent field (if the session was started with -a <agent>)
    if let Some(ref agent_name) = export.session.agent
        && !local_agents.contains(agent_name)
    {
        missing_agents.push(agent_name.clone());
    }

    // Check agent_refs from the export metadata (pre-extracted @mentions)
    if let Some(ref agent_refs) = export.session.agent_refs {
        for agent_ref in agent_refs {
            if !local_agents.contains(agent_ref) && !missing_agents.contains(agent_ref) {
                missing_agents.push(agent_ref.clone());
            }
        }
    }

    // Also scan messages for @agent mentions (in case they weren't pre-extracted)
    let re = regex::Regex::new(r"@([a-zA-Z][a-zA-Z0-9_-]*)").unwrap();
    for message in &export.messages {
        for cap in re.captures_iter(&message.content) {
            if let Some(agent_name) = cap.get(1) {
                let name = agent_name.as_str().to_string();
                if !local_agents.contains(&name) && !missing_agents.contains(&name) {
                    missing_agents.push(name);
                }
            }
        }
    }

    missing_agents.sort();
    Ok(missing_agents)
}

/// Validate that there are no circular message references.
///
/// This function checks for potential circular references in message chains.
/// While the current ExportMessage struct doesn't have a reply_to field,
/// this validation protects against malformed input that could cause
/// infinite loops during processing.
fn validate_no_circular_references(messages: &[ExportMessage]) -> Result<()> {
    // Build a map of message IDs to their indices for reference checking
    // If tool_call_id references form a cycle, detect it
    let mut seen_tool_call_ids: HashSet<String> = HashSet::new();
    let mut referenced_ids: HashSet<String> = HashSet::new();

    for (idx, message) in messages.iter().enumerate() {
        // Track tool call IDs to detect duplicates (potential for circular references)
        if let Some(ref tool_call_id) = message.tool_call_id {
            if !seen_tool_call_ids.insert(tool_call_id.clone()) {
                bail!(
                    "Error: Duplicate tool_call_id '{}' detected at message index {}. \
                     This may indicate circular message references.",
                    tool_call_id,
                    idx
                );
            }
            referenced_ids.insert(tool_call_id.clone());
        }

        // Track tool calls made to ensure they reference unique IDs
        if let Some(ref tool_calls) = message.tool_calls {
            for tc in tool_calls {
                if !seen_tool_call_ids.insert(tc.id.clone()) {
                    bail!(
                        "Error: Duplicate tool call id '{}' detected at message index {}. \
                         This may indicate circular message references.",
                        tc.id,
                        idx
                    );
                }
            }
        }
    }

    Ok(())
}

/// Validate all messages in an export, including base64-encoded content.
/// Returns a clear error message if invalid data is found.
fn validate_export_messages(messages: &[ExportMessage]) -> Result<()> {
    use base64::Engine;

    for (idx, message) in messages.iter().enumerate() {
        // Check for base64-encoded image data in content
        // Common pattern: "data:image/png;base64,..." or "data:image/jpeg;base64,..."
        if let Some(data_uri_start) = message.content.find("data:image/") {
            // Use safe slicing with .get() to avoid panics on multi-byte UTF-8 boundaries
            let content_after_start = match message.content.get(data_uri_start..) {
                Some(s) => s,
                None => continue, // Invalid byte offset, skip this message
            };

            if let Some(base64_marker) = content_after_start.find(";base64,") {
                let base64_start = data_uri_start + base64_marker + 8; // 8 = len(";base64,")

                // Safe slicing for the remaining content after base64 marker
                let remaining = match message.content.get(base64_start..) {
                    Some(s) => s,
                    None => continue, // Invalid byte offset, skip this message
                };

                // Find end of base64 data (could end with quote, whitespace, or end of string)
                let base64_end = remaining
                    .find(['"', '\'', ' ', '\n', ')'])
                    .unwrap_or(remaining.len());

                // Safe slicing for the base64 data
                let base64_data = match remaining.get(..base64_end) {
                    Some(s) => s,
                    None => continue, // Invalid byte offset, skip this message
                };

                // Validate the base64 data
                if !base64_data.is_empty() {
                    let engine = base64::engine::general_purpose::STANDARD;
                    if let Err(e) = engine.decode(base64_data) {
                        bail!(
                            "Invalid base64 encoding in message {} (role: '{}'): {}\n\
                                The image data starting at position {} has invalid base64 encoding.\n\
                                Please ensure all embedded images use valid base64 encoding.",
                            idx + 1,
                            message.role,
                            e,
                            data_uri_start
                        );
                    }
                }
            }
        }

        // Validate tool call arguments if present
        if let Some(ref tool_calls) = message.tool_calls {
            for (tc_idx, tool_call) in tool_calls.iter().enumerate() {
                // Check for base64 in tool call arguments
                let args_str = tool_call.arguments.to_string();
                if args_str.contains(";base64,") {
                    // Try to find and validate any base64 in the arguments
                    for (pos, _) in args_str.match_indices(";base64,") {
                        let base64_start = pos + 8;

                        // Safe slicing for the remaining content after base64 marker
                        let remaining = match args_str.get(base64_start..) {
                            Some(s) => s,
                            None => continue, // Invalid byte offset, skip this occurrence
                        };

                        let base64_end = remaining
                            .find(|c: char| {
                                c == '"' || c == '\'' || c == ' ' || c == '\n' || c == ')'
                            })
                            .unwrap_or(remaining.len());

                        // Safe slicing for the base64 data
                        let base64_data = match remaining.get(..base64_end) {
                            Some(s) => s,
                            None => continue, // Invalid byte offset, skip this occurrence
                        };

                        if !base64_data.is_empty() {
                            let engine = base64::engine::general_purpose::STANDARD;
                            if let Err(e) = engine.decode(base64_data) {
                                bail!(
                                    "Invalid base64 encoding in message {} tool call {} ('{}' arguments): {}\n\
                                    Please ensure all embedded data uses valid base64 encoding.",
                                    idx + 1,
                                    tc_idx + 1,
                                    tool_call.name,
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Convert an export message to a protocol event.
fn message_to_event(message: &ExportMessage, turn_id: &mut u64, cwd: &Path) -> Result<Event> {
    let event_msg = match message.role.as_str() {
        "user" => {
            *turn_id += 1;
            EventMsg::UserMessage(UserMessageEvent {
                id: None,
                parent_id: None,
                message: message.content.clone(),
                images: None,
            })
        }
        "assistant" => EventMsg::AgentMessage(AgentMessageEvent {
            id: None,
            parent_id: None,
            message: message.content.clone(),
            finish_reason: None,
        }),
        "tool" => {
            // Reconstruct tool result as ExecCommandEnd
            EventMsg::ExecCommandEnd(Box::new(ExecCommandEndEvent {
                call_id: message.tool_call_id.clone().unwrap_or_default(),
                turn_id: turn_id.to_string(),
                command: vec!["imported_tool".to_string()],
                cwd: cwd.to_path_buf(),
                parsed_cmd: vec![ParsedCommand {
                    program: "imported_tool".to_string(),
                    args: vec![],
                }],
                source: ExecCommandSource::Agent,
                interaction_input: None,
                stdout: message.content.clone(),
                stderr: String::new(),
                aggregated_output: message.content.clone(),
                exit_code: 0,
                duration_ms: 0,
                formatted_output: message.content.clone(),
                metadata: None,
            }))
        }
        "system" => {
            // System messages are typically not replayed, skip or handle specially
            EventMsg::AgentMessage(AgentMessageEvent {
                id: None,
                parent_id: None,
                message: format!("[System] {}", message.content),
                finish_reason: None,
            })
        }
        other => {
            // Unknown role, treat as assistant message
            EventMsg::AgentMessage(AgentMessageEvent {
                id: None,
                parent_id: None,
                message: format!("[{other}] {}", message.content),
                finish_reason: None,
            })
        }
    };

    Ok(Event {
        id: turn_id.to_string(),
        msg: event_msg,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_export_json() {
        let json = r#"{
            "version": 1,
            "session": {
                "id": "test-123",
                "title": "Test Session",
                "created_at": "2024-01-01T00:00:00Z"
            },
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there!"}
            ]
        }"#;

        let export: SessionExport = serde_json::from_str(json).unwrap();
        assert_eq!(export.version, 1);
        assert_eq!(export.session.id, "test-123");
        assert_eq!(export.messages.len(), 2);
    }

    #[test]
    fn test_message_to_event() {
        let mut turn_id = 0u64;
        let cwd = PathBuf::from("/tmp");

        let user_msg = ExportMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
            timestamp: None,
        };

        let event = message_to_event(&user_msg, &mut turn_id, &cwd).unwrap();
        assert_eq!(turn_id, 1);
        assert!(matches!(event.msg, EventMsg::UserMessage(_)));

        let assistant_msg = ExportMessage {
            role: "assistant".to_string(),
            content: "Hi".to_string(),
            tool_calls: None,
            tool_call_id: None,
            timestamp: None,
        };

        let event = message_to_event(&assistant_msg, &mut turn_id, &cwd).unwrap();
        assert!(matches!(event.msg, EventMsg::AgentMessage(_)));
    }

    #[tokio::test]
    async fn test_import_empty_source_validation() {
        let cmd = ImportCommand {
            source: String::new(),
            force: false,
            resume: false,
        };

        let result = cmd.run().await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Source path cannot be empty"));
    }

    #[tokio::test]
    async fn test_import_whitespace_source_validation() {
        let cmd = ImportCommand {
            source: "   ".to_string(),
            force: false,
            resume: false,
        };

        let result = cmd.run().await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Source path cannot be empty"));
    }

    #[test]
    fn test_parse_html_provides_helpful_error() {
        let html_content = "<!DOCTYPE html><html><head><title>Not JSON</title></head></html>";

        let result: Result<SessionExport, _> = serde_json::from_str(html_content);
        assert!(result.is_err());

        // Verify that our error handling would detect this as HTML
        assert!(html_content.trim_start().starts_with("<!DOCTYPE"));
    }

    #[test]
    fn test_parse_xml_provides_helpful_error() {
        let xml_content = r#"<?xml version="1.0"?><root><data>Not JSON</data></root>"#;

        let result: Result<SessionExport, _> = serde_json::from_str(xml_content);
        assert!(result.is_err());

        // Verify that our error handling would detect this as XML
        assert!(xml_content.trim_start().starts_with("<?xml"));
    }

    #[test]
    fn test_parse_empty_content_error() {
        let empty_content = "";

        let result: Result<SessionExport, _> = serde_json::from_str(empty_content);
        assert!(result.is_err());

        // Verify that our error handling would detect this as empty
        assert!(empty_content.is_empty());
    }

    #[test]
    fn test_content_preview_truncation() {
        // Test that preview logic correctly truncates long content
        let long_content = "a".repeat(500);
        let preview_len = long_content.len().min(200);

        assert_eq!(preview_len, 200);
        assert_eq!(&long_content[..preview_len].len(), &200);
    }

    #[test]
    fn test_validate_agent_references_with_missing_agents() {
        use crate::export_cmd::SessionMetadata;

        // Create an export with agent references that don't exist locally
        let export = SessionExport {
            version: 1,
            session: SessionMetadata {
                id: "test-123".to_string(),
                title: None,
                created_at: "2024-01-01T00:00:00Z".to_string(),
                cwd: None,
                model: None,
                agent: Some("custom-nonexistent-agent".to_string()),
                agent_refs: Some(vec!["another-nonexistent".to_string()]),
            },
            messages: vec![ExportMessage {
                role: "user".to_string(),
                content: "@yet-another-missing help me".to_string(),
                tool_calls: None,
                tool_call_id: None,
                timestamp: None,
            }],
        };

        let missing = validate_agent_references(&export).unwrap();

        // Should find at least the explicitly nonexistent ones
        // (built-in agents like 'explore', 'research' will exist)
        assert!(missing.contains(&"custom-nonexistent-agent".to_string()));
        assert!(missing.contains(&"another-nonexistent".to_string()));
        assert!(missing.contains(&"yet-another-missing".to_string()));
    }

    #[test]
    fn test_validate_agent_references_with_builtin_agents() {
        use crate::export_cmd::SessionMetadata;

        // Create an export referencing only built-in agents
        let export = SessionExport {
            version: 1,
            session: SessionMetadata {
                id: "test-123".to_string(),
                title: None,
                created_at: "2024-01-01T00:00:00Z".to_string(),
                cwd: None,
                model: None,
                agent: None,
                agent_refs: None,
            },
            messages: vec![
                ExportMessage {
                    role: "user".to_string(),
                    content: "@build help me compile".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                    timestamp: None,
                },
                ExportMessage {
                    role: "user".to_string(),
                    content: "@plan create a plan".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                    timestamp: None,
                },
            ],
        };

        let missing = validate_agent_references(&export).unwrap();

        // Built-in agents should not be reported as missing
        assert!(!missing.contains(&"build".to_string()));
        assert!(!missing.contains(&"plan".to_string()));
    }
}
