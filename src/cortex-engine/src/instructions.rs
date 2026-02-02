//! User instructions management.
//!
//! Provides a system for managing custom instructions that modify
//! agent behavior, including project-specific and global instructions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Instruction source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum InstructionSource {
    /// Built-in default.
    #[default]
    Default,
    /// Global user configuration.
    Global,
    /// Project-specific.
    Project,
    /// Session-specific.
    Session,
    /// Command-line override.
    CommandLine,
    /// Runtime override.
    Runtime,
}

impl InstructionSource {
    /// Get priority (higher = more important).
    pub fn priority(&self) -> u8 {
        match self {
            Self::Default => 0,
            Self::Global => 1,
            Self::Project => 2,
            Self::Session => 3,
            Self::CommandLine => 4,
            Self::Runtime => 5,
        }
    }
}

/// Instruction category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum InstructionCategory {
    /// General behavior.
    #[default]
    General,
    /// Code style and formatting.
    CodeStyle,
    /// Testing requirements.
    Testing,
    /// Documentation requirements.
    Documentation,
    /// Security requirements.
    Security,
    /// Performance considerations.
    Performance,
    /// Tool usage.
    Tools,
    /// Communication style.
    Communication,
    /// Error handling.
    ErrorHandling,
    /// Git/version control.
    Git,
}

/// A single instruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instruction {
    /// Instruction ID.
    pub id: String,
    /// The instruction text.
    pub content: String,
    /// Category.
    pub category: InstructionCategory,
    /// Source.
    pub source: InstructionSource,
    /// Is enabled.
    pub enabled: bool,
    /// Priority within category.
    pub priority: u8,
    /// Conditions for when to apply.
    pub conditions: Vec<InstructionCondition>,
    /// Tags.
    pub tags: Vec<String>,
}

impl Instruction {
    /// Create a new instruction.
    pub fn new(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            category: InstructionCategory::General,
            source: InstructionSource::Default,
            enabled: true,
            priority: 50,
            conditions: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// Set category.
    pub fn with_category(mut self, category: InstructionCategory) -> Self {
        self.category = category;
        self
    }

    /// Set source.
    pub fn with_source(mut self, source: InstructionSource) -> Self {
        self.source = source;
        self
    }

    /// Set priority.
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Add condition.
    pub fn when(mut self, condition: InstructionCondition) -> Self {
        self.conditions.push(condition);
        self
    }

    /// Add tag.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Disable instruction.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Check if instruction applies to context.
    pub fn applies_to(&self, context: &InstructionContext) -> bool {
        if !self.enabled {
            return false;
        }

        if self.conditions.is_empty() {
            return true;
        }

        self.conditions.iter().all(|c| c.matches(context))
    }
}

/// Condition for instruction application.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum InstructionCondition {
    /// File extension match.
    FileExtension(String),
    /// File path pattern.
    FilePath(String),
    /// Project type.
    ProjectType(String),
    /// Framework.
    Framework(String),
    /// Has file.
    HasFile(String),
    /// Task type.
    TaskType(String),
    /// Language.
    Language(String),
}

impl InstructionCondition {
    /// Check if condition matches context.
    pub fn matches(&self, context: &InstructionContext) -> bool {
        match self {
            Self::FileExtension(ext) => context.file_extensions.contains(ext),
            Self::FilePath(pattern) => context
                .current_file
                .as_ref()
                .map(|f| f.to_string_lossy().contains(pattern))
                .unwrap_or(false),
            Self::ProjectType(pt) => context.project_type.as_deref() == Some(pt),
            Self::Framework(fw) => context.framework.as_deref() == Some(fw),
            Self::HasFile(file) => context.root.join(file).exists(),
            Self::TaskType(task) => context.task_type.as_deref() == Some(task),
            Self::Language(lang) => context.languages.contains(lang),
        }
    }
}

/// Context for instruction evaluation.
#[derive(Debug, Clone, Default)]
pub struct InstructionContext {
    /// Project root.
    pub root: PathBuf,
    /// Current file being worked on.
    pub current_file: Option<PathBuf>,
    /// File extensions in project.
    pub file_extensions: Vec<String>,
    /// Project type.
    pub project_type: Option<String>,
    /// Framework.
    pub framework: Option<String>,
    /// Languages used.
    pub languages: Vec<String>,
    /// Task type.
    pub task_type: Option<String>,
}

impl InstructionContext {
    /// Create from project path.
    pub fn from_path(path: impl AsRef<Path>) -> Self {
        let root = path.as_ref().to_path_buf();
        let project_type = detect_project_type(&root);

        Self {
            root,
            current_file: None,
            file_extensions: Vec::new(),
            project_type,
            framework: None,
            languages: Vec::new(),
            task_type: None,
        }
    }

    /// Set current file.
    pub fn with_file(mut self, file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        if let Some(ext) = file.extension() {
            self.file_extensions.push(ext.to_string_lossy().to_string());
        }
        self.current_file = Some(file);
        self
    }

