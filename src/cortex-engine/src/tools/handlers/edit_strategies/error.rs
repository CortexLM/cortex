//! Error types for edit replacement strategies.

use super::helpers::truncate_string;

/// Errors that can occur during edit operations
#[derive(Debug, Clone)]
pub enum EditError {
    /// No match found for the search text
    NoMatchFound {
        search: String,
        strategies_tried: Vec<&'static str>,
    },
    /// Multiple matches found (ambiguous)
    MultipleMatches {
        count: usize,
        strategy: &'static str,
        hint: String,
    },
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::NoMatchFound {
                search,
                strategies_tried,
            } => {
                write!(
                    f,
                    "Could not find '{}' in file (tried strategies: {})",
                    truncate_string(search, 50),
                    strategies_tried.join(", ")
                )
            }
            EditError::MultipleMatches {
                count,
                strategy,
                hint,
            } => {
                write!(
                    f,
                    "Found {} occurrences using {} strategy. {}",
                    count, strategy, hint
                )
            }
        }
    }
}

impl std::error::Error for EditError {}
