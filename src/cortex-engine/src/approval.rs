//! Approval system for tool execution.
//!
//! Provides a comprehensive approval system for controlling
//! when and how tools can be executed.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};

/// Risk level for operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RiskLevel {
    /// Safe operation.
    Low,
    /// Medium risk.
    #[default]
    Medium,
    /// High risk.
    High,
    /// Critical risk.
    Critical,
}

/// Approval mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ApprovalMode {
    /// Always require approval.
    Always,
    /// Auto-approve safe operations.
    AutoApprove,
    /// Never require approval (dangerous).
    Never,
    /// Use smart heuristics.
    #[default]
    Smart,
}

/// Approval preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ApprovalPreset {
    /// Strict: require approval for everything.
    Strict,
    /// Moderate: auto-approve read operations.
    #[default]
    Moderate,
    /// Permissive: auto-approve most operations.
    Permissive,
    /// Custom configuration.
    Custom,
}

impl ApprovalPreset {
    /// Get the approval configuration for this preset.
    pub fn config(&self) -> ApprovalConfig {
        match self {
            Self::Strict => ApprovalConfig {
                mode: ApprovalMode::Always,
                auto_approve_read: false,
                auto_approve_write_known: false,
                auto_approve_low_risk: false,
                require_approval_paths: Vec::new(),
                skip_approval_paths: Vec::new(),
                require_approval_tools: Vec::new(),
                skip_approval_tools: Vec::new(),
                max_auto_approvals: 0,
                timeout: Duration::from_secs(300),
            },
            Self::Moderate => ApprovalConfig {
                mode: ApprovalMode::Smart,
                auto_approve_read: true,
                auto_approve_write_known: true,
                auto_approve_low_risk: true,
                require_approval_paths: vec![
                    "/etc".to_string(),
                    "/usr".to_string(),
                    "~/.ssh".to_string(),
                ],
                skip_approval_paths: Vec::new(),
                require_approval_tools: vec!["shell".to_string(), "delete".to_string()],
                skip_approval_tools: vec![
                    "read_file".to_string(),
                    "list_dir".to_string(),
                    "grep".to_string(),
                ],
                max_auto_approvals: 100,
                timeout: Duration::from_secs(300),
            },
            Self::Permissive => ApprovalConfig {
                mode: ApprovalMode::AutoApprove,
                auto_approve_read: true,
                auto_approve_write_known: true,
                auto_approve_low_risk: true,
                require_approval_paths: vec!["/etc".to_string(), "~/.ssh".to_string()],
                skip_approval_paths: Vec::new(),
                require_approval_tools: Vec::new(),
                skip_approval_tools: Vec::new(),
                max_auto_approvals: 1000,
                timeout: Duration::from_secs(60),
            },
            Self::Custom => ApprovalConfig::default(),
        }
    }
}

/// Approval configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalConfig {
    /// Approval mode.
    pub mode: ApprovalMode,
    /// Auto-approve read operations.
    pub auto_approve_read: bool,
    /// Auto-approve writes to known/tracked files.
    pub auto_approve_write_known: bool,
    /// Auto-approve low-risk operations.
    pub auto_approve_low_risk: bool,
    /// Paths that always require approval.
    pub require_approval_paths: Vec<String>,
    /// Paths that skip approval.
    pub skip_approval_paths: Vec<String>,
    /// Tools that always require approval.
    pub require_approval_tools: Vec<String>,
    /// Tools that skip approval.
    pub skip_approval_tools: Vec<String>,
    /// Maximum auto-approvals before requiring manual.
    pub max_auto_approvals: u32,
    /// Timeout for approval requests.
    pub timeout: Duration,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        ApprovalPreset::Moderate.config()
    }
}

/// Approval request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Request ID.
    pub id: String,
    /// Tool name.
    pub tool: String,
    /// Tool arguments.
    pub arguments: serde_json::Value,
    /// Description of the operation.
    pub description: String,
    /// Risk level.
    pub risk: RiskLevel,
    /// Affected paths.
    pub affected_paths: Vec<PathBuf>,
    /// Created timestamp.
    pub created_at: u64,
    /// Expires at timestamp.
    pub expires_at: u64,
    /// Reason for requiring approval.
    pub reason: Option<String>,
}

impl ApprovalRequest {
    /// Create a new approval request.
    pub fn new(tool: impl Into<String>, arguments: serde_json::Value) -> Self {
        let now = timestamp_now();
        Self {
            id: generate_id(),
            tool: tool.into(),
            arguments,
            description: String::new(),
            risk: RiskLevel::Medium,
            affected_paths: Vec::new(),
            created_at: now,
            expires_at: now + 300,
            reason: None,
        }
    }

    /// Set description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Set risk level.
    pub fn with_risk(mut self, risk: RiskLevel) -> Self {
        self.risk = risk;
        self
    }

