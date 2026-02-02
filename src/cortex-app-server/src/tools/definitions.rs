//! Tool definitions for API.

use serde_json::json;

use super::types::ToolDefinition;

/// Get all tool definitions.
pub fn get_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "Execute".to_string(),
            description: "Execute a shell command on the local system".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "oneOf": [
                            { "type": "string" },
                            { "type": "array", "items": { "type": "string" } }
                        ],
                        "description": "Command to execute (string or array of args)"
                    },
                    "workdir": {
                        "type": "string",
                        "description": "Working directory"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds"
                    }
                },
                "required": ["command"]
            }),
            category: "execution".to_string(),
        },
        ToolDefinition {
            name: "Read".to_string(),
            description: "Read the contents of a file".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line offset to start reading from"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read"
                    }
                },
                "required": ["file_path"]
            }),
            category: "filesystem".to_string(),
        },
        ToolDefinition {
            name: "Create".to_string(),
            description: "Create a new file with content".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path for the new file"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write"
                    }
                },
                "required": ["file_path", "content"]
            }),
            category: "filesystem".to_string(),
        },
        ToolDefinition {
            name: "Edit".to_string(),
            description: "Edit a file by finding and replacing text".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the file to edit"
                    },
                    "old_str": {
                        "type": "string",
                        "description": "Text to find"
                    },
                    "new_str": {
                        "type": "string",
                        "description": "Text to replace with"
                    },
                    "change_all": {
                        "type": "boolean",
                        "description": "Replace all occurrences"
                    }
                },
                "required": ["file_path", "old_str", "new_str"]
            }),
            category: "filesystem".to_string(),
        },
        ToolDefinition {
            name: "LS".to_string(),
            description: "List directory contents".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "directory_path": {
                        "type": "string",
                        "description": "Path to directory"
                    }
                },
                "required": []
            }),
            category: "filesystem".to_string(),
        },
        ToolDefinition {
            name: "Grep".to_string(),
            description: "Search file contents for a pattern".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Search pattern"
                    },
                    "path": {
                        "type": "string",
                        "description": "Path to search in"
                    },
                    "case_insensitive": {
                        "type": "boolean",
                        "description": "Case insensitive search"
                    },
                    "line_numbers": {
                        "type": "boolean",
                        "description": "Show line numbers"
                    }
                },
                "required": ["pattern"]
            }),
            category: "search".to_string(),
        },
        ToolDefinition {
            name: "Glob".to_string(),
            description: "Find files matching glob patterns".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "patterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Glob patterns to match"
                    },
                    "folder": {
                        "type": "string",
                        "description": "Directory to search in"
                    }
                },
                "required": ["patterns"]
            }),
            category: "search".to_string(),
        },
        ToolDefinition {
            name: "FetchUrl".to_string(),
            description: "Fetch content from a URL".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to fetch"
                    }
                },
                "required": ["url"]
            }),
            category: "web".to_string(),
        },
        ToolDefinition {
            name: "WebSearch".to_string(),
            description: "Search the web".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    }
                },
                "required": ["query"]
            }),
            category: "web".to_string(),
        },
        ToolDefinition {
            name: "ApplyPatch".to_string(),
            description: "Apply a unified diff patch".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "patch": {
                        "type": "string",
                        "description": "Unified diff patch content"
                    }
                },
                "required": ["patch"]
            }),
            category: "filesystem".to_string(),
        },
        ToolDefinition {
            name: "TodoWrite".to_string(),
            description: "Draft and maintain a structured todo list for tracking multi-step work"
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "oneOf": [
                            {
                                "type": "string",
                                "description": "The todo list as a numbered string with status markers. Format: '1. [completed] Task\\n2. [in_progress] Task\\n3. [pending] Task'"
                            },
                            {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "id": { "type": "string" },
                                        "content": { "type": "string" },
                                        "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] },
                                        "priority": { "type": "string", "enum": ["high", "medium", "low"] }
                                    },
                                    "required": ["id", "content", "status"]
                                },
                                "description": "Array of todo items"
                            }
                        ]
                    }
                },
                "required": ["todos"]
            }),
            category: "planning".to_string(),
        },
        ToolDefinition {
            name: "TodoRead".to_string(),
            description: "Read the current todo list".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            category: "planning".to_string(),
        },
        ToolDefinition {
            name: "MultiEdit".to_string(),
            description: "Apply multiple edits to files in a single operation".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "edits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "file_path": { "type": "string" },
                                "old_str": { "type": "string" },
                                "new_str": { "type": "string" }
                            },
                            "required": ["file_path", "old_str", "new_str"]
                        },
                        "description": "Array of edit operations"
                    }
                },
                "required": ["edits"]
            }),
            category: "filesystem".to_string(),
        },
        ToolDefinition {
            name: "Questions".to_string(),
            description: "Ask the user a series of questions to gather requirements. Each question can have predefined options or allow free text input.".to_string(),
            parameters: json!({
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
                                "id": { "type": "string" },
                                "question": { "type": "string" },
                                "type": {
                                    "type": "string",
                                    "enum": ["single", "multiple", "text", "number"],
                                    "description": "single=radio, multiple=checkbox, text=free input, number=numeric input"
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
                                                "description": "Whether this option should be pre-selected (recommended)"
                                            }
                                        },
                                        "required": ["value", "label"]
                                    },
                                    "description": "Predefined options for single/multiple choice"
                                },
                                "placeholder": { "type": "string" },
                                "required": { "type": "boolean" },
                                "allow_custom": {
                                    "type": "boolean",
                                    "description": "Allow user to type custom answer even with predefined options"
                                }
                            },
                            "required": ["id", "question", "type"]
                        }
                    }
                },
                "required": ["title", "questions"]
            }),
            category: "workflow".to_string(),
        },
        ToolDefinition {
            name: "Plan".to_string(),
            description: "Present an implementation plan for user approval before coding. Use this to outline approach, list tasks, and get confirmation.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short title for the plan"
                    },
                    "description": {
                        "type": "string",
                        "description": "Detailed description of what will be implemented and how"
                    },
                    "tasks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "title": { "type": "string" },
                                "description": { "type": "string" },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"]
                                }
                            },
                            "required": ["id", "title"]
                        },
                        "description": "List of tasks to complete"
                    },
                    "estimated_changes": {
                        "type": "string",
                        "description": "Brief estimate of scope (e.g., 'Small: ~50 lines', 'Medium: ~200 lines')"
                    }
                },
                "required": ["title", "description", "tasks"]
            }),
            category: "workflow".to_string(),
        },
    ]
}
