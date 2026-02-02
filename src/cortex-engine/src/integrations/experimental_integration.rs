//! Experimental features integration for cortex-core.
//!
//! Connects cortex-experimental to manage feature flags.

use cortex_experimental::registry::get_builtin_features;
use cortex_experimental::{Feature, FeatureInfo, FeatureRegistry, FeatureStage, FeaturesConfig};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Experimental features integration.
pub struct ExperimentalIntegration {
    registry: Arc<RwLock<FeatureRegistry>>,
}

impl ExperimentalIntegration {
    /// Create a new experimental integration.
    pub fn new() -> Self {
        let mut registry = FeatureRegistry::new();

        // Register builtin features
        for feature in get_builtin_features() {
            registry.register(feature);
        }

        Self {
            registry: Arc::new(RwLock::new(registry)),
        }
    }

    /// Load config from file.
    pub async fn load_config(&self, config_path: &Path) -> anyhow::Result<()> {
        if config_path.exists() {
            let config = FeaturesConfig::load(config_path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to load features config: {e}"))?;

            let mut registry = self.registry.write().await;
            *registry = FeatureRegistry::new().with_config(config);

            // Re-register builtin features
            for feature in get_builtin_features() {
                registry.register(feature);
            }

            debug!("Loaded experimental features config from {:?}", config_path);
        }
        Ok(())
    }

    /// Save config to file.
    pub async fn save_config(&self, config_path: &Path) -> anyhow::Result<()> {
        let registry = self.registry.read().await;
        registry
            .config()
            .save(config_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to save features config: {e}"))?;
        Ok(())
    }

    /// Check if a feature is enabled.
    pub async fn is_enabled(&self, feature_id: &str) -> bool {
        self.registry.read().await.is_enabled(feature_id)
    }

    /// Enable a feature.
    pub async fn enable(&self, feature_id: &str) -> Result<(), String> {
        self.registry.write().await.enable(feature_id)?;
        info!("Enabled feature: {}", feature_id);
        Ok(())
    }

    /// Disable a feature.
    pub async fn disable(&self, feature_id: &str) -> Result<(), String> {
        self.registry.write().await.disable(feature_id)?;
        info!("Disabled feature: {}", feature_id);
        Ok(())
    }

    /// Toggle a feature.
    pub async fn toggle(&self, feature_id: &str) -> Result<bool, String> {
        let result = self.registry.write().await.toggle(feature_id)?;
        info!(
            "Toggled feature {}: now {}",
            feature_id,
            if result { "enabled" } else { "disabled" }
        );
        Ok(result)
    }

    /// Get info about a feature.
    pub async fn get_info(&self, feature_id: &str) -> Option<FeatureInfo> {
        self.registry.read().await.get_info(feature_id)
    }

    /// List all features.
    pub async fn list_all(&self) -> Vec<FeatureInfo> {
        self.registry.read().await.list_all()
    }

    /// List features by stage.
    pub async fn list_by_stage(&self, stage: FeatureStage) -> Vec<FeatureInfo> {
        self.registry.read().await.list_by_stage(stage)
    }

    /// Register a custom feature.
    pub async fn register(&self, feature: Feature) {
        self.registry.write().await.register(feature);
    }

    /// Get features for display in menu.
    pub async fn get_menu_items(&self) -> Vec<FeatureMenuItem> {
        let features = self.list_all().await;

        features
            .into_iter()
            .map(|info| FeatureMenuItem {
                id: info.feature.id.clone(),
                name: info.feature.name.clone(),
                description: info.feature.description.clone(),
                stage: info.feature.stage,
                enabled: info.enabled,
                can_toggle: info.can_toggle,
                blocked_reason: info.toggle_blocked_reason,
            })
            .collect()
    }
}

impl Default for ExperimentalIntegration {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ExperimentalIntegration {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
        }
    }
}

/// Feature menu item for UI display.
#[derive(Debug, Clone)]
pub struct FeatureMenuItem {
    pub id: String,
    pub name: String,
    pub description: String,
    pub stage: FeatureStage,
    pub enabled: bool,
    pub can_toggle: bool,
    pub blocked_reason: Option<String>,
}

impl FeatureMenuItem {
    /// Format for display.
    pub fn format(&self) -> String {
        let status = if self.enabled { "[x]" } else { "[ ]" };
        let stage = match self.stage {
            FeatureStage::UnderDevelopment => " (dev)",
            FeatureStage::Experimental => " (experimental)",
            FeatureStage::Beta => " (beta)",
            FeatureStage::Stable => "",
            FeatureStage::Deprecated => " (deprecated)",
            FeatureStage::Removed => " (removed)",
        };

        format!("{} {}{} - {}", status, self.name, stage, self.description)
    }
}
