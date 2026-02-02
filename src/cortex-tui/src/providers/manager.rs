//! Cortex Provider Manager
//!
//! Manages the connection to Cortex backend and handles completion requests.
//! Requires authentication - all requests go through the Cortex API.

use anyhow::{Result, anyhow};
use cortex_engine::client::{
    CompletionRequest, CompletionResponse, CortexClient, CortexModel, Message, ModelCapabilities,
    ModelClient, ResponseStream, ToolDefinition, create_client,
};

use super::config::CortexConfig;
use super::models::{ModelInfo, get_models_for_provider, get_popular_models};

// ============================================================
// PROVIDER MANAGER
// ============================================================

/// Manages the Cortex backend connection and handles completion requests.
///
/// Requires authentication via OAuth. Use `cortex login` to authenticate.
pub struct ProviderManager {
    /// Configuration.
    config: CortexConfig,
    /// Current provider ID (always "cortex").
    current_provider: String,
    /// Current model ID.
    current_model: String,
    /// OAuth access token.
    auth_token: Option<String>,
    /// Active model client.
    client: Option<Box<dyn ModelClient>>,
    /// Cached models from backend.
    cached_models: Option<Vec<CortexModel>>,
}

impl ProviderManager {
    /// Creates a new ProviderManager with the given configuration.
    pub fn new(config: CortexConfig) -> Self {
        let (current_provider, current_model) =
            if let Some((provider, model)) = config.get_last_model() {
                (provider.to_string(), model.to_string())
            } else {
                (
                    config.default_provider.clone(),
                    config.default_model.clone(),
                )
            };

        Self {
            config,
            current_provider,
            current_model,
            auth_token: None,
            client: None,
            cached_models: None,
        }
    }

    /// Creates a new ProviderManager with authentication token.
    pub fn with_auth(config: CortexConfig, auth_token: String) -> Self {
        let mut manager = Self::new(config);
        manager.auth_token = Some(auth_token);
        manager
    }

    /// Loads configuration from file and creates a new ProviderManager.
    pub fn load() -> Result<Self> {
        let config = CortexConfig::load()?;
        Ok(Self::new(config))
    }

    /// Loads configuration and sets auth token.
    pub fn load_with_auth(auth_token: String) -> Result<Self> {
        let config = CortexConfig::load()?;
        Ok(Self::with_auth(config, auth_token))
    }

    /// Sets the authentication token.
    pub fn set_auth_token(&mut self, token: String) {
        self.auth_token = Some(token);
        self.client = None;
    }

    /// Checks if authenticated.
    pub fn is_authenticated(&self) -> bool {
        self.auth_token.is_some()
    }

    /// Gets the current configuration.
    pub fn config(&self) -> &CortexConfig {
        &self.config
    }

    /// Gets a mutable reference to the configuration.
    pub fn config_mut(&mut self) -> &mut CortexConfig {
        &mut self.config
    }

    /// Gets the current provider ID.
    pub fn current_provider(&self) -> &str {
        &self.current_provider
    }

    /// Gets the current model ID.
    pub fn current_model(&self) -> &str {
        &self.current_model
    }

    /// Gets the display name of the current provider.
    pub fn current_provider_name(&self) -> &str {
        "Cortex"
    }

