//! AI utilities.
//!
//! Provides utilities for AI/LLM operations including
//! prompt construction, response parsing, and streaming.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Chat message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Role {
    /// System message.
    System,
    /// User message.
    #[default]
    User,
    /// Assistant message.
    Assistant,
    /// Tool message.
    Tool,
}

/// Chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Role.
    pub role: Role,
    /// Content.
    pub content: String,
    /// Name (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tool call ID (for tool messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            name: None,
            tool_call_id: None,
        }
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            name: None,
            tool_call_id: None,
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            name: None,
            tool_call_id: None,
        }
    }

    /// Create a tool message.
    pub fn tool(content: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            name: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    /// Set name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Token estimate (rough).
    pub fn token_estimate(&self) -> usize {
        // Rough estimate: ~4 chars per token
        self.content.len() / 4 + 4
    }
}

/// Conversation history.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Conversation {
    /// Messages.
    pub messages: Vec<ChatMessage>,
    /// Metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl Conversation {
    /// Create a new conversation.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a message.
    pub fn add(&mut self, message: ChatMessage) {
        self.messages.push(message);
    }

    /// Add a system message.
    pub fn add_system(&mut self, content: impl Into<String>) {
        self.add(ChatMessage::system(content));
    }

    /// Add a user message.
    pub fn add_user(&mut self, content: impl Into<String>) {
        self.add(ChatMessage::user(content));
    }

    /// Add an assistant message.
    pub fn add_assistant(&mut self, content: impl Into<String>) {
        self.add(ChatMessage::assistant(content));
    }

    /// Get message count.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Get last message.
    pub fn last(&self) -> Option<&ChatMessage> {
        self.messages.last()
    }

    /// Get last user message.
    pub fn last_user(&self) -> Option<&ChatMessage> {
        self.messages.iter().rev().find(|m| m.role == Role::User)
    }

    /// Get last assistant message.
    pub fn last_assistant(&self) -> Option<&ChatMessage> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant)
    }

    /// Estimate total tokens.
    pub fn token_estimate(&self) -> usize {
        self.messages.iter().map(ChatMessage::token_estimate).sum()
    }

    /// Truncate to fit token limit.
    pub fn truncate_to_fit(&mut self, max_tokens: usize) {
        while self.token_estimate() > max_tokens && self.messages.len() > 1 {
            // Keep system message if present
            if self.messages.len() > 1 && self.messages[0].role == Role::System {
                self.messages.remove(1);
            } else {
                self.messages.remove(0);
            }
        }
    }

    /// Clear messages.
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Set metadata.
    pub fn set_metadata(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.metadata.insert(key.into(), value.into());
    }
}

/// Tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Parameters schema.
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    /// Create a new tool definition.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    /// Set parameters.
    pub fn with_parameters(mut self, parameters: serde_json::Value) -> Self {
        self.parameters = parameters;
        self
    }

    /// Add a parameter.
    pub fn add_parameter(
        mut self,
        name: impl Into<String>,
        param_type: impl Into<String>,
        description: impl Into<String>,
        required: bool,
    ) -> Self {
        let name = name.into();

        if let Some(props) = self.parameters.get_mut("properties")
            && let Some(obj) = props.as_object_mut()
        {
            obj.insert(
                name.clone(),
                serde_json::json!({
                    "type": param_type.into(),
                    "description": description.into(),
                }),
            );
        }

        if required {
            if let Some(req) = self.parameters.get_mut("required") {
                if let Some(arr) = req.as_array_mut() {
                    arr.push(serde_json::Value::String(name));
                }
            } else {
                self.parameters["required"] = serde_json::json!([name]);
            }
        }

        self
    }
}

/// Tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments.
    pub arguments: serde_json::Value,
}

impl ToolCall {
    /// Parse arguments as type.
    pub fn parse_args<T: for<'de> Deserialize<'de>>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.arguments.clone())
    }

    /// Get argument value.
    pub fn get_arg(&self, key: &str) -> Option<&serde_json::Value> {
        self.arguments.get(key)
    }

    /// Get string argument.
    pub fn get_string(&self, key: &str) -> Option<&str> {
        self.get_arg(key).and_then(|v| v.as_str())
    }

    /// Get integer argument.
    pub fn get_int(&self, key: &str) -> Option<i64> {
        self.get_arg(key).and_then(serde_json::Value::as_i64)
    }

    /// Get boolean argument.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get_arg(key).and_then(serde_json::Value::as_bool)
    }
}

