//! Build script for cortex-cli.
//!
//! Captures build-time information (git commit, build date) for --version output.

use std::process::Command;

fn main() {
    // Capture git commit hash
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Capture build date (UTC)
    let build_date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // Emit cargo instructions
    println!("cargo:rustc-env=CORTEX_GIT_HASH={}", git_hash);
    println!("cargo:rustc-env=CORTEX_BUILD_DATE={}", build_date);

    // Rerun if git HEAD changes
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/heads");
}
