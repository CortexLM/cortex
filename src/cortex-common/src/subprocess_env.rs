//! Subprocess environment sanitization.
//!
//! Provides functions for sanitizing environment variables before spawning
//! subprocesses, removing potentially problematic variables.
//!
//! # Issues Addressed
//! - #2804: Subprocess inherits unsanitized shell environment variables

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;

/// Environment variables that should be sanitized (removed or reset) before
/// spawning subprocesses to prevent unexpected behavior.
const UNSAFE_ENV_VARS: &[&str] = &[
    // Shell variables that affect parsing/behavior
    "IFS",             // Internal Field Separator - affects word splitting
    "POSIXLY_CORRECT", // Changes command behavior to strict POSIX mode
    // Dynamic linker variables - security concern
    "LD_PRELOAD",               // Could load malicious libraries
    "LD_LIBRARY_PATH",          // Could redirect to malicious libraries
    "LD_AUDIT",                 // Could load audit libraries
    "LD_DEBUG",                 // Could leak information
    "LD_DEBUG_OUTPUT",          // Could write to arbitrary files
    "LD_DYNAMIC_WEAK",          // Affects symbol resolution
    "LD_ORIGIN_PATH",           // Affects library search
    "LD_PROFILE",               // Enables profiling
    "LD_PROFILE_OUTPUT",        // Could write to arbitrary files
    "LD_SHOW_AUXV",             // Could leak information
    "LD_USE_LOAD_BIAS",         // Affects memory layout
    "LD_VERBOSE",               // Verbose output
    "LD_WARN",                  // Warning output
    "LD_PREFER_MAP_32BIT_EXEC", // Memory layout
    // macOS equivalents
    "DYLD_INSERT_LIBRARIES", // macOS LD_PRELOAD equivalent
    "DYLD_LIBRARY_PATH",     // macOS library path
    "DYLD_FRAMEWORK_PATH",   // macOS framework path
    "DYLD_FALLBACK_LIBRARY_PATH",
    "DYLD_FALLBACK_FRAMEWORK_PATH",
    "DYLD_IMAGE_SUFFIX",
    "DYLD_PRINT_OPTS",
    "DYLD_PRINT_ENV",
    "DYLD_PRINT_LIBRARIES",
    "DYLD_PRINT_SEGMENTS",
    "DYLD_PRINT_BINDINGS",
    "DYLD_PRINT_INITIALIZERS",
    "DYLD_PRINT_APIS",
    "DYLD_PRINT_STATISTICS",
    "DYLD_PRINT_DOFS",
    "DYLD_PRINT_RPATHS",
    // Python-related
    "PYTHONPATH",    // Could load malicious Python code
    "PYTHONHOME",    // Could redirect Python interpreter
    "PYTHONSTARTUP", // Executes script at startup
    // Node.js related
    "NODE_OPTIONS", // Could inject arbitrary options
    "NODE_PATH",    // Module resolution
    // Ruby related
    "RUBYOPT", // Ruby options
    "RUBYLIB", // Ruby library path
    // Perl related
    "PERL5OPT", // Perl options
    "PERL5LIB", // Perl library path
    "PERLLIB",  // Perl library path
    // Java related
    "JAVA_TOOL_OPTIONS", // JVM options
    "_JAVA_OPTIONS",     // JVM options
    // Bash specific
    "BASH_ENV",       // Executed before script
    "ENV",            // sh/ksh startup file
    "CDPATH",         // Could redirect cd commands
    "GLOBIGNORE",     // Affects glob expansion
    "PROMPT_COMMAND", // Executed before each prompt
    // Locale that might cause issues
    "LOCPATH", // Custom locale path
];

/// Environment variables that should be reset to safe defaults instead of removed.
const ENV_VARS_TO_RESET: &[(&str, &str)] = &[
    ("IFS", " \t\n"), // Reset to standard whitespace
];

/// Get a sanitized copy of the current environment.
///
/// This function returns a HashMap of environment variables with potentially
/// dangerous variables removed or reset to safe defaults.
///
/// # Returns
/// A HashMap containing the sanitized environment variables.
///
/// # Examples
/// ```
/// use cortex_common::subprocess_env::get_sanitized_env;
///
/// let env = get_sanitized_env();
/// assert!(!env.contains_key("LD_PRELOAD"));
/// ```
pub fn get_sanitized_env() -> HashMap<String, String> {
    let unsafe_set: HashSet<&str> = UNSAFE_ENV_VARS.iter().copied().collect();
    let reset_map: HashMap<&str, &str> = ENV_VARS_TO_RESET.iter().copied().collect();

    let mut result = HashMap::new();

    // Copy all safe environment variables
    for (key, value) in std::env::vars() {
        if let Some(reset_value) = reset_map.get(key.as_str()) {
            // Reset to safe default
            result.insert(key, reset_value.to_string());
        } else if !unsafe_set.contains(key.as_str()) {
            // Keep the variable as-is
            result.insert(key, value);
        }
        // Unsafe variables are not copied (effectively removed)
    }

    result
}

