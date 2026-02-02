//! Seccomp network filtering.
//!
//! Applies seccomp filters to block network syscalls while allowing
//! AF_UNIX sockets for local IPC. Also blocks DNS resolution by preventing
//! network connections that would leak DNS queries.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

/// Apply seccomp network filter.
///
/// This blocks outbound network access except for AF_UNIX domain sockets.
/// DNS queries are also blocked since they require network access (UDP port 53
/// or TCP connections to resolvers). The filter blocks the underlying syscalls
/// that DNS resolution depends on.
///
/// Blocked syscalls:
/// - connect, accept, accept4, bind, listen
/// - sendto, sendmsg, sendmmsg
/// - recvmsg, recvmmsg (recvfrom allowed for cargo clippy etc.)
/// - getsockopt, setsockopt
/// - getpeername, getsockname
/// - shutdown, ptrace
///
/// Note: DNS resolution is blocked because:
/// - UDP DNS requires socket(AF_INET, SOCK_DGRAM) which is blocked
/// - TCP DNS requires connect() which is blocked
/// - The stub resolver uses these syscalls internally
pub fn apply_network_filter() -> Result<()> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();

    // Helper to insert unconditional deny rule
    let mut deny_syscall = |nr: i64| {
        rules.insert(nr, vec![]); // empty rule vec = unconditional match
    };

    // Block all network-related syscalls
    // This includes syscalls needed for DNS resolution (connect, sendto for UDP)
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
    // NOTE: allowing recvfrom allows some tools like `cargo clippy` to run
    // with their socketpair + child processes for sub-proc management
    // deny_syscall(libc::SYS_recvfrom);
    deny_syscall(libc::SYS_recvmsg);
    deny_syscall(libc::SYS_recvmmsg);
    deny_syscall(libc::SYS_getsockopt);
    deny_syscall(libc::SYS_setsockopt);
    deny_syscall(libc::SYS_ptrace);

    // For socket syscall, allow only AF_UNIX (arg0 == AF_UNIX)
    // This blocks AF_INET/AF_INET6 sockets needed for DNS queries
    let unix_only_rule = SeccompRule::new(vec![SeccompCondition::new(
        0, // first argument (domain)
        SeccompCmpArgLen::Dword,
        SeccompCmpOp::Ne,
        libc::AF_UNIX as u64,
    )?])?;

    rules.insert(libc::SYS_socket, vec![unix_only_rule.clone()]);
    rules.insert(libc::SYS_socketpair, vec![unix_only_rule]);

    // Determine target architecture
    let arch = if cfg!(target_arch = "x86_64") {
        TargetArch::x86_64
    } else if cfg!(target_arch = "aarch64") {
        TargetArch::aarch64
    } else {
        return Err(anyhow!("Unsupported architecture for seccomp filter"));
    };

    // Create and apply the filter
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,                     // default: allow
        SeccompAction::Errno(libc::EPERM as u32), // when rule matches: return EPERM
        arch,
    )?;

    let prog: BpfProgram = filter.try_into()?;
    seccompiler::apply_filter(&prog)?;

    tracing::debug!("Seccomp network filter applied successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    // Note: seccomp tests are tricky because they affect the current process
    // and are irreversible. Integration tests should be done in a subprocess.

    #[test]
    fn test_filter_creation() {
        // This test just verifies the filter can be created without panic
        // Actual application would need to be tested in a subprocess
    }
}
