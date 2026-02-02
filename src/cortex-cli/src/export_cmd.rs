//! Session export command for Cortex CLI.
//!
//! Exports a session to a portable JSON format that can be shared or imported.

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use cortex_engine::list_sessions;
use cortex_engine::rollout::get_rollout_path;
use cortex_engine::rollout::reader::{RolloutItem, get_session_meta, read_rollout};
use cortex_protocol::{ConversationId, EventMsg};

/// Export format for sessions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ExportFormat {
    /// JSON format (default)
    #[default]
    Json,
    /// YAML format
    Yaml,
    /// CSV format (simplified, messages only)
    Csv,
}

/// Export a session to JSON format.
#[derive(Debug, Parser)]
pub struct ExportCommand {
    /// Session ID to export (interactive picker if not provided)
    #[arg(value_name = "SESSION_ID")]
    pub session_id: Option<String>,

    /// Output file path (stdout if not specified)
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Output format (json, yaml, csv)
    #[arg(short, long, value_enum, default_value_t = ExportFormat::Json)]
    pub format: ExportFormat,

    /// Pretty-print the output (for json/yaml)
    #[arg(long, default_value_t = true)]
    pub pretty: bool,
}

/// Portable session export format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExport {
    /// Export format version.
    pub version: u32,
    /// Session metadata.
    pub session: SessionMetadata,
    /// Conversation messages.
    pub messages: Vec<ExportMessage>,
}

/// Session metadata in export format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Session ID.
    pub id: String,
    /// Session title (derived from first user message or cwd).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Creation timestamp (ISO 8601).
    pub created_at: String,
    /// Working directory where session was created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Model used for the session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Agent used for the session (if any).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent: Option<String>,
    /// Agent references found in messages (for validation during import).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_refs: Option<Vec<String>>,
}

/// Message in export format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportMessage {
    /// Message role (user, assistant, system, tool).
    pub role: String,
    /// Message content.
    pub content: String,
    /// Tool calls made by the assistant (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ExportToolCall>>,
    /// Tool call ID this message is responding to (for tool results).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Message timestamp (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// Tool call in export format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportToolCall {
    /// Tool call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Tool arguments as JSON.
    pub arguments: serde_json::Value,
}

