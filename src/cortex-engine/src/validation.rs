//! Input validation utilities.
//!
//! Provides comprehensive validation for various input types
//! including paths, commands, URLs, and configuration values.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CortexError, Result};

/// Validation result.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Is valid.
    pub valid: bool,
    /// Errors.
    pub errors: Vec<ValidationError>,
    /// Warnings.
    pub warnings: Vec<String>,
}

impl ValidationResult {
    /// Create a valid result.
    pub fn valid() -> Self {
        Self {
            valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Create an invalid result.
    pub fn invalid(errors: Vec<ValidationError>) -> Self {
        Self {
            valid: false,
            errors,
            warnings: Vec::new(),
        }
    }

    /// Add an error.
    pub fn add_error(&mut self, error: ValidationError) {
        self.valid = false;
        self.errors.push(error);
    }

    /// Add a warning.
    pub fn add_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    /// Merge with another result.
    pub fn merge(&mut self, other: ValidationResult) {
        if !other.valid {
            self.valid = false;
        }
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
    }

    /// Convert to Result.
    pub fn to_result<T>(self, value: T) -> Result<T> {
        if self.valid {
            Ok(value)
        } else {
            Err(CortexError::InvalidInput(
                self.errors
                    .iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
            ))
        }
    }
}

impl Default for ValidationResult {
    fn default() -> Self {
        Self::valid()
    }
}

/// Validation error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationError {
    /// Field name.
    pub field: String,
    /// Error message.
    pub message: String,
    /// Error code.
    pub code: String,
}

impl ValidationError {
    /// Create a new error.
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
            code: "validation_error".to_string(),
        }
    }

    /// Create with code.
    pub fn with_code(
        field: impl Into<String>,
        message: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
            code: code.into(),
        }
    }
}

/// Path validator.
pub struct PathValidator {
    /// Allow relative paths.
    pub allow_relative: bool,
    /// Require existence.
    pub require_exists: bool,
    /// Require directory.
    pub require_directory: bool,
    /// Require file.
    pub require_file: bool,
    /// Allowed extensions.
    pub allowed_extensions: Option<Vec<String>>,
    /// Blocked paths.
    pub blocked_paths: Vec<PathBuf>,
    /// Max path length.
    pub max_length: usize,
}

impl PathValidator {
    /// Create a new validator.
    pub fn new() -> Self {
        Self {
            allow_relative: true,
            require_exists: false,
            require_directory: false,
            require_file: false,
            allowed_extensions: None,
            blocked_paths: vec![
                PathBuf::from("/"),
                PathBuf::from("/etc"),
                PathBuf::from("/root"),
                PathBuf::from("/home"),
            ],
            max_length: 4096,
        }
    }

    /// Require absolute paths.
    pub fn absolute_only(mut self) -> Self {
        self.allow_relative = false;
        self
    }

    /// Require path to exist.
    pub fn must_exist(mut self) -> Self {
        self.require_exists = true;
        self
    }

    /// Require directory.
    pub fn must_be_directory(mut self) -> Self {
        self.require_directory = true;
        self
    }

    /// Require file.
    pub fn must_be_file(mut self) -> Self {
        self.require_file = true;
        self
    }

    /// Set allowed extensions.
    pub fn allow_extensions(mut self, exts: Vec<&str>) -> Self {
        self.allowed_extensions = Some(
            exts.into_iter()
                .map(std::string::ToString::to_string)
                .collect(),
        );
        self
    }

