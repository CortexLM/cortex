//! AppRunner - High-level application runner for Cortex TUI.

use super::auth_status::AuthStatus;
use super::exit_info::{AppExitInfo, ExitReason};
use super::trusted_workspaces::{is_workspace_trusted, mark_workspace_trusted};

use crate::app::AppState;
use crate::bridge::SessionBridge;
use crate::providers::ProviderManager;
use crate::runner::event_loop::EventLoop;
use crate::runner::terminal::{CortexTerminal, TerminalOptions};
use crate::session::CortexSession;

use anyhow::Result;
use cortex_engine::Config;
use cortex_login::{CredentialsStoreMode, load_auth, logout_with_fallback};
use cortex_protocol::ConversationId;
use std::path::PathBuf;
use tracing;

// ============================================================================
// AppRunner
// ============================================================================

/// High-level application runner for Cortex TUI.
///
/// This is the main entry point for running the TUI application. It handles:
/// - Terminal initialization and cleanup
/// - Session creation or resumption
/// - Event loop execution
/// - Graceful shutdown and error handling
///
/// # Example
///
/// ```rust,ignore
/// use cortex_tui::runner::AppRunner;
/// use cortex_engine::Config;
///
/// // Basic usage
/// let config = Config::load_sync(Default::default())?;
/// let mut runner = AppRunner::new(config);
/// runner.run().await?;
///
/// // With builder pattern
/// let exit_info = AppRunner::new(config)
///     .with_initial_prompt("Explain this codebase")
///     .inline()
///     .run()
///     .await?;
/// ```
///
/// # Lifecycle
///
/// 1. **Initialization**: Terminal is set up with the configured options
/// 2. **Session**: A new session is created or an existing one is resumed
/// 3. **Event Loop**: The main loop processes events until exit
/// 4. **Cleanup**: Terminal is restored and resources are released
pub struct AppRunner {
    /// The cortex-core configuration.
    pub(crate) config: Config,
    /// Initial prompt to send on startup (optional).
    pub(crate) initial_prompt: Option<String>,
    /// Conversation ID to resume (optional).
    pub(crate) conversation_id: Option<ConversationId>,
    /// Cortex session ID to resume (optional).
    pub(crate) cortex_session_id: Option<String>,
    /// Terminal configuration options.
    pub(crate) terminal_options: TerminalOptions,
    /// Whether to use direct provider mode (bypasses backend).
    pub(crate) use_direct_provider: bool,
}

impl AppRunner {
    // ========================================================================
    // Constructor
    // ========================================================================

    /// Create a new app runner with the given configuration.
    ///
    /// This creates a runner with default terminal options (full-screen mode
    /// with alternate screen, mouse capture, etc.).
    ///
    /// # Arguments
    ///
    /// * `config` - The cortex-core configuration for the session
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let config = Config::load_sync(Default::default())?;
    /// let runner = AppRunner::new(config);
    /// ```
    pub fn new(config: Config) -> Self {
        Self {
            config,
            initial_prompt: None,
            conversation_id: None,
            cortex_session_id: None,
            terminal_options: TerminalOptions::default(),
            use_direct_provider: true, // Default to direct provider mode
        }
    }

    // ========================================================================
    // Builder Methods
    // ========================================================================

