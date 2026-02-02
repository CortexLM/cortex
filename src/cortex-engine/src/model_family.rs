//! Model family detection and information.
//!
//! Provides utilities for identifying model families, capabilities,
//! and optimal configurations.

use serde::{Deserialize, Serialize};

/// Model family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ModelFamily {
    /// OpenAI GPT-4 family.
    Gpt4,
    /// OpenAI GPT-4 Turbo family.
    Gpt4Turbo,
    /// OpenAI GPT-4o family.
    Gpt4o,
    /// OpenAI GPT-3.5 family.
    Gpt35,
    /// OpenAI o1 reasoning family.
    O1,
    /// OpenAI o3 reasoning family.
    O3,
    /// Anthropic Claude 3 Opus.
    Claude3Opus,
    /// Anthropic Claude 3.5 Sonnet.
    Claude35Sonnet,
    /// Anthropic Claude 3 Haiku.
    Claude3Haiku,
    /// Anthropic Claude 4.
    Claude4,
    /// Google Gemini 1.5 Pro.
    Gemini15Pro,
    /// Google Gemini 1.5 Flash.
    Gemini15Flash,
    /// Google Gemini 2.0.
    Gemini20,
    /// Meta Llama 3.
    Llama3,
    /// Meta Llama 3.1.
    Llama31,
    /// Meta Llama 3.2.
    Llama32,
    /// Mistral Large.
    MistralLarge,
    /// Mistral Medium.
    MistralMedium,
    /// Mistral Small.
    MistralSmall,
    /// Mixtral.
    Mixtral,
    /// DeepSeek.
    DeepSeek,
    /// DeepSeek Coder.
    DeepSeekCoder,
    /// Qwen.
    Qwen,
    /// Cohere Command.
    Command,
    /// Cohere Command R.
    CommandR,
    /// Unknown model.
    #[default]
    Unknown,
}

impl ModelFamily {
    /// Detect model family from model name.
    pub fn from_model_name(name: &str) -> Self {
        let name_lower = name.to_lowercase();

        // OpenAI models
        if name_lower.contains("o3") {
            return Self::O3;
        }
        if name_lower.contains("o1") {
            return Self::O1;
        }
        if name_lower.contains("gpt-4o") || name_lower.contains("gpt4o") {
            return Self::Gpt4o;
        }
        if name_lower.contains("gpt-4-turbo") || name_lower.contains("gpt4-turbo") {
            return Self::Gpt4Turbo;
        }
        if name_lower.contains("gpt-4") || name_lower.contains("gpt4") {
            return Self::Gpt4;
        }
        if name_lower.contains("gpt-3.5") || name_lower.contains("gpt35") {
            return Self::Gpt35;
        }

        // Anthropic models
        if name_lower.contains("claude-4") || name_lower.contains("claude4") {
            return Self::Claude4;
        }
        if name_lower.contains("claude-3-5-sonnet") || name_lower.contains("claude-3.5-sonnet") {
            return Self::Claude35Sonnet;
        }
        if name_lower.contains("claude-3-opus") {
            return Self::Claude3Opus;
        }
        if name_lower.contains("claude-3-haiku") {
            return Self::Claude3Haiku;
        }

        // Google models
        if name_lower.contains("gemini-2") {
            return Self::Gemini20;
        }
        if name_lower.contains("gemini-1.5-pro") || name_lower.contains("gemini-pro") {
            return Self::Gemini15Pro;
        }
        if name_lower.contains("gemini-1.5-flash") || name_lower.contains("gemini-flash") {
            return Self::Gemini15Flash;
        }

        // Meta models
        if name_lower.contains("llama-3.2") || name_lower.contains("llama3.2") {
            return Self::Llama32;
        }
        if name_lower.contains("llama-3.1") || name_lower.contains("llama3.1") {
            return Self::Llama31;
        }
        if name_lower.contains("llama-3") || name_lower.contains("llama3") {
            return Self::Llama3;
        }

        // Mistral models
        if name_lower.contains("mistral-large") {
            return Self::MistralLarge;
        }
        if name_lower.contains("mistral-medium") {
            return Self::MistralMedium;
        }
        if name_lower.contains("mistral-small") || name_lower.contains("mistral-7b") {
            return Self::MistralSmall;
        }
        if name_lower.contains("mixtral") {
            return Self::Mixtral;
        }

        // DeepSeek models
        if name_lower.contains("deepseek-coder") {
            return Self::DeepSeekCoder;
        }
        if name_lower.contains("deepseek") {
            return Self::DeepSeek;
        }

        // Qwen models
        if name_lower.contains("qwen") {
            return Self::Qwen;
        }

        // Cohere models
        if name_lower.contains("command-r") {
            return Self::CommandR;
        }
        if name_lower.contains("command") {
            return Self::Command;
        }

        Self::Unknown
    }

