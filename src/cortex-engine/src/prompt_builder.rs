//! Prompt builder utilities.
//!
//! Provides a fluent API for constructing prompts with
//! system messages, context, examples, and formatting.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Role {
    /// System message.
    System,
    /// User message.
    #[default]
    User,
    /// Assistant message.
    Assistant,
    /// Tool result.
    Tool,
}

impl Role {
    /// Get role name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// Message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Role.
    pub role: Role,
    /// Content.
    pub content: String,
    /// Name (optional).
    pub name: Option<String>,
    /// Tool call ID (for tool results).
    pub tool_call_id: Option<String>,
}

impl Message {
    /// Create a new message.
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            name: None,
            tool_call_id: None,
        }
    }

    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, content)
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(Role::Assistant, content)
    }

    /// Create a tool result message.
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            name: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    /// Set name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// Prompt section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    /// Section title.
    pub title: String,
    /// Section content.
    pub content: String,
    /// Priority (higher = more important).
    pub priority: i32,
}

impl Section {
    /// Create a new section.
    pub fn new(title: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            content: content.into(),
            priority: 0,
        }
    }

    /// Set priority.
    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Format section.
    pub fn format(&self) -> String {
        format!("## {}\n\n{}", self.title, self.content)
    }
}

/// Few-shot example.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    /// Input.
    pub input: String,
    /// Output.
    pub output: String,
    /// Description.
    pub description: Option<String>,
}

impl Example {
    /// Create a new example.
    pub fn new(input: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
            description: None,
        }
    }

    /// Set description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Format as conversation.
    pub fn format_conversation(&self) -> Vec<Message> {
        vec![Message::user(&self.input), Message::assistant(&self.output)]
    }

    /// Format as text.
    pub fn format_text(&self) -> String {
        let mut s = String::new();
        if let Some(ref desc) = self.description {
            s.push_str(&format!("// {desc}\n"));
        }
        s.push_str(&format!("Input: {}\nOutput: {}", self.input, self.output));
        s
    }
}

/// Prompt builder.
#[derive(Debug, Clone, Default)]
pub struct PromptBuilder {
    /// Base system message.
    system_base: Option<String>,
    /// System sections.
    sections: Vec<Section>,
    /// Examples.
    examples: Vec<Example>,
    /// Context items.
    context: Vec<String>,
    /// Conversation history.
    history: Vec<Message>,
    /// Current user message.
    user_message: Option<String>,
    /// Variables.
    variables: HashMap<String, String>,
    /// Maximum tokens estimate.
    max_tokens: Option<u32>,
}

