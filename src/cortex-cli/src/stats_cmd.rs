//! Stats command - show usage statistics.
//!
//! Provides functionality to:
//! - Show token usage statistics
//! - Calculate costs per provider/model
//! - Show tool usage breakdown
//! - Filter by date range

use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;

/// Validate that --days is within the allowed range (1-3650).
/// 3650 days = ~10 years, which is a reasonable upper bound.
fn validate_days_range(s: &str) -> Result<u32, String> {
    let days: u32 = s
        .parse()
        .map_err(|_| format!("'{}' is not a valid number", s))?;

    if days == 0 {
        return Err("--days must be at least 1".to_string());
    }

    if days > 3650 {
        return Err(format!(
            "--days cannot exceed 3650 (approximately 10 years). Got: {}",
            days
        ));
    }

    Ok(days)
}

/// Stats CLI.
#[derive(Debug, Parser)]
pub struct StatsCli {
    /// Number of days to include (default: 30, range: 1-3650)
    #[arg(long, short = 'd', default_value = "30", value_parser = validate_days_range)]
    pub days: u32,

    /// Filter by specific provider
    #[arg(long, short = 'p')]
    pub provider: Option<String>,

    /// Filter by specific model
    #[arg(long, short = 'm')]
    pub model: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Show detailed breakdown
    #[arg(long, short = 'v')]
    pub verbose: bool,
}

/// Usage statistics.
#[derive(Debug, Default, serde::Serialize)]
pub struct UsageStats {
    pub total_sessions: u64,
    pub total_messages: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub estimated_cost_usd: f64,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub by_provider: HashMap<String, ProviderStats>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub by_model: HashMap<String, ModelStats>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub tool_usage: HashMap<String, u64>,
    pub date_range: DateRange,
}

/// Per-provider statistics.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ProviderStats {
    pub sessions: u64,
    pub messages: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
}

/// Per-model statistics.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ModelStats {
    pub provider: String,
    pub sessions: u64,
    pub messages: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
}

/// Date range for statistics.
#[derive(Debug, Default, serde::Serialize)]
pub struct DateRange {
    pub start: String,
    pub end: String,
    pub days: u32,
}

/// Pricing information per 1M tokens.
#[derive(Debug, Clone)]
struct ModelPricing {
    input_per_million: f64,
    output_per_million: f64,
}

/// Custom pricing configuration loaded from config file or environment.
/// This allows users to override default pricing when provider prices change.
fn load_custom_pricing() -> std::collections::HashMap<String, ModelPricing> {
    let mut custom = std::collections::HashMap::new();

    // Try to load from environment variables in format:
    // CORTEX_PRICING_<MODEL>=<input_price>,<output_price>
    // Example: CORTEX_PRICING_GPT4O=2.5,10.0
    for (key, value) in std::env::vars() {
        if let Some(model_suffix) = key.strip_prefix("CORTEX_PRICING_") {
            let model_name = model_suffix.to_lowercase().replace('_', "-");
            let parts: Vec<&str> = value.split(',').collect();
            if parts.len() == 2
                && let (Ok(input), Ok(output)) = (
                    parts[0].trim().parse::<f64>(),
                    parts[1].trim().parse::<f64>(),
                )
            {
                custom.insert(
                    model_name,
                    ModelPricing {
                        input_per_million: input,
                        output_per_million: output,
                    },
                );
            }
        }
    }

    custom
}

impl StatsCli {
    /// Run the stats command.
    pub async fn run(self) -> Result<()> {
        let cortex_home = get_cortex_home();
        let sessions_dir = cortex_home.join("sessions");

        if !sessions_dir.exists() {
            if self.json {
                // Output empty stats as valid JSON
                let now = chrono::Utc::now();
                let start_date = now - chrono::Duration::days(self.days as i64);

                let stats = UsageStats {
                    date_range: DateRange {
                        start: start_date.format("%Y-%m-%d").to_string(),
                        end: now.format("%Y-%m-%d").to_string(),
                        days: self.days,
                    },
                    ..Default::default()
                };

                let json_output = serde_json::to_string_pretty(&stats)?;
                println!("{json_output}");
            } else {
                println!("No sessions found. Start using Cortex to generate statistics!");
                println!();
                println!("The stats command will track:");
                println!("  - Session counts and message totals");
                println!("  - Token usage (input and output tokens)");
                println!("  - Estimated costs by provider and model");
                println!("  - Tool call frequency");
            }
            return Ok(());
        }

        let stats = collect_stats(&sessions_dir, &self).await?;

        if self.json {
            let json_output = serde_json::to_string_pretty(&stats)?;
            println!("{json_output}");
            return Ok(());
        }

        print_stats(&stats, self.verbose);
        Ok(())
    }
}

