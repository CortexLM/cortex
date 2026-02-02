//! Slack slash command handling.
//!
//! Supports the following slash commands:
//! - `/cortex <prompt>` - Run a Cortex prompt
//! - `/cortex-review <pr-url>` - Review a pull request
//!
//! Slash commands require HTTP endpoints (not Socket Mode) but can send
//! delayed responses via response_url.

use serde::{Deserialize, Serialize};
use tracing::{debug, error, warn};

use crate::error::{SlackError, SlackResult};

/// Slack slash command payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashCommandPayload {
    /// Command token (deprecated, use signing secret instead).
    #[serde(default)]
    pub token: String,
    /// Team ID.
    pub team_id: String,
    /// Team domain.
    #[serde(default)]
    pub team_domain: String,
    /// Enterprise ID (for Enterprise Grid).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enterprise_id: Option<String>,
    /// Enterprise name (for Enterprise Grid).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enterprise_name: Option<String>,
    /// Channel ID where command was invoked.
    pub channel_id: String,
    /// Channel name.
    #[serde(default)]
    pub channel_name: String,
    /// User ID who invoked the command.
    pub user_id: String,
    /// Username.
    #[serde(default)]
    pub user_name: String,
    /// The command (e.g., "/cortex").
    pub command: String,
    /// Text after the command.
    #[serde(default)]
    pub text: String,
    /// API app ID.
    #[serde(default)]
    pub api_app_id: String,
    /// Whether the channel is a shared channel.
    #[serde(default)]
    pub is_enterprise_install: bool,
    /// URL for delayed responses.
    pub response_url: String,
    /// Trigger ID for opening modals.
    pub trigger_id: String,
}

/// Response type for slash command responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ResponseType {
    /// Only visible to the user who invoked the command.
    #[default]
    Ephemeral,
    /// Visible to everyone in the channel.
    InChannel,
}

/// Immediate response to a slash command.
///
/// Must be sent within 3 seconds of receiving the command.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlashCommandResponse {
    /// Response type (ephemeral or in_channel).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_type: Option<ResponseType>,
    /// Simple text response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Block Kit blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<serde_json::Value>,
    /// Attachment (legacy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<serde_json::Value>>,
}

impl SlashCommandResponse {
    /// Create a new empty response.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a simple text response.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Default::default()
        }
    }

    /// Set response type to ephemeral (only visible to invoker).
    pub fn ephemeral(mut self) -> Self {
        self.response_type = Some(ResponseType::Ephemeral);
        self
    }

    /// Set response type to in_channel (visible to all).
    pub fn in_channel(mut self) -> Self {
        self.response_type = Some(ResponseType::InChannel);
        self
    }

    /// Set the text content.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Set Block Kit blocks.
    pub fn with_blocks(mut self, blocks: serde_json::Value) -> Self {
        self.blocks = Some(blocks);
        self
    }
}

/// Delayed response sent via response_url.
///
/// Can be sent up to 30 minutes after the original command.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DelayedResponse {
    /// Response type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_type: Option<ResponseType>,
    /// Whether to replace the original message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replace_original: Option<bool>,
    /// Whether to delete the original message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete_original: Option<bool>,
    /// Text content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Block Kit blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<serde_json::Value>,
}

impl DelayedResponse {
    /// Create a new delayed response.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set text content.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Set Block Kit blocks.
    pub fn with_blocks(mut self, blocks: serde_json::Value) -> Self {
        self.blocks = Some(blocks);
        self
    }

    /// Set response type to in_channel.
    pub fn in_channel(mut self) -> Self {
        self.response_type = Some(ResponseType::InChannel);
        self
    }

    /// Replace the original acknowledgment message.
    pub fn replace_original(mut self) -> Self {
        self.replace_original = Some(true);
        self
    }

    /// Delete the original acknowledgment message.
    pub fn delete_original(mut self) -> Self {
        self.delete_original = Some(true);
        self
    }
}

