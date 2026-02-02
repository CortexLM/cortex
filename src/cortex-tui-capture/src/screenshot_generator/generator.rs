//! Core screenshot generator implementation.

use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;

use crate::config::CaptureConfig;
use crate::mock_terminal::MockTerminal;
use crate::types::CaptureResult;

use super::mocks::{InputMocks, StateMocks, ToolMocks, ViewMocks, WidgetMocks};
use super::scenarios::ScenarioRegistry;
use super::types::{GeneratorConfig, GeneratorResult, ScreenshotScenario};

/// The main screenshot generator.
pub struct ScreenshotGenerator {
    config: GeneratorConfig,
    scenarios: Vec<ScreenshotScenario>,
    capture_config: CaptureConfig,
}

impl ScreenshotGenerator {
    /// Create a new generator with default configuration.
    pub fn new() -> Self {
        Self::with_config(GeneratorConfig::default())
    }

    /// Create with specific configuration.
    pub fn with_config(config: GeneratorConfig) -> Self {
        let capture_config = CaptureConfig::new(config.width, config.height)
            .with_title("TUI Screenshots")
            .with_timestamps(true)
            .with_frame_numbers(true);

        let mut generator = Self {
            config,
            scenarios: Vec::new(),
            capture_config,
        };

        // Register all built-in scenarios
        generator.register_all_scenarios();
        generator
    }

    /// Get all registered scenarios.
    pub fn scenarios(&self) -> &[ScreenshotScenario] {
        &self.scenarios
    }

    /// Get scenarios filtered by category.
    pub fn scenarios_by_category(&self, category: &str) -> Vec<&ScreenshotScenario> {
        self.scenarios
            .iter()
            .filter(|s| s.category.eq_ignore_ascii_case(category))
            .collect()
    }

    /// Get all category names.
    pub fn categories(&self) -> Vec<String> {
        let mut cats: Vec<_> = self.scenarios.iter().map(|s| s.category.clone()).collect();
        cats.sort();
        cats.dedup();
        cats
    }

    /// Generate all screenshots.
    pub async fn generate_all(&self) -> CaptureResult<GeneratorResult> {
        let mut result = GeneratorResult {
            total: self.scenarios.len(),
            success: 0,
            failed: Vec::new(),
            output_dir: self.config.output_dir.clone(),
            files: Vec::new(),
        };

        // Create output directory
        fs::create_dir_all(&self.config.output_dir).await?;

        // Create category subdirectories
        for category in self.categories() {
            let cat_dir = self.config.output_dir.join(&category);
            fs::create_dir_all(&cat_dir).await?;
        }

        // Generate each scenario
        for scenario in &self.scenarios {
            // Check filters
            if !self.config.categories.is_empty()
                && !self
                    .config
                    .categories
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case(&scenario.category))
            {
                continue;
            }

            match self.generate_scenario(scenario).await {
                Ok(file_path) => {
                    result.success += 1;
                    result.files.push(file_path);
                }
                Err(e) => {
                    result
                        .failed
                        .push((scenario.id.clone(), format!("{:?}", e)));
                }
            }
        }

        // Generate index if configured
        if self.config.generate_index {
            self.generate_index(&result).await?;
        }

