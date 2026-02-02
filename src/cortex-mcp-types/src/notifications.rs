//! Notification types for MCP protocol.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::jsonrpc::RequestId;

/// Progress notification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProgressNotification {
    /// Progress token.
    pub progress_token: ProgressToken,
    /// Current progress.
    pub progress: f64,
    /// Total (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
}

/// Progress token.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ProgressToken {
    /// Numeric token.
    Number(i64),
    /// String token.
    String(String),
}

/// Cancelled notification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CancelledNotification {
    /// Request ID to cancel.
    pub request_id: RequestId,
    /// Reason for cancellation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