    /// Set task type.
    pub fn with_task(mut self, task_type: impl Into<String>) -> Self {
        self.task_type = Some(task_type.into());
        self
    }
}

/// Instruction manager.
pub struct InstructionManager {
    /// Instructions indexed by ID.
    instructions: HashMap<String, Instruction>,
    /// Instructions grouped by category.
    by_category: HashMap<InstructionCategory, Vec<String>>,
    /// Instructions grouped by source.
    by_source: HashMap<InstructionSource, Vec<String>>,
}

impl InstructionManager {
    /// Create a new instruction manager.
    pub fn new() -> Self {
        Self {
            instructions: HashMap::new(),
            by_category: HashMap::new(),
            by_source: HashMap::new(),
        }
    }

    /// Add an instruction.
    pub fn add(&mut self, instruction: Instruction) {
        let id = instruction.id.clone();
        let category = instruction.category;
        let source = instruction.source;

        self.instructions.insert(id.clone(), instruction);

        self.by_category
            .entry(category)
            .or_default()
            .push(id.clone());

        self.by_source.entry(source).or_default().push(id);
    }

    /// Remove an instruction.
    pub fn remove(&mut self, id: &str) -> Option<Instruction> {
        if let Some(instruction) = self.instructions.remove(id) {
            // Clean up indexes
            if let Some(ids) = self.by_category.get_mut(&instruction.category) {
                ids.retain(|i| i != id);
            }
            if let Some(ids) = self.by_source.get_mut(&instruction.source) {
                ids.retain(|i| i != id);
            }
            Some(instruction)
        } else {
            None
        }
    }

    /// Get an instruction.
    pub fn get(&self, id: &str) -> Option<&Instruction> {
        self.instructions.get(id)
    }

    /// Enable an instruction.
    pub fn enable(&mut self, id: &str) -> bool {
        if let Some(inst) = self.instructions.get_mut(id) {
            inst.enabled = true;
            true
        } else {
            false
        }
    }

    /// Disable an instruction.
    pub fn disable(&mut self, id: &str) -> bool {
        if let Some(inst) = self.instructions.get_mut(id) {
            inst.enabled = false;
            true
        } else {
            false
        }
    }

    /// Get all instructions for a context.
    pub fn for_context(&self, context: &InstructionContext) -> Vec<&Instruction> {
        let mut instructions: Vec<_> = self
            .instructions
            .values()
            .filter(|i| i.applies_to(context))
            .collect();

        // Sort by source priority, then instruction priority
        instructions.sort_by(|a, b| {
            let source_cmp = a.source.priority().cmp(&b.source.priority());
            if source_cmp == std::cmp::Ordering::Equal {
                b.priority.cmp(&a.priority)
            } else {
                source_cmp
            }
        });

        instructions
    }

