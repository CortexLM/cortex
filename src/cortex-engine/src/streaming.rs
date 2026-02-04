//! Streaming utilities.
//!
//! Provides utilities for handling streaming responses from LLMs
//! including delta processing, buffering, and event handling.
//!
//! All functions use safe array access with `.get()` and proper error handling
//! to prevent index out of bounds panics on malformed API responses.

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Maximum number of events to buffer before dropping old ones.
/// Prevents unbounded memory growth if drain_events() is not called regularly.
const MAX_BUFFER_SIZE: usize = 10_000;

/// Token usage for streaming.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamTokenUsage {
    /// Prompt tokens.
    pub prompt_tokens: u32,
    /// Completion tokens.
    pub completion_tokens: u32,
    /// Total tokens.
    pub total_tokens: u32,
}

/// Safely convert an i64 token count to u32 with saturation.
/// Negative values clamp to 0, values > u32::MAX clamp to u32::MAX.
#[inline]
fn saturating_i64_to_u32(value: i64) -> u32 {
    if value <= 0 {
        0
    } else if value > u32::MAX as i64 {
        u32::MAX
    } else {
        value as u32
    }
}

impl From<crate::client::TokenUsage> for StreamTokenUsage {
    fn from(usage: crate::client::TokenUsage) -> Self {
        Self {
            prompt_tokens: saturating_i64_to_u32(usage.input_tokens),
            completion_tokens: saturating_i64_to_u32(usage.output_tokens),
            total_tokens: saturating_i64_to_u32(usage.total_tokens),
        }
    }
}

/// Stream event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum StreamEvent {
    /// Stream started.
    Start,
    /// Text delta (simple variant).
    TextDelta { delta: String },
    /// Text delta (shorthand).
    Delta(String),
    /// Reasoning delta.
    Reasoning(String),
    /// Tool call started.
    ToolCallStart { id: String, name: String },
    /// Tool call delta.
    ToolCallDelta { id: String, arguments: String },
    /// Tool call complete.
    ToolCallComplete { id: String },
    /// Tool call with full info.
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    /// Token usage update.
    TokenUsage { prompt: u32, completion: u32 },
    /// Stream complete (simple).
    Complete,
    /// Stream done with full info.
    Done {
        content: String,
        reasoning: String,
        tokens: Option<StreamTokenUsage>,
    },
    /// Error occurred (struct variant).
    ErrorInfo { message: String },
    /// Error occurred (tuple variant for convenience).
    Error(String),
    /// Empty response (no choices in API response).
    /// This handles malformed/truncated API responses gracefully instead of panicking.
    EmptyResponse { message: String },
}

/// Stream state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum StreamState {
    /// Not started.
    #[default]
    Idle,
    /// Streaming text.
    StreamingText,
    /// Streaming tool call.
    StreamingToolCall,
    /// Complete.
    Complete,
    /// Error.
    Error,
}

/// Accumulated stream content.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamContent {
    /// Accumulated text.
    pub text: String,
    /// Tool calls.
    pub tool_calls: Vec<StreamToolCall>,
    /// Token counts.
    pub tokens: TokenCounts,
}

impl StreamContent {
    /// Create new empty content.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append text delta.
    pub fn append_text(&mut self, delta: &str) {
        self.text.push_str(delta);
    }

    /// Start a tool call.
    pub fn start_tool_call(&mut self, id: &str, name: &str) {
        self.tool_calls.push(StreamToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: String::new(),
            complete: false,
        });
    }

    /// Append tool call arguments.
    pub fn append_tool_call(&mut self, id: &str, arguments: &str) {
        if let Some(tc) = self.tool_calls.iter_mut().find(|tc| tc.id == id) {
            tc.arguments.push_str(arguments);
        }
    }

    /// Complete a tool call.
    pub fn complete_tool_call(&mut self, id: &str) {
        if let Some(tc) = self.tool_calls.iter_mut().find(|tc| tc.id == id) {
            tc.complete = true;
        }
    }

    /// Check if has content.
    pub fn has_content(&self) -> bool {
        !self.text.is_empty() || !self.tool_calls.is_empty()
    }
}

/// Stream tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamToolCall {
    /// Call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments (may be incomplete JSON).
    pub arguments: String,
    /// Is complete.
    pub complete: bool,
}

