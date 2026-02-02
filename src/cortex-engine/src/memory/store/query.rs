//! Memory query and filter types.

use std::path::PathBuf;

use super::types::{MemoryScope, MemoryType};

/// Memory query for filtering.
#[derive(Debug, Clone, Default)]
pub struct MemoryQuery {
    /// Filter by memory types.
    pub types: Option<Vec<MemoryType>>,
    /// Filter by scope.
    pub scope: Option<MemoryScope>,
    /// Minimum relevance score.
    pub min_relevance: Option<f32>,
    /// Maximum age in hours.
    pub max_age_hours: Option<f32>,
    /// Filter by tags.
    pub tags: Option<Vec<String>>,
    /// Filter by file path prefix.
    pub file_path_prefix: Option<PathBuf>,
    /// Limit results.
    pub limit: Option<usize>,
    /// Offset for pagination.
    pub offset: Option<usize>,
}

/// Memory filter for deletion/search.
#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    /// Filter by memory types.
    pub types: Option<Vec<MemoryType>>,
    /// Filter by scope.
    pub scope: Option<MemoryScope>,
    /// Minimum age in hours.
    pub min_age_hours: Option<f32>,
    /// Maximum relevance score.
    pub max_relevance: Option<f32>,
    /// Filter by tags.
    pub tags: Option<Vec<String>>,
}
