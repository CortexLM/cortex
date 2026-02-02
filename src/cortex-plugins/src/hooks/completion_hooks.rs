//! Command completion hooks for plugin-provided autocompletion.
//!
//! This module provides hooks that allow plugins to:
//! - Register custom completion providers
//! - Provide completions for custom commands
//! - Extend existing command completions
//! - Add context-aware suggestions

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::types::{HookPriority, HookResult};
use crate::Result;

// ============================================================================
// COMPLETION TYPES
// ============================================================================

/// Type of completion item
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    /// Command name
    Command,
    /// Command argument
    Argument,
    /// File path
    File,
    /// Directory path
    Directory,
    /// Model name
    Model,
    /// Variable/setting name
    Variable,
    /// Value for a setting
    Value,
    /// Keyword
    Keyword,
    /// Custom type
    Custom,
}

impl Default for CompletionKind {
    fn default() -> Self {
        Self::Custom
    }
}

/// A completion item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionItem {
    /// The completion text to insert
    pub text: String,
    /// Display label (if different from text)
    #[serde(default)]
    pub label: Option<String>,
    /// Short description
    #[serde(default)]
    pub description: Option<String>,
    /// Detailed documentation
    #[serde(default)]
    pub detail: Option<String>,
    /// Kind of completion
    #[serde(default)]
    pub kind: CompletionKind,
    /// Sort priority (lower = higher priority)
    #[serde(default = "default_sort_priority")]
    pub sort_priority: i32,
    /// Filter text (for fuzzy matching)
    #[serde(default)]
    pub filter_text: Option<String>,
    /// Insert text (if different from text, e.g., with placeholders)
    #[serde(default)]
    pub insert_text: Option<String>,
    /// Whether this completion is deprecated
    #[serde(default)]
    pub deprecated: bool,
    /// Associated command (for command completions)
    #[serde(default)]
    pub command: Option<String>,
    /// Additional data for the plugin
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

fn default_sort_priority() -> i32 {
    100
}

impl CompletionItem {
    /// Create a new completion item.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            label: None,
            description: None,
            detail: None,
            kind: CompletionKind::default(),
            sort_priority: default_sort_priority(),
            filter_text: None,
            insert_text: None,
            deprecated: false,
            command: None,
            data: None,
        }
    }

    /// Set the label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the kind.
    pub fn with_kind(mut self, kind: CompletionKind) -> Self {
        self.kind = kind;
        self
    }

    /// Set the sort priority.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.sort_priority = priority;
        self
    }
}

/// Completion context providing information about the completion request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionContext {
    /// The input text being completed
    pub input: String,
    /// Cursor position in the input
    pub cursor_position: usize,
    /// The word being completed (extracted from input)
    #[serde(default)]
    pub word: Option<String>,
    /// The command being completed (if any)
    #[serde(default)]
    pub command: Option<String>,
    /// Argument index (0-based, if completing command args)
    #[serde(default)]
    pub arg_index: Option<usize>,
    /// Previous arguments (if completing command args)
    #[serde(default)]
    pub previous_args: Vec<String>,
    /// Whether this is triggered manually (vs automatic)
    #[serde(default)]
    pub manual_trigger: bool,
    /// Trigger character (if any)
    #[serde(default)]
    pub trigger_character: Option<char>,
}

// ============================================================================
// COMPLETION PROVIDER REGISTRATION HOOK
// ============================================================================

/// Completion provider definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionProvider {
    /// Provider identifier
    pub id: String,
    /// Provider name (for display)
    pub name: String,
    /// Commands this provider handles (empty = all)
    #[serde(default)]
    pub commands: Vec<String>,
    /// Trigger characters that activate completion
    #[serde(default)]
    pub trigger_characters: Vec<char>,
    /// Priority (lower = higher priority)
    #[serde(default = "default_sort_priority")]
    pub priority: i32,
}

