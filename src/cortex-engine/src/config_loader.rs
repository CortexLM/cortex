//! Configuration loading utilities.
//!
//! Provides utilities for loading configuration from multiple
//! sources with precedence handling and merging.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CortexError, Result};

/// Configuration source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ConfigSource {
    /// Default values.
    #[default]
    Default,
    /// System configuration.
    System,
    /// User configuration.
    User,
    /// Project configuration.
    Project,
    /// Environment variables.
    Environment,
    /// Command line arguments.
    CommandLine,
    /// Runtime overrides.
    Runtime,
}

impl ConfigSource {
    /// Get priority (higher = more important).
    pub fn priority(&self) -> u8 {
        match self {
            Self::Default => 0,
            Self::System => 1,
            Self::User => 2,
            Self::Project => 3,
            Self::Environment => 4,
            Self::CommandLine => 5,
            Self::Runtime => 6,
        }
    }

    /// Get source name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::System => "system",
            Self::User => "user",
            Self::Project => "project",
            Self::Environment => "environment",
            Self::CommandLine => "command_line",
            Self::Runtime => "runtime",
        }
    }
}

/// Configuration value with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigValue {
    /// The value.
    pub value: serde_json::Value,
    /// Source of the value.
    pub source: ConfigSource,
    /// File path if from file.
    pub file_path: Option<PathBuf>,
    /// Environment variable if from env.
    pub env_var: Option<String>,
}

impl ConfigValue {
    /// Create a new value.
    pub fn new(value: serde_json::Value, source: ConfigSource) -> Self {
        Self {
            value,
            source,
            file_path: None,
            env_var: None,
        }
    }

    /// Set file path.
    pub fn from_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file_path = Some(path.into());
        self
    }

    /// Set env var.
    pub fn from_env(mut self, var: impl Into<String>) -> Self {
        self.env_var = Some(var.into());
        self
    }

    /// Get as string.
    pub fn as_str(&self) -> Option<&str> {
        self.value.as_str()
    }

    /// Get as bool.
    pub fn as_bool(&self) -> Option<bool> {
        self.value.as_bool()
    }

    /// Get as i64.
    pub fn as_i64(&self) -> Option<i64> {
        self.value.as_i64()
    }

    /// Get as f64.
    pub fn as_f64(&self) -> Option<f64> {
        self.value.as_f64()
    }

    /// Get as array.
    pub fn as_array(&self) -> Option<&Vec<serde_json::Value>> {
        self.value.as_array()
    }

    /// Get as object.
    pub fn as_object(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.value.as_object()
    }
}

/// Configuration loader.
pub struct ConfigLoader {
    /// Config values by key.
    values: HashMap<String, ConfigValue>,
    /// Search paths.
    search_paths: Vec<PathBuf>,
    /// Environment prefix.
    env_prefix: Option<String>,
    /// Config file names.
    config_names: Vec<String>,
}

