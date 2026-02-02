//! Cortex Engine - Business logic for the Cortex CLI agent.
//!
//! This crate contains the core functionality:
//! - Agent orchestration and conversation management
//! - Model client implementations (OpenAI, Anthropic, etc.)
//! - Tool routing and execution
//! - Sandbox management
//! - Configuration loading and management
//!
//! NOTE: This crate should NOT contain any UI/TUI code.
//! Terminal rendering belongs in cortex-tui.

#![deny(clippy::print_stdout, clippy::print_stderr)]
#![allow(
    clippy::collapsible_if,
    clippy::needless_return,
    clippy::manual_range_contains,
    clippy::bool_assert_comparison,
    clippy::needless_borrows_for_generic_args,
    clippy::uninlined_format_args,
    clippy::manual_strip,
    clippy::implicit_saturating_sub,
    clippy::type_complexity,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::module_name_repetitions,
    clippy::too_many_lines,
    clippy::if_same_then_else,
    clippy::needless_range_loop,
    clippy::ptr_arg,
    clippy::single_match,
    clippy::match_single_binding,
    clippy::struct_field_names,
    clippy::similar_names,
    clippy::redundant_closure_for_method_calls,
    clippy::unnecessary_wraps,
    clippy::useless_format,
    clippy::from_over_into,
    clippy::cast_lossless,
    clippy::match_wildcard_for_single_variants,
    clippy::items_after_statements,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::trivially_copy_pass_by_ref,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::option_if_let_else,
    clippy::return_self_not_must_use,
    clippy::redundant_else,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::single_match_else,
    clippy::match_same_arms,
    clippy::unnecessary_fallible_conversions,
    clippy::derivable_impls,
    clippy::should_implement_trait,
    clippy::format_in_format_args,
    clippy::needless_pass_by_value,
    clippy::default_trait_access,
    clippy::unused_self,
    clippy::inline_always,
    clippy::wildcard_imports,
    clippy::large_types_passed_by_value,
    clippy::significant_drop_tightening,
    clippy::len_without_is_empty,
    clippy::iter_without_into_iter,
    clippy::new_without_default,
    clippy::struct_excessive_bools,
    clippy::redundant_closure,
    clippy::unnecessary_filter_map,
    clippy::inherent_to_string,
    clippy::unwrap_or_default,
    clippy::manual_map,
    clippy::manual_is_multiple_of,
    clippy::needless_borrow,
    clippy::match_like_matches_macro,
    clippy::or_fun_call,
    clippy::unnecessary_lazy_evaluations,
    clippy::stable_sort_primitive,
    clippy::explicit_iter_loop,
    clippy::redundant_pattern_matching,
    clippy::string_add_assign,
    clippy::let_and_return,
    clippy::comparison_to_empty,
    clippy::collapsible_else_if,
    clippy::map_entry,
    clippy::manual_find,
    clippy::single_char_add_str,
    clippy::wildcard_in_or_patterns,
    clippy::unnecessary_map_or,
    clippy::get_first,
    clippy::ref_as_ptr,
    clippy::borrow_as_ptr,
    clippy::default_constructed_unit_structs,
    clippy::manual_inspect,
    clippy::too_many_arguments,
    clippy::unnecessary_cast,
    clippy::let_underscore_future,
    clippy::implied_bounds_in_impls,
    clippy::useless_asref,
    clippy::ref_option,
    clippy::map_clone,
    clippy::field_reassign_with_default,
    clippy::assigning_clones,
    clippy::vec_init_then_push,
    clippy::while_let_on_iterator,
    clippy::useless_vec,
    clippy::clone_on_copy,
    clippy::trim_split_whitespace,
    clippy::assertions_on_constants,
    clippy::panicking_unwrap,
    clippy::manual_ok_err
)]
// Allow unknown cfg values for optional features
#![allow(unexpected_cfgs)]

// === CORE MODULES ===
pub mod agent;
pub mod client;
pub mod config;
pub mod error;
pub mod message_parts;
pub mod prompt;
pub mod session;
pub mod tools;

// === EXECUTION & SAFETY ===
pub mod approval;
pub mod command_executor;
pub mod exec;
pub mod permission;
pub mod safety;
pub mod sandbox;
pub mod sandboxing;
pub mod security;

