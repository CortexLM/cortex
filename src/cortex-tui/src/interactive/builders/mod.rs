//! Builders for interactive selection states.
//!
//! Each builder creates an `InteractiveState` configured for a specific command.

pub mod account;
pub mod agents;
pub mod approval;
pub mod billing;
pub mod export;
pub mod files;
pub mod login;
pub mod mcp;
pub mod model;
// pub mod provider; // REMOVED (single Cortex provider)
pub mod resume_picker;
pub mod scroll;
pub mod sessions;
pub mod settings;
pub mod temperature;
pub mod theme;

pub use account::{AccountFlowState, AccountStatus, build_account_selector};
pub use agents::{
    AgentCategory, AgentCreationMethod, AgentDisplayInfo, AgentLocation, NewAgentConfig,
    PermissionPreset, build_agent_ai_description_form, build_agent_confirm_selector,
    build_agent_location_selector, build_agent_method_selector, build_agents_selector,
    build_permission_selector,
};
pub use approval::{build_approval_selector, build_log_level_selector};
pub use billing::{BillingFlowState, BillingStatus, build_billing_selector};
pub use export::build_export_selector;
pub use files::{build_context_list, build_context_remove, build_file_browser};
pub use login::{
    LoginFlowState, LoginStatus, build_already_logged_in_selector, build_login_selector,
};
pub use mcp::{
    build_mcp_add_server_form, build_mcp_http_form, build_mcp_registry_browser, build_mcp_selector,
    build_mcp_source_selector, build_mcp_stdio_form, build_mcp_transport_selector,
};
pub use model::build_model_selector;
// pub use provider::build_provider_selector; // REMOVED (single Cortex provider)
pub use resume_picker::build_resume_picker;
pub use scroll::build_scroll_selector;
pub use sessions::build_sessions_selector;
pub use settings::{SettingsSnapshot, build_settings_selector, build_settings_selector_with_tab};
pub use temperature::build_temperature_selector;
pub use theme::build_theme_selector;