    /// Get model capabilities.
    pub fn capabilities(&self) -> ModelCapabilities {
        match self {
            Self::Gpt4o => ModelCapabilities {
                vision: true,
                tools: true,
                reasoning: false,
                json_mode: true,
                streaming: true,
                context_window: 128000,
                max_output: 16384,
            },
            Self::Gpt4 | Self::Gpt4Turbo => ModelCapabilities {
                vision: true,
                tools: true,
                reasoning: false,
                json_mode: true,
                streaming: true,
                context_window: 128000,
                max_output: 4096,
            },
            Self::Gpt35 => ModelCapabilities {
                vision: false,
                tools: true,
                reasoning: false,
                json_mode: true,
                streaming: true,
                context_window: 16385,
                max_output: 4096,
            },
            Self::O1 | Self::O3 => ModelCapabilities {
                vision: true,
                tools: true,
                reasoning: true,
                json_mode: true,
                streaming: true,
                context_window: 200000,
                max_output: 100000,
            },
            Self::Claude4 | Self::Claude35Sonnet => ModelCapabilities {
                vision: true,
                tools: true,
                reasoning: true,
                json_mode: false,
                streaming: true,
                context_window: 200000,
                max_output: 8192,
            },
            Self::Claude3Opus => ModelCapabilities {
                vision: true,
                tools: true,
                reasoning: false,
                json_mode: false,
                streaming: true,
                context_window: 200000,
                max_output: 4096,
            },
            Self::Claude3Haiku => ModelCapabilities {
                vision: true,
                tools: true,
                reasoning: false,
                json_mode: false,
                streaming: true,
                context_window: 200000,
                max_output: 4096,
            },
            Self::Gemini20 | Self::Gemini15Pro => ModelCapabilities {
                vision: true,
                tools: true,
                reasoning: true,
                json_mode: true,
                streaming: true,
                context_window: 1000000,
                max_output: 8192,
            },
            Self::Gemini15Flash => ModelCapabilities {
                vision: true,
                tools: true,
                reasoning: false,
                json_mode: true,
                streaming: true,
                context_window: 1000000,
                max_output: 8192,
            },
            Self::Llama32 | Self::Llama31 => ModelCapabilities {
                vision: true,
                tools: true,
                reasoning: false,
                json_mode: true,
                streaming: true,
                context_window: 128000,
                max_output: 4096,
            },
            Self::Llama3 => ModelCapabilities {
                vision: false,
                tools: true,
                reasoning: false,
                json_mode: true,
                streaming: true,
                context_window: 8192,
                max_output: 2048,
            },
            Self::MistralLarge => ModelCapabilities {
                vision: false,
                tools: true,
                reasoning: false,
                json_mode: true,
                streaming: true,
                context_window: 128000,
                max_output: 4096,
            },
            Self::DeepSeek | Self::DeepSeekCoder => ModelCapabilities {
                vision: false,
                tools: true,
                reasoning: true,
                json_mode: true,
                streaming: true,
                context_window: 128000,
                max_output: 8192,
            },
            _ => ModelCapabilities::default(),
        }
    }

