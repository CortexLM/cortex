//! LSP request methods implementation.

use crate::{LspError, Result};
use lsp_types::request::{GotoImplementationParams, GotoImplementationResponse};
use lsp_types::*;
use std::path::Path;

use super::LspClient;

impl LspClient {
    /// Open a document.
    pub async fn did_open(&self, path: &Path, language_id: &str, text: &str) -> Result<()> {
        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: language_id.to_string(),
                version: 1,
                text: text.to_string(),
            },
        };

        self.notify("textDocument/didOpen", params).await
    }

    /// Close a document.
    pub async fn did_close(&self, path: &Path) -> Result<()> {
        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri },
        };

        self.notify("textDocument/didClose", params).await
    }

    /// Get hover information.
    pub async fn hover(&self, path: &Path, line: u32, character: u32) -> Result<Option<Hover>> {
        self.check_capability("hover").await?;

        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
        };

        self.request("textDocument/hover", params).await
    }

    /// Go to definition.
    pub async fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Option<GotoDefinitionResponse>> {
        self.check_capability("definition").await?;

        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.request("textDocument/definition", params).await
    }

    /// Find all references to a symbol.
    pub async fn find_references(
        &self,
        path: &Path,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> Result<Option<Vec<Location>>> {
        self.check_capability("references").await?;

        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            context: ReferenceContext {
                include_declaration,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.request("textDocument/references", params).await
    }

    /// Get document symbols (functions, classes, variables, etc.).
    pub async fn document_symbols(&self, path: &Path) -> Result<Option<DocumentSymbolResponse>> {
        self.check_capability("documentSymbol").await?;

        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.request("textDocument/documentSymbol", params).await
    }

    /// Search for symbols across the workspace.
    pub async fn workspace_symbols(&self, query: &str) -> Result<Option<Vec<SymbolInformation>>> {
        self.check_capability("workspaceSymbol").await?;

        let params = WorkspaceSymbolParams {
            query: query.to_string(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.request("workspace/symbol", params).await
    }

    /// Get completion items at a position.
    pub async fn completion(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Option<CompletionResponse>> {
        self.check_capability("completion").await?;

        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };

        self.request("textDocument/completion", params).await
    }

    /// Get signature help at a position.
    pub async fn signature_help(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Option<SignatureHelp>> {
        self.check_capability("signatureHelp").await?;

        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = SignatureHelpParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
            context: None,
        };

        self.request("textDocument/signatureHelp", params).await
    }

    /// Rename a symbol across the workspace.
    pub async fn rename(
        &self,
        path: &Path,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<Option<WorkspaceEdit>> {
        self.check_capability("rename").await?;

        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            new_name: new_name.to_string(),
            work_done_progress_params: Default::default(),
        };

        self.request("textDocument/rename", params).await
    }

    /// Format a document.
    pub async fn format(&self, path: &Path) -> Result<Option<Vec<TextEdit>>> {
        self.check_capability("formatting").await?;

        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri },
            options: FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        };

        self.request("textDocument/formatting", params).await
    }

    /// Go to implementation (find implementations of interface/trait).
    pub async fn goto_implementation(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Option<GotoImplementationResponse>> {
        self.check_capability("implementation").await?;

        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = GotoImplementationParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.request("textDocument/implementation", params).await
    }

    /// Prepare call hierarchy at a position.
    /// Returns call hierarchy items that can be used with incoming/outgoing calls.
    pub async fn prepare_call_hierarchy(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        self.check_capability("callHierarchy").await?;

        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = CallHierarchyPrepareParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
        };

        self.request("textDocument/prepareCallHierarchy", params)
            .await
    }

    /// Get incoming calls (find all callers of a function).
    pub async fn incoming_calls(
        &self,
        item: &CallHierarchyItem,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        let params = CallHierarchyIncomingCallsParams {
            item: item.clone(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.request("callHierarchy/incomingCalls", params).await
    }

    /// Get outgoing calls (find all functions called by this function).
    pub async fn outgoing_calls(
        &self,
        item: &CallHierarchyItem,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        let params = CallHierarchyOutgoingCallsParams {
            item: item.clone(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.request("callHierarchy/outgoingCalls", params).await
    }

    /// Get code actions (quick fixes, refactorings) for a range.
    pub async fn code_actions(
        &self,
        path: &Path,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
    ) -> Result<Option<CodeActionResponse>> {
        self.check_capability("codeAction").await?;

        let uri = Url::from_file_path(path)
            .map_err(|_| LspError::Communication("Invalid path".into()))?;

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range {
                start: Position {
                    line: start_line,
                    character: start_char,
                },
                end: Position {
                    line: end_line,
                    character: end_char,
                },
            },
            context: CodeActionContext {
                diagnostics: Vec::new(),
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.request("textDocument/codeAction", params).await
    }
}
