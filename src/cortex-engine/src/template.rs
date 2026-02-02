//! Template system.
//!
//! Provides template parsing, variable substitution, and
//! prompt template management.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CortexError, Result};

/// Template variable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateVariable {
    /// Variable name.
    pub name: String,
    /// Description.
    pub description: Option<String>,
    /// Default value.
    pub default: Option<String>,
    /// Is required.
    pub required: bool,
    /// Value type.
    pub value_type: ValueType,
}

impl TemplateVariable {
    /// Create a new variable.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            default: None,
            required: true,
            value_type: ValueType::String,
        }
    }

    /// Set description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set default value.
    pub fn default_value(mut self, value: impl Into<String>) -> Self {
        self.default = Some(value.into());
        self.required = false;
        self
    }

    /// Mark as optional.
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Set value type.
    pub fn of_type(mut self, vt: ValueType) -> Self {
        self.value_type = vt;
        self
    }
}

/// Value type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ValueType {
    /// String value.
    #[default]
    String,
    /// Number value.
    Number,
    /// Boolean value.
    Boolean,
    /// List value.
    List,
    /// Object value.
    Object,
}

/// Template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    /// Template name.
    pub name: String,
    /// Template content.
    pub content: String,
    /// Description.
    pub description: Option<String>,
    /// Variables.
    pub variables: Vec<TemplateVariable>,
    /// Tags.
    pub tags: Vec<String>,
    /// Category.
    pub category: Option<String>,
}

impl Template {
    /// Create a new template.
    pub fn new(name: impl Into<String>, content: impl Into<String>) -> Self {
        let content = content.into();
        let variables = Self::extract_variables(&content);

        Self {
            name: name.into(),
            content,
            description: None,
            variables,
            tags: Vec::new(),
            category: None,
        }
    }

    /// Set description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Add tag.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Set category.
    pub fn category(mut self, cat: impl Into<String>) -> Self {
        self.category = Some(cat.into());
        self
    }

    /// Add variable definition.
    pub fn variable(mut self, var: TemplateVariable) -> Self {
        // Update existing or add new
        if let Some(v) = self.variables.iter_mut().find(|v| v.name == var.name) {
            *v = var;
        } else {
            self.variables.push(var);
        }
        self
    }

    /// Extract variables from template content.
    fn extract_variables(content: &str) -> Vec<TemplateVariable> {
        let mut vars = Vec::new();
        let mut i = 0;
        let chars: Vec<char> = content.chars().collect();

        while i < chars.len() {
            if i + 1 < chars.len() && chars[i] == '{' && chars[i + 1] == '{' {
                // Find closing braces
                let start = i + 2;
                let mut end = start;
                let mut depth = 1;

                while end < chars.len() && depth > 0 {
                    if end + 1 < chars.len() && chars[end] == '}' && chars[end + 1] == '}' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    } else if end + 1 < chars.len() && chars[end] == '{' && chars[end + 1] == '{' {
                        depth += 1;
                        end += 1;
                    }
                    end += 1;
                }

                if depth == 0 {
                    let var_name: String = chars[start..end].iter().collect();
                    let var_name = var_name.trim();

                    // Skip if already exists
                    if !vars.iter().any(|v: &TemplateVariable| v.name == var_name) {
                        vars.push(TemplateVariable::new(var_name));
                    }

                    i = end + 2;
                    continue;
                }
            }
            i += 1;
        }

        vars
    }

    /// Render template with values.
    pub fn render(&self, values: &HashMap<String, String>) -> Result<String> {
        // Check required variables
        for var in &self.variables {
            if var.required && !values.contains_key(&var.name) && var.default.is_none() {
                return Err(CortexError::InvalidInput(format!(
                    "Missing required variable: {}",
                    var.name
                )));
            }
        }

        // Build substitution map with defaults
        let mut subs = HashMap::new();
        for var in &self.variables {
            if let Some(value) = values.get(&var.name) {
                subs.insert(&var.name, value.as_str());
            } else if let Some(ref default) = var.default {
                subs.insert(&var.name, default.as_str());
            }
        }

        // Perform substitution
        let mut result = self.content.clone();
        for (name, value) in subs {
            let pattern = format!("{{{{{name}}}}}");
            result = result.replace(&pattern, value);
        }

        Ok(result)
    }

    /// Get required variables.
    pub fn required_variables(&self) -> Vec<&TemplateVariable> {
        self.variables.iter().filter(|v| v.required).collect()
    }

    /// Get optional variables.
    pub fn optional_variables(&self) -> Vec<&TemplateVariable> {
        self.variables.iter().filter(|v| !v.required).collect()
    }
}