impl CompletionProvider {
    /// Create a new completion provider.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            commands: Vec::new(),
            trigger_characters: Vec::new(),
            priority: default_sort_priority(),
        }
    }

    /// Add commands this provider handles.
    pub fn for_commands(mut self, commands: Vec<String>) -> Self {
        self.commands = commands;
        self
    }

    /// Add trigger characters.
    pub fn with_triggers(mut self, chars: Vec<char>) -> Self {
        self.trigger_characters = chars;
        self
    }
}

/// Input for completion provider registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionProviderRegisterInput {
    /// Plugin ID registering the provider
    pub plugin_id: String,
    /// Provider definition
    pub provider: CompletionProvider,
}

/// Output for completion provider registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionProviderRegisterOutput {
    /// Whether registration succeeded
    pub success: bool,
    /// Provider ID assigned
    #[serde(default)]
    pub provider_id: Option<String>,
    /// Error if failed
    #[serde(default)]
    pub error: Option<String>,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl CompletionProviderRegisterOutput {
    /// Create a success output.
    pub fn success(provider_id: impl Into<String>) -> Self {
        Self {
            success: true,
            provider_id: Some(provider_id.into()),
            error: None,
            result: HookResult::Continue,
        }
    }

    /// Create an error output.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            provider_id: None,
            error: Some(message.into()),
            result: HookResult::Continue,
        }
    }
}

/// Handler for completion provider registration
#[async_trait]
pub trait CompletionProviderRegisterHook: Send + Sync {
    /// Get the hook priority.
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Execute the hook.
    async fn execute(
        &self,
        input: &CompletionProviderRegisterInput,
        output: &mut CompletionProviderRegisterOutput,
    ) -> Result<()>;
}

// ============================================================================
// COMPLETION REQUEST HOOK
// ============================================================================

/// Input for completion request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequestInput {
    /// Session ID
    pub session_id: String,
    /// Completion context
    pub context: CompletionContext,
    /// Maximum number of items to return
    #[serde(default = "default_max_items")]
    pub max_items: usize,
}

fn default_max_items() -> usize {
    50
}

/// Output for completion request
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompletionRequestOutput {
    /// Completion items
    #[serde(default)]
    pub items: Vec<CompletionItem>,
    /// Whether the list is incomplete (more items available)
    #[serde(default)]
    pub is_incomplete: bool,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl CompletionRequestOutput {
    /// Create a new empty completion output.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a completion item.
    pub fn add_item(&mut self, item: CompletionItem) {
        self.items.push(item);
    }

    /// Add multiple completion items.
    pub fn add_items(&mut self, items: impl IntoIterator<Item = CompletionItem>) {
        self.items.extend(items);
    }

    /// Mark the list as incomplete.
    pub fn set_incomplete(mut self, incomplete: bool) -> Self {
        self.is_incomplete = incomplete;
        self
    }
}

/// Handler for completion requests
#[async_trait]
pub trait CompletionRequestHook: Send + Sync {
    /// Get the hook priority.
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Commands this hook provides completions for (empty = all)
    fn commands(&self) -> Vec<String> {
        vec![]
    }

    /// Execute the hook.
    async fn execute(
        &self,
        input: &CompletionRequestInput,
        output: &mut CompletionRequestOutput,
    ) -> Result<()>;
}

// ============================================================================
// COMPLETION RESOLVE HOOK
// ============================================================================

/// Input for resolving completion item details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResolveInput {
    /// The completion item to resolve
    pub item: CompletionItem,
    /// Session ID
    pub session_id: String,
}

/// Output for completion resolution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResolveOutput {
    /// The resolved completion item with additional details
    pub item: CompletionItem,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl CompletionResolveOutput {
    /// Create a new resolve output.
    pub fn new(item: CompletionItem) -> Self {
        Self {
            item,
            result: HookResult::Continue,
        }
    }
}