    /// Gets the display name of the current model.
    pub fn current_model_name(&self) -> String {
        let models = get_popular_models();
        models
            .iter()
            .find(|m| m.id == self.current_model)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| {
                self.current_model
                    .split('/')
                    .next_back()
                    .unwrap_or(&self.current_model)
                    .to_string()
            })
    }

    /// Gets the API URL.
    pub fn api_url(&self) -> &str {
        &self.config.api_url
    }

    /// Takes the client out of the manager.
    pub fn take_client(&mut self) -> Option<Box<dyn ModelClient>> {
        self.client.take()
    }

    /// Restores a client to the manager.
    pub fn restore_client(&mut self, client: Box<dyn ModelClient>) {
        self.client = Some(client);
    }

    /// Checks if a client is currently available.
    pub fn has_client(&self) -> bool {
        self.client.is_some()
    }

    /// Lists available providers (for compatibility - always Cortex).
    pub fn available_providers(&self) -> Vec<(&'static str, &'static str)> {
        vec![("cortex", "Cortex")]
    }

    /// Lists models for the current provider.
    /// Returns cached models from API if available, otherwise falls back to hardcoded list.
    pub fn available_models(&self) -> Vec<ModelInfo> {
        // Use cached models from API if available
        if let Some(ref cached) = self.cached_models {
            return cached
                .iter()
                .map(|m| {
                    let provider = m
                        .owned_by
                        .split('/')
                        .nth(1)
                        .unwrap_or(&m.owned_by)
                        .to_string();
                    ModelInfo {
                        id: m.id.clone(),
                        name: m.display_name.clone(),
                        provider,
                        context_length: m.context_length as u32,
                        context_window: m.context_length as u32,
                        max_output_tokens: m.max_output_tokens as u32,
                        description: String::new(),
                        vision: m
                            .capabilities
                            .get("vision")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        tools: m
                            .capabilities
                            .get("tools")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true),
                        credit_multiplier_input: Some(m.credit_multiplier_input.clone()),
                        credit_multiplier_output: Some(m.credit_multiplier_output.clone()),
                        price_version: Some(m.price_version),
                    }
                })
                .collect();
        }
        // Fallback to hardcoded list
        get_models_for_provider(&self.current_provider)
    }

    /// Lists popular models.
    pub fn popular_models(&self) -> Vec<ModelInfo> {
        get_popular_models()
    }

    /// Checks if the provider is available (authenticated).
    pub fn is_available(&self) -> bool {
        self.is_authenticated()
            || std::env::var("CORTEX_AUTH_TOKEN").is_ok()
            || cortex_login::has_valid_auth()
    }

    /// Sets the current provider (for compatibility - always sets to "cortex").
    pub fn set_provider(&mut self, _provider: &str) -> Result<()> {
        self.current_provider = "cortex".to_string();
        self.client = None;
        Ok(())
    }

    /// Sets the current model.
    pub fn set_model(&mut self, model: &str) -> Result<()> {
        let resolved = self.config.resolve_alias(model);
        self.current_model = resolved;
        self.client = None;
        Ok(())
    }

    /// Fetches available models from the Cortex backend.
    pub async fn fetch_models(&mut self) -> Result<Vec<CortexModel>> {
        let api_url = &self.config.api_url;
        tracing::debug!("Fetching models from: {}/v1/models", api_url);

        // Try to get token, but don't fail if not available (models endpoint is public)
        let token_result = self.get_token();
        let has_token = token_result.is_ok();

        tracing::debug!(
            "Auth token available: {}, from: {}",
            has_token,
            if self.auth_token.is_some() {
                "manager"
            } else if std::env::var("CORTEX_AUTH_TOKEN").is_ok() {
                "env"
            } else {
                "keyring"
            }
        );

        let cortex_client = CortexClient::new("".to_string(), Some(api_url.clone()));
        let cortex_client = if let Ok(token) = token_result {
            cortex_client.with_auth_token(token)
        } else {
            tracing::debug!("No auth token, calling models endpoint without auth");
            cortex_client
        };

        let models = cortex_client.list_models().await.map_err(|e| {
            tracing::error!("Failed to fetch models: {}", e);
            anyhow!("{}", e)
        })?;

        tracing::info!("Successfully fetched {} models", models.len());
        self.cached_models = Some(models.clone());
        Ok(models)
    }

    /// Validates the current session by making an authenticated API call.
    ///
    /// Returns:
    /// - `Ok(true)` if the session is valid
    /// - `Ok(false)` if authentication failed (401/403)
    /// - `Err` for network or other errors
    pub async fn validate_session(&self) -> Result<bool> {
        let api_url = &self.config.api_url;
        let token = match self.get_token() {
            Ok(t) => t,
            Err(_) => {
                tracing::debug!("No auth token available for session validation");
                return Ok(false);
            }
        };

        tracing::debug!("Validating session with API at {}", api_url);

        // Make a lightweight authenticated request to check if the token is valid
        // We use the /v1/models endpoint with auth header - if token is invalid, it will return 401
        let client = match cortex_engine::create_client_builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to create HTTP client for session validation: {}", e);
                return Err(anyhow!("Failed to create HTTP client: {}", e));
            }
        };

        let resp = match client
            .get(format!("{}/v1/models", api_url))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Session validation request failed: {}", e);
                return Err(anyhow!("Network error during session validation: {}", e));
            }
        };

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            tracing::info!("Session validation failed: server returned {}", status);
            return Ok(false);
        }

        if status.is_success() {
            tracing::debug!("Session validation succeeded");
            return Ok(true);
        }

        // For other errors, log but don't fail - the session might still be valid
        tracing::debug!(
            "Session validation returned status {}, assuming valid",
            status
        );
        Ok(true)
    }

    /// Gets cached models or fetches them.
    pub async fn get_models(&mut self) -> Result<Vec<CortexModel>> {
        if let Some(ref models) = self.cached_models {
            return Ok(models.clone());
        }
        self.fetch_models().await
    }

    /// Sets cached models from an external source (e.g., background fetch).
    ///
    /// This is useful for setting models that were fetched in a background task
    /// to avoid blocking the main thread during startup.
    pub fn set_cached_models(&mut self, models: Vec<CortexModel>) {
        tracing::debug!(
            "Setting {} cached models from external source",
            models.len()
        );
        self.cached_models = Some(models);
    }

    /// Checks if models are cached.
    pub fn has_cached_models(&self) -> bool {
        self.cached_models.is_some()
    }

    /// Gets the auth token using centralized auth module.
    fn get_token(&self) -> Result<String> {
        cortex_engine::auth_token::get_auth_token(self.auth_token.as_deref())
            .map_err(|e| anyhow!("{}", e))
    }

    /// Ensures a client is created for the current model.
    pub fn ensure_client(&mut self) -> Result<()> {
        if self.client.is_some() {
            return Ok(());
        }
        let token = self.get_token()?;
        self.client = Some(create_client("cortex", &self.current_model, &token, None)?);
        Ok(())
    }

    /// Gets model capabilities if client is available.
    pub fn capabilities(&self) -> Option<&ModelCapabilities> {
        self.client.as_ref().map(|c| c.capabilities())
    }

    /// Sends a completion request and returns a stream of responses.
    pub async fn complete(&mut self, messages: Vec<Message>) -> Result<ResponseStream> {
        self.ensure_client()?;
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("No client available"))?;
        let request = CompletionRequest {
            messages,
            model: self.current_model.clone(),
            max_tokens: Some(self.config.max_tokens),
            temperature: Some(self.config.temperature),
            seed: None,
            tools: vec![],
            stream: true,
        };
        client.complete(request).await.map_err(|e| anyhow!("{}", e))
    }

    /// Sends a completion request with tools.
    pub async fn complete_with_tools(
        &mut self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> Result<ResponseStream> {
        self.ensure_client()?;
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("No client available"))?;
        let request = CompletionRequest {
            messages,
            model: self.current_model.clone(),
            max_tokens: Some(self.config.max_tokens),
            temperature: Some(self.config.temperature),
            seed: None,
            tools,
            stream: true,
        };
        client.complete(request).await.map_err(|e| anyhow!("{}", e))
    }

    /// Sends a completion request and waits for the full response.
    pub async fn complete_sync(&mut self, messages: Vec<Message>) -> Result<CompletionResponse> {
        self.ensure_client()?;
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("No client available"))?;
        let request = CompletionRequest {
            messages,
            model: self.current_model.clone(),
            max_tokens: Some(self.config.max_tokens),
            temperature: Some(self.config.temperature),
            seed: None,
            tools: vec![],
            stream: false,
        };
        client
            .complete_sync(request)
            .await
            .map_err(|e| anyhow!("{}", e))
    }

    /// Saves the current configuration to file.
    pub fn save_config(&self) -> Result<()> {
        self.config.save()
    }

    /// Reloads configuration from file.
    ///
    /// This method re-loads the config from disk and also re-evaluates any environment
    /// variable substitutions (e.g., `{env:VAR_NAME}` placeholders) to pick up current values.
    ///
    /// Issue #2320: Environment variables are now re-evaluated during config reload,
    /// ensuring that changes to env vars (like API keys) take effect without a full restart.
    pub fn reload_config(&mut self) -> Result<()> {
        // Re-load config from file (this will pick up any file changes)
        self.config = CortexConfig::load()?;

        // Issue #2320: Re-evaluate ALL environment-dependent config values
        // This ensures env var changes take effect on config reload

        // Re-check CORTEX_API_URL in case it was set/changed after initial load
        if let Ok(url) = std::env::var("CORTEX_API_URL")
            && !url.is_empty()
        {
            self.config.api_url = url;
        }

        // Re-check CORTEX_AUTH_TOKEN - force token refresh by clearing cached auth
        // The next API call will re-read the token from environment or keyring
        self.auth_token = None;

        // If CORTEX_AUTH_TOKEN is now set, update our cached token
        if let Ok(token) = std::env::var("CORTEX_AUTH_TOKEN")
            && !token.is_empty()
        {
            self.auth_token = Some(token);
        }

        // Re-check CORTEX_DEFAULT_MODEL if set
        if let Ok(model) = std::env::var("CORTEX_DEFAULT_MODEL")
            && !model.is_empty()
        {
            self.config.default_model = model;
        }

        // Re-check CORTEX_MAX_TOKENS if set
        if let Ok(max_tokens) = std::env::var("CORTEX_MAX_TOKENS")
            && let Ok(tokens) = max_tokens.parse::<u32>()
        {
            self.config.max_tokens = tokens;
        }

        // Re-check CORTEX_TEMPERATURE if set
        if let Ok(temp) = std::env::var("CORTEX_TEMPERATURE")
            && let Ok(temperature) = temp.parse::<f32>()
        {
            self.config.temperature = temperature;
        }

        // Reset client to pick up new config values
        // This forces a new client to be created with updated settings
        self.client = None;

        // Clear cached models since API credentials may have changed
        self.cached_models = None;

        tracing::info!("Configuration reloaded with fresh environment variable values");
        Ok(())
    }

    /// Formats provider information for display.
    pub fn format_provider_info(&self) -> String {
        format!("Cortex / {}", self.format_short_model())
    }

    /// Formats a short model identifier for status bar.
    pub fn format_short_model(&self) -> String {
        let model_name = self
            .current_model
            .split('/')
            .next_back()
            .unwrap_or(&self.current_model);
        match model_name {
            n if n.contains("claude-opus-4") => "Opus 4".to_string(),
            n if n.contains("claude-sonnet-4") => "Sonnet 4".to_string(),
            n if n.contains("claude-3.5-sonnet") => "Sonnet 3.5".to_string(),
            n if n.contains("claude-3.5-haiku") => "Haiku 3.5".to_string(),
            n if n.contains("gpt-4o-mini") => "GPT-4o Mini".to_string(),
            n if n.contains("gpt-4o") => "GPT-4o".to_string(),
            n if n.contains("o3-mini") => "O3 Mini".to_string(),
            n if n.contains("o3") => "O3".to_string(),
            n if n.contains("o1-mini") => "O1 Mini".to_string(),
            n if n.contains("o1") => "O1".to_string(),
            n if n.contains("gemini-2.5-pro") => "Gemini 2.5".to_string(),
            n if n.contains("gemini-2.0-flash") => "Gemini 2.0".to_string(),
            n if n.contains("deepseek-r1") => "R1".to_string(),
            n if n.contains("deepseek-chat") => "DeepSeek V3".to_string(),
            n if n.contains("llama-3.3") => "Llama 3.3".to_string(),
            n if n.contains("llama-3.1") => "Llama 3.1".to_string(),
            _ => {
                if model_name.len() > 15 {
                    format!("{}...", &model_name[..12])
                } else {
                    model_name.to_string()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_manager_new() {
        let config = CortexConfig::default();
        let manager = ProviderManager::new(config);
        assert!(!manager.is_authenticated());
        assert_eq!(manager.current_provider(), "cortex");
    }

    #[test]
    fn test_with_auth() {
        let config = CortexConfig::default();
        let manager = ProviderManager::with_auth(config, "test-token".to_string());
        assert!(manager.is_authenticated());
    }

    #[test]
    fn test_format_short_model() {
        let config = CortexConfig::default();
        let mut manager = ProviderManager::new(config);
        manager.current_model = "anthropic/claude-opus-4-20250514".to_string();
        assert_eq!(manager.format_short_model(), "Opus 4");
    }
}
