//! Feature flags and experiments.
//!
//! Provides a system for managing feature flags, A/B testing,
//! and gradual rollouts.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Feature flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feature {
    /// Feature name/ID.
    pub name: String,
    /// Description.
    pub description: String,
    /// Is enabled by default.
    pub default_enabled: bool,
    /// Current state.
    pub enabled: bool,
    /// Rollout percentage (0-100).
    pub rollout_percentage: u8,
    /// Required features.
    pub requires: Vec<String>,
    /// Conflicts with features.
    pub conflicts: Vec<String>,
    /// Tags for categorization.
    pub tags: Vec<String>,
    /// Metadata.
    pub metadata: HashMap<String, String>,
    /// Is experimental.
    pub experimental: bool,
    /// Is deprecated.
    pub deprecated: bool,
    /// Deprecation message.
    pub deprecation_message: Option<String>,
}

impl Feature {
    /// Create a new feature.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            default_enabled: false,
            enabled: false,
            rollout_percentage: 0,
            requires: Vec::new(),
            conflicts: Vec::new(),
            tags: Vec::new(),
            metadata: HashMap::new(),
            experimental: false,
            deprecated: false,
            deprecation_message: None,
        }
    }

    /// Set enabled by default.
    pub fn enabled_by_default(mut self) -> Self {
        self.default_enabled = true;
        self.enabled = true;
        self
    }

    /// Set rollout percentage.
    pub fn with_rollout(mut self, percentage: u8) -> Self {
        self.rollout_percentage = percentage.min(100);
        self
    }

    /// Add required feature.
    pub fn requires(mut self, feature: impl Into<String>) -> Self {
        self.requires.push(feature.into());
        self
    }

    /// Add conflicting feature.
    pub fn conflicts_with(mut self, feature: impl Into<String>) -> Self {
        self.conflicts.push(feature.into());
        self
    }

    /// Add tag.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Add metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Mark as experimental.
    pub fn experimental(mut self) -> Self {
        self.experimental = true;
        self
    }

    /// Mark as deprecated.
    pub fn deprecated(mut self, message: impl Into<String>) -> Self {
        self.deprecated = true;
        self.deprecation_message = Some(message.into());
        self
    }

    /// Enable the feature.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Disable the feature.
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Check if feature is available for a given user/context.
    pub fn is_available_for(&self, user_hash: u64) -> bool {
        if !self.enabled {
            return false;
        }

        if self.rollout_percentage >= 100 {
            return true;
        }

        if self.rollout_percentage == 0 {
            return false;
        }

        // Simple hash-based rollout
        let bucket = (user_hash % 100) as u8;
        bucket < self.rollout_percentage
    }
}

/// Feature flag manager.
pub struct FeatureManager {
    /// Features indexed by name.
    features: RwLock<HashMap<String, Feature>>,
    /// Overrides per user/context.
    overrides: RwLock<HashMap<String, HashMap<String, bool>>>,
    /// Default context ID.
    default_context: RwLock<Option<String>>,
}

impl FeatureManager {
    /// Create a new feature manager.
    pub fn new() -> Self {
        Self {
            features: RwLock::new(HashMap::new()),
            overrides: RwLock::new(HashMap::new()),
            default_context: RwLock::new(None),
        }
    }

    /// Create with standard features.
    pub fn with_standard_features() -> Self {
        // Register standard features synchronously isn't possible with async,
        // so we return the manager and let the caller register features
        Self::new()
    }

    /// Register a feature.
    pub async fn register(&self, feature: Feature) {
        self.features
            .write()
            .await
            .insert(feature.name.clone(), feature);
    }

    /// Get a feature.
    pub async fn get(&self, name: &str) -> Option<Feature> {
        self.features.read().await.get(name).cloned()
    }

    /// Check if a feature is enabled.
    pub async fn is_enabled(&self, name: &str) -> bool {
        self.is_enabled_for(name, None).await
    }

