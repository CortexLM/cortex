//! Security module - SSRF protection, URL validation, path safety, and runtime protections.
//!
//! Provides comprehensive protection against:
//! - Server-Side Request Forgery (SSRF) attacks via URL validation
//! - Path traversal attacks via path validation utilities
//! - Doom loop detection (infinite tool call patterns)
//! - Session-scoped approval memory
//! - File time tracking for read-before-write protection
//! - Wildcard pattern matching for command permissions
//! - External directory access protection

pub mod approval_memory;
pub mod doom_loop;
pub mod external_directory;
pub mod file_time;
pub mod path_safety;
pub mod ssrf;
pub mod wildcard;

pub use ssrf::{
    SsrfConfig, SsrfError, SsrfProtection, SsrfResult, is_safe_url, validate_url_for_fetch,
};

pub use path_safety::{
    PathValidationError, PathValidationOptions, PathValidationResult, contains_traversal,
    normalize_path, resolve_and_validate_path, sanitize_filename, validate_path_within_any_root,
    validate_path_within_any_root_with_options, validate_path_within_root,
    validate_path_within_root_with_options, validate_zip_entry_path,
};

pub use approval_memory::{ApprovalDecision, ApprovalMemory, ApprovalType, global_memory};

pub use doom_loop::{DoomLoopCheck, DoomLoopConfig, HashDoomLoopDetector};

pub use external_directory::{ExternalDirectoryChecker, common_external_dirs, is_external_path};

pub use file_time::{FileTimeError, FileTimeTracker, global_tracker};

pub use wildcard::{Permission, PermissionPattern, WildcardMatcher, default_bash_patterns};
