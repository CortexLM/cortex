//! Paths command handler.

use anyhow::Result;
use std::path::PathBuf;

use crate::debug_cmd::commands::PathsArgs;
use crate::debug_cmd::types::{PathInfo, PathsDebugOutput};
use crate::debug_cmd::utils::{format_size, get_cortex_home};

/// Run the paths debug command.
pub async fn run_paths(args: PathsArgs) -> Result<()> {
    // Use catch_unwind to handle potential panics from path resolution
    // which can occur with malformed XDG_DATA_HOME or similar env vars (#2002)
    let cortex_home = std::panic::catch_unwind(get_cortex_home).unwrap_or_else(|_| {
        eprintln!("Warning: Failed to determine cortex home directory.");
        eprintln!(
            "This may be caused by invalid XDG_DATA_HOME or CORTEX_HOME environment variables."
        );
        PathBuf::from(".cortex")
    });

    // Handle --check-writable flag for Docker read-only container validation
    if args.check_writable {
        if let Some(warning) = cortex_engine::file_utils::check_write_permissions(&cortex_home) {
            if args.json {
                let statuses = cortex_engine::file_utils::validate_write_locations(&cortex_home);
                let output: Vec<_> = statuses
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "path": s.path,
                            "description": s.description,
                            "is_writable": s.is_writable,
                            "error": s.error,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                eprintln!("{warning}");
            }
            std::process::exit(1);
        } else {
            if args.json {
                let statuses = cortex_engine::file_utils::validate_write_locations(&cortex_home);
                let output: Vec<_> = statuses
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "path": s.path,
                            "description": s.description,
                            "is_writable": s.is_writable,
                            "error": s.error,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!("✓ All write locations are accessible.");
            }
            return Ok(());
        }
    }

    let output = PathsDebugOutput {
        cortex_home: PathInfo::new(cortex_home.clone()),
        config_dir: PathInfo::new(cortex_home.clone()),
        data_dir: PathInfo::new(cortex_home.clone()),
        cache_dir: PathInfo::new(cortex_home.join("cache")),
        sessions_dir: PathInfo::new(cortex_home.join("sessions")),
        plugins_dir: PathInfo::new(cortex_home.join("plugins")),
        skills_dir: PathInfo::new(cortex_home.join("skills")),
        agents_dir: PathInfo::new(cortex_home.join("agents")),
        mcp_dir: PathInfo::new(cortex_home.join("mcp")),
        logs_dir: PathInfo::new(cortex_home.join("logs")),
        snapshots_dir: PathInfo::new(cortex_home.join("snapshots")),
        temp_dir: PathInfo::new(std::env::temp_dir().join("Cortex")),
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Cortex Paths");
        println!("{}", "=".repeat(70));
        println!("{:<20} {:<40} {:>8}", "Directory", "Path", "Status");
        println!("{}", "-".repeat(70));

        let paths = [
            ("Cortex Home", &output.cortex_home),
            ("Config", &output.config_dir),
            ("Data", &output.data_dir),
            ("Cache", &output.cache_dir),
            ("Sessions", &output.sessions_dir),
            ("Plugins", &output.plugins_dir),
            ("Skills", &output.skills_dir),
            ("Agents", &output.agents_dir),
            ("MCP", &output.mcp_dir),
            ("Logs", &output.logs_dir),
            ("Snapshots", &output.snapshots_dir),
            ("Temp", &output.temp_dir),
        ];

        for (name, info) in paths {
            let status = if info.exists { "exists" } else { "missing" };
            let path_display = info.path.display().to_string();
            let path_truncated = if path_display.len() > 38 {
                format!("...{}", &path_display[path_display.len() - 35..])
            } else {
                path_display
            };
            println!("{:<20} {:<40} {:>8}", name, path_truncated, status);
            if let Some(size) = info.size_bytes
                && size > 0
            {
                println!("{:>62}", format!("({} )", format_size(size)));
            }
        }
    }

    Ok(())
}
