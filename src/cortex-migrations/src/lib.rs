//! Model migration warnings and deprecation notices for Cortex CLI.
//!
//! Tracks deprecated models and provides migration guidance.

pub mod deprecations;
pub mod migrations;
pub mod warnings;

pub use deprecations::{DeprecatedModel, DeprecationInfo, DEPRECATED_MODELS};
pub use migrations::{get_migration_path, MigrationPath};
pub use warnings::{check_model_warnings, ModelWarning, WarningLevel};
