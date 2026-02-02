//! TUI Screenshot Generator for comprehensive debugging.
//!
//! This module provides a command-line tool to generate screenshots of ALL
//! possible TUI states with mocked data. This is invaluable for:
//! - Visual regression testing
//! - Debugging UI rendering
//! - Documenting TUI behavior
//! - AI agent visual understanding
//!
//! ## Usage
//!
//! ```bash
//! # Generate all screenshots
//! cargo run --bin generate_tui_screenshots
//!
//! # Or with custom output directory
//! cargo run --bin generate_tui_screenshots -- --output ./my-screenshots
//! ```
//!
//! ## Generated Screenshots
//!
//! The generator creates screenshots for:
//! - All views (Session, Approval, Questions, Settings, Help)
//! - All widget states (autocomplete, modals, dropdowns)
//! - All interaction states (streaming, tool execution, errors)
//! - All permission modes
//! - Edge cases and error conditions

mod generator;
pub mod mocks;
mod scenarios;
mod types;

pub use generator::ScreenshotGenerator;
pub use types::{DEFAULT_OUTPUT_DIR, GeneratorConfig, GeneratorResult, ScreenshotScenario};

use crate::types::CaptureResult;
use std::path::PathBuf;

/// Run the screenshot generator with default settings.
pub async fn generate_all_screenshots() -> CaptureResult<GeneratorResult> {
    let generator = ScreenshotGenerator::new();
    generator.generate_all().await
}

/// Run the screenshot generator with custom output directory.
pub async fn generate_screenshots_to(
    output_dir: impl Into<PathBuf>,
) -> CaptureResult<GeneratorResult> {
    let config = GeneratorConfig::default().with_output_dir(output_dir);
    let generator = ScreenshotGenerator::with_config(config);
    generator.generate_all().await
}
