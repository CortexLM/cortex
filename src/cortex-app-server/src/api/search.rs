//! Project search endpoint.

use std::path::Path;
use std::process::Command;

use axum::{Json, extract::Query};

use crate::error::AppResult;

use super::types::{SearchMatch, SearchQuery, SearchResponse, SearchResult};

/// Search the project using ripgrep.
pub async fn search_project(Query(query): Query<SearchQuery>) -> AppResult<Json<SearchResponse>> {
    let path = Path::new(&query.path);
    if !path.exists() {
        return Ok(Json(SearchResponse { results: vec![] }));
    }

    // Use ripgrep for fast searching
    let mut cmd = Command::new("rg");
    cmd.arg("--json").arg("--line-number").arg("--column");

    if !query.case_sensitive {
        cmd.arg("-i");
    }
    if query.whole_word {
        cmd.arg("-w");
    }
    if !query.regex {
        cmd.arg("-F");
    }

    if let Some(include) = &query.include {
        for pattern in include.split(',') {
            cmd.arg("-g").arg(pattern.trim());
        }
    }

    if let Some(exclude) = &query.exclude {
        for pattern in exclude.split(',') {
            cmd.arg("-g").arg(format!("!{}", pattern.trim()));
        }
    }

    // Default excludes
    cmd.arg("-g")
        .arg("!node_modules")
        .arg("-g")
        .arg("!.git")
        .arg("-g")
        .arg("!target")
        .arg("-g")
        .arg("!dist")
        .arg("-g")
        .arg("!build");

    cmd.arg(&query.query).arg(&query.path);

    let output = cmd.output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut results: std::collections::HashMap<String, Vec<SearchMatch>> =
                std::collections::HashMap::new();

            for line in stdout.lines() {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(line)
                    && json["type"] == "match"
                    && let (Some(path), Some(line_num), Some(text)) = (
                        json["data"]["path"]["text"].as_str(),
                        json["data"]["line_number"].as_u64(),
                        json["data"]["lines"]["text"].as_str(),
                    )
                {
                    let rel_path = path
                        .strip_prefix(&format!("{}/", query.path))
                        .unwrap_or(path)
                        .to_string();

                    let column =
                        json["data"]["submatches"][0]["start"].as_u64().unwrap_or(0) as usize;
                    let match_end =
                        json["data"]["submatches"][0]["end"].as_u64().unwrap_or(0) as usize;

                    let entry = results.entry(rel_path.clone()).or_default();
                    entry.push(SearchMatch {
                        line: line_num as usize,
                        column,
                        text: text.trim_end().to_string(),
                        match_start: column,
                        match_end,
                    });
                }
            }

            let results: Vec<SearchResult> = results
                .into_iter()
                .map(|(file, matches)| SearchResult { file, matches })
                .collect();

            Ok(Json(SearchResponse { results }))
        }
        Err(_) => {
            // Fallback: ripgrep not available
            Ok(Json(SearchResponse { results: vec![] }))
        }
    }
}
