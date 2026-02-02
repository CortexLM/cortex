//! Configuration variable substitution for Cortex CLI.
//!
//! Supports substitution of environment variables and file contents in TOML configuration:
//! - `{env:VAR_NAME}` - Substitutes with environment variable value
//! - `{env:VAR_NAME:default}` - Substitutes with environment variable or default if not set
//! - `{file:path}` - Substitutes with file content (first line, trimmed)
//! - `{file:~/.secrets/key}` - Supports ~ expansion for home directory

use regex::Regex;
use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during configuration substitution.
#[derive(Debug, Error)]
pub enum SubstitutionError {
    /// Environment variable not found and no default provided.
    #[error("Environment variable '{0}' not found and no default provided")]
    EnvVarNotFound(String),

    /// Referenced file was not found.
    #[error("File not found: {0}")]
    FileNotFound(PathBuf),

    /// Failed to read the referenced file.
    #[error("Failed to read file {path}: {source}")]
    FileReadError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Invalid substitution syntax encountered.
    #[error("Invalid substitution syntax: {0}")]
    InvalidSyntax(String),

    /// Home directory could not be determined.
    #[error("Could not determine home directory")]
    HomeDirNotFound,
}

/// Configuration substitution engine.
///
/// Handles replacement of `{env:...}` and `{file:...}` placeholders
/// in configuration strings.
pub struct ConfigSubstitution {
    /// Regex for environment variable substitution: {env:VAR} or {env:VAR:default}
    env_regex: Regex,
    /// Regex for file content substitution: {file:path}
    file_regex: Regex,
}

impl Default for ConfigSubstitution {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigSubstitution {
    /// Creates a new `ConfigSubstitution` instance with compiled regex patterns.
    #[must_use]
    pub fn new() -> Self {
        Self {
            // Matches {env:VAR_NAME} or {env:VAR_NAME:default_value}
            // Group 1: variable name
            // Group 2: optional default value (after second colon)
            env_regex: Regex::new(r"\{env:([^:}]+)(?::([^}]*))?\}").unwrap_or_else(|e| {
                panic!("Failed to compile env regex: {e}");
            }),
            // Matches {file:path}
            // Group 1: file path
            file_regex: Regex::new(r"\{file:([^}]+)\}").unwrap_or_else(|e| {
                panic!("Failed to compile file regex: {e}");
            }),
        }
    }

    /// Substitutes all variables in a string.
    ///
    /// Processes environment variables first, then file references.
    ///
    /// # Arguments
    /// * `input` - The string containing placeholders to substitute
    ///
    /// # Returns
    /// The string with all placeholders replaced, or an error if substitution fails.
    ///
    /// # Examples
    /// ```
    /// use cortex_common::ConfigSubstitution;
    ///
    /// // SAFETY: Test-only example, no concurrent access to this env var
    /// unsafe { std::env::set_var("MY_VAR", "hello") };
    /// let sub = ConfigSubstitution::new();
    /// let result = sub.substitute("{env:MY_VAR}").unwrap();
    /// assert_eq!(result, "hello");
    /// // SAFETY: Test cleanup
    /// unsafe { std::env::remove_var("MY_VAR") };
    /// ```
    pub fn substitute(&self, input: &str) -> Result<String, SubstitutionError> {
        let result = self.substitute_env(input)?;
        self.substitute_file(&result)
    }

