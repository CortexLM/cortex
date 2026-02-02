//! Memory types and core data structures.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Embedding vector type.
pub type Embedding = Vec<f32>;

/// Memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    /// Unique identifier.
    pub id: Uuid,
    /// Memory content.
    pub content: String,
    /// Embedding vector.
    pub embedding: Embedding,
    /// Type of memory.
    pub memory_type: MemoryType,
    /// Creation timestamp.
    pub timestamp: DateTime<Utc>,
    /// Last accessed timestamp.
    pub last_accessed: DateTime<Utc>,
    /// Relevance score (decays over time).
    pub relevance_score: f32,
    /// Access count.
    pub access_count: u32,
    /// Scope (session or global).
    pub scope: MemoryScope,
    /// Additional metadata.
    pub metadata: MemoryMetadata,
}

impl Memory {
    /// Create a new memory.
    pub fn new(
        content: impl Into<String>,
        embedding: Embedding,
        memory_type: MemoryType,
        metadata: MemoryMetadata,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            content: content.into(),
            embedding,
            memory_type,
            timestamp: now,
            last_accessed: now,
            relevance_score: 1.0,
            access_count: 0,
            scope: MemoryScope::Global,
            metadata,
        }
    }

    /// Create a session-scoped memory.
    pub fn session(
        content: impl Into<String>,
        embedding: Embedding,
        memory_type: MemoryType,
        session_id: impl Into<String>,
        metadata: MemoryMetadata,
    ) -> Self {
        let mut memory = Self::new(content, embedding, memory_type, metadata);
        memory.scope = MemoryScope::Session(session_id.into());
        memory
    }

    /// Mark this memory as accessed.
    pub fn mark_accessed(&mut self) {
        self.last_accessed = Utc::now();
        self.access_count += 1;
    }

    /// Apply decay based on age.
    pub fn apply_decay(&mut self, half_life_hours: f32) {
        let age_hours = (Utc::now() - self.timestamp).num_hours() as f32;
        // Exponential decay: score = initial * 0.5^(age/half_life)
        let decay_factor = 0.5_f32.powf(age_hours / half_life_hours);
        self.relevance_score *= decay_factor;
    }

    /// Get age in hours.
    pub fn age_hours(&self) -> f32 {
        (Utc::now() - self.timestamp).num_hours() as f32
    }

    /// Check if memory is expired (relevance too low).
    pub fn is_expired(&self, threshold: f32) -> bool {
        self.relevance_score < threshold
    }
}

/// Memory type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// User message.
    UserMessage,
    /// Assistant response.
    AssistantMessage,
    /// Tool call and result.
    ToolInteraction,
    /// Code snippet.
    Code,
    /// File content.
    FileContent,
    /// Project context.
    ProjectContext,
    /// Factual information.
    Fact,
    /// User preference.
    Preference,
    /// Error or issue.
    Error,
    /// General note.
    Note,
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserMessage => write!(f, "user_message"),
            Self::AssistantMessage => write!(f, "assistant_message"),
            Self::ToolInteraction => write!(f, "tool_interaction"),
            Self::Code => write!(f, "code"),
            Self::FileContent => write!(f, "file_content"),
            Self::ProjectContext => write!(f, "project_context"),
            Self::Fact => write!(f, "fact"),
            Self::Preference => write!(f, "preference"),
            Self::Error => write!(f, "error"),
            Self::Note => write!(f, "note"),
        }
    }
}

/// Memory scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// Global memory (persists across sessions).
    Global,
    /// Session-specific memory.
    Session(String),
    /// Project-specific memory.
    Project(String),
}

impl Default for MemoryScope {
    fn default() -> Self {
        Self::Global
    }
}

impl std::fmt::Display for MemoryScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Global => write!(f, "global"),
            Self::Session(id) => write!(f, "session:{}", id),
            Self::Project(id) => write!(f, "project:{}", id),
        }
    }
}

/// Memory metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryMetadata {
    /// Source file path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<PathBuf>,
    /// Line range in file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_range: Option<(usize, usize)>,
    /// Programming language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Related entity name (function, class, etc).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_name: Option<String>,
    /// Tags for categorization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Custom key-value pairs.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom: HashMap<String, serde_json::Value>,
}

impl MemoryMetadata {
    /// Create metadata for a file.
    pub fn for_file(path: impl Into<PathBuf>, language: Option<String>) -> Self {
        Self {
            file_path: Some(path.into()),
            language,
            ..Default::default()
        }
    }

    /// Create metadata for a code entity.
    pub fn for_code(
        path: impl Into<PathBuf>,
        entity: impl Into<String>,
        language: impl Into<String>,
        lines: (usize, usize),
    ) -> Self {
        Self {
            file_path: Some(path.into()),
            entity_name: Some(entity.into()),
            language: Some(language.into()),
            line_range: Some(lines),
            ..Default::default()
        }
    }

    /// Add a tag.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Add custom metadata.
    pub fn with_custom(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.custom.insert(key.into(), value);
        self
    }
}
