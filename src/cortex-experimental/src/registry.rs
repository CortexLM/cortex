//! Feature registry.

use crate::{Feature, FeatureInfo, FeatureStage, FeaturesConfig};
use std::collections::HashMap;

/// Registry of available features.
pub struct FeatureRegistry {
    features: HashMap<String, Feature>,
    config: FeaturesConfig,
}

impl FeatureRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            features: HashMap::new(),
            config: FeaturesConfig::default(),
        };

        // Register builtin features
        for feature in BUILTIN_FEATURES.iter() {
            registry.register(feature.clone());
        }

        registry
    }

    pub fn with_config(mut self, config: FeaturesConfig) -> Self {
        self.config = config;
        self
    }

    /// Register a feature.
    pub fn register(&mut self, feature: Feature) {
        self.features.insert(feature.id.clone(), feature);
    }

    /// Check if a feature is enabled.
    pub fn is_enabled(&self, feature_id: &str) -> bool {
        // Check config override first
        if let Some(&enabled) = self.config.overrides.get(feature_id) {
            return enabled;
        }

        // Check default
        self.features
            .get(feature_id)
            .map(|f| f.default_enabled)
            .unwrap_or(false)
    }

    /// Get feature info.
    pub fn get_info(&self, feature_id: &str) -> Option<FeatureInfo> {
        let feature = self.features.get(feature_id)?.clone();
        let enabled = self.is_enabled(feature_id);
        let mut info = FeatureInfo::new(feature, enabled);

        // Check dependencies
        let deps = info.feature.dependencies.clone();
        for dep in &deps {
            if !self.is_enabled(dep) {
                info = info.blocked(format!("Requires '{}' to be enabled", dep));
                break;
            }
        }

        // Check conflicts
        let conflicts = info.feature.conflicts.clone();
        for conflict in &conflicts {
            if self.is_enabled(conflict) {
                info = info.blocked(format!("Conflicts with '{}' which is enabled", conflict));
                break;
            }
        }

        Some(info)
    }

    /// List all features.
    pub fn list_all(&self) -> Vec<FeatureInfo> {
        self.features
            .keys()
            .filter_map(|id| self.get_info(id))
            .collect()
    }

    /// List features by stage.
    pub fn list_by_stage(&self, stage: FeatureStage) -> Vec<FeatureInfo> {
        self.list_all()
            .into_iter()
            .filter(|info| info.feature.stage == stage)
            .collect()
    }

    /// Enable a feature.
    pub fn enable(&mut self, feature_id: &str) -> Result<(), String> {
        if let Some(info) = self.get_info(feature_id) {
            if !info.can_toggle {
                return Err(info
                    .toggle_blocked_reason
                    .unwrap_or_else(|| "Cannot toggle".to_string()));
            }
        } else {
            return Err(format!("Feature '{}' not found", feature_id));
        }

        self.config.overrides.insert(feature_id.to_string(), true);
        Ok(())
    }

    /// Disable a feature.
    pub fn disable(&mut self, feature_id: &str) -> Result<(), String> {
        // Check if any enabled feature depends on this
        for id in self.features.keys() {
            if let Some(info) = self.get_info(id) {
                if info.enabled && info.feature.dependencies.contains(&feature_id.to_string()) {
                    return Err(format!("Cannot disable: '{}' depends on this feature", id));
                }
            }
        }

        self.config.overrides.insert(feature_id.to_string(), false);
        Ok(())
    }

    /// Toggle a feature.
    pub fn toggle(&mut self, feature_id: &str) -> Result<bool, String> {
        let currently_enabled = self.is_enabled(feature_id);
        if currently_enabled {
            self.disable(feature_id)?;
        } else {
            self.enable(feature_id)?;
        }
        Ok(!currently_enabled)
    }

    /// Get the current config.
    pub fn config(&self) -> &FeaturesConfig {
        &self.config
    }
}

impl Default for FeatureRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Builtin features.
pub static BUILTIN_FEATURES: &[Feature] = &[Feature {
    id: String::new(), // Will be set properly below
    name: String::new(),
    description: String::new(),
    stage: FeatureStage::Experimental,
    default_enabled: false,
    requires_restart: false,
    dependencies: Vec::new(),
    conflicts: Vec::new(),
}];