impl ExportCommand {
    /// Run the export command.
    pub async fn run(self) -> Result<()> {
        let cortex_home = dirs::home_dir()
            .map(|h| h.join(".cortex"))
            .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

        // Get session ID - either from arg or interactive picker
        let session_id = match self.session_id {
            Some(id) => id,
            None => select_session(&cortex_home).await?,
        };

        // Validate session ID is not empty
        if session_id.trim().is_empty() {
            bail!("Session ID cannot be empty");
        }

        // Parse conversation ID - accept both full UUID and short 8-char prefix
        let conversation_id: ConversationId = match session_id.parse() {
            Ok(id) => id,
            Err(_) => {
                // If parsing failed, check if it's a short ID (8 chars)
                if session_id.len() == 8 {
                    // Try to find a session with matching prefix
                    let sessions = list_sessions(&cortex_home)?;
                    let matching: Vec<_> = sessions
                        .iter()
                        .filter(|s| s.id.starts_with(&session_id))
                        .collect();

                    match matching.len() {
                        0 => bail!("No session found with ID prefix: {session_id}"),
                        1 => matching[0].id.parse().map_err(|_| {
                            anyhow::anyhow!("Internal error: invalid session ID format")
                        })?,
                        _ => bail!(
                            "Ambiguous session ID prefix '{}' matches {} sessions. Please provide more characters.",
                            session_id,
                            matching.len()
                        ),
                    }
                } else {
                    bail!(
                        "Invalid session ID format: {session_id}. Expected full UUID or 8-character prefix."
                    );
                }
            }
        };

        // Read rollout file
        let rollout_path = get_rollout_path(&cortex_home, &conversation_id);
        if !rollout_path.exists() {
            bail!("Session not found: {session_id}");
        }

        let entries = read_rollout(&rollout_path)
            .with_context(|| format!("Failed to read session: {}", rollout_path.display()))?;

        // Extract metadata
        let meta = get_session_meta(&entries);

        // Extract messages from events
        let messages = extract_messages(&entries);

        // Extract agent references from messages (@agent mentions)
        let agent_refs = extract_agent_refs(&messages);

        let session_meta = SessionMetadata {
            id: conversation_id.to_string(),
            title: derive_title(&entries),
            created_at: meta
                .map(|m| m.timestamp.clone())
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
            cwd: meta.map(|m| m.cwd.clone()),
            model: meta.and_then(|m| m.model.clone()),
            agent: None, // Agent extraction from session config planned
            agent_refs: if agent_refs.is_empty() {
                None
            } else {
                Some(agent_refs)
            },
        };

        // Build export
        let export = SessionExport {
            version: 1,
            session: session_meta,
            messages: messages.clone(),
        };

        // Serialize to the requested format
        let output_content = match self.format {
            ExportFormat::Json => {
                if self.pretty {
                    serde_json::to_string_pretty(&export)?
                } else {
                    serde_json::to_string(&export)?
                }
            }
            ExportFormat::Yaml => {
                serde_yaml::to_string(&export).with_context(|| "Failed to serialize to YAML")?
            }
            ExportFormat::Csv => {
                // CSV format: simplified, messages only
                let mut csv_output = String::new();
                csv_output.push_str("timestamp,role,content\n");
                for msg in &messages {
                    let timestamp = msg.timestamp.as_deref().unwrap_or("");
                    // Escape CSV content: double quotes, wrap in quotes if contains comma/newline
                    let content = escape_csv_field(&msg.content);
                    csv_output.push_str(&format!("{},{},{}\n", timestamp, msg.role, content));
                }
                csv_output
            }
        };

        // Write to output
        match self.output {
            Some(path) => {
                std::fs::write(&path, &output_content)
                    .with_context(|| format!("Failed to write to: {}", path.display()))?;
                eprintln!("Exported session to: {}", path.display());
            }
            None => {
                println!("{output_content}");
            }
        }

        Ok(())
    }
}

