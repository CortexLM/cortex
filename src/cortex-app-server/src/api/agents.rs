//! Custom agents API endpoints.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

use super::types::{
    AgentDefinition, CreateAgentRequest, GeneratePromptRequest, GeneratePromptResponse,
    ImportAgentRequest, UpdateAgentRequest,
};

/// Read agent file from disk.
fn read_agent_file(path: &std::path::Path, scope: &str) -> Option<AgentDefinition> {
    if path.extension().and_then(|e| e.to_str()) != Some("md") {
        return None;
    }

    let content = std::fs::read_to_string(path).ok()?;
    let name = path.file_stem()?.to_str()?.to_string();

    // Parse YAML frontmatter
    if content.starts_with("---") {
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() >= 3
            && let Ok(frontmatter) = serde_yaml::from_str::<serde_json::Value>(parts[1])
        {
            return Some(AgentDefinition {
                name,
                description: frontmatter["description"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                tools: frontmatter["tools"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                model: frontmatter["model"]
                    .as_str()
                    .unwrap_or("inherit")
                    .to_string(),
                permission_mode: frontmatter["permissionMode"]
                    .as_str()
                    .unwrap_or("default")
                    .to_string(),
                prompt: parts[2].trim().to_string(),
                scope: scope.to_string(),
            });
        }
    }

    // No frontmatter - entire file is the prompt
    Some(AgentDefinition {
        name,
        description: String::new(),
        tools: Vec::new(),
        model: "inherit".to_string(),
        permission_mode: "default".to_string(),
        prompt: content,
        scope: scope.to_string(),
    })
}

/// List all agents.
pub async fn list_agents() -> AppResult<Json<Vec<AgentDefinition>>> {
    let mut agents = Vec::new();

    // Project agents (.factory/agents/)
    let project_dir = std::path::Path::new(".factory/agents");
    if project_dir.exists()
        && let Ok(entries) = std::fs::read_dir(project_dir)
    {
        for entry in entries.flatten() {
            if let Some(agent) = read_agent_file(&entry.path(), "project") {
                agents.push(agent);
            }
        }
    }

    // User agents (~/.factory/agents/)
    if let Some(home) = dirs::home_dir() {
        let user_dir = home.join(".factory/agents");
        if user_dir.exists()
            && let Ok(entries) = std::fs::read_dir(&user_dir)
        {
            for entry in entries.flatten() {
                if let Some(agent) = read_agent_file(&entry.path(), "user") {
                    agents.push(agent);
                }
            }
        }
    }

    Ok(Json(agents))
}

/// Get a specific agent.
pub async fn get_agent(Path(name): Path<String>) -> AppResult<Json<AgentDefinition>> {
    // Check project first
    let project_path = std::path::Path::new(".factory/agents").join(format!("{}.md", name));
    if let Some(agent) = read_agent_file(&project_path, "project") {
        return Ok(Json(agent));
    }

    // Check user
    if let Some(home) = dirs::home_dir() {
        let user_path = home.join(".factory/agents").join(format!("{}.md", name));
        if let Some(agent) = read_agent_file(&user_path, "user") {
            return Ok(Json(agent));
        }
    }

    Err(AppError::NotFound(format!("Agent not found: {}", name)))
}

/// Create or update an agent.
pub async fn create_agent(Json(req): Json<CreateAgentRequest>) -> AppResult<Json<AgentDefinition>> {
    let dir = if req.scope == "project" {
        std::path::PathBuf::from(".factory/agents")
    } else {
        dirs::home_dir()
            .ok_or_else(|| AppError::Internal("Cannot find home directory".to_string()))?
            .join(".factory/agents")
    };

    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::Internal(format!("Failed to create directory: {}", e)))?;

    let path = dir.join(format!("{}.md", req.name));

    // Build markdown with YAML frontmatter
    let content = format!(
        "---\ndescription: {}\ntools: [{}]\nmodel: {}\npermissionMode: {}\n---\n\n{}",
        serde_yaml::to_string(&req.description)
            .unwrap_or_default()
            .trim(),
        req.tools
            .iter()
            .map(|t| format!("\"{}\"", t))
            .collect::<Vec<_>>()
            .join(", "),
        req.model,
        req.permission_mode,
        req.prompt
    );

    std::fs::write(&path, &content)
        .map_err(|e| AppError::Internal(format!("Failed to write agent file: {}", e)))?;

    Ok(Json(AgentDefinition {
        name: req.name,
        description: req.description,
        tools: req.tools,
        model: req.model,
        permission_mode: req.permission_mode,
        prompt: req.prompt,
        scope: req.scope,
    }))
}

/// Delete an agent.
pub async fn delete_agent(Path(name): Path<String>) -> AppResult<Json<serde_json::Value>> {
    // Try project first
    let project_path = std::path::Path::new(".factory/agents").join(format!("{}.md", name));
    if project_path.exists() {
        std::fs::remove_file(&project_path)
            .map_err(|e| AppError::Internal(format!("Failed to delete: {}", e)))?;
        return Ok(Json(serde_json::json!({"deleted": true})));
    }

    // Try user
    if let Some(home) = dirs::home_dir() {
        let user_path = home.join(".factory/agents").join(format!("{}.md", name));
        if user_path.exists() {
            std::fs::remove_file(&user_path)
                .map_err(|e| AppError::Internal(format!("Failed to delete: {}", e)))?;
            return Ok(Json(serde_json::json!({"deleted": true})));
        }
    }

    Err(AppError::NotFound(format!("Agent not found: {}", name)))
}

/// List built-in agents.
pub async fn list_builtin_agents() -> Json<Vec<AgentDefinition>> {
    Json(vec![
        AgentDefinition {
            name: "general".to_string(),
            description: "General purpose agent for coding and file operations".to_string(),
            tools: vec!["*".to_string()],
            model: "inherit".to_string(),
            permission_mode: "default".to_string(),
            prompt: String::new(),
            scope: "builtin".to_string(),
        },
        AgentDefinition {
            name: "explore".to_string(),
            description: "Fast exploration agent for reading and searching code".to_string(),
            tools: vec!["Read".to_string(), "Grep".to_string(), "Glob".to_string(), "LS".to_string()],
            model: "inherit".to_string(),
            permission_mode: "default".to_string(),
            prompt: "You are a code exploration assistant. Focus on reading and understanding code without making changes.".to_string(),
            scope: "builtin".to_string(),
        },
        AgentDefinition {
            name: "research".to_string(),
            description: "Research and analysis agent for web searches and documentation".to_string(),
            tools: vec!["Read".to_string(), "WebSearch".to_string(), "FetchUrl".to_string()],
            model: "inherit".to_string(),
            permission_mode: "default".to_string(),
            prompt: "You are a research assistant. Help users find information, documentation, and answers to their questions.".to_string(),
            scope: "builtin".to_string(),
        },
    ])
}

/// Update an existing agent.
pub async fn update_agent(
    Path(name): Path<String>,
    Json(req): Json<UpdateAgentRequest>,
) -> AppResult<Json<AgentDefinition>> {
    // Find existing agent
    let project_path = std::path::Path::new(".factory/agents").join(format!("{}.md", name));
    let user_path =
        dirs::home_dir().map(|h| h.join(".factory/agents").join(format!("{}.md", name)));

    let (existing, path) = if let Some(agent) = read_agent_file(&project_path, "project") {
        (agent, project_path)
    } else if let Some(ref user_path) = user_path {
        if let Some(agent) = read_agent_file(user_path, "user") {
            (agent, user_path.clone())
        } else {
            return Err(AppError::NotFound(format!("Agent not found: {}", name)));
        }
    } else {
        return Err(AppError::NotFound(format!("Agent not found: {}", name)));
    };

    // Merge updates
    let updated = AgentDefinition {
        name: existing.name,
        description: req.description.unwrap_or(existing.description),
        tools: req.tools.unwrap_or(existing.tools),
        model: req.model.unwrap_or(existing.model),
        permission_mode: req.permission_mode.unwrap_or(existing.permission_mode),
        prompt: req.prompt.unwrap_or(existing.prompt),
        scope: existing.scope,
    };

    // Write back
    let content = format!(
        "---\ndescription: {}\ntools: [{}]\nmodel: {}\npermissionMode: {}\n---\n\n{}",
        serde_yaml::to_string(&updated.description)
            .unwrap_or_default()
            .trim(),
        updated
            .tools
            .iter()
            .map(|t| format!("\"{}\"", t))
            .collect::<Vec<_>>()
            .join(", "),
        updated.model,
        updated.permission_mode,
        updated.prompt
    );

    std::fs::write(&path, &content)
        .map_err(|e| AppError::Internal(format!("Failed to update agent: {}", e)))?;

    Ok(Json(updated))
}

/// Parse agent content from various formats.
fn parse_agent_content(content: &str, _format: &str) -> AppResult<(String, AgentDefinition)> {
    // Try to extract name from YAML frontmatter
    let (name, description, tools, model, permission_mode, prompt) = if content.starts_with("---") {
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() >= 3 {
            if let Ok(frontmatter) = serde_yaml::from_str::<serde_json::Value>(parts[1]) {
                let name = frontmatter["name"]
                    .as_str()
                    .unwrap_or("imported-agent")
                    .to_string();
                let description = frontmatter["description"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let tools = frontmatter["tools"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let model = frontmatter["model"]
                    .as_str()
                    .unwrap_or("inherit")
                    .to_string();
                let permission_mode = frontmatter["permissionMode"]
                    .as_str()
                    .unwrap_or("default")
                    .to_string();
                let prompt = parts[2].trim().to_string();

                (name, description, tools, model, permission_mode, prompt)
            } else {
                return Err(AppError::BadRequest("Invalid YAML frontmatter".to_string()));
            }
        } else {
            return Err(AppError::BadRequest("Invalid markdown format".to_string()));
        }
    } else {
        // No frontmatter - generate a name and use content as prompt
        let name = "imported-agent".to_string();
        (
            name,
            String::new(),
            Vec::new(),
            "inherit".to_string(),
            "default".to_string(),
            content.to_string(),
        )
    };

    Ok((
        name.clone(),
        AgentDefinition {
            name,
            description,
            tools,
            model,
            permission_mode,
            prompt,
            scope: String::new(),
        },
    ))
}

/// Import an agent from file content.
pub async fn import_agent(Json(req): Json<ImportAgentRequest>) -> AppResult<Json<AgentDefinition>> {
    // Parse the content to extract name and other metadata
    let (name, agent) = parse_agent_content(&req.content, &req.format)?;

    // Determine directory
    let dir = if req.scope == "project" {
        std::path::PathBuf::from(".factory/agents")
    } else {
        dirs::home_dir()
            .ok_or_else(|| AppError::Internal("Cannot find home directory".to_string()))?
            .join(".factory/agents")
    };

    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::Internal(format!("Failed to create directory: {}", e)))?;

    let path = dir.join(format!("{}.md", name));

    // Write the file
    std::fs::write(&path, &req.content)
        .map_err(|e| AppError::Internal(format!("Failed to write agent file: {}", e)))?;

    Ok(Json(AgentDefinition {
        name,
        description: agent.description,
        tools: agent.tools,
        model: agent.model,
        permission_mode: agent.permission_mode,
        prompt: agent.prompt,
        scope: req.scope,
    }))
}

/// Generate an agent prompt using AI via cortex-core.
/// NOTE: This endpoint is temporarily disabled as the providers module was removed.
pub async fn generate_agent_prompt(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<GeneratePromptRequest>,
) -> AppResult<Json<GeneratePromptResponse>> {
    // Return a stub response - providers module was removed
    let name = req.name.unwrap_or_else(|| "custom-agent".to_string());
    Ok(Json(GeneratePromptResponse {
        name: name.clone(),
        description: format!("Agent for: {}", req.description),
        prompt: format!(
            "You are an AI assistant specialized in: {}\n\nPlease help the user with tasks related to this domain.",
            req.description
        ),
        tools: if req.tools.is_empty() {
            vec![
                "Read".to_string(),
                "Create".to_string(),
                "Edit".to_string(),
                "Execute".to_string(),
            ]
        } else {
            req.tools
        },
        model: "inherit".to_string(),
        permission_mode: "default".to_string(),
    }))
}
