//! Execution context for policy decisions.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Execution context that affects policy decisions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Whether running inside a container
    pub is_container: bool,

    /// Whether running inside a sandbox
    pub is_sandboxed: bool,

    /// Whether running as root/admin
    pub is_root: bool,

    /// Current working directory
    pub cwd: Option<String>,

    /// Allowed network access
    pub network_allowed: bool,

    /// Custom allowed programs (bypass Ask for these)
    pub allowed_programs: HashSet<String>,

    /// Custom denied programs (always Deny)
    pub denied_programs: HashSet<String>,
}

impl ExecutionContext {
    /// Creates a new execution context with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a context for containerized execution (more permissive).
    pub fn container() -> Self {
        Self {
            is_container: true,
            is_sandboxed: true,
            ..Default::default()
        }
    }

    /// Creates a context for sandboxed execution.
    pub fn sandboxed() -> Self {
        Self {
            is_sandboxed: true,
            ..Default::default()
        }
    }

    /// Builder: set container mode.
    #[must_use]
    pub fn with_container(mut self, is_container: bool) -> Self {
        self.is_container = is_container;
        self
    }

    /// Builder: set sandbox mode.
    #[must_use]
    pub fn with_sandboxed(mut self, is_sandboxed: bool) -> Self {
        self.is_sandboxed = is_sandboxed;
        self
    }

    /// Builder: set root/admin status.
    #[must_use]
    pub fn with_root(mut self, is_root: bool) -> Self {
        self.is_root = is_root;
        self
    }

    /// Builder: add allowed program.
    #[must_use]
    pub fn with_allowed_program(mut self, program: impl Into<String>) -> Self {
        self.allowed_programs.insert(program.into());
        self
    }

    /// Builder: add denied program.
    #[must_use]
    pub fn with_denied_program(mut self, program: impl Into<String>) -> Self {
        self.denied_programs.insert(program.into());
        self
    }
}