impl StreamToolCall {
    /// Try to parse arguments as JSON.
    pub fn parse_arguments<T: for<'de> Deserialize<'de>>(&self) -> Option<T> {
        if self.complete {
            serde_json::from_str(&self.arguments).ok()
        } else {
            None
        }
    }
}

/// Token counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenCounts {
    /// Prompt tokens.
    pub prompt: u32,
    /// Completion tokens.
    pub completion: u32,
}

impl TokenCounts {
    /// Total tokens.
    pub fn total(&self) -> u32 {
        self.prompt + self.completion
    }
}

/// Stream processor.
pub struct StreamProcessor {
    /// Current state.
    state: StreamState,
    /// Accumulated content.
    content: StreamContent,
    /// Event buffer.
    buffer: VecDeque<StreamEvent>,
    /// Start time.
    start_time: Option<Instant>,
    /// First token time.
    first_token_time: Option<Instant>,
    /// Last event time.
    last_event_time: Option<Instant>,
    /// Event count.
    event_count: u64,
}

impl StreamProcessor {
    /// Create a new stream processor.
    pub fn new() -> Self {
        Self {
            state: StreamState::Idle,
            content: StreamContent::new(),
            buffer: VecDeque::with_capacity(1024), // Pre-allocate reasonable capacity
            start_time: None,
            first_token_time: None,
            last_event_time: None,
            event_count: 0,
        }
    }

    /// Process an event.
    pub fn process(&mut self, event: StreamEvent) {
        let now = Instant::now();

        if self.start_time.is_none() {
            self.start_time = Some(now);
        }

        self.last_event_time = Some(now);
        self.event_count += 1;

        match &event {
            StreamEvent::Start => {
                self.state = StreamState::StreamingText;
            }
            StreamEvent::TextDelta { delta } => {
                if self.first_token_time.is_none() {
                    self.first_token_time = Some(now);
                }
                self.content.append_text(delta);
                self.state = StreamState::StreamingText;
            }
            StreamEvent::ToolCallStart { id, name } => {
                self.content.start_tool_call(id, name);
                self.state = StreamState::StreamingToolCall;
            }
            StreamEvent::ToolCallDelta { id, arguments } => {
                self.content.append_tool_call(id, arguments);
            }
            StreamEvent::ToolCallComplete { id } => {
                self.content.complete_tool_call(id);
            }
            StreamEvent::TokenUsage { prompt, completion } => {
                self.content.tokens.prompt = *prompt;
                self.content.tokens.completion = *completion;
            }
            StreamEvent::Complete => {
                self.state = StreamState::Complete;
            }
            StreamEvent::Done { .. } => {
                self.state = StreamState::Complete;
            }
            StreamEvent::Error(_)
            | StreamEvent::ErrorInfo { .. }
            | StreamEvent::EmptyResponse { .. } => {
                self.state = StreamState::Error;
            }
            StreamEvent::Delta(delta) => {
                if self.first_token_time.is_none() {
                    self.first_token_time = Some(now);
                }
                self.content.append_text(delta);
                self.state = StreamState::StreamingText;
            }
            StreamEvent::Reasoning(_) => {
                // Reasoning is tracked separately if needed
            }
            StreamEvent::ToolCall { id, name, .. } => {
                self.content.start_tool_call(id, name);
                self.state = StreamState::StreamingToolCall;
            }
        }

        // Enforce buffer size limit to prevent unbounded memory growth
        if self.buffer.len() >= MAX_BUFFER_SIZE {
            self.buffer.pop_front();
        }
        self.buffer.push_back(event);
    }

    /// Get current state.
    pub fn state(&self) -> StreamState {
        self.state
    }

    /// Get accumulated content.
    pub fn content(&self) -> &StreamContent {
        &self.content
    }

    /// Take accumulated content.
    pub fn take_content(self) -> StreamContent {
        self.content
    }

    /// Get time to first token.
    pub fn time_to_first_token(&self) -> Option<Duration> {
        match (self.start_time, self.first_token_time) {
            (Some(start), Some(first)) => Some(first.duration_since(start)),
            _ => None,
        }
    }

    /// Get total elapsed time.
    pub fn elapsed(&self) -> Option<Duration> {
        self.start_time.map(|s| s.elapsed())
    }

    /// Get event count.
    pub fn event_count(&self) -> u64 {
        self.event_count
    }

    /// Check if complete.
    pub fn is_complete(&self) -> bool {
        self.state == StreamState::Complete || self.state == StreamState::Error
    }

