//! Favorite command - toggles the favorite status of the current session.
//!
//! Usage: /favorite
//!
//! This command toggles the favorite status of the current session.
//! Favorite sessions are preserved longer and can be filtered in the
//! session list using `cortex sessions --favorites`.

/// Result of executing the favorite command.
#[derive(Debug, Clone)]
pub enum FavoriteResult {
    /// Favorite status changed.
    Toggled {
        /// Whether the session is now a favorite.
        is_favorite: bool,
    },
    /// Error during operation.
    Error(String),
}

impl FavoriteResult {
    /// Get a user-friendly message for the result.
    pub fn message(&self) -> String {
        match self {
            FavoriteResult::Toggled { is_favorite: true } => {
                "Session marked as favorite".to_string()
            }
            FavoriteResult::Toggled { is_favorite: false } => {
                "Session removed from favorites".to_string()
            }
            FavoriteResult::Error(e) => format!("Error: Failed to toggle favorite: {}", e),
        }
    }

    /// Check if the operation was successful.
    pub fn is_success(&self) -> bool {
        matches!(self, FavoriteResult::Toggled { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_favorite_result_messages() {
        let added = FavoriteResult::Toggled { is_favorite: true };
        assert!(added.message().contains("marked as favorite"));
        assert!(added.is_success());

        let removed = FavoriteResult::Toggled { is_favorite: false };
        assert!(removed.message().contains("removed"));
        assert!(removed.is_success());

        let error = FavoriteResult::Error("test error".to_string());
        assert!(error.message().contains("Failed"));
        assert!(!error.is_success());
    }
}
