//! Builder for device code login interactive flow.

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState};

/// Login flow status
#[derive(Debug, Clone, Default)]
pub enum LoginStatus {
    #[default]
    Loading,
    Waiting,
    Polling,
    Success,
    Expired,
    Error(String),
}

/// Login flow state stored in AppState
#[derive(Debug, Clone)]
pub struct LoginFlowState {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub status: LoginStatus,
}

impl LoginFlowState {
    pub fn new(device_code: String, user_code: String, verification_uri: String) -> Self {
        Self {
            device_code,
            user_code,
            verification_uri,
            status: LoginStatus::Waiting,
        }
    }

    /// Create a new login flow in loading state
    pub fn loading() -> Self {
        Self {
            device_code: String::new(),
            user_code: String::new(),
            verification_uri: String::new(),
            status: LoginStatus::Loading,
        }
    }

    /// Update with device code data received from API
    pub fn set_device_code(
        &mut self,
        device_code: String,
        user_code: String,
        verification_uri: String,
    ) {
        self.device_code = device_code;
        self.user_code = user_code;
        self.verification_uri = verification_uri;
        self.status = LoginStatus::Waiting;
    }
}

/// Build interactive state for device code login.
pub fn build_login_selector(flow: &LoginFlowState) -> InteractiveState {
    let mut items = Vec::new();

    match &flow.status {
        LoginStatus::Loading => {
            items.push(
                InteractiveItem::new("__loading__", "Connecting to Cortex...").as_separator(),
            );
            items.push(InteractiveItem::new("__spacer__", "").as_separator());
            items.push(InteractiveItem::new("cancel", "Cancel").with_description("Cancel login"));
        }
        _ => {
            items.push(
                InteractiveItem::new("__code__", format!("Code: {}", flow.user_code))
                    .as_separator(),
            );
            items.push(
                InteractiveItem::new("__url__", format!("URL: {}", flow.verification_uri))
                    .as_separator(),
            );
            let status_text = match &flow.status {
                LoginStatus::Loading => "Connecting...",
                LoginStatus::Waiting => "Waiting for authentication...",
                LoginStatus::Polling => "Checking...",
                LoginStatus::Success => "Authenticated!",
                LoginStatus::Expired => "Code expired",
                LoginStatus::Error(e) => e.as_str(),
            };
            items.push(InteractiveItem::new("__status__", status_text).as_separator());
            items.push(InteractiveItem::new("__spacer__", "").as_separator());
            items.push(
                InteractiveItem::new("copy_code", "Copy code")
                    .with_description("Copy to clipboard"),
            );
            items.push(
                InteractiveItem::new("open_browser", "Open browser")
                    .with_description("Open verification URL"),
            );
            items.push(InteractiveItem::new("cancel", "Cancel").with_description("Cancel login"));
        }
    }

    InteractiveState::new("Login to Cortex", items, InteractiveAction::DeviceLogin)
        .with_hints(vec![
            ("Enter".into(), "select".into()),
            ("Esc".into(), "cancel".into()),
        ])
        .with_max_visible(10)
}

/// Build interactive state for already logged in confirmation.
pub fn build_already_logged_in_selector() -> InteractiveState {
    let items = vec![
        InteractiveItem::new("__info__", "You are currently logged in").as_separator(),
        InteractiveItem::new("__spacer__", "").as_separator(),
        InteractiveItem::new("switch_account", "Login with another account")
            .with_description("Sign out and login again"),
        InteractiveItem::new("cancel", "Cancel").with_description("Keep current session"),
    ];

    InteractiveState::new(
        "Already Logged In",
        items,
        InteractiveAction::AlreadyLoggedIn,
    )
    .with_hints(vec![
        ("Enter".into(), "select".into()),
        ("Esc".into(), "cancel".into()),
    ])
    .with_max_visible(10)
}