/// Send a delayed response to the response_url.
///
/// This should be called after the initial 3-second acknowledgment
/// to send the actual command result.
pub async fn send_delayed_response(
    client: &reqwest::Client,
    response_url: &str,
    response: &DelayedResponse,
) -> SlackResult<()> {
    debug!("Sending delayed response to: {}", response_url);

    let resp = client.post(response_url).json(response).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        error!("Failed to send delayed response: {} - {}", status, body);
        return Err(SlackError::Api(format!(
            "Failed to send delayed response: {} - {}",
            status, body
        )));
    }

    debug!("Delayed response sent successfully");
    Ok(())
}

/// Parsed slash command.
#[derive(Debug, Clone)]
pub enum ParsedCommand {
    /// /cortex <prompt> - Run a general prompt.
    Cortex {
        /// The prompt text.
        prompt: String,
        /// Context from the slash command.
        context: CommandContext,
    },
    /// /cortex-review <pr-url> - Review a PR.
    CortexReview {
        /// The PR URL to review.
        pr_url: String,
        /// Context from the slash command.
        context: CommandContext,
    },
    /// Unknown command.
    Unknown {
        /// The command name.
        command: String,
        /// The text after the command.
        text: String,
        /// Context from the slash command.
        context: CommandContext,
    },
}

/// Context information from a slash command.
#[derive(Debug, Clone)]
pub struct CommandContext {
    /// User ID who invoked the command.
    pub user_id: String,
    /// Channel ID where command was invoked.
    pub channel_id: String,
    /// Team ID.
    pub team_id: String,
    /// URL for delayed responses.
    pub response_url: String,
    /// Trigger ID for modals.
    pub trigger_id: String,
}

impl From<&SlashCommandPayload> for CommandContext {
    fn from(payload: &SlashCommandPayload) -> Self {
        Self {
            user_id: payload.user_id.clone(),
            channel_id: payload.channel_id.clone(),
            team_id: payload.team_id.clone(),
            response_url: payload.response_url.clone(),
            trigger_id: payload.trigger_id.clone(),
        }
    }
}

/// Parse a slash command payload into a structured command.
pub fn parse_command(payload: &SlashCommandPayload) -> ParsedCommand {
    let context = CommandContext::from(payload);
    let command = payload.command.to_lowercase();
    let text = payload.text.trim().to_string();

    match command.as_str() {
        "/cortex" => {
            if text.is_empty() {
                ParsedCommand::Cortex {
                    prompt: "What can you help me with?".to_string(),
                    context,
                }
            } else {
                ParsedCommand::Cortex {
                    prompt: text,
                    context,
                }
            }
        }
        "/cortex-review" => {
            if text.is_empty() {
                warn!("cortex-review command without PR URL");
                ParsedCommand::Unknown {
                    command: payload.command.clone(),
                    text: "Missing PR URL".to_string(),
                    context,
                }
            } else {
                // Validate it looks like a URL
                if text.starts_with("http://") || text.starts_with("https://") {
                    ParsedCommand::CortexReview {
                        pr_url: text,
                        context,
                    }
                } else {
                    // Try to be helpful - maybe they just gave a PR number
                    warn!("cortex-review command with non-URL text: {}", text);
                    ParsedCommand::CortexReview {
                        pr_url: text, // Let the handler deal with it
                        context,
                    }
                }
            }
        }
        _ => ParsedCommand::Unknown {
            command: payload.command.clone(),
            text,
            context,
        },
    }
}

