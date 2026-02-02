//! LSP server capabilities caching.

use lsp_types::InitializeResult;

/// Server capabilities cached from initialization.
#[derive(Debug, Clone, Default)]
pub struct CachedServerCapabilities {
    /// Whether the server supports hover.
    pub hover: bool,
    /// Whether the server supports goto definition.
    pub goto_definition: bool,
    /// Whether the server supports find references.
    pub find_references: bool,
    /// Whether the server supports document symbols.
    pub document_symbol: bool,
    /// Whether the server supports workspace symbols.
    pub workspace_symbol: bool,
    /// Whether the server supports completion.
    pub completion: bool,
    /// Whether the server supports signature help.
    pub signature_help: bool,
    /// Whether the server supports rename.
    pub rename: bool,
    /// Whether the server supports formatting.
    pub formatting: bool,
    /// Whether the server supports goto implementation.
    pub implementation: bool,
    /// Whether the server supports call hierarchy.
    pub call_hierarchy: bool,
    /// Whether the server supports code actions.
    pub code_action: bool,
}

impl CachedServerCapabilities {
    /// Parse capabilities from InitializeResult.
    pub fn from_initialize_result(result: &InitializeResult) -> Self {
        let caps = &result.capabilities;
        Self {
            hover: caps.hover_provider.is_some(),
            goto_definition: caps.definition_provider.is_some(),
            find_references: caps.references_provider.is_some(),
            document_symbol: caps.document_symbol_provider.is_some(),
            workspace_symbol: caps.workspace_symbol_provider.is_some(),
            completion: caps.completion_provider.is_some(),
            signature_help: caps.signature_help_provider.is_some(),
            rename: caps.rename_provider.is_some(),
            formatting: caps.document_formatting_provider.is_some(),
            implementation: caps.implementation_provider.is_some(),
            call_hierarchy: caps.call_hierarchy_provider.is_some(),
            code_action: caps.code_action_provider.is_some(),
        }
    }
}
