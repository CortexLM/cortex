//! Background agent execution module.
//!
//! This module provides true parallel execution of agents in the background
//! with asynchronous communication between agents.
//!
//! # Features
//!
//! - **Background Execution**: Agents run as tokio tasks without blocking the UI
//! - **Async Messaging**: Inter-agent communication via channels
//! - **Event Broadcasting**: Subscribe to agent lifecycle events
//! - **Graceful Cancellation**: Cancel running agents with cleanup
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_agents::background::{BackgroundAgentManager, AgentConfig};
//!
//! let mut manager = BackgroundAgentManager::new(5);
//!
//! // Subscribe to events
//! let mut events = manager.subscribe();
//!
//! // Spawn a background agent
//! let agent_id = manager.spawn(AgentConfig::new("Search for patterns")).await?;
//!
//! // Listen for events
//! while let Ok(event) = events.recv().await {
//!     match event {
//!         AgentEvent::Completed { id, result } => {
//!             println!("Agent {} completed: {:?}", id, result);
//!             break;
//!         }
//!         AgentEvent::Progress { id, message } => {
//!             println!("Agent {} progress: {}", id, message);
//!         }
//!         _ => {}
//!     }
//! }
//! ```

pub mod events;
pub mod executor;
pub mod messaging;

pub use events::{AgentEvent, AgentResult, AgentStatus};
pub use executor::{
    AgentConfig, BackgroundAgent, BackgroundAgentManager, BackgroundAgentManagerError,
    RunningAgentInfo,
};
pub use messaging::{AgentMailbox, AgentMessage, AgentMessageBroker, MessageContent};
