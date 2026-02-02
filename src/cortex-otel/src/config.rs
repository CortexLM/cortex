//! OpenTelemetry configuration.

use serde::{Deserialize, Serialize};

/// OpenTelemetry settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtelSettings {
    /// Whether OpenTelemetry is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// OTLP endpoint URL.
    #[serde(default)]
    pub endpoint: Option<String>,

    /// Service name for telemetry.
    #[serde(default = "default_service_name")]
    pub service_name: String,

    /// Service version.
    #[serde(default)]
    pub service_version: Option<String>,

    /// Additional resource attributes.
    #[serde(default)]
    pub resource_attributes: std::collections::HashMap<String, String>,

    /// Whether to enable trace context propagation.
    #[serde(default = "default_true")]
    pub propagate_context: bool,

    /// Sampling ratio (0.0 to 1.0).
    #[serde(default = "default_sampling_ratio")]
    pub sampling_ratio: f64,

    /// Export timeout in seconds.
    #[serde(default = "default_export_timeout")]
    pub export_timeout_secs: u64,
}

impl Default for OtelSettings {
    fn default() -> Self {
        OtelSettings {
            enabled: false,
            endpoint: None,
            service_name: default_service_name(),
            service_version: None,
            resource_attributes: std::collections::HashMap::new(),
            propagate_context: default_true(),
            sampling_ratio: default_sampling_ratio(),
            export_timeout_secs: default_export_timeout(),
        }
    }
}

fn default_service_name() -> String {
    "cortex-cli".to_string()
}

fn default_true() -> bool {
    true
}

fn default_sampling_ratio() -> f64 {
    1.0
}

fn default_export_timeout() -> u64 {
    30
}

impl OtelSettings {
    /// Create settings from environment variables.
    pub fn from_env() -> Self {
        let mut settings = Self::default();

        if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
            settings.endpoint = Some(endpoint);
            settings.enabled = true;
        }

        if let Ok(service_name) = std::env::var("OTEL_SERVICE_NAME") {
            settings.service_name = service_name;
        }

        if let Ok(ratio) = std::env::var("OTEL_TRACES_SAMPLER_ARG")
            && let Ok(ratio) = ratio.parse()
        {
            settings.sampling_ratio = ratio;
        }

        settings
    }

    /// Check if OpenTelemetry should be initialized.
    pub fn should_initialize(&self) -> bool {
        self.enabled && self.endpoint.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = OtelSettings::default();
        assert!(!settings.enabled);
        assert!(settings.endpoint.is_none());
        assert_eq!(settings.service_name, "cortex-cli");
        assert!(settings.propagate_context);
        assert_eq!(settings.sampling_ratio, 1.0);
    }
}