/// Escape a field for CSV output.
fn escape_csv_field(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') {
        // Escape double quotes by doubling them, wrap in quotes
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// Select a session interactively.
async fn select_session(cortex_home: &PathBuf) -> Result<String> {
    let sessions = list_sessions(cortex_home)?;

    if sessions.is_empty() {
        bail!("No sessions found. Create a session first.");
    }

    // For non-interactive mode, just pick the most recent
    // In a full TUI implementation, this would show an interactive picker
    println!("Available sessions:");
    println!("{:-<80}", "");

    for (i, session) in sessions.iter().take(10).enumerate() {
        let date = if session.timestamp.len() >= 19 {
            session.timestamp[..19].replace('T', " ")
        } else {
            session.timestamp.clone()
        };
        let model = session.model.as_deref().unwrap_or("unknown");
        println!(
            "{:>2}. {} | {} | {} msgs | {}",
            i + 1,
            &session.id[..8.min(session.id.len())],
            date,
            session.message_count,
            model,
        );
    }

    if sessions.len() > 10 {
        println!("\n... and {} more sessions", sessions.len() - 10);
    }

    println!("\nUsing most recent session: {}", sessions[0].id);
    Ok(sessions[0].id.clone())
}

/// Derive a title from the session content.
fn derive_title(entries: &[cortex_engine::rollout::reader::RolloutEntry]) -> Option<String> {
    // Try to get the first user message as the title
    for entry in entries {
        if let RolloutItem::EventMsg(EventMsg::UserMessage(msg)) = &entry.item {
            let title = msg.message.chars().take(60).collect::<String>();
            return Some(if msg.message.len() > 60 {
                format!("{}...", title)
            } else {
                title
            });
        }
    }
    None
}

/// Extract messages from rollout entries.
fn extract_messages(
    entries: &[cortex_engine::rollout::reader::RolloutEntry],
) -> Vec<ExportMessage> {
    let mut messages = Vec::new();

    for entry in entries {
        match &entry.item {
            RolloutItem::EventMsg(EventMsg::UserMessage(msg)) => {
                messages.push(ExportMessage {
                    role: "user".to_string(),
                    content: msg.message.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                    timestamp: Some(entry.timestamp.clone()),
                });
            }
            RolloutItem::EventMsg(EventMsg::AgentMessage(msg)) => {
                messages.push(ExportMessage {
                    role: "assistant".to_string(),
                    content: msg.message.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                    timestamp: Some(entry.timestamp.clone()),
                });
            }
            RolloutItem::EventMsg(EventMsg::ExecCommandEnd(exec)) => {
                // Include tool outputs as separate messages
                let _tool_name = exec
                    .command
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                messages.push(ExportMessage {
                    role: "tool".to_string(),
                    content: exec.formatted_output.clone(),
                    tool_calls: None,
                    tool_call_id: Some(exec.call_id.clone()),
                    timestamp: Some(entry.timestamp.clone()),
                });
            }
            _ => {}
        }
    }

    messages
}

/// Extract agent references (@mentions) from messages.
/// Returns a deduplicated list of agent names that are referenced.
fn extract_agent_refs(messages: &[ExportMessage]) -> Vec<String> {
    use std::collections::HashSet;

    // Regex to match @agent mentions (e.g., @explore, @general, @my-custom-agent)
    let re = regex::Regex::new(r"@([a-zA-Z][a-zA-Z0-9_-]*)").unwrap();

    let mut agent_refs: HashSet<String> = HashSet::new();

    for message in messages {
        for cap in re.captures_iter(&message.content) {
            if let Some(agent_name) = cap.get(1) {
                agent_refs.insert(agent_name.as_str().to_string());
            }
        }
    }

    let mut refs: Vec<String> = agent_refs.into_iter().collect();
    refs.sort();
    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_export_serialization() {
        let export = SessionExport {
            version: 1,
            session: SessionMetadata {
                id: "test-id".to_string(),
                title: Some("Test Session".to_string()),
                created_at: "2024-01-01T00:00:00Z".to_string(),
                cwd: Some("/home/user".to_string()),
                model: Some("claude-3".to_string()),
                agent: None,
                agent_refs: None,
            },
            messages: vec![
                ExportMessage {
                    role: "user".to_string(),
                    content: "Hello".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                    timestamp: Some("2024-01-01T00:00:01Z".to_string()),
                },
                ExportMessage {
                    role: "assistant".to_string(),
                    content: "Hi there!".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                    timestamp: Some("2024-01-01T00:00:02Z".to_string()),
                },
            ],
        };

        let json = serde_json::to_string_pretty(&export).unwrap();
        assert!(json.contains("\"version\": 1"));
        assert!(json.contains("\"role\": \"user\""));
        assert!(json.contains("\"content\": \"Hello\""));
    }

    #[test]
    fn test_extract_agent_refs() {
        let messages = vec![
            ExportMessage {
                role: "user".to_string(),
                content: "@explore find the main function".to_string(),
                tool_calls: None,
                tool_call_id: None,
                timestamp: None,
            },
            ExportMessage {
                role: "user".to_string(),
                content: "@research analyze this code @my-agent".to_string(),
                tool_calls: None,
                tool_call_id: None,
                timestamp: None,
            },
        ];

        let refs = extract_agent_refs(&messages);
        assert_eq!(refs.len(), 3);
        assert!(refs.contains(&"explore".to_string()));
        assert!(refs.contains(&"research".to_string()));
        assert!(refs.contains(&"my-agent".to_string()));
    }

    #[test]
    fn test_extract_agent_refs_no_duplicates() {
        let messages = vec![
            ExportMessage {
                role: "user".to_string(),
                content: "@explore task 1".to_string(),
                tool_calls: None,
                tool_call_id: None,
                timestamp: None,
            },
            ExportMessage {
                role: "user".to_string(),
                content: "@explore task 2".to_string(),
                tool_calls: None,
                tool_call_id: None,
                timestamp: None,
            },
        ];

        let refs = extract_agent_refs(&messages);
        assert_eq!(refs.len(), 1);
        assert!(refs.contains(&"explore".to_string()));
    }
}
