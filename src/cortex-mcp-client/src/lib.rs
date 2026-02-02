//! MCP Client implementation for Cortex

pub mod client;
pub mod discovery;
pub mod transport;

pub use client::McpClient;
pub use discovery::ToolDiscovery;
pub use transport::Transport;
