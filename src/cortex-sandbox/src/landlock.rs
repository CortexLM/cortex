//! Linux Landlock sandbox implementation.
//!
//! Features:
//! - Uses ABI::V5 with CompatLevel::BestEffort for maximum compatibility
//! - Always allows write access to /dev/null (required for many commands)
//! - Sets no_new_privs for security
//! - Applies seccomp FIRST, then Landlock (order matters!)
//! - Uses path_beneath_rules() helper

use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr, RulesetCreated,
    RulesetCreatedAttr, RulesetStatus,
};
use std::path::Path;

use crate::SandboxBackend;

/// Landlock sandbox backend.
pub struct LandlockSandbox {
    available: bool,
}

impl LandlockSandbox {
    /// Create a new Landlock sandbox.
    pub fn new() -> Self {
        Self {
            available: Self::check_available(),
        }
    }

    fn check_available() -> bool {
        let abi = ABI::V5;
        let status = Ruleset::default()
            .set_compatibility(CompatLevel::BestEffort)
            .handle_access(AccessFs::from_all(abi))
            .map(landlock::Ruleset::create)
            .ok()
            .and_then(std::result::Result::ok);

        matches!(status, Some(RulesetCreated { .. }))
    }

    /// Apply Landlock rules for sandbox.
    /// Installs filesystem isolation rules on the current thread.
    pub fn apply(
        &self,
        writable_roots: &[&Path],
        _read_only_roots: &[&Path],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let abi = ABI::V5;
        let access_rw = AccessFs::from_all(abi);
        let access_ro = AccessFs::from_read(abi);

        // Create ruleset with read-only root and writable /dev/null
        let mut ruleset = Ruleset::default()
            .set_compatibility(CompatLevel::BestEffort)
            .handle_access(access_rw)?
            .create()?
            .add_rules(landlock::path_beneath_rules(&["/"], access_ro))?
            .add_rules(landlock::path_beneath_rules(&["/dev/null"], access_rw))?
            .set_no_new_privs(true);

        // Add write access to writable roots (if any)
        if !writable_roots.is_empty() {
            ruleset = ruleset.add_rules(landlock::path_beneath_rules(writable_roots, access_rw))?;
        }

        let status = ruleset.restrict_self()?;

        if status.ruleset == RulesetStatus::NotEnforced {
            return Err("Landlock not enforced".into());
        }

        Ok(())
    }

    /// Apply sandbox with network filtering.
    /// Applies sandbox policy to the current thread:
    /// - Applies seccomp first (if network disabled)
    /// - Then applies Landlock
    pub fn apply_with_network_filter(
        &self,
        writable_roots: &[&Path],
        _read_only_roots: &[&Path],
        allow_network: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Apply seccomp first, then Landlock (order matters for security)
        if !allow_network {
            apply_seccomp_network_filter()?;
        }

        self.apply(writable_roots, _read_only_roots)?;

        Ok(())
    }
}

impl Default for LandlockSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxBackend for LandlockSandbox {
    fn name(&self) -> &str {
        "landlock"
    }

    fn is_available(&self) -> bool {
        self.available
    }
}

/// Installs a seccomp filter that blocks outbound network access except for
/// AF_UNIX domain sockets.
/// Blocks network-related syscalls to prevent unauthorized network access.
fn apply_seccomp_network_filter() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
        SeccompRule, TargetArch,
    };
    use std::collections::BTreeMap;

    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();

    let mut deny_syscall = |nr: i64| {
        rules.insert(nr, vec![]);
    };

    // Block network-related syscalls
    deny_syscall(libc::SYS_connect);
    deny_syscall(libc::SYS_accept);
    deny_syscall(libc::SYS_accept4);
    deny_syscall(libc::SYS_bind);
    deny_syscall(libc::SYS_listen);
    deny_syscall(libc::SYS_getpeername);
    deny_syscall(libc::SYS_getsockname);
    deny_syscall(libc::SYS_shutdown);
    deny_syscall(libc::SYS_sendto);
    deny_syscall(libc::SYS_sendmsg);
    deny_syscall(libc::SYS_sendmmsg);
    // NOTE: allowing recvfrom allows some tools like: `cargo clippy` to run
    // with their socketpair + child processes for sub-proc management
    // deny_syscall(libc::SYS_recvfrom);  // INTENTIONALLY NOT BLOCKED (needed for cargo clippy etc.)
    deny_syscall(libc::SYS_recvmsg);
    deny_syscall(libc::SYS_recvmmsg);
    deny_syscall(libc::SYS_getsockopt);
    deny_syscall(libc::SYS_setsockopt);
    deny_syscall(libc::SYS_ptrace);

    // For socket syscall, allow only AF_UNIX
    let unix_only_rule = SeccompRule::new(vec![SeccompCondition::new(
        0,
        SeccompCmpArgLen::Dword,
        SeccompCmpOp::Ne,
        libc::AF_UNIX as u64,
    )?])?;

    rules.insert(libc::SYS_socket, vec![unix_only_rule.clone()]);
    rules.insert(libc::SYS_socketpair, vec![unix_only_rule]);

    let arch = if cfg!(target_arch = "x86_64") {
        TargetArch::x86_64
    } else if cfg!(target_arch = "aarch64") {
        TargetArch::aarch64
    } else {
        return Err("Unsupported architecture for seccomp filter".into());
    };

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )?;

    let prog: BpfProgram = filter.try_into()?;
    seccompiler::apply_filter(&prog)?;

    Ok(())
}
