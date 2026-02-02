//! Model warnings and checks.

use crate::deprecations::is_deprecated;
use crate::migrations::get_migration_path;
use serde::{Deserialize, Serialize};

/// Warning level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WarningLevel {
    Info,
    Warning,
    Error,
}

impl std::fmt::Display for WarningLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "INFO"),
            Self::Warning => write!(f, "WARNING"),
            Self::Error => write!(f, "ERROR"),
        }
    }
}

/// A warning about a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelWarning {
    /// Warning level.
    pub level: WarningLevel,
    /// Warning message.
    pub message: String,
    /// Suggested action.
    pub action: Option<String>,
    /// Model this warning is for.
    pub model: String,
}

impl ModelWarning {
    pub fn info(model: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level: WarningLevel::Info,
            message: message.into(),
            action: None,
            model: model.into(),
        }
    }

    pub fn warning(model: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level: WarningLevel::Warning,
            message: message.into(),
            action: None,
            model: model.into(),
        }
    }

    pub fn error(model: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level: WarningLevel::Error,
            message: message.into(),
            action: None,
            model: model.into(),
        }
    }

    pub fn with_action(mut self, action: impl Into<String>) -> Self {
        self.action = Some(action.into());
        self
    }

    /// Format for display.
    pub fn format(&self) -> String {
        let mut msg = format!("[{}] {}: {}", self.level, self.model, self.message);
        if let Some(ref action) = self.action {
            msg.push_str(&format!("\n  Action: {}", action));
        }
        msg
    }
}

/// Check a model for warnings.
pub fn check_model_warnings(model_id: &str) -> Vec<ModelWarning> {
    let mut warnings = Vec::new();

    // Check deprecation
    let deprecation = is_deprecated(model_id);
    if deprecation.is_deprecated {
        if deprecation.is_removed {
            warnings.push(
                ModelWarning::error(
                    model_id,
                    "This model has been removed and is no longer available",
                )
                .with_action("Please switch to a supported model"),
            );
        } else if let Some(ref details) = deprecation.details {
            let mut warning = ModelWarning::warning(
                model_id,
                deprecation
                    .warning_message
                    .unwrap_or_else(|| "Model is deprecated".to_string()),
            );

            if let Some(ref replacement) = details.replacement {
                warning = warning.with_action(format!("Migrate to '{}'", replacement));
            }

            warnings.push(warning);
        }
    }

    // Check for migration path
    if let Some(migration) = get_migration_path(model_id) {
        if !migration.breaking_changes.is_empty() {
            warnings.push(
                ModelWarning::info(
                    model_id,
                    format!(
                        "Migration to '{}' available with breaking changes",
                        migration.to
                    ),
                )
                .with_action(format!(
                    "Review breaking changes: {}",
                    migration.breaking_changes.join(", ")
                )),
            );
        } else if !warnings
            .iter()
            .any(|w| w.level == WarningLevel::Warning || w.level == WarningLevel::Error)
        {
            warnings.push(ModelWarning::info(
                model_id,
                format!(
                    "Consider migrating to '{}' for better performance",
                    migration.to
                ),
            ));
        }
    }

    warnings
}

/// Check if model can be used.
pub fn can_use_model(model_id: &str) -> (bool, Option<String>) {
    let deprecation = is_deprecated(model_id);

    if deprecation.is_removed {
        let reason = if let Some(ref details) = deprecation.details {
            if let Some(ref replacement) = details.replacement {
                format!(
                    "Model '{}' has been removed. Please use '{}' instead.",
                    model_id, replacement
                )
            } else {
                format!("Model '{}' has been removed.", model_id)
            }
        } else {
            format!("Model '{}' is not available.", model_id)
        };
        return (false, Some(reason));
    }

    (true, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_warnings() {
        let warnings = check_model_warnings("gpt-4-0314");
        assert!(!warnings.is_empty());
    }

    #[test]
    fn test_can_use_model() {
        let (can_use, _) = can_use_model("gpt-4-turbo");
        assert!(can_use);
    }
}
