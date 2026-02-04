//! Tool handlers.

pub mod apply_patch;
pub mod batch;
mod create_agent;
mod edit_file;
pub mod edit_strategies;
mod fetch_url;
mod file_ops;
mod glob;
mod grep;
mod local_shell;
pub mod lsp_tool;
mod plan;
mod propose;
mod questions;
pub mod skill;
pub mod subagent;
pub mod task;
mod todo;
mod web_search;

pub use apply_patch::ApplyPatchHandler;
pub use batch::{
    BatchCallResult, BatchParams, BatchResult, BatchToolArgs, BatchToolCall, BatchToolExecutor,
    BatchToolHandler, DISALLOWED_TOOLS, LegacyBatchToolCall, MAX_BATCH_SIZE, batch_tool_definition,
    execute_batch,
};
pub use cortex_common::DEFAULT_BATCH_TIMEOUT_SECS;
pub use create_agent::CreateAgentHandler;
pub use edit_file::PatchHandler;

// Edit strategies exports - 8 cascading replacement strategies
pub use edit_strategies::{
    BlockAnchorReplacer,
    CascadeReplacer,
    CascadeResult,
    ContextAwareReplacer,
    EditError,
    // Core trait and types
    EditStrategy,
    EscapeNormalizedReplacer,
    FuzzyMatcher,
    IndentationFlexibleReplacer,
    LineTrimmedReplacer,
    MatchError,
    MatchResult,
    // Individual strategies
    SimpleReplacer,
    // Legacy compatibility
    Strategy,
    TrimmedBoundaryReplacer,
    WhitespaceNormalizedReplacer,
    fuzzy_replace,
};
pub use fetch_url::FetchUrlHandler;
pub use file_ops::{ReadFileHandler, SearchFilesHandler, TreeHandler, WriteFileHandler};
// Backward compatibility alias
pub use file_ops::TreeHandler as ListDirHandler;
pub use glob::GlobHandler;
pub use grep::GrepHandler;
pub use local_shell::LocalShellHandler;
pub use lsp_tool::{LspOperation, LspParams, execute_lsp, lsp_tool_definition};
pub use plan::{PlanHandler, PlanTask, PlanTaskStatus};
pub use propose::ProposeHandler;
pub use questions::QuestionsHandler;
pub use todo::{TodoItem, TodoPriority, TodoReadHandler, TodoStatus, TodoWriteHandler};
pub use web_search::WebSearchHandler;

// Skill exports
pub use skill::{
    LoadedSkill, SkillArg, SkillDefinition, SkillHandler, SkillLoader,
    SkillRegistry as SkillToolRegistry, SkillSource,
};

// Subagent and Task exports
pub use subagent::{
    ProgressEvent, SubagentConfig, SubagentExecutor, SubagentProgress, SubagentResult,
    SubagentSession, SubagentStatus, SubagentType,
};
pub use task::{ListSubagentsHandler, SimpleTaskHandler, TaskHandler};

pub use super::context::ToolContext;
pub use super::spec::{ToolHandler, ToolResult};