impl PromptBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set base system message.
    pub fn system(mut self, content: impl Into<String>) -> Self {
        self.system_base = Some(content.into());
        self
    }

    /// Add a section.
    pub fn section(mut self, section: Section) -> Self {
        self.sections.push(section);
        self
    }

    /// Add a section with title and content.
    pub fn add_section(self, title: impl Into<String>, content: impl Into<String>) -> Self {
        self.section(Section::new(title, content))
    }

    /// Add an example.
    pub fn example(mut self, example: Example) -> Self {
        self.examples.push(example);
        self
    }

    /// Add an example with input/output.
    pub fn add_example(self, input: impl Into<String>, output: impl Into<String>) -> Self {
        self.example(Example::new(input, output))
    }

    /// Add context.
    pub fn context(mut self, ctx: impl Into<String>) -> Self {
        self.context.push(ctx.into());
        self
    }

    /// Add file context.
    pub fn file_context(self, path: &str, content: &str) -> Self {
        self.context(format!("File: {path}\n```\n{content}\n```"))
    }

    /// Add conversation history.
    pub fn history(mut self, messages: Vec<Message>) -> Self {
        self.history = messages;
        self
    }

    /// Add a single history message.
    pub fn add_history(mut self, message: Message) -> Self {
        self.history.push(message);
        self
    }

    /// Set user message.
    pub fn user_message(mut self, message: impl Into<String>) -> Self {
        self.user_message = Some(message.into());
        self
    }

    /// Set variable.
    pub fn var(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.variables.insert(name.into(), value.into());
        self
    }

    /// Set max tokens.
    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Build system message.
    pub fn build_system(&self) -> Option<String> {
        let mut parts = Vec::new();

        // Base system message
        if let Some(ref base) = self.system_base {
            parts.push(self.substitute_vars(base));
        }

        // Sections (sorted by priority)
        let mut sections = self.sections.clone();
        sections.sort_by(|a, b| b.priority.cmp(&a.priority));
        for section in sections {
            parts.push(self.substitute_vars(&section.format()));
        }

        // Context
        if !self.context.is_empty() {
            parts.push("## Context\n".to_string());
            for ctx in &self.context {
                parts.push(self.substitute_vars(ctx));
            }
        }

        // Examples
        if !self.examples.is_empty() {
            parts.push("## Examples\n".to_string());
            for (i, example) in self.examples.iter().enumerate() {
                parts.push(format!("### Example {}\n{}", i + 1, example.format_text()));
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }

    /// Build messages.
    pub fn build(&self) -> Vec<Message> {
        let mut messages = Vec::new();

        // System message
        if let Some(system) = self.build_system() {
            messages.push(Message::system(system));
        }

        // Examples as conversation
        for example in &self.examples {
            messages.extend(example.format_conversation());
        }

        // History
        messages.extend(self.history.iter().cloned());

        // User message
        if let Some(ref user) = self.user_message {
            messages.push(Message::user(self.substitute_vars(user)));
        }

        messages
    }

    /// Substitute variables in text.
    fn substitute_vars(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (name, value) in &self.variables {
            let pattern = format!("{{{{{name}}}}}");
            result = result.replace(&pattern, value);
        }
        result
    }

    /// Estimate token count.
    pub fn estimate_tokens(&self) -> u32 {
        let messages = self.build();
        let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
        // Rough estimate: ~4 chars per token
        (total_chars as f32 / 4.0).ceil() as u32
    }

    /// Check if within token limit.
    pub fn within_limit(&self) -> bool {
        if let Some(max) = self.max_tokens {
            self.estimate_tokens() <= max
        } else {
            true
        }
    }
}

/// Instruction builder for structured prompts.
#[derive(Debug, Clone, Default)]
pub struct InstructionBuilder {
    /// Task description.
    task: Option<String>,
    /// Steps.
    steps: Vec<String>,
    /// Constraints.
    constraints: Vec<String>,
    /// Output format.
    output_format: Option<String>,
    /// Notes.
    notes: Vec<String>,
}

impl InstructionBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set task.
    pub fn task(mut self, task: impl Into<String>) -> Self {
        self.task = Some(task.into());
        self
    }

    /// Add a step.
    pub fn step(mut self, step: impl Into<String>) -> Self {
        self.steps.push(step.into());
        self
    }

    /// Add a constraint.
    pub fn constraint(mut self, constraint: impl Into<String>) -> Self {
        self.constraints.push(constraint.into());
        self
    }

    /// Set output format.
    pub fn output_format(mut self, format: impl Into<String>) -> Self {
        self.output_format = Some(format.into());
        self
    }

    /// Add a note.
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Build instruction text.
    pub fn build(&self) -> String {
        let mut parts = Vec::new();

        if let Some(ref task) = self.task {
            parts.push(format!("## Task\n{task}"));
        }

        if !self.steps.is_empty() {
            let steps: Vec<String> = self
                .steps
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{}. {}", i + 1, s))
                .collect();
            parts.push(format!("## Steps\n{}", steps.join("\n")));
        }

        if !self.constraints.is_empty() {
            let constraints: Vec<String> =
                self.constraints.iter().map(|c| format!("- {c}")).collect();
            parts.push(format!("## Constraints\n{}", constraints.join("\n")));
        }

        if let Some(ref format) = self.output_format {
            parts.push(format!("## Output Format\n{format}"));
        }

        if !self.notes.is_empty() {
            let notes: Vec<String> = self.notes.iter().map(|n| format!("- {n}")).collect();
            parts.push(format!("## Notes\n{}", notes.join("\n")));
        }

        parts.join("\n\n")
    }
}

/// Chain of thought builder.
#[derive(Debug, Clone, Default)]
pub struct ChainOfThoughtBuilder {
    /// Problem statement.
    problem: Option<String>,
    /// Reasoning prefix.
    reasoning_prefix: String,
    /// Answer prefix.
    answer_prefix: String,
    /// Examples with reasoning.
    examples: Vec<(String, String, String)>, // (problem, reasoning, answer)
}

