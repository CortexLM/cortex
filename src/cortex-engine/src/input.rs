//! Input handling utilities.
//!
//! Provides utilities for handling user input including
//! readline, history, and completion.

use std::collections::VecDeque;
use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};

/// Input mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum InputMode {
    /// Normal single-line input.
    #[default]
    Normal,
    /// Multi-line input.
    MultiLine,
    /// Password input (hidden).
    Password,
    /// Confirmation (yes/no).
    Confirm,
    /// Selection from options.
    Select,
}

/// Input configuration.
#[derive(Debug, Clone, Default)]
pub struct InputConfig {
    /// Prompt string.
    pub prompt: String,
    /// Default value.
    pub default: Option<String>,
    /// Input mode.
    pub mode: InputMode,
    /// Validation pattern.
    pub pattern: Option<String>,
    /// Maximum length.
    pub max_length: Option<usize>,
    /// Minimum length.
    pub min_length: Option<usize>,
    /// Required.
    pub required: bool,
    /// Trim whitespace.
    pub trim: bool,
    /// Options for select mode.
    pub options: Vec<String>,
}

impl InputConfig {
    /// Create a new config.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            default: None,
            mode: InputMode::Normal,
            pattern: None,
            max_length: None,
            min_length: None,
            required: true,
            trim: true,
            options: Vec::new(),
        }
    }

    /// Set default value.
    pub fn default_value(mut self, value: impl Into<String>) -> Self {
        self.default = Some(value.into());
        self.required = false;
        self
    }

    /// Set mode.
    pub fn mode(mut self, mode: InputMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set as password.
    pub fn password(mut self) -> Self {
        self.mode = InputMode::Password;
        self
    }

    /// Set as confirmation.
    pub fn confirm(mut self) -> Self {
        self.mode = InputMode::Confirm;
        self
    }

    /// Set as selection.
    pub fn select(mut self, options: Vec<impl Into<String>>) -> Self {
        self.mode = InputMode::Select;
        self.options = options.into_iter().map(std::convert::Into::into).collect();
        self
    }

    /// Set as multi-line.
    pub fn multi_line(mut self) -> Self {
        self.mode = InputMode::MultiLine;
        self
    }

    /// Set max length.
    pub fn max(mut self, len: usize) -> Self {
        self.max_length = Some(len);
        self
    }

    /// Set min length.
    pub fn min(mut self, len: usize) -> Self {
        self.min_length = Some(len);
        self
    }

    /// Mark as optional.
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Get prompt with default.
    pub fn formatted_prompt(&self) -> String {
        if let Some(ref default) = self.default {
            format!("{} [{}]: ", self.prompt, default)
        } else {
            format!("{}: ", self.prompt)
        }
    }
}

/// Input result.
#[derive(Debug, Clone)]
pub struct InputResult {
    /// Raw value.
    pub raw: String,
    /// Processed value.
    pub value: String,
    /// Is empty.
    pub is_empty: bool,
    /// Used default.
    pub used_default: bool,
    /// Cancelled.
    pub cancelled: bool,
}

impl InputResult {
    /// Create a new result.
    pub fn new(raw: impl Into<String>, value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            raw: raw.into(),
            is_empty: value.is_empty(),
            value,
            used_default: false,
            cancelled: false,
        }
    }

    /// Create a cancelled result.
    pub fn cancelled() -> Self {
        Self {
            raw: String::new(),
            value: String::new(),
            is_empty: true,
            used_default: false,
            cancelled: true,
        }
    }

    /// Create a default result.
    pub fn from_default(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            raw: String::new(),
            is_empty: value.is_empty(),
            value,
            used_default: true,
            cancelled: false,
        }
    }

    /// Get value or default.
    pub fn value_or<'a>(&'a self, default: &'a str) -> &'a str {
        if self.is_empty { default } else { &self.value }
    }
}

/// Input reader.
pub struct InputReader<R: BufRead, W: Write> {
    /// Reader.
    reader: R,
    /// Writer.
    writer: W,
    /// History.
    history: VecDeque<String>,
    /// History limit.
    history_limit: usize,
}