    /// Substitutes environment variables in the input string.
    ///
    /// Handles both `{env:VAR}` and `{env:VAR:default}` syntax.
    fn substitute_env(&self, input: &str) -> Result<String, SubstitutionError> {
        let mut result = input.to_string();
        let mut error: Option<SubstitutionError> = None;

        // Collect all matches first to avoid borrowing issues
        let matches: Vec<_> = self
            .env_regex
            .captures_iter(input)
            .map(|cap| {
                let full_match = cap.get(0).map(|m| m.as_str().to_string());
                let var_name = cap.get(1).map(|m| m.as_str().to_string());
                let default_value = cap.get(2).map(|m| m.as_str().to_string());
                (full_match, var_name, default_value)
            })
            .collect();

        for (full_match, var_name, default_value) in matches {
            let Some(full) = full_match else { continue };
            let Some(var) = var_name else { continue };

            let replacement = match std::env::var(&var) {
                Ok(value) => value,
                Err(_) => {
                    if let Some(default) = default_value {
                        default
                    } else {
                        error = Some(SubstitutionError::EnvVarNotFound(var));
                        continue;
                    }
                }
            };

            result = result.replace(&full, &replacement);
        }

        if let Some(e) = error {
            return Err(e);
        }

        Ok(result)
    }

    /// Substitutes file references in the input string.
    ///
    /// Handles `{file:path}` syntax with support for `~` home directory expansion.
    /// Reads only the first line of the file, trimmed of whitespace.
    fn substitute_file(&self, input: &str) -> Result<String, SubstitutionError> {
        let mut result = input.to_string();
        let mut error: Option<SubstitutionError> = None;

        // Collect all matches first
        let matches: Vec<_> = self
            .file_regex
            .captures_iter(input)
            .map(|cap| {
                let full_match = cap.get(0).map(|m| m.as_str().to_string());
                let file_path = cap.get(1).map(|m| m.as_str().to_string());
                (full_match, file_path)
            })
            .collect();

        for (full_match, file_path) in matches {
            let Some(full) = full_match else { continue };
            let Some(path_str) = file_path else { continue };

            let path = Self::expand_path(&path_str)?;

            if !path.exists() {
                error = Some(SubstitutionError::FileNotFound(path));
                continue;
            }

            let content =
                std::fs::read_to_string(&path).map_err(|e| SubstitutionError::FileReadError {
                    path: path.clone(),
                    source: e,
                })?;

            // Get first line, trimmed
            let first_line = content.lines().next().unwrap_or("").trim();
            result = result.replace(&full, first_line);
        }

        if let Some(e) = error {
            return Err(e);
        }

        Ok(result)
    }