    /// Add blocked path.
    pub fn block_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.blocked_paths.push(path.into());
        self
    }

    /// Validate a path.
    pub fn validate(&self, path: &Path) -> ValidationResult {
        let mut result = ValidationResult::valid();

        // Check length
        if path.to_string_lossy().len() > self.max_length {
            result.add_error(ValidationError::new("path", "Path exceeds maximum length"));
        }

        // Check relative
        if !self.allow_relative && path.is_relative() {
            result.add_error(ValidationError::new("path", "Relative paths not allowed"));
        }

        // Check existence
        if self.require_exists && !path.exists() {
            result.add_error(ValidationError::new("path", "Path does not exist"));
        }

        // Check type
        if self.require_directory && path.exists() && !path.is_dir() {
            result.add_error(ValidationError::new("path", "Path is not a directory"));
        }

        if self.require_file && path.exists() && !path.is_file() {
            result.add_error(ValidationError::new("path", "Path is not a file"));
        }

        // Check extension
        if let Some(ref allowed) = self.allowed_extensions {
            if let Some(ext) = path.extension() {
                let ext_str = ext.to_string_lossy().to_lowercase();
                if !allowed.iter().any(|a| a.to_lowercase() == ext_str) {
                    result.add_error(ValidationError::new(
                        "path",
                        format!("Extension .{ext_str} not allowed"),
                    ));
                }
            } else {
                result.add_error(ValidationError::new("path", "File must have an extension"));
            }
        }

        // Check blocked paths
        for blocked in &self.blocked_paths {
            if path.starts_with(blocked) || path == blocked {
                result.add_error(ValidationError::new("path", "Path is in blocked location"));
                break;
            }
        }

        result
    }
}

impl Default for PathValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Command validator.
pub struct CommandValidator {
    /// Allowed commands.
    pub allowed: Option<HashSet<String>>,
    /// Blocked commands.
    pub blocked: HashSet<String>,
    /// Blocked patterns.
    pub blocked_patterns: Vec<String>,
    /// Max command length.
    pub max_length: usize,
    /// Allow shell operators.
    pub allow_shell_operators: bool,
}

impl CommandValidator {
    /// Create a new validator.
    pub fn new() -> Self {
        let mut blocked = HashSet::new();
        blocked.insert("rm -rf /".to_string());
        blocked.insert("rm -rf ~".to_string());
        blocked.insert("sudo rm -rf".to_string());
        blocked.insert("mkfs".to_string());
        blocked.insert("dd if=/dev".to_string());

        Self {
            allowed: None,
            blocked,
            blocked_patterns: vec![
                ":(){ :|:& };:".to_string(), // Fork bomb
                "> /dev/sda".to_string(),
            ],
            max_length: 10000,
            allow_shell_operators: true,
        }
    }

    /// Set allowed commands.
    pub fn allow_only(mut self, commands: Vec<&str>) -> Self {
        self.allowed = Some(
            commands
                .into_iter()
                .map(std::string::ToString::to_string)
                .collect(),
        );
        self
    }

    /// Block a command.
    pub fn block(mut self, command: &str) -> Self {
        self.blocked.insert(command.to_string());
        self
    }

    /// Block a pattern.
    pub fn block_pattern(mut self, pattern: &str) -> Self {
        self.blocked_patterns.push(pattern.to_string());
        self
    }

    /// Disallow shell operators.
    pub fn no_shell_operators(mut self) -> Self {
        self.allow_shell_operators = false;
        self
    }

    /// Validate a command.
    pub fn validate(&self, command: &str) -> ValidationResult {
        let mut result = ValidationResult::valid();

        // Check length
        if command.len() > self.max_length {
            result.add_error(ValidationError::new(
                "command",
                "Command exceeds maximum length",
            ));
        }

        // Check allowed list
        if let Some(ref allowed) = self.allowed {
            let cmd = command.split_whitespace().next().unwrap_or("");
            if !allowed.contains(cmd) {
                result.add_error(ValidationError::new(
                    "command",
                    format!("Command '{cmd}' not allowed"),
                ));
            }
        }

        // Check blocked commands
        for blocked in &self.blocked {
            if command.contains(blocked) {
                result.add_error(ValidationError::new(
                    "command",
                    "Command contains blocked pattern",
                ));
                break;
            }
        }

        // Check blocked patterns
        for pattern in &self.blocked_patterns {
            if command.contains(pattern) {
                result.add_error(ValidationError::new(
                    "command",
                    "Command contains dangerous pattern",
                ));
                break;
            }
        }

        // Check shell operators
        if !self.allow_shell_operators {
            let operators = ["&&", "||", ";", "|", ">", "<", "$", "`"];
            for op in &operators {
                if command.contains(op) {
                    result.add_error(ValidationError::new(
                        "command",
                        format!("Shell operator '{op}' not allowed"),
                    ));
                }
            }
        }

        result
    }
}

