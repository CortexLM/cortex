//! Billing client for Cortex CLI.
//!
//! Provides access to billing, usage, and subscription information via the
//! Cortex backend API. Requires authentication via CORTEX_AUTH_TOKEN.
//!
//! # Endpoints
//! - GET /billing/usage - Current usage summary
//! - GET /billing/usage/models - Per-model usage breakdown
//! - GET /billing/subscription - Current subscription info
//! - GET /billing/limits - Rate and usage limits

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

use crate::api_client::create_default_client;
use crate::error::{CortexError, Result};

// ============================================================================
// CONFIGURATION
// ============================================================================

/// Default API URL for billing endpoints.
const DEFAULT_API_URL: &str = "https://api.cortex.foundation";

// ============================================================================
// RESPONSE TYPES
// ============================================================================

/// Quota information for the current billing period.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaInfo {
    /// Monthly token quota (if applicable).
    pub monthly_tokens: Option<u64>,

    /// Tokens used this period.
    pub tokens_used: u64,

    /// Tokens remaining (if quota set).
    pub tokens_remaining: Option<u64>,

    /// Monthly spend limit in USD (if applicable).
    pub monthly_spend_limit: Option<f64>,

    /// Amount spent this period in USD.
    pub amount_spent: f64,

    /// Spend remaining in USD (if limit set).
    pub spend_remaining: Option<f64>,

    /// Percentage of quota used.
    pub percent_used: f64,
}

/// Usage by model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    /// Model identifier.
    pub model: String,

    /// Provider name.
    pub provider: String,

    /// Total tokens (input + output).
    pub tokens: u64,

    /// Input tokens.
    pub input_tokens: u64,

    /// Output tokens.
    pub output_tokens: u64,

    /// Number of requests.
    pub requests: u64,

    /// Cost in USD.
    pub cost_usd: f64,
}

/// Usage summary for the billing period.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    /// Organization ID.
    pub org_id: uuid::Uuid,

    /// Billing period start.
    pub period_start: DateTime<Utc>,

    /// Billing period end.
    pub period_end: DateTime<Utc>,

    /// Total tokens used.
    pub total_tokens: u64,

    /// Input tokens.
    pub input_tokens: u64,

    /// Output tokens.
    pub output_tokens: u64,

    /// Total requests.
    pub total_requests: u64,

    /// Successful requests.
    pub successful_requests: u64,

    /// Total cost in USD.
    pub total_cost_usd: f64,

    /// Usage breakdown by model.
    pub by_model: Vec<ModelUsage>,

    /// Quota information (if applicable).
    pub quota: Option<QuotaInfo>,
}

/// Plan limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanLimits {
    /// Requests per minute.
    pub rpm: Option<u32>,

    /// Requests per day.
    pub rpd: Option<u32>,

    /// Tokens per minute.
    pub tpm: Option<u32>,

    /// Tokens per day.
    pub tpd: Option<u64>,

    /// Monthly token quota.
    pub monthly_tokens: Option<u64>,

    /// Monthly spend limit in USD.
    pub monthly_spend_usd: Option<f64>,
}

/// Plan details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanDetails {
    /// Plan identifier.
    pub id: String,

    /// Plan display name.
    pub name: String,

    /// Plan description.
    pub description: String,

    /// Monthly price in USD.
    pub price_usd_monthly: f64,

    /// List of plan features.
    pub features: Vec<String>,

    /// Plan limits.
    pub limits: PlanLimits,
}

/// Payment method info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentMethodInfo {
    /// Payment method ID.
    pub id: String,

    /// Card brand (e.g., "visa", "mastercard").
    pub brand: String,

    /// Last 4 digits.
    pub last_four: String,

    /// Expiration month.
    pub exp_month: u32,

    /// Expiration year.
    pub exp_year: u32,

    /// Whether this is the default payment method.
    pub is_default: bool,
}