    /// Expands a path string, handling `~` for home directory.
    fn expand_path(path: &str) -> Result<PathBuf, SubstitutionError> {
        let trimmed = path.trim();

        if let Some(rest) = trimmed.strip_prefix("~/") {
            let home = dirs::home_dir().ok_or(SubstitutionError::HomeDirNotFound)?;
            Ok(home.join(rest))
        } else if trimmed == "~" {
            dirs::home_dir().ok_or(SubstitutionError::HomeDirNotFound)
        } else {
            Ok(PathBuf::from(trimmed))
        }
    }
}

/// Recursively substitutes variables in a TOML value.
///
/// This function traverses the TOML value tree and applies substitution
/// to all string values, including those nested in arrays and tables.
///
/// # Arguments
/// * `value` - The TOML value to process (mutated in place)
/// * `substitution` - The substitution engine to use
///
/// # Returns
/// `Ok(())` if all substitutions succeed, or the first error encountered.
///
/// # Examples
/// ```
/// use cortex_common::{ConfigSubstitution, substitute_toml_value};
///
/// // SAFETY: Test-only example, no concurrent access to this env var
/// unsafe { std::env::set_var("API_KEY", "secret123") };
/// let mut value = toml::Value::String("{env:API_KEY}".to_string());
/// let sub = ConfigSubstitution::new();
/// substitute_toml_value(&mut value, &sub).unwrap();
/// assert_eq!(value.as_str(), Some("secret123"));
/// // SAFETY: Test cleanup
/// unsafe { std::env::remove_var("API_KEY") };
/// ```
pub fn substitute_toml_value(
    value: &mut toml::Value,
    substitution: &ConfigSubstitution,
) -> Result<(), SubstitutionError> {
    match value {
        toml::Value::String(s) => {
            *s = substitution.substitute(s)?;
        }
        toml::Value::Array(arr) => {
            for item in arr.iter_mut() {
                substitute_toml_value(item, substitution)?;
            }
        }
        toml::Value::Table(table) => {
            for (_, v) in table.iter_mut() {
                substitute_toml_value(v, substitution)?;
            }
        }
        // Integer, Float, Boolean, Datetime are not affected
        _ => {}
    }
    Ok(())
}

/// Convenience function to substitute variables in a TOML string.
///
/// Parses the TOML string, applies substitutions, and returns the modified value.
///
/// # Arguments
/// * `toml_content` - The TOML content as a string
///
/// # Returns
/// The parsed and substituted TOML value, or an error.
///
/// # Errors
/// Returns an error if TOML parsing fails or substitution fails.
pub fn substitute_toml_string(toml_content: &str) -> Result<toml::Value, SubstitutionParseError> {
    let mut value: toml::Value = toml::from_str(toml_content)?;
    let substitution = ConfigSubstitution::new();
    substitute_toml_value(&mut value, &substitution)?;
    Ok(value)
}

/// Error type for TOML parsing and substitution.
#[derive(Debug, Error)]
pub enum SubstitutionParseError {
    /// TOML parsing error.
    #[error("TOML parse error: {0}")]
    TomlError(#[from] toml::de::Error),

    /// Substitution error.
    #[error("Substitution error: {0}")]
    SubstitutionError(#[from] SubstitutionError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // Helper functions for safely setting/removing env vars in tests
    // SAFETY: These are only used in tests which run single-threaded by default
    // when using `cargo test -- --test-threads=1` or with serial_test
    unsafe fn set_test_env(key: &str, value: &str) {
        // SAFETY: Caller guarantees no concurrent access to this env var
        unsafe { std::env::set_var(key, value) };
    }

    unsafe fn remove_test_env(key: &str) {
        // SAFETY: Caller guarantees no concurrent access to this env var
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn test_env_substitution_basic() {
        // SAFETY: Test environment, no concurrent access to this specific var
        unsafe { set_test_env("TEST_CORTEX_VAR", "test_value") };

        let sub = ConfigSubstitution::new();
        let result = sub.substitute("{env:TEST_CORTEX_VAR}").unwrap();

        assert_eq!(result, "test_value");

        // SAFETY: Test cleanup
        unsafe { remove_test_env("TEST_CORTEX_VAR") };
    }

    #[test]
    fn test_env_substitution_with_default() {
        // Make sure the variable doesn't exist
        // SAFETY: Test environment
        unsafe { remove_test_env("TEST_NONEXISTENT_VAR") };

        let sub = ConfigSubstitution::new();
        let result = sub
            .substitute("{env:TEST_NONEXISTENT_VAR:default_value}")
            .unwrap();

        assert_eq!(result, "default_value");
    }

    #[test]
    fn test_env_substitution_empty_default() {
        // SAFETY: Test environment
        unsafe { remove_test_env("TEST_NONEXISTENT_VAR2") };

        let sub = ConfigSubstitution::new();
        let result = sub.substitute("{env:TEST_NONEXISTENT_VAR2:}").unwrap();

        assert_eq!(result, "");
    }

    #[test]
    fn test_env_substitution_existing_ignores_default() {
        // SAFETY: Test environment
        unsafe { set_test_env("TEST_EXISTING_VAR", "actual_value") };

        let sub = ConfigSubstitution::new();
        let result = sub.substitute("{env:TEST_EXISTING_VAR:default}").unwrap();

        assert_eq!(result, "actual_value");

        // SAFETY: Test cleanup
        unsafe { remove_test_env("TEST_EXISTING_VAR") };
    }

    #[test]
    fn test_env_substitution_not_found_error() {
        // SAFETY: Test environment
        unsafe { remove_test_env("TEST_MISSING_VAR") };

        let sub = ConfigSubstitution::new();
        let result = sub.substitute("{env:TEST_MISSING_VAR}");

        assert!(result.is_err());
        match result {
            Err(SubstitutionError::EnvVarNotFound(var)) => {
                assert_eq!(var, "TEST_MISSING_VAR");
            }
            _ => panic!("Expected EnvVarNotFound error"),
        }
    }

    #[test]
    fn test_file_substitution_basic() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "secret_api_key").unwrap();
        temp_file.flush().unwrap();

        let path = temp_file.path().to_string_lossy();
        let sub = ConfigSubstitution::new();
        let result = sub.substitute(&format!("{{file:{path}}}")).unwrap();

        assert_eq!(result, "secret_api_key");
    }

    #[test]
    fn test_file_substitution_multiline_reads_first() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "first_line").unwrap();
        writeln!(temp_file, "second_line").unwrap();
        writeln!(temp_file, "third_line").unwrap();
        temp_file.flush().unwrap();

