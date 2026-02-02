//! Types and data structures for screenshot generation.

use std::path::PathBuf;

/// Default output directory for screenshots.
pub const DEFAULT_OUTPUT_DIR: &str = "./tui-screenshots";

/// A single screenshot scenario to generate.
#[derive(Debug, Clone)]
pub struct ScreenshotScenario {
    /// Unique identifier for this scenario.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Category for grouping.
    pub category: String,
    /// Description of what this shows.
    pub description: String,
    /// Tags for filtering.
    pub tags: Vec<String>,
}

impl ScreenshotScenario {
    /// Create a new scenario.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        category: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            category: category.into(),
            description: description.into(),
            tags: Vec::new(),
        }
    }

    /// Add tags to the scenario.
    pub fn with_tags(mut self, tags: Vec<&str>) -> Self {
        self.tags = tags.into_iter().map(String::from).collect();
        self
    }

    /// Get the filename for this scenario.
    pub fn filename(&self) -> String {
        format!(
            "{}_{}.md",
            self.category.replace(' ', "_").to_lowercase(),
            self.id.replace(' ', "_").to_lowercase()
        )
    }
}

/// Configuration for the screenshot generator.
#[derive(Debug, Clone)]
pub struct GeneratorConfig {
    /// Output directory for screenshots.
    pub output_dir: PathBuf,
    /// Terminal width.
    pub width: u16,
    /// Terminal height.
    pub height: u16,
    /// Whether to generate an index file.
    pub generate_index: bool,
    /// Whether to include ASCII art in output.
    pub include_ascii: bool,
    /// Categories to include (empty = all).
    pub categories: Vec<String>,
    /// Tags to filter by (empty = all).
    pub tags: Vec<String>,
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from(DEFAULT_OUTPUT_DIR),
            width: 120,
            height: 40,
            generate_index: true,
            include_ascii: true,
            categories: Vec::new(),
            tags: Vec::new(),
        }
    }
}

impl GeneratorConfig {
    /// Create with a specific output directory.
    pub fn with_output_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.output_dir = dir.into();
        self
    }

    /// Set terminal dimensions.
    pub fn with_size(mut self, width: u16, height: u16) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Filter by categories.
    pub fn with_categories(mut self, categories: Vec<&str>) -> Self {
        self.categories = categories.into_iter().map(String::from).collect();
        self
    }
}

/// Result of generating screenshots.
#[derive(Debug)]
pub struct GeneratorResult {
    /// Total scenarios attempted.
    pub total: usize,
    /// Successfully generated.
    pub success: usize,
    /// Failed scenarios.
    pub failed: Vec<(String, String)>,
    /// Output directory.
    pub output_dir: PathBuf,
    /// Generated files.
    pub files: Vec<PathBuf>,
}

impl GeneratorResult {
    /// Check if all scenarios succeeded.
    pub fn all_success(&self) -> bool {
        self.failed.is_empty()
    }

    /// Get summary string.
    pub fn summary(&self) -> String {
        format!(
            "Generated {} of {} screenshots ({} failed) in {:?}",
            self.success,
            self.total,
            self.failed.len(),
            self.output_dir
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scenario_filename() {
        let scenario =
            ScreenshotScenario::new("test_id", "Test Name", "Test Category", "Test description");
        assert_eq!(scenario.filename(), "test_category_test_id.md");
    }

    #[test]
    fn test_generator_config_defaults() {
        let config = GeneratorConfig::default();
        assert_eq!(config.width, 120);
        assert_eq!(config.height, 40);
        assert!(config.generate_index);
    }

    #[test]
    fn test_generator_result_summary() {
        let result = GeneratorResult {
            total: 10,
            success: 8,
            failed: vec![("id1".to_string(), "error".to_string())],
            output_dir: PathBuf::from("./test"),
            files: Vec::new(),
        };
        assert!(!result.all_success());
        assert!(result.summary().contains("8 of 10"));
    }
}