/// Create an acknowledgment response for a command.
pub fn create_ack_response(command: &ParsedCommand) -> SlashCommandResponse {
    match command {
        ParsedCommand::Cortex { prompt, .. } => {
            let preview = if prompt.len() > 50 {
                format!("{}...", &prompt[..50])
            } else {
                prompt.clone()
            };
            SlashCommandResponse::text(format!("🔄 Processing: {}", preview)).in_channel()
        }
        ParsedCommand::CortexReview { pr_url, .. } => {
            SlashCommandResponse::text(format!("📝 Reviewing PR: {}", pr_url)).in_channel()
        }
        ParsedCommand::Unknown { command, .. } => {
            SlashCommandResponse::text(format!("❓ Unknown command: {}", command)).ephemeral()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_payload(command: &str, text: &str) -> SlashCommandPayload {
        SlashCommandPayload {
            token: "test-token".to_string(),
            team_id: "T12345".to_string(),
            team_domain: "test".to_string(),
            enterprise_id: None,
            enterprise_name: None,
            channel_id: "C67890".to_string(),
            channel_name: "general".to_string(),
            user_id: "U11111".to_string(),
            user_name: "testuser".to_string(),
            command: command.to_string(),
            text: text.to_string(),
            api_app_id: "A22222".to_string(),
            is_enterprise_install: false,
            response_url: "https://hooks.slack.com/commands/xxx".to_string(),
            trigger_id: "trigger123".to_string(),
        }
    }

    #[test]
    fn test_parse_cortex_command() {
        let payload = create_test_payload("/cortex", "help me with rust");
        let cmd = parse_command(&payload);

        match cmd {
            ParsedCommand::Cortex { prompt, context } => {
                assert_eq!(prompt, "help me with rust");
                assert_eq!(context.user_id, "U11111");
                assert_eq!(context.channel_id, "C67890");
            }
            _ => panic!("Expected Cortex command"),
        }
    }

    #[test]
    fn test_parse_cortex_empty() {
        let payload = create_test_payload("/cortex", "");
        let cmd = parse_command(&payload);

        match cmd {
            ParsedCommand::Cortex { prompt, .. } => {
                assert_eq!(prompt, "What can you help me with?");
            }
            _ => panic!("Expected Cortex command"),
        }
    }

    #[test]
    fn test_parse_cortex_review() {
        let payload = create_test_payload("/cortex-review", "https://github.com/org/repo/pull/123");
        let cmd = parse_command(&payload);

        match cmd {
            ParsedCommand::CortexReview { pr_url, .. } => {
                assert_eq!(pr_url, "https://github.com/org/repo/pull/123");
            }
            _ => panic!("Expected CortexReview command"),
        }
    }

    #[test]
    fn test_parse_unknown_command() {
        let payload = create_test_payload("/unknown", "some text");
        let cmd = parse_command(&payload);

        match cmd {
            ParsedCommand::Unknown { command, text, .. } => {
                assert_eq!(command, "/unknown");
                assert_eq!(text, "some text");
            }
            _ => panic!("Expected Unknown command"),
        }
    }

    #[test]
    fn test_slash_command_response() {
        let response = SlashCommandResponse::text("Hello!").in_channel();

        assert_eq!(response.text, Some("Hello!".to_string()));
        assert_eq!(response.response_type, Some(ResponseType::InChannel));
    }

    #[test]
    fn test_delayed_response() {
        let response = DelayedResponse::new()
            .with_text("Done!")
            .in_channel()
            .replace_original();

        assert_eq!(response.text, Some("Done!".to_string()));
        assert_eq!(response.response_type, Some(ResponseType::InChannel));
        assert_eq!(response.replace_original, Some(true));
    }

    #[test]
    fn test_create_ack_response_cortex() {
        let payload = create_test_payload("/cortex", "test prompt");
        let cmd = parse_command(&payload);
        let ack = create_ack_response(&cmd);

        assert!(ack.text.unwrap().contains("Processing"));
        assert_eq!(ack.response_type, Some(ResponseType::InChannel));
    }

    #[test]
    fn test_create_ack_response_review() {
        let payload = create_test_payload("/cortex-review", "https://github.com/test");
        let cmd = parse_command(&payload);
        let ack = create_ack_response(&cmd);

        assert!(ack.text.unwrap().contains("Reviewing PR"));
    }

    #[test]
    fn test_create_ack_response_unknown() {
        let payload = create_test_payload("/unknown", "text");
        let cmd = parse_command(&payload);
        let ack = create_ack_response(&cmd);

        assert!(ack.text.unwrap().contains("Unknown command"));
        assert_eq!(ack.response_type, Some(ResponseType::Ephemeral));
    }

    #[test]
    fn test_command_context_from_payload() {
        let payload = create_test_payload("/cortex", "test");
        let context = CommandContext::from(&payload);

        assert_eq!(context.user_id, "U11111");
        assert_eq!(context.channel_id, "C67890");
        assert_eq!(context.team_id, "T12345");
        assert!(context.response_url.contains("hooks.slack.com"));
    }
}
