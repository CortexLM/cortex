//! Stream controller for managing streaming response state and buffering.
//!
//! This module provides the [`StreamController`] struct that manages the
//! lifecycle of a streaming response.

use std::time::{Duration, Instant};

use cortex_core::animation::Typewriter;

use super::state::StreamState;

/// Controller for managing streaming response state and buffering.
///
/// Handles the lifecycle of a streaming response, including:
/// - State transitions (idle → processing → streaming → complete)
/// - Text buffering with newline-gated display
/// - Time tracking for metrics
/// - Integration with Typewriter animation
///
/// # Newline-Gated Display
///
/// By default, text is only displayed after a newline is received.
/// This prevents partial words from appearing and provides a smoother
/// reading experience. Use [`StreamController::immediate_display`] to
/// disable this behavior.
///
/// # Typewriter Animation
///
/// When created with [`StreamController::with_typewriter`], text is
/// revealed character-by-character at the specified rate. Call
/// [`StreamController::tick`] each frame to advance the animation.
pub struct StreamController {
    /// Current state of the stream.
    state: StreamState,

    /// Buffer for incoming text (before newline gate).
    pending_buffer: String,

    /// Committed text (after newline).
    committed_buffer: String,

    /// Typewriter for animated text reveal.
    typewriter: Option<Typewriter>,

    /// Whether to use newline-gated display.
    newline_gated: bool,

    /// Token count for current stream.
    token_count: u32,

    /// Time of first token.
    first_token_time: Option<Instant>,

    /// Start time of current operation.
    start_time: Option<Instant>,
}

impl StreamController {
    /// Creates a new stream controller with default settings.
    ///
    /// The controller starts in [`StreamState::Idle`] with newline-gated
    /// display enabled and no typewriter animation.
    pub fn new() -> Self {
        Self {
            state: StreamState::Idle,
            pending_buffer: String::new(),
            committed_buffer: String::new(),
            typewriter: None,
            newline_gated: true,
            token_count: 0,
            first_token_time: None,
            start_time: None,
        }
    }

    /// Creates a new stream controller with typewriter animation.
    ///
    /// # Arguments
    ///
    /// * `chars_per_second` - Base characters per second for animation.
    ///   The typewriter will speed up automatically when text arrives faster.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Base speed of 120 chars/sec, speeds up dynamically
    /// let controller = StreamController::with_typewriter(120.0);
    /// ```
    pub fn with_typewriter(chars_per_second: f32) -> Self {
        let mut controller = Self::new();
        // Use dynamic typewriter that adapts to stream speed
        controller.typewriter = Some(Typewriter::dynamic(String::new(), chars_per_second));
        // Disable newline gating for immediate display
        controller.newline_gated = false;
        controller
    }

    /// Disables newline-gated display, showing characters immediately.
    ///
    /// By default, text is only shown after a newline is received.
    /// Call this method to show characters as soon as they arrive.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let controller = StreamController::new().immediate_display();
    /// ```
    pub fn immediate_display(mut self) -> Self {
        self.newline_gated = false;
        self
    }

    // --------------------------------------------------------
    // State Transition Methods
    // --------------------------------------------------------

    /// Starts processing state (user sent a message).
    ///
    /// Call this when a user message is submitted to begin tracking
    /// time-to-first-token metrics. Clears all buffers and resets counters.
    pub fn start_processing(&mut self) {
        self.state = StreamState::Processing;
        self.start_time = Some(Instant::now());
        self.pending_buffer.clear();
        self.committed_buffer.clear();
        self.token_count = 0;
        self.first_token_time = None;

        if let Some(ref mut tw) = self.typewriter {
            tw.set_text(String::new());
        }
    }

    /// Transitions to streaming state on first token.
    ///
    /// This is typically called automatically by [`StreamController::append_text`]
    /// when the first token arrives, but can be called manually if needed.
    pub fn start_streaming(&mut self) {
        if matches!(
            self.state,
            StreamState::Processing | StreamState::Reasoning { .. }
        ) {
            self.first_token_time = Some(Instant::now());
            self.state = StreamState::Streaming {
                tokens_received: 0,
                started_at: self.start_time.unwrap_or_else(Instant::now),
            };
        }
    }

