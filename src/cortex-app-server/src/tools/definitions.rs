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
            description: "Present a comprehensive implementation plan with multi-agent expert analysis for user approval. Include security, architecture, performance, and other expert perspectives.".to_string(),
            parameters: json!({
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
            category: "workflow".to_string(),
        },
    ]
}
