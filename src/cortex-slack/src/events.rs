//! Event handling for Slack events.
//!
//! Handles various Slack events:
//! - `app_mention` - When the bot is @mentioned
//! - `message.im` - Direct messages to the bot
//! - `message.channels` - Messages in channels (if bot is member)
//!
//! Events are received via Socket Mode WebSocket connection.

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::{SlackError, SlackResult};

/// Slack event types that we handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SlackEvent {
    /// App mention event (@cortex in a channel).
    AppMention(AppMentionEvent),
    /// Direct message event.
    Message(MessageEvent),
    /// Unknown event type (for forward compatibility).
    #[serde(other)]
    Unknown,
}

/// Event payload for app mentions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppMentionEvent {
    /// User who mentioned the bot.
    pub user: String,
    /// Text of the message (including the mention).
    pub text: String,
    /// Channel where the mention occurred.
    pub channel: String,
    /// Timestamp of the message.
    pub ts: String,
    /// Thread timestamp (if in a thread).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
    /// Event timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_ts: Option<String>,
}

/// Event payload for messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEvent {
    /// User who sent the message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Text of the message.
    #[serde(default)]
    pub text: String,
    /// Channel where the message was sent.
    pub channel: String,
    /// Channel type (im, channel, group, mpim).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_type: Option<String>,
    /// Timestamp of the message.
    pub ts: String,
    /// Thread timestamp (if in a thread).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
    /// Event timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_ts: Option<String>,
    /// Subtype of message (e.g., "bot_message").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
    /// Bot ID (if message is from a bot).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
}

impl MessageEvent {
    /// Check if this is a direct message.
    pub fn is_direct_message(&self) -> bool {
        self.channel.starts_with('D') || self.channel_type.as_deref() == Some("im")
    }

    /// Check if this is a bot message (should be ignored).
    pub fn is_bot_message(&self) -> bool {
        self.bot_id.is_some() || self.subtype.as_deref() == Some("bot_message")
    }

    /// Check if this is a message change/edit event.
    pub fn is_message_changed(&self) -> bool {
        self.subtype.as_deref() == Some("message_changed")
    }

    /// Check if this is a message deletion event.
    pub fn is_message_deleted(&self) -> bool {
        self.subtype.as_deref() == Some("message_deleted")
    }
}

/// Socket Mode envelope wrapping events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocketModeEnvelope {
    /// Envelope ID for acknowledgment.
    pub envelope_id: String,
    /// Type of payload.
    #[serde(rename = "type")]
    pub envelope_type: String,
    /// Actual payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<EventPayload>,
    /// Accepts response payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepts_response_payload: Option<bool>,
}

/// Event callback payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventPayload {
    /// Event token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Team ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    /// API app ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_app_id: Option<String>,
    /// The actual event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<serde_json::Value>,
    /// Event type.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub payload_type: Option<String>,
    /// Event ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// Event time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_time: Option<u64>,
}

/// Socket Mode acknowledgment response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocketModeAck {
    /// Envelope ID being acknowledged.
    pub envelope_id: String,
    /// Optional response payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

impl SocketModeAck {
    /// Create a simple acknowledgment.
    pub fn new(envelope_id: impl Into<String>) -> Self {
        Self {
            envelope_id: envelope_id.into(),
            payload: None,
        }
    }

    /// Create an acknowledgment with a response payload.
    pub fn with_payload(envelope_id: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            envelope_id: envelope_id.into(),
            payload: Some(payload),
        }
    }
}

/// Extract the prompt from a mention text (removes the @mention part).
///
/// # Example
///
/// ```rust
/// use cortex_slack::events::extract_prompt;
///
/// let text = "<@U12345> review this PR";
/// let prompt = extract_prompt(text);
/// assert_eq!(prompt, "review this PR");
/// ```
pub fn extract_prompt(text: &str) -> String {
    // Pattern: <@USER_ID> or <@USER_ID|username>
    let mut result = text.to_string();

    // Remove user mentions: <@U...> or <@U...|name>
    while let Some(start) = result.find("<@") {
        if let Some(end) = result[start..].find('>') {
            result = format!("{}{}", &result[..start], &result[start + end + 1..]);
        } else {
            break;
        }
    }

    // Trim whitespace
    result.trim().to_string()
}

/// Context for processing an event.
#[derive(Debug, Clone)]
pub struct EventContext {
    /// User ID who triggered the event.
    pub user_id: String,
    /// Channel ID where the event occurred.
    pub channel_id: String,
    /// Thread timestamp (for replying in thread).
    pub thread_ts: Option<String>,
    /// Original message timestamp.
    pub message_ts: String,
    /// Team ID.
    pub team_id: Option<String>,
}

impl EventContext {
    /// Create context from an app mention event.
    pub fn from_app_mention(event: &AppMentionEvent, team_id: Option<String>) -> Self {
        Self {
            user_id: event.user.clone(),
            channel_id: event.channel.clone(),
            // If there's a thread_ts, use it; otherwise use the message ts to start a thread
            thread_ts: event.thread_ts.clone().or_else(|| Some(event.ts.clone())),
            message_ts: event.ts.clone(),
            team_id,
        }
    }

    /// Create context from a message event.
    pub fn from_message(event: &MessageEvent, team_id: Option<String>) -> Option<Self> {
        let user_id = event.user.clone()?;

        Some(Self {
            user_id,
            channel_id: event.channel.clone(),
            thread_ts: event.thread_ts.clone(),
            message_ts: event.ts.clone(),
            team_id,
        })
    }
}

