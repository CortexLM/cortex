//! OpenTelemetry provider implementation.

#[cfg(feature = "otel")]
use opentelemetry::KeyValue;
#[cfg(feature = "otel")]
use opentelemetry::trace::TracerProvider as _;
#[cfg(feature = "otel")]
use opentelemetry_otlp::WithExportConfig;
#[cfg(feature = "otel")]
use opentelemetry_sdk::{
    Resource,
    trace::{Sampler, SdkTracerProvider},
};
#[cfg(feature = "otel")]
use opentelemetry_semantic_conventions::resource::SERVICE_NAME;
#[cfg(feature = "otel")]
use reqwest::header::HeaderMap;
#[cfg(feature = "otel")]
use tracing::Span;

use crate::config::OtelSettings;

/// OpenTelemetry provider for Cortex CLI.
#[cfg(feature = "otel")]
pub struct OtelProvider {
    tracer_provider: SdkTracerProvider,
}

#[cfg(feature = "otel")]
impl OtelProvider {
    /// Create a new provider from settings.
    pub fn from(settings: &OtelSettings) -> Option<Self> {
        if !settings.should_initialize() {
            return None;
        }

        let endpoint = settings.endpoint.as_ref()?;

        // Build resource attributes
        let mut attrs = vec![KeyValue::new(SERVICE_NAME, settings.service_name.clone())];

        if let Some(version) = &settings.service_version {
            attrs.push(KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_VERSION,
                version.clone(),
            ));
        }

        for (key, value) in &settings.resource_attributes {
            attrs.push(KeyValue::new(key.clone(), value.clone()));
        }

        let resource = Resource::builder_empty().with_attributes(attrs).build();

        // Configure sampler
        let sampler = if settings.sampling_ratio >= 1.0 {
            Sampler::AlwaysOn
        } else if settings.sampling_ratio <= 0.0 {
            Sampler::AlwaysOff
        } else {
            Sampler::TraceIdRatioBased(settings.sampling_ratio)
        };

        // Build OTLP exporter using HTTP (more portable than tonic/gRPC)
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(endpoint)
            .with_timeout(std::time::Duration::from_secs(settings.export_timeout_secs))
            .build()
            .ok()?;

        // Build tracer provider
        let tracer_provider = SdkTracerProvider::builder()
            .with_resource(resource)
            .with_sampler(sampler)
            .with_batch_exporter(exporter)
            .build();

        Some(Self { tracer_provider })
    }

    /// Get trace context headers from the current span.
    pub fn headers(_span: &Span) -> HeaderMap {
        use opentelemetry::propagation::TextMapPropagator;
        use opentelemetry_sdk::propagation::TraceContextPropagator;

        let mut headers = HeaderMap::new();
        let propagator = TraceContextPropagator::new();

        // Create a carrier to inject headers
        struct HeaderCarrier<'a>(&'a mut HeaderMap);

        impl<'a> opentelemetry::propagation::Injector for HeaderCarrier<'a> {
            fn set(&mut self, key: &str, value: String) {
                if let Ok(name) = reqwest::header::HeaderName::from_bytes(key.as_bytes())
                    && let Ok(val) = reqwest::header::HeaderValue::from_str(&value)
                {
                    self.0.insert(name, val);
                }
            }
        }

        // Get the current context and inject into headers
        let cx = opentelemetry::Context::current();
        propagator.inject_context(&cx, &mut HeaderCarrier(&mut headers));

        headers
    }

    /// Shutdown the provider and flush any pending spans.
    pub fn shutdown(&self) {
        if let Err(e) = self.tracer_provider.shutdown() {
            tracing::warn!("failed to shutdown tracer provider: {e}");
        }
    }

    /// Get a tracer from this provider.
    pub fn tracer(&self, name: &'static str) -> opentelemetry_sdk::trace::Tracer {
        self.tracer_provider.tracer(name)
    }
}

#[cfg(feature = "otel")]
impl Drop for OtelProvider {
    fn drop(&mut self) {
        self.shutdown();
    }
}
