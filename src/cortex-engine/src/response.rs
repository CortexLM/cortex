//! Response processing and streaming.
//!
//! Handles processing of model responses, streaming content,
//! and extracting structured data from responses.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::client::types::{CompletionResponse, ToolCall};
use crate::error::Result;

/// Response processor configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseConfig {
    /// Maximum response length.
    pub max_length: usize,
    /// Enable streaming.
    pub streaming: bool,
    /// Stream buffer size.
    pub buffer_size: usize,
    /// Timeout for response.
    pub timeout: Duration,
    /// Parse tool calls.
    pub parse_tools: bool,
    /// Extract code blocks.
    pub extract_code: bool,
    /// Stop sequences to strip from output (if present at the end).
    #[serde(default)]
    pub stop_sequences: Vec<String>,
}

impl Default for ResponseConfig {
    fn default() -> Self {
        Self {
            max_length: 100000,
            streaming: true,
            buffer_size: 1024,
            timeout: Duration::from_secs(300),
            parse_tools: true,
            extract_code: true,
            stop_sequences: Vec::new(),
        }
    }
}

/// Strip stop sequences from the end of a response text.
/// Stop sequences cause generation to stop but should not be included in the output.
pub fn strip_stop_sequences(text: &str, stop_sequences: &[String]) -> String {
    if stop_sequences.is_empty() {
        return text.to_string();
    }

    let mut result = text.to_string();

    // Check each stop sequence and remove it if it appears at the end
    for stop_seq in stop_sequences {
        if !stop_seq.is_empty() && result.ends_with(stop_seq) {
            result = result[..result.len() - stop_seq.len()].to_string();
            // Only strip one stop sequence (the one that actually caused the stop)
            break;
        }
    }

    result
}

/// A streaming response chunk.
#[derive(Debug, Clone)]
pub enum ResponseChunk {
    /// Text content.
    Text(String),
    /// Reasoning/thinking content.
    Reasoning(String),
    /// Tool call.
    ToolCall(ToolCallChunk),
    /// Usage statistics.
    Usage(UsageStats),
    /// Response complete.
    Done(ResponseSummary),
    /// Error occurred.
    Error(String),
}

/// Tool call chunk during streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallChunk {
    /// Tool call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments (may be partial during streaming).
    pub arguments: String,
    /// Whether this is complete.
    pub complete: bool,
}

/// Usage statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageStats {
    /// Input tokens.
    pub input_tokens: u32,
    /// Output tokens.
    pub output_tokens: u32,
    /// Cached tokens.
    pub cached_tokens: u32,
    /// Reasoning tokens.
    pub reasoning_tokens: u32,
    /// Total tokens.
    pub total_tokens: u32,
    /// Time to first token.
    pub time_to_first_token_ms: Option<u64>,
    /// Total duration.
    pub total_duration_ms: u64,
}

/// Response summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseSummary {
    /// Full text content.
    pub text: String,
    /// Tool calls made.
    pub tool_calls: Vec<ToolCall>,
    /// Finish reason.
    pub finish_reason: FinishReason,
    /// Usage statistics.
    pub usage: UsageStats,
    /// Extracted code blocks.
    pub code_blocks: Vec<CodeBlock>,
}

/// Finish reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum FinishReason {
    /// Normal completion.
    #[default]
    Stop,
    /// Hit length limit.
    Length,
    /// Tool use requested.
    ToolUse,
    /// Content filtered.
    ContentFilter,
    /// Error occurred.
    Error,
}

/// Extracted code block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeBlock {
    /// Language identifier.
    pub language: Option<String>,
    /// Code content.
    pub code: String,
    /// Start position in response.
    pub start: usize,
    /// End position in response.
    pub end: usize,
}

/// Response processor.
pub struct ResponseProcessor {
    /// Configuration.
    config: ResponseConfig,
}

