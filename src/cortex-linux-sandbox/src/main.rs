//! Linux sandbox wrapper binary.
//!
//! This binary applies Landlock filesystem restrictions and seccomp network
//! filtering before executing the target command.
//!
//! Usage:
//!   cortex-linux-sandbox --sandbox-policy-cwd /path/to/cwd \
//!                        --sandbox-policy '{"type":"WorkspaceWrite",...}' \
//!                        -- command arg1 arg2

fn main() -> ! {
    cortex_linux_sandbox::run_main()
}