// Actual builtin features function
pub fn get_builtin_features() -> Vec<Feature> {
    vec![
        // ============================================================
        // Stable features
        // ============================================================
        Feature::new("web_search", "Web Search", "Allow agent to search the web")
            .stage(FeatureStage::Stable)
            .default_enabled(false),
        Feature::new("view_image", "View Image", "Enable image viewing tool")
            .stage(FeatureStage::Stable)
            .default_enabled(true),
        Feature::new(
            "streaming_markdown",
            "Streaming Markdown",
            "Render markdown while streaming",
        )
        .stage(FeatureStage::Stable)
        .default_enabled(true),
        Feature::new("shell_tool", "Shell Tool", "Enable default shell tool")
            .stage(FeatureStage::Stable)
            .default_enabled(true),
        // ============================================================
        // Beta features
        // ============================================================
        Feature::new(
            "auto_compact",
            "Auto Compaction",
            "Automatically compact context when full",
        )
        .stage(FeatureStage::Beta)
        .default_enabled(true),
        Feature::new("multi_agent", "Multi-Agent", "Enable sub-agent delegation")
            .stage(FeatureStage::Beta),
        // ============================================================
        // Experimental features (visible in /experimental menu)
        // ============================================================
        Feature::new(
            "unified_exec",
            "Background Terminal",
            "Run long-running terminal commands in background (PTY-backed)",
        )
        .stage(FeatureStage::Experimental),
        Feature::new(
            "ghost_commits",
            "Ghost Commits",
            "Auto-create git snapshots for undo",
        )
        .stage(FeatureStage::Experimental),
        Feature::new(
            "shell_snapshot",
            "Shell Snapshot",
            "Snapshot shell to avoid re-running login scripts",
        )
        .stage(FeatureStage::Experimental),
        Feature::new(
            "skills",
            "Skills System",
            "Enable skills discovery and injection",
        )
        .stage(FeatureStage::Experimental),
        Feature::new(
            "lsp_diagnostics",
            "LSP Diagnostics",
            "Show LSP diagnostics in tool results",
        )
        .stage(FeatureStage::Experimental),
        Feature::new(
            "session_share",
            "Session Sharing",
            "Enable sharing sessions via URL",
        )
        .stage(FeatureStage::Experimental),
        Feature::new(
            "steer",
            "Steer Mode",
            "Enter key submits immediately (no newline)",
        )
        .stage(FeatureStage::Experimental),
        Feature::new(
            "backtracking",
            "Backtracking",
            "Navigate and rollback to previous conversation states",
        )
        .stage(FeatureStage::Experimental),
        Feature::new(
            "collaboration_modes",
            "Collaboration Modes",
            "Switch between Plan/Code/Pair/Execute modes",
        )
        .stage(FeatureStage::Experimental),
        Feature::new(
            "network_proxy",
            "Network Proxy",
            "Control network access with domain filtering",
        )
        .stage(FeatureStage::Experimental),
        // ============================================================
        // Under Development features (not visible in menu)
        // ============================================================
        Feature::new(
            "exec_policy",
            "Execution Policy",
            "Shell/unified exec policy enforcement (Allow/Prompt/Forbidden)",
        )
        .stage(FeatureStage::UnderDevelopment),
        Feature::new(
            "collab",
            "Multi-Agent Collaboration",
            "Full multi-agent collaboration with spawn/wait/close",
        )
        .stage(FeatureStage::UnderDevelopment),
        Feature::new(
            "connectors",
            "Connectors",
            "Apps/connectors integration for external services",
        )
        .stage(FeatureStage::UnderDevelopment),
        Feature::new(
            "responses_websockets",
            "WebSocket Transport",
            "WebSocket transport for OpenAI Responses API",
        )
        .stage(FeatureStage::UnderDevelopment),
        Feature::new(
            "remote_compaction",
            "Remote Compaction",
            "Remote context compaction (ChatGPT auth only)",
        )
        .stage(FeatureStage::UnderDevelopment),
        Feature::new(
            "child_agents_md",
            "AGENTS.md Support",
            "Append AGENTS.md to instructions for sub-agents",
        )
        .stage(FeatureStage::UnderDevelopment),
        Feature::new(
            "request_compression",
            "Request Compression",
            "Zstd compression for API requests",
        )
        .stage(FeatureStage::UnderDevelopment),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry() {
        let mut registry = FeatureRegistry::new();

        // Register a test feature
        registry.register(
            Feature::new("test", "Test Feature", "A test feature")
                .stage(FeatureStage::Experimental),
        );

        assert!(!registry.is_enabled("test"));

        registry.enable("test").unwrap();
        assert!(registry.is_enabled("test"));
    }
}