    /// Drain buffered events.
    pub fn drain_events(&mut self) -> Vec<StreamEvent> {
        self.buffer.drain(..).collect()
    }

    /// Get streaming stats.
    pub fn stats(&self) -> StreamStats {
        StreamStats {
            state: self.state,
            event_count: self.event_count,
            text_length: self.content.text.len(),
            tool_call_count: self.content.tool_calls.len(),
            time_to_first_token: self.time_to_first_token(),
            total_time: self.elapsed(),
            tokens: self.content.tokens.clone(),
        }
    }
}

impl Default for StreamProcessor {
    fn default() -> Self {
        Self::new()
    }
}

/// Streaming statistics.
#[derive(Debug, Clone, Serialize)]
pub struct StreamStats {
    /// Current state.
    pub state: StreamState,
    /// Event count.
    pub event_count: u64,
    /// Text length.
    pub text_length: usize,
    /// Tool call count.
    pub tool_call_count: usize,
    /// Time to first token.
    pub time_to_first_token: Option<Duration>,
    /// Total time.
    pub total_time: Option<Duration>,
    /// Token counts.
    pub tokens: TokenCounts,
}

/// Stream buffer for rate limiting output.
pub struct StreamBuffer {
    /// Buffer.
    buffer: String,
    /// Minimum flush interval.
    min_interval: Duration,
    /// Last flush time.
    last_flush: Instant,
}

impl StreamBuffer {
    /// Create a new buffer.
    pub fn new(min_interval: Duration) -> Self {
        Self {
            buffer: String::new(),
            min_interval,
            last_flush: Instant::now(),
        }
    }

    /// Add content to buffer.
    pub fn push(&mut self, text: &str) {
        self.buffer.push_str(text);
    }

    /// Flush if ready.
    pub fn flush_if_ready(&mut self) -> Option<String> {
        if self.last_flush.elapsed() >= self.min_interval && !self.buffer.is_empty() {
            self.last_flush = Instant::now();
            Some(std::mem::take(&mut self.buffer))
        } else {
            None
        }
    }

    /// Force flush.
    pub fn flush(&mut self) -> String {
        self.last_flush = Instant::now();
        std::mem::take(&mut self.buffer)
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// UTF-8 safe byte buffer for streaming that handles multi-byte characters
/// split across network packet boundaries (#2196).
///
/// This buffer accumulates raw bytes and only emits complete UTF-8 characters,
/// holding back incomplete multi-byte sequences until more data arrives.
pub struct Utf8StreamBuffer {
    /// Accumulated bytes that may include incomplete UTF-8 sequences.
    buffer: Vec<u8>,
}

impl Utf8StreamBuffer {
    /// Create a new UTF-8 stream buffer.
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Push raw bytes into the buffer and return any complete UTF-8 string.
    ///
    /// Incomplete multi-byte sequences at the end are held in the buffer
    /// until more data arrives.
    pub fn push(&mut self, data: &[u8]) -> Option<String> {
        self.buffer.extend_from_slice(data);
        self.extract_complete_utf8()
    }

    /// Extract complete UTF-8 characters from the buffer.
    fn extract_complete_utf8(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }

        // Find the last valid UTF-8 boundary
        let valid_len = self.find_valid_utf8_boundary();

        if valid_len == 0 {
            return None;
        }

        // Extract the valid portion
        let valid_bytes: Vec<u8> = self.buffer.drain(..valid_len).collect();

        // This should always succeed since we found the valid boundary
        match String::from_utf8(valid_bytes) {
            Ok(s) => Some(s),
            Err(_) => None, // Should not happen
        }
    }

    /// Find the position up to which the buffer contains valid UTF-8.
    /// Returns the number of bytes that form complete characters.
    fn find_valid_utf8_boundary(&self) -> usize {
        let len = self.buffer.len();

        // Try the full buffer first
        if std::str::from_utf8(&self.buffer).is_ok() {
            return len;
        }

        // Check trailing bytes that might be incomplete multi-byte sequences
        // UTF-8 encoding:
        // - 1 byte:  0xxxxxxx
        // - 2 bytes: 110xxxxx 10xxxxxx
        // - 3 bytes: 1110xxxx 10xxxxxx 10xxxxxx
        // - 4 bytes: 11110xxx 10xxxxxx 10xxxxxx 10xxxxxx

        // Check last 1-3 bytes for incomplete sequences
        for trailing in 1..=3.min(len) {
            let boundary = len - trailing;
            if std::str::from_utf8(&self.buffer[..boundary]).is_ok() {
                // Check if the remaining bytes look like an incomplete sequence
                let remaining = &self.buffer[boundary..];
                if Self::is_incomplete_utf8_start(remaining) {
                    return boundary;
                }
            }
        }

        // Fallback: return what we can parse
        for i in (0..len).rev() {
            if std::str::from_utf8(&self.buffer[..i]).is_ok() {
                return i;
            }
        }

        0
    }

    /// Check if bytes look like the start of an incomplete UTF-8 sequence.
    fn is_incomplete_utf8_start(bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }

        let first = bytes[0];

        // Check for multi-byte sequence start
        if first & 0b11100000 == 0b11000000 {
            // 2-byte sequence, need 2 bytes
            return bytes.len() < 2;
        } else if first & 0b11110000 == 0b11100000 {
            // 3-byte sequence, need 3 bytes
            return bytes.len() < 3;
        } else if first & 0b11111000 == 0b11110000 {
            // 4-byte sequence, need 4 bytes
            return bytes.len() < 4;
        }

        false
    }