/// Template engine.
pub struct TemplateEngine {
    /// Registered templates.
    templates: HashMap<String, Template>,
    /// Template directories.
    template_dirs: Vec<PathBuf>,
    /// Global variables.
    globals: HashMap<String, String>,
}

impl TemplateEngine {
    /// Create a new engine.
    pub fn new() -> Self {
        Self {
            templates: HashMap::new(),
            template_dirs: Vec::new(),
            globals: HashMap::new(),
        }
    }

    /// Add template directory.
    pub fn add_dir(&mut self, dir: impl Into<PathBuf>) {
        self.template_dirs.push(dir.into());
    }

    /// Set global variable.
    pub fn set_global(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.globals.insert(name.into(), value.into());
    }

    /// Register a template.
    pub fn register(&mut self, template: Template) {
        self.templates.insert(template.name.clone(), template);
    }

    /// Get a template.
    pub fn get(&self, name: &str) -> Option<&Template> {
        self.templates.get(name)
    }

    /// List all templates.
    pub fn list(&self) -> Vec<&Template> {
        self.templates.values().collect()
    }

    /// List templates by category.
    pub fn by_category(&self, category: &str) -> Vec<&Template> {
        self.templates
            .values()
            .filter(|t| t.category.as_deref() == Some(category))
            .collect()
    }

    /// List templates by tag.
    pub fn by_tag(&self, tag: &str) -> Vec<&Template> {
        self.templates
            .values()
            .filter(|t| t.tags.contains(&tag.to_string()))
            .collect()
    }

    /// Load templates from directories.
    pub fn load_templates(&mut self) -> Result<usize> {
        let mut count = 0;

        for dir in self.template_dirs.clone() {
            if !dir.exists() {
                continue;
            }

            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_file()
                    && let Some(ext) = path.extension()
                {
                    if ext == "json" || ext == "yaml" || ext == "yml" {
                        if let Ok(template) = self.load_template_file(&path) {
                            self.register(template);
                            count += 1;
                        }
                    } else if (ext == "txt" || ext == "md")
                        && let Ok(template) = self.load_raw_template(&path)
                    {
                        self.register(template);
                        count += 1;
                    }
                }
            }
        }

        Ok(count)
    }

    /// Load template from file.
    fn load_template_file(&self, path: &Path) -> Result<Template> {
        let content = std::fs::read_to_string(path)?;

        if path.extension().map(|e| e == "json").unwrap_or(false) {
            let template: Template = serde_json::from_str(&content)?;
            Ok(template)
        } else {
            // YAML
            let template: Template = serde_yaml::from_str(&content)
                .map_err(|e| CortexError::InvalidInput(e.to_string()))?;
            Ok(template)
        }
    }

    /// Load raw template file.
    fn load_raw_template(&self, path: &Path) -> Result<Template> {
        let content = std::fs::read_to_string(path)?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();

        Ok(Template::new(name, content))
    }

    /// Render a template.
    pub fn render(&self, name: &str, values: &HashMap<String, String>) -> Result<String> {
        let template = self
            .get(name)
            .ok_or_else(|| CortexError::NotFound(format!("Template not found: {name}")))?;

        // Merge with globals
        let mut merged = self.globals.clone();
        merged.extend(values.iter().map(|(k, v)| (k.clone(), v.clone())));

        template.render(&merged)
    }

    /// Render inline template.
    pub fn render_inline(&self, content: &str, values: &HashMap<String, String>) -> Result<String> {
        let template = Template::new("inline", content);

        let mut merged = self.globals.clone();
        merged.extend(values.iter().map(|(k, v)| (k.clone(), v.clone())));

        template.render(&merged)
    }
}

impl Default for TemplateEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Prompt template for LLM interactions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    /// System message template.
    pub system: Option<String>,
    /// User message template.
    pub user: String,
    /// Variables.
    pub variables: Vec<TemplateVariable>,
    /// Model hints.
    pub model_hints: Option<ModelHints>,
}

