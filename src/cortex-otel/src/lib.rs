//! Cortex OpenTelemetry - Telemetry integration for Cortex CLI.
//!
//! This module provides optional OpenTelemetry integration for
//! tracing, metrics, and logging.

pub mod config;

#[cfg(feature = "otel")]
pub mod otel_provider;

#[cfg(feature = "otel")]
pub mod otel_event_manager;

// When otel feature is disabled, provide stub implementations
#[cfg(not(feature = "otel"))]
mod stub {
    use reqwest::header::HeaderMap;
    use tracing::Span;

    /// Stub OpenTelemetry provider when feature is disabled.
    pub struct OtelProvider;

    impl OtelProvider {
        /// Create a new provider (no-op when feature is disabled).
        pub fn from(_settings: &crate::config::OtelSettings) -> Option<Self> {
            None
        }

        /// Get headers for a span (empty when feature is disabled).
        pub fn headers(_span: &Span) -> HeaderMap {
            HeaderMap::new()
        }

        /// Shutdown the provider (no-op when feature is disabled).
        pub fn shutdown(&self) {}
    }
}

#[cfg(not(feature = "otel"))]
pub use stub::OtelProvider;

#[cfg(feature = "otel")]
pub use otel_provider::OtelProvider;