/// Get a sanitized copy of the environment as OsString pairs.
///
/// This is useful for passing directly to `Command::envs()`.
///
/// # Returns
/// A Vec of (OsString, OsString) pairs for the sanitized environment.
pub fn get_sanitized_env_os() -> Vec<(OsString, OsString)> {
    get_sanitized_env()
        .into_iter()
        .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        .collect()
}

/// Check if an environment variable is considered unsafe.
///
/// # Arguments
/// * `name` - The name of the environment variable to check
///
/// # Returns
/// `true` if the variable is in the unsafe list.
pub fn is_unsafe_env_var(name: &str) -> bool {
    UNSAFE_ENV_VARS.contains(&name)
}

/// Sanitize a single environment variable value.
///
/// For variables in the reset list, returns the safe default.
/// For variables in the unsafe list, returns None.
/// For other variables, returns the original value.
///
/// # Arguments
/// * `name` - The variable name
/// * `value` - The current value
///
/// # Returns
/// `Some(value)` if the variable should be kept, `None` if it should be removed.
pub fn sanitize_env_var(name: &str, value: &str) -> Option<String> {
    // Check if it should be reset
    for (reset_name, reset_value) in ENV_VARS_TO_RESET {
        if name == *reset_name {
            return Some(reset_value.to_string());
        }
    }

    // Check if it's unsafe
    if is_unsafe_env_var(name) {
        return None;
    }

    // Keep the original value
    Some(value.to_string())
}

/// Builder for configuring subprocess environment sanitization.
#[derive(Debug, Clone)]
pub struct EnvSanitizer {
    /// Additional variables to remove
    remove: HashSet<String>,
    /// Variables to explicitly keep (override unsafe list)
    keep: HashSet<String>,
    /// Variables to set to specific values
    set: HashMap<String, String>,
}

impl EnvSanitizer {
    /// Create a new environment sanitizer.
    pub fn new() -> Self {
        Self {
            remove: HashSet::new(),
            keep: HashSet::new(),
            set: HashMap::new(),
        }
    }

    /// Add a variable to be removed.
    pub fn remove(mut self, name: impl Into<String>) -> Self {
        self.remove.insert(name.into());
        self
    }

    /// Add a variable to explicitly keep (even if in unsafe list).
    pub fn keep(mut self, name: impl Into<String>) -> Self {
        self.keep.insert(name.into());
        self
    }

    /// Set a variable to a specific value.
    pub fn set(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.set.insert(name.into(), value.into());
        self
    }

    /// Build the sanitized environment.
    pub fn build(&self) -> HashMap<String, String> {
        let mut env = get_sanitized_env();

        // Add explicitly kept variables from current env
        for name in &self.keep {
            if let Ok(value) = std::env::var(name) {
                env.insert(name.clone(), value);
            }
        }

        // Remove additional variables
        for name in &self.remove {
            env.remove(name);
        }

        // Set specific values
        for (name, value) in &self.set {
            env.insert(name.clone(), value.clone());
        }

        env
    }
}

impl Default for EnvSanitizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_unsafe_env_var() {
        assert!(is_unsafe_env_var("LD_PRELOAD"));
        assert!(is_unsafe_env_var("IFS"));
        assert!(is_unsafe_env_var("DYLD_INSERT_LIBRARIES"));
        assert!(!is_unsafe_env_var("PATH"));
        assert!(!is_unsafe_env_var("HOME"));
    }

    #[test]
    fn test_sanitize_env_var_removes_unsafe() {
        assert!(sanitize_env_var("LD_PRELOAD", "/some/path").is_none());
    }

    #[test]
    fn test_sanitize_env_var_resets_ifs() {
        let result = sanitize_env_var("IFS", ":");
        assert_eq!(result, Some(" \t\n".to_string()));
    }

    #[test]
    fn test_sanitize_env_var_keeps_safe() {
        let result = sanitize_env_var("PATH", "/usr/bin");
        assert_eq!(result, Some("/usr/bin".to_string()));
    }

    #[test]
    fn test_get_sanitized_env_no_ld_preload() {
        // Set an unsafe var (this may fail if we don't have permission)
        let original = std::env::var("LD_PRELOAD").ok();
        // SAFETY: This is a test that temporarily modifies environment variables
        // to verify the sanitization logic works correctly. The original value
        // is restored at the end of the test.
        unsafe {
            std::env::set_var("LD_PRELOAD", "/tmp/evil.so");
        }

        let env = get_sanitized_env();

        // LD_PRELOAD should not be in sanitized env
        assert!(!env.contains_key("LD_PRELOAD"));

        // Restore original
        // SAFETY: Restoring the original environment variable value
        unsafe {
            if let Some(val) = original {
                std::env::set_var("LD_PRELOAD", val);
            } else {
                std::env::remove_var("LD_PRELOAD");
            }
        }
    }

    #[test]
    fn test_env_sanitizer_builder() {
        let env = EnvSanitizer::new()
            .remove("CUSTOM_VAR")
            .set("MY_VAR", "value")
            .build();

        assert_eq!(env.get("MY_VAR"), Some(&"value".to_string()));
        assert!(!env.contains_key("CUSTOM_VAR"));
    }
}
