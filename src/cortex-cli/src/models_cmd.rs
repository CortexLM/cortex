//! Models command - list available AI models.
//!
//! Provides functionality to list all available models, grouped by provider,
//! with their capabilities (vision, tools, etc.).

use anyhow::Result;
use clap::Parser;

/// Models CLI.
#[derive(Debug, Parser)]
pub struct ModelsCli {
    #[command(subcommand)]
    pub subcommand: Option<ModelsSubcommand>,

    /// Filter by provider name
    #[arg(value_name = "PROVIDER")]
    pub provider: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Models subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum ModelsSubcommand {
    /// List all available models
    List(ListModelsArgs),
}

/// Sort order for models list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModelSortOrder {
    /// Sort by model ID (default)
    #[default]
    Id,
    /// Sort by model name
    Name,
    /// Sort by provider
    Provider,
}

impl std::str::FromStr for ModelSortOrder {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "id" => Ok(Self::Id),
            "name" => Ok(Self::Name),
            "provider" => Ok(Self::Provider),
            _ => Err(format!(
                "Invalid sort order '{}'. Use: id, name, or provider",
                s
            )),
        }
    }
}

/// Arguments for list command.
#[derive(Debug, Parser)]
pub struct ListModelsArgs {
    /// Filter by provider name
    #[arg(value_name = "PROVIDER")]
    pub provider: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Limit number of results (for pagination)
    #[arg(long)]
    pub limit: Option<usize>,

    /// Offset for pagination (skip first N models)
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// Sort order for models (id, name, provider) (Issue #1993)
    #[arg(long, default_value = "id")]
    pub sort: String,

    /// Show full model IDs without truncation (Issue #1991)
    #[arg(long)]
    pub full: bool,
}

/// Model information for display.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub capabilities: ModelCapabilities,
    /// Input cost per million tokens (USD). Null for local/free models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_cost_per_million: Option<f64>,
    /// Output cost per million tokens (USD). Null for local/free models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_cost_per_million: Option<f64>,
}

/// Model capabilities.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ModelCapabilities {
    pub vision: bool,
    pub tools: bool,
    /// Whether the model supports parallel tool calls.
    /// Some models support tools but not parallel execution.
    #[serde(default)]
    pub parallel_tools: bool,
    pub streaming: bool,
    pub json_mode: bool,
}

impl ModelsCli {
    /// Run the models command.
    pub async fn run(self) -> Result<()> {
        match self.subcommand {
            Some(ModelsSubcommand::List(args)) => {
                run_list(
                    args.provider,
                    args.json,
                    args.limit,
                    args.offset,
                    &args.sort,
                    args.full,
                )
                .await
            }
            None => {
                // Default: list models with optional provider filter (no pagination)
                run_list(self.provider, self.json, None, 0, "id", false).await
            }
        }
    }
}

