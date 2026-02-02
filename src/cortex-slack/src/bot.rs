//! Main Slack bot implementation.
//!
//! The `CortexSlackBot` is the central component that:
//! - Connects to Slack via Socket Mode (WebSocket)
//! - Handles incoming events
//! - Sends messages and responses
//! - Manages the connection lifecycle
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_slack::{CortexSlackBot, SlackConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = SlackConfig::from_env()?;
//!     let bot = CortexSlackBot::new(config).await?;
//!     
//!     // Start the bot (runs forever)
//!     bot.start().await?;
//!     
//!     Ok(())
//! }
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures::stream::SplitStream;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio::time::interval;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::{debug, error, info, warn};

use crate::commands::{
    DelayedResponse, SlashCommandPayload, create_ack_response, parse_command, send_delayed_response,
};
use crate::config::SlackConfig;
use crate::error::{SlackApiError, SlackError, SlackResult};
use crate::events::{
    AppMentionEvent, EventContext, EventPayload, MessageEvent, SlackEvent, SlackEventHandler,
    SocketModeAck, SocketModeEnvelope, extract_prompt, parse_event,
};
use crate::messages::{SlackMessageContent, format_agent_response, format_error_response};

/// Type alias for the WebSocket connection.
type WsConnection = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Configuration for bot behavior.
#[derive(Debug, Clone)]
pub struct BotOptions {
    /// Timeout for API requests.
    pub api_timeout: Duration,
    /// Maximum retries for failed API calls.
    pub max_retries: u32,
    /// Delay between reconnection attempts.
    pub reconnect_delay: Duration,
    /// Ping interval for WebSocket keep-alive.
    pub ping_interval: Duration,
}

impl Default for BotOptions {
    fn default() -> Self {
        Self {
            api_timeout: Duration::from_secs(30),
            max_retries: 3,
            reconnect_delay: Duration::from_secs(5),
            ping_interval: Duration::from_secs(30),
        }
    }
}

/// The main Cortex Slack bot.
pub struct CortexSlackBot {
    /// Slack configuration.
    config: SlackConfig,
    /// HTTP client for API calls.
    client: reqwest::Client,
    /// Bot options.
    options: BotOptions,
    /// Bot's own user ID (set after connection).
    bot_user_id: Arc<RwLock<Option<String>>>,
    /// Shutdown signal sender.
    shutdown_tx: broadcast::Sender<()>,
    /// Event handler (optional).
    event_handler: Arc<RwLock<Option<Arc<dyn SlackEventHandler>>>>,
}

impl CortexSlackBot {
    /// Create a new Slack bot with the given configuration.
    pub async fn new(config: SlackConfig) -> SlackResult<Self> {
        config.validate()?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| SlackError::Internal(format!("Failed to create HTTP client: {}", e)))?;

        let (shutdown_tx, _) = broadcast::channel(1);

        Ok(Self {
            config,
            client,
            options: BotOptions::default(),
            bot_user_id: Arc::new(RwLock::new(None)),
            shutdown_tx,
            event_handler: Arc::new(RwLock::new(None)),
        })
    }

    /// Create a new bot with custom options.
    pub async fn with_options(config: SlackConfig, options: BotOptions) -> SlackResult<Self> {
        let mut bot = Self::new(config).await?;
        bot.options = options;
        Ok(bot)
    }

    /// Set the event handler for processing events.
    pub async fn set_event_handler<H: SlackEventHandler + 'static>(&self, handler: H) {
        let mut guard = self.event_handler.write().await;
        *guard = Some(Arc::new(handler));
    }

    /// Get the bot's user ID (if known).
    pub async fn bot_user_id(&self) -> Option<String> {
        self.bot_user_id.read().await.clone()
    }

    /// Start the bot and run until shutdown.
    pub async fn start(&self) -> SlackResult<()> {
        info!("Starting Cortex Slack bot...");

        // Test authentication
        self.test_auth().await?;

        // Run the Socket Mode connection loop
        self.run_socket_mode().await
    }

    /// Shutdown the bot gracefully.
    pub fn shutdown(&self) {
        info!("Shutting down Slack bot...");
        let _ = self.shutdown_tx.send(());
    }

    /// Test authentication by calling auth.test.
    async fn test_auth(&self) -> SlackResult<()> {
        debug!("Testing Slack authentication...");

        let response: serde_json::Value =
            self.api_call("auth.test", &serde_json::json!({})).await?;

        if response.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = response
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown");
            return Err(SlackError::Auth(format!("auth.test failed: {}", error)));
        }

        // Store bot user ID
        if let Some(user_id) = response.get("user_id").and_then(|v| v.as_str()) {
            let mut guard = self.bot_user_id.write().await;
            *guard = Some(user_id.to_string());
            info!("Authenticated as bot user: {}", user_id);
        }

        Ok(())
    }

    /// Run the Socket Mode connection loop.
    async fn run_socket_mode(&self) -> SlackResult<()> {
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        loop {
            // Get WebSocket URL
            let ws_url = self.get_socket_mode_url().await?;

            info!("Connecting to Socket Mode...");

            match self.connect_and_run(&ws_url, &mut shutdown_rx).await {
                Ok(()) => {
                    info!("Socket Mode connection closed gracefully");
                    break;
                }
                Err(e) => {
                    error!("Socket Mode connection error: {}", e);

                    // Check if we should shutdown
                    if shutdown_rx.try_recv().is_ok() {
                        break;
                    }

                    // Wait before reconnecting
                    info!("Reconnecting in {:?}...", self.options.reconnect_delay);
                    tokio::time::sleep(self.options.reconnect_delay).await;
                }
            }
        }

        Ok(())
    }

    /// Get the WebSocket URL for Socket Mode.
    async fn get_socket_mode_url(&self) -> SlackResult<String> {
        let response = self
            .client
            .post("https://slack.com/api/apps.connections.open")
            .header(
                "Authorization",
                format!("Bearer {}", self.config.app_token()),
            )
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await?;

        let json: serde_json::Value = response.json().await?;

        if json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = json
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown");
            return Err(SlackError::Api(format!(
                "apps.connections.open failed: {}",
                error
            )));
        }

        json.get("url")
            .and_then(|u| u.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| SlackError::Api("Missing url in response".to_string()))
    }

    /// Connect to WebSocket and run event loop.
    async fn connect_and_run(
        &self,
        ws_url: &str,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> SlackResult<()> {
        let (ws_stream, _) = connect_async(ws_url).await?;
        let (write, read) = ws_stream.split();

        let write = Arc::new(tokio::sync::Mutex::new(write));
        let write_clone = write.clone();

        // Channel for outgoing messages
        let (msg_tx, mut msg_rx) = mpsc::channel::<WsMessage>(100);

        // Spawn write task
        let write_task = tokio::spawn(async move {
            while let Some(msg) = msg_rx.recv().await {
                let mut guard = write_clone.lock().await;
                if let Err(e) = guard.send(msg).await {
                    error!("Failed to send WebSocket message: {}", e);
                    break;
                }
            }
        });

        // Spawn ping task
        let ping_tx = msg_tx.clone();
        let ping_interval = self.options.ping_interval;
        let ping_task = tokio::spawn(async move {
            let mut interval = interval(ping_interval);
            loop {
                interval.tick().await;
                if ping_tx.send(WsMessage::Ping(vec![])).await.is_err() {
                    break;
                }
            }
        });

        // Process incoming messages
        let result = self.process_messages(read, msg_tx, shutdown_rx).await;

        // Cleanup
        ping_task.abort();
        write_task.abort();

        result
    }

    /// Process incoming WebSocket messages.
    async fn process_messages(
        &self,
        mut read: SplitStream<WsConnection>,
        msg_tx: mpsc::Sender<WsMessage>,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> SlackResult<()> {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("Received shutdown signal");
                    return Ok(());
                }
                msg = read.next() => {
                    match msg {
                        Some(Ok(WsMessage::Text(text))) => {
                            self.handle_socket_message(&text, &msg_tx).await;
                        }
                        Some(Ok(WsMessage::Ping(data))) => {
                            let _ = msg_tx.send(WsMessage::Pong(data)).await;
                        }
                        Some(Ok(WsMessage::Pong(_))) => {
                            // Pong received, connection is alive
                        }
                        Some(Ok(WsMessage::Close(_))) => {
                            info!("WebSocket closed by server");
                            return Ok(());
                        }
                        Some(Err(e)) => {
                            return Err(SlackError::WebSocket(e.to_string()));
                        }
                        None => {
                            return Ok(());
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Handle a Socket Mode message.
    async fn handle_socket_message(&self, text: &str, msg_tx: &mpsc::Sender<WsMessage>) {
        debug!("Received Socket Mode message: {}", text);

        let envelope: SocketModeEnvelope = match serde_json::from_str(text) {
            Ok(env) => env,
            Err(e) => {
                warn!("Failed to parse Socket Mode envelope: {}", e);
                return;
            }
        };

        // Always acknowledge first
        let ack = SocketModeAck::new(&envelope.envelope_id);
        let ack_json = serde_json::to_string(&ack).unwrap();
        let _ = msg_tx.send(WsMessage::Text(ack_json)).await;

        // Process based on envelope type
        match envelope.envelope_type.as_str() {
            "events_api" => {
                if let Some(payload) = envelope.payload {
                    self.handle_event_payload(payload).await;
                }
            }
            "slash_commands" => {
                if let Some(payload) = envelope.payload {
                    self.handle_slash_command_payload(payload).await;
                }
            }
            "interactive" => {
                // Interactive components (buttons, menus) - not implemented yet
                debug!("Received interactive payload (not implemented)");
            }
            "hello" => {
                info!("Socket Mode connection established");
            }
            "disconnect" => {
                info!("Received disconnect request from Slack");
            }
            _ => {
                debug!("Unknown envelope type: {}", envelope.envelope_type);
            }
        }
    }

    /// Handle an events API payload.
    async fn handle_event_payload(&self, payload: EventPayload) {
        let team_id = payload.team_id.clone();

        match parse_event(&payload) {
            Ok(SlackEvent::AppMention(event)) => {
                let ctx = EventContext::from_app_mention(&event, team_id);
                self.handle_app_mention(event, ctx).await;
            }
            Ok(SlackEvent::Message(event)) => {
                // Ignore bot messages
                if event.is_bot_message() {
                    return;
                }
                // Only handle DMs
                if event.is_direct_message()
                    && let Some(ctx) = EventContext::from_message(&event, team_id)
                {
                    self.handle_direct_message(event, ctx).await;
                }
            }
            Ok(SlackEvent::Unknown) => {
                debug!("Received unknown event type");
            }
            Err(e) => {
                warn!("Failed to parse event: {}", e);
            }
        }
    }

    /// Handle an app mention event.
    async fn handle_app_mention(&self, event: AppMentionEvent, ctx: EventContext) {
        info!(
            "Handling app mention from user {} in channel {}",
            ctx.user_id, ctx.channel_id
        );

        // Extract the prompt
        let prompt = extract_prompt(&event.text);

        if prompt.is_empty() {
            // Send a helpful message
            let message = SlackMessageContent::new()
                .with_text("👋 Hi! I'm Cortex. Mention me with a request, like `@cortex help me write a function`");

            let _ = self.send_message(&ctx.channel_id, message).await;
            return;
        }

        // Send acknowledgment
        let ack_message = SlackMessageContent::new()
            .with_text("🔄 Processing...")
            .in_thread(ctx.thread_ts.as_deref().unwrap_or(&ctx.message_ts));

        if let Err(e) = self.send_message(&ctx.channel_id, ack_message).await {
            error!("Failed to send acknowledgment: {}", e);
        }

        // Try event handler first
        let handler_result = {
            let guard = self.event_handler.read().await;
            if let Some(handler) = guard.as_ref() {
                Some(handler.handle_app_mention(event.clone(), ctx.clone()).await)
            } else {
                None
            }
        };

        match handler_result {
            Some(Ok(Some(response))) => {
                let message = format_agent_response(&response, 0, 0)
                    .in_thread(ctx.thread_ts.as_deref().unwrap_or(&ctx.message_ts));

                if let Err(e) = self.send_message(&ctx.channel_id, message).await {
                    error!("Failed to send response: {}", e);
                }
            }
            Some(Ok(None)) => {
                // Handler handled it but no response needed
            }
            Some(Err(e)) => {
                error!("Event handler error: {}", e);
                let message = format_error_response(&e.to_string())
                    .in_thread(ctx.thread_ts.as_deref().unwrap_or(&ctx.message_ts));
                let _ = self.send_message(&ctx.channel_id, message).await;
            }
            None => {
                // No handler, send default response
                let message = SlackMessageContent::new()
                    .with_text("I received your message, but no handler is configured. Set an event handler to process prompts.")
                    .in_thread(ctx.thread_ts.as_deref().unwrap_or(&ctx.message_ts));
                let _ = self.send_message(&ctx.channel_id, message).await;
            }
        }
    }

    /// Handle a direct message event.
    async fn handle_direct_message(&self, event: MessageEvent, ctx: EventContext) {
        info!("Handling DM from user {}", ctx.user_id);

        let prompt = &event.text;

        if prompt.is_empty() {
            return;
        }

        // Send acknowledgment
        let ack_message = SlackMessageContent::new().with_text("🔄 Processing...");
        if let Err(e) = self.send_message(&ctx.channel_id, ack_message).await {
            error!("Failed to send acknowledgment: {}", e);
        }

        // Try event handler
        let handler_result = {
            let guard = self.event_handler.read().await;
            if let Some(handler) = guard.as_ref() {
                Some(
                    handler
                        .handle_direct_message(event.clone(), ctx.clone())
                        .await,
                )
            } else {
                None
            }
        };

        match handler_result {
            Some(Ok(Some(response))) => {
                let message = format_agent_response(&response, 0, 0);
                if let Err(e) = self.send_message(&ctx.channel_id, message).await {
                    error!("Failed to send response: {}", e);
                }
            }
            Some(Ok(None)) => {}
            Some(Err(e)) => {
                error!("Event handler error: {}", e);
                let message = format_error_response(&e.to_string());
                let _ = self.send_message(&ctx.channel_id, message).await;
            }
            None => {
                let message = SlackMessageContent::new()
                    .with_text("I received your message, but no handler is configured.");
                let _ = self.send_message(&ctx.channel_id, message).await;
            }
        }
    }

    /// Handle a slash command payload.
    async fn handle_slash_command_payload(&self, payload: EventPayload) {
        // Parse slash command from the event payload
        let event_value = match &payload.event {
            Some(v) => v,
            None => {
                warn!("Slash command payload missing event field");
                return;
            }
        };

        let command_payload: SlashCommandPayload = match serde_json::from_value(event_value.clone())
        {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to parse slash command payload: {}", e);
                return;
            }
        };

        info!(
            "Handling slash command {} from user {}",
            command_payload.command, command_payload.user_id
        );

        let parsed = parse_command(&command_payload);
        let ack = create_ack_response(&parsed);

        // Send acknowledgment via response_url
        let ack_response = DelayedResponse::new()
            .with_text(ack.text.unwrap_or_default())
            .in_channel();

        if let Err(e) =
            send_delayed_response(&self.client, &command_payload.response_url, &ack_response).await
        {
            error!("Failed to send command acknowledgment: {}", e);
        }

        // Process command asynchronously
        match parsed {
            crate::commands::ParsedCommand::Cortex { prompt, context } => {
                self.process_cortex_command(&prompt, &context.response_url)
                    .await;
            }
            crate::commands::ParsedCommand::CortexReview { pr_url, context } => {
                self.process_review_command(&pr_url, &context.response_url)
                    .await;
            }
            crate::commands::ParsedCommand::Unknown {
                command, context, ..
            } => {
                let response = DelayedResponse::new()
                    .with_text(format!("❓ Unknown command: {}", command))
                    .replace_original();
                let _ = send_delayed_response(&self.client, &context.response_url, &response).await;
            }
        }
    }

    /// Process a /cortex command.
    async fn process_cortex_command(&self, prompt: &str, response_url: &str) {
        // This would integrate with the agent system
        // For now, send a placeholder response
        let response = DelayedResponse::new()
            .with_text(format!(
                "🤖 *Cortex Response*\n\nProcessed prompt: {}\n\n_Agent integration pending_",
                prompt
            ))
            .in_channel()
            .replace_original();

        if let Err(e) = send_delayed_response(&self.client, response_url, &response).await {
            error!("Failed to send cortex command response: {}", e);
        }
    }

    /// Process a /cortex-review command.
    async fn process_review_command(&self, pr_url: &str, response_url: &str) {
        // This would integrate with the review system
        let response = DelayedResponse::new()
            .with_text(format!(
                "📝 *PR Review*\n\nReviewing: {}\n\n_Review integration pending_",
                pr_url
            ))
            .in_channel()
            .replace_original();

        if let Err(e) = send_delayed_response(&self.client, response_url, &response).await {
            error!("Failed to send review command response: {}", e);
        }
    }

    /// Send a message to a channel.
    pub async fn send_message(
        &self,
        channel: &str,
        content: SlackMessageContent,
    ) -> SlackResult<String> {
        let mut payload = serde_json::json!({
            "channel": channel,
        });

        if let Some(text) = &content.text {
            payload["text"] = serde_json::json!(text);
        }
        if let Some(blocks) = &content.blocks {
            payload["blocks"] = serde_json::json!(blocks);
        }
        if let Some(thread_ts) = &content.thread_ts {
            payload["thread_ts"] = serde_json::json!(thread_ts);
        }
        if let Some(reply_broadcast) = content.reply_broadcast {
            payload["reply_broadcast"] = serde_json::json!(reply_broadcast);
        }

        let response: serde_json::Value = self.api_call("chat.postMessage", &payload).await?;

        if response.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = response
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown");
            return Err(SlackApiError::new(error, error).into());
        }

        response
            .get("ts")
            .and_then(|ts| ts.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| SlackError::Api("Missing ts in response".to_string()))
    }

    /// Update an existing message.
    pub async fn update_message(
        &self,
        channel: &str,
        ts: &str,
        content: SlackMessageContent,
    ) -> SlackResult<()> {
        let mut payload = serde_json::json!({
            "channel": channel,
            "ts": ts,
        });

        if let Some(text) = &content.text {
            payload["text"] = serde_json::json!(text);
        }
        if let Some(blocks) = &content.blocks {
            payload["blocks"] = serde_json::json!(blocks);
        }

        let response: serde_json::Value = self.api_call("chat.update", &payload).await?;

        if response.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = response
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown");
            return Err(SlackApiError::new(error, error).into());
        }

        Ok(())
    }

    /// Make an API call to Slack.
    async fn api_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        payload: &serde_json::Value,
    ) -> SlackResult<T> {
        let url = format!("https://slack.com/api/{}", method);

        let response = self
            .client
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.config.bot_token()),
            )
            .header("Content-Type", "application/json; charset=utf-8")
            .json(payload)
            .send()
            .await?;

        // Check for rate limiting
        if response.status() == 429 {
            let retry_after = response
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or(30);
            return Err(SlackError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(SlackError::Api(format!("{}: {}", status, body)));
        }

        let result: T = response.json().await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bot_options_default() {
        let options = BotOptions::default();
        assert_eq!(options.api_timeout, Duration::from_secs(30));
        assert_eq!(options.max_retries, 3);
        assert_eq!(options.reconnect_delay, Duration::from_secs(5));
        assert_eq!(options.ping_interval, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn test_bot_creation_invalid_config() {
        let config = SlackConfig::new("invalid-token", "xapp-valid", "secret");
        let result = CortexSlackBot::new(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_bot_creation_valid_config() {
        let config = SlackConfig::new("xoxb-valid-token", "xapp-valid-token", "secret");
        let result = CortexSlackBot::new(config).await;
        assert!(result.is_ok());
    }
}