/// Completion options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionOptions {
    /// Model name.
    pub model: String,
    /// Temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Max tokens.
    pub max_tokens: Option<u32>,
    /// Top P.
    pub top_p: Option<f32>,
    /// Stop sequences.
    #[serde(default)]
    pub stop: Vec<String>,
    /// Presence penalty.
    pub presence_penalty: Option<f32>,
    /// Frequency penalty.
    pub frequency_penalty: Option<f32>,
    /// Stream.
    #[serde(default)]
    pub stream: bool,
}

fn default_temperature() -> f32 {
    0.7
}

impl Default for CompletionOptions {
    fn default() -> Self {
        Self {
            model: "gpt-4".to_string(),
            temperature: default_temperature(),
            max_tokens: None,
            top_p: None,
            stop: Vec::new(),
            presence_penalty: None,
            frequency_penalty: None,
            stream: false,
        }
    }
}

impl CompletionOptions {
    /// Create for model.
    pub fn for_model(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            ..Default::default()
        }
    }

    /// Set temperature.
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = temp;
        self
    }

    /// Set max tokens.
    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Enable streaming.
    pub fn with_streaming(mut self) -> Self {
        self.stream = true;
        self
    }

    /// Add stop sequence.
    pub fn with_stop(mut self, stop: impl Into<String>) -> Self {
        self.stop.push(stop.into());
        self
    }
}

/// Completion response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// Generated text.
    pub content: String,
    /// Finish reason.
    pub finish_reason: Option<String>,
    /// Tool calls.
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// Usage.
    pub usage: Option<Usage>,
}

impl CompletionResponse {
    /// Check if has tool calls.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Get tool call by name.
    pub fn get_tool_call(&self, name: &str) -> Option<&ToolCall> {
        self.tool_calls.iter().find(|t| t.name == name)
    }
}

/// Token usage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Prompt tokens.
    pub prompt_tokens: u32,
    /// Completion tokens.
    pub completion_tokens: u32,
    /// Total tokens.
    pub total_tokens: u32,
}

impl Usage {
    /// Estimate cost (rough).
    pub fn estimate_cost(&self, input_price: f64, output_price: f64) -> f64 {
        let input_cost = self.prompt_tokens as f64 * input_price / 1000.0;
        let output_cost = self.completion_tokens as f64 * output_price / 1000.0;
        input_cost + output_cost
    }
}

/// Response parser.
pub struct ResponseParser;

impl ResponseParser {
    /// Extract code blocks.
    pub fn extract_code_blocks(content: &str) -> Vec<CodeBlock> {
        let mut blocks = Vec::new();
        let mut in_block = false;
        let mut current_lang = String::new();
        let mut current_code = String::new();

        for line in content.lines() {
            if line.starts_with("```") {
                if in_block {
                    // End of block
                    blocks.push(CodeBlock {
                        language: if current_lang.is_empty() {
                            None
                        } else {
                            Some(current_lang.clone())
                        },
                        code: current_code.trim().to_string(),
                    });
                    current_code.clear();
                    current_lang.clear();
                    in_block = false;
                } else {
                    // Start of block
                    current_lang = line[3..].trim().to_string();
                    in_block = true;
                }
            } else if in_block {
                current_code.push_str(line);
                current_code.push('\n');
            }
        }

        blocks
    }

    /// Extract JSON from content.
    pub fn extract_json(content: &str) -> Option<serde_json::Value> {
        // Try to parse entire content as JSON
        if let Ok(json) = serde_json::from_str(content) {
            return Some(json);
        }

        // Try to find JSON in code blocks
        for block in Self::extract_code_blocks(content) {
            if let Some(lang) = &block.language
                && lang == "json"
                && let Ok(json) = serde_json::from_str(&block.code)
            {
                return Some(json);
            }
        }

        // Try to find JSON between braces
        if let Some(start) = content.find('{') {
            let sub = &content[start..];
            let mut brace_count = 0;
            let mut end_idx = 0;

            for (i, c) in sub.chars().enumerate() {
                if c == '{' {
                    brace_count += 1;
                } else if c == '}' {
                    brace_count -= 1;
                    if brace_count == 0 {
                        end_idx = i + 1;
                        break;
                    }
                }
            }

            if end_idx > 0
                && let Ok(json) = serde_json::from_str(&sub[..end_idx])
            {
                return Some(json);
            }
        }

        None
    }

