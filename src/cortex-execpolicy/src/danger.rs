//! Danger detection types and categories.

use serde::{Deserialize, Serialize};

/// Category of dangerous command detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DangerCategory {
    /// Destructive file operations (rm -rf /)
    DestructiveFileOp,
    /// Disk/device operations (dd, fdisk, mkfs)
    DiskOperation,
    /// Privilege escalation (sudo, doas, su)
    PrivilegeEscalation,
    /// Fork bomb or resource exhaustion
    ForkBomb,
    /// Remote code execution (curl | sh)
    RemoteCodeExecution,
    /// Insecure permissions (chmod 777)
    InsecurePermissions,
    /// System service modification
    SystemServiceMod,
    /// Network exposure (nc -l, http.server)
    NetworkExposure,
    /// Credential/secret access
    CredentialAccess,
    /// Kernel/system modification
    KernelModification,
    /// Container escape attempt
    ContainerEscape,
    /// History manipulation
    HistoryManipulation,
    /// Crypto mining potential
    CryptoMining,
    /// Custom rule match
    CustomRule,
}

impl std::fmt::Display for DangerCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DestructiveFileOp => write!(f, "destructive file operation"),
            Self::DiskOperation => write!(f, "disk/device operation"),
            Self::PrivilegeEscalation => write!(f, "privilege escalation"),
            Self::ForkBomb => write!(f, "fork bomb / resource exhaustion"),
            Self::RemoteCodeExecution => write!(f, "remote code execution"),
            Self::InsecurePermissions => write!(f, "insecure permissions"),
            Self::SystemServiceMod => write!(f, "system service modification"),
            Self::NetworkExposure => write!(f, "network exposure"),
            Self::CredentialAccess => write!(f, "credential/secret access"),
            Self::KernelModification => write!(f, "kernel/system modification"),
            Self::ContainerEscape => write!(f, "container escape attempt"),
            Self::HistoryManipulation => write!(f, "history manipulation"),
            Self::CryptoMining => write!(f, "potential crypto mining"),
            Self::CustomRule => write!(f, "custom policy rule"),
        }
    }
}

/// Result of dangerous command detection.
#[derive(Debug, Clone)]
pub struct DangerDetection {
    /// Whether the command is dangerous.
    pub is_dangerous: bool,

    /// Categories of danger detected.
    pub categories: Vec<DangerCategory>,

    /// Human-readable reason for the detection.
    pub reason: String,

    /// Severity level (1-10, 10 being most severe).
    pub severity: u8,

    /// Whether the danger can be mitigated by context (e.g., container).
    pub context_mitigatable: bool,
}

impl DangerDetection {
    /// Create a safe (non-dangerous) detection result.
    pub fn safe() -> Self {
        Self {
            is_dangerous: false,
            categories: vec![],
            reason: String::new(),
            severity: 0,
            context_mitigatable: false,
        }
    }

    /// Create a dangerous detection result.
    pub fn dangerous(
        category: DangerCategory,
        reason: impl Into<String>,
        severity: u8,
        context_mitigatable: bool,
    ) -> Self {
        Self {
            is_dangerous: true,
            categories: vec![category],
            reason: reason.into(),
            severity: severity.min(10),
            context_mitigatable,
        }
    }

    /// Add another danger category.
    pub fn add_category(&mut self, category: DangerCategory, reason: &str) {
        if !self.categories.contains(&category) {
            self.categories.push(category);
            if !self.reason.is_empty() {
                self.reason.push_str("; ");
            }
            self.reason.push_str(reason);
        }
    }
}
