//! Quick-start functions for running the TUI.

use super::exit_info::AppExitInfo;
use super::runner::AppRunner;
use anyhow::Result;
use cortex_engine::Config;
use cortex_protocol::ConversationId;

// ============================================================================
// Quick-start Functions
// ============================================================================

/// Quick-start function for running the TUI with defaults.
///
/// This is a convenience function that creates an `AppRunner` and runs it
/// with optional initial prompt.
///
/// # Arguments
///
/// * `config` - The cortex-core configuration
/// * `initial_prompt` - Optional initial prompt to send on startup
///
/// # Returns
///
/// Returns `AppExitInfo` containing exit details.
///
/// # Errors
///
/// Returns an error if the runner encounters any issues.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_tui::runner::app_runner;
///
/// let exit_info = app_runner::run(config, Some("Hello!".to_string())).await?;
/// ```
pub async fn run(config: Config, initial_prompt: Option<String>) -> Result<AppExitInfo> {
    let mut runner = AppRunner::new(config);
    if let Some(prompt) = initial_prompt {
        runner = runner.with_initial_prompt(prompt);
    }
    runner.run().await
}

/// Quick-start function for resuming a session.
///
/// This is a convenience function that creates an `AppRunner` configured
/// to resume an existing conversation.
///
/// # Arguments
///
/// * `config` - The cortex-core configuration
/// * `conversation_id` - The ID of the conversation to resume
///
/// # Returns
///
/// Returns `AppExitInfo` containing exit details.
///
/// # Errors
///
/// Returns an error if the session cannot be found or resumed.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_tui::runner::app_runner;
/// use cortex_protocol::ConversationId;
///
/// let id = ConversationId::from_str("abc-123")?;
/// let exit_info = app_runner::resume(config, id).await?;
/// ```
pub async fn resume(config: Config, conversation_id: ConversationId) -> Result<AppExitInfo> {
    AppRunner::new(config)
        .with_conversation_id(conversation_id)
        .run()
        .await
}

/// Run in demo mode (no backend).
///
/// This is a convenience function for running the TUI in demo mode
/// without connecting to any backend.
///
/// # Arguments
///
/// * `config` - The cortex-core configuration (used for UI settings)
///
/// # Returns
///
/// Returns `AppExitInfo` with default values.
///
/// # Errors
///
/// Returns an error if terminal initialization fails.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_tui::runner::app_runner;
///
/// let exit_info = app_runner::run_demo(config).await?;
/// ```
pub async fn run_demo(config: Config) -> Result<AppExitInfo> {
    AppRunner::new(config).run_demo().await
}

/// Quick-start function for running with direct provider mode.
///
/// This creates an `AppRunner` configured for direct provider access
/// (bypasses backend session bridge).
///
/// # Arguments
///
/// * `config` - The cortex-core configuration
/// * `initial_prompt` - Optional initial prompt to send on startup
///
/// # Returns
///
/// Returns `AppExitInfo` containing exit details.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_tui::runner::app_runner;
///
/// let exit_info = app_runner::run_direct(config, None).await?;
/// ```
pub async fn run_direct(config: Config, initial_prompt: Option<String>) -> Result<AppExitInfo> {
    let mut runner = AppRunner::new(config).direct_provider(true);
    if let Some(prompt) = initial_prompt {
        runner = runner.with_initial_prompt(prompt);
    }
    runner.run().await
}

/// Resume a Cortex session (direct provider mode).
///
/// # Arguments
///
/// * `config` - The cortex-core configuration
/// * `session_id` - The Cortex session ID to resume
///
/// # Returns
///
/// Returns `AppExitInfo` containing exit details.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_tui::runner::app_runner;
///
/// let exit_info = app_runner::resume_cortex(config, "abc-123").await?;
/// ```
pub async fn resume_cortex(config: Config, session_id: impl Into<String>) -> Result<AppExitInfo> {
    AppRunner::new(config)
        .with_cortex_session_id(session_id)
        .run()
        .await
}