    /// Get provider name.
    pub fn provider(&self) -> &'static str {
        match self {
            Self::Gpt4 | Self::Gpt4Turbo | Self::Gpt4o | Self::Gpt35 | Self::O1 | Self::O3 => {
                "openai"
            }
            Self::Claude4 | Self::Claude35Sonnet | Self::Claude3Opus | Self::Claude3Haiku => {
                "anthropic"
            }
            Self::Gemini20 | Self::Gemini15Pro | Self::Gemini15Flash => "google",
            Self::Llama3 | Self::Llama31 | Self::Llama32 => "meta",
            Self::MistralLarge | Self::MistralMedium | Self::MistralSmall | Self::Mixtral => {
                "mistral"
            }
            Self::DeepSeek | Self::DeepSeekCoder => "deepseek",
            Self::Qwen => "alibaba",
            Self::Command | Self::CommandR => "cohere",
            Self::Unknown => "unknown",
        }
    }

    /// Check if this is a reasoning model.
    pub fn is_reasoning(&self) -> bool {
        matches!(
            self,
            Self::O1
                | Self::O3
                | Self::Claude4
                | Self::Claude35Sonnet
                | Self::Gemini20
                | Self::DeepSeek
        )
    }

    /// Check if this model supports vision.
    pub fn supports_vision(&self) -> bool {
        self.capabilities().vision
    }

    /// Check if this model supports tools.
    pub fn supports_tools(&self) -> bool {
        self.capabilities().tools
    }

    /// Get recommended temperature.
    pub fn recommended_temperature(&self) -> f32 {
        if self.is_reasoning() {
            1.0 // Reasoning models often work best at temp 1
        } else {
            0.7 // Standard temperature for most tasks
        }
    }

    /// Get model tier (quality/cost ranking).
    pub fn tier(&self) -> ModelTier {
        match self {
            Self::O3 | Self::Claude4 => ModelTier::Frontier,
            Self::O1 | Self::Gpt4o | Self::Claude35Sonnet | Self::Claude3Opus | Self::Gemini20 => {
                ModelTier::Premium
            }
            Self::Gpt4
            | Self::Gpt4Turbo
            | Self::Gemini15Pro
            | Self::MistralLarge
            | Self::DeepSeek => ModelTier::Standard,
            Self::Gpt35
            | Self::Claude3Haiku
            | Self::Gemini15Flash
            | Self::Llama32
            | Self::Llama31 => ModelTier::Efficient,
            Self::Llama3 | Self::MistralSmall | Self::Mixtral => ModelTier::Base,
            _ => ModelTier::Unknown,
        }
    }
}

impl std::fmt::Display for ModelFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gpt4 => write!(f, "GPT-4"),
            Self::Gpt4Turbo => write!(f, "GPT-4 Turbo"),
            Self::Gpt4o => write!(f, "GPT-4o"),
            Self::Gpt35 => write!(f, "GPT-3.5"),
            Self::O1 => write!(f, "o1"),
            Self::O3 => write!(f, "o3"),
            Self::Claude4 => write!(f, "Claude 4"),
            Self::Claude35Sonnet => write!(f, "Claude 3.5 Sonnet"),
            Self::Claude3Opus => write!(f, "Claude 3 Opus"),
            Self::Claude3Haiku => write!(f, "Claude 3 Haiku"),
            Self::Gemini20 => write!(f, "Gemini 2.0"),
            Self::Gemini15Pro => write!(f, "Gemini 1.5 Pro"),
            Self::Gemini15Flash => write!(f, "Gemini 1.5 Flash"),
            Self::Llama3 => write!(f, "Llama 3"),
            Self::Llama31 => write!(f, "Llama 3.1"),
            Self::Llama32 => write!(f, "Llama 3.2"),
            Self::MistralLarge => write!(f, "Mistral Large"),
            Self::MistralMedium => write!(f, "Mistral Medium"),
            Self::MistralSmall => write!(f, "Mistral Small"),
            Self::Mixtral => write!(f, "Mixtral"),
            Self::DeepSeek => write!(f, "DeepSeek"),
            Self::DeepSeekCoder => write!(f, "DeepSeek Coder"),
            Self::Qwen => write!(f, "Qwen"),
            Self::Command => write!(f, "Command"),
            Self::CommandR => write!(f, "Command R"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Model capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Supports vision/images.
    pub vision: bool,
    /// Supports tool/function calling.
    pub tools: bool,
    /// Has extended reasoning capabilities.
    pub reasoning: bool,
    /// Supports JSON mode.
    pub json_mode: bool,
    /// Supports streaming.
    pub streaming: bool,
    /// Context window size in tokens.
    pub context_window: u32,
    /// Maximum output tokens.
    pub max_output: u32,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            vision: false,
            tools: false,
            reasoning: false,
            json_mode: false,
            streaming: true,
            context_window: 4096,
            max_output: 2048,
        }
    }
}

