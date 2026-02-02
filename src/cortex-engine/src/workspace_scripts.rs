use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::{CortexError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ScriptType {
    Setup,
    Run,
    Archive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    Concurrent,
    NonConcurrent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceScript {
    pub script_type: ScriptType,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub run_mode: RunMode,
}

fn default_timeout() -> u64 {
    30
}

impl Default for RunMode {
    fn default() -> Self {
        Self::NonConcurrent
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceScriptsConfig {
    #[serde(default)]
    pub scripts: Vec<WorkspaceScript>,
}

impl WorkspaceScriptsConfig {
    pub fn load_from_file(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)?;

        let config: Self = serde_json::from_str(&content).map_err(|e| {
            CortexError::InvalidInput(format!("Failed to parse scripts config: {e}"))
        })?;

        Ok(config)
    }

    pub fn get_script(&self, script_type: ScriptType) -> Option<&WorkspaceScript> {
        self.scripts.iter().find(|s| s.script_type == script_type)
    }
}

impl Default for WorkspaceScriptsConfig {
    fn default() -> Self {
        Self { scripts: vec![] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_type_serde() {
        let setup = serde_json::to_string(&ScriptType::Setup).unwrap();
        assert_eq!(setup, r#""setup""#);

        let parsed: ScriptType = serde_json::from_str(r#""run""#).unwrap();
        assert_eq!(parsed, ScriptType::Run);
    }

    #[test]
    fn test_workspace_script_default() {
        let script = WorkspaceScript {
            script_type: ScriptType::Setup,
            command: "npm".to_string(),
            args: vec!["install".to_string()],
            env: HashMap::new(),
            timeout_secs: 30,
            run_mode: RunMode::default(),
        };

        assert_eq!(script.timeout_secs, 30);
        assert_eq!(script.run_mode, RunMode::NonConcurrent);
    }

    #[test]
    fn test_config_get_script() {
        let config = WorkspaceScriptsConfig {
            scripts: vec![
                WorkspaceScript {
                    script_type: ScriptType::Setup,
                    command: "npm".to_string(),
                    args: vec!["install".to_string()],
                    env: HashMap::new(),
                    timeout_secs: 30,
                    run_mode: RunMode::NonConcurrent,
                },
                WorkspaceScript {
                    script_type: ScriptType::Run,
                    command: "npm".to_string(),
                    args: vec!["run", "dev"].iter().map(|s| s.to_string()).collect(),
                    env: HashMap::new(),
                    timeout_secs: 0,
                    run_mode: RunMode::Concurrent,
                },
            ],
        };

        let setup = config.get_script(ScriptType::Setup);
        assert!(setup.is_some());
        assert_eq!(setup.unwrap().command, "npm");

        let archive = config.get_script(ScriptType::Archive);
        assert!(archive.is_none());
    }
}
