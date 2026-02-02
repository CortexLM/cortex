#![allow(unused_imports, clippy::manual_strip, dead_code)]
//! Common utilities shared across Cortex CLI crates.

pub mod ansi;
pub mod approval_presets;
pub mod config_substitution;
pub mod cwd_guard;
pub mod dirs;
pub mod duration_utils;
pub mod file_locking;
pub mod file_permissions;
pub mod fuzzy_match;
pub mod http_client;
pub mod model_presets;
pub mod path_consistency;
pub mod path_utils;
pub mod signal_safety;
pub mod subprocess_env;
pub mod subprocess_output;
pub mod text_sanitize;
pub mod truncate;

#[cfg(feature = "cli")]
pub mod config_override;

pub use ansi::{
    colors as ansi_colors, maybe_color, maybe_color_stderr, should_colorize,
    should_colorize_stderr, strip_ansi_codes,
};
pub use approval_presets::*;
pub use config_substitution::{
    ConfigSubstitution, SubstitutionError, SubstitutionParseError, substitute_toml_string,
    substitute_toml_value,
};
pub use cwd_guard::{CwdGuard, in_directory, in_directory_result};
pub use dirs::{AppDirs, get_app_dirs, get_cortex_home};
pub use duration_utils::{
    MonotonicTimer, format_duration, format_rate, safe_duration_since, safe_rate,
};
pub use file_locking::{
    FileLockError, FileLockGuard, FileLockManager, FileLockResult, LockConfig, LockMode,
    acquire_lock, atomic_write, global_lock_manager, locked_read_modify_write, try_acquire_lock,
};
pub use file_permissions::{
    apply_umask, create_dir_all_with_umask, create_dir_with_umask, create_file_with_mode,
    create_file_with_umask, get_umask, open_for_append_with_umask,
};
pub use http_client::{
    DEFAULT_TIMEOUT, HEALTH_CHECK_TIMEOUT, STREAMING_TIMEOUT, USER_AGENT, create_blocking_client,
    create_blocking_client_with_timeout, create_client_builder, create_client_with_timeout,
    create_default_client, create_health_check_client, create_streaming_client,
};
pub use model_presets::*;
pub use path_consistency::{
    PathNormalizationOptions, SymlinkStrategy, find_file_case_aware, is_case_sensitive_fs,
    normalize_path_consistent, normalize_path_lexically, paths_match, resolve_config_path,
};
pub use path_utils::{
    PathError, PathResult, ensure_parent_dir, expand_home_path, normalize_path, validate_path_safe,
};
pub use signal_safety::{
    SignalCleanupGuard, get_signal_count, increment_signal_count, is_signal_handling,
    release_signal_lock, reset_signal_count, safe_signal_handler, should_force_exit,
    try_acquire_signal_lock,
};
pub use subprocess_env::{
    EnvSanitizer, get_sanitized_env, get_sanitized_env_os, is_unsafe_env_var, sanitize_env_var,
};
pub use subprocess_output::{
    OutputConfig, SeparatedOutput, StreamingProcess, is_valid_json, run_with_separated_output,
    spawn_with_streaming, try_clean_json_output,
};
pub use text_sanitize::{
    has_control_chars, normalize_code_fences, sanitize_control_chars, sanitize_for_terminal,
};
pub use truncate::{
    truncate_command, truncate_first_line, truncate_for_display, truncate_id, truncate_id_default,
    truncate_model_name, truncate_with_ellipsis, truncate_with_unicode_ellipsis,
};

#[cfg(feature = "cli")]
pub use config_override::{CliConfigOverrides, ConfigOverride};

#[cfg(test)]
mod tests;