impl ResponseProcessor {
    /// Create a new response processor.
    pub fn new(config: ResponseConfig) -> Self {
        Self { config }
    }

    /// Create with default config.
    pub fn default_processor() -> Self {
        Self::new(ResponseConfig::default())
    }

    /// Process a complete response.
    pub fn process(&self, response: &CompletionResponse) -> Result<ProcessedResponse> {
        let text = response
            .message
            .as_ref()
            .and_then(|m| m.content.as_text())
            .map(std::string::ToString::to_string)
            .unwrap_or_default();

        let tool_calls = response.tool_calls.clone();

        let code_blocks = if self.config.extract_code {
            self.extract_code_blocks(&text)
        } else {
            Vec::new()
        };

        let finish_reason = match response.finish_reason {
            crate::client::types::FinishReason::Stop => FinishReason::Stop,
            crate::client::types::FinishReason::Length => FinishReason::Length,
            crate::client::types::FinishReason::ToolCalls => FinishReason::ToolUse,
            crate::client::types::FinishReason::ContentFilter => FinishReason::ContentFilter,
            crate::client::types::FinishReason::Error => FinishReason::Error,
        };

        Ok(ProcessedResponse {
            text,
            tool_calls,
            code_blocks,
            finish_reason,
            usage: UsageStats {
                input_tokens: response.usage.input_tokens as u32,
                output_tokens: response.usage.output_tokens as u32,
                total_tokens: response.usage.total_tokens as u32,
                ..Default::default()
            },
        })
    }

    /// Extract code blocks from text.
    pub fn extract_code_blocks(&self, text: &str) -> Vec<CodeBlock> {
        let mut blocks = Vec::new();
        let mut in_block = false;
        let mut current_lang = None;
        let mut current_code = String::new();
        let mut block_start = 0;

        for (i, line) in text.lines().enumerate() {
            if line.starts_with("```") {
                if in_block {
                    // End of block
                    blocks.push(CodeBlock {
                        language: current_lang.take(),
                        code: current_code.trim().to_string(),
                        start: block_start,
                        end: i,
                    });
                    current_code.clear();
                    in_block = false;
                } else {
                    // Start of block
                    in_block = true;
                    block_start = i;
                    let lang = line.trim_start_matches('`').trim();
                    if !lang.is_empty() {
                        current_lang = Some(lang.to_string());
                    }
                }
            } else if in_block {
                if !current_code.is_empty() {
                    current_code.push('\n');
                }
                current_code.push_str(line);
            }
        }

        blocks
    }

    /// Parse tool calls from response.
    pub fn parse_tool_calls(&self, response: &CompletionResponse) -> Vec<ParsedToolCall> {
        response
            .tool_calls
            .iter()
            .map(|tc| {
                let arguments: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::Value::Null);

                ParsedToolCall {
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    arguments,
                    raw_arguments: tc.function.arguments.clone(),
                }
            })
            .collect()
    }

    /// Check if response indicates tool use.
    pub fn has_tool_calls(&self, response: &CompletionResponse) -> bool {
        !response.tool_calls.is_empty()
    }

    /// Check if response is complete.
    pub fn is_complete(&self, response: &CompletionResponse) -> bool {
        matches!(
            response.finish_reason,
            crate::client::types::FinishReason::Stop | crate::client::types::FinishReason::Length
        )
    }

    /// Truncate response if needed.
    pub fn truncate_if_needed(&self, text: &str) -> (String, bool) {
        if text.len() > self.config.max_length {
            let truncated = text[..self.config.max_length].to_string();
            (truncated, true)
        } else {
            (text.to_string(), false)
        }
    }
}

/// Processed response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessedResponse {
    /// Text content.
    pub text: String,
    /// Tool calls.
    pub tool_calls: Vec<ToolCall>,
    /// Code blocks.
    pub code_blocks: Vec<CodeBlock>,
    /// Finish reason.
    pub finish_reason: FinishReason,
    /// Usage stats.
    pub usage: UsageStats,
}

