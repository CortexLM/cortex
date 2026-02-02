//! Trusted workspace management.

use anyhow::Result;
use std::path::PathBuf;

// ============================================================================
// Trusted Workspaces
// ============================================================================

/// Check if a workspace is already trusted.
pub fn is_workspace_trusted(workspace: &std::path::Path) -> bool {
    let trusted_file = match dirs::home_dir() {
        Some(home) => home.join(".cortex").join("trusted_workspaces.json"),
        None => return false,
    };

    if !trusted_file.exists() {
        return false;
    }

    let content = match std::fs::read_to_string(&trusted_file) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let data: serde_json::Value = match serde_json::from_str(&content) {
        Ok(d) => d,
        Err(_) => return false,
    };

    let workspace_str = workspace.to_string_lossy();
    if let Some(trusted) = data.get("trusted").and_then(|t| t.as_array()) {
        return trusted
            .iter()
            .filter_map(|v| v.as_str())
            .any(|path| path == workspace_str);
    }

    false
}

/// Mark a workspace as trusted.
pub fn mark_workspace_trusted(workspace: &std::path::Path) -> Result<()> {
    let cortex_dir = dirs::home_dir()
        .map(|h| h.join(".cortex"))
        .unwrap_or_else(|| PathBuf::from(".cortex"));

    std::fs::create_dir_all(&cortex_dir)?;

    let trusted_file = cortex_dir.join("trusted_workspaces.json");
    let mut trusted: Vec<String> = Vec::new();

    if trusted_file.exists()
        && let Ok(content) = std::fs::read_to_string(&trusted_file)
        && let Ok(data) = serde_json::from_str::<serde_json::Value>(&content)
        && let Some(arr) = data.get("trusted").and_then(|t| t.as_array())
    {
        trusted = arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }

    let workspace_str = workspace.to_string_lossy().to_string();
    if !trusted.contains(&workspace_str) {
        trusted.push(workspace_str);
    }

    let data = serde_json::json!({
        "version": 1,
        "trusted": trusted
    });

    std::fs::write(&trusted_file, serde_json::to_string_pretty(&data)?)?;
    Ok(())
}