/// Get all available models.
fn get_available_models() -> Vec<ModelInfo> {
    vec![
        // Anthropic models
        ModelInfo {
            id: "claude-sonnet-4-20250514".to_string(),
            name: "Claude Sonnet 4".to_string(),
            provider: "anthropic".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(3.0),
            output_cost_per_million: Some(15.0),
        },
        ModelInfo {
            id: "claude-opus-4-20250514".to_string(),
            name: "Claude Opus 4".to_string(),
            provider: "anthropic".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(15.0),
            output_cost_per_million: Some(75.0),
        },
        ModelInfo {
            id: "claude-3-5-sonnet-20241022".to_string(),
            name: "Claude 3.5 Sonnet".to_string(),
            provider: "anthropic".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(3.0),
            output_cost_per_million: Some(15.0),
        },
        ModelInfo {
            id: "claude-3-5-haiku-20241022".to_string(),
            name: "Claude 3.5 Haiku".to_string(),
            provider: "anthropic".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(0.80),
            output_cost_per_million: Some(4.0),
        },
        // OpenAI models
        ModelInfo {
            id: "gpt-4o".to_string(),
            name: "GPT-4o".to_string(),
            provider: "openai".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(2.50),
            output_cost_per_million: Some(10.0),
        },
        ModelInfo {
            id: "gpt-4o-mini".to_string(),
            name: "GPT-4o Mini".to_string(),
            provider: "openai".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(0.15),
            output_cost_per_million: Some(0.60),
        },
        ModelInfo {
            id: "o1".to_string(),
            name: "O1".to_string(),
            provider: "openai".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: false, // O1 does not support parallel tool calls
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(15.0),
            output_cost_per_million: Some(60.0),
        },
        ModelInfo {
            id: "o1-mini".to_string(),
            name: "O1 Mini".to_string(),
            provider: "openai".to_string(),
            capabilities: ModelCapabilities {
                vision: false,
                tools: true,
                parallel_tools: false, // O1 Mini does not support parallel tool calls
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(3.0),
            output_cost_per_million: Some(12.0),
        },
        ModelInfo {
            id: "o3-mini".to_string(),
            name: "O3 Mini".to_string(),
            provider: "openai".to_string(),
            capabilities: ModelCapabilities {
                vision: false,
                tools: true,
                parallel_tools: false, // O3 Mini does not support parallel tool calls
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(1.10),
            output_cost_per_million: Some(4.40),
        },
        // Google models
        ModelInfo {
            id: "gemini-2.0-flash".to_string(),
            name: "Gemini 2.0 Flash".to_string(),
            provider: "google".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(0.075),
            output_cost_per_million: Some(0.30),
        },
        ModelInfo {
            id: "gemini-2.0-flash-thinking".to_string(),
            name: "Gemini 2.0 Flash Thinking".to_string(),
            provider: "google".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(0.075),
            output_cost_per_million: Some(0.30),
        },
        ModelInfo {
            id: "gemini-1.5-pro".to_string(),
            name: "Gemini 1.5 Pro".to_string(),
            provider: "google".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(1.25),
            output_cost_per_million: Some(5.0),
        },
        // Groq models
        ModelInfo {
            id: "llama-3.3-70b-versatile".to_string(),
            name: "Llama 3.3 70B".to_string(),
            provider: "groq".to_string(),
            capabilities: ModelCapabilities {
                vision: false,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(0.59),
            output_cost_per_million: Some(0.79),
        },
        ModelInfo {
            id: "llama-3.1-8b-instant".to_string(),
            name: "Llama 3.1 8B".to_string(),
            provider: "groq".to_string(),
            capabilities: ModelCapabilities {
                vision: false,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(0.05),
            output_cost_per_million: Some(0.08),
        },
        // Mistral models
        ModelInfo {
            id: "mistral-large-latest".to_string(),
            name: "Mistral Large".to_string(),
            provider: "mistral".to_string(),
            capabilities: ModelCapabilities {
                vision: false,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(2.0),
            output_cost_per_million: Some(6.0),
        },
        ModelInfo {
            id: "codestral-latest".to_string(),
            name: "Codestral".to_string(),
            provider: "mistral".to_string(),
            capabilities: ModelCapabilities {
                vision: false,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(0.20),
            output_cost_per_million: Some(0.60),
        },
        // xAI models
        ModelInfo {
            id: "grok-2".to_string(),
            name: "Grok 2".to_string(),
            provider: "xai".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(2.0),
            output_cost_per_million: Some(10.0),
        },
        // DeepSeek models
        ModelInfo {
            id: "deepseek-chat".to_string(),
            name: "DeepSeek Chat".to_string(),
            provider: "deepseek".to_string(),
            capabilities: ModelCapabilities {
                vision: false,
                tools: true,
                parallel_tools: false, // DeepSeek has limited parallel tool support
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(0.14),
            output_cost_per_million: Some(0.28),
        },
        ModelInfo {
            id: "deepseek-reasoner".to_string(),
            name: "DeepSeek Reasoner".to_string(),
            provider: "deepseek".to_string(),
            capabilities: ModelCapabilities {
                vision: false,
                tools: true,
                parallel_tools: false, // DeepSeek Reasoner has limited parallel tool support
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(0.55),
            output_cost_per_million: Some(2.19),
        },
    ]
}

async fn run_list(
    provider_filter: Option<String>,
    json: bool,
    limit: Option<usize>,
    offset: usize,
    sort_by: &str,
    show_full: bool,
) -> Result<()> {
    let mut models = get_available_models();

    // Parse sort order (Issue #1993)
    let sort_order: ModelSortOrder = sort_by.parse().unwrap_or_default();

    // Issue #2323: Sort models with stable ordering to prevent duplicates/missing
    // models when paginating. All sort modes use secondary sort by ID to ensure
    // consistent ordering across paginated requests.
    match sort_order {
        ModelSortOrder::Id => {
            // Primary sort by ID ensures unique ordering
            models.sort_by(|a, b| a.id.cmp(&b.id));
        }
        ModelSortOrder::Name => {
            // Sort by name, then by ID for stable ordering when names are equal
            models.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        }
        ModelSortOrder::Provider => {
            // Sort by provider, then by id for stable ordering
            models.sort_by(|a, b| a.provider.cmp(&b.provider).then_with(|| a.id.cmp(&b.id)));
        }
    }

    // Filter by provider if specified
    let filtered: Vec<_> = if let Some(ref provider) = provider_filter {
        let provider_lower = provider.to_lowercase();
        models
            .into_iter()
            .filter(|m| m.provider.to_lowercase().contains(&provider_lower))
            .collect()
    } else {
        models
    };

    let total_count = filtered.len();

    // Apply pagination
    let paginated: Vec<_> = filtered.into_iter().skip(offset).collect();
    let paginated: Vec<_> = if let Some(limit) = limit {
        paginated.into_iter().take(limit).collect()
    } else {
        paginated
    };

    if json {
        // For JSON output, include pagination metadata
        let output = serde_json::json!({
            "models": paginated,
            "pagination": {
                "total": total_count,
                "offset": offset,
                "limit": limit,
                "count": paginated.len(),
            }
        });
        let json_output = serde_json::to_string_pretty(&output)?;
        println!("{json_output}");
        return Ok(());
    }

    if paginated.is_empty() {
        if let Some(provider) = provider_filter {
            println!("No models found for provider '{provider}'.");
        } else if offset > 0 {
            println!("No models at offset {offset} (total: {total_count}).");
        } else {
            println!("No models available.");
        }
        return Ok(());
    }

    // Group by provider
    let mut by_provider: std::collections::BTreeMap<String, Vec<&ModelInfo>> =
        std::collections::BTreeMap::new();

    for model in &paginated {
        by_provider
            .entry(model.provider.clone())
            .or_default()
            .push(model);
    }

    // Print header
    println!("Available Models:");
    println!("{}", "=".repeat(80));

    // Determine column width for model ID (Issue #1991)
    // If --full flag is used, calculate max width; otherwise use 35 chars with truncation
    let id_col_width = if show_full {
        // Find the longest model ID
        paginated
            .iter()
            .map(|m| m.id.len())
            .max()
            .unwrap_or(35)
            .max(10) // minimum width
    } else {
        35
    };

    for (provider, models) in by_provider {
        println!("\n{} ({} models)", provider, models.len());
        println!("{}", "-".repeat(40));
        println!(
            "{:<width$} {:^6} {:^12} {:^8} {:^6}",
            "Model ID",
            "Vision",
            "Tools",
            "Stream",
            "JSON",
            width = id_col_width
        );
        println!("{}", "-".repeat(id_col_width + 41));

        for model in models {
            let vision = if model.capabilities.vision {
                "✓"
            } else {
                "-"
            };
            // Show tools capability with parallel support indicator
            let tools = if model.capabilities.tools {
                if model.capabilities.parallel_tools {
                    "✓ (parallel)"
                } else {
                    "✓ (serial)"
                }
            } else {
                "-"
            };
            let streaming = if model.capabilities.streaming {
                "✓"
            } else {
                "-"
            };
            let json_mode = if model.capabilities.json_mode {
                "✓"
            } else {
                "-"
            };

            // Display model ID - truncate if not using --full flag (Issue #1991)
            let display_id = if show_full {
                model.id.clone()
            } else if model.id.len() > 32 {
                format!("{}...", &model.id[..29])
            } else {
                model.id.clone()
            };

            println!(
                "{:<width$} {:^6} {:^12} {:^8} {:^6}",
                display_id,
                vision,
                tools,
                streaming,
                json_mode,
                width = id_col_width
            );
        }
    }

    println!();
    println!("Use `cortex --model <model-id>` to use a specific model.");
    if !show_full {
        println!("Use `cortex models list --full` to show full model IDs.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==========================================================================
    // ModelSortOrder tests
    // ==========================================================================

    #[test]
    fn test_model_sort_order_from_str_id() {
        let result: ModelSortOrder = "id".parse().expect("parsing 'id' should succeed");
        assert_eq!(result, ModelSortOrder::Id);
    }

    #[test]
    fn test_model_sort_order_from_str_name() {
        let result: ModelSortOrder = "name".parse().expect("parsing 'name' should succeed");
        assert_eq!(result, ModelSortOrder::Name);
    }

    #[test]
    fn test_model_sort_order_from_str_provider() {
        let result: ModelSortOrder = "provider"
            .parse()
            .expect("parsing 'provider' should succeed");
        assert_eq!(result, ModelSortOrder::Provider);
    }

    #[test]
    fn test_model_sort_order_from_str_case_insensitive() {
        let upper: ModelSortOrder = "ID".parse().expect("parsing 'ID' should succeed");
        assert_eq!(upper, ModelSortOrder::Id);

        let mixed: ModelSortOrder = "NaMe".parse().expect("parsing 'NaMe' should succeed");
        assert_eq!(mixed, ModelSortOrder::Name);

        let caps: ModelSortOrder = "PROVIDER"
            .parse()
            .expect("parsing 'PROVIDER' should succeed");
        assert_eq!(caps, ModelSortOrder::Provider);
    }

    #[test]
    fn test_model_sort_order_from_str_invalid() {
        let result: Result<ModelSortOrder, String> = "invalid".parse();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(
            err.contains("Invalid sort order"),
            "Error should mention 'Invalid sort order'"
        );
        assert!(
            err.contains("invalid"),
            "Error should include the invalid value"
        );
    }

    #[test]
    fn test_model_sort_order_default() {
        let default = ModelSortOrder::default();
        assert_eq!(default, ModelSortOrder::Id);
    }

    // ==========================================================================
    // ModelCapabilities tests
    // ==========================================================================

    #[test]
    fn test_model_capabilities_default() {
        let caps = ModelCapabilities::default();
        assert!(!caps.vision);
        assert!(!caps.tools);
        assert!(!caps.parallel_tools);
        assert!(!caps.streaming);
        assert!(!caps.json_mode);
    }

    #[test]
    fn test_model_capabilities_serialization() {
        let caps = ModelCapabilities {
            vision: true,
            tools: true,
            parallel_tools: true,
            streaming: true,
            json_mode: true,
        };

        let json = serde_json::to_string(&caps).expect("serialization should succeed");
        assert!(json.contains("\"vision\":true"));
        assert!(json.contains("\"tools\":true"));
        assert!(json.contains("\"parallel_tools\":true"));
        assert!(json.contains("\"streaming\":true"));
        assert!(json.contains("\"json_mode\":true"));
    }

    #[test]
    fn test_model_capabilities_serialization_default_values() {
        let caps = ModelCapabilities {
            vision: false,
            tools: false,
            parallel_tools: false,
            streaming: false,
            json_mode: false,
        };

        let json = serde_json::to_string(&caps).expect("serialization should succeed");
        assert!(json.contains("\"vision\":false"));
        assert!(json.contains("\"tools\":false"));
        assert!(json.contains("\"parallel_tools\":false"));
        assert!(json.contains("\"streaming\":false"));
        assert!(json.contains("\"json_mode\":false"));
    }

    // ==========================================================================
    // ModelInfo tests
    // ==========================================================================

    #[test]
    fn test_model_info_serialization_with_costs() {
        let model = ModelInfo {
            id: "test-model-id".to_string(),
            name: "Test Model".to_string(),
            provider: "test-provider".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: false,
                streaming: true,
                json_mode: false,
            },
            input_cost_per_million: Some(3.0),
            output_cost_per_million: Some(15.0),
        };

        let json = serde_json::to_string_pretty(&model).expect("serialization should succeed");

        assert!(json.contains("\"id\": \"test-model-id\""));
        assert!(json.contains("\"name\": \"Test Model\""));
        assert!(json.contains("\"provider\": \"test-provider\""));
        assert!(json.contains("\"input_cost_per_million\": 3.0"));
        assert!(json.contains("\"output_cost_per_million\": 15.0"));
    }

    #[test]
    fn test_model_info_serialization_without_costs() {
        let model = ModelInfo {
            id: "local-model".to_string(),
            name: "Local Model".to_string(),
            provider: "local".to_string(),
            capabilities: ModelCapabilities::default(),
            input_cost_per_million: None,
            output_cost_per_million: None,
        };

        let json = serde_json::to_string(&model).expect("serialization should succeed");

        // Costs should be skipped when None (skip_serializing_if)
        assert!(
            !json.contains("input_cost_per_million"),
            "input_cost should be skipped when None"
        );
        assert!(
            !json.contains("output_cost_per_million"),
            "output_cost should be skipped when None"
        );
    }

    #[test]
    fn test_model_info_clone() {
        let model = ModelInfo {
            id: "clone-test".to_string(),
            name: "Clone Test".to_string(),
            provider: "test".to_string(),
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                parallel_tools: true,
                streaming: true,
                json_mode: true,
            },
            input_cost_per_million: Some(1.5),
            output_cost_per_million: Some(7.5),
        };

        let cloned = model.clone();
        assert_eq!(cloned.id, model.id);
        assert_eq!(cloned.name, model.name);
        assert_eq!(cloned.provider, model.provider);
        assert_eq!(cloned.capabilities.vision, model.capabilities.vision);
        assert_eq!(cloned.input_cost_per_million, model.input_cost_per_million);
        assert_eq!(
            cloned.output_cost_per_million,
            model.output_cost_per_million
        );
    }

    // ==========================================================================
    // get_available_models tests
    // ==========================================================================

    #[test]
    fn test_get_available_models_not_empty() {
        let models = get_available_models();
        assert!(!models.is_empty(), "Available models should not be empty");
    }

    #[test]
    fn test_get_available_models_has_anthropic() {
        let models = get_available_models();
        let anthropic_models: Vec<_> = models
            .iter()
            .filter(|m| m.provider == "anthropic")
            .collect();
        assert!(!anthropic_models.is_empty(), "Should have Anthropic models");
    }

    #[test]
    fn test_get_available_models_has_openai() {
        let models = get_available_models();
        let openai_models: Vec<_> = models.iter().filter(|m| m.provider == "openai").collect();
        assert!(!openai_models.is_empty(), "Should have OpenAI models");
    }

    #[test]
    fn test_get_available_models_has_google() {
        let models = get_available_models();
        let google_models: Vec<_> = models.iter().filter(|m| m.provider == "google").collect();
        assert!(!google_models.is_empty(), "Should have Google models");
    }

    #[test]
    fn test_get_available_models_unique_ids() {
        let models = get_available_models();
        let mut seen_ids = std::collections::HashSet::new();

        for model in &models {
            assert!(
                seen_ids.insert(&model.id),
                "Duplicate model ID found: {}",
                model.id
            );
        }
    }

    #[test]
    fn test_get_available_models_all_have_required_fields() {
        let models = get_available_models();
        for model in &models {
            assert!(!model.id.is_empty(), "Model ID should not be empty");
            assert!(!model.name.is_empty(), "Model name should not be empty");
            assert!(
                !model.provider.is_empty(),
                "Model provider should not be empty"
            );
        }
    }

    #[test]
    fn test_get_available_models_o1_no_parallel_tools() {
        let models = get_available_models();

        // O1 and O1 Mini should not support parallel tools
        for model in models.iter().filter(|m| m.id.starts_with("o1")) {
            assert!(
                !model.capabilities.parallel_tools,
                "O1 model {} should not support parallel tools",
                model.id
            );
        }
    }

    #[test]
    fn test_get_available_models_claude_supports_vision() {
        let models = get_available_models();

        for model in models.iter().filter(|m| m.id.starts_with("claude")) {
            assert!(
                model.capabilities.vision,
                "Claude model {} should support vision",
                model.id
            );
        }
    }

    // ==========================================================================
    // CLI argument struct tests
    // ==========================================================================

    #[test]
    fn test_list_models_args_default_offset() {
        use clap::Parser;

        // Parse with minimal args
        let args = ListModelsArgs::try_parse_from(["list"]).expect("parsing should succeed");
        assert_eq!(args.offset, 0);
        assert_eq!(args.sort, "id");
        assert!(!args.full);
        assert!(!args.json);
        assert!(args.provider.is_none());
        assert!(args.limit.is_none());
    }

    #[test]
    fn test_list_models_args_with_options() {
        use clap::Parser;

        let args = ListModelsArgs::try_parse_from([
            "list",
            "--json",
            "--limit",
            "10",
            "--offset",
            "5",
            "--sort",
            "name",
            "--full",
            "anthropic",
        ])
        .expect("parsing should succeed");

        assert!(args.json);
        assert_eq!(args.limit, Some(10));
        assert_eq!(args.offset, 5);
        assert_eq!(args.sort, "name");
        assert!(args.full);
        assert_eq!(args.provider, Some("anthropic".to_string()));
    }

    #[test]
    fn test_models_cli_parsing() {
        use clap::Parser;

        let cli = ModelsCli::try_parse_from(["models"]).expect("parsing should succeed");
        assert!(cli.subcommand.is_none());
        assert!(cli.provider.is_none());
        assert!(!cli.json);
    }

    #[test]
    fn test_models_cli_with_provider() {
        use clap::Parser;

        let cli = ModelsCli::try_parse_from(["models", "openai"]).expect("parsing should succeed");
        assert_eq!(cli.provider, Some("openai".to_string()));
    }

    #[test]
    fn test_models_cli_with_json_flag() {
        use clap::Parser;

        let cli = ModelsCli::try_parse_from(["models", "--json"]).expect("parsing should succeed");
        assert!(cli.json);
    }
}