impl ProcessedResponse {
    /// Check if response has content.
    pub fn has_content(&self) -> bool {
        !self.text.is_empty()
    }

    /// Check if response has tool calls.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Check if response has code.
    pub fn has_code(&self) -> bool {
        !self.code_blocks.is_empty()
    }

    /// Get first code block of a language.
    pub fn get_code(&self, language: &str) -> Option<&CodeBlock> {
        self.code_blocks
            .iter()
            .find(|b| b.language.as_ref().map(|l| l == language).unwrap_or(false))
    }
}

/// Parsed tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedToolCall {
    /// Call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Parsed arguments.
    pub arguments: serde_json::Value,
    /// Raw arguments string.
    pub raw_arguments: String,
}

impl ParsedToolCall {
    /// Get argument by key.
    pub fn get_arg(&self, key: &str) -> Option<&serde_json::Value> {
        self.arguments.get(key)
    }

    /// Get string argument.
    pub fn get_string(&self, key: &str) -> Option<&str> {
        self.arguments.get(key).and_then(|v| v.as_str())
    }

    /// Get bool argument.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.arguments.get(key).and_then(serde_json::Value::as_bool)
    }

    /// Get number argument.
    pub fn get_number(&self, key: &str) -> Option<f64> {
        self.arguments.get(key).and_then(serde_json::Value::as_f64)
    }
}

/// Stream aggregator for building complete responses from chunks.
#[derive(Debug, Default)]
pub struct StreamAggregator {
    /// Accumulated text.
    text: String,
    /// Accumulated reasoning.
    reasoning: String,
    /// Tool calls being built.
    tool_calls: HashMap<String, ToolCallBuilder>,
    /// Start time.
    start_time: Option<Instant>,
    /// Time to first token.
    first_token_time: Option<Instant>,
    /// Last usage stats.
    usage: UsageStats,
}

impl StreamAggregator {
    /// Create a new aggregator.
    pub fn new() -> Self {
        Self {
            start_time: Some(Instant::now()),
            ..Default::default()
        }
    }

    /// Process a chunk.
    pub fn process_chunk(&mut self, chunk: ResponseChunk) {
        if self.first_token_time.is_none() {
            self.first_token_time = Some(Instant::now());
        }

        match chunk {
            ResponseChunk::Text(text) => {
                self.text.push_str(&text);
            }
            ResponseChunk::Reasoning(text) => {
                self.reasoning.push_str(&text);
            }
            ResponseChunk::ToolCall(tc) => {
                let builder = self
                    .tool_calls
                    .entry(tc.id.clone())
                    .or_insert_with(|| ToolCallBuilder::new(&tc.id, &tc.name));
                builder.append_arguments(&tc.arguments);
                if tc.complete {
                    builder.complete = true;
                }
            }
            ResponseChunk::Usage(usage) => {
                self.usage = usage;
            }
            ResponseChunk::Done(_) | ResponseChunk::Error(_) => {}
        }
    }

    /// Build the final response.
    pub fn build(self) -> ProcessedResponse {
        let tool_calls: Vec<ToolCall> = self
            .tool_calls
            .into_values()
            .filter(|b| b.complete)
            .map(ToolCallBuilder::build)
            .collect();

        let mut usage = self.usage;
        if let (Some(start), Some(first)) = (self.start_time, self.first_token_time) {
            usage.time_to_first_token_ms = Some(first.duration_since(start).as_millis() as u64);
        }
        if let Some(start) = self.start_time {
            usage.total_duration_ms = start.elapsed().as_millis() as u64;
        }

        let has_tool_calls = !tool_calls.is_empty();

        ProcessedResponse {
            text: self.text,
            tool_calls,
            code_blocks: Vec::new(),
            finish_reason: if has_tool_calls {
                FinishReason::ToolUse
            } else {
                FinishReason::Stop
            },
            usage,
        }
    }

