use std::collections::HashMap;
use std::fs;
use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SystemPromptError {
    #[error("Failed to read system prompt file: {0}")]
    ReadError(#[from] std::io::Error),
    #[error("System prompt file not found: {0}")]
    NotFound(String),
    #[error("Template variable error: {0}")]
    TemplateError(String),
}

pub type Result<T> = std::result::Result<T, SystemPromptError>;

const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../../../../cortex_prompt.txt");

pub fn load_system_prompt(path: Option<&Path>) -> Result<String> {
    match path {
        Some(p) => {
            if !p.exists() {
                return Err(SystemPromptError::NotFound(p.display().to_string()));
            }
            let content = fs::read_to_string(p)?;
            Ok(content)
        }
        None => Ok(DEFAULT_SYSTEM_PROMPT.to_string()),
    }
}

pub fn replace_variables(template: &str, variables: &HashMap<String, String>) -> Result<String> {
    let mut result = template.to_string();

    for (key, value) in variables {
        let placeholder = format!("{{{{{}}}}}", key);
        if result.contains(&placeholder) {
            result = result.replace(&placeholder, value);
        }
    }

    let remaining_placeholders: Vec<&str> = result
        .match_indices("{{")
        .filter_map(|(idx, _)| {
            result[idx..]
                .find("}}")
                .map(|end| &result[idx..idx + end + 2])
        })
        .collect();

    if !remaining_placeholders.is_empty() {
        return Err(SystemPromptError::TemplateError(format!(
            "Unresolved template variables: {:?}",
            remaining_placeholders
        )));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_default_system_prompt() {
        let prompt = load_system_prompt(None).unwrap();
        assert!(prompt.contains("CORTEX"));
        assert!(prompt.contains("PRIME DIRECTIVES"));
    }

    #[test]
    fn test_replace_variables() {
        let template = "Hello {{name}}, your role is {{role}}.";
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "Cortex".to_string());
        vars.insert("role".to_string(), "AI Assistant".to_string());

        let result = replace_variables(template, &vars).unwrap();
        assert_eq!(result, "Hello Cortex, your role is AI Assistant.");
    }

    #[test]
    fn test_replace_variables_missing() {
        let template = "Hello {{name}}, your role is {{role}}.";
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "Cortex".to_string());

        let result = replace_variables(template, &vars);
        assert!(result.is_err());
    }

    #[test]
    fn test_replace_variables_no_placeholders() {
        let template = "Hello world";
        let vars = HashMap::new();

        let result = replace_variables(template, &vars).unwrap();
        assert_eq!(result, "Hello world");
    }
}