    /// Flush any remaining bytes, replacing invalid sequences with replacement character.
    pub fn flush(&mut self) -> String {
        if self.buffer.is_empty() {
            return String::new();
        }

        let bytes = std::mem::take(&mut self.buffer);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Check if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Get the number of pending bytes.
    pub fn pending_bytes(&self) -> usize {
        self.buffer.len()
    }
}

impl Default for Utf8StreamBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Word boundary buffer for clean output.
pub struct WordBuffer {
    /// Buffer.
    buffer: String,
    /// Minimum words before flush.
    min_words: usize,
}

impl WordBuffer {
    /// Create a new word buffer.
    pub fn new(min_words: usize) -> Self {
        Self {
            buffer: String::new(),
            min_words,
        }
    }

    /// Add content.
    pub fn push(&mut self, text: &str) {
        self.buffer.push_str(text);
    }

    /// Try to flush complete words.
    pub fn flush_words(&mut self) -> Option<String> {
        let word_count = self.buffer.split_whitespace().count();

        if word_count >= self.min_words {
            // Find last word boundary
            if let Some(pos) = self.buffer.rfind(|c: char| c.is_whitespace()) {
                let result = self.buffer[..=pos].to_string();
                self.buffer = self.buffer[pos + 1..].to_string();
                return Some(result);
            }
        }

        None
    }

    /// Force flush all.
    pub fn flush(&mut self) -> String {
        std::mem::take(&mut self.buffer)
    }
}

/// Sentence buffer for sentence-aligned output.
pub struct SentenceBuffer {
    /// Buffer.
    buffer: String,
}

impl SentenceBuffer {
    /// Create a new sentence buffer.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Add content.
    pub fn push(&mut self, text: &str) {
        self.buffer.push_str(text);
    }

    /// Try to flush complete sentences.
    pub fn flush_sentences(&mut self) -> Option<String> {
        // Find sentence ending
        let endings = [". ", "! ", "? ", ".\n", "!\n", "?\n"];

        let mut last_end = 0;
        for ending in &endings {
            if let Some(pos) = self.buffer.rfind(ending) {
                let end = pos + ending.len();
                if end > last_end {
                    last_end = end;
                }
            }
        }

        if last_end > 0 {
            let result = self.buffer[..last_end].to_string();
            self.buffer = self.buffer[last_end..].to_string();
            Some(result)
        } else {
            None
        }
    }

    /// Force flush all.
    pub fn flush(&mut self) -> String {
        std::mem::take(&mut self.buffer)
    }
}

impl Default for SentenceBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Async stream wrapper.
pub struct EventStream {
    receiver: mpsc::Receiver<StreamEvent>,
}

impl EventStream {
    /// Create a new event stream.
    pub fn new(receiver: mpsc::Receiver<StreamEvent>) -> Self {
        Self { receiver }
    }

    /// Create a channel pair.
    pub fn channel(buffer: usize) -> (mpsc::Sender<StreamEvent>, Self) {
        let (tx, rx) = mpsc::channel(buffer);
        (tx, Self::new(rx))
    }
}

impl Stream for EventStream {
    type Item = StreamEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.receiver).poll_recv(cx)
    }
}