    /// Add affected path.
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.affected_paths.push(path.into());
        self
    }

    /// Set reason.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Check if request has expired.
    pub fn is_expired(&self) -> bool {
        timestamp_now() > self.expires_at
    }

    /// Format for display.
    pub fn format(&self) -> String {
        let mut output = String::new();

        output.push_str(&format!("Tool: {}\n", self.tool));
        output.push_str(&format!("Risk: {:?}\n", self.risk));

        if !self.description.is_empty() {
            output.push_str(&format!("Description: {}\n", self.description));
        }

        if !self.affected_paths.is_empty() {
            output.push_str("Affected paths:\n");
            for path in &self.affected_paths {
                output.push_str(&format!("  - {}\n", path.display()));
            }
        }

        if let Some(ref reason) = self.reason {
            output.push_str(&format!("Reason: {reason}\n"));
        }

        output
    }
}

/// Approval response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    /// Request ID.
    pub request_id: String,
    /// Is approved.
    pub approved: bool,
    /// Reason for decision.
    pub reason: Option<String>,
    /// Was auto-approved.
    pub auto_approved: bool,
    /// Response timestamp.
    pub timestamp: u64,
    /// Responder (user ID or "system").
    pub responder: String,
}

impl ApprovalResponse {
    /// Create an approval response.
    pub fn approve(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            approved: true,
            reason: None,
            auto_approved: false,
            timestamp: timestamp_now(),
            responder: "user".to_string(),
        }
    }

    /// Create a denial response.
    pub fn deny(request_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            approved: false,
            reason: Some(reason.into()),
            auto_approved: false,
            timestamp: timestamp_now(),
            responder: "user".to_string(),
        }
    }

    /// Create an auto-approval response.
    pub fn auto_approve(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            approved: true,
            reason: Some("Auto-approved".to_string()),
            auto_approved: true,
            timestamp: timestamp_now(),
            responder: "system".to_string(),
        }
    }

    /// Create a timeout response.
    pub fn timeout(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            approved: false,
            reason: Some("Approval request timed out".to_string()),
            auto_approved: false,
            timestamp: timestamp_now(),
            responder: "system".to_string(),
        }
    }
}

/// Approval manager.
pub struct ApprovalManager {
    /// Configuration.
    config: RwLock<ApprovalConfig>,
    /// Pending requests.
    pending: RwLock<HashMap<String, ApprovalRequest>>,
    /// History of responses.
    history: RwLock<Vec<ApprovalResponse>>,
    /// Auto-approval counter.
    auto_approval_count: RwLock<u32>,
    /// Known/trusted paths.
    trusted_paths: RwLock<HashSet<PathBuf>>,
}

impl ApprovalManager {
    /// Create a new approval manager.
    pub fn new(config: ApprovalConfig) -> Self {
        Self {
            config: RwLock::new(config),
            pending: RwLock::new(HashMap::new()),
            history: RwLock::new(Vec::new()),
            auto_approval_count: RwLock::new(0),
            trusted_paths: RwLock::new(HashSet::new()),
        }
    }

    /// Create with default config.
    pub fn default_manager() -> Self {
        Self::new(ApprovalConfig::default())
    }

    /// Create with preset.
    pub fn with_preset(preset: ApprovalPreset) -> Self {
        Self::new(preset.config())
    }

    /// Check if approval is required.
    pub async fn requires_approval(&self, request: &ApprovalRequest) -> bool {
        let config = self.config.read().await;

        // Check mode
        match config.mode {
            ApprovalMode::Never => return false,
            ApprovalMode::Always => return true,
            _ => {}
        }

        // Check if tool is in skip list
        if config.skip_approval_tools.contains(&request.tool) {
            return false;
        }

        // Check if tool requires approval
        if config.require_approval_tools.contains(&request.tool) {
            return true;
        }

        // Check paths
        for path in &request.affected_paths {
            let path_str = path.to_string_lossy();

            // Check required approval paths
            for pattern in &config.require_approval_paths {
                if path_str.contains(pattern) || pattern.contains(&path_str.to_string()) {
                    return true;
                }
            }

            // Check skip paths
            for pattern in &config.skip_approval_paths {
                if path_str.contains(pattern) {
                    return false;
                }
            }
        }

        // Check auto-approval conditions
        if config.auto_approve_low_risk && request.risk == RiskLevel::Low {
            return false;
        }

        // Check max auto-approvals
        let count = *self.auto_approval_count.read().await;
        if count >= config.max_auto_approvals {
            return true;
        }

        // Smart mode: analyze the request
        if config.mode == ApprovalMode::Smart {
            return self.smart_requires_approval(request, &config).await;
        }

        true
    }

