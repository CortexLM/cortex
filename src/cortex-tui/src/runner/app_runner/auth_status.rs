//! Authentication status for startup check.

// ============================================================================
// Authentication Status
// ============================================================================

/// Authentication status for startup check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatus {
    /// User is authenticated.
    Authenticated,
    /// Session has expired.
    Expired,
    /// User is not authenticated.
    NotAuthenticated,
}
