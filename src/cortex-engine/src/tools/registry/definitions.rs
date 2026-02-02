//! Default tool definitions and registration.

use std::sync::Arc;

use serde_json::json;

use super::ToolRegistry;
use crate::agent::tools::{
    LspDiagnosticsTool, LspHoverTool, MultiEditTool, PatchTool, WebSearchTool,
};
use crate::tools::handlers::LocalShellHandler;
use crate::tools::spec::ToolDefinition;

impl ToolRegistry {
    pub(super) fn register_default_tools(&mut self) {
        self.register_execute_tool();
        self.register_apply_patch_tool();
        self.register_read_tool();
        self.register_ls_tool();
        self.register_create_tool();
        self.register_search_files_tool();
        self.register_web_search_tool();
        self.register_edit_tool();
        self.register_grep_tool();
        self.register_glob_tool();
        self.register_fetch_url_tool();
        self.register_web_fetch_tool();
        self.register_todo_tools();
        self.register_multi_edit_tool();
        self.register_task_tools();
        self.register_lsp_tools();
        self.register_plan_tool();
        self.register_questions_tool();
        self.register_skill_tool();
        self.register_batch_tool();
    }

    fn register_execute_tool(&mut self) {
        self.register_with_handler(
            ToolDefinition::new(
                "Execute",
                "Execute a shell command on the local system. Use for running CLI tools, scripts, and system commands. \
                Long-running processes may timeout.",
                json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Command and arguments to execute"
                        },
                        "workdir": {
                            "type": "string",
                            "description": "Working directory (optional, defaults to cwd)"
                        },
                        "timeout": {
                            "type": "integer",
                            "description": "Timeout in milliseconds (optional)"
                        }
                    },
                    "required": ["command"]
                }),
            ),
            Arc::new(LocalShellHandler::new()),
        );
    }

    fn register_apply_patch_tool(&mut self) {
        self.register_with_handler(
            ToolDefinition::new(
                "ApplyPatch",
                "Apply changes to files using a unified diff format. Supports creating, modifying, and deleting files.",
                json!({
                    "type": "object",
                    "properties": {
                        "patch": {
                            "type": "string",
                            "description": "Unified diff patch to apply"
                        },
                        "dry_run": {
                            "type": "boolean",
                            "description": "If true, only check if patch can be applied without making changes",
                            "default": false
                        }
                    },
                    "required": ["patch"]
                }),
            ),
            Arc::new(PatchTool::new()),
        );
    }

    fn register_read_tool(&mut self) {
        self.register(ToolDefinition::new(
            "Read",
            "Read the contents of a file. By default reads entire file, but for large files results are truncated. Use offset and limit for specific portions. Requires absolute file paths.",
            json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The absolute path to the file to read (must be absolute, not relative)"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "The line number to start reading from (0-based, defaults to 0)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "The maximum number of lines to read (defaults to 2400)"
                    }
                },
                "required": ["file_path"]
            }),
        ));
    }

    fn register_ls_tool(&mut self) {
        self.register(ToolDefinition::new(
            "LS",
            "List the contents of a directory with optional pattern-based filtering. Prefer usage of Grep and Glob tools for more targeted searches. Requires absolute directory paths.",
            json!({
                "type": "object",
                "properties": {
                    "directory_path": {
                        "type": "string",
                        "description": "The absolute path to the directory to list (must be absolute, not relative)"
                    },
                    "ignorePatterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Array of glob patterns to ignore when listing files and directories. Example: [\"node_modules/**\", \"*.log\"]"
                    }
                },
                "required": []
            }),
        ));
    }

    fn register_create_tool(&mut self) {
        self.register(ToolDefinition::new(
            "Create",
            "Creates a new file on the file system with the specified content. Prefer editing existing files unless you need to create a new file.",
            json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The path to the file for the new file"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the file"
                    }
                },
                "required": ["file_path", "content"]
            }),
        ));
    }

    fn register_search_files_tool(&mut self) {
        self.register(ToolDefinition::new(
            "SearchFiles",
            "Search for files matching a pattern.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Search pattern (glob or regex)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in"
                    },
                    "content_pattern": {
                        "type": "string",
                        "description": "Pattern to search within file contents"
                    }
                },
                "required": ["pattern"]
            }),
        ));
    }

    fn register_web_search_tool(&mut self) {
        self.register_with_handler(
            ToolDefinition::new(
                "WebSearch",
                "Search the web for information using Exa AI. Returns relevant results from the internet.",
                json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query"
                        },
                        "num_results": {
                            "type": "integer",
                            "description": "Number of results to return (default: 8)"
                        },
                        "category": {
                            "type": "string",
                            "description": "Category of search: 'company', 'research paper', 'news', etc."
                        },
                        "include_domains": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Limit results to these domains"
                        },
                        "exclude_domains": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Exclude results from these domains"
                        },
                        "use_neural": {
                            "type": "boolean",
                            "description": "Use neural search for better relevance"
                        },
                        "livecrawl": {
                            "type": "string",
                            "enum": ["fallback", "preferred"],
                            "description": "Live crawl mode"
                        },
                        "type": {
                            "type": "string",
                            "enum": ["auto", "fast", "deep"],
                            "description": "Search type"
                        },
                        "context_max_characters": {
                            "type": "integer",
                            "description": "Max characters for context"
                        }
                    },
                    "required": ["query"]
                }),
            ),
            Arc::new(WebSearchTool::new()),
        );
    }

    fn register_edit_tool(&mut self) {
        self.register(ToolDefinition::new(
            "Edit",
            "Edit the contents of a file by finding and replacing text. The old_str must be unique in the file, or change_all must be true.",
            json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "The path to the file to edit"
                    },
                    "old_str": {
                        "type": "string",
                        "description": "The exact text to find and replace in the file"
                    },
                    "new_str": {
                        "type": "string",
                        "description": "The text to replace the old_str with"
                    },
                    "change_all": {
                        "type": "boolean",
                        "description": "Whether to replace all occurrences (true) or just the first one (false). Defaults to false."
                    }
                },
                "required": ["file_path", "old_str", "new_str"]
            }),
        ));
    }

    fn register_grep_tool(&mut self) {
        self.register(ToolDefinition::new(
            "Grep",
            "High-performance file content search. Searches for patterns in file contents with regex support.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "A search pattern to match in file contents. Supports regex syntax."
                    },
                    "path": {
                        "type": "string",
                        "description": "Path to a file or directory to search in. Defaults to current directory."
                    },
                    "case_insensitive": {
                        "type": "boolean",
                        "description": "Perform case-insensitive matching."
                    },
                    "line_numbers": {
                        "type": "boolean",
                        "description": "Show line numbers in output."
                    },
                    "context": {
                        "type": "integer",
                        "description": "Number of lines to show before and after each match."
                    },
                    "context_before": {
                        "type": "integer",
                        "description": "Number of lines to show before each match."
                    },
                    "context_after": {
                        "type": "integer",
                        "description": "Number of lines to show after each match."
                    },
                    "glob_pattern": {
                        "type": "string",
                        "description": "Glob pattern to filter files. Example: \"*.js\" for JavaScript files."
                    },
                    "output_mode": {
                        "type": "string",
                        "enum": ["file_paths", "content"],
                        "description": "Output format: 'file_paths' returns only matching file paths, 'content' returns matching lines with context."
                    },
                    "head_limit": {
                        "type": "integer",
                        "description": "Limit output to first N lines/entries."
                    }
                },
                "required": ["pattern"]
            }),
        ));
    }

    fn register_glob_tool(&mut self) {
        self.register(ToolDefinition::new(
            "Glob",
            "Advanced file path search using glob patterns with multiple pattern support and exclusions.",
            json!({
                "type": "object",
                "properties": {
                    "patterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Array of glob patterns to match file paths. Examples: [\"*.js\", \"*.ts\"] for JavaScript and TypeScript files."
                    },
                    "folder": {
                        "type": "string",
                        "description": "Path to the directory to search in. Defaults to current directory."
                    },
                    "exclude_patterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Array of glob patterns to exclude from results."
                    }
                },
                "required": ["patterns"]
            }),
        ));
    }

    fn register_fetch_url_tool(&mut self) {
        self.register(ToolDefinition::new(
            "FetchUrl",
            "Scrapes content from URLs and returns the contents. Use for fetching web pages, API responses, and documents.",
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch content from. Must be a valid http:// or https:// URL."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["text", "markdown", "html"],
                        "description": "The format to return the content in (default: markdown)"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Optional timeout in seconds"
                    }
                },
                "required": ["url"]
            }),
        ));
    }

    fn register_web_fetch_tool(&mut self) {
        self.register(ToolDefinition::new(
            "WebFetch",
            "Scrapes content from URLs and returns the contents. Improved version with better markdown conversion and metadata extraction.",
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch content from. Must be a valid http:// or https:// URL."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["text", "markdown", "html"],
                        "description": "The format to return the content in (default: markdown)"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Optional timeout in seconds"
                    }
                },
                "required": ["url"]
            }),
        ));
    }

    fn register_todo_tools(&mut self) {
        self.register(ToolDefinition::new(
            "TodoWrite",
            "Draft and maintain a structured todo list for the current coding session. Helps organize multi-step work and track progress.",
            json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "A unique identifier for the todo item"
                                },
                                "content": {
                                    "type": "string",
                                    "description": "The content of the todo item"
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"],
                                    "description": "The status of the todo item"
                                },
                                "priority": {
                                    "type": "string",
                                    "enum": ["high", "medium", "low"],
                                    "description": "The priority level of the todo item"
                                }
                            },
                            "required": ["id", "content", "status", "priority"]
                        },
                        "description": "The todo list items"
                    }
                },
                "required": ["todos"]
            }),
        ));

        self.register(ToolDefinition::new(
            "TodoRead",
            "Read the current todo list to see progress and remaining tasks.",
            json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        ));
    }

    fn register_multi_edit_tool(&mut self) {
        self.register_with_handler(
            ToolDefinition::new(
                "MultiEdit",
                "Edit multiple files in a single operation. More efficient than multiple Edit calls when making related changes across files. \
                This tool ensures atomic application - either all edits succeed or none are applied.",
                json!({
                    "type": "object",
                    "properties": {
                        "edits": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "file_path": {
                                        "type": "string",
                                        "description": "The path to the file to edit"
                                    },
                                    "old_str": {
                                        "type": "string",
                                        "description": "The exact text to find and replace"
                                    },
                                    "new_str": {
                                        "type": "string",
                                        "description": "The text to replace with"
                                    },
                                    "change_all": {
                                        "type": "boolean",
                                        "description": "Whether to replace all occurrences (true) or just the first one (false). Defaults to false."
                                    }
                                },
                                "required": ["file_path", "old_str", "new_str"]
                            },
                            "description": "Array of edit operations to perform"
                        }
                    },
                    "required": ["edits"]
                }),
            ),
            Arc::new(MultiEditTool::new()),
        );
    }

    fn register_task_tools(&mut self) {
        // Task - Spawn a subagent for complex tasks
        // Uses the definition from TaskHandler for consistency
        self.register(crate::tools::handlers::task::SimpleTaskHandler::definition());

        // ListSubagents - List available subagent types
        self.register(crate::tools::handlers::task::ListSubagentsHandler::definition());
    }

    fn register_lsp_tools(&mut self) {
        self.register_with_handler(
            ToolDefinition::new(
                "LspDiagnostics",
                "Get compiler and linter diagnostics for a file or workspace. Supports Rust, Python, JavaScript/TypeScript, and Go.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path to check. If omitted, checks entire workspace."
                        },
                        "severity": {
                            "type": "string",
                            "enum": ["error", "warning", "all"],
                            "description": "Filter by severity level (default: all)"
                        }
                    },
                    "required": []
                }),
            ),
            Arc::new(LspDiagnosticsTool::new_handler()),
        );

        self.register_with_handler(
            ToolDefinition::new(
                "LspHover",
                "Get type information and documentation for a symbol at a specific position. Useful for understanding code.",
                json!({
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "File path containing the symbol"
                        },
                        "line": {
                            "type": "integer",
                            "description": "Line number (1-based)"
                        },
                        "column": {
                            "type": "integer",
                            "description": "Column number (1-based)"
                        }
                    },
                    "required": ["file", "line", "column"]
                }),
            ),
            Arc::new(LspHoverTool::new()),
        );

        self.register(ToolDefinition::new(
            "LspSymbols",
            "Search for symbols (functions, classes, types, etc.) in the workspace. Use for navigating large codebases.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Symbol name or partial name to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (default: current directory)"
                    }
                },
                "required": ["query"]
            }),
        ));
    }

    fn register_plan_tool(&mut self) {
        self.register(ToolDefinition::new(
            "Plan",
            "Present a comprehensive implementation plan with multi-agent expert analysis for user approval. Include security, architecture, performance, and other expert perspectives.",
            json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short title for the plan"
                    },
                    "description": {
                        "type": "string",
                        "description": "Detailed description of what will be implemented and the overall approach"
                    },
                    "architecture": {
                        "type": "string",
                        "description": "High-level architecture description"
                    },
                    "tech_stack": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Technologies, frameworks, and tools to be used"
                    },
                    "tasks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "title": { "type": "string" },
                                "description": { "type": "string" },
                                "subtasks": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                },
                                "dependencies": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "IDs of tasks this depends on"
                                },
                                "complexity": {
                                    "type": "string",
                                    "enum": ["low", "medium", "high", "critical"]
                                },
                                "estimated_time": { "type": "string" },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"]
                                }
                            },
                            "required": ["id", "title"]
                        },
                        "description": "List of tasks with subtasks and dependencies"
                    },
                    "use_cases": {
                        "type": "array",
                        "items": {
                            "oneOf": [
                                { "type": "string" },
                                {
                                    "type": "object",
                                    "properties": {
                                        "name": { "type": "string" },
                                        "description": { "type": "string" },
                                        "actors": { "type": "array", "items": { "type": "string" } },
                                        "flow": { "type": "array", "items": { "type": "string" } }
                                    }
                                }
                            ]
                        },
                        "description": "User stories and use cases"
                    },
                    "agent_analyses": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "agent": {
                                    "type": "string",
                                    "description": "Name of the expert agent (e.g., 'Security Analyst', 'Performance Engineer')"
                                },
                                "role": {
                                    "type": "string",
                                    "description": "Role description"
                                },
                                "findings": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Key findings from analysis"
                                },
                                "recommendations": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Recommendations and best practices"
                                },
                                "risk_level": {
                                    "type": "string",
                                    "enum": ["low", "medium", "high", "critical"]
                                },
                                "priority_items": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                }
                            },
                            "required": ["agent", "role", "findings", "recommendations"]
                        },
                        "description": "Expert analyses from different perspectives (security, performance, UX, etc.)"
                    },
                    "risks": {
                        "type": "array",
                        "items": {
                            "oneOf": [
                                { "type": "string" },
                                {
                                    "type": "object",
                                    "properties": {
                                        "risk": { "type": "string" },
                                        "level": { "type": "string", "enum": ["low", "medium", "high", "critical"] },
                                        "mitigation": { "type": "string" }
                                    }
                                }
                            ]
                        },
                        "description": "Identified risks and mitigation strategies"
                    },
                    "success_criteria": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Criteria to determine if implementation is successful"
                    },
                    "timeline": {
                        "type": "string",
                        "description": "Estimated timeline for completion"
                    },
                    "estimated_changes": {
                        "type": "string",
                        "description": "Brief estimate of scope (e.g., 'Small: ~50 lines', 'Large: ~1000 lines')"
                    }
                },
                "required": ["title", "description", "tasks", "agent_analyses"]
            }),
        ));
    }

    fn register_questions_tool(&mut self) {
        self.register(ToolDefinition::new(
            "Questions",
            "Ask the user a series of questions to gather requirements. Each question can have predefined options or allow free text input. Use this tool when you need to collect structured information from the user.",
            json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Title for the questions form"
                    },
                    "description": {
                        "type": "string",
                        "description": "Brief description of why these questions are being asked"
                    },
                    "questions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "Unique identifier for the question"
                                },
                                "question": {
                                    "type": "string",
                                    "description": "The question text"
                                },
                                "type": {
                                    "type": "string",
                                    "enum": ["single", "multiple", "text", "number"],
                                    "description": "Question type: single (radio), multiple (checkbox), text (free input), number (numeric)"
                                },
                                "options": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "value": { "type": "string" },
                                            "label": { "type": "string" },
                                            "description": { "type": "string" },
                                            "selected": { 
                                                "type": "boolean",
                                                "description": "Whether this option should be pre-selected as recommended"
                                            }
                                        },
                                        "required": ["value", "label"]
                                    },
                                    "description": "Predefined options for single/multiple choice questions"
                                },
                                "placeholder": {
                                    "type": "string",
                                    "description": "Placeholder text for text/number inputs"
                                },
                                "required": {
                                    "type": "boolean",
                                    "description": "Whether an answer is required"
                                },
                                "allow_custom": {
                                    "type": "boolean",
                                    "description": "Allow custom answer even with predefined options"
                                }
                            },
                            "required": ["id", "question", "type"]
                        },
                        "description": "List of questions to ask"
                    }
                },
                "required": ["title", "questions"]
            }),
        ));
    }

    fn register_skill_tool(&mut self) {
        self.register_with_handler(
            crate::tools::handlers::SkillHandler::definition(),
            Arc::new(crate::tools::handlers::SkillHandler::new()),
        );
    }

    fn register_batch_tool(&mut self) {
        // Batch - Execute multiple tools in parallel
        // Note: The actual handler is registered in ToolRouter since it needs
        // access to the executor. Here we just register the definition.
        self.register(crate::tools::handlers::batch_tool_definition());
    }
}