    /// Smart approval analysis.
    async fn smart_requires_approval(
        &self,
        request: &ApprovalRequest,
        config: &ApprovalConfig,
    ) -> bool {
        // Read operations
        if is_read_operation(&request.tool) && config.auto_approve_read {
            return false;
        }

        // Write to known paths
        if config.auto_approve_write_known {
            let trusted = self.trusted_paths.read().await;
            for path in &request.affected_paths {
                if trusted.contains(path) {
                    return false;
                }
            }
        }

        // High risk always requires approval
        if request.risk == RiskLevel::High || request.risk == RiskLevel::Critical {
            return true;
        }

        // Default to requiring approval for unknown operations
        true
    }

    /// Submit an approval request.
    pub async fn submit(&self, request: ApprovalRequest) -> String {
        let id = request.id.clone();
        self.pending.write().await.insert(id.clone(), request);
        id
    }

    /// Get a pending request.
    pub async fn get_pending(&self, id: &str) -> Option<ApprovalRequest> {
        self.pending.read().await.get(id).cloned()
    }

    /// List pending requests.
    pub async fn list_pending(&self) -> Vec<ApprovalRequest> {
        self.pending.read().await.values().cloned().collect()
    }

    /// Respond to a request.
    pub async fn respond(&self, response: ApprovalResponse) -> Result<()> {
        let mut pending = self.pending.write().await;

        if !pending.contains_key(&response.request_id) {
            return Err(CortexError::NotFound(format!(
                "Approval request not found: {}",
                response.request_id
            )));
        }

        pending.remove(&response.request_id);

        // Track auto-approvals
        if response.auto_approved {
            *self.auto_approval_count.write().await += 1;
        }

        // Add to history
        self.history.write().await.push(response);

        Ok(())
    }

    /// Auto-approve a request if applicable.
    pub async fn try_auto_approve(&self, request: &ApprovalRequest) -> Option<ApprovalResponse> {
        if !self.requires_approval(request).await {
            let response = ApprovalResponse::auto_approve(&request.id);
            Some(response)
        } else {
            None
        }
    }

    /// Add trusted path.
    pub async fn trust_path(&self, path: impl Into<PathBuf>) {
        self.trusted_paths.write().await.insert(path.into());
    }

    /// Remove trusted path.
    pub async fn untrust_path(&self, path: &PathBuf) {
        self.trusted_paths.write().await.remove(path);
    }

    /// Get approval history.
    pub async fn history(&self) -> Vec<ApprovalResponse> {
        self.history.read().await.clone()
    }

    /// Clear pending requests.
    pub async fn clear_pending(&self) {
        self.pending.write().await.clear();
    }

    /// Reset auto-approval counter.
    pub async fn reset_counter(&self) {
        *self.auto_approval_count.write().await = 0;
    }

    /// Update configuration.
    pub async fn update_config(&self, config: ApprovalConfig) {
        *self.config.write().await = config;
    }

    /// Get current config.
    pub async fn get_config(&self) -> ApprovalConfig {
        self.config.read().await.clone()
    }
}

impl Default for ApprovalManager {
    fn default() -> Self {
        Self::default_manager()
    }
}

/// Check if an operation is read-only.
fn is_read_operation(tool: &str) -> bool {
    const READ_TOOLS: &[&str] = &[
        "read_file",
        "list_dir",
        "grep",
        "find",
        "search",
        "view",
        "cat",
        "head",
        "tail",
        "git_status",
        "git_log",
        "git_diff",
    ];
    READ_TOOLS.contains(&tool)
}

/// Generate unique ID.
fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    format!("apr_{ts:x}")
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_approval_preset() {
        let config = ApprovalPreset::Strict.config();
        assert_eq!(config.mode, ApprovalMode::Always);
        assert!(!config.auto_approve_read);

        let config = ApprovalPreset::Permissive.config();
        assert_eq!(config.mode, ApprovalMode::AutoApprove);
        assert!(config.auto_approve_read);
    }

    #[test]
    fn test_approval_request() {
        let request = ApprovalRequest::new("write_file", serde_json::json!({"path": "/test"}))
            .with_description("Write to file")
            .with_risk(RiskLevel::Medium);

        assert_eq!(request.tool, "write_file");
        assert_eq!(request.risk, RiskLevel::Medium);
    }

    #[tokio::test]
    async fn test_approval_manager() {
        let manager = ApprovalManager::with_preset(ApprovalPreset::Moderate);

        let request =
            ApprovalRequest::new("read_file", serde_json::json!({})).with_risk(RiskLevel::Low);

        // Read operations should be auto-approved with moderate preset
        assert!(!manager.requires_approval(&request).await);
    }

    #[tokio::test]
    async fn test_trusted_paths() {
        let manager = ApprovalManager::default_manager();
        let path = PathBuf::from("/project/src/main.rs");

        manager.trust_path(&path).await;

        let trusted = manager.trusted_paths.read().await;
        assert!(trusted.contains(&path));
    }
}