impl PromptTemplate {
    /// Create a new prompt template.
    pub fn new(user: impl Into<String>) -> Self {
        let user = user.into();
        let variables = Template::extract_variables(&user);

        Self {
            system: None,
            user,
            variables,
            model_hints: None,
        }
    }

    /// Set system message.
    pub fn system(mut self, msg: impl Into<String>) -> Self {
        let msg = msg.into();
        let sys_vars = Template::extract_variables(&msg);
        self.system = Some(msg);

        // Add system variables
        for var in sys_vars {
            if !self.variables.iter().any(|v| v.name == var.name) {
                self.variables.push(var);
            }
        }

        self
    }

    /// Set model hints.
    pub fn hints(mut self, hints: ModelHints) -> Self {
        self.model_hints = Some(hints);
        self
    }

    /// Render to messages.
    pub fn render(&self, values: &HashMap<String, String>) -> Result<RenderedPrompt> {
        let user_template = Template::new("user", &self.user);
        let user = user_template.render(values)?;

        let system = if let Some(ref sys) = self.system {
            let sys_template = Template::new("system", sys);
            Some(sys_template.render(values)?)
        } else {
            None
        };

        Ok(RenderedPrompt { system, user })
    }
}

/// Model hints for prompt templates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelHints {
    /// Suggested temperature.
    pub temperature: Option<f32>,
    /// Suggested max tokens.
    pub max_tokens: Option<u32>,
    /// Suggested model.
    pub model: Option<String>,
}

impl ModelHints {
    /// Create new hints.
    pub fn new() -> Self {
        Self {
            temperature: None,
            max_tokens: None,
            model: None,
        }
    }

    /// Set temperature.
    pub fn temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    /// Set max tokens.
    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Set model.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
}

impl Default for ModelHints {
    fn default() -> Self {
        Self::new()
    }
}

/// Rendered prompt.
#[derive(Debug, Clone, Serialize)]
pub struct RenderedPrompt {
    /// System message.
    pub system: Option<String>,
    /// User message.
    pub user: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_variable() {
        let var = TemplateVariable::new("name")
            .description("User name")
            .default_value("World");

        assert_eq!(var.name, "name");
        assert!(!var.required);
        assert_eq!(var.default, Some("World".to_string()));
    }

    #[test]
    fn test_template_extraction() {
        let template = Template::new("test", "Hello {{name}}! Today is {{day}}.");

        assert_eq!(template.variables.len(), 2);
        assert!(template.variables.iter().any(|v| v.name == "name"));
        assert!(template.variables.iter().any(|v| v.name == "day"));
    }

    #[test]
    fn test_template_render() {
        let template = Template::new("greeting", "Hello {{name}}!")
            .variable(TemplateVariable::new("name").default_value("World"));

        let mut values = HashMap::new();
        values.insert("name".to_string(), "Alice".to_string());

        let result = template.render(&values).unwrap();
        assert_eq!(result, "Hello Alice!");

        // Test default
        let result = template.render(&HashMap::new()).unwrap();
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_template_missing_required() {
        let template = Template::new("test", "Hello {{name}}!");

        let result = template.render(&HashMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn test_template_engine() {
        let mut engine = TemplateEngine::new();
        engine.set_global("app", "Cortex");

        let template = Template::new("welcome", "Welcome to {{app}}!");
        engine.register(template);

        let result = engine.render("welcome", &HashMap::new()).unwrap();
        assert_eq!(result, "Welcome to Cortex!");
    }

    #[test]
    fn test_prompt_template() {
        let prompt =
            PromptTemplate::new("Summarize: {{text}}").system("You are a helpful assistant.");

        let mut values = HashMap::new();
        values.insert("text".to_string(), "Hello world".to_string());

        let rendered = prompt.render(&values).unwrap();
        assert_eq!(rendered.user, "Summarize: Hello world");
        assert_eq!(
            rendered.system,
            Some("You are a helpful assistant.".to_string())
        );
    }

    #[test]
    fn test_categories_and_tags() {
        let mut engine = TemplateEngine::new();

        engine.register(Template::new("t1", "").category("coding").tag("python"));
        engine.register(Template::new("t2", "").category("coding").tag("rust"));
        engine.register(Template::new("t3", "").category("writing"));

        assert_eq!(engine.by_category("coding").len(), 2);
        assert_eq!(engine.by_tag("python").len(), 1);
    }
}