// === CONTEXT & CONVERSATION ===
pub mod compact;
pub mod compaction;
pub mod context;
pub mod conversation;
pub mod conversation_state;
pub mod memory;
pub mod small_model;
pub mod summarization;

// === PROVIDERS & CLIENTS ===
pub mod acp;
pub mod api_client;
pub use api_client::{
    DEFAULT_TIMEOUT, HEALTH_CHECK_TIMEOUT, STREAMING_TIMEOUT, USER_AGENT, create_blocking_client,
    create_blocking_client_with_timeout, create_client_builder, create_client_with_timeout,
    create_default_client, create_health_check_client, create_streaming_client,
};
pub mod billing_client;
pub mod github;
pub mod jira;
pub mod linear;
pub mod mcp;

// === NETWORK DISCOVERY ===
pub mod mdns;

// === PERSISTENCE ===
pub mod git_snapshot;
pub mod rollout;
pub mod share_service;
pub mod snapshot;
pub mod state;

// === EXTENSIBILITY ===
pub mod agents;
pub mod autonomy;
pub mod commands;
pub mod custom_command;
pub mod delegates;
pub mod extensions;
pub mod hooks;
pub mod output_format;
pub mod skills;
pub mod workspace_scripts;

// === UTILITIES ===
pub mod ai_utils;
pub mod async_utils;
pub mod auth;
pub mod auth_token;
pub mod code_analysis;
pub mod config_loader;
pub mod context_utils;
pub mod diff;
pub mod embeddings;
pub mod environment;
pub mod events;
pub mod features;
pub mod file_utils;
pub mod formatting;
pub mod git;
pub mod git_info;
pub mod git_ops;
pub mod health;
pub mod input;
pub mod instructions;
pub mod json_utils;
pub mod language_utils;
pub mod logging;
pub mod metrics;
pub mod model_family;

pub mod output;
pub mod parse_command;
pub mod plugin;
pub mod process_utils;
pub mod project;
pub mod prompt_builder;
pub mod ratelimit;
pub mod response;
pub mod retry;
pub mod review;
pub mod search;
pub mod secrets;
pub mod shell;
pub mod streaming;
pub mod tasks;
pub mod template;
pub mod testing;
pub mod text_encoding;
pub mod text_utils;
pub mod tokenizer;
pub mod truncate;
pub mod unified_exec;
pub mod validation;
pub mod version_utils;
pub mod workspace;

// Background terminal management (different from TUI terminal)
pub mod terminal;

// REMOVED: These belong in cortex-tui or cortex-cli
// pub mod tui_terminal;   // UI code - moved to cortex-tui
// pub mod cli_utils;  // CLI code - moved to cortex-cli

// Re-exports
pub use agent::CortexAgent;
pub use client::{CompletionRequest, ModelClient, ResponseEvent};
pub use config::{Config, ConfigOverrides};
pub use conversation::ConversationManager;
pub use error::{CortexError, Result};
pub use message_parts::MessagePartsBuilder;
pub use safety::{RiskLevel, SafetyAnalysis, analyze_command};
pub use session::{Session, SessionHandle, SessionInfo, list_sessions};

// Auth re-exports
pub use auth::{AuthConfig, AuthManager, CredentialStore};

// Environment re-exports
pub use environment::{EnvironmentContext, EnvironmentContextBuilder};

// Parse command re-exports
pub use parse_command::{CommandBuilder, CommandParser, ParsedCommand};

// Truncate re-exports
pub use truncate::{TruncateConfig, TruncateResult, TruncateStrategy, truncate};

// Diff re-exports
pub use diff::{DiffBuilder, DiffLine, FileDiff, Hunk, UnifiedDiff};

// Embeddings re-exports
pub use embeddings::{EmbeddingClient, TextChunker, VectorStore, cosine_similarity};

// Memory/RAG re-exports
pub use memory::{
    ContextAssembler, ContextConfig, Embedder, EmbedderConfig, Memory, MemoryFilter,
    MemoryMetadata, MemoryScope, MemoryStore, MemoryStoreConfig, MemorySystem, MemorySystemConfig,
    MemoryType, RetrievedContext, Retriever, RetrieverConfig, SearchQuery,
    SearchResult as MemorySearchResult, StorageBackend,
};

