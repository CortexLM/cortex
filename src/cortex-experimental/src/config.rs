//! Features configuration.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Configuration for experimental features.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeaturesConfig {
    /// Feature overrides (feature_id -> enabled).
    #[serde(default)]
    pub overrides: HashMap<String, bool>,
}

impl FeaturesConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load from a TOML file.
    pub async fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = tokio::fs::read_to_string(path).await?;

        // Parse as TOML table and extract features section
        let table: toml::Table = content.parse()?;

        if let Some(features) = table.get("features") {
            let config: FeaturesConfig = features.clone().try_into()?;
            return Ok(config);
        }

        Ok(Self::default())
    }

    /// Save to a TOML file.
    pub async fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let content = toml::to_string_pretty(self)?;
        tokio::fs::write(path, content).await?;
        Ok(())
    }

    /// Check if a feature is explicitly enabled.
    pub fn is_enabled(&self, feature_id: &str) -> Option<bool> {
        self.overrides.get(feature_id).copied()
    }

    /// Set feature enabled state.
    pub fn set_enabled(&mut self, feature_id: &str, enabled: bool) {
        self.overrides.insert(feature_id.to_string(), enabled);
    }

    /// Remove override (use default).
    pub fn reset(&mut self, feature_id: &str) {
        self.overrides.remove(feature_id);
    }

    /// Reset all overrides.
    pub fn reset_all(&mut self) {
        self.overrides.clear();
    }
}

// Implement TryFrom for toml::Value
impl TryFrom<toml::Value> for FeaturesConfig {
    type Error = toml::de::Error;

    fn try_from(value: toml::Value) -> Result<Self, Self::Error> {
        let mut config = FeaturesConfig::default();

        if let toml::Value::Table(table) = value {
            for (key, val) in table {
                if let toml::Value::Boolean(enabled) = val {
                    config.overrides.insert(key, enabled);
                }
            }
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_features_config() {
        let mut config = FeaturesConfig::new();

        config.set_enabled("test_feature", true);
        assert_eq!(config.is_enabled("test_feature"), Some(true));

        config.reset("test_feature");
        assert_eq!(config.is_enabled("test_feature"), None);
    }
}
