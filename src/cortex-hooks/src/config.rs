//! Hook configuration loading and management.

use crate::{Hook, HookType, BUILTIN_FORMATTERS};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Hook configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookConfig {
    /// File edited hooks (pattern -> hook definitions).
    #[serde(default)]
    pub file_edited: HashMap<String, Vec<HookDefinition>>,
    /// Session completed hooks.
    #[serde(default)]
    pub session_completed: Vec<HookDefinition>,
    /// Formatter configuration.
    #[serde(default)]
    pub formatter: FormatterSettings,
}

/// A hook definition from config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    /// Command to run.
    pub command: Vec<String>,
    /// Environment variables.
    #[serde(default)]
    pub environment: HashMap<String, String>,
    /// Timeout in seconds.
    pub timeout: Option<u64>,
}

/// Formatter settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatterSettings {
    /// Whether formatting is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Formatter overrides by extension.
    #[serde(default)]
    pub overrides: HashMap<String, FormatterOverride>,
    /// Disabled formatters.
    #[serde(default)]
    pub disabled: Vec<String>,
}

impl Default for FormatterSettings {
    fn default() -> Self {
        FormatterSettings {
            enabled: true,
            overrides: HashMap::new(),
            disabled: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// Formatter override configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatterOverride {
    /// Command override.
    pub command: Option<Vec<String>>,
    /// Whether disabled.
    #[serde(default)]
    pub disabled: bool,
    /// Custom extensions.
    pub extensions: Option<Vec<String>>,
}

impl HookConfig {
    /// Load from a JSON file.
    pub async fn load_from_file(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = tokio::fs::read_to_string(path).await?;
        let config: Self = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Convert to hooks.
    pub fn to_hooks(&self) -> Vec<Hook> {
        let mut hooks = Vec::new();

        // File edited hooks
        for (pattern, definitions) in &self.file_edited {
            for (i, def) in definitions.iter().enumerate() {
                let hook = Hook::new(
                    format!("file_edited_{}_{}", pattern, i),
                    HookType::FileEdited,
                    def.command.clone(),
                )
                .with_pattern(pattern.clone());

                let mut hook = hook;
                for (k, v) in &def.environment {
                    hook = hook.with_env(k.clone(), v.clone());
                }
                if let Some(timeout) = def.timeout {
                    hook = hook.with_timeout(timeout);
                }

                hooks.push(hook);
            }
        }

        // Session completed hooks
        for (i, def) in self.session_completed.iter().enumerate() {
            let hook = Hook::new(
                format!("session_completed_{}", i),
                HookType::SessionCompleted,
                def.command.clone(),
            );

            let mut hook = hook;
            for (k, v) in &def.environment {
                hook = hook.with_env(k.clone(), v.clone());
            }
            if let Some(timeout) = def.timeout {
                hook = hook.with_timeout(timeout);
            }

            hooks.push(hook);
        }

        hooks
    }

    /// Get formatter hooks for file editing.
    pub fn get_formatter_hooks(&self) -> Vec<Hook> {
        if !self.formatter.enabled {
            return Vec::new();
        }

        let mut hooks = Vec::new();

        for formatter in BUILTIN_FORMATTERS.iter() {
            // Check if disabled
            if self.formatter.disabled.contains(&formatter.id) {
                continue;
            }

            // Check for override
            let extensions = formatter.extensions.clone();
            let command = formatter.command.clone();

            // Create pattern from extensions
            for ext in extensions {
                let pattern = format!("*.{}", ext);

                let hook = Hook::new(
                    format!("formatter_{}_{}", formatter.id, ext),
                    HookType::FileEdited,
                    command.clone(),
                )
                .with_pattern(pattern);

                hooks.push(hook);
            }
        }

        hooks
    }

    /// Merge with another config (other takes precedence).
    pub fn merge(&mut self, other: HookConfig) {
        // Merge file_edited
        for (pattern, hooks) in other.file_edited {
            self.file_edited.entry(pattern).or_default().extend(hooks);
        }

        // Merge session_completed
        self.session_completed.extend(other.session_completed);

        // Merge formatter settings
        if !other.formatter.enabled {
            self.formatter.enabled = false;
        }
        self.formatter.disabled.extend(other.formatter.disabled);
        self.formatter.overrides.extend(other.formatter.overrides);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_config_to_hooks() {
        let mut config = HookConfig::default();
        config.file_edited.insert(
            "*.rs".to_string(),
            vec![HookDefinition {
                command: vec!["rustfmt".to_string(), "{file}".to_string()],
                environment: HashMap::new(),
                timeout: Some(30),
            }],
        );

        let hooks = config.to_hooks();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].hook_type, HookType::FileEdited);
    }

    #[test]
    fn test_formatter_hooks() {
        let config = HookConfig::default();
        let hooks = config.get_formatter_hooks();

        // Should have hooks for all builtin formatters
        assert!(!hooks.is_empty());
    }
}
