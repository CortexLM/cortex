//! Custom commands system for Cortex CLI.
//!
//! Custom commands are user-defined prompt templates that can be invoked
//! via slash commands (e.g., `/review`) or CLI arguments (`--command review`).
//!
//! # Command Sources
//!
//! Commands can be defined in three locations (in order of priority):
//! 1. **Project commands**: `.cortex/commands/*.md` - Shared with team
//! 2. **Personal commands**: `~/.cortex/commands/*.md` - Personal customizations
//! 3. **Config commands**: `[[commands]]` in `config.toml` - Quick inline definitions
//!
//! # File Format
//!
//! Command files use Markdown with YAML frontmatter:
//!
//! ```markdown
//! ---
//! name: review
//! description: Review code changes
//! agent: plan
//! model: claude-3-5-sonnet
//! subtask: false
//! aliases: [r, code-review]
//! category: Development
//! ---
//!
//! Please review the following code changes and provide feedback:
//!
//! {{input}}
//!
//! Focus on:
//! - Code quality
//! - Potential bugs
//! - Performance issues
//! ```
//!
//! # Template Variables
//!
//! - `{{input}}` - User input/arguments
//! - `{{file:path}}` - Content of a file
//! - `{{selection}}` - Selected text (from editor)
//! - `{{clipboard}}` - Clipboard content
//! - `{{cwd}}` - Current working directory
//! - `{{date}}`, `{{time}}`, `{{datetime}}` - Current date/time
//! - `{{env:VAR}}` - Environment variable
//!
//! # Config Format
//!
//! Quick inline definitions in `config.toml`:
//!
//! ```toml
//! [[commands]]
//! name = "quick-fix"
//! description = "Quick fix for common issues"
//! template = "Fix this issue: {{input}}"
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use cortex_engine::custom_command::{CustomCommandRegistry, TemplateContext};
//!
//! // Initialize registry
//! let registry = CustomCommandRegistry::new(&cortex_home, Some(&project_root));
//! registry.scan().await?;
//!
//! // Execute a command
//! let ctx = TemplateContext::new("the code to review");
//! if let Some(result) = registry.execute("review", &ctx).await {
//!     println!("Prompt: {}", result.prompt);
//! }
//! ```

mod loader;
mod registry;
mod template;
mod types;

pub use loader::{
    create_command_file, generate_command_md, load_command_file, personal_commands_dir,
    project_commands_dir, scan_directory,
};
pub use registry::{
    CustomCommandRegistry, global_registry, init_global_registry, try_global_registry,
};
pub use template::{
    TemplateContext, expand_template, has_variables, list_variables, validate_template,
};
pub use types::{
    CommandExecutionResult, CommandMetadata, CommandSource, CustomCommand, CustomCommandConfig,
};
