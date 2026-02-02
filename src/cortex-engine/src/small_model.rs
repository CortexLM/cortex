//! Small Model Fallback System
//!
//! Provides automatic selection and usage of lightweight models for
//! specific tasks like title generation, summarization, and classification.
//!
//! The system prioritizes models by:
//! 1. User-configured small model preference
//! 2. Available providers (checking API keys)
//! 3. Built-in priority order (fastest/cheapest first)

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::client::{CompletionRequest, CompletionResponse, Message, ModelClient};

// ============================================================================
// Constants - Small Models by Provider
// ============================================================================

/// Small/lightweight models by provider, ordered by priority (speed/cost efficiency).
///
/// These models are optimized for:
/// - Fast response times
/// - Low token costs  
/// - Simple tasks (titles, summaries, classifications)
pub const SMALL_MODELS: &[(&str, &str)] = &[
    ("groq", "llama-3.1-8b-instant"),         // Ultra-fast, free tier
    ("openai", "gpt-4o-mini"),                // Fast, cheap, reliable
    ("anthropic", "claude-3-5-haiku-latest"), // Fast Anthropic option
    ("google", "gemini-2.0-flash-lite"),      // Fast Google option
    ("mistral", "mistral-small-latest"),      // Fast Mistral option
    ("xai", "grok-2-mini"),                   // xAI smaller model
];

/// Environment variable names for provider API keys.
pub const PROVIDER_ENV_VARS: &[(&str, &str)] = &[("cortex", "CORTEX_AUTH_TOKEN")];

// ============================================================================
// Small Model Tasks
// ============================================================================

/// Tasks that are suitable for small/lightweight models.
///
/// Each task type defines appropriate parameters like max tokens and temperature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmallModelTask {
    /// Generate a concise title for a session (3-7 words).
    GenerateTitle,
    /// Generate a summary for conversation compaction.
    GenerateSummary,
    /// Extract keywords from text.
    ExtractKeywords,
    /// Classify user intent or message type.
    ClassifyIntent,
    /// Simple question answering.
    SimpleQA,
    /// Text reformatting or cleanup.
    FormatText,
    /// Generate commit message.
    CommitMessage,
    /// Extract entities from text.
    ExtractEntities,
}

impl SmallModelTask {
    /// Get the maximum tokens appropriate for this task.
    pub fn max_tokens(&self) -> u32 {
        match self {
            Self::GenerateTitle => 50,
            Self::GenerateSummary => 500,
            Self::ExtractKeywords => 100,
            Self::ClassifyIntent => 50,
            Self::SimpleQA => 200,
            Self::FormatText => 500,
            Self::CommitMessage => 100,
            Self::ExtractEntities => 200,
        }
    }

    /// Get the temperature appropriate for this task.
    ///
    /// Lower temperature = more deterministic output.
    /// Higher temperature = more creative output.
    pub fn temperature(&self) -> f32 {
        match self {
            Self::GenerateTitle => 0.7,   // Some creativity for titles
            Self::GenerateSummary => 0.3, // More factual
            Self::ExtractKeywords => 0.2, // Very deterministic
            Self::ClassifyIntent => 0.1,  // Very deterministic
            Self::SimpleQA => 0.5,        // Balanced
            Self::FormatText => 0.2,      // Deterministic
            Self::CommitMessage => 0.5,   // Some creativity
            Self::ExtractEntities => 0.1, // Very deterministic
        }
    }

