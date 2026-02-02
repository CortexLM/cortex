//! Template variable expansion for custom commands.
//!
//! Supports the following variables:
//! - `{{input}}` - User input/arguments
//! - `{{file:path}}` - Content of a file
//! - `{{selection}}` - Selected text (from editor)
//! - `{{clipboard}}` - Clipboard content
//! - `{{cwd}}` - Current working directory
//! - `{{date}}` - Current date
//! - `{{time}}` - Current time
//! - `{{datetime}}` - Current date and time
//! - `{{env:VAR}}` - Environment variable

use std::collections::HashMap;
use std::path::Path;

use regex::Regex;

/// Template context for variable expansion.
#[derive(Debug, Clone, Default)]
pub struct TemplateContext {
    /// User input/arguments.
    pub input: String,
    /// Selected text (from editor integration).
    pub selection: Option<String>,
    /// Clipboard content.
    pub clipboard: Option<String>,
    /// Current working directory.
    pub cwd: Option<String>,
    /// Additional custom variables.
    pub custom: HashMap<String, String>,
}

impl TemplateContext {
    /// Create a new template context with user input.
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            ..Default::default()
        }
    }

    /// Set the selection.
    pub fn with_selection(mut self, selection: impl Into<String>) -> Self {
        self.selection = Some(selection.into());
        self
    }

    /// Set the clipboard content.
    pub fn with_clipboard(mut self, clipboard: impl Into<String>) -> Self {
        self.clipboard = Some(clipboard.into());
        self
    }

    /// Set the current working directory.
    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Add a custom variable.
    pub fn with_var(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.custom.insert(name.into(), value.into());
        self
    }
}

/// Expand template variables in a string.
pub fn expand_template(template: &str, ctx: &TemplateContext) -> String {
    let mut result = template.to_string();

    // Simple variables
    result = result.replace("{{input}}", &ctx.input);
    result = result.replace("{{selection}}", ctx.selection.as_deref().unwrap_or(""));
    result = result.replace("{{clipboard}}", ctx.clipboard.as_deref().unwrap_or(""));
    result = result.replace("{{cwd}}", ctx.cwd.as_deref().unwrap_or("."));

    // Date/time variables
    let now = chrono::Local::now();
    result = result.replace("{{date}}", &now.format("%Y-%m-%d").to_string());
    result = result.replace("{{time}}", &now.format("%H:%M:%S").to_string());
    result = result.replace("{{datetime}}", &now.format("%Y-%m-%d %H:%M:%S").to_string());

    // File variables: {{file:path}}
    result = expand_file_variables(&result);

    // Environment variables: {{env:VAR}}
    result = expand_env_variables(&result);

    // Custom variables
    for (name, value) in &ctx.custom {
        result = result.replace(&format!("{{{{{name}}}}}"), value);
    }

    result
}

/// Expand file variables like {{file:path/to/file}}.
fn expand_file_variables(template: &str) -> String {
    let re = Regex::new(r"\{\{file:([^}]+)\}\}").unwrap();

    re.replace_all(template, |caps: &regex::Captures| {
        let path_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let path = Path::new(path_str);

        match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) => format!("[Error reading {}: {}]", path_str, e),
        }
    })
    .to_string()
}

/// Expand environment variables like {{env:HOME}}.
fn expand_env_variables(template: &str) -> String {
    let re = Regex::new(r"\{\{env:([^}]+)\}\}").unwrap();

    re.replace_all(template, |caps: &regex::Captures| {
        let var_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        std::env::var(var_name).unwrap_or_default()
    })
    .to_string()
}

/// Check if a template contains any variables.
pub fn has_variables(template: &str) -> bool {
    template.contains("{{")
}

/// List all variables in a template.
pub fn list_variables(template: &str) -> Vec<String> {
    let re = Regex::new(r"\{\{([^}]+)\}\}").unwrap();

    re.captures_iter(template)
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// Validate that all required variables can be resolved.
pub fn validate_template(template: &str, ctx: &TemplateContext) -> Result<(), Vec<String>> {
    let variables = list_variables(template);
    let mut missing = Vec::new();

    for var in variables {
        let can_resolve = match var.as_str() {
            "input" => true, // Always available
            "selection" => ctx.selection.is_some(),
            "clipboard" => ctx.clipboard.is_some(),
            "cwd" => ctx.cwd.is_some() || std::env::current_dir().is_ok(),
            "date" | "time" | "datetime" => true,
            v if v.starts_with("file:") => {
                let path = &v[5..];
                Path::new(path).exists()
            }
            v if v.starts_with("env:") => {
                let var_name = &v[4..];
                std::env::var(var_name).is_ok()
            }
            v => ctx.custom.contains_key(v),
        };

        if !can_resolve {
            missing.push(var);
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_simple_variables() {
        let ctx = TemplateContext::new("my input")
            .with_selection("selected text")
            .with_clipboard("clipboard content")
            .with_cwd("/home/user");

        let template =
            "Input: {{input}}\nSelection: {{selection}}\nClipboard: {{clipboard}}\nCWD: {{cwd}}";
        let result = expand_template(template, &ctx);

        assert!(result.contains("Input: my input"));
        assert!(result.contains("Selection: selected text"));
        assert!(result.contains("Clipboard: clipboard content"));
        assert!(result.contains("CWD: /home/user"));
    }

    #[test]
    fn test_expand_datetime_variables() {
        let ctx = TemplateContext::new("");
        let template = "Date: {{date}}, Time: {{time}}";
        let result = expand_template(template, &ctx);

        // Just check that variables were replaced (not empty)
        assert!(!result.contains("{{date}}"));
        assert!(!result.contains("{{time}}"));
    }

    #[test]
    fn test_expand_custom_variables() {
        let ctx = TemplateContext::new("")
            .with_var("project", "my-project")
            .with_var("version", "1.0.0");

        let template = "Project: {{project}} v{{version}}";
        let result = expand_template(template, &ctx);

        assert_eq!(result, "Project: my-project v1.0.0");
    }

    #[test]
    fn test_has_variables() {
        assert!(has_variables("Hello {{name}}"));
        assert!(!has_variables("Hello world"));
    }

    #[test]
    fn test_list_variables() {
        let vars = list_variables("{{a}} and {{b}} and {{file:test.txt}}");
        assert_eq!(vars.len(), 3);
        assert!(vars.contains(&"a".to_string()));
        assert!(vars.contains(&"b".to_string()));
        assert!(vars.contains(&"file:test.txt".to_string()));
    }

    #[test]
    fn test_missing_selection() {
        let ctx = TemplateContext::new("input");
        let template = "Selection: {{selection}}";
        let result = expand_template(template, &ctx);

        // Should replace with empty string
        assert_eq!(result, "Selection: ");
    }
}
