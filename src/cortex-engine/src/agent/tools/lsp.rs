use crate::error::Result;
use crate::integrations::LspIntegration;
use crate::tools::context::ToolContext;
use crate::tools::handlers::ToolHandler;
use crate::tools::spec::ToolResult;
use async_trait::async_trait;
use cortex_lsp::DiagnosticSeverity;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Tool for querying LSP diagnostics across the project.
pub struct LspDiagnosticsTool {
    lsp: Option<Arc<LspIntegration>>,
}

impl LspDiagnosticsTool {
    /// Create a new LSP diagnostics tool.
    pub fn new(lsp: Arc<LspIntegration>) -> Self {
        Self { lsp: Some(lsp) }
    }

    /// Create a new LSP diagnostics tool for use as a handler.
    pub fn new_handler() -> Self {
        Self { lsp: None }
    }

    /// Run the tool to get project-wide diagnostics.
    pub async fn run_with_lsp(&self, lsp: &LspIntegration) -> Result<ToolResult> {
        if !lsp.is_running().await {
            return Ok(ToolResult::error(
                "LSP is not running or not enabled. Project-wide diagnostics are unavailable.",
            ));
        }

        let all_diags = lsp.get_all_diagnostics().await;

        if all_diags.is_empty() {
            return Ok(ToolResult::success(
                "✓ No diagnostics found. The project is clean.",
            ));
        }

        let mut output = String::from("LSP Diagnostics:\n\n");
        let mut total_errors = 0;
        let mut total_warnings = 0;
        let mut file_count = 0;

        // Sort files by path for consistent output
        let mut paths: Vec<_> = all_diags.keys().collect();
        paths.sort();

        for path in paths {
            let diags = &all_diags[path];
            if diags.is_empty() {
                continue;
            }

            file_count += 1;
            output.push_str(&format!("File: {}\n", path.display()));

            for diag in diags {
                match diag.severity {
                    DiagnosticSeverity::Error => total_errors += 1,
                    DiagnosticSeverity::Warning => total_warnings += 1,
                    _ => {}
                }
                output.push_str(&format!("  {}\n", diag.format()));
            }
            output.push_str("\n");
        }

        if total_errors == 0 && total_warnings == 0 {
            return Ok(ToolResult::success("✓ No errors or warnings found."));
        }

        output.push_str(&format!(
            "Summary: {} file(s) with diagnostics, {} error(s), {} warning(s)",
            file_count, total_errors, total_warnings
        ));

        Ok(ToolResult::success(output))
    }
}

#[async_trait]
impl ToolHandler for LspDiagnosticsTool {
    fn name(&self) -> &str {
        "LspDiagnostics"
    }

    async fn execute(&self, _arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        if let Some(lsp) = &context.lsp {
            self.run_with_lsp(lsp).await
        } else if let Some(lsp) = &self.lsp {
            self.run_with_lsp(lsp).await
        } else {
            Ok(ToolResult::error(
                "LSP integration is not available in the current context.",
            ))
        }
    }
}

/// Tool for querying type information or documentation for a symbol.
pub struct LspHoverTool;

#[derive(Debug, Deserialize)]
struct HoverArgs {
    file: String,
    line: u32,
    column: u32,
}

impl LspHoverTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LspHoverTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for LspHoverTool {
    fn name(&self) -> &str {
        "LspHover"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        let args: HoverArgs = match serde_json::from_value(arguments) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Invalid arguments: {e}"))),
        };

        let path = context.resolve_path(&args.file);
        let path_str = path.to_string_lossy();

        if let Some(lsp) = &context.lsp {
            match lsp
                .hover(
                    &path_str,
                    args.line.saturating_sub(1),
                    args.column.saturating_sub(1),
                )
                .await
            {
                Ok(Some(hover)) => Ok(ToolResult::success(hover)),
                Ok(None) => Ok(ToolResult::success(
                    "No hover information available at this position.",
                )),
                Err(e) => Ok(ToolResult::error(format!("LSP hover failed: {e}"))),
            }
        } else {
            Ok(ToolResult::error(
                "LSP integration is not available in the current context.",
            ))
        }
    }
}