impl ConfigLoader {
    /// Create a new loader.
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
            search_paths: Vec::new(),
            env_prefix: None,
            config_names: vec![
                "config.toml".to_string(),
                "config.yaml".to_string(),
                "config.json".to_string(),
            ],
        }
    }

    /// Add search path.
    pub fn search_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.search_paths.push(path.into());
        self
    }

    /// Set environment prefix.
    pub fn env_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.env_prefix = Some(prefix.into());
        self
    }

    /// Add config file name.
    pub fn config_name(mut self, name: impl Into<String>) -> Self {
        self.config_names.push(name.into());
        self
    }

    /// Set a default value.
    pub fn default<T: Serialize>(mut self, key: impl Into<String>, value: T) -> Self {
        if let Ok(v) = serde_json::to_value(value) {
            self.values
                .insert(key.into(), ConfigValue::new(v, ConfigSource::Default));
        }
        self
    }

    /// Load from all sources.
    pub fn load(&mut self) -> Result<()> {
        // Load from files
        for path in &self.search_paths.clone() {
            for name in &self.config_names.clone() {
                let file_path = path.join(name);
                if file_path.exists() {
                    self.load_file(&file_path)?;
                }
            }
        }

        // Load from environment
        self.load_environment()?;

        Ok(())
    }

    /// Load from a specific file.
    pub fn load_file(&mut self, path: &Path) -> Result<()> {
        let content = fs::read_to_string(path)?;

        let source = if path.starts_with("/etc") {
            ConfigSource::System
        } else if path.starts_with(dirs::home_dir().unwrap_or_default()) {
            ConfigSource::User
        } else {
            ConfigSource::Project
        };

        let values: serde_json::Value = if path.extension().map(|e| e == "toml").unwrap_or(false) {
            toml::from_str(&content)
                .map(|v: toml::Value| serde_json::to_value(v).unwrap_or_default())
                .map_err(|e| CortexError::InvalidInput(e.to_string()))?
        } else if path
            .extension()
            .map(|e| e == "yaml" || e == "yml")
            .unwrap_or(false)
        {
            serde_yaml::from_str(&content).map_err(|e| CortexError::InvalidInput(e.to_string()))?
        } else {
            serde_json::from_str(&content)?
        };

        self.merge_values(values, source, Some(path.to_path_buf()))?;

        Ok(())
    }

    /// Load from environment.
    fn load_environment(&mut self) -> Result<()> {
        let prefix = self.env_prefix.as_deref().unwrap_or("Cortex");
        let prefix_with_underscore = format!("{prefix}_");

        for (key, value) in std::env::vars() {
            if key.starts_with(&prefix_with_underscore) {
                let config_key = key[prefix_with_underscore.len()..]
                    .to_lowercase()
                    .replace("__", ".");

                let json_value = parse_env_value(&value);

                self.values.insert(
                    config_key,
                    ConfigValue::new(json_value, ConfigSource::Environment).from_env(&key),
                );
            }
        }

        Ok(())
    }

    /// Merge values from a JSON object.
    fn merge_values(
        &mut self,
        values: serde_json::Value,
        source: ConfigSource,
        file_path: Option<PathBuf>,
    ) -> Result<()> {
        if let Some(obj) = values.as_object() {
            for (key, value) in obj {
                self.merge_value(key, value.clone(), source, file_path.clone());
            }
        }
        Ok(())
    }

    /// Merge a single value.
    fn merge_value(
        &mut self,
        key: &str,
        value: serde_json::Value,
        source: ConfigSource,
        file_path: Option<PathBuf>,
    ) {
        // Flatten nested objects
        if let Some(obj) = value.as_object() {
            for (nested_key, nested_value) in obj {
                let full_key = format!("{key}.{nested_key}");
                self.merge_value(&full_key, nested_value.clone(), source, file_path.clone());
            }
        } else {
            // Check if we should override
            let should_insert = match self.values.get(key) {
                Some(existing) => source.priority() >= existing.source.priority(),
                None => true,
            };

            if should_insert {
                let mut config_value = ConfigValue::new(value, source);
                if let Some(path) = file_path {
                    config_value = config_value.from_file(path);
                }
                self.values.insert(key.to_string(), config_value);
            }
        }
    }

    /// Set a runtime value.
    pub fn set<T: Serialize>(&mut self, key: impl Into<String>, value: T) {
        if let Ok(v) = serde_json::to_value(value) {
            self.values
                .insert(key.into(), ConfigValue::new(v, ConfigSource::Runtime));
        }
    }

    /// Get a value.
    pub fn get(&self, key: &str) -> Option<&ConfigValue> {
        self.values.get(key)
    }

    /// Get a string value.
    pub fn get_string(&self, key: &str) -> Option<String> {
        self.get(key)
            .and_then(|v| v.as_str().map(std::string::ToString::to_string))
    }

    /// Get a bool value.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(ConfigValue::as_bool)
    }

    /// Get an i64 value.
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(ConfigValue::as_i64)
    }

    /// Get a f64 value.
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(ConfigValue::as_f64)
    }

    /// Get string with default.
    pub fn get_string_or(&self, key: &str, default: &str) -> String {
        self.get_string(key).unwrap_or_else(|| default.to_string())
    }

    /// Get bool with default.
    pub fn get_bool_or(&self, key: &str, default: bool) -> bool {
        self.get_bool(key).unwrap_or(default)
    }

    /// Get i64 with default.
    pub fn get_i64_or(&self, key: &str, default: i64) -> i64 {
        self.get_i64(key).unwrap_or(default)
    }

    /// Check if key exists.
    pub fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// Get all keys.
    pub fn keys(&self) -> Vec<&String> {
        self.values.keys().collect()
    }

    /// Get all values.
    pub fn all(&self) -> &HashMap<String, ConfigValue> {
        &self.values
    }

    /// Export to JSON.
    pub fn to_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        for (key, value) in &self.values {
            obj.insert(key.clone(), value.value.clone());
        }
        serde_json::Value::Object(obj)
    }
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse environment value to JSON.
fn parse_env_value(value: &str) -> serde_json::Value {
    // Try parsing as JSON first
    if let Ok(parsed) = serde_json::from_str(value) {
        return parsed;
    }

    // Try as bool
    if value.eq_ignore_ascii_case("true") {
        return serde_json::Value::Bool(true);
    }
    if value.eq_ignore_ascii_case("false") {
        return serde_json::Value::Bool(false);
    }

    // Try as number
    if let Ok(n) = value.parse::<i64>() {
        return serde_json::Value::Number(n.into());
    }
    if let Ok(n) = value.parse::<f64>()
        && let Some(n) = serde_json::Number::from_f64(n)
    {
        return serde_json::Value::Number(n);
    }

    // Default to string
    serde_json::Value::String(value.to_string())
}

