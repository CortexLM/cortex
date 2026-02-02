//! Session-scoped approval memory.
//!
//! Remembers user decisions (allow/deny/always) for the duration of a session.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// User's decision for an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    /// Allow this specific request.
    Allow,
    /// Deny this specific request.
    Deny,
    /// Always allow requests matching this pattern.
    Always,
    /// Always deny requests matching this pattern.
    Never,
}

/// Type of approval being remembered.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ApprovalType {
    /// Bash command execution.
    BashCommand(String),
    /// File edit operation.
    FileEdit(String),
    /// External directory access.
    ExternalDirectory(String),
    /// Skill execution.
    Skill(String),
    /// URL fetch.
    UrlFetch(String),
    /// Custom approval type.
    Custom(String, String),
}

impl ApprovalType {
    /// Get a hash key for this approval type.
    fn key(&self) -> String {
        match self {
            ApprovalType::BashCommand(cmd) => format!("bash:{}", cmd),
            ApprovalType::FileEdit(path) => format!("edit:{}", path),
            ApprovalType::ExternalDirectory(path) => format!("external:{}", path),
            ApprovalType::Skill(name) => format!("skill:{}", name),
            ApprovalType::UrlFetch(url) => format!("url:{}", url),
            ApprovalType::Custom(typ, val) => format!("{}:{}", typ, val),
        }
    }

    /// Get a pattern key for "always" decisions.
    fn pattern_key(&self) -> String {
        match self {
            ApprovalType::BashCommand(cmd) => {
                // Extract command prefix for pattern matching
                let parts: Vec<&str> = cmd.split_whitespace().collect();
                if parts.len() >= 2 {
                    format!("bash:{}:{}", parts[0], parts[1])
                } else if !parts.is_empty() {
                    format!("bash:{}", parts[0])
                } else {
                    "bash:".to_string()
                }
            }
            ApprovalType::FileEdit(_) => "edit:*".to_string(),
            ApprovalType::ExternalDirectory(_) => "external:*".to_string(),
            ApprovalType::Skill(name) => format!("skill:{}", name),
            ApprovalType::UrlFetch(url) => {
                // Extract domain for pattern
                if let Some(domain) = url.split('/').nth(2) {
                    format!("url:{}", domain)
                } else {
                    format!("url:{}", url)
                }
            }
            ApprovalType::Custom(typ, _) => format!("{}:*", typ),
        }
    }
}

/// Memory of approval decisions for a session.
#[derive(Debug, Default)]
pub struct ApprovalMemory {
    /// Exact approvals (specific requests).
    exact: Arc<RwLock<HashMap<String, ApprovalDecision>>>,
    /// Pattern approvals (for "always" decisions).
    patterns: Arc<RwLock<HashMap<String, ApprovalDecision>>>,
}

impl ApprovalMemory {
    /// Create new approval memory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an approval decision.
    pub async fn record(&self, approval_type: ApprovalType, decision: ApprovalDecision) {
        let key = approval_type.key();

        match decision {
            ApprovalDecision::Always | ApprovalDecision::Never => {
                // Store as pattern for future matching
                let pattern_key = approval_type.pattern_key();
                let mut patterns = self.patterns.write().await;
                patterns.insert(pattern_key, decision);
            }
            _ => {
                // Store exact match only
                let mut exact = self.exact.write().await;
                exact.insert(key, decision);
            }
        }
    }

    /// Check if we have a remembered decision for this approval type.
    pub async fn check(&self, approval_type: &ApprovalType) -> Option<ApprovalDecision> {
        let key = approval_type.key();
        let pattern_key = approval_type.pattern_key();

        // Check exact match first
        {
            let exact = self.exact.read().await;
            if let Some(decision) = exact.get(&key) {
                return Some(*decision);
            }
        }

        // Check pattern match
        {
            let patterns = self.patterns.read().await;
            if let Some(decision) = patterns.get(&pattern_key) {
                return Some(*decision);
            }
        }

        None
    }

    /// Clear all approvals.
    pub async fn clear(&self) {
        self.exact.write().await.clear();
        self.patterns.write().await.clear();
    }

    /// Get count of remembered approvals.
    pub async fn count(&self) -> usize {
        let exact = self.exact.read().await;
        let patterns = self.patterns.read().await;
        exact.len() + patterns.len()
    }
}

impl Clone for ApprovalMemory {
    fn clone(&self) -> Self {
        Self {
            exact: Arc::clone(&self.exact),
            patterns: Arc::clone(&self.patterns),
        }
    }
}

/// Global approval memory instance.
static GLOBAL_MEMORY: std::sync::OnceLock<ApprovalMemory> = std::sync::OnceLock::new();

/// Get the global approval memory.
pub fn global_memory() -> &'static ApprovalMemory {
    GLOBAL_MEMORY.get_or_init(ApprovalMemory::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_exact_approval() {
        let memory = ApprovalMemory::new();
        let approval = ApprovalType::BashCommand("git status".to_string());

        // No decision yet
        assert!(memory.check(&approval).await.is_none());

        // Record allow
        memory
            .record(approval.clone(), ApprovalDecision::Allow)
            .await;

        // Should remember
        assert_eq!(memory.check(&approval).await, Some(ApprovalDecision::Allow));
    }

    #[tokio::test]
    async fn test_always_approval() {
        let memory = ApprovalMemory::new();
        let approval1 = ApprovalType::BashCommand("git push origin main".to_string());
        let approval2 = ApprovalType::BashCommand("git push origin dev".to_string());

        // Record "always" for git push
        memory
            .record(approval1.clone(), ApprovalDecision::Always)
            .await;

        // Both should match
        assert_eq!(
            memory.check(&approval1).await,
            Some(ApprovalDecision::Always)
        );
        assert_eq!(
            memory.check(&approval2).await,
            Some(ApprovalDecision::Always)
        );
    }

    #[tokio::test]
    async fn test_different_types() {
        let memory = ApprovalMemory::new();

        let bash = ApprovalType::BashCommand("ls".to_string());
        let edit = ApprovalType::FileEdit("/tmp/test.txt".to_string());

        memory.record(bash.clone(), ApprovalDecision::Allow).await;
        memory.record(edit.clone(), ApprovalDecision::Deny).await;

        assert_eq!(memory.check(&bash).await, Some(ApprovalDecision::Allow));
        assert_eq!(memory.check(&edit).await, Some(ApprovalDecision::Deny));
    }
}