// Metrics re-exports
pub use metrics::{MetricsCollector, MetricsConfig, MetricsSnapshot};

// Rate limit re-exports
pub use ratelimit::{ConcurrencyLimiter, RateLimitConfig, SlidingWindow, TokenBucket};

// Retry re-exports
pub use retry::{BackoffStrategy, CircuitBreaker, Retry, RetryBuilder, RetryConfig};

// Response re-exports
pub use response::{ProcessedResponse, ResponseChunk, ResponseProcessor, StreamAggregator};

// Shell re-exports
pub use shell::{ShellCommand, ShellInfo, ShellType};

// Permission re-exports
pub use permission::{
    PatternMatcher, Permission as ToolPermission, PermissionCheckResult, PermissionContext,
    PermissionManager, PermissionManagerConfig, PermissionPrompt, PermissionResponse,
    PermissionScope, PermissionStorage, PromptResponse, RiskLevel as PermissionRiskLevel,
    global_manager as global_permission_manager,
    init_global_manager as init_global_permission_manager,
};

// Snapshot re-exports
pub use snapshot::SnapshotService;

// Share re-exports
pub use share_service::ShareService;

// REMOVED: Terminal re-exports (moved to cortex-tui)
// pub use terminal::{TerminalCapabilities, Style, Spinner, ProgressBar};

// Test module
#[cfg(test)]
mod tests;

// Tasks re-exports
pub use tasks::{TaskMeta, TaskPriority, TaskResult, TaskStatus, TaskType};

// State re-exports
pub use state::{ServiceState, ServiceStatus, SessionPhase, SessionState, TurnPhase, TurnState};

// Unified exec re-exports
pub use unified_exec::{ExecRequest, ExecResult, ExecSession, UnifiedExecutor};

// Git info re-exports
pub use git_info::{GitBranch, GitCommit, GitDiff, GitInfo};

// Git hooks re-exports
pub use git::{
    CommitMsgHook, GitHook, GitHookRunner, HookConfig, HookExecutionResult, HookIssue, HookManager,
    HookStatus, IssueCategory, IssueSeverity, PatternCheck, PreCommitHook, PrePushHook,
    PrepareCommitMsgHook, should_ignore_path,
};

// Summarization re-exports
pub use summarization::{SUMMARIZATION_SYSTEM_PROMPT, SummarizationStrategy};

// Small model re-exports
pub use small_model::{
    PROVIDER_ENV_VARS, SMALL_MODELS, SmallModelConfig, SmallModelInfo, SmallModelSelector,
    SmallModelTask, call_small_model, call_with_client, classify_intent,
    detect_available_providers, extract_keywords, generate_commit_message, generate_summary,
    generate_title, get_provider_api_key, get_small_model, global_selector, has_small_model,
    is_provider_available, list_small_models,
};

// Model family re-exports
pub use model_family::{ModelCapabilities, ModelFamily, ModelPricing, ModelTier};

// Text encoding re-exports
pub use text_encoding::{Encoding, LineEnding, decode, encode};

// Project re-exports
pub use project::{Framework, ProjectInfo, ProjectType};

// MCP re-exports (OAuth support only - runtime code removed)
pub use mcp::{McpConfig, McpServerConfig};

// GitHub re-exports
pub use github::{
    GitHubClient, GitHubEvent, IssueCommentEvent, IssueEvent, PullRequestEvent, PullRequestInfo,
    PullRequestReviewEvent, WorkflowConfig, generate_workflow, parse_event,
};

// Linear re-exports
pub use linear::{
    Comment as LinearComment, CommentsConnection as LinearCommentsConnection,
    CreateIssueInput as LinearCreateIssueInput, Issue as LinearIssue,
    IssueDetails as LinearIssueDetails, IssueState as LinearIssueState, LINEAR_OAUTH_AUTHORIZE,
    LINEAR_OAUTH_TOKEN, Label as LinearLabel, LabelsConnection as LinearLabelsConnection,
    LinearClient, LinearRef, Team as LinearTeam, TeamInfo as LinearTeamInfo, User as LinearUser,
    extract_linear_issues,
};