    /// Get a description of this task type.
    pub fn description(&self) -> &'static str {
        match self {
            Self::GenerateTitle => "Generate concise session title",
            Self::GenerateSummary => "Generate conversation summary",
            Self::ExtractKeywords => "Extract keywords from text",
            Self::ClassifyIntent => "Classify user intent",
            Self::SimpleQA => "Answer simple questions",
            Self::FormatText => "Format or clean up text",
            Self::CommitMessage => "Generate commit message",
            Self::ExtractEntities => "Extract named entities",
        }
    }

    /// Check if this task benefits from a system prompt.
    pub fn needs_system_prompt(&self) -> bool {
        matches!(
            self,
            Self::GenerateTitle
                | Self::GenerateSummary
                | Self::ClassifyIntent
                | Self::CommitMessage
        )
    }

    /// Get the default system prompt for this task.
    pub fn default_system_prompt(&self) -> Option<&'static str> {
        match self {
            Self::GenerateTitle => Some(
                "Generate a concise, descriptive title (3-7 words) based on the content. \
                 Output only the title text, no quotes or formatting.",
            ),
            Self::GenerateSummary => Some(
                "Summarize the key points concisely. Focus on:\n\
                 1. Main goals and requests\n\
                 2. Actions taken\n\
                 3. Important decisions and outcomes\n\
                 Keep it under 200 words.",
            ),
            Self::ClassifyIntent => Some(
                "Classify the user's intent into one of these categories:\n\
                 - question: asking for information\n\
                 - task: requesting an action\n\
                 - clarification: asking for clarification\n\
                 - feedback: providing feedback\n\
                 - other: none of the above\n\
                 Output only the category name.",
            ),
            Self::CommitMessage => Some(
                "Generate a concise git commit message following conventional commits format.\n\
                 Format: type(scope): description\n\
                 Types: feat, fix, docs, style, refactor, test, chore\n\
                 Keep it under 72 characters.",
            ),
            Self::ExtractKeywords => Some(
                "Extract the most important keywords from the text.\n\
                 Output as a comma-separated list, max 10 keywords.",
            ),
            Self::ExtractEntities => Some(
                "Extract named entities (people, places, organizations, etc.) from the text.\n\
                 Output as JSON: {\"type\": \"entity_name\", ...}",
            ),
            _ => None,
        }
    }
}

impl std::fmt::Display for SmallModelTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.description())
    }
}

// ============================================================================
// Small Model Selector
// ============================================================================

/// Selector for choosing the best available small model.
///
/// Priorities:
/// 1. Explicitly configured small model
/// 2. First available model from SMALL_MODELS list
#[derive(Debug, Clone)]
pub struct SmallModelSelector {
    /// Explicitly configured small model (provider/model format).
    configured_model: Option<String>,
    /// Cached list of available providers.
    available_providers: Vec<String>,
    /// Custom model mappings per provider.
    custom_models: HashMap<String, String>,
}

impl Default for SmallModelSelector {
    fn default() -> Self {
        Self::new()
    }
}

impl SmallModelSelector {
    /// Create a new selector by detecting available providers.
    pub fn new() -> Self {
        Self {
            configured_model: None,
            available_providers: detect_available_providers(),
            custom_models: HashMap::new(),
        }
    }

    /// Create a selector with a specific configured model.
    pub fn with_configured_model(model: impl Into<String>) -> Self {
        let mut selector = Self::new();
        selector.configured_model = Some(model.into());
        selector
    }

    /// Create a selector with a specific list of available providers.
    pub fn with_providers(providers: Vec<String>) -> Self {
        Self {
            configured_model: None,
            available_providers: providers,
            custom_models: HashMap::new(),
        }
    }

    /// Set a custom model for a specific provider.
    pub fn set_custom_model(&mut self, provider: &str, model: &str) {
        self.custom_models
            .insert(provider.to_string(), model.to_string());
    }

    /// Select the best available small model.
    ///
    /// Returns `(provider, model)` tuple if a model is available.
    pub fn select(&self) -> Option<(String, String)> {
        // 1. If explicitly configured, parse and return
        if let Some(ref configured) = self.configured_model {
            return self.parse_model_string(configured);
        }

        // 2. Find first available model from priority list
        for (provider, model) in SMALL_MODELS {
            if self.is_provider_available(provider) {
                // Check for custom model override
                let model = self
                    .custom_models
                    .get(*provider)
                    .map(|s| s.as_str())
                    .unwrap_or(model);
                return Some((provider.to_string(), model.to_string()));
            }
        }

        None
    }

    /// Get the small model for a specific provider.
    pub fn for_provider(&self, provider: &str) -> Option<String> {
        // Check custom mapping first
        if let Some(model) = self.custom_models.get(provider) {
            return Some(model.clone());
        }

        // Check default mappings
        SMALL_MODELS
            .iter()
            .find(|(p, _)| *p == provider)
            .map(|(_, m)| m.to_string())
    }

    /// Check if a provider is available (has API key configured).
    pub fn is_provider_available(&self, provider: &str) -> bool {
        self.available_providers.contains(&provider.to_string())
    }

    /// Get the list of available providers.
    pub fn available_providers(&self) -> &[String] {
        &self.available_providers
    }

