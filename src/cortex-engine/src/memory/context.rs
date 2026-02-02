//! Context assembly for LLM prompts.
//!
//! Assembles retrieved memories into context for prompts with:
//! - Token budget management
//! - Prioritization by relevance and type
//! - Formatting for different LLM providers

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::retriever::{Retriever, SearchQuery, SearchResult};
use super::store::MemoryType;
use crate::error::Result;

/// Context configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    /// Maximum tokens for retrieved context.
    pub max_tokens: usize,
    /// Number of results to retrieve per query.
    pub results_per_query: usize,
    /// Include code context.
    pub include_code: bool,
    /// Include conversation history.
    pub include_conversation: bool,
    /// Include facts/notes.
    pub include_facts: bool,
    /// Priority weights for different memory types.
    pub type_weights: TypeWeights,
    /// Minimum similarity for inclusion.
    pub min_similarity: f32,
    /// Format style for context.
    pub format_style: ContextFormatStyle,
    /// Separator between context items.
    pub separator: String,
    /// Include source references.
    pub include_sources: bool,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_tokens: 8000,
            results_per_query: 10,
            include_code: true,
            include_conversation: true,
            include_facts: true,
            type_weights: TypeWeights::default(),
            min_similarity: 0.5,
            format_style: ContextFormatStyle::Markdown,
            separator: "\n\n---\n\n".to_string(),
            include_sources: true,
        }
    }
}

/// Priority weights for memory types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeWeights {
    pub code: f32,
    pub file_content: f32,
    pub conversation: f32,
    pub fact: f32,
    pub tool_interaction: f32,
    pub note: f32,
}

impl Default for TypeWeights {
    fn default() -> Self {
        Self {
            code: 1.2,
            file_content: 1.1,
            conversation: 1.0,
            fact: 1.3,
            tool_interaction: 0.8,
            note: 0.9,
        }
    }
}

impl TypeWeights {
    fn get(&self, memory_type: &MemoryType) -> f32 {
        match memory_type {
            MemoryType::Code => self.code,
            MemoryType::FileContent => self.file_content,
            MemoryType::UserMessage | MemoryType::AssistantMessage => self.conversation,
            MemoryType::Fact => self.fact,
            MemoryType::ToolInteraction => self.tool_interaction,
            MemoryType::Note => self.note,
            MemoryType::ProjectContext => self.file_content,
            MemoryType::Preference => self.fact,
            MemoryType::Error => self.tool_interaction,
        }
    }
}

/// Format style for context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextFormatStyle {
    /// Plain text.
    Plain,
    /// Markdown formatted.
    Markdown,
    /// XML tags.
    Xml,
    /// JSON structure.
    Json,
}

/// Retrieved context for a prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievedContext {
    /// Formatted context string.
    pub content: String,
    /// Source memories used.
    pub sources: Vec<ContextSource>,
    /// Total tokens (estimated).
    pub token_count: usize,
    /// Number of memories included.
    pub memory_count: usize,
    /// Query used.
    pub query: String,
}

impl RetrievedContext {
    /// Check if context is empty.
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Get context for system prompt injection.
    pub fn as_system_context(&self) -> String {
        if self.is_empty() {
            return String::new();
        }

        format!(
            "<retrieved_context>\n{}\n</retrieved_context>",
            self.content
        )
    }

    /// Get context formatted for user message injection.
    pub fn as_user_context(&self) -> String {
        if self.is_empty() {
            return String::new();
        }

        format!("Relevant context from memory:\n\n{}", self.content)
    }
}

/// Source reference for retrieved memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSource {
    /// Memory ID.
    pub id: uuid::Uuid,
    /// Memory type.
    pub memory_type: MemoryType,
    /// Similarity score.
    pub score: f32,
    /// File path if applicable.
    pub file_path: Option<std::path::PathBuf>,
    /// Line range if applicable.
    pub line_range: Option<(usize, usize)>,
    /// Content preview.
    pub preview: String,
}

