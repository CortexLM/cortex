#![allow(
    clippy::cast_possible_truncation,
    clippy::missing_errors_doc,
    clippy::uninlined_format_args
)]
//! Cortex File Search - Fuzzy file finder with caching and incremental updates.
//!
//! This crate provides high-performance fuzzy file search functionality using
//! nucleo-matcher for fuzzy matching and the `ignore` crate for respecting
//! .gitignore patterns.
//!
//! # Features
//!
//! - Fuzzy file name and path matching using nucleo-matcher
//! - Glob pattern support
//! - Caching of file listings with incremental updates
//! - Respects .gitignore and custom ignore patterns
//! - Configurable result ranking and scoring
//!
//! # Example
//!
//! ```no_run
//! use cortex_file_search::{FileSearch, SearchConfig, SearchMode};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let mut search = FileSearch::new("/path/to/project");
//!     search.build_index().await?;
//!     
//!     let results = search.search("main.rs", SearchMode::FileName, 10).await?;
//!     for result in results {
//!         println!("{}: {}", result.score, result.path.display());
//!     }
//!     Ok(())
//! }
//! ```

mod cache;
mod config;
mod error;
mod index;
mod matcher;
mod result;
mod search;

pub use cache::FileCache;
pub use config::{SearchConfig, SearchConfigBuilder};
pub use error::{SearchError, SearchResult};
pub use index::FileIndex;
pub use matcher::FuzzyMatcher;
pub use result::{SearchMatch, SearchMode};
pub use search::FileSearch;

/// Re-export anyhow for convenience
pub use anyhow;