impl<R: BufRead, W: Write> InputReader<R, W> {
    /// Create a new reader.
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            history: VecDeque::new(),
            history_limit: 100,
        }
    }

    /// Read input with config.
    pub fn read(&mut self, config: &InputConfig) -> io::Result<InputResult> {
        match config.mode {
            InputMode::Normal => self.read_normal(config),
            InputMode::MultiLine => self.read_multiline(config),
            InputMode::Password => self.read_password(config),
            InputMode::Confirm => self.read_confirm(config),
            InputMode::Select => self.read_select(config),
        }
    }

    /// Read normal input.
    fn read_normal(&mut self, config: &InputConfig) -> io::Result<InputResult> {
        write!(self.writer, "{}", config.formatted_prompt())?;
        self.writer.flush()?;

        let mut input = String::new();
        self.reader.read_line(&mut input)?;

        let value = if config.trim {
            input.trim().to_string()
        } else {
            input.trim_end_matches('\n').to_string()
        };

        if value.is_empty()
            && let Some(ref default) = config.default
        {
            return Ok(InputResult::from_default(default));
        }

        // Add to history
        if !value.is_empty() {
            self.add_to_history(&value);
        }

        Ok(InputResult::new(&input, &value))
    }

    /// Read multi-line input.
    fn read_multiline(&mut self, config: &InputConfig) -> io::Result<InputResult> {
        writeln!(
            self.writer,
            "{} (enter empty line to finish):",
            config.prompt
        )?;
        self.writer.flush()?;

        let mut lines = Vec::new();

        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line)?;

            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }

            lines.push(line.trim_end_matches('\n').to_string());
        }

        let value = lines.join("\n");
        Ok(InputResult::new(&value, &value))
    }

    /// Read password input.
    fn read_password(&mut self, config: &InputConfig) -> io::Result<InputResult> {
        write!(self.writer, "{}: ", config.prompt)?;
        self.writer.flush()?;

        // In a real implementation, we'd use termios to hide input
        let mut input = String::new();
        self.reader.read_line(&mut input)?;

        let value = input.trim().to_string();
        writeln!(self.writer)?;

        Ok(InputResult::new("***", &value))
    }

    /// Read confirmation.
    fn read_confirm(&mut self, config: &InputConfig) -> io::Result<InputResult> {
        write!(self.writer, "{} [y/N]: ", config.prompt)?;
        self.writer.flush()?;

        let mut input = String::new();
        self.reader.read_line(&mut input)?;

        let trimmed = input.trim().to_lowercase();
        let confirmed = matches!(trimmed.as_str(), "y" | "yes" | "true" | "1");

        Ok(InputResult::new(
            &input,
            if confirmed { "yes" } else { "no" },
        ))
    }

    /// Read selection.
    fn read_select(&mut self, config: &InputConfig) -> io::Result<InputResult> {
        writeln!(self.writer, "{}", config.prompt)?;

        for (i, option) in config.options.iter().enumerate() {
            writeln!(self.writer, "  {}. {}", i + 1, option)?;
        }

        write!(self.writer, "Select [1-{}]: ", config.options.len())?;
        self.writer.flush()?;

        let mut input = String::new();
        self.reader.read_line(&mut input)?;

        let trimmed = input.trim();

        // Try parsing as number
        if let Ok(idx) = trimmed.parse::<usize>()
            && idx >= 1
            && idx <= config.options.len()
        {
            let value = config.options[idx - 1].clone();
            return Ok(InputResult::new(&input, &value));
        }

        // Try matching option text
        for option in &config.options {
            if option.to_lowercase() == trimmed.to_lowercase() {
                return Ok(InputResult::new(&input, option));
            }
        }

        Ok(InputResult::new(&input, trimmed))
    }

    /// Add to history.
    fn add_to_history(&mut self, value: &str) {
        // Avoid duplicates
        if self.history.front().map(std::string::String::as_str) == Some(value) {
            return;
        }

        self.history.push_front(value.to_string());

        while self.history.len() > self.history_limit {
            self.history.pop_back();
        }
    }

    /// Get history.
    pub fn history(&self) -> &VecDeque<String> {
        &self.history
    }
}

/// Prompt builder.
pub struct PromptBuilder {
    /// Lines.
    lines: Vec<PromptLine>,
}

impl PromptBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    /// Add a text prompt.
    pub fn text(mut self, prompt: impl Into<String>) -> Self {
        self.lines.push(PromptLine::Text(InputConfig::new(prompt)));
        self
    }

    /// Add a password prompt.
    pub fn password(mut self, prompt: impl Into<String>) -> Self {
        self.lines
            .push(PromptLine::Text(InputConfig::new(prompt).password()));
        self
    }

    /// Add a confirmation prompt.
    pub fn confirm(mut self, prompt: impl Into<String>) -> Self {
        self.lines
            .push(PromptLine::Text(InputConfig::new(prompt).confirm()));
        self
    }

    /// Add a selection prompt.
    pub fn select(mut self, prompt: impl Into<String>, options: Vec<impl Into<String>>) -> Self {
        self.lines
            .push(PromptLine::Text(InputConfig::new(prompt).select(options)));
        self
    }

    /// Get configs.
    pub fn configs(&self) -> Vec<&InputConfig> {
        self.lines
            .iter()
            .filter_map(|l| match l {
                PromptLine::Text(config) => Some(config),
            })
            .collect()
    }
}

impl Default for PromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Prompt line.
#[derive(Debug, Clone)]
enum PromptLine {
    /// Text input.
    Text(InputConfig),
}

/// Completion source.
pub trait CompletionSource: Send + Sync {
    /// Get completions for prefix.
    fn completions(&self, prefix: &str) -> Vec<String>;
}

/// Static completion source.
pub struct StaticCompletions {
    /// Values.
    values: Vec<String>,
}

impl StaticCompletions {
    /// Create a new source.
    pub fn new(values: Vec<impl Into<String>>) -> Self {
        Self {
            values: values.into_iter().map(std::convert::Into::into).collect(),
        }
    }
}