/// Handler for resolving completion item details (lazy loading)
#[async_trait]
pub trait CompletionResolveHook: Send + Sync {
    /// Get the hook priority.
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Execute the hook.
    async fn execute(
        &self,
        input: &CompletionResolveInput,
        output: &mut CompletionResolveOutput,
    ) -> Result<()>;
}

// ============================================================================
// ARGUMENT COMPLETION HOOK
// ============================================================================

/// Argument definition for completion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArgumentDefinition {
    /// Argument name
    pub name: String,
    /// Argument description
    #[serde(default)]
    pub description: Option<String>,
    /// Whether required
    #[serde(default)]
    pub required: bool,
    /// Possible values (for enum-like args)
    #[serde(default)]
    pub values: Vec<String>,
    /// Value type hint
    #[serde(default)]
    pub value_type: Option<String>,
}

/// Input for argument completion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArgumentCompletionInput {
    /// Plugin ID
    pub plugin_id: String,
    /// Command name
    pub command: String,
    /// Argument index
    pub arg_index: usize,
    /// Current argument value (partial)
    pub current_value: String,
    /// Previous arguments
    pub previous_args: Vec<String>,
    /// Session ID
    pub session_id: String,
}

/// Output for argument completion
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArgumentCompletionOutput {
    /// Completion items for this argument
    #[serde(default)]
    pub items: Vec<CompletionItem>,
    /// Argument definition (for help/hints)
    #[serde(default)]
    pub argument_def: Option<ArgumentDefinition>,
    /// Hook result
    #[serde(default)]
    pub result: HookResult,
}

impl ArgumentCompletionOutput {
    /// Create a new empty argument completion output.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add completion items.
    pub fn add_items(&mut self, items: impl IntoIterator<Item = CompletionItem>) {
        self.items.extend(items);
    }

    /// Set argument definition.
    pub fn with_definition(mut self, def: ArgumentDefinition) -> Self {
        self.argument_def = Some(def);
        self
    }
}

/// Handler for argument-specific completions
#[async_trait]
pub trait ArgumentCompletionHook: Send + Sync {
    /// Get the hook priority.
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Commands this hook provides argument completions for
    fn commands(&self) -> Vec<String> {
        vec![]
    }

    /// Execute the hook.
    async fn execute(
        &self,
        input: &ArgumentCompletionInput,
        output: &mut ArgumentCompletionOutput,
    ) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_item() {
        let item = CompletionItem::new("test")
            .with_label("Test Item")
            .with_description("A test completion")
            .with_kind(CompletionKind::Command)
            .with_priority(10);

        assert_eq!(item.text, "test");
        assert_eq!(item.label, Some("Test Item".to_string()));
        assert_eq!(item.kind, CompletionKind::Command);
        assert_eq!(item.sort_priority, 10);
    }

    #[test]
    fn test_completion_provider() {
        let provider = CompletionProvider::new("my-provider", "My Provider")
            .for_commands(vec!["test".to_string(), "demo".to_string()])
            .with_triggers(vec!['/', '@']);

        assert_eq!(provider.id, "my-provider");
        assert_eq!(provider.commands.len(), 2);
        assert_eq!(provider.trigger_characters.len(), 2);
    }

    #[test]
    fn test_completion_request_output() {
        let mut output = CompletionRequestOutput::new();
        output.add_item(CompletionItem::new("item1"));
        output.add_items(vec![
            CompletionItem::new("item2"),
            CompletionItem::new("item3"),
        ]);

        assert_eq!(output.items.len(), 3);
    }

    #[test]
    fn test_completion_context() {
        let context = CompletionContext {
            input: "/model claude".to_string(),
            cursor_position: 13,
            word: Some("claude".to_string()),
            command: Some("model".to_string()),
            arg_index: Some(0),
            previous_args: vec![],
            manual_trigger: false,
            trigger_character: None,
        };

        assert_eq!(context.command, Some("model".to_string()));
        assert_eq!(context.arg_index, Some(0));
    }
}
