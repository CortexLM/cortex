//! LM Studio client implementation

use crate::models::{ChatRequest, ChatResponse, Model, ModelsResponse};
use crate::{DEFAULT_LMSTUDIO_URL, LMStudioError, Result};
use std::path::Path;
use std::time::Duration;

/// Client for interacting with LM Studio's local server
#[derive(Clone)]
pub struct LMStudioClient {
    client: reqwest::Client,
    base_url: String,
}

impl LMStudioClient {
    /// Create a new LM Studio client with the given base URL
    ///
    /// # Arguments
    /// * `base_url` - The base URL of the LM Studio server (e.g., "http://localhost:1234/v1")
    ///
    /// # Returns
    /// A new client if the server is reachable, or an error otherwise
    pub async fn new(base_url: impl Into<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(300)) // 5 min timeout for completions
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let lmstudio = Self {
            client,
            base_url: base_url.into(),
        };
        lmstudio.check_server().await?;
        Ok(lmstudio)
    }

    /// Create a new LM Studio client with the default URL
    pub async fn new_default() -> Result<Self> {
        Self::new(DEFAULT_LMSTUDIO_URL).await
    }

    /// Create a client without checking server connectivity (for testing)
    #[cfg(test)]
    pub(crate) fn from_host_root(host_root: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            base_url: host_root.into(),
        }
    }

    /// Get the base URL
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Check if the server is reachable
    async fn check_server(&self) -> Result<()> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let response = self.client.get(&url).send().await;

        match response {
            Ok(resp) if resp.status().is_success() => Ok(()),
            Ok(resp) => Err(LMStudioError::ServerError(format!(
                "{} - {}",
                resp.status(),
                "LM Studio is not responding"
            ))),
            Err(_) => Err(LMStudioError::ConnectionError),
        }
    }

    /// Fetch the list of available models from the server
    pub async fn fetch_models(&self) -> Result<Vec<Model>> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let response = self.client.get(&url).send().await?;

        if response.status().is_success() {
            let models_resp: ModelsResponse = response.json().await?;
            Ok(models_resp.data)
        } else {
            Err(LMStudioError::ServerError(format!(
                "Failed to fetch models: {}",
                response.status()
            )))
        }
    }

    /// List loaded models (alias for fetch_models for API compatibility)
    pub async fn list_loaded_models(&self) -> Result<Vec<Model>> {
        self.fetch_models().await
    }

    /// Load a model by sending a minimal request
    pub async fn load_model(&self, model: &str) -> Result<()> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let request_body = serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": ""}],
            "max_tokens": 1
        });

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await?;

        if response.status().is_success() {
            tracing::info!("Successfully loaded model '{model}'");
            Ok(())
        } else {
            Err(LMStudioError::ServerError(format!(
                "Failed to load model: {}",
                response.status()
            )))
        }
    }

    /// Send a chat completion request
    pub async fn chat_completion(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(request)
            .send()
            .await?;

        if response.status().is_success() {
            let chat_response: ChatResponse = response.json().await?;
            Ok(chat_response)
        } else {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            Err(LMStudioError::ServerError(format!(
                "Chat completion failed: {} - {}",
                status, body
            )))
        }
    }

    /// Find the lms CLI tool, checking fallback paths if not in PATH
    fn find_lms() -> Result<String> {
        Self::find_lms_with_home_dir(None)
    }

    fn find_lms_with_home_dir(home_dir: Option<&str>) -> Result<String> {
        // First try 'lms' in PATH
        if which::which("lms").is_ok() {
            return Ok("lms".to_string());
        }

        // Platform-specific fallback paths
        let home = match home_dir {
            Some(dir) => dir.to_string(),
            None => {
                #[cfg(unix)]
                {
                    std::env::var("HOME").unwrap_or_default()
                }
                #[cfg(windows)]
                {
                    std::env::var("USERPROFILE").unwrap_or_default()
                }
            }
        };

        #[cfg(unix)]
        let fallback_path = format!("{home}/.lmstudio/bin/lms");

        #[cfg(windows)]
        let fallback_path = format!("{home}/.lmstudio/bin/lms.exe");

        if Path::new(&fallback_path).exists() {
            Ok(fallback_path)
        } else {
            Err(LMStudioError::LMStudioNotFound)
        }
    }

    /// Download a model using the lms CLI tool
    pub async fn download_model(&self, model: &str) -> Result<()> {
        let lms = Self::find_lms()?;
        eprintln!("Downloading model: {model}");

        let status = std::process::Command::new(&lms)
            .args(["get", "--yes", model])
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| {
                LMStudioError::DownloadError(format!(
                    "Failed to execute '{lms} get --yes {model}': {e}"
                ))
            })?;

        if !status.success() {
            return Err(LMStudioError::DownloadError(format!(
                "Model download failed with exit code: {}",
                status.code().unwrap_or(-1)
            )));
        }

        tracing::info!("Successfully downloaded model '{model}'");
        Ok(())
    }

    /// Discover models in the local LM Studio models directory
    pub async fn discover_models(&self) -> Result<Vec<String>> {
        let home = {
            #[cfg(unix)]
            {
                std::env::var("HOME").unwrap_or_default()
            }
            #[cfg(windows)]
            {
                std::env::var("USERPROFILE").unwrap_or_default()
            }
        };

        let models_dir = Path::new(&home).join(".lmstudio").join("models");

        if !models_dir.exists() {
            return Ok(Vec::new());
        }

        let mut models = Vec::new();
        Self::scan_models_dir(&models_dir, &mut models)?;
        Ok(models)
    }

    /// Recursively scan a directory for model files
    fn scan_models_dir(dir: &Path, models: &mut Vec<String>) -> Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                Self::scan_models_dir(&path, models)?;
            } else if let Some(ext) = path.extension() {
                // Common model file extensions
                if (ext == "gguf" || ext == "bin" || ext == "safetensors")
                    && let Some(name) = path.file_stem()
                {
                    models.push(name.to_string_lossy().to_string());
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_models_happy_path() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/models"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_raw(
                    serde_json::json!({
                        "object": "list",
                        "data": [
                            {"id": "openai/gpt-oss-20b", "object": "model", "owned_by": "openai", "created": 0}
                        ]
                    })
                    .to_string(),
                    "application/json",
                ),
            )
            .mount(&server)
            .await;

        let client = LMStudioClient::from_host_root(server.uri());
        let models = client.fetch_models().await.expect("fetch models");
        assert!(models.iter().any(|m| m.id == "openai/gpt-oss-20b"));
    }

    #[tokio::test]
    async fn test_fetch_models_server_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/models"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = LMStudioClient::from_host_root(server.uri());
        let result = client.fetch_models().await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to fetch models")
        );
    }

    #[tokio::test]
    async fn test_chat_completion() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_raw(
                    serde_json::json!({
                        "id": "chatcmpl-123",
                        "object": "chat.completion",
                        "created": 1677652288,
                        "model": "test-model",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": "Hello! How can I help you?"
                            },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 9,
                            "completion_tokens": 12,
                            "total_tokens": 21
                        }
                    })
                    .to_string(),
                    "application/json",
                ),
            )
            .mount(&server)
            .await;

        let client = LMStudioClient::from_host_root(server.uri());
        let request = ChatRequest::new(
            "test-model",
            vec![crate::models::ChatMessage::user("Hello!")],
        );
        let response = client
            .chat_completion(&request)
            .await
            .expect("chat completion");
        assert_eq!(response.content(), Some("Hello! How can I help you?"));
    }

    #[test]
    fn test_find_lms() {
        let result = LMStudioClient::find_lms();

        // Either found or not found error - both are valid
        match result {
            Ok(_) => {}
            Err(e) => {
                assert!(matches!(e, LMStudioError::LMStudioNotFound));
            }
        }
    }

    #[test]
    fn test_from_host_root() {
        let client = LMStudioClient::from_host_root("http://localhost:1234");
        assert_eq!(client.base_url(), "http://localhost:1234");

        let client = LMStudioClient::from_host_root("https://example.com:8080/api");
        assert_eq!(client.base_url(), "https://example.com:8080/api");
    }
}