    /// Get current text.
    pub fn current_text(&self) -> &str {
        &self.text
    }

    /// Get current reasoning.
    pub fn current_reasoning(&self) -> &str {
        &self.reasoning
    }
}

/// Tool call builder for streaming.
#[derive(Debug)]
struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
    complete: bool,
}

impl ToolCallBuilder {
    fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            arguments: String::new(),
            complete: false,
        }
    }

    fn append_arguments(&mut self, args: &str) {
        self.arguments.push_str(args);
    }

    fn build(self) -> ToolCall {
        ToolCall {
            id: self.id,
            call_type: "function".to_string(),
            function: crate::client::types::FunctionCall {
                name: self.name,
                arguments: self.arguments,
            },
        }
    }
}

/// Response formatter.
#[allow(dead_code)]
pub struct ResponseFormatter {
    /// Show reasoning.
    show_reasoning: bool,
    /// Show tool calls.
    show_tool_calls: bool,
    /// Show usage.
    show_usage: bool,
    /// Max line width.
    max_width: usize,
}

impl ResponseFormatter {
    /// Create a new formatter.
    pub fn new() -> Self {
        Self {
            show_reasoning: true,
            show_tool_calls: true,
            show_usage: false,
            max_width: 100,
        }
    }

    /// Format a response for display.
    pub fn format(&self, response: &ProcessedResponse) -> String {
        let mut output = String::new();

        // Main text
        output.push_str(&response.text);

        // Tool calls
        if self.show_tool_calls && !response.tool_calls.is_empty() {
            output.push_str("\n\n--- Tool Calls ---\n");
            for tc in &response.tool_calls {
                output.push_str(&format!(
                    "• {}: {}\n",
                    tc.function.name, tc.function.arguments
                ));
            }
        }

        // Usage
        if self.show_usage {
            output.push_str(&format!(
                "\n[Tokens: {} in, {} out, {} total]\n",
                response.usage.input_tokens,
                response.usage.output_tokens,
                response.usage.total_tokens
            ));
        }

        output
    }

    /// Set whether to show reasoning.
    pub fn show_reasoning(mut self, show: bool) -> Self {
        self.show_reasoning = show;
        self
    }

    /// Set whether to show tool calls.
    pub fn show_tool_calls(mut self, show: bool) -> Self {
        self.show_tool_calls = show;
        self
    }

    /// Set whether to show usage.
    pub fn show_usage(mut self, show: bool) -> Self {
        self.show_usage = show;
        self
    }
}

impl Default for ResponseFormatter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_code_blocks() {
        let processor = ResponseProcessor::default_processor();
        let text = r#"Here's some code:

```rust
fn main() {
    println!("Hello");
}
```

And more:

```python
print("hi")
```
"#;

        let blocks = processor.extract_code_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].language, Some("rust".to_string()));
        assert_eq!(blocks[1].language, Some("python".to_string()));
    }

    #[test]
    fn test_stream_aggregator() {
        let mut agg = StreamAggregator::new();

        agg.process_chunk(ResponseChunk::Text("Hello ".to_string()));
        agg.process_chunk(ResponseChunk::Text("world!".to_string()));

        assert_eq!(agg.current_text(), "Hello world!");

        let response = agg.build();
        assert_eq!(response.text, "Hello world!");
    }

    #[test]
    fn test_parsed_tool_call() {
        let tc = ParsedToolCall {
            id: "1".to_string(),
            name: "test".to_string(),
            arguments: serde_json::json!({"key": "value", "num": 42}),
            raw_arguments: r#"{"key": "value", "num": 42}"#.to_string(),
        };

        assert_eq!(tc.get_string("key"), Some("value"));
        assert_eq!(tc.get_number("num"), Some(42.0));
    }

    #[test]
    fn test_usage_stats() {
        let usage = UsageStats {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            ..Default::default()
        };

        assert_eq!(usage.total_tokens, 150);
    }
}
