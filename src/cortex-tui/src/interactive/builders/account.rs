//! Builder for account information display with loading state.
//!
//! Displays account info in a card similar to /settings panel:
//! - Loading state while fetching
//! - Account details when ready

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState};

/// Account loading status
#[derive(Debug, Clone, Default)]
pub enum AccountStatus {
    #[default]
    Loading,
    Ready,
    Error(String),
    NotLoggedIn,
}

/// Account flow state stored in AppState
#[derive(Debug, Clone, Default)]
pub struct AccountFlowState {
    pub status: AccountStatus,
    // Auth info
    pub auth_method: Option<String>,
    pub expires_at: Option<String>,
    pub account_id: Option<String>,
}

impl AccountFlowState {
    /// Create a new account flow in loading state
    pub fn loading() -> Self {
        Self {
            status: AccountStatus::Loading,
            auth_method: None,
            expires_at: None,
            account_id: None,
        }
    }

    /// Set account data from auth info
    pub fn set_account_data(
        &mut self,
        auth_method: String,
        expires_at: Option<String>,
        account_id: Option<String>,
    ) {
        self.auth_method = Some(auth_method);
        self.expires_at = expires_at;
        self.account_id = account_id;
        self.status = AccountStatus::Ready;
    }

    /// Set error state
    pub fn set_error(&mut self, error: String) {
        self.status = AccountStatus::Error(error);
    }

    /// Set not logged in state
    pub fn set_not_logged_in(&mut self) {
        self.status = AccountStatus::NotLoggedIn;
    }
}

/// Build interactive state for account information display.
pub fn build_account_selector(flow: &AccountFlowState) -> InteractiveState {
    let mut items = Vec::new();

    match &flow.status {
        AccountStatus::Loading => {
            items.push(
                InteractiveItem::new("__loading__", "Loading account information...")
                    .as_separator(),
            );
            items.push(InteractiveItem::new("__spacer__", "").as_separator());
            items.push(
                InteractiveItem::new("cancel", "Close")
                    .with_description("Close panel")
                    .with_icon(' '),
            );
        }
        AccountStatus::NotLoggedIn => {
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
        AccountStatus::Error(err) => {
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
        AccountStatus::Ready => {
            // Account Information section
            items.push(InteractiveItem::new("__cat_info__", "Account").as_separator());

            // Auth Method
            if let Some(ref method) = flow.auth_method {
                items.push(
                    InteractiveItem::new("auth_method", "Auth Method")
                        .with_description(method.clone())
                        .with_icon(' '),
                );
            }

            // Account ID
            if let Some(ref id) = flow.account_id {
                items.push(
                    InteractiveItem::new("account_id", "Account ID")
                        .with_description(id.clone())
                        .with_icon(' '),
                );
            }

            // Expiration
            if let Some(ref expires) = flow.expires_at {
                items.push(
                    InteractiveItem::new("expires", "Expires")
                        .with_description(expires.clone())
                        .with_icon(' '),
                );
            }

            // Status
            items.push(
                InteractiveItem::new("status", "Status")
                    .with_description("Active".to_string())
                    .with_icon('>'),
            );

            items.push(InteractiveItem::new("__spacer__", "").as_separator());

            // Actions section
            items.push(InteractiveItem::new("__cat_actions__", "Actions").as_separator());
            items.push(
                InteractiveItem::new("logout", "Logout")
                    .with_description("Sign out of account")
                    .with_icon(' '),
            );
            items.push(
                InteractiveItem::new("cancel", "Close")
                    .with_description("Close panel")
                    .with_icon(' '),
            );
        }
    }

    InteractiveState::new("Account", items, InteractiveAction::AccountAction)
        .with_hints(vec![
            ("Enter".into(), "select".into()),
            ("Esc".into(), "close".into()),
        ])
        .with_max_visible(15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_account_selector_loading() {
        let flow = AccountFlowState::loading();
        let state = build_account_selector(&flow);
        assert_eq!(state.title, "Account");
        assert!(!state.items.is_empty());
    }

    #[test]
    fn test_build_account_selector_ready() {
        let mut flow = AccountFlowState::loading();
        flow.set_account_data(
            "OAuth".to_string(),
            Some("2025-12-31".to_string()),
            Some("user123".to_string()),
        );
        let state = build_account_selector(&flow);
        assert!(state.items.iter().any(|i| i.id == "auth_method"));
        assert!(state.items.iter().any(|i| i.id == "account_id"));
    }

    #[test]
    fn test_build_account_selector_not_logged_in() {
        let mut flow = AccountFlowState::loading();
        flow.set_not_logged_in();
        let state = build_account_selector(&flow);
        assert!(state.items.iter().any(|i| i.id == "login"));
    }

    #[test]
    fn test_build_account_selector_error() {
        let mut flow = AccountFlowState::loading();
        flow.set_error("Connection failed".to_string());
        let state = build_account_selector(&flow);
        assert!(state.items.iter().any(|i| i.id == "retry"));
    }
}
