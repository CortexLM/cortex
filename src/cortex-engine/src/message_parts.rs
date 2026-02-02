//! Message parts integration for sessions.
//!
//! This module provides utilities for converting between the internal
//! message representation and the rich MessageWithParts format.

use cortex_protocol::{
    ConversationId, IndexedPart, MessagePart, MessageRole, MessageWithParts, PartDelta,
    PartDeltaEvent, PartRemovedEvent, PartUpdatedEvent, SubtaskStatus, TokenUsage, ToolState,
};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::client::Message;

/// Builder for constructing MessageWithParts from streaming events.
#[derive(Debug)]
pub struct MessagePartsBuilder {
    /// The message being built.
    message: MessageWithParts,
    /// Part ID counter.
    part_counter: usize,
    /// Tool call ID to part index mapping.
    tool_parts: HashMap<String, usize>,
}

impl MessagePartsBuilder {
    /// Create a new builder for a user message.
    pub fn user(id: String, session_id: ConversationId) -> Self {
        Self {
            message: MessageWithParts::user(id, session_id),
            part_counter: 0,
            tool_parts: HashMap::new(),
        }
    }

    /// Create a new builder for an assistant message.
    pub fn assistant(
        id: String,
        session_id: ConversationId,
        parent_id: String,
        model_id: String,
        provider_id: String,
    ) -> Self {
        Self {
            message: MessageWithParts::assistant(id, session_id, parent_id, model_id, provider_id),
            part_counter: 0,
            tool_parts: HashMap::new(),
        }
    }

    /// Set the agent name.
    pub fn with_agent(mut self, agent: String) -> Self {
        self.message.agent = Some(agent);
        self
    }

    /// Generate a new part ID.
    fn new_part_id(&mut self) -> String {
        let id = format!("part_{}", self.part_counter);
        self.part_counter += 1;
        id
    }