impl CompletionSource for StaticCompletions {
    fn completions(&self, prefix: &str) -> Vec<String> {
        let prefix_lower = prefix.to_lowercase();
        self.values
            .iter()
            .filter(|v| v.to_lowercase().starts_with(&prefix_lower))
            .cloned()
            .collect()
    }
}

/// Path completion source.
pub struct PathCompletions {
    /// Base directory.
    base_dir: std::path::PathBuf,
}

impl PathCompletions {
    /// Create a new source.
    pub fn new(base_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Create for current directory.
    pub fn current_dir() -> Self {
        Self::new(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
    }
}

impl CompletionSource for PathCompletions {
    fn completions(&self, prefix: &str) -> Vec<String> {
        let path = self.base_dir.join(prefix);
        let dir = if path.is_dir() {
            &path
        } else {
            path.parent().unwrap_or(&self.base_dir)
        };

        let prefix_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if let Ok(entries) = std::fs::read_dir(dir) {
            entries
                .filter_map(std::result::Result::ok)
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with(prefix_name) {
                        let full_path = e.path();
                        Some(full_path.to_string_lossy().to_string())
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        }
    }
}

/// Validation result.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Is valid.
    pub valid: bool,
    /// Error message.
    pub error: Option<String>,
}

impl ValidationResult {
    /// Create valid result.
    pub fn valid() -> Self {
        Self {
            valid: true,
            error: None,
        }
    }

    /// Create invalid result.
    pub fn invalid(error: impl Into<String>) -> Self {
        Self {
            valid: false,
            error: Some(error.into()),
        }
    }
}

/// Input validator.
pub trait InputValidator: Send + Sync {
    /// Validate input.
    fn validate(&self, input: &str) -> ValidationResult;
}

/// Required validator.
pub struct RequiredValidator;

impl InputValidator for RequiredValidator {
    fn validate(&self, input: &str) -> ValidationResult {
        if input.trim().is_empty() {
            ValidationResult::invalid("This field is required")
        } else {
            ValidationResult::valid()
        }
    }
}

/// Length validator.
pub struct LengthValidator {
    /// Minimum.
    min: Option<usize>,
    /// Maximum.
    max: Option<usize>,
}

impl LengthValidator {
    /// Create a new validator.
    pub fn new(min: Option<usize>, max: Option<usize>) -> Self {
        Self { min, max }
    }
}

impl InputValidator for LengthValidator {
    fn validate(&self, input: &str) -> ValidationResult {
        let len = input.len();

        if let Some(min) = self.min
            && len < min
        {
            return ValidationResult::invalid(format!("Must be at least {min} characters"));
        }

        if let Some(max) = self.max
            && len > max
        {
            return ValidationResult::invalid(format!("Must be at most {max} characters"));
        }

        ValidationResult::valid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_input_config() {
        let config = InputConfig::new("Name").default_value("John").max(50);

        assert_eq!(config.prompt, "Name");
        assert_eq!(config.default, Some("John".to_string()));
        assert_eq!(config.max_length, Some(50));
        assert!(!config.required);
    }

    #[test]
    fn test_input_result() {
        let result = InputResult::new("test", "test");
        assert_eq!(result.value, "test");
        assert!(!result.is_empty);
        assert!(!result.used_default);

        let result = InputResult::from_default("default");
        assert!(result.used_default);
    }

    #[test]
    fn test_input_reader() {
        let input = Cursor::new("John\n");
        let output = Vec::new();

        let mut reader = InputReader::new(input, output);
        let config = InputConfig::new("Name");

        let result = reader.read(&config).unwrap();
        assert_eq!(result.value, "John");
    }

    #[test]
    fn test_input_reader_default() {
        let input = Cursor::new("\n");
        let output = Vec::new();

        let mut reader = InputReader::new(input, output);
        let config = InputConfig::new("Name").default_value("DefaultName");

        let result = reader.read(&config).unwrap();
        assert_eq!(result.value, "DefaultName");
        assert!(result.used_default);
    }

    #[test]
    fn test_static_completions() {
        let source = StaticCompletions::new(vec!["apple", "banana", "apricot"]);

        let completions = source.completions("ap");
        assert_eq!(completions.len(), 2);
        assert!(completions.contains(&"apple".to_string()));
        assert!(completions.contains(&"apricot".to_string()));
    }

    #[test]
    fn test_required_validator() {
        let validator = RequiredValidator;

        assert!(validator.validate("hello").valid);
        assert!(!validator.validate("").valid);
        assert!(!validator.validate("  ").valid);
    }

    #[test]
    fn test_length_validator() {
        let validator = LengthValidator::new(Some(3), Some(10));

        assert!(!validator.validate("ab").valid);
        assert!(validator.validate("abc").valid);
        assert!(validator.validate("abcdefghij").valid);
        assert!(!validator.validate("abcdefghijk").valid);
    }

    #[test]
    fn test_prompt_builder() {
        let builder = PromptBuilder::new()
            .text("Name")
            .password("Password")
            .confirm("Agree?");

        assert_eq!(builder.configs().len(), 3);
    }
}