/// Configuration builder for typed configs.
pub struct ConfigBuilder<T> {
    loader: ConfigLoader,
    _phantom: std::marker::PhantomData<T>,
}

impl<T: for<'de> Deserialize<'de>> ConfigBuilder<T> {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            loader: ConfigLoader::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Add search path.
    pub fn search_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.loader = self.loader.search_path(path);
        self
    }

    /// Set environment prefix.
    pub fn env_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.loader = self.loader.env_prefix(prefix);
        self
    }

    /// Build the configuration.
    pub fn build(mut self) -> Result<T> {
        self.loader.load()?;
        let json = self.loader.to_json();
        serde_json::from_value(json).map_err(|e| CortexError::InvalidInput(e.to_string()))
    }
}

impl<T: for<'de> Deserialize<'de>> Default for ConfigBuilder<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Watch configuration for changes.
pub struct ConfigWatcher {
    /// Paths to watch.
    paths: Vec<PathBuf>,
    /// Callback on change.
    on_change: Option<Box<dyn Fn(&Path) + Send + Sync>>,
}

impl ConfigWatcher {
    /// Create a new watcher.
    pub fn new() -> Self {
        Self {
            paths: Vec::new(),
            on_change: None,
        }
    }

    /// Add path to watch.
    pub fn watch(mut self, path: impl Into<PathBuf>) -> Self {
        self.paths.push(path.into());
        self
    }

    /// Set change callback.
    pub fn on_change<F>(mut self, callback: F) -> Self
    where
        F: Fn(&Path) + Send + Sync + 'static,
    {
        self.on_change = Some(Box::new(callback));
        self
    }

    /// Get watched paths.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

impl Default for ConfigWatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_config_source_priority() {
        assert!(ConfigSource::Runtime.priority() > ConfigSource::CommandLine.priority());
        assert!(ConfigSource::CommandLine.priority() > ConfigSource::Environment.priority());
        assert!(ConfigSource::Environment.priority() > ConfigSource::Project.priority());
    }

    #[test]
    fn test_config_value() {
        let value = ConfigValue::new(serde_json::json!("test"), ConfigSource::User);
        assert_eq!(value.as_str(), Some("test"));
        assert_eq!(value.source, ConfigSource::User);

        let value = ConfigValue::new(serde_json::json!(42), ConfigSource::Default);
        assert_eq!(value.as_i64(), Some(42));

        let value = ConfigValue::new(serde_json::json!(true), ConfigSource::Environment);
        assert_eq!(value.as_bool(), Some(true));
    }

    #[test]
    fn test_config_loader() {
        let mut loader = ConfigLoader::new()
            .default("key1", "value1")
            .default("key2", 42);

        loader.set("key1", "overridden");

        assert_eq!(loader.get_string("key1"), Some("overridden".to_string()));
        assert_eq!(loader.get_i64("key2"), Some(42));
    }

    #[test]
    fn test_parse_env_value() {
        assert_eq!(parse_env_value("true"), serde_json::json!(true));
        assert_eq!(parse_env_value("false"), serde_json::json!(false));
        assert_eq!(parse_env_value("42"), serde_json::json!(42));
        assert_eq!(parse_env_value("2.5"), serde_json::json!(2.5));
        assert_eq!(parse_env_value("hello"), serde_json::json!("hello"));
        assert_eq!(parse_env_value("[1,2,3]"), serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn test_config_loader_file() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        fs::write(
            &config_path,
            r#"{"key": "value", "nested": {"inner": 123}}"#,
        )
        .unwrap();

        let mut loader = ConfigLoader::new().search_path(dir.path());

        loader.load().unwrap();

        assert_eq!(loader.get_string("key"), Some("value".to_string()));
        assert_eq!(loader.get_i64("nested.inner"), Some(123));
    }

    #[test]
    fn test_config_loader_defaults() {
        let loader = ConfigLoader::new()
            .default("timeout", 30)
            .default("enabled", true);

        assert_eq!(loader.get_i64_or("timeout", 0), 30);
        assert_eq!(loader.get_bool_or("enabled", false), true);
        assert_eq!(loader.get_string_or("missing", "default"), "default");
    }

    #[test]
    fn test_config_loader_override() {
        let mut loader = ConfigLoader::new().default("key", "default");

        loader.set("key", "override");

        let value = loader.get("key").unwrap();
        assert_eq!(value.as_str(), Some("override"));
        assert_eq!(value.source, ConfigSource::Runtime);
    }
}
