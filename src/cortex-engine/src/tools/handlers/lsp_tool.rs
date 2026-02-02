//! LSP Tool - Full Language Server Protocol operations.
//!
//! Provides code navigation and analysis via LSP:
//! - Go to definition
//! - Find references  
//! - Hover information
//! - Document symbols
//! - Workspace symbols
//! - Go to implementation
//! - Call hierarchy

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::Result;
use crate::integrations::LspIntegration;
use crate::tools::{ToolContext, ToolDefinition, ToolResult};

/// LSP operations available.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LspOperation {
    /// Go to definition of symbol.
    GoToDefinition,
    /// Find all references to symbol.
    FindReferences,
    /// Get hover information.
    Hover,
    /// Get document symbols (outline).
    DocumentSymbol,
    /// Search workspace symbols.
    WorkspaceSymbol,
    /// Go to implementation.
    GoToImplementation,
    /// Prepare call hierarchy.
    PrepareCallHierarchy,
    /// Get incoming calls.
    IncomingCalls,
    /// Get outgoing calls.
    OutgoingCalls,
    /// Get diagnostics for file.
    Diagnostics,
    /// Get completions at position.
    Completions,
    /// Get signature help.
    SignatureHelp,
    /// Rename symbol.
    Rename,
    /// Get code actions.
    CodeActions,
}

impl LspOperation {
    /// Get all available operations.
    pub fn all() -> Vec<Self> {
        vec![
            Self::GoToDefinition,
            Self::FindReferences,
            Self::Hover,
            Self::DocumentSymbol,
            Self::WorkspaceSymbol,
            Self::GoToImplementation,
            Self::PrepareCallHierarchy,
            Self::IncomingCalls,
            Self::OutgoingCalls,
            Self::Diagnostics,
            Self::Completions,
            Self::SignatureHelp,
            Self::Rename,
            Self::CodeActions,
        ]
    }

    /// Get operation name as string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GoToDefinition => "goToDefinition",
            Self::FindReferences => "findReferences",
            Self::Hover => "hover",
            Self::DocumentSymbol => "documentSymbol",
            Self::WorkspaceSymbol => "workspaceSymbol",
            Self::GoToImplementation => "goToImplementation",
            Self::PrepareCallHierarchy => "prepareCallHierarchy",
            Self::IncomingCalls => "incomingCalls",
            Self::OutgoingCalls => "outgoingCalls",
            Self::Diagnostics => "diagnostics",
            Self::Completions => "completions",
            Self::SignatureHelp => "signatureHelp",
            Self::Rename => "rename",
            Self::CodeActions => "codeActions",
        }
    }
}

/// Parameters for the LSP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspParams {
    /// The LSP operation to perform.
    pub operation: String,
    /// The file path (absolute or relative).
    #[serde(rename = "filePath")]
    pub file_path: String,
    /// Line number (1-based).
    #[serde(default = "default_line")]
    pub line: u32,
    /// Character offset (1-based).
    #[serde(default = "default_character")]
    pub character: u32,
    /// Query string for workspace symbol search.
    #[serde(default)]
    pub query: Option<String>,
    /// New name for rename operation.
    #[serde(rename = "newName")]
    pub new_name: Option<String>,
}

fn default_line() -> u32 {
    1
}
fn default_character() -> u32 {
    1
}

/// Get the LSP tool definition.
pub fn lsp_tool_definition() -> ToolDefinition {
    ToolDefinition::new(
        "lsp",
        "Perform Language Server Protocol operations for code navigation and analysis. \
         Use this to understand code structure, find definitions, references, and more. \
         Operations: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol, \
         goToImplementation, prepareCallHierarchy, incomingCalls, outgoingCalls, diagnostics, \
         completions, signatureHelp, rename, codeActions.",
        json!({
            "type": "object",
            "required": ["operation", "filePath"],
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": [
                        "goToDefinition",
                        "findReferences",
                        "hover",
                        "documentSymbol",
                        "workspaceSymbol",
                        "goToImplementation",
                        "prepareCallHierarchy",
                        "incomingCalls",
                        "outgoingCalls",
                        "diagnostics",
                        "completions",
                        "signatureHelp",
                        "rename",
                        "codeActions"
                    ],
                    "description": "The LSP operation to perform"
                },
                "filePath": {
                    "type": "string",
                    "description": "The absolute or relative path to the file"
                },
                "line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Line number (1-based, as shown in editors)"
                },
                "character": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Character offset (1-based, as shown in editors)"
                },
                "query": {
                    "type": "string",
                    "description": "Query string for workspaceSymbol search"
                },
                "newName": {
                    "type": "string",
                    "description": "New name for rename operation"
                }
            }
        }),
    )
}

