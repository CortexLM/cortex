//! Command execution with placeholder substitution.

use crate::command::{Command, hints, substitute_placeholders};

/// Context for command execution.
#[derive(Debug, Clone, Default)]
pub struct ExecutionContext {
    /// Arguments passed to the command.
    pub arguments: String,

    /// Override agent (takes precedence over command config).
    pub agent_override: Option<String>,

    /// Override model (takes precedence over command config).
    pub model_override: Option<String>,

    /// Override subtask flag.
    pub subtask_override: Option<bool>,
}

impl ExecutionContext {
    /// Create a new execution context with arguments.
    pub fn new(arguments: impl Into<String>) -> Self {
        Self {
            arguments: arguments.into(),
            ..Default::default()
        }
    }

    /// Set the arguments.
    pub fn with_arguments(mut self, arguments: impl Into<String>) -> Self {
        self.arguments = arguments.into();
        self
    }

    /// Set agent override.
    pub fn with_agent(mut self, agent: impl Into<String>) -> Self {
        self.agent_override = Some(agent.into());
        self
    }

    /// Set model override.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model_override = Some(model.into());
        self
    }

    /// Set subtask override.
    pub fn with_subtask(mut self, subtask: bool) -> Self {
        self.subtask_override = Some(subtask);
        self
    }
}

/// Result of executing a command.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// The command that was executed.
    pub command_name: String,

    /// The expanded prompt after placeholder substitution.
    pub prompt: String,

    /// The agent to use (from command config or override).
    pub agent: Option<String>,

    /// The model to use (from command config or override).
    pub model: Option<String>,

    /// Whether to run as subtask.
    pub subtask: bool,
}

impl ExecutionResult {
    /// Check if this result specifies an agent.
    pub fn has_agent(&self) -> bool {
        self.agent.is_some()
    }

    /// Check if this result specifies a model.
    pub fn has_model(&self) -> bool {
        self.model.is_some()
    }
}

/// Executor for custom commands.
#[derive(Debug, Clone, Default)]
pub struct Executor;

impl Executor {
    /// Create a new executor.
    pub fn new() -> Self {
        Self
    }

    /// Execute a command with the given context.
    pub fn execute(&self, command: &Command, context: &ExecutionContext) -> ExecutionResult {
        // Substitute placeholders
        let prompt = substitute_placeholders(&command.template, &context.arguments);

        // Resolve agent (override > command config)
        let agent = context
            .agent_override
            .clone()
            .or_else(|| command.config.agent.clone());

        // Resolve model (override > command config)
        let model = context
            .model_override
            .clone()
            .or_else(|| command.config.model.clone());

        // Resolve subtask flag (override > command config > default false)
        let subtask = context
            .subtask_override
            .or(command.config.subtask)
            .unwrap_or(false);

        ExecutionResult {
            command_name: command.name.clone(),
            prompt,
            agent,
            model,
            subtask,
        }
    }

    /// Execute a command with simple arguments string.
    pub fn execute_simple(&self, command: &Command, arguments: &str) -> ExecutionResult {
        let context = ExecutionContext::new(arguments);
        self.execute(command, &context)
    }

    /// Get the expected placeholders for a command.
    pub fn get_hints(&self, command: &Command) -> Vec<String> {
        hints(&command.template)
    }

    /// Preview the substitution without full execution.
    pub fn preview(&self, command: &Command, arguments: &str) -> String {
        substitute_placeholders(&command.template, arguments)
    }

    /// Validate that required arguments are provided.
    ///
    /// Returns a list of missing placeholders.
    pub fn validate_arguments(&self, command: &Command, arguments: &str) -> Vec<String> {
        let hints = self.get_hints(command);
        let args: Vec<&str> = arguments.split_whitespace().collect();
        let mut missing = Vec::new();

        for hint in hints {
            if hint == "$ARGUMENTS" {
                // $ARGUMENTS is always satisfied (can be empty)
                continue;
            }

            // Parse the number from $N
            if let Some(n) = hint.strip_prefix('$').and_then(|s| s.parse::<usize>().ok())
                && n > args.len()
            {
                missing.push(hint);
            }
        }

        missing
    }
}