impl ChainOfThoughtBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            problem: None,
            reasoning_prefix: "Let me think through this step by step:".to_string(),
            answer_prefix: "Therefore, the answer is:".to_string(),
            examples: Vec::new(),
        }
    }

    /// Set problem.
    pub fn problem(mut self, problem: impl Into<String>) -> Self {
        self.problem = Some(problem.into());
        self
    }

    /// Set reasoning prefix.
    pub fn reasoning_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.reasoning_prefix = prefix.into();
        self
    }

    /// Set answer prefix.
    pub fn answer_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.answer_prefix = prefix.into();
        self
    }

    /// Add example with reasoning.
    pub fn example(
        mut self,
        problem: impl Into<String>,
        reasoning: impl Into<String>,
        answer: impl Into<String>,
    ) -> Self {
        self.examples
            .push((problem.into(), reasoning.into(), answer.into()));
        self
    }

    /// Build prompt.
    pub fn build(&self) -> Vec<Message> {
        let mut messages = Vec::new();

        // System message
        let system = format!(
            "When solving problems, first think through the problem step by step, then provide the answer.\n\n\
            Use this format:\n\
            {}\n[Your reasoning]\n\n{}\n[Your answer]",
            self.reasoning_prefix, self.answer_prefix
        );
        messages.push(Message::system(system));

        // Examples
        for (problem, reasoning, answer) in &self.examples {
            messages.push(Message::user(problem));
            messages.push(Message::assistant(format!(
                "{}\n{}\n\n{}\n{}",
                self.reasoning_prefix, reasoning, self.answer_prefix, answer
            )));
        }

        // Current problem
        if let Some(ref problem) = self.problem {
            messages.push(Message::user(problem));
        }

        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message() {
        let msg = Message::system("You are helpful");
        assert_eq!(msg.role, Role::System);

        let msg = Message::user("Hello").name("Alice");
        assert_eq!(msg.name, Some("Alice".to_string()));
    }

    #[test]
    fn test_section() {
        let section = Section::new("Rules", "Follow these rules").priority(10);
        assert_eq!(section.priority, 10);
        assert!(section.format().contains("## Rules"));
    }

    #[test]
    fn test_example() {
        let example = Example::new("2+2", "4").description("Simple math");

        let messages = example.format_conversation();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].role, Role::Assistant);
    }

    #[test]
    fn test_prompt_builder() {
        let prompt = PromptBuilder::new()
            .system("You are a helpful assistant")
            .add_section("Task", "Help the user")
            .context("Today is Monday")
            .user_message("Hello!");

        let messages = prompt.build();
        assert!(messages.len() >= 2);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages.last().unwrap().role, Role::User);
    }

    #[test]
    fn test_prompt_variables() {
        let prompt = PromptBuilder::new()
            .system("You are {{role}}")
            .var("role", "a coding assistant")
            .user_message("Help me with {{task}}")
            .var("task", "debugging");

        let messages = prompt.build();
        assert!(messages[0].content.contains("a coding assistant"));
        assert!(messages[1].content.contains("debugging"));
    }

    #[test]
    fn test_instruction_builder() {
        let instruction = InstructionBuilder::new()
            .task("Analyze the code")
            .step("Read the code")
            .step("Identify issues")
            .constraint("Be concise")
            .output_format("Bullet points")
            .build();

        assert!(instruction.contains("## Task"));
        assert!(instruction.contains("## Steps"));
        assert!(instruction.contains("1. Read the code"));
    }

    #[test]
    fn test_chain_of_thought() {
        let cot = ChainOfThoughtBuilder::new()
            .example("What is 2+2?", "2+2 means adding 2 and 2", "4")
            .problem("What is 3+3?")
            .build();

        assert!(cot.len() >= 4);
        assert_eq!(cot.last().unwrap().content, "What is 3+3?");
    }

    #[test]
    fn test_token_estimation() {
        let prompt = PromptBuilder::new()
            .system("Hello world")
            .user_message("How are you?");

        let tokens = prompt.estimate_tokens();
        assert!(tokens > 0);
    }
}
