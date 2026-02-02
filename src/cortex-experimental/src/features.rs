//! Feature definitions.

use serde::{Deserialize, Serialize};

/// Stage of a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeatureStage {
    /// Still under development, not for external use.
    UnderDevelopment,
    /// Experimental - may change or be removed.
    Experimental,
    /// Beta - mostly stable but not guaranteed.
    Beta,
    /// Stable - ready for production use.
    Stable,
    /// Deprecated - will be removed.
    Deprecated,
    /// Removed but kept for compatibility.
    Removed,
}

impl std::fmt::Display for FeatureStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnderDevelopment => write!(f, "Under Development"),
            Self::Experimental => write!(f, "Experimental"),
            Self::Beta => write!(f, "Beta"),
            Self::Stable => write!(f, "Stable"),
            Self::Deprecated => write!(f, "Deprecated"),
            Self::Removed => write!(f, "Removed"),
        }
    }
}

impl FeatureStage {
    /// Check if the feature is available for user access.
    pub fn is_available(&self) -> bool {
        !matches!(self, Self::UnderDevelopment | Self::Removed)
    }

    /// Check if the feature should show in the experimental menu.
    pub fn show_in_menu(&self) -> bool {
        matches!(self, Self::Experimental | Self::Beta)
    }

    /// Get a short description.
    pub fn description(&self) -> &str {
        match self {
            Self::UnderDevelopment => "Not ready for use",
            Self::Experimental => "May change or be removed",
            Self::Beta => "Mostly stable, not guaranteed",
            Self::Stable => "Ready for production use",
            Self::Deprecated => "Will be removed in future",
            Self::Removed => "No longer available",
        }
    }
}

/// A feature that can be enabled/disabled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feature {
    /// Feature ID.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Stage.
    pub stage: FeatureStage,
    /// Default enabled state.
    pub default_enabled: bool,
    /// Whether feature requires restart.
    pub requires_restart: bool,
    /// Dependencies (other features that must be enabled).
    pub dependencies: Vec<String>,
    /// Conflicts (other features that must be disabled).
    pub conflicts: Vec<String>,
}

impl Feature {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            stage: FeatureStage::Experimental,
            default_enabled: false,
            requires_restart: false,
            dependencies: Vec::new(),
            conflicts: Vec::new(),
        }
    }

    pub fn stage(mut self, stage: FeatureStage) -> Self {
        self.stage = stage;
        self
    }

    pub fn default_enabled(mut self, enabled: bool) -> Self {
        self.default_enabled = enabled;
        self
    }

    pub fn requires_restart(mut self) -> Self {
        self.requires_restart = true;
        self
    }

    pub fn depends_on(mut self, feature_id: impl Into<String>) -> Self {
        self.dependencies.push(feature_id.into());
        self
    }

    pub fn conflicts_with(mut self, feature_id: impl Into<String>) -> Self {
        self.conflicts.push(feature_id.into());
        self
    }
}

/// Information about a feature's current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureInfo {
    /// The feature.
    pub feature: Feature,
    /// Current enabled state.
    pub enabled: bool,
    /// Whether it can be toggled (dependencies met).
    pub can_toggle: bool,
    /// Reason if can't toggle.
    pub toggle_blocked_reason: Option<String>,
}

impl FeatureInfo {
    pub fn new(feature: Feature, enabled: bool) -> Self {
        Self {
            feature,
            enabled,
            can_toggle: true,
            toggle_blocked_reason: None,
        }
    }

    pub fn blocked(mut self, reason: impl Into<String>) -> Self {
        self.can_toggle = false;
        self.toggle_blocked_reason = Some(reason.into());
        self
    }
}
