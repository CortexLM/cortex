//! Execution policy decision types.

use serde::{Deserialize, Serialize};

/// Execution policy decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Decision {
    /// Execution authorized automatically.
    Allow,
    /// Requires user confirmation.
    Ask,
    /// Execution prohibited.
    Deny,
}

impl Decision {
    /// Returns true if the decision allows execution (possibly after confirmation).
    pub fn allows_execution(&self) -> bool {
        matches!(self, Decision::Allow | Decision::Ask)
    }

    /// Returns true if the decision requires user interaction.
    pub fn requires_confirmation(&self) -> bool {
        matches!(self, Decision::Ask)
    }

    /// Returns true if the decision blocks execution.
    pub fn is_blocked(&self) -> bool {
        matches!(self, Decision::Deny)
    }

    /// Combine two decisions, taking the most restrictive.
    pub fn combine(self, other: Decision) -> Decision {
        match (self, other) {
            (Decision::Deny, _) | (_, Decision::Deny) => Decision::Deny,
            (Decision::Ask, _) | (_, Decision::Ask) => Decision::Ask,
            _ => Decision::Allow,
        }
    }
}

impl std::fmt::Display for Decision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Decision::Allow => write!(f, "ALLOW"),
            Decision::Ask => write!(f, "ASK"),
            Decision::Deny => write!(f, "DENY"),
        }
    }
}
