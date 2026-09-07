//! Private controller IPC, never mounted on the miner API. Bind each instance
//! to one scope and lease; agents cannot supply accounts, fences or resources.

#![forbid(unsafe_code)]

mod experiment;
mod transport;

pub use experiment::*;
pub use transport::*;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeScope {
    pub role: String,
    pub id: String,
    pub commitment: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCall {
    pub schema_version: u32,
    pub scope: RuntimeScope,
    pub operation: String,
    pub arguments: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("runtime request is outside its capability")]
    Scope,
    #[error("runtime operation unavailable or interrupted")]
    Unavailable,
}

impl From<proof_autonomy_pg::StoreError> for RuntimeError {
    fn from(_: proof_autonomy_pg::StoreError) -> Self {
        Self::Scope
    }
}

impl From<proof_research::ResearchError> for RuntimeError {
    fn from(_: proof_research::ResearchError) -> Self {
        Self::Unavailable
    }
}

impl From<sqlx::Error> for RuntimeError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}

impl From<serde_json::Error> for RuntimeError {
    fn from(_: serde_json::Error) -> Self {
        Self::Scope
    }
}

/// Implementations must verify their bound scope and current durable ownership
/// on every call. This interface conveys no authority to sign or publish.
#[async_trait]
pub trait RuntimeOperations: Send + Sync {
    async fn call(&self, request: RuntimeCall) -> Result<Value, RuntimeError>;
}