/// Subscription information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionInfo {
    /// Subscription ID.
    pub id: Option<uuid::Uuid>,

    /// Stripe subscription ID.
    pub stripe_subscription_id: Option<String>,

    /// Plan details.
    pub plan: PlanDetails,

    /// Subscription status (active, canceled, past_due, etc.).
    pub status: String,

    /// Current period start.
    pub current_period_start: DateTime<Utc>,

    /// Current period end.
    pub current_period_end: DateTime<Utc>,

    /// Whether subscription will cancel at period end.
    pub cancel_at_period_end: bool,

    /// When subscription was canceled (if applicable).
    pub canceled_at: Option<DateTime<Utc>>,

    /// Trial end date (if applicable).
    pub trial_end: Option<DateTime<Utc>>,

    /// Default payment method.
    pub payment_method: Option<PaymentMethodInfo>,
}

/// Rate limits information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitsInfo {
    /// Requests per minute limit.
    pub rpm: Option<u32>,

    /// Requests per day limit.
    pub rpd: Option<u32>,

    /// Tokens per minute limit.
    pub tpm: Option<u32>,

    /// Tokens per day limit.
    pub tpd: Option<u64>,
}

/// Current usage against limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentLimitUsage {
    /// Requests this minute.
    pub requests_this_minute: u32,

    /// Requests today.
    pub requests_today: u32,

    /// Tokens this minute.
    pub tokens_this_minute: u32,

    /// Tokens today.
    pub tokens_today: u64,
}

/// Limits information (rate limits + current usage).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsInfo {
    /// Rate limits configuration.
    pub rate_limits: RateLimitsInfo,

    /// Current usage against limits.
    pub current_usage: CurrentLimitUsage,
}

/// Query parameters for usage endpoints.
#[derive(Debug, Clone, Default)]
pub struct UsageQuery {
    /// Start date (YYYY-MM-DD).
    pub start_date: Option<String>,

    /// End date (YYYY-MM-DD).
    pub end_date: Option<String>,

    /// Filter by model.
    pub model: Option<String>,
}

// ============================================================================
// CLIENT
// ============================================================================

/// Client for accessing billing and usage API endpoints.
pub struct BillingClient {
    client: Client,
    base_url: String,
    auth_token: Option<String>,
}

impl BillingClient {
    /// Creates a new billing client.
    ///
    /// Uses CORTEX_API_URL environment variable if set, otherwise defaults to
    /// https://api.cortex.foundation.
    pub fn new() -> Result<Self> {
        let base_url =
            std::env::var("CORTEX_API_URL").unwrap_or_else(|_| DEFAULT_API_URL.to_string());
        let client = create_default_client()?;

        Ok(Self {
            client,
            base_url,
            auth_token: None,
        })
    }

    /// Creates a billing client with explicit base URL.
    pub fn with_base_url(base_url: String) -> Result<Self> {
        let client = create_default_client()?;

        Ok(Self {
            client,
            base_url,
            auth_token: None,
        })
    }

    /// Sets the authentication token.
    pub fn with_auth_token(mut self, token: String) -> Self {
        self.auth_token = Some(token);
        self
    }

    /// Gets the auth token using centralized auth module.
    /// Supports both API key and OAuth login methods.
    fn get_auth_token(&self) -> Result<String> {
        crate::auth_token::get_auth_token(self.auth_token.as_deref())
    }

    /// Makes an authenticated GET request.
    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let token = self.get_auth_token()?;
        let url = format!("{}{}", self.base_url, path);

