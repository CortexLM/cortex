//! Billing handlers for the TUI.
//!
//! This module contains handlers for billing-related commands:
//! - /billing - Display billing status
//! - /usage - Display detailed usage breakdown
//!
//! These handlers were extracted from `event_loop.rs` to improve modularity
//! and reduce file size.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::events::ToolEvent;

/// Billing status data.
#[derive(Debug, Clone)]
pub struct BillingData {
    pub plan_name: String,
    pub status: String,
    pub period_start: String,
    pub period_end: String,
    pub total_tokens: Option<u64>,
    pub total_requests: Option<u64>,
    pub total_cost_usd: Option<f64>,
    pub quota_used: Option<u64>,
    pub quota_limit: Option<u64>,
}

/// Parse billing data from the tool event output.
pub fn parse_billing_data(output: &str) -> Option<BillingData> {
    if !output.starts_with("billing:data:") {
        return None;
    }

    let data_part = output.strip_prefix("billing:data:")?;
    let mut data = BillingData {
        plan_name: String::new(),
        status: String::new(),
        period_start: String::new(),
        period_end: String::new(),
        total_tokens: None,
        total_requests: None,
        total_cost_usd: None,
        quota_used: None,
        quota_limit: None,
    };

    for part in data_part.split('|') {
        if let Some((key, value)) = part.split_once('=') {
            match key {
                "plan" => data.plan_name = value.to_string(),
                "status" => data.status = value.to_string(),
                "period_start" => data.period_start = value.to_string(),
                "period_end" => data.period_end = value.to_string(),
                "tokens" => data.total_tokens = value.parse().ok(),
                "requests" => data.total_requests = value.parse().ok(),
                "cost" => data.total_cost_usd = value.parse().ok(),
                "quota_used" => data.quota_used = value.parse().ok(),
                "quota_limit" => data.quota_limit = value.parse().ok(),
                _ => {}
            }
        }
    }

    Some(data)
}

/// Fetch billing status in the background.
pub fn spawn_billing_fetch(tx: mpsc::Sender<ToolEvent>) {
    tokio::spawn(async move {
        use cortex_engine::billing_client::BillingClient;

        // Create billing client
        let client = match BillingClient::new() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send(ToolEvent::Failed {
                        id: "billing".to_string(),
                        name: "billing".to_string(),
                        error: format!("billing:error:{}", e),
                        duration: Duration::from_secs(0),
                    })
                    .await;
                return;
            }
        };

        // Fetch subscription info
        let subscription = match client.get_subscription().await {
            Ok(sub) => sub,
            Err(e) => {
                let error_msg = if e.to_string().contains("Not authenticated")
                    || e.to_string().contains("Authentication failed")
                {
                    "billing:not_logged_in".to_string()
                } else if e.to_string().contains("not found") {
                    "billing:error:No subscription found. You may be on a free plan.".to_string()
                } else {
                    format!("billing:error:{}", e)
                };
                let _ = tx
                    .send(ToolEvent::Failed {
                        id: "billing".to_string(),
                        name: "billing".to_string(),
                        error: error_msg,
                        duration: Duration::from_secs(0),
                    })
                    .await;
                return;
            }
        };

        // Fetch usage summary
        let usage = client.get_usage(None).await.ok();

        // Format dates
        let period_start = subscription
            .current_period_start
            .format("%Y-%m-%d")
            .to_string();
        let period_end = subscription
            .current_period_end
            .format("%Y-%m-%d")
            .to_string();

        // Build billing data message
        let mut data = format!(
            "billing:data:plan={}|status={}|period_start={}|period_end={}",
            subscription.plan.name, subscription.status, period_start, period_end
        );

        if let Some(ref usage) = usage {
            data.push_str(&format!(
                "|tokens={}|requests={}|cost={}",
                usage.total_tokens, usage.total_requests, usage.total_cost_usd
            ));
            if let Some(ref quota) = usage.quota {
                data.push_str(&format!(
                    "|quota_used={}|quota_limit={}",
                    quota.tokens_used,
                    quota.monthly_tokens.unwrap_or(0)
                ));
            }
        }

        let _ = tx
            .send(ToolEvent::Completed {
                id: "billing".to_string(),
                name: "billing".to_string(),
                output: data,
                success: true,
                duration: Duration::from_secs(0),
            })
            .await;
    });
}

/// Fetch usage breakdown in the background.
pub fn spawn_usage_fetch(tx: mpsc::Sender<ToolEvent>, from: Option<String>, to: Option<String>) {
    tokio::spawn(async move {
        use cortex_engine::billing_client::{BillingClient, UsageQuery, format_usage_breakdown};

        // Build query with date strings
        let query = UsageQuery {
            start_date: from,
            end_date: to,
            model: None,
        };

        // Create billing client
        let client = match BillingClient::new() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send(ToolEvent::Failed {
                        id: "usage".to_string(),
                        name: "usage".to_string(),
                        error: format!("usage:error:{}", e),
                        duration: Duration::from_secs(0),
                    })
                    .await;
                return;
            }
        };

        // Fetch usage by model
        let query_opt = if query.start_date.is_some() || query.end_date.is_some() {
            Some(query.clone())
        } else {
            None
        };

        let models = match client.get_usage_by_model(query_opt.clone()).await {
            Ok(m) => m,
            Err(e) => {
                let error_msg = if e.to_string().contains("Not authenticated") {
                    "usage:not_logged_in".to_string()
                } else {
                    format!("usage:error:{}", e)
                };
                let _ = tx
                    .send(ToolEvent::Failed {
                        id: "usage".to_string(),
                        name: "usage".to_string(),
                        error: error_msg,
                        duration: Duration::from_secs(0),
                    })
                    .await;
                return;
            }
        };

        // Format the usage breakdown
        let output = format_usage_breakdown(&models, &query_opt);

        let _ = tx
            .send(ToolEvent::Completed {
                id: "usage".to_string(),
                name: "usage".to_string(),
                output,
                success: true,
                duration: Duration::from_secs(0),
            })
            .await;
    });
}
