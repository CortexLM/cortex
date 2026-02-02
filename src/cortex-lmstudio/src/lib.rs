//! LM Studio client for Cortex CLI
//!
//! This crate provides functionality to interact with LM Studio's local server
//! for model management and inference.

mod client;
mod models;

pub use client::LMStudioClient;
pub use models::{ChatMessage, ChatRequest, ChatResponse, Model, ModelInfo};

/// Default OSS model to use when `--oss` is passed without an explicit `-m`.
pub const DEFAULT_OSS_MODEL: &str = "openai/gpt-oss-20b";

/// Default LM Studio server URL
pub const DEFAULT_LMSTUDIO_URL: &str = "http://localhost:1234/v1";

/// LM Studio provider identifier
pub const LMSTUDIO_PROVIDER_ID: &str = "lmstudio";

/// Error types for LM Studio operations
#[derive(Debug, thiserror::Error)]
pub enum LMStudioError {
    #[error(
        "LM Studio is not responding. Install from https://lmstudio.ai/download and run 'lms server start'."
    )]
    ConnectionError,

    #[error("Server returned error: {0}")]
    ServerError(String),

    #[error("Request failed: {0}")]
    RequestError(#[from] reqwest::Error),

    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("LM Studio not found. Please install LM Studio from https://lmstudio.ai/")]
    LMStudioNotFound,

    #[error("Model download failed: {0}")]
    DownloadError(String),

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Result type for LM Studio operations
pub type Result<T> = std::result::Result<T, LMStudioError>;

/// Prepare the local OSS environment when `--oss` is selected.
///
/// - Ensures a local LM Studio server is reachable.
/// - Checks if the model exists locally and downloads it if missing.
pub async fn ensure_oss_ready(
    base_url: Option<&str>,
    model: Option<&str>,
) -> Result<LMStudioClient> {
    let model = model.unwrap_or(DEFAULT_OSS_MODEL);
    let base_url = base_url.unwrap_or(DEFAULT_LMSTUDIO_URL);

    // Verify local LM Studio is reachable
    let client = LMStudioClient::new(base_url).await?;

    match client.fetch_models().await {
        Ok(models) => {
            if !models.iter().any(|m| m.id == model) {
                client.download_model(model).await?;
            }
        }
        Err(err) => {
            // Not fatal; higher layers may still proceed and surface errors later.
            tracing::warn!("Failed to query local models from LM Studio: {}.", err);
        }
    }

    // Load the model in the background
    let load_client = client.clone();
    let model_name = model.to_string();
    tokio::spawn(async move {
        if let Err(e) = load_client.load_model(&model_name).await {
            tracing::warn!("Failed to load model {}: {}", model_name, e);
        }
    });

    Ok(client)
}