/// Trait for handling Slack events.
#[async_trait::async_trait]
pub trait SlackEventHandler: Send + Sync {
    /// Handle an app mention event.
    async fn handle_app_mention(
        &self,
        event: AppMentionEvent,
        context: EventContext,
    ) -> SlackResult<Option<String>>;

    /// Handle a direct message event.
    async fn handle_direct_message(
        &self,
        event: MessageEvent,
        context: EventContext,
    ) -> SlackResult<Option<String>>;
}

/// Parse a raw event from the Socket Mode envelope.
pub fn parse_event(payload: &EventPayload) -> SlackResult<SlackEvent> {
    let event_json = payload
        .event
        .as_ref()
        .ok_or_else(|| SlackError::InvalidPayload("Missing event field".to_string()))?;

    // Get event type
    let event_type = event_json
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("unknown");

    debug!("Parsing event type: {}", event_type);

    match event_type {
        "app_mention" => {
            let event: AppMentionEvent = serde_json::from_value(event_json.clone())?;
            Ok(SlackEvent::AppMention(event))
        }
        "message" => {
            let event: MessageEvent = serde_json::from_value(event_json.clone())?;
            Ok(SlackEvent::Message(event))
        }
        _ => {
            warn!("Unknown event type: {}", event_type);
            Ok(SlackEvent::Unknown)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_prompt_simple() {
        let text = "<@U12345> review this PR";
        assert_eq!(extract_prompt(text), "review this PR");
    }

    #[test]
    fn test_extract_prompt_with_username() {
        let text = "<@U12345|cortex> help me with this code";
        assert_eq!(extract_prompt(text), "help me with this code");
    }

    #[test]
    fn test_extract_prompt_multiple_mentions() {
        let text = "<@U12345> <@U67890> check this";
        assert_eq!(extract_prompt(text), "check this");
    }

    #[test]
    fn test_extract_prompt_no_mention() {
        let text = "just a regular message";
        assert_eq!(extract_prompt(text), "just a regular message");
    }

    #[test]
    fn test_message_event_is_direct_message() {
        let event = MessageEvent {
            user: Some("U12345".to_string()),
            text: "hello".to_string(),
            channel: "D12345".to_string(),
            channel_type: Some("im".to_string()),
            ts: "1234567890.123456".to_string(),
            thread_ts: None,
            event_ts: None,
            subtype: None,
            bot_id: None,
        };
        assert!(event.is_direct_message());

        let channel_event = MessageEvent {
            channel: "C12345".to_string(),
            channel_type: Some("channel".to_string()),
            ..event.clone()
        };
        assert!(!channel_event.is_direct_message());
    }

    #[test]
    fn test_message_event_is_bot_message() {
        let bot_event = MessageEvent {
            user: None,
            text: "bot message".to_string(),
            channel: "C12345".to_string(),
            channel_type: None,
            ts: "1234567890.123456".to_string(),
            thread_ts: None,
            event_ts: None,
            subtype: Some("bot_message".to_string()),
            bot_id: Some("B12345".to_string()),
        };
        assert!(bot_event.is_bot_message());

        let user_event = MessageEvent {
            user: Some("U12345".to_string()),
            bot_id: None,
            subtype: None,
            ..bot_event.clone()
        };
        assert!(!user_event.is_bot_message());
    }

    #[test]
    fn test_socket_mode_ack() {
        let ack = SocketModeAck::new("env-123");
        assert_eq!(ack.envelope_id, "env-123");
        assert!(ack.payload.is_none());

        let ack_with_payload =
            SocketModeAck::with_payload("env-456", serde_json::json!({"text": "response"}));
        assert_eq!(ack_with_payload.envelope_id, "env-456");
        assert!(ack_with_payload.payload.is_some());
    }

    #[test]
    fn test_event_context_from_app_mention() {
        let event = AppMentionEvent {
            user: "U12345".to_string(),
            text: "<@B00000> hello".to_string(),
            channel: "C67890".to_string(),
            ts: "1234567890.123456".to_string(),
            thread_ts: None,
            event_ts: Some("1234567890.123457".to_string()),
        };

        let ctx = EventContext::from_app_mention(&event, Some("T11111".to_string()));

        assert_eq!(ctx.user_id, "U12345");
        assert_eq!(ctx.channel_id, "C67890");
        assert_eq!(ctx.message_ts, "1234567890.123456");
        // Should use message ts as thread_ts since there's no existing thread
        assert_eq!(ctx.thread_ts, Some("1234567890.123456".to_string()));
    }

    #[test]
    fn test_event_context_from_message() {
        let event = MessageEvent {
            user: Some("U12345".to_string()),
            text: "hello".to_string(),
            channel: "D67890".to_string(),
            channel_type: Some("im".to_string()),
            ts: "1234567890.123456".to_string(),
            thread_ts: Some("1234567890.000000".to_string()),
            event_ts: None,
            subtype: None,
            bot_id: None,
        };

        let ctx = EventContext::from_message(&event, None).unwrap();

        assert_eq!(ctx.user_id, "U12345");
        assert_eq!(ctx.channel_id, "D67890");
        assert_eq!(ctx.thread_ts, Some("1234567890.000000".to_string()));
    }

    #[test]
    fn test_event_context_from_message_no_user() {
        let event = MessageEvent {
            user: None,
            text: "bot message".to_string(),
            channel: "C12345".to_string(),
            channel_type: None,
            ts: "1234567890.123456".to_string(),
            thread_ts: None,
            event_ts: None,
            subtype: Some("bot_message".to_string()),
            bot_id: Some("B12345".to_string()),
        };

        let ctx = EventContext::from_message(&event, None);
        assert!(ctx.is_none());
    }
}
