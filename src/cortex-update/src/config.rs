//! Update configuration types.

use serde::{Deserialize, Serialize};

/// Release channel for updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseChannel {
    /// Stable releases (default)
    #[default]
    Stable,
    /// Beta releases
    Beta,
    /// Nightly builds
    Nightly,
}

impl ReleaseChannel {
    /// Get the channel as a string for API queries.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Nightly => "nightly",
        }
    }
}

impl std::fmt::Display for ReleaseChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Update behavior mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    /// Automatically check and prompt to install
    #[default]
    Auto,
    /// Only notify about updates, don't download
    Notify,
    /// Completely disabled
    Disabled,
}

/// User configuration for updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfig {
    /// Check for updates on startup
    #[serde(default = "default_true")]
    pub check_on_startup: bool,

    /// Update behavior mode
    #[serde(default)]
    pub mode: UpdateMode,

    /// Minutes between automatic checks (default: 5)
    #[serde(default = "default_5")]
    pub check_interval_minutes: u32,

    /// Release channel to follow
    #[serde(default)]
    pub channel: ReleaseChannel,

    /// Version to skip (don't prompt for this version)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_version: Option<String>,

    /// Last version that was notified to user (to avoid repeated notifications)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_notified_version: Option<String>,

    /// Custom software distribution URL (for testing/enterprise)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_url: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_5() -> u32 {
    5
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            check_on_startup: true,
            mode: UpdateMode::Auto,
            check_interval_minutes: 5,
            channel: ReleaseChannel::Stable,
            skip_version: None,
            last_notified_version: None,
            custom_url: None,
        }
    }
}

impl UpdateConfig {
    /// Load config from the standard location (~/.cortex/update.json).
    pub fn load() -> Self {
        let config_path = dirs::home_dir()
            .map(|h| h.join(".cortex").join("update.json"))
            .filter(|p| p.exists());

        if let Some(path) = config_path {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str(&content) {
                    return config;
                }
            }
        }

        Self::default()
    }

    /// Save config to the standard location.
    pub fn save(&self) -> Result<(), std::io::Error> {
        let config_dir = dirs::home_dir().map(|h| h.join(".cortex")).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "No home directory")
        })?;

        std::fs::create_dir_all(&config_dir)?;

        let config_path = config_dir.join("update.json");
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(config_path, content)?;

        Ok(())
    }

    /// Check if updates should be checked on startup.
    pub fn should_check_on_startup(&self) -> bool {
        self.check_on_startup && self.mode != UpdateMode::Disabled
    }

    /// Check if a version should be skipped.
    pub fn is_version_skipped(&self, version: &str) -> bool {
        self.skip_version.as_deref() == Some(version)
    }
}