/// Get the cortex home directory.
fn get_cortex_home() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".cortex"))
        .unwrap_or_else(|| PathBuf::from(".cortex"))
}

/// Get pricing for a model.
/// Checks custom pricing from environment first, then falls back to defaults.
fn get_model_pricing(model: &str) -> ModelPricing {
    // First check for custom pricing from environment
    let custom_pricing = load_custom_pricing();
    let model_lower = model.to_lowercase();

    // Check for exact match in custom pricing
    if let Some(pricing) = custom_pricing.get(&model_lower) {
        return pricing.clone();
    }

    // Check for partial match in custom pricing (e.g., "gpt-4o" matches "gpt-4o-mini")
    for (key, pricing) in &custom_pricing {
        if model_lower.contains(key) {
            return pricing.clone();
        }
    }

    // Fall back to default pricing (may be outdated - users can override via CORTEX_PRICING_*)
    // Pricing per 1M tokens (as of late 2024/early 2025 - may change)
    match model {
        // Anthropic
        m if m.contains("claude-opus-4") || m.contains("opus-4") => ModelPricing {
            input_per_million: 15.0,
            output_per_million: 75.0,
        },
        m if m.contains("claude-sonnet-4") || m.contains("sonnet-4") => ModelPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
        },
        m if m.contains("claude-3-5-sonnet") => ModelPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
        },
        m if m.contains("claude-3-5-haiku") || m.contains("haiku") => ModelPricing {
            input_per_million: 0.80,
            output_per_million: 4.0,
        },
        // OpenAI
        m if m.contains("gpt-4o-mini") => ModelPricing {
            input_per_million: 0.15,
            output_per_million: 0.60,
        },
        m if m.contains("gpt-4o") => ModelPricing {
            input_per_million: 2.50,
            output_per_million: 10.0,
        },
        m if m.contains("o1-mini") => ModelPricing {
            input_per_million: 3.0,
            output_per_million: 12.0,
        },
        m if m.contains("o1") || m.contains("o3") => ModelPricing {
            input_per_million: 15.0,
            output_per_million: 60.0,
        },
        // Google
        m if m.contains("gemini-2.0-flash") => ModelPricing {
            input_per_million: 0.075,
            output_per_million: 0.30,
        },
        m if m.contains("gemini-1.5-pro") => ModelPricing {
            input_per_million: 1.25,
            output_per_million: 5.0,
        },
        // Groq (free tier / very cheap)
        m if m.contains("llama") && m.contains("groq") => ModelPricing {
            input_per_million: 0.05,
            output_per_million: 0.08,
        },
        // Mistral
        m if m.contains("mistral-large") => ModelPricing {
            input_per_million: 2.0,
            output_per_million: 6.0,
        },
        m if m.contains("codestral") => ModelPricing {
            input_per_million: 0.20,
            output_per_million: 0.60,
        },
        // DeepSeek (very cheap)
        m if m.contains("deepseek") => ModelPricing {
            input_per_million: 0.14,
            output_per_million: 0.28,
        },
        // xAI
        m if m.contains("grok") => ModelPricing {
            input_per_million: 2.0,
            output_per_million: 10.0,
        },
        // Default (conservative estimate)
        _ => ModelPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
        },
    }
}

/// Calculate cost for token usage.
fn calculate_cost(model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    let pricing = get_model_pricing(model);
    let input_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input_per_million;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * pricing.output_per_million;
    input_cost + output_cost
}

