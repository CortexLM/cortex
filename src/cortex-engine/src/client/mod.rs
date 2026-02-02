//! Cortex Backend Client
//!
//! Provides unified interface for the Cortex Backend API.
//! All LLM requests go through the Cortex backend with OAuth authentication.

mod cortex;
pub mod types;

pub use cortex::{CortexClient, CortexModel, PricingInfo};
pub use types::*;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use tokio_stream::Stream;

use crate::error::{CortexError, Result};

/// Stream type for response events.
pub type ResponseStream = Pin<Box<dyn Stream<Item = Result<ResponseEvent>> + Send>>;

/// Simple tool call reference for MessageContent::ToolCalls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRef {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Trait for model clients.
#[async_trait]
pub trait ModelClient: Send + Sync {
    /// Get the model name.
    fn model(&self) -> &str;

    /// Get the provider name.
    fn provider(&self) -> &str;

    /// Get model capabilities.
    fn capabilities(&self) -> &ModelCapabilities;

    /// Send a completion request and get a stream of responses.
    async fn complete(&self, request: CompletionRequest) -> Result<ResponseStream>;

    /// Send a completion request and get the full response (non-streaming).
    async fn complete_sync(&self, request: CompletionRequest) -> Result<CompletionResponse>;
}

/// Get the Cortex auth token from environment or keyring.
fn get_auth_token() -> Result<String> {
    // First check environment variable
    if let Ok(token) = std::env::var("CORTEX_AUTH_TOKEN") {
        return Ok(token);
    }

    // Try to load from cortex-login keyring storage
    if let Some(token) = cortex_login::get_auth_token() {
        return Ok(token);
    }

    // Not authenticated
    Err(CortexError::Auth(
        "Not authenticated. Run 'cortex login' first or set CORTEX_AUTH_TOKEN environment variable.".to_string()
    ))
}

/// Create a Cortex backend client.
///
/// All requests go through the Cortex backend with OAuth authentication.
///
/// # Legacy signature compatibility
/// The `_provider_id` and `_base_url` parameters are ignored - all requests
/// go through Cortex backend. The `api_key` parameter is used as the auth token.
pub fn create_client(
    _provider_id: &str,
    model: &str,
    api_key: &str,
    _base_url: Option<&str>,
) -> Result<Box<dyn ModelClient>> {
    // Use provided api_key as auth token, or try to get from environment/keyring
    let auth_token = if !api_key.is_empty() {
        api_key.to_string()
    } else {
        get_auth_token()?
    };

    let base_url = std::env::var("CORTEX_API_URL").ok();
    Ok(Box::new(
        CortexClient::new(model.to_string(), base_url).with_auth_token(auth_token),
    ))
}

/// Create a Cortex client with explicit auth token.
pub fn create_client_with_auth(model: &str, auth_token: &str) -> Box<dyn ModelClient> {
    let base_url = std::env::var("CORTEX_API_URL").ok();
    Box::new(CortexClient::new(model.to_string(), base_url).with_auth_token(auth_token.to_string()))
}

/// Create a Cortex client with custom base URL.
pub fn create_client_with_url(
    model: &str,
    auth_token: &str,
    base_url: &str,
) -> Box<dyn ModelClient> {
    Box::new(
        CortexClient::new(model.to_string(), Some(base_url.to_string()))
            .with_auth_token(auth_token.to_string()),
    )
}
