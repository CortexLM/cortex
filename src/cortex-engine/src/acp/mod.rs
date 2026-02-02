//! ACP (Agent Client Protocol) module.
//!
//! This module implements the ACP protocol for IDE integration,
//! enabling editors like Zed to communicate with the Cortex Agent.
//!
//! ## Protocol Overview
//!
//! ACP uses JSON-RPC 2.0 over either stdio or HTTP transport:
//! - `initialize` - Initialize the connection with capabilities
//! - `session/new` - Create a new agent session
//! - `session/load` - Load an existing session
//! - `session/list` - List available sessions
//! - `session/prompt` - Send a prompt to the agent
//! - `session/cancel` - Cancel the current operation
//! - `models/list` - List available models
//! - `agents/list` - List available agents
//!
//! ## Streaming
//!
//! Session updates are streamed via notifications:
//! - `session/update` - Contains agent message chunks, tool calls, etc.

pub mod handler;
pub mod protocol;
pub mod server;
pub mod types;

pub use handler::{AcpHandler, AcpNotificationEvent, AcpSessionState};
pub use protocol::{AcpError, AcpNotification, AcpRequest, AcpRequestId, AcpResponse};
pub use server::AcpServer;
pub use types::*;