/// Infer provider from model name.
fn infer_provider(model: &str) -> String {
    let model_lower = model.to_lowercase();
    if model_lower.contains("claude") {
        "anthropic".to_string()
    } else if model_lower.contains("gpt")
        || model_lower.contains("o1")
        || model_lower.contains("o3")
    {
        "openai".to_string()
    } else if model_lower.contains("gemini") {
        "google".to_string()
    } else if model_lower.contains("llama") {
        "groq".to_string()
    } else if model_lower.contains("mistral") || model_lower.contains("codestral") {
        "mistral".to_string()
    } else if model_lower.contains("deepseek") {
        "deepseek".to_string()
    } else if model_lower.contains("grok") {
        "xai".to_string()
    } else if model_lower.contains("qwen") {
        "deepseek".to_string() // Qwen models often available via DeepSeek
    } else {
        "unknown".to_string()
    }
}

/// Collect statistics from session files.
async fn collect_stats(sessions_dir: &PathBuf, cli: &StatsCli) -> Result<UsageStats> {
    let mut stats = UsageStats::default();

    // Calculate date range
    let now = chrono::Utc::now();
    let start_date = now - chrono::Duration::days(cli.days as i64);

    stats.date_range = DateRange {
        start: start_date.format("%Y-%m-%d").to_string(),
        end: now.format("%Y-%m-%d").to_string(),
        days: cli.days,
    };

    // Read session files
    let entries = std::fs::read_dir(sessions_dir).context("Failed to read sessions directory")?;

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip if not a session file
        if !path.is_file() {
            continue;
        }

        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if !filename.ends_with(".json") {
            continue;
        }

        // Try to parse the session
        if let Ok(session_data) = parse_session_file(&path) {
            // Check if session is within date range
            if let Some(ref timestamp) = session_data.timestamp
                && let Ok(session_date) = chrono::DateTime::parse_from_rfc3339(timestamp)
                && session_date < start_date
            {
                continue;
            }

            let model = session_data.model.as_deref().unwrap_or("unknown");
            let provider = infer_provider(model);

            // Apply filters
            if let Some(ref filter_provider) = cli.provider
                && !provider.eq_ignore_ascii_case(filter_provider)
            {
                continue;
            }

            if let Some(ref filter_model) = cli.model
                && !model.to_lowercase().contains(&filter_model.to_lowercase())
            {
                continue;
            }

            // Aggregate statistics
            stats.total_sessions += 1;
            stats.total_messages += session_data.message_count;
            stats.input_tokens += session_data.input_tokens;
            stats.output_tokens += session_data.output_tokens;
            stats.total_tokens += session_data.input_tokens + session_data.output_tokens;

            let session_cost =
                calculate_cost(model, session_data.input_tokens, session_data.output_tokens);
            stats.estimated_cost_usd += session_cost;

            // Per-provider stats
            let provider_stats = stats.by_provider.entry(provider.clone()).or_default();
            provider_stats.sessions += 1;
            provider_stats.messages += session_data.message_count;
            provider_stats.input_tokens += session_data.input_tokens;
            provider_stats.output_tokens += session_data.output_tokens;
            provider_stats.estimated_cost_usd += session_cost;

            // Per-model stats
            let model_stats =
                stats
                    .by_model
                    .entry(model.to_string())
                    .or_insert_with(|| ModelStats {
                        provider: provider.clone(),
                        ..Default::default()
                    });
            model_stats.sessions += 1;
            model_stats.messages += session_data.message_count;
            model_stats.input_tokens += session_data.input_tokens;
            model_stats.output_tokens += session_data.output_tokens;
            model_stats.estimated_cost_usd += session_cost;

            // Tool usage
            for (tool, count) in session_data.tool_usage {
                *stats.tool_usage.entry(tool).or_default() += count;
            }
        }
    }

    Ok(stats)
}

/// Session data extracted from file.
#[derive(Debug, Default)]
struct SessionData {
    timestamp: Option<String>,
    model: Option<String>,
    message_count: u64,
    input_tokens: u64,
    output_tokens: u64,
    tool_usage: HashMap<String, u64>,
}

