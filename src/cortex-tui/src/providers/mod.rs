//! Cortex Provider Management
//!
//! This module manages the connection to the Cortex backend API.
//! All model requests go through Cortex with OAuth authentication.
//!
//! ## Authentication Required
//!
//! Before using the CLI, you must authenticate with `cortex login`.

pub mod config;
pub mod manager;
pub mod models;

pub use config::{CortexConfig, PROVIDERS, ProviderConfig, ProviderInfo};
pub use manager::ProviderManager;
pub use models::{ModelInfo, get_models_for_provider, get_popular_models};