    /// Parse a "provider/model" string into components.
    fn parse_model_string(&self, model_str: &str) -> Option<(String, String)> {
        if let Some((provider, model)) = model_str.split_once('/') {
            Some((provider.to_string(), model.to_string()))
        } else {
            // If no provider specified, try to find provider for this model
            for (provider, default_model) in SMALL_MODELS {
                if *default_model == model_str {
                    return Some((provider.to_string(), model_str.to_string()));
                }
            }
            None
        }
    }

    /// Refresh the list of available providers.
    pub fn refresh(&mut self) {
        self.available_providers = detect_available_providers();
    }
}

// ============================================================================
// Provider Detection
// ============================================================================

/// Detect which providers have API keys configured.
pub fn detect_available_providers() -> Vec<String> {
    let mut providers = Vec::new();

    for (provider, env_var) in PROVIDER_ENV_VARS {
        if std::env::var(env_var).is_ok() {
            providers.push(provider.to_string());
        }
    }

    // Also check for backend availability
    if std::env::var("CORTEX_BACKEND_URL").is_ok() {
        providers.push("backend".to_string());
    }

    providers
}

/// Check if a specific provider is available.
pub fn is_provider_available(provider: &str) -> bool {
    PROVIDER_ENV_VARS
        .iter()
        .find(|(p, _)| *p == provider)
        .map(|(_, env_var)| std::env::var(env_var).is_ok())
        .unwrap_or(false)
}

/// Get the API key for a provider.
pub fn get_provider_api_key(provider: &str) -> Option<String> {
    PROVIDER_ENV_VARS
        .iter()
        .find(|(p, _)| *p == provider)
        .and_then(|(_, env_var)| std::env::var(env_var).ok())
}

// ============================================================================
// Small Model Client Helper
// ============================================================================

/// Configuration for small model calls.
#[derive(Debug, Clone)]
pub struct SmallModelConfig {
    /// The task being performed.
    pub task: SmallModelTask,
    /// Optional custom system prompt (overrides task default).
    pub system_prompt: Option<String>,
    /// Optional temperature override.
    pub temperature: Option<f32>,
    /// Optional max_tokens override.
    pub max_tokens: Option<u32>,
}

impl SmallModelConfig {
    /// Create a new config for a task.
    pub fn new(task: SmallModelTask) -> Self {
        Self {
            task,
            system_prompt: None,
            temperature: None,
            max_tokens: None,
        }
    }

    /// Set a custom system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set a custom temperature.
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set custom max tokens.
    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Get the effective temperature.
    pub fn effective_temperature(&self) -> f32 {
        self.temperature.unwrap_or_else(|| self.task.temperature())
    }

    /// Get the effective max tokens.
    pub fn effective_max_tokens(&self) -> u32 {
        self.max_tokens.unwrap_or_else(|| self.task.max_tokens())
    }

    /// Get the effective system prompt.
    pub fn effective_system_prompt(&self) -> Option<&str> {
        self.system_prompt
            .as_deref()
            .or_else(|| self.task.default_system_prompt())
    }
}

/// Call a small model for a specific task.
///
/// This is the main entry point for using small models.
///
/// # Arguments
/// * `config` - Configuration for the call
/// * `prompt` - The user prompt/input
/// * `client_factory` - Factory function to create a ModelClient
///
/// # Example
/// ```ignore
/// let result = call_small_model(
///     SmallModelConfig::new(SmallModelTask::GenerateTitle),
///     "User asked about implementing a caching system",
///     |provider, model| create_client(provider, model, &api_key, None),
/// ).await?;
/// ```
pub async fn call_small_model<F>(
    config: SmallModelConfig,
    prompt: &str,
    client_factory: F,
) -> Result<String>
where
    F: Fn(&str, &str) -> Result<Box<dyn ModelClient>>,
{
    let selector = SmallModelSelector::new();
    let (provider, model) = selector
        .select()
        .ok_or_else(|| anyhow!("No small model available. Please configure an API key."))?;

    let client = client_factory(&provider, &model)?;

    call_with_client(config, prompt, client.as_ref()).await
}