/// Execute the LSP tool.
pub async fn execute_lsp(
    params: LspParams,
    context: &ToolContext,
    lsp: Option<Arc<LspIntegration>>,
) -> Result<ToolResult> {
    let lsp = match lsp {
        Some(l) => l,
        None => {
            return Ok(ToolResult::error(
                "LSP integration not available. Language servers may not be configured.",
            ));
        }
    };

    // Resolve file path
    let file_path = if Path::new(&params.file_path).is_absolute() {
        PathBuf::from(&params.file_path)
    } else {
        context.cwd.join(&params.file_path)
    };

    // Check file exists
    if !file_path.exists() {
        return Ok(ToolResult::error(format!(
            "File not found: {}",
            file_path.display()
        )));
    }

    let file_path_str = file_path.to_string_lossy().to_string();

    // Convert to 0-based indices for LSP
    let line = params.line.saturating_sub(1);
    let character = params.character.saturating_sub(1);

    let rel_path = file_path
        .strip_prefix(&context.cwd)
        .unwrap_or(&file_path)
        .display()
        .to_string();

    let _title = format!(
        "{} {}:{}:{}",
        params.operation, rel_path, params.line, params.character
    );

    // Execute the operation
    let result: Value = match params.operation.as_str() {
        "goToDefinition" => match lsp.go_to_definition(&file_path_str, line, character).await {
            Ok(locations) => json!(locations),
            Err(e) => return Ok(ToolResult::error(format!("goToDefinition failed: {}", e))),
        },
        "findReferences" => match lsp.find_references(&file_path_str, line, character).await {
            Ok(locations) => json!(locations),
            Err(e) => return Ok(ToolResult::error(format!("findReferences failed: {}", e))),
        },
        "hover" => match lsp.hover(&file_path_str, line, character).await {
            Ok(hover) => json!(hover),
            Err(e) => return Ok(ToolResult::error(format!("hover failed: {}", e))),
        },
        "documentSymbol" => match lsp.document_symbols(&file_path_str).await {
            Ok(symbols) => json!(symbols),
            Err(e) => return Ok(ToolResult::error(format!("documentSymbol failed: {}", e))),
        },
        "workspaceSymbol" => {
            let query = params.query.as_deref().unwrap_or("");
            match lsp.workspace_symbols(query).await {
                Ok(symbols) => json!(symbols),
                Err(e) => return Ok(ToolResult::error(format!("workspaceSymbol failed: {}", e))),
            }
        }
        "goToImplementation" => {
            match lsp
                .go_to_implementation(&file_path_str, line, character)
                .await
            {
                Ok(locations) => json!(locations),
                Err(e) => {
                    return Ok(ToolResult::error(format!(
                        "goToImplementation failed: {}",
                        e
                    )));
                }
            }
        }
        "prepareCallHierarchy" => {
            match lsp
                .prepare_call_hierarchy(&file_path_str, line, character)
                .await
            {
                Ok(items) => json!(items),
                Err(e) => {
                    return Ok(ToolResult::error(format!(
                        "prepareCallHierarchy failed: {}",
                        e
                    )));
                }
            }
        }
        "incomingCalls" => match lsp.incoming_calls(&file_path_str, line, character).await {
            Ok(calls) => json!(calls),
            Err(e) => return Ok(ToolResult::error(format!("incomingCalls failed: {}", e))),
        },
        "outgoingCalls" => match lsp.outgoing_calls(&file_path_str, line, character).await {
            Ok(calls) => json!(calls),
            Err(e) => return Ok(ToolResult::error(format!("outgoingCalls failed: {}", e))),
        },
        "diagnostics" => {
            let diags = lsp.get_diagnostics(&file_path).await;
            json!(diags)
        }
        "completions" => match lsp.completions(&file_path_str, line, character).await {
            Ok(items) => json!(items),
            Err(e) => return Ok(ToolResult::error(format!("completions failed: {}", e))),
        },
        "signatureHelp" => match lsp.signature_help(&file_path_str, line, character).await {
            Ok(help) => json!(help),
            Err(e) => return Ok(ToolResult::error(format!("signatureHelp failed: {}", e))),
        },
        "rename" => {
            let new_name = match params.new_name {
                Some(name) => name,
                None => return Ok(ToolResult::error("rename requires 'newName' parameter")),
            };
            match lsp.rename(&file_path_str, line, character, &new_name).await {
                Ok(edits) => json!(edits),
                Err(e) => return Ok(ToolResult::error(format!("rename failed: {}", e))),
            }
        }
        "codeActions" => match lsp.code_actions(&file_path_str, line, character).await {
            Ok(actions) => json!(actions),
            Err(e) => return Ok(ToolResult::error(format!("codeActions failed: {}", e))),
        },
        _ => {
            return Ok(ToolResult::error(format!(
                "Unknown LSP operation: {}. Available: goToDefinition, findReferences, hover, \
                 documentSymbol, workspaceSymbol, goToImplementation, prepareCallHierarchy, \
                 incomingCalls, outgoingCalls, diagnostics, completions, signatureHelp, rename, codeActions",
                params.operation
            )));
        }
    };

    // Format output
    let output = if result.is_array() && result.as_array().map(|a| a.is_empty()).unwrap_or(false) {
        format!("No results found for {}", params.operation)
    } else {
        serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string())
    };

    Ok(ToolResult::success(output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lsp_operations() {
        let ops = LspOperation::all();
        assert!(ops.len() >= 10);
    }

    #[test]
    fn test_lsp_tool_definition() {
        let def = lsp_tool_definition();
        assert_eq!(def.name, "lsp");
        assert!(def.description.contains("Language Server Protocol"));
    }
}
