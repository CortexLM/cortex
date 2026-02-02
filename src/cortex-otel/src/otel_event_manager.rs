//! OpenTelemetry event management.

#[cfg(feature = "otel")]
use opentelemetry::trace::{Span, SpanKind, Status, Tracer};
#[cfg(feature = "otel")]
use opentelemetry_sdk::trace::Tracer as SdkTracer;

/// Event types for telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    /// User input received.
    UserInput,
    /// Agent response started.
    AgentResponseStart,
    /// Agent response completed.
    AgentResponseEnd,
    /// Tool execution started.
    ToolExecutionStart,
    /// Tool execution completed.
    ToolExecutionEnd,
    /// Error occurred.
    Error,
    /// Session started.
    SessionStart,
    /// Session ended.
    SessionEnd,
}

impl EventType {
    /// Get the event name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::UserInput => "user_input",
            Self::AgentResponseStart => "agent_response_start",
            Self::AgentResponseEnd => "agent_response_end",
            Self::ToolExecutionStart => "tool_execution_start",
            Self::ToolExecutionEnd => "tool_execution_end",
            Self::Error => "error",
            Self::SessionStart => "session_start",
            Self::SessionEnd => "session_end",
        }
    }
}

/// Manager for OpenTelemetry events.
#[cfg(feature = "otel")]
pub struct OtelEventManager {
    tracer: SdkTracer,
}

#[cfg(feature = "otel")]
impl OtelEventManager {
    /// Create a new event manager.
    pub fn new(tracer: SdkTracer) -> Self {
        Self { tracer }
    }

    /// Record an event.
    pub fn record_event(&self, event_type: EventType, attributes: Vec<(&'static str, String)>) {
        let mut span = self
            .tracer
            .span_builder(event_type.name())
            .with_kind(SpanKind::Internal)
            .start(&self.tracer);

        for (key, value) in attributes {
            span.set_attribute(opentelemetry::KeyValue::new(key, value));
        }

        span.end();
    }

    /// Record an error event.
    pub fn record_error(&self, error: &str, attributes: Vec<(&'static str, String)>) {
        let mut span = self
            .tracer
            .span_builder(EventType::Error.name())
            .with_kind(SpanKind::Internal)
            .start(&self.tracer);

        span.set_status(Status::error(error.to_string()));
        span.record_error(&std::io::Error::other(error));

        for (key, value) in attributes {
            span.set_attribute(opentelemetry::KeyValue::new(key, value));
        }

        span.end();
    }

    /// Start a span for a long-running operation.
    pub fn start_span(&self, name: &'static str) -> impl Span {
        self.tracer
            .span_builder(name)
            .with_kind(SpanKind::Internal)
            .start(&self.tracer)
    }
}

/// Stub event manager when otel feature is disabled.
#[cfg(not(feature = "otel"))]
pub struct OtelEventManager;

#[cfg(not(feature = "otel"))]
impl OtelEventManager {
    /// Create a new event manager (no-op).
    pub fn new() -> Self {
        Self
    }

    /// Record an event (no-op).
    pub fn record_event(&self, _event_type: EventType, _attributes: Vec<(&'static str, String)>) {}

    /// Record an error event (no-op).
    pub fn record_error(&self, _error: &str, _attributes: Vec<(&'static str, String)>) {}
}

#[cfg(not(feature = "otel"))]
impl Default for OtelEventManager {
    fn default() -> Self {
        Self::new()
    }
}