impl Default for CommandValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// URL validator.
pub struct UrlValidator {
    /// Allowed schemes.
    pub allowed_schemes: Vec<String>,
    /// Allowed hosts.
    pub allowed_hosts: Option<Vec<String>>,
    /// Blocked hosts.
    pub blocked_hosts: Vec<String>,
    /// Require HTTPS.
    pub require_https: bool,
}

impl UrlValidator {
    /// Create a new validator.
    pub fn new() -> Self {
        Self {
            allowed_schemes: vec!["http".to_string(), "https".to_string()],
            allowed_hosts: None,
            blocked_hosts: vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "0.0.0.0".to_string(),
            ],
            require_https: false,
        }
    }

    /// Require HTTPS.
    pub fn https_only(mut self) -> Self {
        self.require_https = true;
        self.allowed_schemes = vec!["https".to_string()];
        self
    }

    /// Allow only specific hosts.
    pub fn allow_hosts(mut self, hosts: Vec<&str>) -> Self {
        self.allowed_hosts = Some(
            hosts
                .into_iter()
                .map(std::string::ToString::to_string)
                .collect(),
        );
        self
    }

    /// Block a host.
    pub fn block_host(mut self, host: &str) -> Self {
        self.blocked_hosts.push(host.to_string());
        self
    }

    /// Validate a URL.
    pub fn validate(&self, url: &str) -> ValidationResult {
        let mut result = ValidationResult::valid();

        // Parse URL
        let parsed = match url::Url::parse(url) {
            Ok(u) => u,
            Err(e) => {
                result.add_error(ValidationError::new("url", format!("Invalid URL: {e}")));
                return result;
            }
        };

        // Check scheme
        let scheme = parsed.scheme();
        if !self.allowed_schemes.contains(&scheme.to_string()) {
            result.add_error(ValidationError::new(
                "url",
                format!("Scheme '{scheme}' not allowed"),
            ));
        }

        // Check HTTPS
        if self.require_https && scheme != "https" {
            result.add_error(ValidationError::new("url", "HTTPS required"));
        }

        // Check host
        if let Some(host) = parsed.host_str() {
            // Check allowed hosts
            if let Some(ref allowed) = self.allowed_hosts
                && !allowed.iter().any(|h| h == host)
            {
                result.add_error(ValidationError::new(
                    "url",
                    format!("Host '{host}' not allowed"),
                ));
            }

            // Check blocked hosts
            for blocked in &self.blocked_hosts {
                if host == blocked {
                    result.add_error(ValidationError::new(
                        "url",
                        format!("Host '{host}' is blocked"),
                    ));
                    break;
                }
            }
        }

        result
    }
}

impl Default for UrlValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// String validator.
pub struct StringValidator {
    /// Minimum length.
    pub min_length: Option<usize>,
    /// Maximum length.
    pub max_length: Option<usize>,
    /// Pattern (regex).
    pub pattern: Option<String>,
    /// Allowed characters.
    pub allowed_chars: Option<String>,
    /// Trim whitespace.
    pub trim: bool,
}

impl StringValidator {
    /// Create a new validator.
    pub fn new() -> Self {
        Self {
            min_length: None,
            max_length: None,
            pattern: None,
            allowed_chars: None,
            trim: true,
        }
    }

    /// Set minimum length.
    pub fn min(mut self, len: usize) -> Self {
        self.min_length = Some(len);
        self
    }

    /// Set maximum length.
    pub fn max(mut self, len: usize) -> Self {
        self.max_length = Some(len);
        self
    }

    /// Set pattern.
    pub fn pattern(mut self, pattern: &str) -> Self {
        self.pattern = Some(pattern.to_string());
        self
    }

    /// Set allowed characters.
    pub fn allowed_chars(mut self, chars: &str) -> Self {
        self.allowed_chars = Some(chars.to_string());
        self
    }

