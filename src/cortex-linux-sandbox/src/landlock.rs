//! Landlock filesystem isolation.
//!
//! Applies Landlock rules to restrict filesystem access:
//! - Read-only access to the entire filesystem
//! - Write access to /dev/null
//! - Write access to specified writable roots

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use landlock::{
    Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetStatus, ABI,
};

/// Apply Landlock filesystem rules.
///
/// This function:
/// 1. Allows read access to the entire filesystem
/// 2. Allows write access to /dev/null (required for many commands)
/// 3. Allows write access to the specified writable roots
pub fn apply_filesystem_rules(writable_roots: &[PathBuf]) -> Result<()> {
    let abi = ABI::V5;
    let access_rw = AccessFs::from_all(abi);
    let access_ro = AccessFs::from_read(abi);

    // Start with read-only access to /
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(access_rw)?
        .create()?
        .add_rules(landlock::path_beneath_rules(&["/"], access_ro))?
        // Always allow write to /dev/null
        .add_rules(landlock::path_beneath_rules(&["/dev/null"], access_rw))?
        .set_no_new_privs(true);

    // Add writable roots
    if !writable_roots.is_empty() {
        let root_refs: Vec<&PathBuf> = writable_roots.iter().collect();
        ruleset = ruleset.add_rules(landlock::path_beneath_rules(&root_refs, access_rw))?;
    }

    // Apply the ruleset
    let status = ruleset.restrict_self()?;

    if status.ruleset == RulesetStatus::NotEnforced {
        return Err(anyhow!("Landlock ruleset not enforced"));
    }

    tracing::debug!("Landlock rules applied successfully");
    Ok(())
}

/// Check if Landlock is available on this system.
#[allow(dead_code)]
pub fn is_landlock_available() -> bool {
    let abi = ABI::V5;
    let status = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(abi))
        .map(|r| r.create())
        .ok()
        .and_then(|r| r.ok());

    status.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_landlock_availability_check() {
        // This test just verifies the function doesn't panic
        let _ = is_landlock_available();
    }
}
