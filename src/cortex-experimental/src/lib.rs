//! Experimental features management for Cortex CLI.
//!
//! Provides a menu to enable/disable experimental features.

pub mod config;
pub mod features;
pub mod registry;

pub use config::FeaturesConfig;
pub use features::{Feature, FeatureInfo, FeatureStage};
pub use registry::{FeatureRegistry, BUILTIN_FEATURES};
