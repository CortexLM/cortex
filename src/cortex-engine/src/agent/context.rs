//! Agent context management with automatic compaction.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, instrument, warn};

use super::{AgentConfig, AgentEvent};
use crate::client::ModelClient;
use crate::client::types::{CompletionRequest, Message, MessageRole};
use crate::error::Result;
use tokio::sync::mpsc;

/// Manages the conversation context for an agent.
pub struct AgentContext {
    /// Conversation history.
    messages: RwLock<Vec<Message>>,
    /// Model client for summarization.
    client: Arc<dyn ModelClient>,
    /// Configuration.
    config: AgentConfig,
    /// Event sender for notifications.
    event_tx: mpsc::UnboundedSender<AgentEvent>,
}

impl AgentContext {
    /// Create a new agent context.
    pub fn new(
        client: Arc<dyn ModelClient>,
        config: AgentConfig,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
    ) -> Self {
        Self {
            messages: RwLock::new(Vec::new()),
            client,
            config,
            event_tx,
        }
    }

    /// Add a message to the context.
    pub async fn add_message(&self, message: Message) -> Result<()> {
        {
            let mut messages = self.messages.write().await;
            messages.push(message);
        }

        if self.config.auto_compact {
            self.check_compaction().await?;
        }

        Ok(())
    }

    /// Get all messages.
    pub async fn messages(&self) -> Vec<Message> {
        self.messages.read().await.clone()
    }

    /// Clear the context.
    pub async fn clear(&self) {
        self.messages.write().await.clear();
    }

    /// Check if compaction is needed based on thresholds.
    async fn check_compaction(&self) -> Result<()> {
        let messages = self.messages.read().await;
        let message_count = messages.len();

        // Estimate token count
        let estimated_tokens = self.estimate_tokens(&messages);

        let threshold_tokens =
            (self.config.max_context_tokens as f32 * self.config.compaction_threshold) as u32;

        // Trigger compaction if we exceed token threshold or message count threshold (50)
        if estimated_tokens > threshold_tokens || message_count > 50 {
            debug!(
                tokens = estimated_tokens,
                count = message_count,
                "Compaction threshold reached"
            );
            drop(messages);
            self.compact().await?;
        }

        Ok(())
    }

    /// Rough estimate of tokens (4 characters per token).
    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        let mut total = 0;
        for msg in messages {
            if let Some(text) = msg.content.as_text() {
                total += (text.len() / 4) as u32;
            }
        }
        total
    }

    /// Compact the context by summarizing early messages.
    #[instrument(skip(self))]
    pub async fn compact(&self) -> Result<()> {
        info!("Starting automatic context compaction");

        let mut messages = self.messages.write().await;

        // Don't compact if we don't have enough history
        if messages.len() <= 15 {
            return Ok(());
        }

        // Separate system messages and conversation messages
        let mut system_messages = Vec::new();
        let mut conversation_messages = Vec::new();

        for msg in messages.drain(..) {
            if msg.role == MessageRole::System {
                system_messages.push(msg);
            } else {
                conversation_messages.push(msg);
            }
        }

        // Keep the last 10 messages of the conversation to maintain immediate context
        let keep_last = 10;
        let split_at = conversation_messages.len().saturating_sub(keep_last);
        let (to_summarize, to_keep) = conversation_messages.split_at(split_at);

        if to_summarize.is_empty() {
            *messages = system_messages;
            messages.extend(to_keep.to_vec());
            return Ok(());
        }

        let messages_removed = to_summarize.len();
        let tokens_saved = self.estimate_tokens(to_summarize);

        // Summarize the early messages
        let summary = match self.summarize_messages(to_summarize).await {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to summarize messages during compaction: {}", e);
                "Summary unavailable due to error.".to_string()
            }
        };

        // Reconstruct: System messages + Summary + Recent messages
        *messages = system_messages;
        messages.push(Message::system(format!(
            "Summary of previous conversation (compacted):\n\n{}",
            summary
        )));
        messages.extend(to_keep.to_vec());

        // Emit event
        let _ = self.event_tx.send(AgentEvent::ContextCompacted {
            messages_removed,
            tokens_saved,
        });

        info!(
            removed = messages_removed,
            saved = tokens_saved,
            "Compaction complete"
        );

        Ok(())
    }

    /// Use the LLM to summarize a slice of messages.
    async fn summarize_messages(&self, messages: &[Message]) -> Result<String> {
        debug!("Summarizing {} messages", messages.len());

        let mut summary_input = String::new();
        for msg in messages {
            let role = match msg.role {
                MessageRole::User => "User",
                MessageRole::Assistant => "Assistant",
                MessageRole::System => "System",
                MessageRole::Tool => "Tool",
            };
            if let Some(text) = msg.content.as_text() {
                summary_input.push_str(&format!("{}: {}\n", role, text));
            }
        }

        let prompt = format!(
            "Summarize the following conversation history concisely. \
             Maintain all important technical decisions, progress made, and current state. \
             Keep it focused and technical.\n\n\
             CONVERSATION HISTORY:\n{}",
            summary_input
        );

        let request = CompletionRequest {
            model: self.config.model.clone(),
            messages: vec![Message::user(prompt)],
            max_tokens: Some(1000),
            temperature: Some(0.3),
            stream: false,
            ..Default::default()
        };

        let response = self
            .client
            .complete_sync(request)
            .await
            .map_err(|e| crate::error::CortexError::Provider(e.to_string()))?;

        Ok(response
            .message
            .and_then(|m| m.content.as_text().map(|s| s.to_string()))
            .unwrap_or_else(|| "Summary generation failed.".to_string()))
    }
}