impl ContextSource {
    fn from_result(result: &SearchResult) -> Self {
        Self {
            id: result.id,
            memory_type: result.memory_type,
            score: result.score,
            file_path: result.file_path.clone(),
            line_range: result.line_range,
            preview: if result.content.len() > 100 {
                format!("{}...", &result.content[..100])
            } else {
                result.content.clone()
            },
        }
    }
}

/// Context assembler for building prompt context.
#[derive(Debug)]
pub struct ContextAssembler {
    retriever: Arc<Retriever>,
    config: ContextConfig,
}

impl ContextAssembler {
    /// Create a new context assembler.
    pub fn new(retriever: Arc<Retriever>, config: ContextConfig) -> Self {
        Self { retriever, config }
    }

    /// Assemble context for a query.
    pub async fn assemble(&self, query: &str) -> Result<RetrievedContext> {
        // Build search query based on config
        let mut search_queries = Vec::new();

        // Main query
        search_queries.push(SearchQuery::new(query).limit(self.config.results_per_query));

        // Type-specific queries
        if self.config.include_code {
            search_queries.push(
                SearchQuery::new(query)
                    .limit(self.config.results_per_query / 2)
                    .types(vec![MemoryType::Code, MemoryType::FileContent]),
            );
        }

        if self.config.include_conversation {
            search_queries.push(
                SearchQuery::new(query)
                    .limit(self.config.results_per_query / 2)
                    .types(vec![MemoryType::UserMessage, MemoryType::AssistantMessage]),
            );
        }

        if self.config.include_facts {
            search_queries.push(
                SearchQuery::new(query)
                    .limit(self.config.results_per_query / 2)
                    .types(vec![
                        MemoryType::Fact,
                        MemoryType::Note,
                        MemoryType::Preference,
                    ]),
            );
        }

        // Execute all searches
        let mut all_results = Vec::new();
        for search_query in search_queries {
            let results = self.retriever.search_query(search_query).await?;
            all_results.extend(results);
        }

        // Deduplicate by ID
        let mut seen = std::collections::HashSet::new();
        let mut unique_results: Vec<_> = all_results
            .into_iter()
            .filter(|r| seen.insert(r.id))
            .filter(|r| r.score >= self.config.min_similarity)
            .collect();

        // Apply type weights and re-sort
        unique_results.sort_by(|a, b| {
            let score_a = a.rank_score * self.config.type_weights.get(&a.memory_type);
            let score_b = b.rank_score * self.config.type_weights.get(&b.memory_type);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Build context within token budget
        let (content, sources, token_count) = self.build_context(&unique_results);
        let memory_count = sources.len();

        Ok(RetrievedContext {
            content,
            sources,
            token_count,
            memory_count,
            query: query.to_string(),
        })
    }

    /// Assemble context for multiple queries (multi-hop).
    pub async fn assemble_multi(&self, queries: &[&str]) -> Result<RetrievedContext> {
        let mut all_results = Vec::new();

        for query in queries {
            let results = self
                .retriever
                .search(query, self.config.results_per_query)
                .await?;
            all_results.extend(results);
        }

        // Deduplicate
        let mut seen = std::collections::HashSet::new();
        let mut unique_results: Vec<_> = all_results
            .into_iter()
            .filter(|r| seen.insert(r.id))
            .filter(|r| r.score >= self.config.min_similarity)
            .collect();

        // Sort by combined score
        unique_results.sort_by(|a, b| {
            let score_a = a.rank_score * self.config.type_weights.get(&a.memory_type);
            let score_b = b.rank_score * self.config.type_weights.get(&b.memory_type);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let (content, sources, token_count) = self.build_context(&unique_results);
        let memory_count = sources.len();

        Ok(RetrievedContext {
            content,
            sources,
            token_count,
            memory_count,
            query: queries.join(" | "),
        })
    }

    /// Build formatted context from results.
    fn build_context(&self, results: &[SearchResult]) -> (String, Vec<ContextSource>, usize) {
        let mut content_parts = Vec::new();
        let mut sources = Vec::new();
        let mut total_tokens = 0;

        for result in results {
            // Estimate tokens (rough: 4 chars per token)
            let result_tokens = result.content.len() / 4;

            if total_tokens + result_tokens > self.config.max_tokens {
                break;
            }

            let formatted = self.format_result(result);
            content_parts.push(formatted);
            sources.push(ContextSource::from_result(result));
            total_tokens += result_tokens;
        }

        let content = content_parts.join(&self.config.separator);
        (content, sources, total_tokens)
    }

    /// Format a single result based on style.
    fn format_result(&self, result: &SearchResult) -> String {
        match self.config.format_style {
            ContextFormatStyle::Plain => self.format_plain(result),
            ContextFormatStyle::Markdown => self.format_markdown(result),
            ContextFormatStyle::Xml => self.format_xml(result),
            ContextFormatStyle::Json => self.format_json(result),
        }
    }

    fn format_plain(&self, result: &SearchResult) -> String {
        let mut output = String::new();

        if self.config.include_sources {
            if let Some(path) = &result.file_path {
                output.push_str(&format!("[Source: {}]\n", path.display()));
            }
        }

        output.push_str(&result.content);
        output
    }

    fn format_markdown(&self, result: &SearchResult) -> String {
        let mut output = String::new();

        // Type header
        let type_label = match result.memory_type {
            MemoryType::Code => "Code",
            MemoryType::FileContent => "File",
            MemoryType::UserMessage => "User",
            MemoryType::AssistantMessage => "Assistant",
            MemoryType::ToolInteraction => "Tool",
            MemoryType::Fact => "Fact",
            MemoryType::Note => "Note",
            MemoryType::ProjectContext => "Project",
            MemoryType::Preference => "Preference",
            MemoryType::Error => "Error",
        };

        if self.config.include_sources {
            if let Some(path) = &result.file_path {
                if let Some((start, end)) = result.line_range {
                    output.push_str(&format!(
                        "**{}** `{}:{}:{}`\n",
                        type_label,
                        path.display(),
                        start,
                        end
                    ));
                } else {
                    output.push_str(&format!("**{}** `{}`\n", type_label, path.display()));
                }
            } else {
                output.push_str(&format!("**{}**\n", type_label));
            }
        }

        // Content with code block if applicable
        if matches!(
            result.memory_type,
            MemoryType::Code | MemoryType::FileContent
        ) {
            let lang = result
                .file_path
                .as_ref()
                .and_then(|p| p.extension())
                .and_then(|e| e.to_str())
                .unwrap_or("");
            output.push_str(&format!("```{}\n{}\n```", lang, result.content));
        } else {
            output.push_str(&result.content);
        }

        output
    }

    fn format_xml(&self, result: &SearchResult) -> String {
        let type_str = format!("{:?}", result.memory_type).to_lowercase();
        let mut output = format!("<memory type=\"{}\"", type_str);

        if let Some(path) = &result.file_path {
            output.push_str(&format!(" file=\"{}\"", path.display()));
        }
        if let Some((start, end)) = result.line_range {
            output.push_str(&format!(" lines=\"{}-{}\"", start, end));
        }
        output.push_str(&format!(" score=\"{:.2}\">\n", result.score));

        // Escape XML content
        let escaped = result
            .content
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        output.push_str(&escaped);

        output.push_str("\n</memory>");
        output
    }

    fn format_json(&self, result: &SearchResult) -> String {
        serde_json::json!({
            "type": format!("{:?}", result.memory_type).to_lowercase(),
            "content": result.content,
            "score": result.score,
            "file_path": result.file_path,
            "line_range": result.line_range,
        })
        .to_string()
    }

    /// Get current configuration.
    pub fn config(&self) -> &ContextConfig {
        &self.config
    }

    /// Update configuration.
    pub fn set_config(&mut self, config: ContextConfig) {
        self.config = config;
    }
}

/// Builder for context configuration.
#[derive(Debug, Default)]
pub struct ContextConfigBuilder {
    config: ContextConfig,
}

impl ContextConfigBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn max_tokens(mut self, tokens: usize) -> Self {
        self.config.max_tokens = tokens;
        self
    }

    pub fn results_per_query(mut self, count: usize) -> Self {
        self.config.results_per_query = count;
        self
    }

    pub fn include_code(mut self, include: bool) -> Self {
        self.config.include_code = include;
        self
    }

    pub fn include_conversation(mut self, include: bool) -> Self {
        self.config.include_conversation = include;
        self
    }

    pub fn include_facts(mut self, include: bool) -> Self {
        self.config.include_facts = include;
        self
    }

    pub fn min_similarity(mut self, threshold: f32) -> Self {
        self.config.min_similarity = threshold;
        self
    }

    pub fn format_style(mut self, style: ContextFormatStyle) -> Self {
        self.config.format_style = style;
        self
    }

    pub fn include_sources(mut self, include: bool) -> Self {
        self.config.include_sources = include;
        self
    }

    pub fn build(self) -> ContextConfig {
        self.config
    }
}

/// Conversation context tracker for contextual retrieval.
#[derive(Debug, Default)]
pub struct ConversationTracker {
    /// Recent messages.
    messages: Vec<String>,
    /// Maximum messages to track.
    max_messages: usize,
}

impl ConversationTracker {
    pub fn new(max_messages: usize) -> Self {
        Self {
            messages: Vec::new(),
            max_messages,
        }
    }

    /// Add a message to tracking.
    pub fn add_message(&mut self, message: impl Into<String>) {
        self.messages.push(message.into());
        while self.messages.len() > self.max_messages {
            self.messages.remove(0);
        }
    }

    /// Get recent messages.
    pub fn recent_messages(&self) -> &[String] {
        &self.messages
    }

    /// Get combined context from recent messages.
    pub fn combined_context(&self) -> String {
        self.messages.join(" ")
    }

    /// Clear tracking.
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_config_builder() {
        let config = ContextConfigBuilder::new()
            .max_tokens(4000)
            .results_per_query(5)
            .include_code(true)
            .include_conversation(false)
            .format_style(ContextFormatStyle::Xml)
            .build();

        assert_eq!(config.max_tokens, 4000);
        assert_eq!(config.results_per_query, 5);
        assert!(config.include_code);
        assert!(!config.include_conversation);
        assert_eq!(config.format_style, ContextFormatStyle::Xml);
    }

    #[test]
    fn test_conversation_tracker() {
        let mut tracker = ConversationTracker::new(3);

        tracker.add_message("First");
        tracker.add_message("Second");
        tracker.add_message("Third");
        tracker.add_message("Fourth");

        // Should only keep last 3
        assert_eq!(tracker.messages.len(), 3);
        assert_eq!(tracker.messages[0], "Second");
    }

    #[test]
    fn test_type_weights() {
        let weights = TypeWeights::default();

        assert!(weights.get(&MemoryType::Code) > 1.0);
        assert!(weights.get(&MemoryType::Fact) > 1.0);
        assert!(weights.get(&MemoryType::ToolInteraction) < 1.0);
    }

    #[test]
    fn test_retrieved_context_formatting() {
        let context = RetrievedContext {
            content: "Test content".to_string(),
            sources: vec![],
            token_count: 100,
            memory_count: 1,
            query: "test".to_string(),
        };

        let system = context.as_system_context();
        assert!(system.contains("<retrieved_context>"));
        assert!(system.contains("Test content"));

        let user = context.as_user_context();
        assert!(user.contains("Relevant context"));
    }
}
