//! TUI Screenshot Generator Binary
//!
//! Generates comprehensive screenshots of ALL possible TUI states for debugging.
//!
//! ## Usage
//!
//! ```bash
//! # Generate all screenshots to ./tui-screenshots
//! cargo run --bin generate_tui_screenshots
//!
//! # Generate to custom directory
//! cargo run --bin generate_tui_screenshots -- --output ./my-screenshots
//!
//! # Generate only specific categories
//! cargo run --bin generate_tui_screenshots -- --categories views,modals
//!
//! # Specify terminal size
//! cargo run --bin generate_tui_screenshots -- --width 100 --height 30
//! ```

use cortex_tui_capture::screenshot_generator::{GeneratorConfig, ScreenshotGenerator};
use std::env;
use std::path::PathBuf;

fn print_help() {
    println!(
        r#"
TUI Screenshot Generator - Generate comprehensive TUI state screenshots

USAGE:
    generate_tui_screenshots [OPTIONS]

OPTIONS:
    -o, --output <DIR>       Output directory (default: ./tui-screenshots)
    -w, --width <WIDTH>      Terminal width (default: 120)
    -h, --height <HEIGHT>    Terminal height (default: 40)
    -c, --categories <LIST>  Comma-separated list of categories to generate
    --no-index               Don't generate index file
    --list-categories        List all available categories
    --list-scenarios         List all available scenarios
    --help                   Show this help message

EXAMPLES:
    # Generate all screenshots
    generate_tui_screenshots

    # Generate to specific directory
    generate_tui_screenshots --output ./docs/screenshots

    # Generate only autocomplete and modal screenshots
    generate_tui_screenshots --categories autocomplete,modals

    # List available categories
    generate_tui_screenshots --list-categories
"#
    );
}

fn parse_args() -> Result<GeneratorConfig, String> {
    let args: Vec<String> = env::args().collect();
    let mut config = GeneratorConfig::default();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" => {
                print_help();
                std::process::exit(0);
            }
            "--list-categories" => {
                let generator = ScreenshotGenerator::new();
                println!("Available categories:");
                for cat in generator.categories() {
                    let count = generator.scenarios_by_category(&cat).len();
                    println!("  {} ({} scenarios)", cat, count);
                }
                std::process::exit(0);
            }
            "--list-scenarios" => {
                let generator = ScreenshotGenerator::new();
                println!("Available scenarios:\n");
                for cat in generator.categories() {
                    println!("== {} ==", cat.to_uppercase());
                    for scenario in generator.scenarios_by_category(&cat) {
                        println!("  {} - {}", scenario.id, scenario.description);
                    }
                    println!();
                }
                std::process::exit(0);
            }
            "-o" | "--output" => {
                i += 1;
                if i >= args.len() {
                    return Err("Missing value for --output".to_string());
                }
                config.output_dir = PathBuf::from(&args[i]);
            }
            "-w" | "--width" => {
                i += 1;
                if i >= args.len() {
                    return Err("Missing value for --width".to_string());
                }
                config.width = args[i]
                    .parse()
                    .map_err(|_| "Invalid width value".to_string())?;
            }
            "-h" | "--height" => {
                i += 1;
                if i >= args.len() {
                    return Err("Missing value for --height".to_string());
                }
                config.height = args[i]
                    .parse()
                    .map_err(|_| "Invalid height value".to_string())?;
            }
            "-c" | "--categories" => {
                i += 1;
                if i >= args.len() {
                    return Err("Missing value for --categories".to_string());
                }
                config.categories = args[i].split(',').map(|s| s.trim().to_string()).collect();
            }
            "--no-index" => {
                config.generate_index = false;
            }
            arg => {
                return Err(format!("Unknown argument: {}", arg));
            }
        }
        i += 1;
    }

    Ok(config)
}

#[tokio::main]
async fn main() {
    // Parse command line arguments
    let config = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!("\nRun with --help for usage information.");
            std::process::exit(1);
        }
    };

    println!("╭─────────────────────────────────────────────────────────────╮");
    println!("│           TUI Screenshot Generator                          │");
    println!("╰─────────────────────────────────────────────────────────────╯");
    println!();
    println!("Configuration:");
    println!("  Output:     {:?}", config.output_dir);
    println!("  Size:       {}x{}", config.width, config.height);
    if !config.categories.is_empty() {
        println!("  Categories: {}", config.categories.join(", "));
    } else {
        println!("  Categories: all");
    }
    println!();

    // Create generator
    let generator = ScreenshotGenerator::with_config(config);

    println!(
        "Registered {} scenarios across {} categories",
        generator.scenarios().len(),
        generator.categories().len()
    );
    println!();
    println!("Generating screenshots...");
    println!();

    // Generate all screenshots
    match generator.generate_all().await {
        Ok(result) => {
            println!();
            println!("╭─────────────────────────────────────────────────────────────╮");
            println!("│                    Generation Complete!                     │");
            println!("╰─────────────────────────────────────────────────────────────╯");
            println!();
            println!("  ✓ Generated {} screenshots", result.success);

            if !result.failed.is_empty() {
                println!("  ✗ Failed: {}", result.failed.len());
                for (id, error) in &result.failed {
                    println!("    - {}: {}", id, error);
                }
            }

            println!();
            println!("  Output directory: {:?}", result.output_dir);
            println!();
            println!("Categories generated:");

            // Count files per category
            let mut cat_counts: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for path in &result.files {
                if let Some(parent) = path.parent()
                    && let Some(cat_name) = parent.file_name()
                {
                    *cat_counts
                        .entry(cat_name.to_string_lossy().to_string())
                        .or_insert(0) += 1;
                }
            }

            for (cat, count) in &cat_counts {
                println!("  - {}: {} files", cat, count);
            }

            println!();
            println!(
                "View the index at: {:?}",
                result.output_dir.join("README.md")
            );
        }
        Err(e) => {
            eprintln!("Error generating screenshots: {:?}", e);
            std::process::exit(1);
        }
    }
}