        debug!(url = %url, "Making billing API request");

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .send()
            .await
            .map_err(|e| {
                error!(error = %e, "Billing API request failed");
                CortexError::ConnectionFailed {
                    endpoint: url.clone(),
                    message: e.to_string(),
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %error_body, "Billing API error response");

            return match status.as_u16() {
                401 => Err(CortexError::Auth(
                    "Authentication failed. Please login again with 'cortex login'.".to_string(),
                )),
                403 => Err(CortexError::Auth(
                    "Access denied. You may not have permission to view billing information."
                        .to_string(),
                )),
                404 => Err(CortexError::NotFound(
                    "Billing information not found. You may not have an active subscription."
                        .to_string(),
                )),
                429 => Err(CortexError::RateLimit(
                    "Rate limit exceeded. Please try again later.".to_string(),
                )),
                _ => Err(CortexError::ConnectionFailed {
                    endpoint: url,
                    message: format!("HTTP {}: {}", status, error_body),
                }),
            };
        }

        response.json::<T>().await.map_err(|e| {
            error!(error = %e, "Failed to parse billing API response");
            CortexError::Internal(format!("Failed to parse billing response: {}", e))
        })
    }

    /// Gets current usage summary.
    ///
    /// # Arguments
    /// * `query` - Optional query parameters for filtering.
    pub async fn get_usage(&self, query: Option<UsageQuery>) -> Result<UsageSummary> {
        let mut path = "/billing/usage".to_string();
        if let Some(q) = query {
            let mut params = Vec::new();
            if let Some(start) = q.start_date {
                params.push(format!("start_date={}", start));
            }
            if let Some(end) = q.end_date {
                params.push(format!("end_date={}", end));
            }
            if let Some(model) = q.model {
                params.push(format!("model={}", model));
            }
            if !params.is_empty() {
                path = format!("{}?{}", path, params.join("&"));
            }
        }

        info!("Fetching usage summary");
        self.get(&path).await
    }

    /// Gets usage breakdown by model.
    ///
    /// # Arguments
    /// * `query` - Optional query parameters for filtering by date range.
    pub async fn get_usage_by_model(&self, query: Option<UsageQuery>) -> Result<Vec<ModelUsage>> {
        let mut path = "/billing/usage/models".to_string();
        if let Some(q) = query {
            let mut params = Vec::new();
            if let Some(start) = q.start_date {
                params.push(format!("start_date={}", start));
            }
            if let Some(end) = q.end_date {
                params.push(format!("end_date={}", end));
            }
            if !params.is_empty() {
                path = format!("{}?{}", path, params.join("&"));
            }
        }

        info!("Fetching model usage breakdown");
        self.get(&path).await
    }

    /// Gets current subscription information.
    pub async fn get_subscription(&self) -> Result<SubscriptionInfo> {
        info!("Fetching subscription info");
        self.get("/billing/subscription").await
    }

    /// Gets rate and usage limits.
    pub async fn get_limits(&self) -> Result<LimitsInfo> {
        info!("Fetching limits info");
        self.get("/billing/limits").await
    }
}

impl Default for BillingClient {
    fn default() -> Self {
        Self::new().expect("Failed to create default billing client")
    }
}

// ============================================================================
// DISPLAY FORMATTING
// ============================================================================

/// Formats a billing status display for the TUI.
pub fn format_billing_status(subscription: &SubscriptionInfo, usage: &UsageSummary) -> String {
    let mut lines = Vec::new();

    // Plan info
    lines.push(format!(
        "Plan: {} (${:.2}/month)",
        subscription.plan.name, subscription.plan.price_usd_monthly
    ));
    lines.push(format!(
        "   Status: {}",
        format_status(&subscription.status)
    ));

    // Period
    let period_end_str = subscription
        .current_period_end
        .format("%Y-%m-%d")
        .to_string();
    lines.push(format!("   Next billing date: {}", period_end_str));

    lines.push(String::new());

    // Usage summary
    lines.push("Current Usage:".to_string());
    lines.push(format!(
        "   Tokens: {} (in: {}, out: {})",
        format_number(usage.total_tokens),
        format_number(usage.input_tokens),
        format_number(usage.output_tokens)
    ));
    lines.push(format!(
        "   Requests: {} ({} successful)",
        format_number(usage.total_requests),
        format_number(usage.successful_requests)
    ));
    lines.push(format!("   Cost: ${:.4}", usage.total_cost_usd));

    // Quota info if present
    if let Some(ref quota) = usage.quota {
        lines.push(String::new());
        lines.push("Quota:".to_string());
        if let Some(remaining) = quota.tokens_remaining {
            lines.push(format!(
                "   Credits remaining: {} tokens ({:.1}% used)",
                format_number(remaining),
                quota.percent_used
            ));
        }
        if let Some(spend_remaining) = quota.spend_remaining {
            lines.push(format!("   Spend remaining: ${:.2}", spend_remaining));
        }
    }

    lines.join("\n")
}