    /// Add a text part.
    pub fn add_text(&mut self, content: String) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_text(id, content);
        self
    }

    /// Add a text part with metadata.
    pub fn add_text_with_metadata(
        &mut self,
        content: String,
        synthetic: Option<bool>,
        ignored: Option<bool>,
        metadata: Option<serde_json::Value>,
    ) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_part(
            id,
            MessagePart::Text {
                content,
                synthetic,
                ignored,
                metadata,
            },
        );
        self
    }

    /// Add a reasoning part.
    pub fn add_reasoning(&mut self, content: String) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_reasoning(id, content);
        self
    }

    /// Add a reasoning part with signature.
    pub fn add_reasoning_with_signature(
        &mut self,
        content: String,
        signature: Option<String>,
        metadata: Option<serde_json::Value>,
    ) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_part(
            id,
            MessagePart::Reasoning {
                content,
                signature,
                metadata,
            },
        );
        self
    }

    /// Add a tool call part.
    pub fn add_tool_call(
        &mut self,
        call_id: String,
        name: String,
        input: serde_json::Value,
    ) -> &mut Self {
        let id = self.new_part_id();
        let index = self.message.parts.len();
        self.tool_parts.insert(call_id.clone(), index);
        self.message.add_tool_call(id, call_id, name, input);
        self
    }

    /// Update a tool to running state.
    pub fn update_tool_running(&mut self, call_id: &str, title: Option<String>) -> bool {
        self.message.update_tool_state(
            call_id,
            ToolState::Running {
                title,
                metadata: None,
            },
        )
    }

    /// Complete a tool call.
    pub fn complete_tool(
        &mut self,
        call_id: &str,
        output: String,
        title: String,
        metadata: serde_json::Value,
    ) -> bool {
        self.message.complete_tool(call_id, output, title, metadata)
    }

    /// Mark a tool as error.
    pub fn error_tool(&mut self, call_id: &str, error: String) -> bool {
        self.message.error_tool(call_id, error)
    }

    /// Add a file part.
    pub fn add_file(
        &mut self,
        path: PathBuf,
        mime_type: String,
        url: Option<String>,
        content: Option<String>,
    ) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_part(
            id,
            MessagePart::File {
                path,
                mime_type,
                content,
                url,
                filename: None,
                source: None,
            },
        );
        self
    }

    /// Add a snapshot part.
    pub fn add_snapshot(&mut self, snapshot_id: String, message: String) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_part(
            id,
            MessagePart::Snapshot {
                snapshot_id,
                message,
            },
        );
        self
    }

    /// Add a patch part.
    pub fn add_patch(
        &mut self,
        file_path: PathBuf,
        diff: String,
        additions: u32,
        deletions: u32,
    ) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_part(
            id,
            MessagePart::Patch {
                file_path,
                diff,
                additions,
                deletions,
                hash: None,
            },
        );
        self
    }

    /// Add an agent switch part.
    pub fn add_agent_switch(&mut self, agent_id: String, agent_name: String) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_part(
            id,
            MessagePart::Agent {
                agent_id,
                agent_name,
                source: None,
            },
        );
        self
    }

    /// Add a step start part.
    pub fn add_step_start(&mut self, step_id: String, model: String) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_part(
            id,
            MessagePart::StepStart {
                step_id,
                model,
                snapshot: None,
            },
        );
        self
    }

    /// Add a step finish part.
    pub fn add_step_finish(
        &mut self,
        step_id: String,
        tokens: TokenUsage,
        cost: Option<f64>,
        duration_ms: u64,
    ) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_part(
            id,
            MessagePart::StepFinish {
                step_id,
                tokens,
                cost,
                duration_ms,
                reason: None,
                snapshot: None,
            },
        );
        self
    }

    /// Add a compaction part.
    pub fn add_compaction(
        &mut self,
        original_tokens: u64,
        compacted_tokens: u64,
        summary: String,
        auto: bool,
    ) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_part(
            id,
            MessagePart::Compaction {
                original_tokens,
                compacted_tokens,
                summary,
                auto,
            },
        );
        self
    }

    /// Add a subtask part.
    pub fn add_subtask(&mut self, task_id: String, description: String) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_part(
            id,
            MessagePart::Subtask {
                task_id,
                description,
                status: SubtaskStatus::Pending,
                agent: None,
                prompt: None,
                command: None,
            },
        );
        self
    }

    /// Add a retry part.
    pub fn add_retry(&mut self, attempt: u32, reason: String) -> &mut Self {
        let id = self.new_part_id();
        self.message.add_part(
            id,
            MessagePart::Retry {
                attempt,
                reason,
                error: None,
            },
        );
        self
    }

    /// Get the tool part index for a call ID.
    pub fn get_tool_part_index(&self, call_id: &str) -> Option<usize> {
        self.tool_parts.get(call_id).copied()
    }

    /// Get a part by index.
    pub fn get_part(&self, index: usize) -> Option<&IndexedPart> {
        self.message.get_part(index)
    }

    /// Complete the message and return it.
    pub fn complete(
        mut self,
        tokens: TokenUsage,
        cost: Option<f64>,
        finish_reason: String,
    ) -> MessageWithParts {
        self.message.complete(tokens, cost, finish_reason);
        self.message
    }

    /// Build without completing (for interrupted messages).
    pub fn build(self) -> MessageWithParts {
        self.message
    }

    /// Get the session ID.
    pub fn session_id(&self) -> &ConversationId {
        &self.message.session_id
    }

    /// Get the message ID.
    pub fn message_id(&self) -> &str {
        &self.message.id
    }

    /// Get the current part count.
    pub fn part_count(&self) -> usize {
        self.message.parts.len()
    }
}

/// Convert a client Message to a MessageWithParts.
pub fn from_client_message(
    msg: &Message,
    id: String,
    session_id: ConversationId,
) -> MessageWithParts {
    let role = match msg.role {
        crate::client::MessageRole::User => MessageRole::User,
        crate::client::MessageRole::Assistant => MessageRole::Assistant,
        crate::client::MessageRole::System => MessageRole::System,
        crate::client::MessageRole::Tool => MessageRole::User, // Tool results are user messages
    };

    let mut message = if role == MessageRole::User {
        MessageWithParts::user(id, session_id)
    } else {
        MessageWithParts::assistant(id, session_id, String::new(), String::new(), String::new())
    };

    // Add content as text part
    if let Some(text) = msg.content.as_text() {
        message.add_text(uuid::Uuid::new_v4().to_string(), text.to_string());
    }

    // Add tool calls if present
    if let Some(tool_calls) = &msg.tool_calls {
        for tc in tool_calls {
            let args: serde_json::Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::Value::Null);
            message.add_tool_call(
                uuid::Uuid::new_v4().to_string(),
                tc.id.clone(),
                tc.function.name.clone(),
                args,
            );
        }
    }

    message
}

/// Create a PartUpdatedEvent for a specific part.
pub fn create_part_updated_event(
    session_id: ConversationId,
    message_id: String,
    part: &IndexedPart,
) -> PartUpdatedEvent {
    PartUpdatedEvent {
        session_id,
        message_id,
        part_index: part.index,
        part_id: part.id.clone(),
        part: part.part.clone(),
        timing: part.timing.clone(),
    }
}