    /// Set an initial prompt to send on startup.
    ///
    /// When set, this prompt will be automatically sent to the session
    /// after initialization completes. This is useful for CLI workflows
    /// where the user provides input via command-line arguments.
    ///
    /// # Arguments
    ///
    /// * `prompt` - The initial message to send
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let runner = AppRunner::new(config)
    ///     .with_initial_prompt("What files are in this directory?");
    /// ```
    pub fn with_initial_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.initial_prompt = Some(prompt.into());
        self
    }

    /// Resume an existing conversation (legacy backend mode).
    ///
    /// When set, the runner will attempt to resume the specified conversation
    /// instead of creating a new one. The conversation history will be
    /// restored from the rollout file.
    ///
    /// # Arguments
    ///
    /// * `id` - The conversation ID to resume
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let conversation_id = ConversationId::from_str("abc-123")?;
    /// let runner = AppRunner::new(config)
    ///     .with_conversation_id(conversation_id);
    /// ```
    pub fn with_conversation_id(mut self, id: ConversationId) -> Self {
        self.conversation_id = Some(id);
        self.use_direct_provider = false; // Use legacy mode when resuming backend sessions
        self
    }

    /// Resume an existing Cortex session.
    ///
    /// When set, the runner will load the specified Cortex session from local storage.
    ///
    /// # Arguments
    ///
    /// * `id` - The Cortex session ID to resume
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let runner = AppRunner::new(config)
    ///     .with_cortex_session_id("abc-123");
    /// ```
    pub fn with_cortex_session_id(mut self, id: impl Into<String>) -> Self {
        self.cortex_session_id = Some(id.into());
        self
    }

    /// Use direct provider mode (bypasses backend session bridge).
    ///
    /// This is the default mode. Direct provider mode uses the ProviderManager
    /// to communicate directly with AI providers and stores sessions locally.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let runner = AppRunner::new(config).direct_provider(true);
    /// ```
    pub fn direct_provider(mut self, enabled: bool) -> Self {
        self.use_direct_provider = enabled;
        self
    }

    /// Use legacy backend mode (requires running cortex server).
    ///
    /// This mode uses the SessionBridge to communicate with a cortex-core backend.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let runner = AppRunner::new(config).legacy_backend();
    /// ```
    pub fn legacy_backend(mut self) -> Self {
        self.use_direct_provider = false;
        self
    }

    /// Use custom terminal options.
    ///
    /// This allows fine-grained control over terminal initialization,
    /// including alternate screen usage, mouse capture, and more.
    ///
    /// # Arguments
    ///
    /// * `options` - The terminal configuration options
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let options = TerminalOptions::new()
    ///     .alternate_screen(true)
    ///     .mouse_capture(false);
    /// let runner = AppRunner::new(config)
    ///     .with_terminal_options(options);
    /// ```
    pub fn with_terminal_options(mut self, options: TerminalOptions) -> Self {
        self.terminal_options = options;
        self
    }

    /// Use inline mode (preserves scrollback).
    ///
    /// Inline mode runs the TUI without using the alternate screen buffer,
    /// which preserves the terminal scrollback and allows output to remain
    /// visible after the TUI exits. This is useful for non-fullscreen
    /// workflows or when you want to see the conversation history in the
    /// terminal after exiting.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let runner = AppRunner::new(config).inline();
    /// ```
    pub fn inline(mut self) -> Self {
        self.terminal_options = TerminalOptions::inline();
        self
    }

    // ========================================================================
    // Configuration Helpers
    // ========================================================================

    /// Get the effective model name.
    ///
    /// Returns the model from the configuration, or a sensible default
    /// if none is configured.
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Get the effective provider name.
    ///
    /// Returns the provider from the configuration, or a sensible default
    /// if none is configured.
    pub fn provider(&self) -> &str {
        &self.config.model_provider_id
    }

    /// Get a reference to the configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    // ========================================================================
    // Run Methods
    // ========================================================================

    /// Run the application.
    ///
    /// This is the main entry point that:
    /// 1. Initializes the terminal with the configured options
    /// 2. Creates or resumes a session (direct provider or legacy bridge)
    /// 3. Sets up the application state
    /// 4. Runs the event loop until exit
    /// 5. Cleans up and returns exit information
    ///
    /// # Returns
    ///
    /// Returns `AppExitInfo` containing the conversation ID and exit reason.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Terminal initialization fails
    /// - Session creation/resumption fails
    /// - The event loop encounters a fatal error
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let exit_info = AppRunner::new(config).run().await?;
    /// println!("Exited: {:?}", exit_info.exit_reason);
    /// ```
    pub async fn run(self) -> Result<AppExitInfo> {
        tracing::info!("Starting Cortex TUI");

        // Use direct provider mode if enabled
        if self.use_direct_provider {
            return self.run_direct_provider().await;
        }

        // Legacy mode: use SessionBridge
        self.run_legacy_bridge().await
    }

    /// Run using direct provider mode (new architecture).
    ///
    /// ## Performance Optimizations
    ///
    /// This function is optimized for fast TUI startup by:
    /// 1. **Deferring non-critical HTTP requests**: Session validation, model fetching,
    ///    and user info are fetched in the background after the terminal is initialized
    /// 2. **Parallel I/O operations**: Session history loading runs concurrently with
    ///    other startup tasks using `tokio::spawn`
    /// 3. **Non-blocking validation**: Server-side session validation happens
    ///    asynchronously and doesn't block the TUI from appearing
    ///
    /// The TUI should appear almost instantly after trust verification and auth check.
    async fn run_direct_provider(self) -> Result<AppExitInfo> {
        // Initialize sound system early for audio notifications
        // This spawns a background thread for audio playback
        crate::sound::init();

        // Trust verification before anything else
        let workspace = std::env::current_dir()?;
        if !is_workspace_trusted(&workspace) {
            use crate::runner::trust_screen::{TrustResult, TrustScreen};
            let mut trust_screen = TrustScreen::new(workspace.clone());
            match trust_screen.run().await? {
                TrustResult::Trusted => {
                    mark_workspace_trusted(&workspace)?;
                }
                TrustResult::Rejected => {
                    return Ok(AppExitInfo {
                        exit_reason: ExitReason::Normal,
                        ..Default::default()
                    });
                }
            }
        }

        // Check authentication before starting (fast local check)
        let cortex_home = dirs::home_dir()
            .map(|h| h.join(".cortex"))
            .unwrap_or_else(|| PathBuf::from(".cortex"));

        // Load provider manager first to check for API keys
        let mut provider_manager = ProviderManager::load().unwrap_or_else(|e| {
            tracing::warn!("Failed to load provider config, using defaults: {}", e);
            ProviderManager::new(Default::default())
        });

        // Try to load auth token from keyring and set it on the provider manager
        if let Some(token) = cortex_login::get_auth_token() {
            tracing::debug!("Loaded auth token from keyring");
            provider_manager.set_auth_token(token);
        }

        // Check if user is authenticated (OAuth/API key login) or has API keys configured
        // This is a fast local check - no network calls
        let auth_status = match load_auth(&cortex_home, CredentialsStoreMode::default()) {
            Ok(Some(auth)) if !auth.is_expired() => {
                tracing::info!("User authenticated via {}", auth.mode);
                AuthStatus::Authenticated
            }
            Ok(Some(_)) => {
                // Token expired - delete stale credentials so user doesn't appear as "logged in"
                tracing::info!("Token expired, removing stale credentials");
                if let Err(e) = logout_with_fallback(&cortex_home) {
                    tracing::warn!("Failed to remove expired credentials: {}", e);
                }
                AuthStatus::Expired
            }
            Ok(None) => {
                // No OAuth credentials - check if API keys are configured
                if provider_manager.is_available() {
                    tracing::info!("Using API key authentication");
                    AuthStatus::Authenticated
                } else if std::env::var("CORTEX_API_KEY").is_ok() {
                    tracing::info!("Using CORTEX_API_KEY environment variable");
                    AuthStatus::Authenticated
                } else {
                    AuthStatus::NotAuthenticated
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load auth: {}", e);
                // Continue anyway if API keys are available
                if provider_manager.is_available() || std::env::var("CORTEX_API_KEY").is_ok() {
                    AuthStatus::Authenticated
                } else {
                    AuthStatus::NotAuthenticated
                }
            }
        };

        // If not authenticated, show the login screen TUI
        // (We skip server validation here and do it in the background later)
        if auth_status != AuthStatus::Authenticated {
            use crate::runner::login_screen::{LoginResult, LoginScreen};

            let message = match auth_status {
                AuthStatus::Expired => Some("Your session has expired.".to_string()),
                AuthStatus::NotAuthenticated => None,
                _ => None,
            };

            let mut login_screen = LoginScreen::new(cortex_home.clone(), message);

            match login_screen.run().await? {
                LoginResult::LoggedIn => {
                    tracing::info!("User logged in successfully");
                    // Reload auth token after login - this is critical!
                    if let Some(token) = cortex_login::get_auth_token() {
                        tracing::info!("Loaded fresh auth token from keyring after login");
                        provider_manager.set_auth_token(token);
                    } else {
                        tracing::warn!("Login succeeded but could not load token from keyring");
                    }
                    let _auth_status = AuthStatus::Authenticated;
                }
                LoginResult::ContinueWithApiKey => {
                    // Show API key setup instructions and exit
                    println!();
                    println!("\x1b[1;36m  API Key Authentication\x1b[0m");
                    println!();
                    println!("  To use Cortex with an API key, set the environment variable:");
                    println!();
                    println!("    \x1b[32mexport CORTEX_API_KEY=ctx-xxxxxxxxxxxxxxxx\x1b[0m");
                    println!();
                    println!("  On Windows (PowerShell):");
                    println!();
                    println!("    \x1b[32m$env:CORTEX_API_KEY = \"ctx-xxxxxxxxxxxxxxxx\"\x1b[0m");
                    println!();
                    println!("  Then run \x1b[1mcortex\x1b[0m again to start.");
                    println!();
                    return Ok(AppExitInfo::default().with_exit_reason(ExitReason::Normal));
                }
                LoginResult::Exit => {
                    println!();
                    println!("\x1b[33m  Login cancelled.\x1b[0m");
                    println!();
                    return Ok(AppExitInfo::default().with_exit_reason(ExitReason::Normal));
                }
                LoginResult::Failed(e) => {
                    tracing::error!("Login failed: {}", e);
                    return Ok(AppExitInfo::default().with_exit_reason(ExitReason::Error));
                }
            }
        }

        // ====================================================================
        // Fetch user info BEFORE showing TUI to avoid "User" placeholder
        // ====================================================================
        let mut user_name: Option<String> = None;
        let mut user_email: Option<String> = None;
        let mut org_name: Option<String> = None;

        // Fetch user info from /me API - wait for this before showing TUI
        if let Some(token) = cortex_login::get_auth_token() {
            tracing::debug!("Fetching user info from /me API...");
            if let Ok(client) = cortex_engine::create_default_client() {
                match client
                    .get("https://api.cortex.foundation/auth/me")
                    .bearer_auth(&token)
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(json) = resp.json::<serde_json::Value>().await {
                            if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
                                user_name = Some(name.to_string());
                                tracing::info!("User info loaded: {}", name);
                            }
                            if let Some(email) = json.get("email").and_then(|v| v.as_str()) {
                                user_email = Some(email.to_string());
                            }
                            if let Some(orgs) = json.get("organizations").and_then(|v| v.as_array())
                                && let Some(first_org) = orgs.first()
                                && let Some(org) =
                                    first_org.get("org_name").and_then(|v| v.as_str())
                            {
                                org_name = Some(org.to_string());
                            }
                        }
                    }
                    Ok(resp) => {
                        tracing::warn!("Failed to fetch user info: HTTP {}", resp.status());
                    }
                    Err(e) => {
                        tracing::warn!("Failed to fetch user info: {}", e);
                    }
                }
            }
        }

        // ====================================================================
        // Now initialize TUI after we have user info
        // ====================================================================

        let mut terminal = CortexTerminal::with_options(self.terminal_options)?;
        terminal.set_title("Cortex")?;

        // Get terminal size for app state
        let (width, height) = terminal.size()?;
        tracing::debug!("Terminal size: {}x{}", width, height);
        tracing::info!("Using direct provider mode");

        let provider = provider_manager.current_provider().to_string();
        let model = provider_manager.current_model().to_string();

        tracing::info!("Provider: {}, Model: {}", provider, model);

        // Create or resume Cortex session
        let cortex_session = if let Some(ref session_id) = self.cortex_session_id {
            tracing::info!("Resuming Cortex session: {}", session_id);
            CortexSession::load(session_id)?
        } else {
            tracing::info!("Creating new Cortex session");
            CortexSession::new(&provider, &model)?
        };

        let _session_id = cortex_session.id().to_string();

        // Create app state with user info already loaded
        let mut app_state = AppState::new()
            .with_model(model.clone())
            .with_provider(provider.clone())
            .with_terminal_size(width, height);

        // Set user info from pre-fetched data
        app_state.user_name = user_name;
        app_state.user_email = user_email;
        app_state.org_name = org_name;

        // Load last used theme from config
        if let Ok(config) = crate::providers::config::CortexConfig::load()
            && let Some(theme) = config.get_last_theme()
        {
            app_state.set_theme(theme);
            tracing::debug!("Loaded theme from config: {}", theme);
        }

        // ====================================================================
        // BACKGROUND TASKS: Spawn non-blocking operations in parallel
        // ====================================================================

        // 1. Session history loading (file I/O) - spawn in background
        let session_history_task =
            tokio::task::spawn_blocking(|| CortexSession::list_recent(50).ok());

        // 3. Models prefetch and session validation - spawn in background
        // We use a channel to receive results and update provider_manager later
        let models_and_validation_task = {
            let api_url = provider_manager.api_url().to_string();
            let token = cortex_login::get_auth_token();
            let cortex_home_clone = cortex_home.clone();
            tokio::spawn(async move {
                let mut validation_failed = false;
                let mut models: Option<Vec<cortex_engine::client::CortexModel>> = None;

                if let Some(token) = token {
                    // Create a client with timeout for faster failure on network issues
                    if let Ok(client) = cortex_engine::create_client_builder()
                        .connect_timeout(std::time::Duration::from_secs(3))
                        .timeout(std::time::Duration::from_secs(10))
                        .build()
                    {
                        // Session validation (lightweight)
                        tracing::debug!("Background: validating session with server...");
                        if let Ok(resp) = client
                            .get(format!("{}/v1/models", api_url))
                            .header("Authorization", format!("Bearer {}", token))
                            .send()
                            .await
                        {
                            let status = resp.status();
                            if status == reqwest::StatusCode::UNAUTHORIZED
                                || status == reqwest::StatusCode::FORBIDDEN
                            {
                                tracing::warn!(
                                    "Background: session validation failed ({})",
                                    status
                                );
                                // Delete invalidated credentials
                                if let Err(e) = logout_with_fallback(&cortex_home_clone) {
                                    tracing::warn!(
                                        "Failed to remove invalidated credentials: {}",
                                        e
                                    );
                                }
                                validation_failed = true;
                            } else if status.is_success() {
                                // Parse models from the same response to avoid another request
                                if let Ok(json) = resp.json::<serde_json::Value>().await
                                    && let Some(data) = json.get("data").and_then(|d| d.as_array())
                                {
                                    let parsed: Vec<cortex_engine::client::CortexModel> = data
                                        .iter()
                                        .filter_map(|m| serde_json::from_value(m.clone()).ok())
                                        .collect();
                                    if !parsed.is_empty() {
                                        tracing::info!(
                                            "Background: loaded {} models from API",
                                            parsed.len()
                                        );
                                        models = Some(parsed);
                                    }
                                }
                            }
                        }
                    }
                }

                (validation_failed, models)
            })
        };

        // ====================================================================
        // Collect background task results (with timeout to not block forever)
        // ====================================================================

        // Wait for session history (file I/O should be fast)
        if let Ok(Some(sessions)) = session_history_task.await {
            use crate::app::SessionSummary;
            for session in sessions {
                if let Ok(session_uuid) = uuid::Uuid::parse_str(&session.id) {
                    let summary = SessionSummary::new(session_uuid, session.title)
                        .with_message_count(session.message_count as usize)
                        .with_timestamp(session.updated_at);
                    app_state.session_history.push(summary);
                }
            }
            tracing::debug!(
                "Loaded {} Cortex session(s) from history",
                app_state.session_history.len()
            );
        }

        // Check validation result (with short timeout - don't block TUI)
        // We'll handle models update after event loop is created
        let validation_result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            models_and_validation_task,
        )
        .await;

        // If validation failed in background, update auth status
        // (This would show a toast in the TUI asking user to re-login)
        if let Ok(Ok((true, _))) = &validation_result {
            tracing::warn!("Session was invalidated by server - user should re-login");
            // The credentials are already deleted in the background task
            // We continue with the TUI but the user will get auth errors on API calls
        }

        // Extract and apply models if we got them from background task
        if let Ok(Ok((_, Some(models)))) = validation_result {
            provider_manager.set_cached_models(models);
        }

        // Create unified tool executor for Task and Batch tools
        // This requires an API key for the subagent's model client
        let unified_executor = {
            use cortex_engine::tools::{ExecutorConfig, UnifiedToolExecutor};
            use std::sync::Arc;

            // Get auth token using the centralized auth module
            // This properly handles: instance token → env var → keyring
            // Previous bug: only checked CORTEX_AUTH_TOKEN env var, missing keyring auth
            let api_key = cortex_engine::auth_token::get_auth_token(None).ok();
            let base_url = provider_manager.config().get_base_url(&provider);

            match api_key {
                Some(api_key) if !api_key.is_empty() => {
                    tracing::debug!(
                        "Using API key for UnifiedToolExecutor (length: {})",
                        api_key.len()
                    );
                    let mut config = ExecutorConfig::new(&provider, &model, &api_key)
                        .with_working_dir(std::env::current_dir().unwrap_or_default());

                    // Add base URL if configured
                    if let Some(url) = base_url {
                        config = config.with_base_url(url);
                    }

                    match UnifiedToolExecutor::new(config) {
                        Ok(executor) => {
                            tracing::info!(
                                "UnifiedToolExecutor initialized - Task and Batch tools enabled"
                            );
                            Some(Arc::new(executor))
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to create UnifiedToolExecutor: {} - Task/Batch will use fallback",
                                e
                            );
                            None
                        }
                    }
                }
                Some(_) => {
                    // Empty API key - treat same as None
                    tracing::warn!(
                        "Empty API key for provider '{}' - Task/Batch tools will use fallback",
                        provider
                    );
                    None
                }
                None => {
                    tracing::warn!(
                        "No API key configured for provider '{}' - Task/Batch tools will use fallback",
                        provider
                    );
                    None
                }
            }
        };

        // Create tool registry for executing tools
        let tool_registry = {
            use cortex_engine::tools::ToolRegistry;
            use std::sync::Arc;

            let registry = ToolRegistry::new();
            tracing::info!("Initialized ToolRegistry");
            Arc::new(registry)
        };

        // Create event loop with provider manager, cortex session, and tool registry
        let mut event_loop = EventLoop::new(app_state)
            .with_provider_manager(provider_manager)
            .with_cortex_session(cortex_session)
            .with_tool_registry(tool_registry);

        // Add unified executor if available
        if let Some(executor) = unified_executor {
            event_loop = event_loop.with_unified_executor(executor);
        }

        // Load persisted MCP server configurations
        event_loop.load_mcp_servers();

        // Handle initial prompt if provided
        if let Some(prompt) = self.initial_prompt {
            tracing::debug!("Initial prompt queued: {}", prompt);
            // Initial prompt sending planned for future implementation
        }

        // Run the main event loop
        let result = event_loop.run(&mut terminal).await;

        // Capture logout message before cleanup
        let logout_message = event_loop.app_state.logout_message.take();

        // Cleanup
        terminal.show_cursor()?;
        drop(terminal);

        match result {
            Ok(()) => {
                tracing::info!("Cortex TUI exited normally");
                let mut exit_info = AppExitInfo {
                    conversation_id: None, // Cortex sessions use string IDs
                    exit_reason: ExitReason::Normal,
                    exit_message: None,
                };
                if let Some(msg) = logout_message {
                    exit_info = exit_info.with_exit_message(msg);
                }
                Ok(exit_info)
            }
            Err(e) => {
                tracing::error!("Cortex TUI error: {}", e);
                Err(e)
            }
        }
    }

    /// Run using legacy SessionBridge mode.
    async fn run_legacy_bridge(self) -> Result<AppExitInfo> {
        // Initialize sound system early for audio notifications
        // This spawns a background thread for audio playback
        crate::sound::init();

        // Initialize terminal
        let mut terminal = CortexTerminal::with_options(self.terminal_options)?;
        terminal.set_title("Cortex")?;

        // Get terminal size for app state
        let (width, height) = terminal.size()?;
        tracing::debug!("Terminal size: {}x{}", width, height);
        tracing::info!("Using legacy SessionBridge mode");

        // Create or resume session via bridge
        let session_bridge = if let Some(id) = self.conversation_id {
            tracing::info!("Resuming session: {}", id);
            SessionBridge::resume(self.config.clone(), id).await?
        } else {
            tracing::info!("Creating new session");
            SessionBridge::new(self.config.clone()).await?
        };

        let conversation_id = *session_bridge.conversation_id();

        // Create app state with configuration
        let mut app_state = AppState::new()
            .with_model(self.config.model.clone())
            .with_provider(self.config.model_provider_id.clone())
            .with_terminal_size(width, height);

        // Load last used theme from config
        if let Ok(config) = crate::providers::config::CortexConfig::load()
            && let Some(theme) = config.get_last_theme()
        {
            app_state.set_theme(theme);
            tracing::debug!("Loaded theme from config: {}", theme);
        }

        // Load session history from cortex-core
        if let Ok(sessions) = cortex_engine::list_sessions(&self.config.cortex_home) {
            use crate::app::SessionSummary;
            for session in sessions.into_iter().take(50) {
                let timestamp = chrono::DateTime::parse_from_rfc3339(&session.timestamp)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());

                let title = session.model.clone().unwrap_or_else(|| {
                    session
                        .cwd
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Session".to_string())
                });

                if let Ok(session_uuid) = uuid::Uuid::parse_str(&session.id) {
                    let summary = SessionSummary::new(session_uuid, title)
                        .with_message_count(session.message_count)
                        .with_timestamp(timestamp);
                    app_state.session_history.push(summary);
                }
            }
            tracing::debug!(
                "Loaded {} session(s) from history",
                app_state.session_history.len()
            );
        }

        // Create tool registry for executing tools
        let tool_registry = {
            use cortex_engine::tools::ToolRegistry;
            use std::sync::Arc;

            let registry = ToolRegistry::new();
            tracing::info!("Initialized ToolRegistry (legacy mode)");
            Arc::new(registry)
        };

        // Create event loop with session bridge and tool registry
        let mut event_loop = EventLoop::new(app_state)
            .with_session(session_bridge)
            .with_tool_registry(tool_registry);

        // Load persisted MCP server configurations
        event_loop.load_mcp_servers();

        // Handle initial prompt if provided
        if let Some(prompt) = self.initial_prompt {
            tracing::debug!("Initial prompt queued: {}", prompt);
        }

        // Run the main event loop
        let result = event_loop.run(&mut terminal).await;

        // Cleanup
        terminal.show_cursor()?;
        drop(terminal);

        match result {
            Ok(()) => {
                tracing::info!("Cortex TUI exited normally");
                Ok(AppExitInfo {
                    conversation_id: Some(conversation_id),
                    exit_reason: ExitReason::Normal,
                    exit_message: None,
                })
            }
            Err(e) => {
                tracing::error!("Cortex TUI error: {}", e);
                Err(e)
            }
        }
    }

    /// Run in demo mode without a backend session.
    ///
    /// Demo mode runs the TUI without connecting to a backend session.
    /// This is useful for:
    /// - Testing the UI without API keys
    /// - Demonstrating the interface
    /// - Development and debugging
    ///
    /// In demo mode, the TUI is fully functional for navigation and input,
    /// but messages won't be sent to any LLM backend.
    ///
    /// # Returns
    ///
    /// Returns `AppExitInfo` with no conversation ID.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal initialization fails or the event loop
    /// encounters a fatal error.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let exit_info = AppRunner::new(config).run_demo().await?;
    /// ```
    pub async fn run_demo(self) -> Result<AppExitInfo> {
        tracing::info!("Starting Cortex TUI in demo mode");

        // Initialize terminal
        let mut terminal = CortexTerminal::with_options(self.terminal_options)?;
        terminal.set_title("Cortex (Demo)")?;

        // Get terminal size
        let (width, height) = terminal.size()?;
        tracing::debug!("Terminal size (demo): {}x{}", width, height);

        // Create app state for demo mode
        let mut app_state = AppState::new()
            .with_model("demo-model".to_string())
            .with_provider("demo".to_string())
            .with_terminal_size(width, height);

        // Load last used theme from config
        if let Ok(config) = crate::providers::config::CortexConfig::load()
            && let Some(theme) = config.get_last_theme()
        {
            app_state.set_theme(theme);
            tracing::debug!("Loaded theme from config: {}", theme);
        }

        // Create event loop WITHOUT session bridge
        let mut event_loop = EventLoop::new(app_state);

        // Load persisted MCP server configurations
        event_loop.load_mcp_servers();

        // Run the event loop
        let result = event_loop.run(&mut terminal).await;

        // Cleanup
        terminal.show_cursor()?;
        drop(terminal);

        match result {
            Ok(()) => {
                tracing::info!("Cortex TUI (demo) exited normally");
                Ok(AppExitInfo::default())
            }
            Err(e) => {
                tracing::error!("Cortex TUI (demo) error: {}", e);
                Err(e)
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::terminal::TerminalOptions;

    #[test]
    fn test_app_runner_builder() {
        let config = Config::default();

        let runner = AppRunner::new(config.clone())
            .with_initial_prompt("Hello")
            .inline();

        assert_eq!(runner.initial_prompt, Some("Hello".to_string()));
        assert!(!runner.terminal_options.alternate_screen);
    }

    #[test]
    fn test_app_runner_model_provider() {
        let config = Config::default();
        let runner = AppRunner::new(config);

        // Default config values
        assert!(!runner.model().is_empty());
        assert!(!runner.provider().is_empty());
    }

    #[test]
    fn test_app_runner_terminal_options() {
        let config = Config::default();

        // Default options
        let runner = AppRunner::new(config.clone());
        assert!(runner.terminal_options.alternate_screen);

        // Custom options
        let custom_options = TerminalOptions::new()
            .alternate_screen(false)
            .mouse_capture(false);

        let runner = AppRunner::new(config.clone()).with_terminal_options(custom_options);
        assert!(!runner.terminal_options.alternate_screen);
        assert!(!runner.terminal_options.mouse_capture);

        // Inline mode
        let runner = AppRunner::new(config).inline();
        assert!(!runner.terminal_options.alternate_screen);
    }

    #[test]
    fn test_app_runner_direct_provider_mode() {
        let config = Config::default();

        // Default is direct provider mode
        let runner = AppRunner::new(config.clone());
        assert!(runner.use_direct_provider);

        // Can explicitly enable
        let runner = AppRunner::new(config.clone()).direct_provider(true);
        assert!(runner.use_direct_provider);

        // Can switch to legacy mode
        let runner = AppRunner::new(config.clone()).legacy_backend();
        assert!(!runner.use_direct_provider);

        // with_conversation_id switches to legacy
        let id = ConversationId::new();
        let runner = AppRunner::new(config).with_conversation_id(id);
        assert!(!runner.use_direct_provider);
    }

    #[test]
    fn test_app_runner_cortex_session_id() {
        let config = Config::default();

        let runner = AppRunner::new(config).with_cortex_session_id("test-session-123");
        assert_eq!(
            runner.cortex_session_id,
            Some("test-session-123".to_string())
        );
        // Direct provider mode should still be enabled
        assert!(runner.use_direct_provider);
    }
}
