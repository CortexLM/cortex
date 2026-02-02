//! Migration paths between models.

use serde::{Deserialize, Serialize};

/// A migration path from one model to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationPath {
    /// Source model.
    pub from: String,
    /// Target model.
    pub to: String,
    /// Migration notes.
    pub notes: Vec<String>,
    /// Whether migration is automatic.
    pub automatic: bool,
    /// Breaking changes.
    pub breaking_changes: Vec<String>,
}

impl MigrationPath {
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            notes: Vec::new(),
            automatic: true,
            breaking_changes: Vec::new(),
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_breaking_change(mut self, change: impl Into<String>) -> Self {
        self.breaking_changes.push(change.into());
        self.automatic = false;
        self
    }

    pub fn manual(mut self) -> Self {
        self.automatic = false;
        self
    }
}

/// Get migration path for a model.
pub fn get_migration_path(from_model: &str) -> Option<MigrationPath> {
    // GPT-4 migrations
    if from_model.starts_with("gpt-4-0314") || from_model.starts_with("gpt-4-0613") {
        return Some(
            MigrationPath::new(from_model, "gpt-4-turbo")
                .with_note("GPT-4 Turbo has improved performance and lower cost")
                .with_note("Context window increased from 8K to 128K tokens"),
        );
    }

    if from_model.starts_with("gpt-4-32k") {
        return Some(
            MigrationPath::new(from_model, "gpt-4-turbo")
                .with_note("GPT-4 Turbo has 128K context, larger than 32K")
                .with_note("Significantly lower cost per token"),
        );
    }

    // GPT-3.5 migrations
    if from_model.starts_with("gpt-3.5-turbo-0301") || from_model.starts_with("gpt-3.5-turbo-0613")
    {
        return Some(
            MigrationPath::new(from_model, "gpt-3.5-turbo")
                .with_note("Latest GPT-3.5 Turbo has improved instruction following"),
        );
    }

    // Claude migrations
    if from_model.starts_with("claude-instant") {
        return Some(
            MigrationPath::new(from_model, "claude-3-haiku-20240307")
                .with_note("Claude 3 Haiku is faster and more capable")
                .with_note("Better at following complex instructions"),
        );
    }

    if from_model.starts_with("claude-2") {
        return Some(
            MigrationPath::new(from_model, "claude-3-sonnet-20240229")
                .with_note("Claude 3 Sonnet offers better reasoning")
                .with_note("Improved code generation capabilities"),
        );
    }

    // Legacy code models
    if from_model.starts_with("code-davinci") || from_model.starts_with("code-cushman") {
        return Some(
            MigrationPath::new(from_model, "gpt-4")
                .with_note("Legacy code models are deprecated")
                .with_note("GPT-4 provides superior code generation")
                .with_breaking_change("API format changed from completions to chat"),
        );
    }

    None
}

/// Get recommended model for a use case.
pub fn recommend_model(use_case: &str) -> Option<&'static str> {
    match use_case.to_lowercase().as_str() {
        "code" | "coding" | "programming" => Some("gpt-4-turbo"),
        "chat" | "conversation" => Some("gpt-3.5-turbo"),
        "analysis" | "reasoning" => Some("claude-3-opus-20240229"),
        "fast" | "quick" => Some("claude-3-haiku-20240307"),
        "cheap" | "budget" => Some("gpt-3.5-turbo"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migration_path() {
        let path = get_migration_path("gpt-4-0314");
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path.to, "gpt-4-turbo");
    }

    #[test]
    fn test_recommend_model() {
        assert_eq!(recommend_model("code"), Some("gpt-4-turbo"));
        assert_eq!(recommend_model("fast"), Some("claude-3-haiku-20240307"));
    }
}
