//! Cortex Feedback - Feedback collection and reporting.
//!
//! This module provides functionality to collect logs and feedback
//! from user sessions for debugging and improvement purposes.

use std::collections::VecDeque;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use cortex_engine::create_default_client;
use cortex_protocol::ConversationId;
use tracing_subscriber::fmt::writer::MakeWriter;

/// Default maximum bytes to store in the ring buffer (4 MiB).
const DEFAULT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Timeout for feedback upload in seconds.
const _UPLOAD_TIMEOUT_SECS: u64 = 10;

/// Feedback collector that captures logs in a ring buffer.
#[derive(Clone)]
pub struct CortexFeedback {
    inner: Arc<FeedbackInner>,
}

impl Default for CortexFeedback {
    fn default() -> Self {
        Self::new()
    }
}

impl CortexFeedback {
    /// Create a new feedback collector with default capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_BYTES)
    }

    /// Create a new feedback collector with specified capacity.
    pub fn with_capacity(max_bytes: usize) -> Self {
        Self {
            inner: Arc::new(FeedbackInner::new(max_bytes)),
        }
    }

    /// Create a writer for use with tracing subscriber.
    pub fn make_writer(&self) -> FeedbackMakeWriter {
        FeedbackMakeWriter {
            inner: self.inner.clone(),
        }
    }

    /// Take a snapshot of the current logs.
    pub fn snapshot(&self, session_id: Option<ConversationId>) -> FeedbackSnapshot {
        let bytes = {
            let guard = self
                .inner
                .ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.snapshot_bytes()
        };
        FeedbackSnapshot {
            bytes,
            thread_id: session_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| format!("no-active-thread-{}", uuid::Uuid::new_v4())),
        }
    }
}

struct FeedbackInner {
    ring: Mutex<RingBuffer>,
}

impl FeedbackInner {
    fn new(max_bytes: usize) -> Self {
        Self {
            ring: Mutex::new(RingBuffer::new(max_bytes)),
        }
    }
}

/// Writer factory for tracing subscriber integration.
#[derive(Clone)]
pub struct FeedbackMakeWriter {
    inner: Arc<FeedbackInner>,
}

impl<'a> MakeWriter<'a> for FeedbackMakeWriter {
    type Writer = FeedbackWriter;

    fn make_writer(&'a self) -> Self::Writer {
        FeedbackWriter {
            inner: self.inner.clone(),
        }
    }
}

/// Writer that captures log output to the ring buffer.
pub struct FeedbackWriter {
    inner: Arc<FeedbackInner>,
}

impl Write for FeedbackWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self.inner.ring.lock().map_err(|_| io::ErrorKind::Other)?;
        guard.push_bytes(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Ring buffer for efficient log storage with bounded memory usage.
struct RingBuffer {
    max: usize,
    buf: VecDeque<u8>,
}

impl RingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            max: capacity,
            buf: VecDeque::with_capacity(capacity),
        }
    }

    fn len(&self) -> usize {
        self.buf.len()
    }

    fn push_bytes(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        // If incoming chunk is larger than capacity, keep only trailing bytes
        if data.len() >= self.max {
            self.buf.clear();
            let start = data.len() - self.max;
            self.buf.extend(data[start..].iter().copied());
            return;
        }

        // Evict from front if we would exceed capacity
        let needed = self.len() + data.len();
        if needed > self.max {
            let to_drop = needed - self.max;
            for _ in 0..to_drop {
                let _ = self.buf.pop_front();
            }
        }

        self.buf.extend(data.iter().copied());
    }

    fn snapshot_bytes(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }
}

/// Snapshot of feedback logs ready for submission.
pub struct FeedbackSnapshot {
    bytes: Vec<u8>,
    /// Thread/session identifier.
    pub thread_id: String,
}

impl FeedbackSnapshot {
    /// Get the raw bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Get the logs as a string (lossy conversion).
    pub fn as_string(&self) -> String {
        String::from_utf8_lossy(&self.bytes).to_string()
    }

    /// Save the snapshot to a temporary file.
    pub fn save_to_temp_file(&self) -> io::Result<PathBuf> {
        let dir = std::env::temp_dir();
        let filename = format!("cortex-feedback-{}.log", self.thread_id);
        let path = dir.join(filename);
        fs::write(&path, self.as_bytes())?;
        Ok(path)
    }

    /// Upload feedback to the feedback service.
    pub async fn upload_feedback(
        &self,
        classification: &str,
        reason: Option<&str>,
        include_logs: bool,
        rollout_path: Option<&std::path::Path>,
        feedback_endpoint: Option<&str>,
    ) -> Result<()> {
        let endpoint =
            feedback_endpoint.unwrap_or("https://feedback.cortex.foundation/api/v1/feedback");

        let client = create_default_client()?;

        let mut payload = serde_json::json!({
            "thread_id": self.thread_id,
            "classification": classification,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "cli_version": env!("CARGO_PKG_VERSION"),
        });

        if let Some(r) = reason {
            payload["reason"] = serde_json::Value::String(r.to_string());
        }

        if include_logs {
            payload["logs"] = serde_json::Value::String(self.as_string());
        }

        if let Some(path) = rollout_path
            && let Ok(content) = fs::read_to_string(path)
        {
            payload["rollout"] = serde_json::Value::String(content);
        }

        let response = client.post(endpoint).json(&payload).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("feedback upload failed: {status} - {body}");
        }

        Ok(())
    }
}

/// Feedback classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackClassification {
    /// Bug report.
    Bug,
    /// Bad result from the AI.
    BadResult,
    /// Good result from the AI.
    GoodResult,
    /// Other feedback.
    Other,
}

impl FeedbackClassification {
    /// Get the string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bug => "bug",
            Self::BadResult => "bad_result",
            Self::GoodResult => "good_result",
            Self::Other => "other",
        }
    }

    /// Get the display name.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Bug => "Bug",
            Self::BadResult => "Bad result",
            Self::GoodResult => "Good result",
            Self::Other => "Other",
        }
    }
}

impl std::fmt::Display for FeedbackClassification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_drops_front_when_full() {
        let fb = CortexFeedback::with_capacity(8);
        {
            let mut w = fb.make_writer().make_writer();
            w.write_all(b"abcdefgh").unwrap();
            w.write_all(b"ij").unwrap();
        }
        let snap = fb.snapshot(None);
        // Capacity 8: after writing 10 bytes, we should keep the last 8
        assert_eq!(std::str::from_utf8(snap.as_bytes()).unwrap(), "cdefghij");
    }

    #[test]
    fn ring_buffer_handles_large_chunk() {
        let fb = CortexFeedback::with_capacity(4);
        {
            let mut w = fb.make_writer().make_writer();
            w.write_all(b"abcdefgh").unwrap();
        }
        let snap = fb.snapshot(None);
        // Should keep only last 4 bytes
        assert_eq!(std::str::from_utf8(snap.as_bytes()).unwrap(), "efgh");
    }

    #[test]
    fn feedback_classification_display() {
        assert_eq!(FeedbackClassification::Bug.as_str(), "bug");
        assert_eq!(
            FeedbackClassification::BadResult.display_name(),
            "Bad result"
        );
    }
}
