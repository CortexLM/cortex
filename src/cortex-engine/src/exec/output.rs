//! Output capture utilities.

use std::sync::{Arc, Mutex};

/// Captures output from multiple sources.
#[derive(Debug, Clone, Default)]
pub struct OutputCapture {
    inner: Arc<Mutex<OutputCaptureInner>>,
}

#[derive(Debug, Default)]
struct OutputCaptureInner {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    combined: Vec<OutputChunk>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct OutputChunk {
    stream: OutputStream,
    data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

impl OutputCapture {
    /// Create a new output capture.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append data to stdout.
    pub fn append_stdout(&self, data: &[u8]) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.stdout.extend_from_slice(data);
            inner.combined.push(OutputChunk {
                stream: OutputStream::Stdout,
                data: data.to_vec(),
            });
        }
    }

    /// Append data to stderr.
    pub fn append_stderr(&self, data: &[u8]) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.stderr.extend_from_slice(data);
            inner.combined.push(OutputChunk {
                stream: OutputStream::Stderr,
                data: data.to_vec(),
            });
        }
    }

    /// Get stdout content.
    pub fn stdout(&self) -> Vec<u8> {
        self.inner
            .lock()
            .map(|i| i.stdout.clone())
            .unwrap_or_default()
    }

    /// Get stderr content.
    pub fn stderr(&self) -> Vec<u8> {
        self.inner
            .lock()
            .map(|i| i.stderr.clone())
            .unwrap_or_default()
    }

    /// Get combined output in order received.
    pub fn combined(&self) -> Vec<u8> {
        self.inner
            .lock()
            .map(|i| i.combined.iter().flat_map(|c| c.data.clone()).collect())
            .unwrap_or_default()
    }

    /// Get stdout as string.
    pub fn stdout_string(&self) -> String {
        String::from_utf8_lossy(&self.stdout()).to_string()
    }

    /// Get stderr as string.
    pub fn stderr_string(&self) -> String {
        String::from_utf8_lossy(&self.stderr()).to_string()
    }

    /// Get combined output as string.
    pub fn combined_string(&self) -> String {
        String::from_utf8_lossy(&self.combined()).to_string()
    }

    /// Clear all captured output.
    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.stdout.clear();
            inner.stderr.clear();
            inner.combined.clear();
        }
    }
}