    /// Enters reasoning mode.
    ///
    /// Use this when the AI enters a thinking/reasoning phase before
    /// generating the actual response.
    pub fn start_reasoning(&mut self) {
        self.state = StreamState::Reasoning {
            started_at: Instant::now(),
        };
    }

    /// Enters tool execution mode.
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool being executed.
    pub fn start_tool(&mut self, tool_name: String) {
        self.state = StreamState::ExecutingTool {
            tool_name,
            started_at: Instant::now(),
        };
    }

    /// Sets or clears tool execution state.
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Some(name) to start tool execution, None to clear it.
    pub fn set_executing_tool(&mut self, tool_name: Option<String>) {
        match tool_name {
            Some(name) => {
                self.state = StreamState::ExecutingTool {
                    tool_name: name,
                    started_at: Instant::now(),
                };
            }
            None => {
                // Only clear if we're in ExecutingTool state
                if matches!(self.state, StreamState::ExecutingTool { .. }) {
                    self.state = StreamState::Idle;
                }
            }
        }
    }

    /// Enters waiting for approval state.
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool awaiting approval.
    pub fn wait_approval(&mut self, tool_name: String) {
        self.state = StreamState::WaitingApproval { tool_name };
    }

    /// Marks the stream as complete.
    ///
    /// Flushes any remaining pending content and records final metrics.
    pub fn complete(&mut self) {
        let duration = self.start_time.map(|s| s.elapsed()).unwrap_or_default();

        // Flush any remaining pending content
        self.flush_pending();

        self.state = StreamState::Complete {
            total_tokens: self.token_count,
            duration,
        };
    }

    /// Marks the stream as interrupted by user.
    ///
    /// Flushes pending content before transitioning to interrupted state.
    pub fn interrupt(&mut self) {
        self.flush_pending();
        self.state = StreamState::Interrupted;
    }

    /// Sets the stream to error state.
    ///
    /// # Arguments
    ///
    /// * `message` - Error message describing what went wrong.
    pub fn set_error(&mut self, message: String) {
        self.state = StreamState::Error(message);
    }

    /// Resets the controller to idle state.
    ///
    /// Clears all buffers, resets counters, and prepares for a new stream.
    pub fn reset(&mut self) {
        self.state = StreamState::Idle;
        self.pending_buffer.clear();
        self.committed_buffer.clear();
        self.token_count = 0;
        self.first_token_time = None;
        self.start_time = None;

        if let Some(ref mut tw) = self.typewriter {
            tw.set_text(String::new());
        }
    }

    // --------------------------------------------------------
    // Text Handling Methods
    // --------------------------------------------------------

    /// Appends streaming text to the buffer.
    ///
    /// This is the primary method for feeding incoming tokens to the controller.
    /// It automatically transitions from Processing to Streaming state on the
    /// first token.
    ///
    /// # Newline-Gated Display
    ///
    /// When newline-gated display is enabled (default), text accumulates in
    /// the pending buffer until a newline is encountered. Only complete lines
    /// are moved to the committed buffer for display.
    ///
    /// # Arguments
    ///
    /// * `text` - The text to append (typically a single token).
    pub fn append_text(&mut self, text: &str) {
        // Auto-transition to streaming on first content
        if matches!(self.state, StreamState::Processing) {
            self.start_streaming();
        }

        self.token_count += 1;

        if self.newline_gated {
            // Add to pending buffer
            self.pending_buffer.push_str(text);

            // Check for newlines and commit complete lines
            while let Some(newline_pos) = self.pending_buffer.find('\n') {
                // Split at newline (including the newline character)
                let committed = self.pending_buffer[..=newline_pos].to_string();
                let rest = self.pending_buffer[newline_pos + 1..].to_string();

                self.committed_buffer.push_str(&committed);
                self.pending_buffer = rest;

                // Update typewriter
                if let Some(ref mut tw) = self.typewriter {
                    tw.append(&committed);
                }
            }
        } else {
            // Immediate display mode
            self.committed_buffer.push_str(text);
            if let Some(ref mut tw) = self.typewriter {
                tw.append(text);
            }
        }

        // Update token count in state
        if let StreamState::Streaming {
            ref mut tokens_received,
            ..
        } = self.state
        {
            *tokens_received = self.token_count;
        }
    }