    /// Check if a feature is enabled for a context.
    pub async fn is_enabled_for(&self, name: &str, context: Option<&str>) -> bool {
        // Check overrides first
        if let Some(ctx) = context.or(self.default_context.read().await.as_deref()) {
            let overrides = self.overrides.read().await;
            if let Some(ctx_overrides) = overrides.get(ctx)
                && let Some(&enabled) = ctx_overrides.get(name)
            {
                return enabled;
            }
        }

        // Check feature state
        let features = self.features.read().await;
        if let Some(feature) = features.get(name) {
            if !feature.enabled {
                return false;
            }

            // Check dependencies (non-recursive check)
            for req in &feature.requires {
                if let Some(req_feature) = features.get(req) {
                    if !req_feature.enabled {
                        return false;
                    }
                } else {
                    return false;
                }
            }

            // Check conflicts (non-recursive check)
            for conflict in &feature.conflicts {
                if let Some(conf_feature) = features.get(conflict)
                    && conf_feature.enabled
                {
                    return false;
                }
            }

            return true;
        }

        false
    }

    /// Enable a feature.
    pub async fn enable(&self, name: &str) -> bool {
        if let Some(feature) = self.features.write().await.get_mut(name) {
            feature.enable();
            return true;
        }
        false
    }

    /// Disable a feature.
    pub async fn disable(&self, name: &str) -> bool {
        if let Some(feature) = self.features.write().await.get_mut(name) {
            feature.disable();
            return true;
        }
        false
    }

    /// Set override for a context.
    pub async fn set_override(&self, context: &str, feature: &str, enabled: bool) {
        let mut overrides = self.overrides.write().await;
        overrides
            .entry(context.to_string())
            .or_insert_with(HashMap::new)
            .insert(feature.to_string(), enabled);
    }

    /// Clear override for a context.
    pub async fn clear_override(&self, context: &str, feature: &str) {
        let mut overrides = self.overrides.write().await;
        if let Some(ctx_overrides) = overrides.get_mut(context) {
            ctx_overrides.remove(feature);
        }
    }

    /// Clear all overrides for a context.
    pub async fn clear_context_overrides(&self, context: &str) {
        self.overrides.write().await.remove(context);
    }

    /// Set default context.
    pub async fn set_default_context(&self, context: impl Into<String>) {
        *self.default_context.write().await = Some(context.into());
    }

    /// List all features.
    pub async fn list(&self) -> Vec<Feature> {
        self.features.read().await.values().cloned().collect()
    }

