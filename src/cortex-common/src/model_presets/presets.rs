//! Model preset data definitions.

use super::types::ModelPreset;

/// Default model for Chutes provider.
/// This is the fallback model when no specific model is provided.
pub const DEFAULT_CHUTES_MODEL: &str = "moonshotai/Kimi-K2.5-TEE";

/// Available model presets.
pub const MODEL_PRESETS: &[ModelPreset] = &[
    ModelPreset {
        id: "gpt-4o",
        name: "GPT-4o",
        provider: "openai",
        context_window: 128_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "gpt-4o-mini",
        name: "GPT-4o Mini",
        provider: "openai",
        context_window: 128_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "o1",
        name: "o1",
        provider: "openai",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: true,
    },
    ModelPreset {
        id: "o1-mini",
        name: "o1-mini",
        provider: "openai",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: true,
    },
    ModelPreset {
        id: "claude-3-5-sonnet",
        name: "Claude 3.5 Sonnet",
        provider: "anthropic",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "claude-3-opus",
        name: "Claude 3 Opus",
        provider: "anthropic",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    // Google Gemini models
    ModelPreset {
        id: "gemini-2.0-flash-exp",
        name: "Gemini 2.0 Flash (Experimental)",
        provider: "google",
        context_window: 1_048_576,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "gemini-2.0-flash",
        name: "Gemini 2.0 Flash",
        provider: "google",
        context_window: 1_048_576,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "gemini-1.5-pro",
        name: "Gemini 1.5 Pro",
        provider: "google",
        context_window: 2_097_152,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "gemini-1.5-flash",
        name: "Gemini 1.5 Flash",
        provider: "google",
        context_window: 1_048_576,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "gemini-1.5-flash-8b",
        name: "Gemini 1.5 Flash 8B",
        provider: "google",
        context_window: 1_048_576,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    // Mistral AI models
    ModelPreset {
        id: "mistral-large-latest",
        name: "Mistral Large",
        provider: "mistral",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "mistral-medium-latest",
        name: "Mistral Medium",
        provider: "mistral",
        context_window: 32_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "mistral-small-latest",
        name: "Mistral Small",
        provider: "mistral",
        context_window: 32_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "codestral-latest",
        name: "Codestral",
        provider: "mistral",
        context_window: 32_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "open-mixtral-8x22b",
        name: "Mixtral 8x22B",
        provider: "mistral",
        context_window: 64_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "open-mistral-7b",
        name: "Mistral 7B",
        provider: "mistral",
        context_window: 32_000,
        supports_vision: false,
        supports_tools: false,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "pixtral-large-latest",
        name: "Pixtral Large",
        provider: "mistral",
        context_window: 128_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    // Groq models (ultra-fast inference)
    ModelPreset {
        id: "llama-3.3-70b-versatile",
        name: "Llama 3.3 70B",
        provider: "groq",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "llama-3.1-70b-versatile",
        name: "Llama 3.1 70B",
        provider: "groq",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "llama-3.1-8b-instant",
        name: "Llama 3.1 8B Instant",
        provider: "groq",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "llama3-70b-8192",
        name: "Llama 3 70B",
        provider: "groq",
        context_window: 8_192,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "llama3-8b-8192",
        name: "Llama 3 8B",
        provider: "groq",
        context_window: 8_192,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "mixtral-8x7b-32768",
        name: "Mixtral 8x7B",
        provider: "groq",
        context_window: 32_768,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "gemma2-9b-it",
        name: "Gemma 2 9B IT",
        provider: "groq",
        context_window: 8_192,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    // Cerebras models (ultra-fast inference on Wafer-Scale Engine)
    // Cerebras is the fastest inference provider in the industry
    ModelPreset {
        id: "llama3.1-8b",
        name: "Llama 3.1 8B (Cerebras - Ultra Fast)",
        provider: "cerebras",
        context_window: 8_192,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "llama3.1-70b",
        name: "Llama 3.1 70B (Cerebras - Fast)",
        provider: "cerebras",
        context_window: 8_192,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "llama-3.3-70b",
        name: "Llama 3.3 70B (Cerebras - Latest)",
        provider: "cerebras",
        context_window: 8_192,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    // xAI (Grok) models
    ModelPreset {
        id: "grok-2",
        name: "Grok 2",
        provider: "xai",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "grok-2-mini",
        name: "Grok 2 Mini",
        provider: "xai",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "grok-beta",
        name: "Grok Beta",
        provider: "xai",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "grok-vision-beta",
        name: "Grok Vision Beta",
        provider: "xai",
        context_window: 8_000,
        supports_vision: true,
        supports_tools: false,
        supports_reasoning: false,
    },
    // GitHub Copilot models (via Copilot subscription)
    ModelPreset {
        id: "copilot/gpt-4o",
        name: "GPT-4o (via Copilot)",
        provider: "github-copilot",
        context_window: 128_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "copilot/gpt-4o-mini",
        name: "GPT-4o Mini (via Copilot)",
        provider: "github-copilot",
        context_window: 128_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "copilot/claude-3.5-sonnet",
        name: "Claude 3.5 Sonnet (via Copilot)",
        provider: "github-copilot",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "copilot/o1-preview",
        name: "o1-preview (via Copilot)",
        provider: "github-copilot",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: true,
    },
    ModelPreset {
        id: "copilot/o1-mini",
        name: "o1-mini (via Copilot)",
        provider: "github-copilot",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: true,
    },
    // Amazon Bedrock models (via AWS)
    ModelPreset {
        id: "anthropic.claude-3-5-sonnet-20241022-v2:0",
        name: "Claude 3.5 Sonnet v2 (Bedrock)",
        provider: "bedrock",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "anthropic.claude-3-5-haiku-20241022-v1:0",
        name: "Claude 3.5 Haiku (Bedrock)",
        provider: "bedrock",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "anthropic.claude-3-opus-20240229-v1:0",
        name: "Claude 3 Opus (Bedrock)",
        provider: "bedrock",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "anthropic.claude-3-sonnet-20240229-v1:0",
        name: "Claude 3 Sonnet (Bedrock)",
        provider: "bedrock",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "anthropic.claude-3-haiku-20240307-v1:0",
        name: "Claude 3 Haiku (Bedrock)",
        provider: "bedrock",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "meta.llama3-1-70b-instruct-v1:0",
        name: "Llama 3.1 70B (Bedrock)",
        provider: "bedrock",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "meta.llama3-1-8b-instruct-v1:0",
        name: "Llama 3.1 8B (Bedrock)",
        provider: "bedrock",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "amazon.titan-text-premier-v1:0",
        name: "Titan Text Premier (Bedrock)",
        provider: "bedrock",
        context_window: 32_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "amazon.titan-text-express-v1",
        name: "Titan Text Express (Bedrock)",
        provider: "bedrock",
        context_window: 8_000,
        supports_vision: false,
        supports_tools: false,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "mistral.mistral-large-2407-v1:0",
        name: "Mistral Large (Bedrock)",
        provider: "bedrock",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    // Together AI models
    ModelPreset {
        id: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        name: "Llama 3.3 70B Instruct Turbo",
        provider: "together",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "meta-llama/Llama-3.1-405B-Instruct-Turbo",
        name: "Llama 3.1 405B Instruct Turbo",
        provider: "together",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "mistralai/Mixtral-8x22B-Instruct-v0.1",
        name: "Mixtral 8x22B Instruct v0.1",
        provider: "together",
        context_window: 65_536,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "Qwen/Qwen2.5-72B-Instruct-Turbo",
        name: "Qwen 2.5 72B Instruct Turbo",
        provider: "together",
        context_window: 32_768,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "deepseek-ai/DeepSeek-V3",
        name: "DeepSeek V3",
        provider: "together",
        context_window: 65_536,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "google/gemma-2-27b-it",
        name: "Gemma 2 27B IT",
        provider: "together",
        context_window: 8_192,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    // DeepInfra models (serverless GPU inference)
    ModelPreset {
        id: "deepinfra/meta-llama/Meta-Llama-3.1-405B-Instruct",
        name: "Llama 3.1 405B Instruct (DeepInfra)",
        provider: "deepinfra",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "deepinfra/meta-llama/Meta-Llama-3.1-70B-Instruct",
        name: "Llama 3.1 70B Instruct (DeepInfra)",
        provider: "deepinfra",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "deepinfra/mistralai/Mixtral-8x22B-Instruct-v0.1",
        name: "Mixtral 8x22B Instruct (DeepInfra)",
        provider: "deepinfra",
        context_window: 65_536,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "deepinfra/microsoft/WizardLM-2-8x22B",
        name: "WizardLM 2 8x22B (DeepInfra)",
        provider: "deepinfra",
        context_window: 65_536,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "deepinfra/Qwen/Qwen2.5-72B-Instruct",
        name: "Qwen 2.5 72B Instruct (DeepInfra)",
        provider: "deepinfra",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    // DeepSeek models (direct API access)
    ModelPreset {
        id: "deepseek-chat",
        name: "DeepSeek-V3",
        provider: "deepseek",
        context_window: 64_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "deepseek-coder",
        name: "DeepSeek-Coder",
        provider: "deepseek",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "deepseek-reasoner",
        name: "DeepSeek-R1",
        provider: "deepseek",
        context_window: 64_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: true,
    },
    // Perplexity AI models (search-augmented)
    // Online models (with web search and citations)
    ModelPreset {
        id: "llama-3.1-sonar-small-128k-online",
        name: "Sonar Small Online (8B)",
        provider: "perplexity",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: false,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "llama-3.1-sonar-large-128k-online",
        name: "Sonar Large Online (70B)",
        provider: "perplexity",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: false,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "llama-3.1-sonar-huge-128k-online",
        name: "Sonar Huge Online (405B)",
        provider: "perplexity",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: false,
        supports_reasoning: false,
    },
    // Chat models (offline, no web search)
    ModelPreset {
        id: "llama-3.1-sonar-small-128k-chat",
        name: "Sonar Small Chat (8B)",
        provider: "perplexity",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: false,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "llama-3.1-sonar-large-128k-chat",
        name: "Sonar Large Chat (70B)",
        provider: "perplexity",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: false,
        supports_reasoning: false,
    },
    // Cortex models (200+ models via unified API)
    // These are the most popular models accessible through OpenRouter
    // DEFAULT MODELS
    ModelPreset {
        id: "anthropic/claude-opus-4.5",
        name: "Claude Opus 4.5 (via Cortex) - DEFAULT",
        provider: "cortex",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: true,
    },
    ModelPreset {
        id: "anthropic/claude-haiku-4.5",
        name: "Claude Haiku 4.5 (via Cortex) - DEFAULT",
        provider: "cortex",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    // Other Cortex models
    ModelPreset {
        id: "openai/gpt-4o",
        name: "GPT-4o (via Cortex)",
        provider: "cortex",
        context_window: 128_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "openai/gpt-4o-mini",
        name: "GPT-4o Mini (via Cortex)",
        provider: "cortex",
        context_window: 128_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "anthropic/claude-3.5-sonnet",
        name: "Claude 3.5 Sonnet (via Cortex)",
        provider: "cortex",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "anthropic/claude-3-opus",
        name: "Claude 3 Opus (via Cortex)",
        provider: "cortex",
        context_window: 200_000,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "google/gemini-pro-1.5",
        name: "Gemini 1.5 Pro (via Cortex)",
        provider: "cortex",
        context_window: 2_097_152,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "google/gemini-flash-1.5",
        name: "Gemini 1.5 Flash (via Cortex)",
        provider: "cortex",
        context_window: 1_048_576,
        supports_vision: true,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "meta-llama/llama-3.1-405b-instruct",
        name: "Llama 3.1 405B Instruct (via Cortex)",
        provider: "cortex",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "meta-llama/llama-3.1-70b-instruct",
        name: "Llama 3.1 70B Instruct (via Cortex)",
        provider: "cortex",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "mistralai/mistral-large",
        name: "Mistral Large (via Cortex)",
        provider: "cortex",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "mistralai/mixtral-8x22b-instruct",
        name: "Mixtral 8x22B Instruct (via Cortex)",
        provider: "cortex",
        context_window: 65_536,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "deepseek/deepseek-chat",
        name: "DeepSeek Chat (via Cortex)",
        provider: "cortex",
        context_window: 64_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "deepseek/deepseek-r1",
        name: "DeepSeek R1 (via Cortex)",
        provider: "cortex",
        context_window: 64_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: true,
    },
    ModelPreset {
        id: "cohere/command-r-plus",
        name: "Command R+ (via Cortex)",
        provider: "cortex",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    // Cohere models
    ModelPreset {
        id: "command-r-plus",
        name: "Command R+",
        provider: "cohere",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "command-r-plus-08-2024",
        name: "Command R+ (Aug 2024)",
        provider: "cohere",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "command-r",
        name: "Command R",
        provider: "cohere",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "command-r-08-2024",
        name: "Command R (Aug 2024)",
        provider: "cohere",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "command-light",
        name: "Command Light",
        provider: "cohere",
        context_window: 4_096,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    ModelPreset {
        id: "command-nightly",
        name: "Command Nightly",
        provider: "cohere",
        context_window: 128_000,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: false,
    },
    // Chutes TEE models (Trusted Execution Environment)
    // Security requirement: Only models with '-TEE' suffix are allowed
    ModelPreset {
        id: "moonshotai/Kimi-K2.5-TEE",
        name: "Kimi K2.5 (TEE)",
        provider: "chutes",
        context_window: 262_144,
        supports_vision: false,
        supports_tools: true,
        supports_reasoning: true,
    },
];

/// Get a model preset by ID.
pub fn get_model_preset(id: &str) -> Option<&'static ModelPreset> {
    MODEL_PRESETS.iter().find(|m| m.id == id)
}

/// Get models for a specific provider.
pub fn get_models_for_provider(provider: &str) -> Vec<&'static ModelPreset> {
    MODEL_PRESETS
        .iter()
        .filter(|m| m.provider == provider)
        .collect()
}

/// Validates that a model is allowed for the Chutes provider.
/// Chutes only allows TEE (Trusted Execution Environment) models for security.
/// Any model ending with '-TEE' suffix (case-insensitive) is accepted.
/// Returns Ok(()) if valid, Err with message if invalid.
///
/// # Security
/// This function performs strict validation to prevent bypass attacks:
/// - Rejects null bytes and control characters (prevents C-string truncation attacks)
/// - Only allows safe ASCII characters: alphanumeric, hyphen, underscore, dot, forward slash
/// - Case-insensitive suffix check for -TEE
pub fn validate_chutes_model(model: &str) -> Result<(), String> {
    let model = model.trim();

    // Check for empty model
    if model.is_empty() {
        return Err("Model name cannot be empty for Chutes provider".to_string());
    }

    // SECURITY: Reject null bytes and control characters (CWE-626, CWE-158)
    // This prevents null byte injection attacks where "malicious\0-TEE" would
    // pass validation but be truncated to "malicious" by C libraries/APIs
    if model.bytes().any(|b| b == 0 || b < 0x20) {
        return Err(
            "Model name contains invalid characters (null bytes or control characters)".to_string(),
        );
    }

    // SECURITY: Only allow safe ASCII characters for model names
    // Allowed: a-z, A-Z, 0-9, hyphen (-), underscore (_), dot (.), forward slash (/)
    // This prevents Unicode homoglyph attacks and other encoding-based bypasses
    if !model
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
    {
        return Err(
            "Model name contains invalid characters. Only alphanumeric characters, \
             hyphens, underscores, dots, and forward slashes are allowed."
                .to_string(),
        );
    }

    // Check suffix (case-insensitive) - any model ending with -TEE is allowed
    if !model.to_uppercase().ends_with("-TEE") {
        return Err(format!(
            "Chutes provider only allows TEE models (models ending with '-TEE'). \
             Model '{}' is not a TEE model. Default model: {}",
            model, DEFAULT_CHUTES_MODEL
        ));
    }

    Ok(())
}

/// Checks if a provider restricts custom models.
/// All providers allow custom models, but Chutes requires -TEE suffix.
pub fn provider_allows_custom_models(provider: &str) -> bool {
    // All providers allow custom models
    // Chutes allows any model with -TEE suffix (validated via validate_chutes_model)
    let _ = provider; // Used for potential future provider-specific restrictions
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_chutes_model_valid() {
        // Default TEE model
        assert!(validate_chutes_model("moonshotai/Kimi-K2.5-TEE").is_ok());
        // Case insensitive
        assert!(validate_chutes_model("moonshotai/kimi-k2.5-tee").is_ok());
        assert!(validate_chutes_model("MOONSHOTAI/KIMI-K2.5-TEE").is_ok());
        // Whitespace handling
        assert!(validate_chutes_model("  moonshotai/Kimi-K2.5-TEE  ").is_ok());
        // Any model with -TEE suffix is valid
        assert!(validate_chutes_model("custom-model-TEE").is_ok());
        assert!(validate_chutes_model("some-provider/my-model-TEE").is_ok());
        assert!(validate_chutes_model("another-model-tee").is_ok());
        assert!(validate_chutes_model("UPPERCASE-MODEL-TEE").is_ok());
        // Allowed special characters
        assert!(validate_chutes_model("provider_name/model.v1-TEE").is_ok());
        assert!(validate_chutes_model("my_custom_model-TEE").is_ok());
    }

    #[test]
    fn test_validate_chutes_model_invalid() {
        // Not a TEE model (no -TEE suffix)
        assert!(validate_chutes_model("gpt-4").is_err());
        assert!(validate_chutes_model("claude-3").is_err());
        assert!(validate_chutes_model("some-model").is_err());

        // TEE in wrong position (not at the end)
        assert!(validate_chutes_model("model-TEE-v2").is_err());
        assert!(validate_chutes_model("TEE-model").is_err());
        assert!(validate_chutes_model("my-TEE-model-v1").is_err());

        // Empty string
        let result = validate_chutes_model("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot be empty"));

        // Whitespace only
        let result = validate_chutes_model("   ");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot be empty"));
    }

    // ===========================================
    // SECURITY TESTS: Bypass attempt prevention
    // ===========================================

    #[test]
    fn test_validate_chutes_model_null_byte_injection() {
        // SECURITY: Null byte injection attack (CWE-626, CWE-158)
        // Attacker tries to bypass TEE check by appending -TEE after a null byte
        // C libraries would see only "gpt-4" but our validation would see "gpt-4\0-TEE"
        let malicious_with_null = "gpt-4\0-TEE";
        let result = validate_chutes_model(malicious_with_null);
        assert!(result.is_err(), "Null byte injection should be rejected");
        assert!(
            result.unwrap_err().contains("invalid characters"),
            "Error should mention invalid characters"
        );

        // More null byte attack variants
        assert!(validate_chutes_model("claude-3\0-TEE").is_err());
        assert!(validate_chutes_model("\0model-TEE").is_err());
        assert!(validate_chutes_model("model\0-TEE\0").is_err());
    }

    #[test]
    fn test_validate_chutes_model_control_characters() {
        // SECURITY: Control character injection
        // Characters below 0x20 (space) could cause parsing issues
        assert!(validate_chutes_model("model\t-TEE").is_err()); // Tab
        assert!(validate_chutes_model("model\n-TEE").is_err()); // Newline
        assert!(validate_chutes_model("model\r-TEE").is_err()); // Carriage return
        assert!(validate_chutes_model("model\x1b-TEE").is_err()); // Escape
        assert!(validate_chutes_model("model\x07-TEE").is_err()); // Bell
    }

    #[test]
    fn test_validate_chutes_model_unicode_attacks() {
        // SECURITY: Unicode homoglyph attacks
        // Attacker tries to use visually similar Unicode characters

        // Cyrillic 'Е' (U+0415) looks like Latin 'E' but is different
        assert!(validate_chutes_model("model-TЕЕ").is_err()); // Cyrillic E

        // Fullwidth characters
        assert!(validate_chutes_model("model-ＴＥＥ").is_err()); // Fullwidth TEE

        // Other Unicode tricks
        assert!(validate_chutes_model("model-TEE\u{200B}").is_err()); // Zero-width space at end
        assert!(validate_chutes_model("model\u{FEFF}-TEE").is_err()); // BOM in middle

        // Combining characters
        assert!(validate_chutes_model("model-TE\u{0301}E").is_err()); // E with combining acute
    }

    #[test]
    fn test_validate_chutes_model_special_characters() {
        // SECURITY: Reject potentially dangerous special characters
        // These could cause issues in shell commands, URLs, or other contexts

        assert!(validate_chutes_model("model;-TEE").is_err()); // Semicolon (command separator)
        assert!(validate_chutes_model("model&-TEE").is_err()); // Ampersand
        assert!(validate_chutes_model("model|-TEE").is_err()); // Pipe
        assert!(validate_chutes_model("model`-TEE").is_err()); // Backtick
        assert!(validate_chutes_model("model$-TEE").is_err()); // Dollar sign
        assert!(validate_chutes_model("model'-TEE").is_err()); // Single quote
        assert!(validate_chutes_model("model\"-TEE").is_err()); // Double quote
        assert!(validate_chutes_model("model<-TEE").is_err()); // Less than
        assert!(validate_chutes_model("model>-TEE").is_err()); // Greater than
        assert!(validate_chutes_model("model(-TEE").is_err()); // Parenthesis
        assert!(validate_chutes_model("model)-TEE").is_err());
        assert!(validate_chutes_model("model{-TEE").is_err()); // Braces
        assert!(validate_chutes_model("model}-TEE").is_err());
        assert!(validate_chutes_model("model[-TEE").is_err()); // Brackets
        assert!(validate_chutes_model("model]-TEE").is_err());
        assert!(validate_chutes_model("model\\-TEE").is_err()); // Backslash
        assert!(validate_chutes_model("model!-TEE").is_err()); // Exclamation
        assert!(validate_chutes_model("model@-TEE").is_err()); // At sign
        assert!(validate_chutes_model("model#-TEE").is_err()); // Hash
        assert!(validate_chutes_model("model%-TEE").is_err()); // Percent
        assert!(validate_chutes_model("model^-TEE").is_err()); // Caret
        assert!(validate_chutes_model("model*-TEE").is_err()); // Asterisk
        assert!(validate_chutes_model("model=-TEE").is_err()); // Equals
        assert!(validate_chutes_model("model+-TEE").is_err()); // Plus
        assert!(validate_chutes_model("model~-TEE").is_err()); // Tilde
        assert!(validate_chutes_model("model?-TEE").is_err()); // Question mark
        assert!(validate_chutes_model("model:-TEE").is_err()); // Colon
        assert!(validate_chutes_model("model,-TEE").is_err()); // Comma
        assert!(validate_chutes_model("model -TEE").is_err()); // Space in middle
    }

    #[test]
    fn test_validate_chutes_model_allowed_characters() {
        // Verify that only allowed characters pass
        // Allowed: a-z, A-Z, 0-9, hyphen (-), underscore (_), dot (.), forward slash (/)

        // All allowed characters
        assert!(validate_chutes_model("abc123-TEE").is_ok());
        assert!(validate_chutes_model("ABC123-TEE").is_ok());
        assert!(validate_chutes_model("model_name-TEE").is_ok());
        assert!(validate_chutes_model("model.v1-TEE").is_ok());
        assert!(validate_chutes_model("provider/model-TEE").is_ok());
        assert!(validate_chutes_model("my-model-TEE").is_ok());
        assert!(validate_chutes_model("Provider123/Model_v1.0-TEE").is_ok());
    }

    #[test]
    fn test_validate_chutes_model_error_message() {
        let result = validate_chutes_model("invalid-model");
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Error message should mention the default model
        assert!(err.contains(DEFAULT_CHUTES_MODEL));
        assert!(err.contains("-TEE"));
    }

    #[test]
    fn test_provider_allows_custom_models() {
        // All providers allow custom models
        assert!(provider_allows_custom_models("chutes"));
        assert!(provider_allows_custom_models("Chutes"));
        assert!(provider_allows_custom_models("CHUTES"));
        assert!(provider_allows_custom_models("cortex"));
        assert!(provider_allows_custom_models("openai"));
        assert!(provider_allows_custom_models("anthropic"));
    }

    #[test]
    fn test_default_chutes_model() {
        // Verify the default model is a valid TEE model
        assert!(DEFAULT_CHUTES_MODEL.to_uppercase().ends_with("-TEE"));
        assert_eq!(DEFAULT_CHUTES_MODEL, "moonshotai/Kimi-K2.5-TEE");
        // Verify the default model passes our own validation
        assert!(
            validate_chutes_model(DEFAULT_CHUTES_MODEL).is_ok(),
            "Default Chutes model must pass validation"
        );
    }
}