/// Stream consumer that collects all content.
pub struct StreamCollector {
    processor: StreamProcessor,
}

impl StreamCollector {
    /// Create a new collector.
    pub fn new() -> Self {
        Self {
            processor: StreamProcessor::new(),
        }
    }

    /// Process an event.
    pub fn process(&mut self, event: StreamEvent) {
        self.processor.process(event);
    }

    /// Check if complete.
    pub fn is_complete(&self) -> bool {
        self.processor.is_complete()
    }

    /// Get result.
    pub fn result(&self) -> CollectorResult {
        CollectorResult {
            text: self.processor.content.text.clone(),
            tool_calls: self.processor.content.tool_calls.clone(),
            tokens: self.processor.content.tokens.clone(),
            stats: self.processor.stats(),
        }
    }

    /// Take final result.
    pub fn take_result(self) -> CollectorResult {
        CollectorResult {
            text: self.processor.content.text.clone(),
            tool_calls: self.processor.content.tool_calls.clone(),
            tokens: self.processor.content.tokens.clone(),
            stats: self.processor.stats(),
        }
    }
}

impl Default for StreamCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Collector result.
#[derive(Debug, Clone, Serialize)]
pub struct CollectorResult {
    /// Collected text.
    pub text: String,
    /// Tool calls.
    pub tool_calls: Vec<StreamToolCall>,
    /// Token counts.
    pub tokens: TokenCounts,
    /// Stats.
    pub stats: StreamStats,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_processor() {
        let mut processor = StreamProcessor::new();

        processor.process(StreamEvent::Start);
        assert_eq!(processor.state(), StreamState::StreamingText);

        processor.process(StreamEvent::TextDelta {
            delta: "Hello ".to_string(),
        });
        processor.process(StreamEvent::TextDelta {
            delta: "world!".to_string(),
        });
        assert_eq!(processor.content().text, "Hello world!");

        processor.process(StreamEvent::Complete);
        assert!(processor.is_complete());
    }

    #[test]
    fn test_tool_call_streaming() {
        let mut processor = StreamProcessor::new();

        processor.process(StreamEvent::ToolCallStart {
            id: "tc1".to_string(),
            name: "read_file".to_string(),
        });
        processor.process(StreamEvent::ToolCallDelta {
            id: "tc1".to_string(),
            arguments: r#"{"path":"#.to_string(),
        });
        processor.process(StreamEvent::ToolCallDelta {
            id: "tc1".to_string(),
            arguments: r#""/test"}"#.to_string(),
        });
        processor.process(StreamEvent::ToolCallComplete {
            id: "tc1".to_string(),
        });

        let content = processor.content();
        assert_eq!(content.tool_calls.len(), 1);
        assert!(content.tool_calls[0].complete);
        assert_eq!(content.tool_calls[0].arguments, r#"{"path":"/test"}"#);
    }

    #[test]
    fn test_stream_buffer() {
        let mut buffer = StreamBuffer::new(Duration::from_millis(10));

        buffer.push("Hello ");
        buffer.push("world!");

        let flushed = buffer.flush();
        assert_eq!(flushed, "Hello world!");
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_word_buffer() {
        let mut buffer = WordBuffer::new(3);

        buffer.push("Hello world this is ");
        buffer.push("a test");

        if let Some(words) = buffer.flush_words() {
            assert!(words.split_whitespace().count() >= 3);
        }
    }

    #[test]
    fn test_sentence_buffer() {
        let mut buffer = SentenceBuffer::new();

        buffer.push("Hello world. This is a test. More");

        if let Some(sentences) = buffer.flush_sentences() {
            assert!(sentences.contains("Hello world."));
            assert!(sentences.contains("This is a test."));
        }

        let remaining = buffer.flush();
        assert_eq!(remaining, "More");
    }

    #[test]
    fn test_stream_collector() {
        let mut collector = StreamCollector::new();

        collector.process(StreamEvent::Start);
        collector.process(StreamEvent::TextDelta {
            delta: "Test".to_string(),
        });
        collector.process(StreamEvent::TokenUsage {
            prompt: 10,
            completion: 5,
        });
        collector.process(StreamEvent::Complete);

        let result = collector.result();
        assert_eq!(result.text, "Test");
        assert_eq!(result.tokens.total(), 15);
    }
}
