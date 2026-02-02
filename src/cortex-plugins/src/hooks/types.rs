//! Core hook types and enums.

use serde::{Deserialize, Serialize};

/// Hook priority - lower values run first.
///
/// # Security
///
/// Hook priorities control the order of execution, with lower values running first.
/// System-reserved priorities (0-49) should only be used by trusted core plugins,
/// as these hooks can intercept and modify operations before any third-party code runs.
///
/// ## Priority Ranges
///
/// | Range   | Usage                     | Who Can Use            |
/// |---------|---------------------------|------------------------|
/// | 0-9     | Critical system hooks     | Core Cortex only       |
/// | 10-49   | System-level hooks        | Trusted system plugins |
/// | 50-99   | High priority plugins     | Third-party (high)     |
/// | 100-174 | Normal priority plugins   | Third-party (normal)   |
/// | 175-255 | Low priority plugins      | Third-party (low)      |
///
/// Third-party plugins attempting to register hooks with priority < 50 should be
/// rejected to prevent priority hijacking attacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HookPriority(pub i32);

impl Default for HookPriority {
    fn default() -> Self {
        Self::NORMAL
    }
}

impl HookPriority {
    /// Critical system priority (runs first) - **RESERVED FOR CORE CORTEX ONLY**
    ///
    /// # Security
    ///
    /// This priority level is reserved for critical system hooks that must run
    /// before any other code. Third-party plugins must NOT use this priority.
    pub const SYSTEM_CRITICAL: Self = Self(0);

    /// System priority - **RESERVED FOR TRUSTED SYSTEM PLUGINS**
    ///
    /// # Security
    ///
    /// Reserved for system-level plugins that are part of the Cortex distribution.
    /// Third-party plugins must NOT use this priority.
    pub const SYSTEM: Self = Self(10);

    /// High system priority - **RESERVED FOR TRUSTED SYSTEM PLUGINS**
    ///
    /// # Security
    ///
    /// Reserved for system-level plugins. Third-party plugins must NOT use this.
    pub const SYSTEM_HIGH: Self = Self(25);

    /// Minimum priority allowed for third-party plugins.
    ///
    /// Third-party plugins should use priorities >= 50.
    pub const PLUGIN_MIN: Self = Self(50);

    /// High priority for third-party plugins (runs early, but after system hooks).
    pub const PLUGIN_HIGH: Self = Self(75);

    /// Normal priority (default for third-party plugins).
    pub const NORMAL: Self = Self(100);

    /// Low priority (runs later).
    pub const LOW: Self = Self(175);

    /// Lowest priority (runs last).
    pub const LOWEST: Self = Self(255);

    // Legacy aliases for backward compatibility
    /// Alias for SYSTEM_CRITICAL (legacy name).
    #[deprecated(
        since = "0.2.0",
        note = "Use SYSTEM_CRITICAL for system hooks or PLUGIN_HIGH for plugins"
    )]
    pub const HIGHEST: Self = Self::SYSTEM_CRITICAL;

    /// Alias for SYSTEM_HIGH (legacy name).
    #[deprecated(
        since = "0.2.0",
        note = "Use SYSTEM_HIGH for system hooks or PLUGIN_HIGH for plugins"
    )]
    pub const HIGH: Self = Self::SYSTEM_HIGH;

    /// Get the raw priority value.
    pub fn value(&self) -> i32 {
        self.0
    }

    /// Create a custom priority value.
    ///
    /// Note: For third-party plugins, use `new_for_plugin` which enforces
    /// the minimum priority requirement.
    pub fn new(value: i32) -> Self {
        Self(value)
    }

    /// Create a priority value safe for third-party plugins.
    ///
    /// Ensures the priority is >= PLUGIN_MIN (50). If a lower value is provided,
    /// it will be clamped to PLUGIN_MIN.
    ///
    /// # Security
    ///
    /// Use this constructor for third-party plugin priorities to prevent
    /// priority hijacking attacks.
    pub fn new_for_plugin(value: i32) -> Self {
        Self(value.max(Self::PLUGIN_MIN.0))
    }

    /// Validate that this priority is allowed for third-party plugins.
    ///
    /// # Security
    ///
    /// Third-party plugins should not be allowed to register hooks with
    /// system-reserved priorities (< 50). This prevents malicious plugins
    /// from intercepting operations before security checks run.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if the priority is valid for third-party use (>= 50)
    /// - `Err` with explanation if the priority is reserved for system use
    pub fn validate_for_plugin(&self) -> std::result::Result<(), &'static str> {
        if self.0 < Self::PLUGIN_MIN.0 {
            return Err("Priority values below 50 are reserved for system use. \
                        Third-party plugins must use priority >= 50.");
        }
        Ok(())
    }

    /// Check if this priority is in the system-reserved range.
    ///
    /// System-reserved priorities (0-49) should only be used by trusted
    /// core Cortex code and system plugins.
    pub fn is_system_reserved(&self) -> bool {
        self.0 < Self::PLUGIN_MIN.0
    }
}

/// Hook execution result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum HookResult {
    /// Continue with normal execution
    #[default]
    Continue,
    /// Skip further hooks and continue
    Skip,
    /// Abort the operation
    Abort { reason: String },
    /// Replace the operation result
    Replace { result: serde_json::Value },
}