/// Model tier/quality level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ModelTier {
    /// Unknown tier.
    #[default]
    Unknown = 0,
    /// Base tier models.
    Base = 1,
    /// Efficient/fast models.
    Efficient = 2,
    /// Standard quality models.
    Standard = 3,
    /// Premium/high quality models.
    Premium = 4,
    /// Frontier/best available models.
    Frontier = 5,
}

/// Model pricing (per 1M tokens).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Input token price.
    pub input: f64,
    /// Output token price.
    pub output: f64,
    /// Cached input price.
    pub cached_input: Option<f64>,
}

impl ModelPricing {
    /// Get approximate pricing for a model family.
    pub fn for_family(family: ModelFamily) -> Self {
        match family {
            ModelFamily::Gpt4o => Self {
                input: 2.50,
                output: 10.00,
                cached_input: Some(1.25),
            },
            ModelFamily::Gpt4 | ModelFamily::Gpt4Turbo => Self {
                input: 10.00,
                output: 30.00,
                cached_input: None,
            },
            ModelFamily::Gpt35 => Self {
                input: 0.50,
                output: 1.50,
                cached_input: None,
            },
            ModelFamily::O1 => Self {
                input: 15.00,
                output: 60.00,
                cached_input: Some(7.50),
            },
            ModelFamily::O3 => Self {
                input: 20.00,
                output: 80.00,
                cached_input: None,
            },
            ModelFamily::Claude35Sonnet => Self {
                input: 3.00,
                output: 15.00,
                cached_input: Some(0.30),
            },
            ModelFamily::Claude3Opus => Self {
                input: 15.00,
                output: 75.00,
                cached_input: None,
            },
            ModelFamily::Claude3Haiku => Self {
                input: 0.25,
                output: 1.25,
                cached_input: Some(0.03),
            },
            ModelFamily::Gemini15Pro => Self {
                input: 1.25,
                output: 5.00,
                cached_input: None,
            },
            ModelFamily::Gemini15Flash => Self {
                input: 0.075,
                output: 0.30,
                cached_input: None,
            },
            ModelFamily::DeepSeek => Self {
                input: 0.27,
                output: 1.10,
                cached_input: Some(0.07),
            },
            _ => Self {
                input: 0.0,
                output: 0.0,
                cached_input: None,
            },
        }
    }

    /// Calculate cost for token usage.
    pub fn calculate(&self, input_tokens: u64, output_tokens: u64, cached_tokens: u64) -> f64 {
        let input_cost = (input_tokens as f64 / 1_000_000.0) * self.input;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * self.output;
        let cached_cost = self
            .cached_input
            .map(|p| (cached_tokens as f64 / 1_000_000.0) * p)
            .unwrap_or(0.0);

        input_cost + output_cost + cached_cost
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_family_detection() {
        assert_eq!(ModelFamily::from_model_name("gpt-4o"), ModelFamily::Gpt4o);
        assert_eq!(
            ModelFamily::from_model_name("claude-3-5-sonnet-20241022"),
            ModelFamily::Claude35Sonnet
        );
        assert_eq!(ModelFamily::from_model_name("o1-preview"), ModelFamily::O1);
        assert_eq!(
            ModelFamily::from_model_name("gemini-1.5-pro"),
            ModelFamily::Gemini15Pro
        );
    }

    #[test]
    fn test_model_capabilities() {
        let caps = ModelFamily::Gpt4o.capabilities();
        assert!(caps.vision);
        assert!(caps.tools);
        assert_eq!(caps.context_window, 128000);
    }

    #[test]
    fn test_model_tier() {
        assert!(ModelFamily::O3.tier() > ModelFamily::Gpt4.tier());
        assert!(ModelFamily::Gpt4.tier() > ModelFamily::Gpt35.tier());
    }

    #[test]
    fn test_pricing() {
        let pricing = ModelPricing::for_family(ModelFamily::Gpt4o);
        let cost = pricing.calculate(1_000_000, 500_000, 0);
        assert!((cost - 7.50).abs() < 0.01);
    }
}