/// Create a PartRemovedEvent.
pub fn create_part_removed_event(
    session_id: ConversationId,
    message_id: String,
    part_index: usize,
    part_id: String,
) -> PartRemovedEvent {
    PartRemovedEvent {
        session_id,
        message_id,
        part_index,
        part_id,
    }
}

/// Create a PartDeltaEvent for text.
pub fn create_text_delta_event(
    session_id: ConversationId,
    message_id: String,
    part_index: usize,
    part_id: String,
    content: String,
) -> PartDeltaEvent {
    PartDeltaEvent {
        session_id,
        message_id,
        part_index,
        part_id,
        delta: PartDelta::Text { content },
    }
}

/// Create a PartDeltaEvent for reasoning.
pub fn create_reasoning_delta_event(
    session_id: ConversationId,
    message_id: String,
    part_index: usize,
    part_id: String,
    content: String,
) -> PartDeltaEvent {
    PartDeltaEvent {
        session_id,
        message_id,
        part_index,
        part_id,
        delta: PartDelta::Reasoning { content },
    }
}

/// Create a PartDeltaEvent for tool output.
pub fn create_tool_output_delta_event(
    session_id: ConversationId,
    message_id: String,
    part_index: usize,
    part_id: String,
    output: String,
) -> PartDeltaEvent {
    PartDeltaEvent {
        session_id,
        message_id,
        part_index,
        part_id,
        delta: PartDelta::ToolOutput { output },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session_id() -> ConversationId {
        ConversationId::new()
    }

    #[test]
    fn test_builder_user_message() {
        let mut builder = MessagePartsBuilder::user("msg_1".to_string(), test_session_id());

        builder.add_text("Hello, world!".to_string());
        builder.add_file(
            PathBuf::from("/tmp/test.txt"),
            "text/plain".to_string(),
            None,
            Some("file content".to_string()),
        );

        let message = builder.build();

        assert_eq!(message.role, MessageRole::User);
        assert_eq!(message.parts.len(), 2);
    }

    #[test]
    fn test_builder_assistant_message() {
        let session_id = test_session_id();
        let mut builder = MessagePartsBuilder::assistant(
            "msg_2".to_string(),
            session_id.clone(),
            "msg_1".to_string(),
            "gpt-4".to_string(),
            "openai".to_string(),
        );

        builder.add_text("Let me help you with that.".to_string());
        builder.add_tool_call(
            "call_123".to_string(),
            "read_file".to_string(),
            serde_json::json!({"path": "/tmp/test.txt"}),
        );

        // Simulate tool completion
        builder.update_tool_running("call_123", Some("Reading file...".to_string()));
        builder.complete_tool(
            "call_123",
            "File contents here".to_string(),
            "Read /tmp/test.txt".to_string(),
            serde_json::json!({"bytes": 18}),
        );

        let message = builder.complete(TokenUsage::default(), Some(0.001), "stop".to_string());

        assert_eq!(message.role, MessageRole::Assistant);
        assert_eq!(message.parts.len(), 2);
        assert!(message.completed_at.is_some());
    }

    #[test]
    fn test_builder_with_reasoning() {
        let session_id = test_session_id();
        let mut builder = MessagePartsBuilder::assistant(
            "msg_3".to_string(),
            session_id,
            "msg_2".to_string(),
            "claude-3-opus".to_string(),
            "anthropic".to_string(),
        );

        builder.add_reasoning("I need to think about this...".to_string());
        builder.add_text("Here's my response.".to_string());

        let message = builder.build();

        assert_eq!(message.parts.len(), 2);

        // Check first part is reasoning
        match &message.parts[0].part {
            MessagePart::Reasoning { content, .. } => {
                assert!(content.contains("think"));
            }
            _ => panic!("Expected reasoning part"),
        }
    }

    #[test]
    fn test_builder_subtask() {
        let session_id = test_session_id();
        let mut builder = MessagePartsBuilder::assistant(
            "msg_4".to_string(),
            session_id,
            "msg_3".to_string(),
            "gpt-4".to_string(),
            "openai".to_string(),
        );

        builder.add_subtask("task_1".to_string(), "Analyze the codebase".to_string());

        let message = builder.build();

        match &message.parts[0].part {
            MessagePart::Subtask {
                task_id,
                description,
                status,
                ..
            } => {
                assert_eq!(task_id, "task_1");
                assert_eq!(description, "Analyze the codebase");
                assert_eq!(*status, SubtaskStatus::Pending);
            }
            _ => panic!("Expected subtask part"),
        }
    }
}
