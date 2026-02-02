//! System command handler.

use anyhow::Result;

use crate::debug_cmd::commands::SystemArgs;
use crate::debug_cmd::types::{
    CortexInfo, EnvironmentInfo, HardwareInfo, OsInfo, SystemDebugOutput,
};
use crate::debug_cmd::utils::{
    format_size, get_available_memory, get_cortex_home_or_default, get_os_version,
    get_rust_version, get_user_info,
};

/// Run the system debug command.
pub async fn run_system(args: SystemArgs) -> Result<()> {
    // Gather OS information
    let os_name = std::env::consts::OS.to_string();
    let os_family = std::env::consts::FAMILY.to_string();

    // Try to get OS version
    let os_version = get_os_version().await;

    let os = OsInfo {
        name: os_name,
        version: os_version,
        family: os_family,
    };

    // Gather hardware information
    let arch = std::env::consts::ARCH.to_string();
    let cpu_cores = std::thread::available_parallelism().map(|p| p.get()).ok();

    // Get memory info, considering container limits
    let (total_memory_bytes, container_memory_limit) = get_available_memory();
    let total_memory = total_memory_bytes.map(format_size);

    let hardware = HardwareInfo {
        arch,
        cpu_cores,
        total_memory_bytes,
        total_memory,
        container_memory_limit,
    };

    // Gather environment information
    let shell = std::env::var("SHELL")
        .ok()
        .or_else(|| std::env::var("COMSPEC").ok());
    let home_dir = dirs::home_dir();
    let current_dir = std::env::current_dir().ok();

    // Get username - fallback to UID string if not found (container environments)
    let (user, uid) = get_user_info();
    let term = std::env::var("TERM").ok();

    let environment = EnvironmentInfo {
        shell,
        home_dir,
        current_dir,
        user,
        uid,
        term,
    };

    // Gather Cortex-specific information
    let version = env!("CARGO_PKG_VERSION").to_string();
    let cortex_home = get_cortex_home_or_default();
    let rust_version = get_rust_version().await;

    let cortex = CortexInfo {
        version,
        cortex_home,
        rust_version,
    };

    let output = SystemDebugOutput {
        os,
        hardware,
        environment,
        cortex,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("System Information");
        println!("{}", "=".repeat(50));

        println!();
        println!("Operating System");
        println!("{}", "-".repeat(40));
        println!("  Name:    {}", output.os.name);
        if let Some(ref version) = output.os.version {
            println!("  Version: {}", version);
        }
        println!("  Family:  {}", output.os.family);

        println!();
        println!("Hardware");
        println!("{}", "-".repeat(40));
        println!("  Architecture: {}", output.hardware.arch);
        if let Some(cores) = output.hardware.cpu_cores {
            println!("  CPU Cores:    {}", cores);
        }
        if let Some(ref mem) = output.hardware.total_memory {
            let mem_note = if output.hardware.container_memory_limit == Some(true) {
                " (container limit)"
            } else {
                ""
            };
            println!("  RAM:          {}{}", mem, mem_note);
        }

        println!();
        println!("Environment");
        println!("{}", "-".repeat(40));
        if let Some(ref shell) = output.environment.shell {
            println!("  Shell:       {}", shell);
        }
        if let Some(ref home) = output.environment.home_dir {
            println!("  Home Dir:    {}", home.display());
        }
        if let Some(ref cwd) = output.environment.current_dir {
            println!("  Current Dir: {}", cwd.display());
        }
        if let Some(ref user) = output.environment.user {
            println!("  User:        {}", user);
        }
        if let Some(uid) = output.environment.uid {
            println!("  UID:         {}", uid);
        }
        if let Some(ref term) = output.environment.term {
            println!("  Terminal:    {}", term);
        }

        println!();
        println!("Cortex");
        println!("{}", "-".repeat(40));
        println!("  Version:     {}", output.cortex.version);
        println!("  Cortex Home: {}", output.cortex.cortex_home.display());
        if let Some(ref rust_ver) = output.cortex.rust_version {
            println!("  Rust:        {}", rust_ver);
        }
    }

    Ok(())
}
