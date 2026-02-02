//! REST API routes and handlers.
//!
//! This module provides the HTTP API for the Cortex app server, organized into
//! logical submodules for maintainability.

mod agents;
mod ai;
mod chat;
mod discovery;
mod files;
mod git;
mod health;
mod models;
mod path_security;
mod proxy;
mod search;
mod sessions;
mod stored_sessions;
mod terminals;
mod tools;
pub mod types;

use std::sync::Arc;

use axum::{
    Router,
    routing::{delete, get, post},
};

use crate::state::AppState;

// Re-export types for external use
pub use types::{
    AgentDefinition, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, HealthResponse,
    ModelInfo, SessionResponse,
};

/// Create the API routes.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Health and metrics
        .route("/health", get(health::health_check))
        .route("/metrics", get(health::get_metrics))
        // mDNS Discovery
        .route("/discover", get(discovery::discover_servers))
        // Port proxy (for dev servers)
        .route("/ports", get(proxy::list_open_ports))
        .route("/proxy/:port", get(proxy::proxy_to_port))
        .route("/proxy/:port/*path", get(proxy::proxy_to_port_path))
        // Sessions
        .route("/sessions", post(sessions::create_session))
        .route("/sessions", get(sessions::list_sessions))
        .route("/sessions/:id", get(sessions::get_session))
        .route("/sessions/:id", delete(sessions::delete_session))
        .route("/sessions/:id/messages", post(sessions::send_message))
        .route("/sessions/:id/messages", get(sessions::list_messages))
        // Models
        .route("/models", get(models::list_models))
        .route("/models/:id", get(models::get_model))
        // Completions (OpenAI-compatible)
        .route("/chat/completions", post(chat::chat_completions))
        // Tools
        .route("/tools", get(tools::list_tools))
        .route("/tools/:name/execute", post(tools::execute_tool))
        // File Explorer
        .route("/files", post(files::list_files))
        .route("/files/tree", get(files::get_file_tree))
        .route("/files/read", post(files::read_file))
        .route("/files/write", post(files::write_file))
        .route("/files/delete", post(files::delete_file))
        .route("/files/mkdir", post(files::create_directory))
        .route("/files/rename", post(files::rename_file))
        .route("/files/watch", get(files::watch_files))
        // Custom agents
        .route("/agents", get(agents::list_agents))
        .route("/agents", post(agents::create_agent))
        .route("/agents/builtin", get(agents::list_builtin_agents))
        .route("/agents/import", post(agents::import_agent))
        .route(
            "/agents/generate-prompt",
            post(agents::generate_agent_prompt),
        )
        .route("/agents/:name", get(agents::get_agent))
        .route("/agents/:name", axum::routing::put(agents::update_agent))
        .route("/agents/:name", delete(agents::delete_agent))
        // Stored sessions (persistent)
        .route(
            "/stored-sessions",
            get(stored_sessions::list_stored_sessions),
        )
        .route(
            "/stored-sessions/:id",
            get(stored_sessions::get_stored_session),
        )
        .route(
            "/stored-sessions/:id",
            delete(stored_sessions::delete_stored_session),
        )
        .route(
            "/stored-sessions/:id/history",
            get(stored_sessions::get_session_history),
        )
        // Terminals (background processes)
        .route("/terminals", get(terminals::list_terminals))
        .route("/terminals/:id/logs", get(terminals::get_terminal_logs))
        // Search
        .route("/search", get(search::search_project))
        // Git
        .route("/git/status", get(git::git_status))
        .route("/git/branch", get(git::git_branch))
        .route("/git/branches", get(git::git_branches))
        .route("/git/diff", get(git::git_diff))
        .route("/git/blame", get(git::git_blame))
        .route("/git/log", get(git::git_log))
        .route("/git/stage", post(git::git_stage))
        .route("/git/unstage", post(git::git_unstage))
        .route("/git/stage-all", post(git::git_stage_all))
        .route("/git/unstage-all", post(git::git_unstage_all))
        .route("/git/commit", post(git::git_commit))
        .route("/git/checkout", post(git::git_checkout))
        .route("/git/push", post(git::git_push))
        .route("/git/pull", post(git::git_pull))
        .route("/git/fetch", post(git::git_fetch))
        .route("/git/discard", post(git::git_discard))
        .route("/git/branch/create", post(git::git_create_branch))
        .route("/git/branch/delete", post(git::git_delete_branch))
        .route("/git/merge", post(git::git_merge))
        .route("/git/stash/list", get(git::git_stash_list))
        .route("/git/stash/create", post(git::git_stash_create))
        .route("/git/stash/apply", post(git::git_stash_apply))
        .route("/git/stash/pop", post(git::git_stash_pop))
        .route("/git/stash/drop", post(git::git_stash_drop))
        // AI endpoints
        .route("/ai/inline", post(ai::ai_inline))
        .route("/ai/predict", post(ai::ai_predict))
}