/// Parse a command invocation string.
///
/// Format: `/command_name arguments...`
///
/// Returns (command_name, arguments) or None if not a valid invocation.
pub fn parse_invocation(input: &str) -> Option<(&str, &str)> {
    let input = input.trim();

    if !input.starts_with('/') {
        return None;
    }

    let without_slash = &input[1..];
    let mut parts = without_slash.splitn(2, char::is_whitespace);

    let command_name = parts.next()?;
    let arguments = parts.next().unwrap_or("").trim();

    Some((command_name, arguments))
}

/// Format a command for display.
pub fn format_command(name: &str, hints: &[String]) -> String {
    if hints.is_empty() {
        format!("/{name}")
    } else {
        let hint_str = hints.join(" ");
        format!("/{name} {hint_str}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, CommandConfig};
    use std::path::PathBuf;

    fn make_command(name: &str, template: &str) -> Command {
        Command::new(
            name,
            CommandConfig::default(),
            template,
            PathBuf::from("/test/cmd.md"),
        )
    }

    #[test]
    fn test_execute_simple() {
        let cmd = make_command("echo", "Echo: $ARGUMENTS");
        let executor = Executor::new();

        let result = executor.execute_simple(&cmd, "hello world");

        assert_eq!(result.command_name, "echo");
        assert_eq!(result.prompt, "Echo: hello world");
        assert!(!result.subtask);
    }

    #[test]
    fn test_execute_with_config() {
        let cmd = Command::new(
            "build",
            CommandConfig {
                agent: Some("build-agent".to_string()),
                model: Some("gpt-4".to_string()),
                subtask: Some(true),
                ..Default::default()
            },
            "Build $1",
            PathBuf::from("/test/build.md"),
        );

        let executor = Executor::new();
        let result = executor.execute_simple(&cmd, "project");

        assert_eq!(result.agent, Some("build-agent".to_string()));
        assert_eq!(result.model, Some("gpt-4".to_string()));
        assert!(result.subtask);
        assert_eq!(result.prompt, "Build project");
    }

    #[test]
    fn test_execute_with_overrides() {
        let cmd = Command::new(
            "test",
            CommandConfig {
                agent: Some("original-agent".to_string()),
                model: Some("original-model".to_string()),
                subtask: Some(false),
                ..Default::default()
            },
            "Test $ARGUMENTS",
            PathBuf::from("/test/test.md"),
        );

        let executor = Executor::new();
        let context = ExecutionContext::new("args")
            .with_agent("override-agent")
            .with_model("override-model")
            .with_subtask(true);

        let result = executor.execute(&cmd, &context);

        assert_eq!(result.agent, Some("override-agent".to_string()));
        assert_eq!(result.model, Some("override-model".to_string()));
        assert!(result.subtask);
    }

    #[test]
    fn test_get_hints() {
        let cmd = make_command("cmd", "First: $1, Second: $2, All: $ARGUMENTS");
        let executor = Executor::new();

        let hints = executor.get_hints(&cmd);

        assert!(hints.contains(&"$ARGUMENTS".to_string()));
        assert!(hints.contains(&"$1".to_string()));
        assert!(hints.contains(&"$2".to_string()));
    }

    #[test]
    fn test_validate_arguments() {
        let cmd = make_command("cmd", "A: $1, B: $2, C: $3");
        let executor = Executor::new();

        let missing = executor.validate_arguments(&cmd, "one two");
        assert_eq!(missing, vec!["$3"]);

        let missing = executor.validate_arguments(&cmd, "one two three");
        assert!(missing.is_empty());
    }

    #[test]
    fn test_parse_invocation() {
        let (name, args) = parse_invocation("/build --release").unwrap();
        assert_eq!(name, "build");
        assert_eq!(args, "--release");

        let (name, args) = parse_invocation("/test").unwrap();
        assert_eq!(name, "test");
        assert_eq!(args, "");

        assert!(parse_invocation("not a command").is_none());
    }

    #[test]
    fn test_format_command() {
        assert_eq!(format_command("help", &[]), "/help");
        assert_eq!(
            format_command("build", &["$1".to_string(), "$2".to_string()]),
            "/build $1 $2"
        );
    }

    #[test]
    fn test_preview() {
        let cmd = make_command("echo", "Message: $ARGUMENTS");
        let executor = Executor::new();

        let preview = executor.preview(&cmd, "hello");
        assert_eq!(preview, "Message: hello");
    }
}
