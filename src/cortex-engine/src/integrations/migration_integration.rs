//! Model migration integration for cortex-core.
//!
//! Connects cortex-migrations to check for deprecated models.

use cortex_migrations::warnings::can_use_model;
use cortex_migrations::{
    MigrationPath, ModelWarning, WarningLevel, check_model_warnings, get_migration_path,
};
use tracing::warn;

/// Migration integration for model warnings.
pub struct MigrationIntegration {
    /// Whether to block deprecated models.
    block_deprecated: bool,
    /// Whether to show warnings.
    show_warnings: bool,
}

impl MigrationIntegration {
    /// Create a new migration integration.
    pub fn new() -> Self {
        Self {
            block_deprecated: false,
            show_warnings: true,
        }
    }

    /// Configure blocking of deprecated models.
    pub fn with_blocking(mut self, block: bool) -> Self {
        self.block_deprecated = block;
        self
    }

    /// Configure warning display.
    pub fn with_warnings(mut self, show: bool) -> Self {
        self.show_warnings = show;
        self
    }

    /// Check if a model can be used.
    pub fn check_model(&self, model_id: &str) -> ModelCheckResult {
        let (can_use, reason) = can_use_model(model_id);

        if !can_use && self.block_deprecated {
            return ModelCheckResult::Blocked {
                reason: reason.unwrap_or_else(|| "Model not available".to_string()),
            };
        }

        let warnings = check_model_warnings(model_id);

        if self.show_warnings {
            for warning in &warnings {
                match warning.level {
                    WarningLevel::Error => warn!("{}", warning.format()),
                    WarningLevel::Warning => warn!("{}", warning.format()),
                    WarningLevel::Info => {} // Skip info in logs
                }
            }
        }

        let migration = get_migration_path(model_id);

        if warnings.iter().any(|w| w.level == WarningLevel::Error) && self.block_deprecated {
            ModelCheckResult::Blocked {
                reason: warnings
                    .iter()
                    .find(|w| w.level == WarningLevel::Error)
                    .map(|w| w.message.clone())
                    .unwrap_or_default(),
            }
        } else if !warnings.is_empty() {
            ModelCheckResult::Deprecated {
                warnings,
                migration,
            }
        } else {
            ModelCheckResult::Ok
        }
    }

    /// Get warnings for display.
    pub fn get_warnings(&self, model_id: &str) -> Vec<ModelWarning> {
        check_model_warnings(model_id)
    }

    /// Get migration path if available.
    pub fn get_migration(&self, model_id: &str) -> Option<MigrationPath> {
        get_migration_path(model_id)
    }

    /// Format warnings for display.
    pub fn format_warnings(&self, model_id: &str) -> Option<String> {
        let warnings = self.get_warnings(model_id);

        if warnings.is_empty() {
            return None;
        }

        let mut output = String::new();

        for warning in &warnings {
            output.push_str(&warning.format());
            output.push('\n');
        }

        Some(output)
    }
}

impl Default for MigrationIntegration {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of checking a model.
#[derive(Debug)]
pub enum ModelCheckResult {
    /// Model is OK to use.
    Ok,
    /// Model is deprecated but can still be used.
    Deprecated {
        warnings: Vec<ModelWarning>,
        migration: Option<MigrationPath>,
    },
    /// Model is blocked and cannot be used.
    Blocked { reason: String },
}

impl ModelCheckResult {
    /// Check if the model can be used.
    pub fn can_use(&self) -> bool {
        !matches!(self, Self::Blocked { .. })
    }

    /// Check if there are warnings.
    pub fn has_warnings(&self) -> bool {
        matches!(self, Self::Deprecated { .. })
    }
}
