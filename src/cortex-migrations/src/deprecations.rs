//! Deprecated model definitions.

use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// Information about a deprecated model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeprecatedModel {
    /// Model ID.
    pub model_id: String,
    /// When it was deprecated.
    pub deprecated_at: NaiveDate,
    /// When it will be removed.
    pub removal_date: Option<NaiveDate>,
    /// Recommended replacement.
    pub replacement: Option<String>,
    /// Deprecation reason.
    pub reason: Option<String>,
}

impl DeprecatedModel {
    pub fn new(model_id: impl Into<String>) -> Self {
        Self {
            model_id: model_id.into(),
            deprecated_at: Utc::now().date_naive(),
            removal_date: None,
            replacement: None,
            reason: None,
        }
    }

    pub fn with_replacement(mut self, replacement: impl Into<String>) -> Self {
        self.replacement = Some(replacement.into());
        self
    }

    pub fn with_removal_date(mut self, year: i32, month: u32, day: u32) -> Self {
        self.removal_date = NaiveDate::from_ymd_opt(year, month, day);
        self
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Check if model is past removal date.
    pub fn is_removed(&self) -> bool {
        if let Some(removal) = self.removal_date {
            Utc::now().date_naive() >= removal
        } else {
            false
        }
    }

    /// Days until removal.
    pub fn days_until_removal(&self) -> Option<i64> {
        self.removal_date
            .map(|removal| (removal - Utc::now().date_naive()).num_days())
    }
}

/// Deprecation info for a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeprecationInfo {
    /// Whether the model is deprecated.
    pub is_deprecated: bool,
    /// Whether the model is removed.
    pub is_removed: bool,
    /// Deprecation details.
    pub details: Option<DeprecatedModel>,
    /// Warning message.
    pub warning_message: Option<String>,
}

impl DeprecationInfo {
    pub fn not_deprecated() -> Self {
        Self {
            is_deprecated: false,
            is_removed: false,
            details: None,
            warning_message: None,
        }
    }

    pub fn deprecated(model: DeprecatedModel) -> Self {
        let is_removed = model.is_removed();
        let warning_message = Some(format_deprecation_warning(&model));
        Self {
            is_deprecated: true,
            is_removed,
            details: Some(model),
            warning_message,
        }
    }
}

fn format_deprecation_warning(model: &DeprecatedModel) -> String {
    let mut msg = format!("Model '{}' is deprecated", model.model_id);

    if let Some(ref replacement) = model.replacement {
        msg.push_str(&format!(". Please use '{}' instead", replacement));
    }

    if let Some(days) = model.days_until_removal() {
        if days > 0 {
            msg.push_str(&format!(". Will be removed in {} days", days));
        } else {
            msg.push_str(". This model has been removed");
        }
    }

    if let Some(ref reason) = model.reason {
        msg.push_str(&format!(". Reason: {}", reason));
    }

    msg
}

/// List of deprecated models.
#[allow(clippy::type_complexity)]
pub static DEPRECATED_MODELS: &[(&str, &str, Option<(i32, u32, u32)>)] = &[
    // OpenAI
    ("gpt-4-0314", "gpt-4", Some((2024, 6, 13))),
    ("gpt-4-32k-0314", "gpt-4-32k", Some((2024, 6, 13))),
    ("gpt-3.5-turbo-0301", "gpt-3.5-turbo", Some((2024, 6, 13))),
    ("text-davinci-003", "gpt-3.5-turbo", Some((2024, 1, 4))),
    ("code-davinci-002", "gpt-4", Some((2024, 1, 4))),
    // Anthropic
    ("claude-instant-1", "claude-3-haiku-20240307", None),
    ("claude-2.0", "claude-3-sonnet-20240229", None),
    ("claude-2.1", "claude-3-sonnet-20240229", None),
];

/// Check if a model is deprecated.
pub fn is_deprecated(model_id: &str) -> DeprecationInfo {
    for (deprecated_id, replacement, removal_date) in DEPRECATED_MODELS {
        if model_id == *deprecated_id || model_id.starts_with(deprecated_id) {
            let mut model = DeprecatedModel::new(*deprecated_id).with_replacement(*replacement);

            if let Some((year, month, day)) = removal_date {
                model = model.with_removal_date(*year, *month, *day);
            }

            return DeprecationInfo::deprecated(model);
        }
    }

    DeprecationInfo::not_deprecated()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deprecated_model() {
        let info = is_deprecated("gpt-4-0314");
        assert!(info.is_deprecated);
        assert!(info.details.unwrap().replacement.is_some());
    }

    #[test]
    fn test_not_deprecated() {
        let info = is_deprecated("gpt-4-turbo");
        assert!(!info.is_deprecated);
    }
}