        Ok(result)
    }

    /// Generate a single scenario.
    async fn generate_scenario(&self, scenario: &ScreenshotScenario) -> CaptureResult<PathBuf> {
        let mut terminal = MockTerminal::from_config(self.capture_config.clone())?;

        // Generate the frame content based on scenario
        let content = self.create_scenario_content(scenario);

        // Draw and capture
        terminal.draw(|frame| {
            use ratatui::prelude::*;
            use ratatui::widgets::*;

            // Create the mock UI for this scenario
            let area = frame.area();

            // Draw border with scenario info
            let block = Block::default()
                .title(format!(" {} ", scenario.name))
                .title_style(Style::default().fg(Color::Cyan).bold())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));

            let inner = block.inner(area);
            frame.render_widget(block, area);

            // Render the scenario content
            let paragraph = Paragraph::new(content.clone())
                .wrap(Wrap { trim: false })
                .style(Style::default());

            frame.render_widget(paragraph, inner);
        })?;

        terminal.capture_frame(Some(&scenario.name));

        // Generate markdown content
        let markdown = self.scenario_to_markdown(scenario, &terminal);

        // Write to file
        let file_path = self
            .config
            .output_dir
            .join(&scenario.category)
            .join(scenario.filename());
        fs::write(&file_path, markdown).await?;

        Ok(file_path)
    }

    /// Create mock content for a scenario.
    fn create_scenario_content(&self, scenario: &ScreenshotScenario) -> String {
        // Try each mock provider in order
        if let Some(content) = self.create_view_content(scenario) {
            return content;
        }
        if let Some(content) = self.create_widget_content(scenario) {
            return content;
        }
        if let Some(content) = self.create_tool_content(scenario) {
            return content;
        }
        if let Some(content) = self.create_state_content(scenario) {
            return content;
        }
        if let Some(content) = self.create_input_content(scenario) {
            return content;
        }

        // Default fallback
        format!(
            "Mock content for: {}\n\n{}",
            scenario.name, scenario.description
        )
    }

    /// Convert a scenario to markdown format.
    fn scenario_to_markdown(
        &self,
        scenario: &ScreenshotScenario,
        terminal: &MockTerminal,
    ) -> String {
        let mut md = String::new();

        // Header
        md.push_str(&format!("# {}\n\n", scenario.name));
        md.push_str(&format!("**Category:** {}\n\n", scenario.category));
        md.push_str(&format!("**Description:** {}\n\n", scenario.description));

        if !scenario.tags.is_empty() {
            md.push_str(&format!("**Tags:** {}\n\n", scenario.tags.join(", ")));
        }

        // Terminal size
        let (width, height) = terminal.size();
        md.push_str(&format!("**Terminal Size:** {}x{}\n\n", width, height));

        // ASCII capture
        md.push_str("## Screenshot\n\n");
        md.push_str("```\n");
        md.push_str(&terminal.snapshot().to_ascii(&self.capture_config));
        md.push_str("\n```\n\n");

        // Metadata
        md.push_str("## Scenario Details\n\n");
        md.push_str(&format!("- **ID:** `{}`\n", scenario.id));
        md.push_str(&format!("- **Category:** `{}`\n", scenario.category));
        md.push_str(&format!(
            "- **Generated:** {}\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        ));

        md
    }

    /// Generate the index file.
    async fn generate_index(&self, result: &GeneratorResult) -> CaptureResult<()> {
        let mut index = String::new();

        index.push_str("# TUI Screenshots Index\n\n");
        index.push_str(&format!(
            "Generated: {}\n\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        ));
        index.push_str(&format!("Total Screenshots: {}\n\n", result.success));

        // Group by category
        let mut by_category: HashMap<String, Vec<&ScreenshotScenario>> = HashMap::new();
        for scenario in &self.scenarios {
            by_category
                .entry(scenario.category.clone())
                .or_default()
                .push(scenario);
        }

        // Generate TOC
        index.push_str("## Table of Contents\n\n");
        for category in self.categories() {
            let count = by_category.get(&category).map(|v| v.len()).unwrap_or(0);
            index.push_str(&format!(
                "- [{}](#{}) ({} screenshots)\n",
                category,
                category.to_lowercase().replace(' ', "-"),
                count
            ));
        }
        index.push_str("\n---\n\n");

        // Generate sections
        for category in self.categories() {
            index.push_str(&format!("## {}\n\n", category));

            if let Some(scenarios) = by_category.get(&category) {
                for scenario in scenarios {
                    let filename = scenario.filename();
                    index.push_str(&format!(
                        "- [{}](./{}/{}) - {}\n",
                        scenario.name, category, filename, scenario.description
                    ));
                }
            }
            index.push('\n');
        }

        // Write index
        let index_path = self.config.output_dir.join("README.md");
        fs::write(&index_path, index).await?;

        Ok(())
    }
}

impl Default for ScreenshotGenerator {
    fn default() -> Self {
        Self::new()
    }
}

// Implement the scenario registry trait
impl ScenarioRegistry for ScreenshotGenerator {
    fn scenarios_mut(&mut self) -> &mut Vec<ScreenshotScenario> {
        &mut self.scenarios
    }
}

// Implement all mock content traits
impl ViewMocks for ScreenshotGenerator {}
impl WidgetMocks for ScreenshotGenerator {}
impl ToolMocks for ScreenshotGenerator {}
impl StateMocks for ScreenshotGenerator {}
impl InputMocks for ScreenshotGenerator {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generator_has_scenarios() {
        let generator = ScreenshotGenerator::new();
        assert!(!generator.scenarios().is_empty());
    }

    #[test]
    fn test_generator_categories() {
        let generator = ScreenshotGenerator::new();
        let categories = generator.categories();
        assert!(categories.contains(&"views".to_string()));
        assert!(categories.contains(&"autocomplete".to_string()));
        assert!(categories.contains(&"modals".to_string()));
    }

    #[test]
    fn test_scenarios_by_category() {
        let generator = ScreenshotGenerator::new();
        let view_scenarios = generator.scenarios_by_category("views");
        assert!(!view_scenarios.is_empty());
        for scenario in view_scenarios {
            assert_eq!(scenario.category, "views");
        }
    }
}