    /// List enabled features.
    pub async fn enabled(&self) -> Vec<String> {
        self.features
            .read()
            .await
            .iter()
            .filter(|(_, f)| f.enabled)
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// List disabled features.
    pub async fn disabled(&self) -> Vec<String> {
        self.features
            .read()
            .await
            .iter()
            .filter(|(_, f)| !f.enabled)
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// List experimental features.
    pub async fn experimental(&self) -> Vec<String> {
        self.features
            .read()
            .await
            .iter()
            .filter(|(_, f)| f.experimental)
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// List deprecated features.
    pub async fn deprecated(&self) -> Vec<String> {
        self.features
            .read()
            .await
            .iter()
            .filter(|(_, f)| f.deprecated)
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// Get features by tag.
    pub async fn by_tag(&self, tag: &str) -> Vec<Feature> {
        self.features
            .read()
            .await
            .values()
            .filter(|f| f.tags.contains(&tag.to_string()))
            .cloned()
            .collect()
    }

    /// Export configuration.
    pub async fn export(&self) -> FeatureConfig {
        let features = self.features.read().await;
        let overrides = self.overrides.read().await;

        FeatureConfig {
            features: features.values().cloned().collect(),
            overrides: overrides.clone(),
        }
    }

    /// Import configuration.
    pub async fn import(&self, config: FeatureConfig) {
        let mut features = self.features.write().await;
        let mut overrides = self.overrides.write().await;

        for feature in config.features {
            features.insert(feature.name.clone(), feature);
        }

        for (ctx, ctx_overrides) in config.overrides {
            overrides.insert(ctx, ctx_overrides);
        }
    }
}

impl Default for FeatureManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Feature configuration for export/import.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureConfig {
    /// Features.
    pub features: Vec<Feature>,
    /// Overrides.
    pub overrides: HashMap<String, HashMap<String, bool>>,
}

/// Standard feature flags.
pub mod standard {
    use super::Feature;

    /// Extended thinking/reasoning.
    pub fn extended_thinking() -> Feature {
        Feature::new(
            "extended_thinking",
            "Enable extended thinking/reasoning for complex tasks",
        )
        .enabled_by_default()
        .with_tag("ai")
    }

    /// Streaming responses.
    pub fn streaming() -> Feature {
        Feature::new("streaming", "Enable streaming responses from models")
            .enabled_by_default()
            .with_tag("performance")
    }

    /// Tool use.
    pub fn tool_use() -> Feature {
        Feature::new("tool_use", "Enable tool/function calling")
            .enabled_by_default()
            .with_tag("ai")
    }

    /// File watching.
    pub fn file_watching() -> Feature {
        Feature::new("file_watching", "Watch files for changes").with_tag("files")
    }

    /// Sandbox execution.
    pub fn sandbox() -> Feature {
        Feature::new("sandbox", "Execute commands in sandbox environment").with_tag("security")
    }

    /// Auto-approval of safe commands.
    pub fn auto_approve() -> Feature {
        Feature::new("auto_approve", "Automatically approve safe commands").with_tag("automation")
    }

    /// Metrics collection.
    pub fn metrics() -> Feature {
        Feature::new("metrics", "Collect usage metrics").with_tag("analytics")
    }

    /// Context compaction.
    pub fn context_compaction() -> Feature {
        Feature::new(
            "context_compaction",
            "Automatically compact context when limit approached",
        )
        .enabled_by_default()
        .with_tag("ai")
    }

    /// MCP support.
    pub fn mcp() -> Feature {
        Feature::new("mcp", "Enable Model Context Protocol support").with_tag("integration")
    }

    /// Undo support.
    pub fn undo() -> Feature {
        Feature::new("undo", "Enable undo functionality for file changes")
            .enabled_by_default()
            .with_tag("files")
    }

    /// Dark mode (for TUI).
    pub fn dark_mode() -> Feature {
        Feature::new("dark_mode", "Use dark color scheme")
            .enabled_by_default()
            .with_tag("ui")
    }

    /// Vim keybindings.
    pub fn vim_mode() -> Feature {
        Feature::new("vim_mode", "Enable vim-style keybindings")
            .with_tag("ui")
            .experimental()
    }

    /// Get all standard features.
    pub fn all() -> Vec<Feature> {
        vec![
            extended_thinking(),
            streaming(),
            tool_use(),
            file_watching(),
            sandbox(),
            auto_approve(),
            metrics(),
            context_compaction(),
            mcp(),
            undo(),
            dark_mode(),
            vim_mode(),
        ]
    }
}

/// Feature guard for conditional execution.
pub struct FeatureGuard {
    manager: Arc<FeatureManager>,
    feature: String,
}

impl FeatureGuard {
    /// Create a new feature guard.
    pub fn new(manager: Arc<FeatureManager>, feature: impl Into<String>) -> Self {
        Self {
            manager,
            feature: feature.into(),
        }
    }

    /// Check if feature is enabled.
    pub async fn is_enabled(&self) -> bool {
        self.manager.is_enabled(&self.feature).await
    }

    /// Execute if feature is enabled.
    pub async fn if_enabled<F, Fut, T>(&self, f: F) -> Option<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        if self.is_enabled().await {
            Some(f().await)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feature_creation() {
        let feature = Feature::new("test", "Test feature")
            .enabled_by_default()
            .experimental()
            .with_tag("testing");

        assert!(feature.enabled);
        assert!(feature.experimental);
        assert!(feature.tags.contains(&"testing".to_string()));
    }

    #[test]
    fn test_rollout() {
        let mut feature = Feature::new("test", "Test").with_rollout(50);

        feature.enable();

        // Test multiple hashes
        let enabled_count = (0..1000)
            .map(|i| feature.is_available_for(i))
            .filter(|&b| b)
            .count();

        // Should be approximately 50% (allow some variance)
        assert!(enabled_count > 400 && enabled_count < 600);
    }

    #[tokio::test]
    async fn test_feature_manager() {
        let manager = FeatureManager::new();

        let feature = Feature::new("test", "Test").enabled_by_default();
        manager.register(feature).await;

        assert!(manager.is_enabled("test").await);

        manager.disable("test").await;
        assert!(!manager.is_enabled("test").await);
    }

    #[tokio::test]
    async fn test_overrides() {
        let manager = FeatureManager::new();

        let feature = Feature::new("test", "Test").enabled_by_default();
        manager.register(feature).await;

        manager.set_override("user1", "test", false).await;

        assert!(manager.is_enabled("test").await);
        assert!(!manager.is_enabled_for("test", Some("user1")).await);
    }

    #[test]
    fn test_standard_features() {
        let features = standard::all();
        assert!(!features.is_empty());

        let streaming = standard::streaming();
        assert!(streaming.default_enabled);
    }
}