    /// Validate a string.
    pub fn validate(&self, value: &str) -> ValidationResult {
        let mut result = ValidationResult::valid();
        let value = if self.trim { value.trim() } else { value };

        // Check min length
        if let Some(min) = self.min_length
            && value.len() < min
        {
            result.add_error(ValidationError::new(
                "value",
                format!("Must be at least {min} characters"),
            ));
        }

        // Check max length
        if let Some(max) = self.max_length
            && value.len() > max
        {
            result.add_error(ValidationError::new(
                "value",
                format!("Must be at most {max} characters"),
            ));
        }

        // Check allowed characters
        if let Some(ref allowed) = self.allowed_chars {
            for c in value.chars() {
                if !allowed.contains(c) {
                    result.add_error(ValidationError::new(
                        "value",
                        format!("Character '{c}' not allowed"),
                    ));
                    break;
                }
            }
        }

        result
    }
}

impl Default for StringValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Number validator.
pub struct NumberValidator<T> {
    /// Minimum value.
    pub min: Option<T>,
    /// Maximum value.
    pub max: Option<T>,
    /// Allowed values.
    pub allowed: Option<Vec<T>>,
}

impl<T: PartialOrd + Copy + std::fmt::Display> NumberValidator<T> {
    /// Create a new validator.
    pub fn new() -> Self {
        Self {
            min: None,
            max: None,
            allowed: None,
        }
    }

    /// Set minimum.
    pub fn min(mut self, min: T) -> Self {
        self.min = Some(min);
        self
    }

    /// Set maximum.
    pub fn max(mut self, max: T) -> Self {
        self.max = Some(max);
        self
    }

    /// Set allowed values.
    pub fn allowed(mut self, values: Vec<T>) -> Self {
        self.allowed = Some(values);
        self
    }

    /// Validate a number.
    pub fn validate(&self, value: T) -> ValidationResult {
        let mut result = ValidationResult::valid();

        // Check min
        if let Some(min) = self.min
            && value < min
        {
            result.add_error(ValidationError::new(
                "value",
                format!("Must be at least {min}"),
            ));
        }

        // Check max
        if let Some(max) = self.max
            && value > max
        {
            result.add_error(ValidationError::new(
                "value",
                format!("Must be at most {max}"),
            ));
        }

        // Check allowed
        if let Some(ref allowed) = self.allowed
            && !allowed.contains(&value)
        {
            result.add_error(ValidationError::new("value", "Value not in allowed list"));
        }

        result
    }
}

impl<T: PartialOrd + Copy + std::fmt::Display> Default for NumberValidator<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_validation() {
        let validator = PathValidator::new().must_exist();

        let result = validator.validate(Path::new("/nonexistent/path"));
        assert!(!result.valid);
    }

    #[test]
    fn test_command_validation() {
        let validator = CommandValidator::new();

        let result = validator.validate("rm -rf /");
        assert!(!result.valid);

        let result = validator.validate("ls -la");
        assert!(result.valid);
    }

    #[test]
    fn test_url_validation() {
        let validator = UrlValidator::new();

        let result = validator.validate("https://example.com");
        assert!(result.valid);

        let result = validator.validate("http://localhost:8080");
        assert!(!result.valid);
    }

    #[test]
    fn test_string_validation() {
        let validator = StringValidator::new().min(3).max(10);

        let result = validator.validate("hi");
        assert!(!result.valid);

        let result = validator.validate("hello");
        assert!(result.valid);

        let result = validator.validate("this is way too long");
        assert!(!result.valid);
    }

    #[test]
    fn test_number_validation() {
        let validator = NumberValidator::<i32>::new().min(0).max(100);

        let result = validator.validate(-1);
        assert!(!result.valid);

        let result = validator.validate(50);
        assert!(result.valid);

        let result = validator.validate(101);
        assert!(!result.valid);
    }

    #[test]
    fn test_validation_merge() {
        let mut result1 = ValidationResult::valid();
        let mut result2 = ValidationResult::valid();
        result2.add_error(ValidationError::new("field", "error"));

        result1.merge(result2);
        assert!(!result1.valid);
        assert_eq!(result1.errors.len(), 1);
    }
}
