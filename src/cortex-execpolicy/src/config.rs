//! Policy configuration.

use serde::{Deserialize, Serialize};

/// Configuration for the policy engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    /// Sensitive filesystem paths that require extra protection.
    pub sensitive_paths: Vec<String>,

    /// Block device patterns (for dd, fdisk protection).
    pub block_device_patterns: Vec<String>,

    /// Network listening ports that are considered dangerous.
    pub dangerous_ports: Vec<u16>,

    /// Maximum permission bits that don't trigger warnings.
    pub max_safe_chmod: u32,

    /// Whether to allow privilege escalation commands.
    pub allow_privilege_escalation: bool,

    /// Whether to allow system service modifications.
    pub allow_service_modifications: bool,

    /// Custom dangerous command patterns.
    pub custom_dangerous_patterns: Vec<String>,

    /// Custom safe command patterns (override defaults).
    pub custom_safe_patterns: Vec<String>,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            sensitive_paths: vec![
                "/".to_string(),
                "/etc".to_string(),
                "/etc/passwd".to_string(),
                "/etc/shadow".to_string(),
                "/etc/sudoers".to_string(),
                "/etc/ssh".to_string(),
                "/root".to_string(),
                "/boot".to_string(),
                "/var".to_string(),
                "/usr".to_string(),
                "/bin".to_string(),
                "/sbin".to_string(),
                "/lib".to_string(),
                "/lib64".to_string(),
                "/sys".to_string(),
                "/proc".to_string(),
                "/home".to_string(),
                "~".to_string(),
                "C:\\Windows".to_string(),
                "C:\\Program Files".to_string(),
                "C:\\Users".to_string(),
            ],
            block_device_patterns: vec![
                "/dev/sd".to_string(),
                "/dev/hd".to_string(),
                "/dev/nvme".to_string(),
                "/dev/vd".to_string(),
                "/dev/xvd".to_string(),
                "/dev/loop".to_string(),
                "/dev/md".to_string(),
                "/dev/dm-".to_string(),
                "/dev/mapper/".to_string(),
                "\\\\.\\PhysicalDrive".to_string(),
            ],
            dangerous_ports: vec![21, 22, 23, 25, 53, 80, 443, 3389, 5900],
            max_safe_chmod: 0o755,
            allow_privilege_escalation: false,
            allow_service_modifications: false,
            custom_dangerous_patterns: vec![],
            custom_safe_patterns: vec![],
        }
    }
}