    /// Get instructions by category.
    pub fn by_category(&self, category: InstructionCategory) -> Vec<&Instruction> {
        self.by_category
            .get(&category)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.instructions.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get instructions by source.
    pub fn by_source(&self, source: InstructionSource) -> Vec<&Instruction> {
        self.by_source
            .get(&source)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.instructions.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Generate prompt text from instructions.
    pub fn generate_prompt(&self, context: &InstructionContext) -> String {
        let instructions = self.for_context(context);

        if instructions.is_empty() {
            return String::new();
        }

        let mut prompt = String::from("# Instructions\n\n");

        // Group by category
        let mut by_cat: HashMap<InstructionCategory, Vec<&Instruction>> = HashMap::new();
        for inst in instructions {
            by_cat.entry(inst.category).or_default().push(inst);
        }

        for (category, insts) in by_cat {
            prompt.push_str(&format!("## {category:?}\n\n"));
            for inst in insts {
                prompt.push_str(&format!("- {}\n", inst.content));
            }
            prompt.push('\n');
        }

        prompt
    }

    /// Load from directory.
    pub fn load_from_dir(
        &mut self,
        dir: impl AsRef<Path>,
        source: InstructionSource,
    ) -> Result<usize> {
        let dir = dir.as_ref();
        let mut count = 0;

        // Look for instruction files
        for name in [
            "instructions.md",
            "INSTRUCTIONS.md",
            ".cortex-instructions",
            "cortex.instructions",
        ] {
            let path = dir.join(name);
            if path.exists() {
                let content = std::fs::read_to_string(&path)?;
                let instructions = parse_instructions(&content, source);
                for inst in instructions {
                    self.add(inst);
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Get all instructions.
    pub fn all(&self) -> Vec<&Instruction> {
        self.instructions.values().collect()
    }

    /// Get count.
    pub fn count(&self) -> usize {
        self.instructions.len()
    }

    /// Clear all instructions.
    pub fn clear(&mut self) {
        self.instructions.clear();
        self.by_category.clear();
        self.by_source.clear();
    }
}

impl Default for InstructionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse instructions from markdown content.
fn parse_instructions(content: &str, source: InstructionSource) -> Vec<Instruction> {
    let mut instructions = Vec::new();
    let mut current_category = InstructionCategory::General;
    let mut id_counter = 0;

    for line in content.lines() {
        let line = line.trim();

        // Category headers
        if line.starts_with("## ") || line.starts_with("# ") {
            let header = line.trim_start_matches('#').trim().to_lowercase();
            current_category = match header.as_str() {
                "code style" | "style" => InstructionCategory::CodeStyle,
                "testing" | "tests" => InstructionCategory::Testing,
                "documentation" | "docs" => InstructionCategory::Documentation,
                "security" => InstructionCategory::Security,
                "performance" => InstructionCategory::Performance,
                "tools" => InstructionCategory::Tools,
                "communication" => InstructionCategory::Communication,
                "error handling" | "errors" => InstructionCategory::ErrorHandling,
                "git" | "version control" => InstructionCategory::Git,
                _ => InstructionCategory::General,
            };
        }
        // Instruction items
        else if line.starts_with("- ") || line.starts_with("* ") {
            let content = line.trim_start_matches('-').trim_start_matches('*').trim();
            if !content.is_empty() {
                id_counter += 1;
                let id = format!("inst_{id_counter}");
                let instruction = Instruction::new(id, content)
                    .with_category(current_category)
                    .with_source(source);
                instructions.push(instruction);
            }
        }
    }

    instructions
}

/// Detect project type from path.
fn detect_project_type(path: &Path) -> Option<String> {
    if path.join("Cargo.toml").exists() {
        Some("rust".to_string())
    } else if path.join("package.json").exists() {
        Some("node".to_string())
    } else if path.join("pyproject.toml").exists() || path.join("setup.py").exists() {
        Some("python".to_string())
    } else if path.join("go.mod").exists() {
        Some("go".to_string())
    } else {
        None
    }
}

/// Default instructions.
pub mod defaults {
    use super::*;

    /// Code style instructions.
    pub fn code_style() -> Vec<Instruction> {
        vec![
            Instruction::new("style_1", "Follow existing code style and conventions")
                .with_category(InstructionCategory::CodeStyle),
            Instruction::new("style_2", "Use meaningful variable and function names")
                .with_category(InstructionCategory::CodeStyle),
            Instruction::new("style_3", "Keep functions small and focused")
                .with_category(InstructionCategory::CodeStyle),
        ]
    }

    /// Testing instructions.
    pub fn testing() -> Vec<Instruction> {
        vec![
            Instruction::new("test_1", "Write tests for new functionality")
                .with_category(InstructionCategory::Testing),
            Instruction::new("test_2", "Ensure existing tests pass")
                .with_category(InstructionCategory::Testing),
        ]
    }

    /// Security instructions.
    pub fn security() -> Vec<Instruction> {
        vec![
            Instruction::new("sec_1", "Never commit secrets or credentials")
                .with_category(InstructionCategory::Security),
            Instruction::new("sec_2", "Validate all user input")
                .with_category(InstructionCategory::Security),
            Instruction::new("sec_3", "Use parameterized queries for database operations")
                .with_category(InstructionCategory::Security),
        ]
    }

    /// All default instructions.
    pub fn all() -> Vec<Instruction> {
        let mut instructions = Vec::new();
        instructions.extend(code_style());
        instructions.extend(testing());
        instructions.extend(security());
        instructions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instruction_creation() {
        let inst = Instruction::new("test", "Test instruction")
            .with_category(InstructionCategory::CodeStyle)
            .with_priority(100);

        assert_eq!(inst.category, InstructionCategory::CodeStyle);
        assert_eq!(inst.priority, 100);
        assert!(inst.enabled);
    }

    #[test]
    fn test_instruction_conditions() {
        let inst = Instruction::new("test", "Test")
            .when(InstructionCondition::FileExtension("rs".to_string()));

        let context = InstructionContext {
            file_extensions: vec!["rs".to_string()],
            ..Default::default()
        };

        assert!(inst.applies_to(&context));

        let other_context = InstructionContext {
            file_extensions: vec!["py".to_string()],
            ..Default::default()
        };

        assert!(!inst.applies_to(&other_context));
    }

    #[test]
    fn test_instruction_manager() {
        let mut manager = InstructionManager::new();

        manager.add(Instruction::new("a", "First"));
        manager.add(Instruction::new("b", "Second").with_category(InstructionCategory::Testing));

        assert_eq!(manager.count(), 2);
        assert_eq!(manager.by_category(InstructionCategory::Testing).len(), 1);
    }

    #[test]
    fn test_parse_instructions() {
        let content = r#"
# Code Style

- Use consistent indentation
- Follow naming conventions

## Testing

- Write unit tests
"#;

        let instructions = parse_instructions(content, InstructionSource::Project);
        assert_eq!(instructions.len(), 3);
    }
}
