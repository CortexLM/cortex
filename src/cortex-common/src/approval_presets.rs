//! Preset configurations for approval policies.

use cortex_protocol::{AskForApproval, SandboxPolicy};

/// Preset approval configuration.
#[derive(Debug, Clone)]
pub struct ApprovalPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub approval_policy: AskForApproval,
    pub sandbox_policy: SandboxPolicy,
}

/// Available presets.
pub const PRESETS: &[ApprovalPreset] = &[
    ApprovalPreset {
        name: "suggest",
        description: "Agent suggests commands, you approve each one",
        approval_policy: AskForApproval::UnlessTrusted,
        sandbox_policy: SandboxPolicy::ReadOnly,
    },
    ApprovalPreset {
        name: "auto-edit",
        description: "Agent can edit files, asks for shell commands",
        approval_policy: AskForApproval::OnRequest,
        sandbox_policy: SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        },
    },
    ApprovalPreset {
        name: "full-auto",
        description: "Agent runs autonomously in sandbox, escalates on failure",
        approval_policy: AskForApproval::OnFailure,
        sandbox_policy: SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![],
            network_access: true,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        },
    },
];

/// Get a preset by name.
pub fn get_preset(name: &str) -> Option<&'static ApprovalPreset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// Get the default preset.
pub fn default_preset() -> &'static ApprovalPreset {
    &PRESETS[1] // auto-edit
}
