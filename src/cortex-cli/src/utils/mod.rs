//! Shared utilities for the Cortex CLI.
//!
//! This module centralizes common functionality used across multiple CLI commands:
//! - Session ID resolution and validation
//! - URL validation and sanitization
//! - File validation and security checks
//! - Clipboard operations
//! - Path validation utilities
//! - Terminal color detection and safe output
//! - Model name validation and resolution
//! - Desktop notification support
//! - System checks (file descriptors, locale)
//! - MIME type detection
//!
//! # Design Principles
//!
//! - **DRY (Don't Repeat Yourself)**: All common functionality is centralized here
//! - **Safety**: Output functions handle broken pipes gracefully
//! - **Consistency**: Validation rules are uniform across all commands
//! - **Modularity**: Each utility module is focused on a single concern

pub mod clipboard;
pub mod file;
pub mod mime;
pub mod model;
pub mod notification;
pub mod paths;
pub mod session;
pub mod system;
pub mod terminal;
pub mod validation;

// Re-export clipboard operations
pub use clipboard::{copy_to_clipboard, read_clipboard};

// Re-export file utilities
pub use file::{
    FileAttachment, MAX_ATTACHMENT_SIZE, MAX_TOTAL_ATTACHMENT_SIZE, process_file_attachments,
    read_file_with_encoding, validate_file_attachment,
};

// Re-export MIME utilities
pub use mime::{
    is_image_mime_type, is_text_mime_type, mime_type_from_extension, mime_type_from_path,
};

// Re-export model utilities
pub use model::{
    KNOWN_PROVIDERS, NON_STREAMING_PATTERNS, ResolvedModel, resolve_model_with_warning,
    supports_streaming, validate_and_resolve_model,
};

// Re-export notification utilities
pub use notification::{NotificationUrgency, send_notification, send_task_notification};

// Re-export path utilities
pub use paths::{
    SENSITIVE_PATHS, expand_tilde, get_cortex_home, is_sensitive_path, safe_join,
    validate_path_safety,
};

// Re-export session utilities
pub use session::{SessionIdError, get_most_recent_session, resolve_session_id};

// Re-export system utilities
pub use system::{
    MIN_RECOMMENDED_FD_LIMIT, check_file_descriptor_limits, is_problematic_locale,
    validate_path_environment, warn_about_locale,
};

// Re-export terminal utilities
pub use terminal::{
    TermColor, ToolDisplay, colors_disabled, format_duration, format_size, get_tool_display,
    is_light_theme, is_terminal_output, restore_terminal, safe_eprint, safe_eprintln, safe_print,
    safe_println, safe_println_empty, should_use_colors, should_use_colors_stderr,
};

// Re-export validation utilities
pub use validation::{
    ALLOWED_URL_SCHEMES, BLOCKED_URL_PATTERNS, MAX_ENV_VAR_NAME_LENGTH, MAX_ENV_VAR_VALUE_LENGTH,
    MAX_SERVER_NAME_LENGTH, MAX_URL_LENGTH, RESERVED_COMMAND_NAMES, is_reserved_command,
    validate_env_var_name, validate_env_var_value, validate_model_name, validate_server_name,
    validate_url, validate_url_allowing_local,
};