/// Call a small model with an existing client.
pub async fn call_with_client(
    config: SmallModelConfig,
    prompt: &str,
    client: &dyn ModelClient,
) -> Result<String> {
    let mut messages = Vec::new();

    // Add system prompt if appropriate
    if let Some(system_prompt) = config.effective_system_prompt() {
        messages.push(Message::system(system_prompt));
    }

    // Add user message
    messages.push(Message::user(prompt));

    let request = CompletionRequest {
        messages,
        model: client.model().to_string(),
        max_tokens: Some(config.effective_max_tokens()),
        temperature: Some(config.effective_temperature()),
        seed: None,
        tools: vec![],
        stream: false,
    };

    let response = client.complete_sync(request).await?;

    extract_response_text(response)
}

/// Extract text content from a completion response.
fn extract_response_text(response: CompletionResponse) -> Result<String> {
    response
        .message
        .and_then(|m| m.content.as_text().map(|s| s.to_string()))
        .ok_or_else(|| anyhow!("No text content in response"))
}

// ============================================================================
// Convenience Functions for Common Tasks
// ============================================================================

/// Generate a title for a conversation.
pub async fn generate_title<F>(content: &str, client_factory: F) -> Result<String>
where
    F: Fn(&str, &str) -> Result<Box<dyn ModelClient>>,
{
    call_small_model(
        SmallModelConfig::new(SmallModelTask::GenerateTitle),
        content,
        client_factory,
    )
    .await
}

/// Generate a summary of conversation content.
pub async fn generate_summary<F>(content: &str, client_factory: F) -> Result<String>
where
    F: Fn(&str, &str) -> Result<Box<dyn ModelClient>>,
{
    call_small_model(
        SmallModelConfig::new(SmallModelTask::GenerateSummary),
        content,
        client_factory,
    )
    .await
}

/// Classify user intent.
pub async fn classify_intent<F>(message: &str, client_factory: F) -> Result<String>
where
    F: Fn(&str, &str) -> Result<Box<dyn ModelClient>>,
{
    call_small_model(
        SmallModelConfig::new(SmallModelTask::ClassifyIntent),
        message,
        client_factory,
    )
    .await
}

/// Extract keywords from text.
pub async fn extract_keywords<F>(text: &str, client_factory: F) -> Result<Vec<String>>
where
    F: Fn(&str, &str) -> Result<Box<dyn ModelClient>>,
{
    let result = call_small_model(
        SmallModelConfig::new(SmallModelTask::ExtractKeywords),
        text,
        client_factory,
    )
    .await?;

    // Parse comma-separated keywords
    Ok(result
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

/// Generate a commit message.
pub async fn generate_commit_message<F>(diff: &str, client_factory: F) -> Result<String>
where
    F: Fn(&str, &str) -> Result<Box<dyn ModelClient>>,
{
    call_small_model(
        SmallModelConfig::new(SmallModelTask::CommitMessage),
        diff,
        client_factory,
    )
    .await
}

// ============================================================================
// Global Selector (Cached)
// ============================================================================

static GLOBAL_SELECTOR: OnceLock<SmallModelSelector> = OnceLock::new();

/// Get or initialize the global small model selector.
pub fn global_selector() -> &'static SmallModelSelector {
    GLOBAL_SELECTOR.get_or_init(SmallModelSelector::new)
}

/// Get the globally selected small model.
pub fn get_small_model() -> Option<(String, String)> {
    global_selector().select()
}

/// Check if any small model is available.
pub fn has_small_model() -> bool {
    global_selector().select().is_some()
}

// ============================================================================
// Model Info
// ============================================================================

/// Information about a small model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmallModelInfo {
    pub provider: String,
    pub model: String,
    pub description: String,
    pub max_context: u32,
    pub cost_per_million_tokens: Option<f64>,
}

/// Get information about all small models.
pub fn list_small_models() -> Vec<SmallModelInfo> {
    SMALL_MODELS
        .iter()
        .map(|(provider, model)| SmallModelInfo {
            provider: provider.to_string(),
            model: model.to_string(),
            description: get_model_description(provider, model),
            max_context: get_model_context(provider, model),
            cost_per_million_tokens: get_model_cost(provider, model),
        })
        .collect()
}

