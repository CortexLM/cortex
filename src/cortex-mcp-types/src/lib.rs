//! Cortex MCP Types - Model Context Protocol type definitions.
//!
//! This crate provides comprehensive type definitions for the Model Context Protocol (MCP),
//! enabling AI applications to connect with data sources and tools through a standardized interface.
//!
//! # Features
//! - JSON-RPC 2.0 request/response types
//! - Tool, Resource, and Prompt definitions
//! - Serialization/deserialization with serde
//! - JSON Schema generation with schemars
//!
//! # Example
//! ```rust
//! use cortex_mcp_types::{Tool, ToolInputSchema, PropertySchema, Resource, Prompt};
//!
//! let tool = Tool::new("search", "Search for files")
//!     .with_schema(ToolInputSchema::object()
//!         .property("query", PropertySchema::string().description("Search query"))
//!         .required(vec!["query"]));
//! ```

// ============================================================================
// Module declarations
// ============================================================================

mod capabilities;
mod content;
mod initialization;
mod jsonrpc;
mod logging;
mod notifications;
mod prompts;
mod resources;
mod roots;
mod sampling;
mod tools;

/// MCP method name constants.
pub mod methods;

// ============================================================================
// Protocol Version
// ============================================================================

/// Current MCP protocol version.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Latest MCP protocol version.
pub const LATEST_PROTOCOL_VERSION: &str = "2024-11-05";

// ============================================================================
// Re-exports
// ============================================================================

// JSON-RPC types
pub use jsonrpc::{
    ErrorCode, JSONRPC_VERSION, JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    RequestId,
};

// Initialization types
pub use initialization::{Implementation, InitializeParams, InitializeResult};

// Capability types
pub use capabilities::{
    ClientCapabilities, LoggingCapability, PromptsCapability, ResourcesCapability, RootsCapability,
    SamplingCapability, ServerCapabilities, ToolsCapability,
};

// Tool types
pub use tools::{
    CallToolParams, CallToolResult, ListToolsParams, ListToolsResult, PropertySchema, Tool,
    ToolInputSchema,
};

// Resource types
pub use resources::{
    ListResourceTemplatesResult, ListResourcesParams, ListResourcesResult, ReadResourceParams,
    ReadResourceResult, Resource, ResourceContent, ResourceTemplate, SubscribeParams,
    UnsubscribeParams,
};

// Prompt types
pub use prompts::{
    GetPromptParams, GetPromptResult, ListPromptsParams, ListPromptsResult, Prompt, PromptArgument,
    PromptMessage, Role,
};

// Content types
pub use content::Content;

// Logging types
pub use logging::{LogLevel, LogMessage, SetLogLevelParams};

// Sampling types
pub use sampling::{
    IncludeContext, ModelHint, ModelPreferences, SamplingMessage, SamplingRequest, SamplingResult,
    StopReason,
};

// Root types
pub use roots::{ListRootsResult, Root};

// Notification types
pub use notifications::{CancelledNotification, ProgressNotification, ProgressToken};
