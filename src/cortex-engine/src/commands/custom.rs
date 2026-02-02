//! Dynamic custom command handler.
//!
//! This module provides the `DynamicCustomCommand` which wraps user-defined
//! custom commands and makes them executable as slash commands.

use async_trait::async_trait;

use crate::error::Result;

use super::types::{
    ArgType, CommandArg, CommandContext, CommandHandler, CommandInvocation, CommandMeta,
    CommandResult,
};

/// Dynamic custom command handler.
/// This wraps a CustomCommand and makes it executable as a slash command.
pub struct DynamicCustomCommand {
    command: crate::custom_command::CustomCommand,
}

impl DynamicCustomCommand {
    /// Create a new dynamic custom command handler.
    pub fn new(command: crate::custom_command::CustomCommand) -> Self {
        Self { command }
    }
}

#[async_trait]
impl CommandHandler for DynamicCustomCommand {
    async fn execute(
        &self,
        invocation: &CommandInvocation,
        _ctx: &CommandContext,
    ) -> Result<CommandResult> {
        // Get the user input (all arguments joined)
        let input = invocation.rest();

        // Create template context
        let template_ctx = crate::custom_command::TemplateContext::new(&input)
            .with_cwd(_ctx.cwd.to_string_lossy().to_string());

        // Expand the template
        let prompt = crate::custom_command::expand_template(&self.command.template, &template_ctx);

        // Return the result with action data
        Ok(CommandResult::with_data(serde_json::json!({
            "action": "execute_custom_command",
            "command_name": self.command.name,
            "prompt": prompt,
            "agent": self.command.agent,
            "model": self.command.model,
            "subtask": self.command.subtask
        })))
    }

    fn metadata(&self) -> &CommandMeta {
        // We need to leak the metadata to get a static reference
        // This is safe because custom commands are loaded once and live for the program duration
        let meta = Box::new(CommandMeta {
            name: self.command.name.clone(),
            aliases: self.command.aliases.clone(),
            description: self.command.description.clone(),
            help: Some(format!(
                "Custom command: {}\n\nTemplate:\n{}\n\nSource: {}",
                self.command.description, self.command.template, self.command.source
            )),
            args: vec![CommandArg {
                name: "input".to_string(),
                description: "Input to pass to the command template".to_string(),
                required: false,
                default: None,
                arg_type: ArgType::String,
            }],
            hidden: false,
            category: self.command.category.clone(),
        });
        Box::leak(meta)
    }
}