    /// Extract list items.
    pub fn extract_list_items(content: &str) -> Vec<String> {
        let mut items = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();

            // Numbered list
            if trimmed
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
                && let Some(idx) = trimmed.find(['.', ')'])
            {
                items.push(trimmed[idx + 1..].trim().to_string());
                continue;
            }

            // Bullet list
            if trimmed.starts_with('-') || trimmed.starts_with('*') || trimmed.starts_with('•') {
                items.push(trimmed[1..].trim().to_string());
            }
        }

        items
    }

    /// Extract key-value pairs.
    pub fn extract_key_values(content: &str) -> HashMap<String, String> {
        let mut pairs = HashMap::new();

        for line in content.lines() {
            if let Some(colon_idx) = line.find(':') {
                let key = line[..colon_idx].trim().to_string();
                let value = line[colon_idx + 1..].trim().to_string();
                if !key.is_empty() {
                    pairs.insert(key, value);
                }
            }
        }

        pairs
    }
}

/// Code block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeBlock {
    /// Language.
    pub language: Option<String>,
    /// Code content.
    pub code: String,
}

impl CodeBlock {
    /// Get file extension for language.
    pub fn extension(&self) -> Option<&str> {
        self.language.as_ref().map(|lang| match lang.as_str() {
            "rust" | "rs" => "rs",
            "python" | "py" => "py",
            "javascript" | "js" => "js",
            "typescript" | "ts" => "ts",
            "go" => "go",
            "java" => "java",
            "c" => "c",
            "cpp" | "c++" => "cpp",
            "csharp" | "cs" => "cs",
            "ruby" | "rb" => "rb",
            "php" => "php",
            "swift" => "swift",
            "kotlin" | "kt" => "kt",
            "scala" => "scala",
            "bash" | "sh" | "shell" => "sh",
            "sql" => "sql",
            "html" => "html",
            "css" => "css",
            "json" => "json",
            "yaml" | "yml" => "yaml",
            "toml" => "toml",
            "markdown" | "md" => "md",
            _ => lang.as_str(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_message() {
        let msg = ChatMessage::user("Hello");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content, "Hello");
    }

    #[test]
    fn test_conversation() {
        let mut conv = Conversation::new();
        conv.add_system("You are helpful");
        conv.add_user("Hello");
        conv.add_assistant("Hi there!");

        assert_eq!(conv.len(), 3);
        assert!(conv.last_assistant().is_some());
    }

    #[test]
    fn test_conversation_truncate() {
        let mut conv = Conversation::new();
        conv.add_system("System");
        for i in 0..10 {
            conv.add_user(format!("Message {}", i));
        }

        conv.truncate_to_fit(50);

        // Should keep system message
        assert_eq!(conv.messages[0].role, Role::System);
    }

    #[test]
    fn test_tool_definition() {
        let tool = ToolDefinition::new("search", "Search the web").add_parameter(
            "query",
            "string",
            "Search query",
            true,
        );

        assert_eq!(tool.name, "search");
        assert!(tool.parameters["properties"]["query"].is_object());
    }

    #[test]
    fn test_extract_code_blocks() {
        let content = r#"
Here's some code:

```rust
fn main() {
    println!("Hello");
}
```

And more text.
"#;

        let blocks = ResponseParser::extract_code_blocks(content);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].language, Some("rust".to_string()));
        assert!(blocks[0].code.contains("fn main()"));
    }

    #[test]
    fn test_extract_json() {
        let content = r#"Here's the result: {"name": "test", "value": 42}"#;
        let json = ResponseParser::extract_json(content);

        assert!(json.is_some());
        assert_eq!(json.unwrap()["name"], "test");
    }

    #[test]
    fn test_extract_list() {
        let content = r#"
1. First item
2. Second item
- Third item
* Fourth item
"#;

        let items = ResponseParser::extract_list_items(content);
        assert_eq!(items.len(), 4);
    }

    #[test]
    fn test_completion_options() {
        let opts = CompletionOptions::for_model("gpt-4")
            .with_temperature(0.5)
            .with_max_tokens(1000)
            .with_streaming();

        assert_eq!(opts.model, "gpt-4");
        assert_eq!(opts.temperature, 0.5);
        assert!(opts.stream);
    }

    #[test]
    fn test_usage_cost() {
        let usage = Usage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
        };

        let cost = usage.estimate_cost(0.01, 0.03);
        assert!(cost > 0.0);
    }
}
