//! Command safety analysis.
//!
//! Determines whether commands are safe to auto-execute.

mod analyzer;
mod patterns;

pub use analyzer::{is_safe_command, analyze_command, CommandAnalysis, RiskLevel};