    /// Flushes pending buffer to committed (on completion).
    ///
    /// This is called automatically by [`StreamController::complete`] and
    /// [`StreamController::interrupt`] to ensure all text is displayed.
    pub fn flush_pending(&mut self) {
        if !self.pending_buffer.is_empty() {
            self.committed_buffer.push_str(&self.pending_buffer);
            if let Some(ref mut tw) = self.typewriter {
                tw.append(&self.pending_buffer);
            }
            self.pending_buffer.clear();
        }
    }

    /// Returns displayable text (respects typewriter animation).
    ///
    /// If a typewriter is configured, returns only the currently visible
    /// portion of the text. Otherwise, returns all committed text.
    pub fn display_text(&self) -> &str {
        if let Some(ref tw) = self.typewriter {
            tw.visible_text()
        } else {
            &self.committed_buffer
        }
    }

    /// Returns full committed text (ignores typewriter animation).
    pub fn committed_text(&self) -> &str {
        &self.committed_buffer
    }

    /// Returns all text received so far (committed + pending).
    /// Use this when you need the complete text regardless of display state.
    pub fn full_text(&self) -> String {
        format!("{}{}", self.committed_buffer, self.pending_buffer)
    }

    /// Returns pending (uncommitted) text.
    ///
    /// This text has been received but not yet displayed because
    /// no newline has been encountered (when newline-gated).
    pub fn pending_text(&self) -> &str {
        &self.pending_buffer
    }

    /// Advances the typewriter animation by one frame.
    ///
    /// Call this method once per frame to advance the typewriter animation.
    /// Has no effect if no typewriter is configured.
    pub fn tick(&mut self) {
        if let Some(ref mut tw) = self.typewriter {
            tw.tick();
        }
    }

    /// Skips typewriter animation to show all text immediately.
    ///
    /// Use this when the user wants to skip the animation (e.g., pressing Enter).
    pub fn skip_animation(&mut self) {
        if let Some(ref mut tw) = self.typewriter {
            tw.skip_to_end();
        }
    }

    /// Returns `true` if the typewriter animation is complete.
    ///
    /// Always returns `true` if no typewriter is configured.
    pub fn animation_complete(&self) -> bool {
        self.typewriter.as_ref().is_none_or(|tw| tw.is_complete())
    }

    // --------------------------------------------------------
    // Metrics and Accessors
    // --------------------------------------------------------

    /// Returns the current state.
    #[inline]
    pub fn state(&self) -> &StreamState {
        &self.state
    }

    /// Returns the number of tokens received.
    #[inline]
    pub fn token_count(&self) -> u32 {
        self.token_count
    }

    /// Returns time-to-first-token (if available).
    ///
    /// This is the duration between starting processing and receiving
    /// the first token. Returns `None` if no tokens have been received.
    pub fn time_to_first_token(&self) -> Option<Duration> {
        match (self.start_time, self.first_token_time) {
            (Some(start), Some(first)) => Some(first.duration_since(start)),
            _ => None,
        }
    }

    /// Returns elapsed time since processing started.
    ///
    /// Returns `None` if processing hasn't started.
    pub fn elapsed(&self) -> Option<Duration> {
        self.start_time.map(|s| s.elapsed())
    }

    /// Returns `true` if actively streaming (alias for state().is_active()).
    #[inline]
    pub fn is_streaming(&self) -> bool {
        self.state.is_active()
    }

    /// Returns `true` if streaming completed successfully.
    #[inline]
    pub fn is_complete(&self) -> bool {
        matches!(self.state, StreamState::Complete { .. })
    }

    /// Returns `true` if newline-gated display is enabled.
    #[inline]
    pub fn is_newline_gated(&self) -> bool {
        self.newline_gated
    }

    /// Returns `true` if a typewriter animation is configured.
    #[inline]
    pub fn has_typewriter(&self) -> bool {
        self.typewriter.is_some()
    }

    /// Returns the number of characters visible (via typewriter) vs total committed.
    ///
    /// Returns `(visible, total)`. If no typewriter is configured,
    /// visible equals total.
    pub fn visible_progress(&self) -> (usize, usize) {
        let total = self.committed_buffer.chars().count();
        let visible = self
            .typewriter
            .as_ref()
            .map_or(total, |tw| tw.visible_char_count());
        (visible, total)
    }
}

impl Default for StreamController {
    fn default() -> Self {
        Self::new()
    }
}