        let path = temp_file.path().to_string_lossy();
        let sub = ConfigSubstitution::new();
        let result = sub.substitute(&format!("{{file:{path}}}")).unwrap();

        assert_eq!(result, "first_line");
    }

    #[test]
    fn test_file_substitution_trims_whitespace() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "  trimmed_value  ").unwrap();
        temp_file.flush().unwrap();

        let path = temp_file.path().to_string_lossy();
        let sub = ConfigSubstitution::new();
        let result = sub.substitute(&format!("{{file:{path}}}")).unwrap();

        assert_eq!(result, "trimmed_value");
    }

    #[test]
    fn test_file_not_found_error() {
        let sub = ConfigSubstitution::new();
        // Use a platform-agnostic path that won't exist
        let nonexistent_path = if cfg!(windows) {
            "C:\\nonexistent\\path\\to\\file.txt"
        } else {
            "/nonexistent/path/to/file.txt"
        };
        let result = sub.substitute(&format!("{{file:{}}}", nonexistent_path));

        assert!(result.is_err());
        match result {
            Err(SubstitutionError::FileNotFound(path)) => {
                assert_eq!(path, PathBuf::from(nonexistent_path));
            }
            _ => panic!("Expected FileNotFound error"),
        }
    }

    #[test]
    fn test_home_expansion() {
        // Test that ~ expansion works (we can't test the actual path without knowing home)
        let result = ConfigSubstitution::expand_path("~/test/path");
        assert!(result.is_ok());

        let path = result.unwrap();
        assert!(path.ends_with("test/path"));
        assert!(!path.to_string_lossy().contains('~'));
    }

    #[test]
    fn test_multiple_substitutions() {
        // SAFETY: Test environment
        unsafe {
            set_test_env("TEST_VAR1", "value1");
            set_test_env("TEST_VAR2", "value2");
        }

        let sub = ConfigSubstitution::new();
        let result = sub
            .substitute("prefix-{env:TEST_VAR1}-middle-{env:TEST_VAR2}-suffix")
            .unwrap();

        assert_eq!(result, "prefix-value1-middle-value2-suffix");

        // SAFETY: Test cleanup
        unsafe {
            remove_test_env("TEST_VAR1");
            remove_test_env("TEST_VAR2");
        }
    }

    #[test]
    fn test_mixed_env_and_file_substitutions() {
        // SAFETY: Test environment
        unsafe { set_test_env("TEST_MIX_VAR", "env_value") };

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "file_value").unwrap();
        temp_file.flush().unwrap();

        let path = temp_file.path().to_string_lossy();
        let sub = ConfigSubstitution::new();
        let result = sub
            .substitute(&format!("env={{env:TEST_MIX_VAR}}, file={{file:{path}}}"))
            .unwrap();

        assert_eq!(result, "env=env_value, file=file_value");

        // SAFETY: Test cleanup
        unsafe { remove_test_env("TEST_MIX_VAR") };
    }

    #[test]
    fn test_no_substitution_needed() {
        let sub = ConfigSubstitution::new();
        let result = sub.substitute("plain string without placeholders").unwrap();

        assert_eq!(result, "plain string without placeholders");
    }

    #[test]
    fn test_toml_value_substitution_string() {
        // SAFETY: Test environment
        unsafe { set_test_env("TEST_TOML_VAR", "toml_value") };

        let mut value = toml::Value::String("{env:TEST_TOML_VAR}".to_string());
        let sub = ConfigSubstitution::new();
        substitute_toml_value(&mut value, &sub).unwrap();

        assert_eq!(value.as_str(), Some("toml_value"));

        // SAFETY: Test cleanup
        unsafe { remove_test_env("TEST_TOML_VAR") };
    }

    #[test]
    fn test_toml_value_substitution_array() {
        // SAFETY: Test environment
        unsafe {
            set_test_env("TEST_ARR_VAR1", "arr1");
            set_test_env("TEST_ARR_VAR2", "arr2");
        }

        let mut value = toml::Value::Array(vec![
            toml::Value::String("{env:TEST_ARR_VAR1}".to_string()),
            toml::Value::String("{env:TEST_ARR_VAR2}".to_string()),
            toml::Value::Integer(42), // Should not be affected
        ]);

        let sub = ConfigSubstitution::new();
        substitute_toml_value(&mut value, &sub).unwrap();

        let arr = value.as_array().unwrap();
        assert_eq!(arr[0].as_str(), Some("arr1"));
        assert_eq!(arr[1].as_str(), Some("arr2"));
        assert_eq!(arr[2].as_integer(), Some(42));

        // SAFETY: Test cleanup
        unsafe {
            remove_test_env("TEST_ARR_VAR1");
            remove_test_env("TEST_ARR_VAR2");
        }
    }

    #[test]
    fn test_toml_value_substitution_table() {
        // SAFETY: Test environment
        unsafe { set_test_env("TEST_TABLE_VAR", "table_value") };

        let mut table = toml::map::Map::new();
        table.insert(
            "key".to_string(),
            toml::Value::String("{env:TEST_TABLE_VAR}".to_string()),
        );
        table.insert("number".to_string(), toml::Value::Integer(100));

        let mut value = toml::Value::Table(table);
        let sub = ConfigSubstitution::new();
        substitute_toml_value(&mut value, &sub).unwrap();

        let t = value.as_table().unwrap();
        assert_eq!(t.get("key").and_then(|v| v.as_str()), Some("table_value"));
        assert_eq!(t.get("number").and_then(|v| v.as_integer()), Some(100));

        // SAFETY: Test cleanup
        unsafe { remove_test_env("TEST_TABLE_VAR") };
    }

    #[test]
    fn test_toml_value_substitution_nested() {
        // SAFETY: Test environment
        unsafe { set_test_env("TEST_NESTED_VAR", "nested_value") };

        let mut inner_table = toml::map::Map::new();
        inner_table.insert(
            "api_key".to_string(),
            toml::Value::String("{env:TEST_NESTED_VAR}".to_string()),
        );

        let mut outer_table = toml::map::Map::new();
        outer_table.insert("provider".to_string(), toml::Value::Table(inner_table));

        let mut value = toml::Value::Table(outer_table);
        let sub = ConfigSubstitution::new();
        substitute_toml_value(&mut value, &sub).unwrap();

        let outer = value.as_table().unwrap();
        let inner = outer.get("provider").and_then(|v| v.as_table()).unwrap();
        assert_eq!(
            inner.get("api_key").and_then(|v| v.as_str()),
            Some("nested_value")
        );

        // SAFETY: Test cleanup
        unsafe { remove_test_env("TEST_NESTED_VAR") };
    }

    #[test]
    fn test_substitute_toml_string() {
        // SAFETY: Test environment
        unsafe { set_test_env("TEST_PARSE_VAR", "parsed_value") };

        let toml_content = r#"
[providers]
api_key = "{env:TEST_PARSE_VAR}"
timeout = 30
"#;

        let value = substitute_toml_string(toml_content).unwrap();
        let table = value.as_table().unwrap();
        let providers = table.get("providers").and_then(|v| v.as_table()).unwrap();

        assert_eq!(
            providers.get("api_key").and_then(|v| v.as_str()),
            Some("parsed_value")
        );
        assert_eq!(
            providers.get("timeout").and_then(|v| v.as_integer()),
            Some(30)
        );

        // SAFETY: Test cleanup
        unsafe { remove_test_env("TEST_PARSE_VAR") };
    }

    #[test]
    fn test_realistic_config_example() {
        // SAFETY: Test environment
        unsafe {
            set_test_env("OPENAI_API_KEY", "sk-test-key-123");
            set_test_env("CUSTOM_API_URL", "https://custom.api.com");
        }

        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "secret-custom-key-456").unwrap();
        temp_file.flush().unwrap();
        // Convert Windows backslashes to forward slashes for TOML compatibility
        let secret_path = temp_file.path().to_string_lossy().replace('\\', "/");

        let toml_content = format!(
            r#"
[providers.openai]
api_key = "{{env:OPENAI_API_KEY}}"

[providers.custom]
api_key = "{{file:{secret_path}}}"
base_url = "{{env:CUSTOM_API_URL}}"

[enterprise]
url = "{{env:ENTERPRISE_URL:https://default.enterprise.com}}"
"#
        );

        let value = substitute_toml_string(&toml_content).unwrap();
        let table = value.as_table().unwrap();
        let providers = table.get("providers").and_then(|v| v.as_table()).unwrap();

        let openai = providers.get("openai").and_then(|v| v.as_table()).unwrap();
        assert_eq!(
            openai.get("api_key").and_then(|v| v.as_str()),
            Some("sk-test-key-123")
        );

        let custom = providers.get("custom").and_then(|v| v.as_table()).unwrap();
        assert_eq!(
            custom.get("api_key").and_then(|v| v.as_str()),
            Some("secret-custom-key-456")
        );
        assert_eq!(
            custom.get("base_url").and_then(|v| v.as_str()),
            Some("https://custom.api.com")
        );

        let enterprise = table.get("enterprise").and_then(|v| v.as_table()).unwrap();
        assert_eq!(
            enterprise.get("url").and_then(|v| v.as_str()),
            Some("https://default.enterprise.com")
        );

        // SAFETY: Test cleanup
        unsafe {
            remove_test_env("OPENAI_API_KEY");
            remove_test_env("CUSTOM_API_URL");
        }
    }

    #[test]
    fn test_special_characters_in_value() {
        // SAFETY: Test environment
        unsafe { set_test_env("TEST_SPECIAL", "value=with:special&chars") };

        let sub = ConfigSubstitution::new();
        let result = sub.substitute("{env:TEST_SPECIAL}").unwrap();

        assert_eq!(result, "value=with:special&chars");

        // SAFETY: Test cleanup
        unsafe { remove_test_env("TEST_SPECIAL") };
    }

    #[test]
    fn test_empty_file() {
        let temp_file = NamedTempFile::new().unwrap();
        // File is empty

        let path = temp_file.path().to_string_lossy();
        let sub = ConfigSubstitution::new();
        let result = sub.substitute(&format!("{{file:{path}}}")).unwrap();

        assert_eq!(result, "");
    }

    #[test]
    fn test_url_in_default() {
        // SAFETY: Test environment
        unsafe { remove_test_env("TEST_URL_VAR") };

        let sub = ConfigSubstitution::new();
        let result = sub
            .substitute("{env:TEST_URL_VAR:https://api.default.com/v1}")
            .unwrap();

        assert_eq!(result, "https://api.default.com/v1");
    }

    #[test]
    fn test_config_substitution_default() {
        let sub = ConfigSubstitution::default();
        // SAFETY: Test environment
        unsafe { set_test_env("TEST_DEFAULT_IMPL", "works") };

        let result = sub.substitute("{env:TEST_DEFAULT_IMPL}").unwrap();
        assert_eq!(result, "works");

        // SAFETY: Test cleanup
        unsafe { remove_test_env("TEST_DEFAULT_IMPL") };
    }
}
