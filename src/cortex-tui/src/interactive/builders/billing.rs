//! Builder for billing information display with loading state.
//!
//! Displays billing info in a card similar to /settings panel:
//! - Loading state while fetching
//! - Billing details when ready

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState};

/// Billing loading status
#[derive(Debug, Clone, Default)]
pub enum BillingStatus {
    #[default]
    Loading,
    Ready,
    Error(String),
    NotLoggedIn,
}

/// Billing flow state stored in AppState
#[derive(Debug, Clone, Default)]
pub struct BillingFlowState {
    pub status: BillingStatus,
    // Subscription info
    pub plan_name: Option<String>,
    pub plan_status: Option<String>,
    pub current_period_start: Option<String>,
    pub current_period_end: Option<String>,
    // Usage info
    pub total_tokens: Option<u64>,
    pub total_requests: Option<u64>,
    pub total_cost_usd: Option<f64>,
    // Quota info
    pub quota_used: Option<u64>,
    pub quota_limit: Option<u64>,
}

impl BillingFlowState {
    /// Create a new billing flow in loading state
    pub fn loading() -> Self {
        Self {
            status: BillingStatus::Loading,
            plan_name: None,
            plan_status: None,
            current_period_start: None,
            current_period_end: None,
            total_tokens: None,
            total_requests: None,
            total_cost_usd: None,
            quota_used: None,
            quota_limit: None,
        }
    }

    /// Set error state
    pub fn set_error(&mut self, error: String) {
        self.status = BillingStatus::Error(error);
    }

    /// Set not logged in state
    pub fn set_not_logged_in(&mut self) {
        self.status = BillingStatus::NotLoggedIn;
    }

    /// Set billing data ready
    pub fn set_ready(&mut self) {
        self.status = BillingStatus::Ready;
    }
}

/// Format token count with K/M suffixes
fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

/// Format USD cost
fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("${:.4}", cost)
    } else {
        format!("${:.2}", cost)
    }
}

/// Build interactive state for billing information display.
pub fn build_billing_selector(flow: &BillingFlowState) -> InteractiveState {
    let mut items = Vec::new();

    match &flow.status {
        BillingStatus::Loading => {
            items.push(
                InteractiveItem::new("__loading__", "Loading billing information...")
                    .as_separator(),
            );
            items.push(InteractiveItem::new("__spacer__", "").as_separator());
            items.push(
                InteractiveItem::new("cancel", "Close")
                    .with_description("Close panel")
                    .with_icon(' '),
            );
        }
        BillingStatus::NotLoggedIn => {
            items.push(InteractiveItem::new("__info__", "Not logged in").as_separator());
            items.push(InteractiveItem::new("__spacer__", "").as_separator());
            items.push(
                InteractiveItem::new("login", "Login")
                    .with_description("Authenticate with Cortex")
                    .with_icon('>'),
            );
            items.push(
                InteractiveItem::new("cancel", "Close")
                    .with_description("Close panel")
                    .with_icon(' '),
            );
        }
        BillingStatus::Error(err) => {
            items.push(InteractiveItem::new("__error__", format!("Error: {}", err)).as_separator());
            items.push(InteractiveItem::new("__spacer__", "").as_separator());
            items.push(
                InteractiveItem::new("retry", "Retry")
                    .with_description("Try again")
                    .with_icon('>'),
            );
            items.push(
                InteractiveItem::new("cancel", "Close")
                    .with_description("Close panel")
                    .with_icon(' '),
            );
        }
        BillingStatus::Ready => {
            // Subscription section
            items.push(InteractiveItem::new("__cat_subscription__", "Subscription").as_separator());

            // Plan Name
            if let Some(ref plan) = flow.plan_name {
                items.push(
                    InteractiveItem::new("plan", "Plan")
                        .with_description(plan.clone())
                        .with_icon('>'),
                );
            }

            // Plan Status
            if let Some(ref status) = flow.plan_status {
                items.push(
                    InteractiveItem::new("plan_status", "Status")
                        .with_description(status.clone())
                        .with_icon(' '),
                );
            }

            // Billing Period
            if let (Some(start), Some(end)) = (&flow.current_period_start, &flow.current_period_end)
            {
                items.push(
                    InteractiveItem::new("period", "Billing Period")
                        .with_description(format!("{} - {}", start, end))
                        .with_icon(' '),
                );
            }

            items.push(InteractiveItem::new("__spacer1__", "").as_separator());

            // Usage section
            items.push(InteractiveItem::new("__cat_usage__", "Usage").as_separator());

            // Total Tokens
            if let Some(tokens) = flow.total_tokens {
                items.push(
                    InteractiveItem::new("tokens", "Total Tokens")
                        .with_description(format_tokens(tokens))
                        .with_icon(' '),
                );
            }

            // Total Requests
            if let Some(requests) = flow.total_requests {
                items.push(
                    InteractiveItem::new("requests", "Total Requests")
                        .with_description(requests.to_string())
                        .with_icon(' '),
                );
            }

            // Cost
            if let Some(cost) = flow.total_cost_usd {
                items.push(
                    InteractiveItem::new("cost", "Total Cost")
                        .with_description(format_cost(cost))
                        .with_icon(' '),
                );
            }

            // Quota
            if let (Some(used), Some(limit)) = (flow.quota_used, flow.quota_limit) {
                let percentage = if limit > 0 {
                    (used as f64 / limit as f64 * 100.0) as u64
                } else {
                    0
                };
                items.push(
                    InteractiveItem::new("quota", "Quota")
                        .with_description(format!(
                            "{} / {} ({}%)",
                            format_tokens(used),
                            format_tokens(limit),
                            percentage
                        ))
                        .with_icon(' '),
                );
            }

            items.push(InteractiveItem::new("__spacer2__", "").as_separator());

            // Actions section
            items.push(InteractiveItem::new("__cat_actions__", "Actions").as_separator());
            items.push(
                InteractiveItem::new("refresh", "Refresh")
                    .with_description("Update billing info")
                    .with_icon(' '),
            );
            items.push(
                InteractiveItem::new("manage", "Manage Billing")
                    .with_description("Open billing portal")
                    .with_icon(' '),
            );
            items.push(
                InteractiveItem::new("cancel", "Close")
                    .with_description("Close panel")
                    .with_icon(' '),
            );
        }
    }

    InteractiveState::new("Billing", items, InteractiveAction::BillingAction)
        .with_hints(vec![
            ("Enter".into(), "select".into()),
            ("Esc".into(), "close".into()),
        ])
        .with_max_visible(20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_billing_selector_loading() {
        let flow = BillingFlowState::loading();
        let state = build_billing_selector(&flow);
        assert_eq!(state.title, "Billing");
        assert!(!state.items.is_empty());
    }

    #[test]
    fn test_build_billing_selector_ready() {
        let mut flow = BillingFlowState::loading();
        flow.plan_name = Some("Pro".to_string());
        flow.plan_status = Some("Active".to_string());
        flow.total_tokens = Some(1_500_000);
        flow.total_cost_usd = Some(12.50);
        flow.set_ready();
        let state = build_billing_selector(&flow);
        assert!(state.items.iter().any(|i| i.id == "plan"));
        assert!(state.items.iter().any(|i| i.id == "tokens"));
    }

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(1_500), "1.5K");
        assert_eq!(format_tokens(1_500_000), "1.5M");
    }

    #[test]
    fn test_format_cost() {
        assert_eq!(format_cost(12.50), "$12.50");
        assert_eq!(format_cost(0.005), "$0.0050");
    }
}