// Jira re-exports
pub use jira::{
    JiraClient, JiraComment, JiraCreateIssueInput, JiraIssue, JiraIssueDetails, JiraIssueType,
    JiraPriority, JiraProject, JiraRef, JiraStatus, JiraTransition, JiraUser, extract_jira_issues,
};

// ACP re-exports
pub use acp::{AcpServer, InitializeRequest, NewSessionRequest, PromptRequest};

// mDNS re-exports
pub use mdns::{DiscoveredService, MdnsBrowser, MdnsService, ServiceBrowser, ServiceInfo};

// Skills re-exports
pub use skills::{Skill, SkillBuilder, SkillMetadata, SkillRegistry, SkillSource};

// Hooks re-exports
pub use hooks::{
    HookContext, HookDefinition, HookEvent, HookHandler, HookRegistry, HookResult, SecurityHook,
};

// Commands re-exports
pub use commands::{
    CommandContext, CommandHandler, CommandInvocation, CommandMeta, CommandRegistry, CommandResult,
};

// Custom commands re-exports
pub use custom_command::{
    CommandExecutionResult, CommandSource as CustomCommandSource, CustomCommand,
    CustomCommandConfig, CustomCommandRegistry, TemplateContext, expand_template,
    global_registry as custom_command_registry,
    init_global_registry as init_custom_command_registry,
    try_global_registry as try_custom_command_registry,
};

// Agents re-exports
pub use agents::{Agent, AgentContext, AgentMetadata, AgentRegistry, AgentResult, AgentSource};

// Delegates re-exports
pub use delegates::{Delegate, DelegateMetadata, DelegateRegistry, DelegateSource, ToolsConfig};

// Autonomy re-exports
pub use autonomy::{
    AutonomyDecision, AutonomyLevel, AutonomyManager, CommandCategory, RiskClassification,
    RiskLevel as AutonomyRiskLevel, SafetyAnalysis as AutonomySafetyAnalysis, classify_command,
};

// Output format re-exports
pub use output_format::{JsonResult, OutputFormat, OutputWriter, StreamEvent};

// Plugin re-exports (enhanced module)
pub use plugin::{
    CombinedHookResult,
    // Hooks
    CompactionHookContext,
    // Loader
    DiscoveredPlugin,
    ErrorHookContext,
    // Core types
    HookContext as PluginHookContext,
    HookDispatcher,
    HookRegistration,
    HookResponse as PluginHookResponse,
    LoadedPluginInfo,
    MessageHookContext,
    PermissionHookContext,
    Plugin,
    PluginConfig,
    // Config
    PluginConfigBuilder,
    PluginConfigEntry,
    PluginEvent,
    PluginFormat,
    PluginHook,
    PluginInfo,
    PluginInstance,
    PluginKind,
    PluginLoadError,
    PluginLoadResult,
    PluginLoader,
    // Manager (PluginRegistry is type alias for backwards compatibility)
    PluginManager,
    PluginManagerLoadResult,
    PluginPermission,
    PluginRegistry,
    PluginSettings,
    PluginSource,
    PluginState,
    PluginStats,
    PluginsConfig,
    RiskLevel as PluginRiskLevel,
    SessionHookContext,
    ToolHookContext,
    global_manager as global_plugin_manager,
    init_global_manager as init_global_plugin_manager,
};

// Re-export protocol types
pub use cortex_protocol as protocol;

// Re-export extension crates for direct use (Phase 1)
pub use cortex_agents_ext;
pub use cortex_batch;
pub use cortex_hooks_ext;
pub use cortex_lsp;
pub use cortex_plugins_ext;
pub use cortex_share;
pub use cortex_snapshot;

// Re-export extension crates for direct use (Phase 2)
pub use cortex_compact_ext;
pub use cortex_experimental;
pub use cortex_ghost;
pub use cortex_migrations;
pub use cortex_ratelimits;
pub use cortex_resume;
pub use cortex_review_ext;

// === INTEGRATIONS MODULE ===
pub mod integrations;

// Integration re-exports
pub use integrations::{
    ExperimentalIntegration, GhostIntegration, LspIntegration, MigrationIntegration,
    RatelimitIntegration, ResumeIntegration,
};