/// Formats a usage breakdown display for the TUI.
pub fn format_usage_breakdown(models: &[ModelUsage], query: &Option<UsageQuery>) -> String {
    let mut lines = Vec::new();

    // Header with date range if specified
    if let Some(q) = query {
        if let (Some(start), Some(end)) = (&q.start_date, &q.end_date) {
            lines.push(format!("Usage from {} to {}:", start, end));
        } else {
            lines.push("Usage for current billing period:".to_string());
        }
    } else {
        lines.push("Usage for current billing period:".to_string());
    }
    lines.push(String::new());

    if models.is_empty() {
        lines.push("   No usage recorded for this period.".to_string());
        return lines.join("\n");
    }

    // Table header
    lines.push(format!(
        "{:<35} {:>12} {:>12} {:>8} {:>10}",
        "Model", "Input", "Output", "Reqs", "Cost"
    ));
    lines.push("-".repeat(80));

    // Model rows
    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let mut total_reqs = 0u64;
    let mut total_cost = 0.0f64;

    for model in models {
        lines.push(format!(
            "{:<35} {:>12} {:>12} {:>8} {:>10}",
            truncate_model_name(&model.model, 35),
            format_number(model.input_tokens),
            format_number(model.output_tokens),
            format_number(model.requests),
            format!("${:.4}", model.cost_usd)
        ));
        total_input += model.input_tokens;
        total_output += model.output_tokens;
        total_reqs += model.requests;
        total_cost += model.cost_usd;
    }

    // Total row
    lines.push("-".repeat(80));
    lines.push(format!(
        "{:<35} {:>12} {:>12} {:>8} {:>10}",
        "TOTAL",
        format_number(total_input),
        format_number(total_output),
        format_number(total_reqs),
        format!("${:.4}", total_cost)
    ));

    lines.join("\n")
}

/// Formats subscription status for display.
fn format_status(status: &str) -> String {
    match status {
        "active" => "Active".to_string(),
        "trialing" => "Trial".to_string(),
        "past_due" => "[!] Past Due".to_string(),
        "canceled" => "[X] Canceled".to_string(),
        "incomplete" => "[~] Incomplete".to_string(),
        _ => status.to_string(),
    }
}

/// Formats a number with thousand separators.
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i) % 3 == 0 {
            result.push(',');
        }
        result.push(*c);
    }
    result
}

/// Truncates a model name to fit display width.
fn truncate_model_name(name: &str, max_len: usize) -> String {
    cortex_common::truncate_model_name(name, max_len).into_owned()
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1000000), "1,000,000");
        assert_eq!(format_number(1234567890), "1,234,567,890");
    }

    #[test]
    fn test_format_status() {
        assert!(format_status("active").contains("Active"));
        assert!(format_status("canceled").contains("Canceled"));
        assert!(format_status("past_due").contains("Past Due"));
    }

    #[test]
    fn test_truncate_model_name() {
        assert_eq!(truncate_model_name("short", 10), "short");
        assert_eq!(
            truncate_model_name("very-long-model-name", 10),
            "very-lo..."
        );
    }

    #[test]
    fn test_usage_query_default() {
        let query = UsageQuery::default();
        assert!(query.start_date.is_none());
        assert!(query.end_date.is_none());
        assert!(query.model.is_none());
    }
}