/// Parse a session file to extract statistics.
fn parse_session_file(path: &PathBuf) -> Result<SessionData> {
    let content = std::fs::read_to_string(path)?;
    let json: serde_json::Value = serde_json::from_str(&content)?;

    let timestamp = json
        .get("timestamp")
        .or_else(|| json.get("created_at"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let model = json.get("model").and_then(|v| v.as_str()).map(String::from);

    let mut data = SessionData {
        timestamp,
        model,
        ..Default::default()
    };

    // Track message IDs we've already counted tokens for to prevent double-counting
    let mut counted_message_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    // Count messages
    if let Some(messages) = json.get("messages").and_then(|v| v.as_array()) {
        data.message_count = messages.len() as u64;

        for msg in messages {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            let msg_id = msg
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default();

            // Skip tool result messages to avoid double-counting tokens
            // Tool results are often just echoes of the tool call with the response
            // The actual token usage should be counted from the assistant message that triggered the tool
            if role == "tool" {
                // Only count tool usage statistics, not tokens
                if let Some(name) = msg.get("name").and_then(|v| v.as_str()) {
                    *data.tool_usage.entry(name.to_string()).or_default() += 1;
                }
                continue;
            }

            // Extract token usage from message metadata (only if not already counted)
            if let Some(usage) = msg.get("usage") {
                // Use message ID or generate a unique key to prevent double-counting
                let count_key = if !msg_id.is_empty() {
                    msg_id.clone()
                } else {
                    // Fallback: use message index position (less reliable but better than nothing)
                    format!("msg_{}", data.message_count)
                };

                if !counted_message_ids.contains(&count_key) {
                    data.input_tokens += usage
                        .get("input_tokens")
                        .or_else(|| usage.get("prompt_tokens"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);

                    data.output_tokens += usage
                        .get("output_tokens")
                        .or_else(|| usage.get("completion_tokens"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);

                    counted_message_ids.insert(count_key);
                }
            }

            // Count tool calls (for tool usage statistics only)
            if let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                for tc in tool_calls {
                    if let Some(name) = tc
                        .get("name")
                        .or_else(|| tc.get("function").and_then(|f| f.get("name")))
                        .and_then(|v| v.as_str())
                    {
                        *data.tool_usage.entry(name.to_string()).or_default() += 1;
                    }
                }
            }
        }
    }

    // Also check for aggregate usage at session level
    // Prefer session-level aggregate if available as it's more accurate
    if let Some(usage) = json.get("usage") {
        let session_input = usage
            .get("input_tokens")
            .or_else(|| usage.get("prompt_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let session_output = usage
            .get("output_tokens")
            .or_else(|| usage.get("completion_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Use session-level usage if available (more accurate than per-message sum)
        // This prevents double-counting that can occur with tool calls
        if session_input > 0 || session_output > 0 {
            data.input_tokens = session_input;
            data.output_tokens = session_output;
        }
    }

    Ok(data)
}

/// Print statistics to console.
fn print_stats(stats: &UsageStats, verbose: bool) {
    println!("Cortex Usage Statistics");
    println!("{}", "=".repeat(60));
    println!(
        "Period: {} to {} ({} days)",
        stats.date_range.start, stats.date_range.end, stats.date_range.days
    );
    println!();

    // Summary
    println!("Summary");
    println!("{}", "-".repeat(40));
    println!(
        "  Sessions:      {:>12}",
        format_number(stats.total_sessions)
    );
    println!(
        "  Messages:      {:>12}",
        format_number(stats.total_messages)
    );
    println!("  Input Tokens:  {:>12}", format_number(stats.input_tokens));
    println!(
        "  Output Tokens: {:>12}",
        format_number(stats.output_tokens)
    );
    println!("  Total Tokens:  {:>12}", format_number(stats.total_tokens));
    println!(
        "  Est. Cost:     {:>12}",
        format_cost(stats.estimated_cost_usd)
    );
    println!();

    // By provider
    if !stats.by_provider.is_empty() {
        println!("By Provider");
        println!("{}", "-".repeat(40));
        println!(
            "  {:<15} {:>10} {:>12} {:>10}",
            "Provider", "Sessions", "Tokens", "Cost"
        );

        let mut providers: Vec<_> = stats.by_provider.iter().collect();
        providers.sort_by(|a, b| {
            b.1.estimated_cost_usd
                .partial_cmp(&a.1.estimated_cost_usd)
                .unwrap()
        });

        for (provider, pstats) in providers {
            let total_tokens = pstats.input_tokens + pstats.output_tokens;
            println!(
                "  {:<15} {:>10} {:>12} {:>10}",
                provider,
                format_number(pstats.sessions),
                format_number(total_tokens),
                format_cost(pstats.estimated_cost_usd)
            );
        }
        println!();
    }

    // By model (if verbose)
    if verbose && !stats.by_model.is_empty() {
        println!("By Model");
        println!("{}", "-".repeat(60));
        println!("  {:<35} {:>8} {:>10}", "Model", "Sessions", "Cost");

        let mut models: Vec<_> = stats.by_model.iter().collect();
        models.sort_by(|a, b| {
            b.1.estimated_cost_usd
                .partial_cmp(&a.1.estimated_cost_usd)
                .unwrap()
        });

        for (model, mstats) in models.iter().take(10) {
            let display_name = if model.len() > 35 {
                format!("{}...", &model[..32])
            } else {
                model.to_string()
            };
            println!(
                "  {:<35} {:>8} {:>10}",
                display_name,
                format_number(mstats.sessions),
                format_cost(mstats.estimated_cost_usd)
            );
        }
        if models.len() > 10 {
            println!("  ... and {} more models", models.len() - 10);
        }
        println!();
    }

    // Tool usage
    if !stats.tool_usage.is_empty() {
        println!("Tool Usage");
        println!("{}", "-".repeat(40));

        let mut tools: Vec<_> = stats.tool_usage.iter().collect();
        tools.sort_by(|a, b| b.1.cmp(a.1));

        for (tool, count) in tools.iter().take(10) {
            println!("  {:<25} {:>10} calls", tool, format_number(**count));
        }
        if tools.len() > 10 {
            println!("  ... and {} more tools", tools.len() - 10);
        }
        println!();
    }

    if stats.total_sessions == 0 {
        println!("No sessions found in the specified time range.");
        println!("Try increasing --days or check if sessions exist.");
    }
}

/// Format a number with thousands separators.
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let chars: Vec<_> = s.chars().rev().collect();

    for (i, c) in chars.iter().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(*c);
    }

    result.chars().rev().collect()
}

/// Format a cost value.
fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("${:.4}", cost)
    } else {
        format!("${:.2}", cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1234567), "1,234,567");
    }

    #[test]
    fn test_format_cost() {
        assert_eq!(format_cost(0.0001), "$0.0001");
        assert_eq!(format_cost(0.50), "$0.50");
        assert_eq!(format_cost(12.34), "$12.34");
    }

    #[test]
    fn test_infer_provider() {
        assert_eq!(infer_provider("claude-sonnet-4"), "anthropic");
        assert_eq!(infer_provider("gpt-4o"), "openai");
        assert_eq!(infer_provider("gemini-2.0-flash"), "google");
        assert_eq!(infer_provider("deepseek-chat"), "deepseek");
    }

    #[test]
    fn test_calculate_cost() {
        // Claude Sonnet 4: $3/$15 per 1M
        let cost = calculate_cost("claude-sonnet-4", 1_000_000, 1_000_000);
        assert!((cost - 18.0).abs() < 0.001);

        // GPT-4o: $2.50/$10 per 1M
        let cost = calculate_cost("gpt-4o", 1_000_000, 1_000_000);
        assert!((cost - 12.5).abs() < 0.001);
    }

    #[test]
    fn test_validate_days_range() {
        // Valid values
        assert!(validate_days_range("1").is_ok());
        assert!(validate_days_range("30").is_ok());
        assert!(validate_days_range("365").is_ok());
        assert!(validate_days_range("3650").is_ok());

        // Invalid: zero
        let err = validate_days_range("0").unwrap_err();
        assert!(err.contains("at least 1"));

        // Invalid: exceeds max
        let err = validate_days_range("3651").unwrap_err();
        assert!(err.contains("cannot exceed 3650"));

        // Invalid: not a number
        let err = validate_days_range("abc").unwrap_err();
        assert!(err.contains("not a valid number"));
    }
}