fn get_model_description(provider: &str, model: &str) -> String {
    match (provider, model) {
        ("groq", "llama-3.1-8b-instant") => {
            "Ultra-fast inference, great for simple tasks".to_string()
        }
        ("openai", "gpt-4o-mini") => "Fast, cheap, reliable OpenAI model".to_string(),
        ("anthropic", "claude-3-5-haiku-latest") => {
            "Fast Anthropic model for quick tasks".to_string()
        }
        ("google", "gemini-2.0-flash-lite") => "Lightweight Google model".to_string(),
        ("mistral", "mistral-small-latest") => "Fast Mistral model".to_string(),
        ("xai", "grok-2-mini") => "xAI smaller model".to_string(),
        _ => format!("{}/{}", provider, model),
    }
}

fn get_model_context(provider: &str, model: &str) -> u32 {
    match (provider, model) {
        ("groq", "llama-3.1-8b-instant") => 131_072,
        ("openai", "gpt-4o-mini") => 128_000,
        ("anthropic", "claude-3-5-haiku-latest") => 200_000,
        ("google", "gemini-2.0-flash-lite") => 1_000_000,
        ("mistral", "mistral-small-latest") => 32_000,
        ("xai", "grok-2-mini") => 131_072,
        _ => 32_000, // Conservative default
    }
}

fn get_model_cost(provider: &str, model: &str) -> Option<f64> {
    // Cost per million input tokens (approximate)
    match (provider, model) {
        ("groq", _) => Some(0.05),
        ("openai", "gpt-4o-mini") => Some(0.15),
        ("anthropic", "claude-3-5-haiku-latest") => Some(0.25),
        ("google", "gemini-2.0-flash-lite") => Some(0.075),
        ("mistral", "mistral-small-latest") => Some(0.1),
        ("xai", "grok-2-mini") => Some(0.1),
        _ => None,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_parameters() {
        assert_eq!(SmallModelTask::GenerateTitle.max_tokens(), 50);
        assert_eq!(SmallModelTask::GenerateSummary.max_tokens(), 500);

        assert!((SmallModelTask::GenerateTitle.temperature() - 0.7).abs() < 0.001);
        assert!((SmallModelTask::ClassifyIntent.temperature() - 0.1).abs() < 0.001);
    }

    #[test]
    fn test_task_system_prompts() {
        assert!(
            SmallModelTask::GenerateTitle
                .default_system_prompt()
                .is_some()
        );
        assert!(
            SmallModelTask::GenerateSummary
                .default_system_prompt()
                .is_some()
        );
        assert!(SmallModelTask::SimpleQA.default_system_prompt().is_none());
    }

    #[test]
    fn test_selector_parse_model_string() {
        let selector = SmallModelSelector::new();

        let result = selector.parse_model_string("openai/gpt-4o-mini");
        assert_eq!(
            result,
            Some(("openai".to_string(), "gpt-4o-mini".to_string()))
        );

        // Model without provider should be looked up
        let result = selector.parse_model_string("gpt-4o-mini");
        assert_eq!(
            result,
            Some(("openai".to_string(), "gpt-4o-mini".to_string()))
        );
    }

    #[test]
    fn test_selector_with_configured_model() {
        let selector =
            SmallModelSelector::with_configured_model("anthropic/claude-3-5-haiku-latest");
        let result = selector.select();
        assert_eq!(
            result,
            Some((
                "anthropic".to_string(),
                "claude-3-5-haiku-latest".to_string()
            ))
        );
    }

    #[test]
    fn test_selector_with_custom_providers() {
        let selector = SmallModelSelector::with_providers(vec!["openai".to_string()]);
        assert!(selector.is_provider_available("openai"));
        assert!(!selector.is_provider_available("anthropic"));
    }

    #[test]
    fn test_for_provider() {
        let selector = SmallModelSelector::new();

        assert_eq!(
            selector.for_provider("openai"),
            Some("gpt-4o-mini".to_string())
        );
        assert_eq!(
            selector.for_provider("anthropic"),
            Some("claude-3-5-haiku-latest".to_string())
        );
        assert_eq!(selector.for_provider("nonexistent"), None);
    }

    #[test]
    fn test_small_model_config() {
        let config = SmallModelConfig::new(SmallModelTask::GenerateTitle)
            .with_temperature(0.5)
            .with_max_tokens(100);

        assert!((config.effective_temperature() - 0.5).abs() < 0.001);
        assert_eq!(config.effective_max_tokens(), 100);
    }

    #[test]
    fn test_list_small_models() {
        let models = list_small_models();
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m.provider == "openai"));
        assert!(models.iter().any(|m| m.provider == "anthropic"));
    }
}
