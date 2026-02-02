//! LSP-related tool executors (diagnostics, hover, symbols).

use serde_json::Value;

use crate::error::Result;
use crate::tools::registry::ToolRegistry;
use crate::tools::spec::ToolResult;

impl ToolRegistry {
    pub(crate) async fn execute_lsp_diagnostics(&self, args: Value) -> Result<ToolResult> {
        let path = args.get("path").and_then(|p| p.as_str());
        let severity_filter = args.get("severity").and_then(|s| s.as_str());

        // Use compiler/linter to get diagnostics
        let mut output = String::new();
        let mut error_count = 0;
        let mut warning_count = 0;

        // Detect language from file extension and run appropriate checker
        if let Some(file_path) = path {
            let ext = std::path::Path::new(file_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");

            let check_output = match ext {
                "rs" => {
                    tokio::process::Command::new("cargo")
                        .args(["check", "--message-format=short"])
                        .output()
                        .await
                }
                "py" => {
                    tokio::process::Command::new("python3")
                        .args(["-m", "py_compile", file_path])
                        .output()
                        .await
                }
                "js" | "ts" | "jsx" | "tsx" => {
                    tokio::process::Command::new("npx")
                        .args(["tsc", "--noEmit", file_path])
                        .output()
                        .await
                }
                "go" => {
                    tokio::process::Command::new("go")
                        .args(["vet", file_path])
                        .output()
                        .await
                }
                _ => {
                    return Ok(ToolResult::success(format!(
                        "No LSP diagnostics available for .{ext} files"
                    )));
                }
            };

            match check_output {
                Ok(result) => {
                    let stdout = String::from_utf8_lossy(&result.stdout);
                    let stderr = String::from_utf8_lossy(&result.stderr);

                    // Parse output for errors/warnings
                    for line in stdout.lines().chain(stderr.lines()) {
                        let line_lower = line.to_lowercase();
                        if line_lower.contains("error") {
                            error_count += 1;
                            if severity_filter.is_none() || severity_filter == Some("error") {
                                output.push_str(&format!("[ERROR] {line}\n"));
                            }
                        } else if line_lower.contains("warning") {
                            warning_count += 1;
                            if severity_filter.is_none() || severity_filter == Some("warning") {
                                output.push_str(&format!("[WARN] {line}\n"));
                            }
                        }
                    }

                    if error_count == 0 && warning_count == 0 {
                        output = format!("[OK] No diagnostics found for {file_path}");
                    } else {
                        output = format!(
                            "Diagnostics for {file_path}: {error_count} error(s), {warning_count} warning(s)\n\n{output}"
                        );
                    }
                }
                Err(e) => {
                    output = format!("Failed to run diagnostics: {e}");
                }
            }
        } else {
            // Run workspace-wide diagnostics
            let cargo_check = tokio::process::Command::new("cargo")
                .args(["check", "--message-format=short"])
                .output()
                .await;

            match cargo_check {
                Ok(result) => {
                    let stderr = String::from_utf8_lossy(&result.stderr);
                    for line in stderr.lines() {
                        if line.contains("error") {
                            error_count += 1;
                            output.push_str(&format!("[ERROR] {line}\n"));
                        } else if line.contains("warning") {
                            warning_count += 1;
                            output.push_str(&format!("[WARN] {line}\n"));
                        }
                    }

                    if output.is_empty() {
                        output = "[OK] No diagnostics found in workspace".to_string();
                    } else {
                        output = format!(
                            "Workspace diagnostics: {error_count} error(s), {warning_count} warning(s)\n\n{output}"
                        );
                    }
                }
                Err(_) => {
                    output = "No Rust project found. Try specifying a file path.".to_string();
                }
            }
        }

        Ok(ToolResult::success(output))
    }

    pub(crate) async fn execute_lsp_hover(&self, args: Value) -> Result<ToolResult> {
        let file = args
            .get("file")
            .and_then(|f| f.as_str())
            .ok_or_else(|| crate::error::CortexError::InvalidInput("file is required".into()))?;
        let line = args
            .get("line")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as usize;
        let column = args
            .get("column")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as usize;

        // Read the file and extract context around the position
        let content = match tokio::fs::read_to_string(file).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(format!("Failed to read file: {e}"))),
        };

        let lines: Vec<&str> = content.lines().collect();
        if line == 0 || line > lines.len() {
            return Ok(ToolResult::error(format!(
                "Line {} is out of range (file has {} lines)",
                line,
                lines.len()
            )));
        }

        let target_line = lines[line - 1];

        // Extract the word at the given column
        let chars: Vec<char> = target_line.chars().collect();
        let col = column.saturating_sub(1).min(chars.len().saturating_sub(1));

        // Find word boundaries
        let mut start = col;
        let mut end = col;

        while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
            start -= 1;
        }
        while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
            end += 1;
        }

        let word: String = chars[start..end].iter().collect();

        if word.is_empty() {
            return Ok(ToolResult::success(
                "No symbol found at cursor position".to_string(),
            ));
        }

        // Try to find definition/documentation
        let ext = std::path::Path::new(file)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let mut hover_info = format!("Symbol: `{word}`\n");
        hover_info.push_str(&format!("Location: {file}:{line}:{column}\n"));
        hover_info.push_str(&format!("Context: {}\n", target_line.trim()));

        // Add language-specific hints
        match ext {
            "rs" => {
                hover_info.push_str("\n[Rust] Use `cargo doc --open` for full documentation");
            }
            "py" => {
                hover_info.push_str("\n[Python] Use `help()` in REPL for documentation");
            }
            "js" | "ts" => {
                hover_info.push_str("\n[JS/TS] Check type definitions in node_modules/@types");
            }
            _ => {}
        }

        Ok(ToolResult::success(hover_info))
    }

    pub(crate) async fn execute_lsp_symbols(&self, args: Value) -> Result<ToolResult> {
        let query = args.get("query").and_then(|q| q.as_str()).unwrap_or("");
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");

        // Use ctags or similar to find symbols
        let output = tokio::process::Command::new("grep")
            .args([
                "-r",
                "-n",
                "-E",
                &format!(
                    "(fn |def |class |struct |enum |interface |type |const |let |var ).*{query}.*"
                ),
                "--include=*.rs",
                "--include=*.py",
                "--include=*.js",
                "--include=*.ts",
                "--include=*.go",
                "--include=*.java",
                "--include=*.c",
                "--include=*.cpp",
                path,
            ])
            .output()
            .await;

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let lines: Vec<&str> = stdout.lines().take(50).collect();

                if lines.is_empty() {
                    return Ok(ToolResult::success(format!(
                        "No symbols found matching '{query}'"
                    )));
                }

                let mut result =
                    format!("Found {} symbol(s) matching '{}':\n\n", lines.len(), query);
                for line in lines {
                    result.push_str(&format!("> {line}\n"));
                }

                Ok(ToolResult::success(result))
            }
            Err(e) => Ok(ToolResult::error(format!("Symbol search failed: {e}"))),
        }
    }
}
