//! Session source tracking for agent hierarchies.

use super::ThreadId;
use serde::{Deserialize, Serialize};

/// Source of a session/agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SessionSource {
    /// Direct user interaction.
    #[default]
    User,

    /// CLI invocation.
    Cli,

    /// GUI invocation.
    Gui,

    /// IDE extension.
    Ide(String),

    /// MCP server.
    Mcp,

    /// Sub-agent spawned by another agent.
    SubAgent(SubAgentSource),
}

/// Source information for sub-agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubAgentSource {
    /// Spawned via thread spawn tool.
    ThreadSpawn {
        /// Parent thread ID.
        parent_thread_id: ThreadId,
        /// Depth level (0 = initial, 1 = first child, etc.).
        depth: i32,
    },

    /// Spawned via mention system (@agent).
    Mention {
        /// Parent thread ID.
        parent_thread_id: ThreadId,
        /// Agent name mentioned.
        agent_name: String,
    },

    /// Spawned for parallel task execution.
    Parallel {
        /// Parent thread ID.
        parent_thread_id: ThreadId,
        /// Task index in parallel batch.
        task_index: usize,
    },
}

impl SubAgentSource {
    /// Get the parent thread ID.
    pub fn parent_thread_id(&self) -> ThreadId {
        match self {
            SubAgentSource::ThreadSpawn {
                parent_thread_id, ..
            } => *parent_thread_id,
            SubAgentSource::Mention {
                parent_thread_id, ..
            } => *parent_thread_id,
            SubAgentSource::Parallel {
                parent_thread_id, ..
            } => *parent_thread_id,
        }
    }

    /// Get the depth level for thread spawn sources.
    pub fn depth(&self) -> i32 {
        match self {
            SubAgentSource::ThreadSpawn { depth, .. } => *depth,
            // Mentions and parallel tasks are at depth 1
            SubAgentSource::Mention { .. } | SubAgentSource::Parallel { .. } => 1,
        }
    }
}

/// Get the next thread spawn depth from a session source.
pub fn next_thread_spawn_depth(session_source: &SessionSource) -> i32 {
    match session_source {
        SessionSource::SubAgent(SubAgentSource::ThreadSpawn { depth, .. }) => depth + 1,
        _ => 1,
    }
}

/// Check if a depth exceeds the limit.
pub fn exceeds_thread_spawn_depth_limit(depth: i32) -> bool {
    depth > super::guards::MAX_THREAD_SPAWN_DEPTH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_next_depth() {
        let user_source = SessionSource::User;
        assert_eq!(next_thread_spawn_depth(&user_source), 1);

        let sub_source = SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: ThreadId::new(),
            depth: 0,
        });
        assert_eq!(next_thread_spawn_depth(&sub_source), 1);

        let deep_source = SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: ThreadId::new(),
            depth: 1,
        });
        assert_eq!(next_thread_spawn_depth(&deep_source), 2);
    }
}
