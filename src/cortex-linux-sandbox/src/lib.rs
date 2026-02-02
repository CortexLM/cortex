//! Linux sandbox library.
//!
//! Provides Landlock filesystem isolation and seccomp network filtering
//! for sandboxed command execution.

#[cfg(target_os = "linux")]
mod landlock;
#[cfg(target_os = "linux")]
mod mounts;
#[cfg(target_os = "linux")]
mod run_main;
#[cfg(target_os = "linux")]
mod seccomp;

#[cfg(target_os = "linux")]
pub use landlock::apply_filesystem_rules;
#[cfg(target_os = "linux")]
pub use mounts::apply_read_only_mounts;
#[cfg(target_os = "linux")]
pub use seccomp::apply_network_filter;

/// Run the sandbox main function.
#[cfg(target_os = "linux")]
pub fn run_main() -> ! {
    run_main::run_main()
}

#[cfg(not(target_os = "linux"))]
pub fn run_main() -> ! {
    eprintln!("cortex-linux-sandbox is only supported on Linux");
    std::process::exit(1)
}
