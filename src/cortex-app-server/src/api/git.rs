//! Git operations endpoints.

use std::process::Command;

use axum::{Json, extract::Query};

use crate::error::AppResult;

use super::types::{
    GitBlameQuery, GitBranch, GitCheckoutRequest, GitCommitRequest, GitCreateBranchRequest,
    GitDeleteBranchRequest, GitDiffQuery, GitLogQuery, GitMergeRequest, GitPathQuery,
    GitPathRequest, GitStageRequest, GitStashCreateRequest, GitStashIndexRequest, GitStatusFile,
    GitStatusResponse,
};

/// Get git status.
pub async fn git_status(Query(query): Query<GitPathQuery>) -> AppResult<Json<GitStatusResponse>> {
    let output = Command::new("git")
        .args(["-C", &query.path, "status", "--porcelain=v2", "-b"])
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut branch = None;
            let mut ahead: Option<u32> = None;
            let mut behind: Option<u32> = None;
            let mut staged = vec![];
            let mut unstaged = vec![];
            let mut conflicts = vec![];

            for line in stdout.lines() {
                if let Some(stripped) = line.strip_prefix("# branch.head ") {
                    branch = Some(stripped.to_string());
                } else if let Some(stripped) = line.strip_prefix("# branch.ab ") {
                    // Parse ahead/behind: # branch.ab +1 -2
                    let parts: Vec<&str> = stripped.split_whitespace().collect();
                    if parts.len() >= 2 {
                        ahead = parts[0].trim_start_matches('+').parse().ok();
                        behind = parts[1].trim_start_matches('-').parse().ok();
                    }
                } else if line.starts_with("1 ") || line.starts_with("2 ") {
                    // Changed entries: 1 XY sub mH mI mW hH hI path
                    let parts: Vec<&str> = line.splitn(9, ' ').collect();
                    if parts.len() >= 9 {
                        let xy = parts[1];
                        let path = parts[8].to_string();
                        let index_status = &xy[0..1];
                        let worktree_status = &xy[1..2];

                        if index_status != "." {
                            staged.push(GitStatusFile {
                                path: path.clone(),
                                status: status_char_to_string(index_status),
                                staged: true,
                                conflict_type: None,
                            });
                        }

                        if worktree_status != "." {
                            unstaged.push(GitStatusFile {
                                path,
                                status: status_char_to_string(worktree_status),
                                staged: false,
                                conflict_type: None,
                            });
                        }
                    }
                } else if let Some(stripped) = line.strip_prefix("? ") {
                    // Untracked: ? path
                    let path = stripped.to_string();
                    unstaged.push(GitStatusFile {
                        path,
                        status: "untracked".to_string(),
                        staged: false,
                        conflict_type: None,
                    });
                } else if line.starts_with("u ") {
                    // Unmerged (conflict): u XY sub m1 m2 m3 mW h1 h2 h3 path
                    let parts: Vec<&str> = line.splitn(11, ' ').collect();
                    if parts.len() >= 11 {
                        let xy = parts[1];
                        let path = parts[10].to_string();
                        let conflict_type = match xy {
                            "DD" => Some("both-deleted".to_string()),
                            "AU" => Some("added-by-us".to_string()),
                            "UD" => Some("deleted-by-them".to_string()),
                            "UA" => Some("added-by-them".to_string()),
                            "DU" => Some("deleted-by-us".to_string()),
                            "AA" => Some("both-added".to_string()),
                            "UU" => Some("both-modified".to_string()),
                            _ => Some("conflict".to_string()),
                        };
                        conflicts.push(GitStatusFile {
                            path,
                            status: "conflict".to_string(),
                            staged: false,
                            conflict_type,
                        });
                    }
                }
            }

            Ok(Json(GitStatusResponse {
                branch,
                staged,
                unstaged,
                conflicts,
                ahead,
                behind,
            }))
        }
        Err(_) => Ok(Json(GitStatusResponse {
            branch: None,
            staged: vec![],
            unstaged: vec![],
            conflicts: vec![],
            ahead: None,
            behind: None,
        })),
    }
}

fn status_char_to_string(c: &str) -> String {
    match c {
        "M" => "modified",
        "A" => "added",
        "D" => "deleted",
        "R" => "renamed",
        "C" => "copied",
        "U" => "unmerged",
        "?" => "untracked",
        _ => "unknown",
    }
    .to_string()
}

/// Get current branch.
pub async fn git_branch(Query(query): Query<GitPathQuery>) -> AppResult<Json<serde_json::Value>> {
    let output = Command::new("git")
        .args(["-C", &query.path, "branch", "--show-current"])
        .output();

    match output {
        Ok(output) => {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(Json(serde_json::json!({ "branch": branch })))
        }
        Err(_) => Ok(Json(serde_json::json!({ "branch": null }))),
    }
}

/// List all branches.
pub async fn git_branches(Query(query): Query<GitPathQuery>) -> AppResult<Json<serde_json::Value>> {
    // Get local branches with verbose info
    let output = Command::new("git")
        .args(["-C", &query.path, "branch", "-vv", "--format=%(HEAD)%(refname:short)|%(upstream:short)|%(upstream:track)|%(objectname:short)|%(subject)"])
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut branches = vec![];

            for line in stdout.lines() {
                if line.is_empty() {
                    continue;
                }

                let current = line.starts_with('*');
                let line = line.trim_start_matches('*').trim_start_matches(' ');
                let parts: Vec<&str> = line.splitn(5, '|').collect();

                if !parts.is_empty() {
                    let name = parts[0].to_string();
                    let upstream = if parts.len() > 1 && !parts[1].is_empty() {
                        Some(parts[1].to_string())
                    } else {
                        None
                    };

                    // Parse ahead/behind from track info like "[ahead 1, behind 2]"
                    let (ahead, behind) = if parts.len() > 2 {
                        parse_tracking_info(parts[2])
                    } else {
                        (None, None)
                    };

                    let last_commit = if parts.len() > 4 && !parts[4].is_empty() {
                        Some(parts[4].to_string())
                    } else {
                        None
                    };

                    branches.push(GitBranch {
                        name,
                        current,
                        remote: None,
                        upstream,
                        ahead,
                        behind,
                        last_commit,
                    });
                }
            }

            // Also get remote branches
            let remote_output = Command::new("git")
                .args([
                    "-C",
                    &query.path,
                    "branch",
                    "-r",
                    "--format=%(refname:short)|%(objectname:short)",
                ])
                .output();

            if let Ok(remote_output) = remote_output {
                let remote_stdout = String::from_utf8_lossy(&remote_output.stdout);
                for line in remote_stdout.lines() {
                    if line.is_empty() || line.contains("HEAD") {
                        continue;
                    }

                    let parts: Vec<&str> = line.splitn(2, '|').collect();
                    if !parts.is_empty() {
                        let name = parts[0].to_string();
                        let remote_name = name.split('/').next().map(|s| s.to_string());

                        branches.push(GitBranch {
                            name,
                            current: false,
                            remote: remote_name,
                            upstream: None,
                            ahead: None,
                            behind: None,
                            last_commit: None,
                        });
                    }
                }
            }

            Ok(Json(serde_json::json!({ "branches": branches })))
        }
        Err(_) => Ok(Json(serde_json::json!({ "branches": [] }))),
    }
}

fn parse_tracking_info(track: &str) -> (Option<u32>, Option<u32>) {
    let mut ahead = None;
    let mut behind = None;

    // Format: [ahead N] or [behind N] or [ahead N, behind M] or [gone]
    let track = track.trim_matches(|c| c == '[' || c == ']');

    for part in track.split(',') {
        let part = part.trim();
        if let Some(stripped) = part.strip_prefix("ahead ") {
            ahead = stripped.trim().parse().ok();
        } else if let Some(stripped) = part.strip_prefix("behind ") {
            behind = stripped.trim().parse().ok();
        }
    }

    (ahead, behind)
}

/// Get diff for a file.
pub async fn git_diff(Query(query): Query<GitDiffQuery>) -> AppResult<Json<serde_json::Value>> {
    let mut args = vec!["-C", &query.path, "diff"];
    if query.staged {
        args.push("--cached");
    }
    args.push("--");
    args.push(&query.file);

    let output = Command::new("git").args(&args).output();

    match output {
        Ok(output) => {
            let diff = String::from_utf8_lossy(&output.stdout);
            let is_binary = diff.contains("Binary files") || diff.contains("GIT binary patch");
            let (hunks, additions, deletions) = parse_diff_with_stats(&diff);

            Ok(Json(serde_json::json!({
                "path": query.file,
                "hunks": hunks,
                "binary": is_binary,
                "additions": additions,
                "deletions": deletions,
            })))
        }
        Err(_) => Ok(Json(
            serde_json::json!({ "path": query.file, "hunks": [], "binary": false, "additions": 0, "deletions": 0 }),
        )),
    }
}

fn parse_diff_with_stats(diff: &str) -> (Vec<serde_json::Value>, usize, usize) {
    let mut hunks = vec![];
    let mut current_hunk: Option<serde_json::Value> = None;
    let mut lines: Vec<serde_json::Value> = vec![];
    let mut old_line = 0usize;
    let mut new_line = 0usize;
    let mut total_additions = 0usize;
    let mut total_deletions = 0usize;

    for line in diff.lines() {
        if line.starts_with("@@") {
            // Save previous hunk
            if let Some(mut hunk) = current_hunk.take() {
                hunk["lines"] = serde_json::json!(lines);
                hunks.push(hunk);
                lines = vec![];
            }

            // Parse hunk header: @@ -old,count +new,count @@
            current_hunk = Some(serde_json::json!({ "header": line }));

            // Extract line numbers
            if let Some(caps) = line.find("-") {
                let after_minus = &line[caps + 1..];
                if let Some(comma) = after_minus.find(',') {
                    old_line = after_minus[..comma].parse().unwrap_or(1);
                }
            }
            if let Some(caps) = line.find("+") {
                let after_plus = &line[caps + 1..];
                if let Some(comma) = after_plus.find(',') {
                    new_line = after_plus[..comma].parse().unwrap_or(1);
                } else if let Some(space) = after_plus.find(' ') {
                    new_line = after_plus[..space].parse().unwrap_or(1);
                }
            }
        } else if current_hunk.is_some() {
            let (line_type, content) = if let Some(stripped) = line.strip_prefix('+') {
                total_additions += 1;
                ("addition", stripped)
            } else if let Some(stripped) = line.strip_prefix('-') {
                total_deletions += 1;
                ("deletion", stripped)
            } else if let Some(stripped) = line.strip_prefix(' ') {
                ("context", stripped)
            } else {
                ("context", line)
            };

            let mut entry = serde_json::json!({
                "type": line_type,
                "content": content,
            });

            match line_type {
                "addition" => {
                    entry["newLineNumber"] = serde_json::json!(new_line);
                    new_line += 1;
                }
                "deletion" => {
                    entry["oldLineNumber"] = serde_json::json!(old_line);
                    old_line += 1;
                }
                "context" => {
                    entry["oldLineNumber"] = serde_json::json!(old_line);
                    entry["newLineNumber"] = serde_json::json!(new_line);
                    old_line += 1;
                    new_line += 1;
                }
                _ => {}
            }

            lines.push(entry);
        }
    }

    // Save last hunk
    if let Some(mut hunk) = current_hunk {
        hunk["lines"] = serde_json::json!(lines);
        hunks.push(hunk);
    }

    (hunks, total_additions, total_deletions)
}

/// Get blame for a file.
pub async fn git_blame(Query(query): Query<GitBlameQuery>) -> AppResult<Json<serde_json::Value>> {
    let output = Command::new("git")
        .args(["-C", &query.path, "blame", "--porcelain", &query.file])
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let lines = parse_blame(&stdout);
            Ok(Json(serde_json::json!({ "lines": lines })))
        }
        Err(_) => Ok(Json(serde_json::json!({ "lines": [] }))),
    }
}

fn parse_blame(blame: &str) -> Vec<serde_json::Value> {
    let mut result = vec![];
    let mut current_commit: Option<serde_json::Value> = None;
    let mut line_number = 0usize;

    for line in blame.lines() {
        if line.len() == 40 && line.chars().all(|c| c.is_ascii_hexdigit())
            || (line.len() > 40
                && line.chars().take(40).all(|c| c.is_ascii_hexdigit())
                && line.chars().nth(40) == Some(' '))
        {
            // New commit line
            let hash = &line[..40];
            let parts: Vec<&str> = line[41..].split_whitespace().collect();
            line_number = parts
                .first()
                .and_then(|s| s.parse().ok())
                .unwrap_or(line_number + 1);

            current_commit = Some(serde_json::json!({
                "hash": hash,
                "shortHash": &hash[..8],
                "author": "",
                "email": "",
                "date": "",
                "message": "",
            }));
        } else if let Some(ref mut commit) = current_commit {
            if let Some(stripped) = line.strip_prefix("author ") {
                commit["author"] = serde_json::json!(stripped.to_string());
            } else if let Some(stripped) = line.strip_prefix("author-mail ") {
                commit["email"] =
                    serde_json::json!(stripped.trim_matches(|c| c == '<' || c == '>').to_string());
            } else if let Some(stripped) = line.strip_prefix("author-time ") {
                if let Ok(timestamp) = stripped.parse::<i64>() {
                    commit["date"] = serde_json::json!(
                        chrono::DateTime::from_timestamp(timestamp, 0)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    );
                }
            } else if let Some(stripped) = line.strip_prefix("summary ") {
                commit["message"] = serde_json::json!(stripped.to_string());
            } else if let Some(stripped) = line.strip_prefix('\t') {
                // This is the actual code line
                if let Some(commit) = current_commit.take() {
                    result.push(serde_json::json!({
                        "lineNumber": line_number,
                        "content": stripped,
                        "commit": commit,
                    }));
                }
            }
        }
    }

    result
}

/// Get git log.
pub async fn git_log(Query(query): Query<GitLogQuery>) -> AppResult<Json<serde_json::Value>> {
    let limit = query.limit.min(500).to_string();

    // Get detailed commit information
    let output = Command::new("git")
        .args([
            "-C",
            &query.path,
            "log",
            &format!("-{}", limit),
            "--format=%H|%h|%s|%an|%ae|%aI|%P|%D",
        ])
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let commits: Vec<serde_json::Value> = stdout
                .lines()
                .filter_map(|line| {
                    let parts: Vec<&str> = line.splitn(8, '|').collect();
                    if parts.len() >= 6 {
                        let hash = parts[0];
                        let short_hash = parts[1];
                        let message = parts[2];
                        let author = parts[3];
                        let email = parts[4];
                        let date = parts[5];
                        let parents: Vec<&str> = if parts.len() > 6 && !parts[6].is_empty() {
                            parts[6].split_whitespace().collect()
                        } else {
                            vec![]
                        };
                        let refs_str = if parts.len() > 7 { parts[7] } else { "" };

                        // Parse refs (branches, tags)
                        let refs: Vec<serde_json::Value> = refs_str
                            .split(',')
                            .map(|r| r.trim())
                            .filter(|r| !r.is_empty())
                            .map(|r| {
                                let (ref_type, name, is_head) = if r.starts_with("HEAD -> ") {
                                    ("head", r.strip_prefix("HEAD -> ").unwrap_or(r), true)
                                } else if r.starts_with("tag: ") {
                                    ("tag", r.strip_prefix("tag: ").unwrap_or(r), false)
                                } else if r.starts_with("origin/") || r.contains("/") {
                                    ("remote", r, false)
                                } else if r == "HEAD" {
                                    return serde_json::json!(null);
                                } else {
                                    ("branch", r, false)
                                };
                                serde_json::json!({
                                    "name": name,
                                    "type": ref_type,
                                    "isHead": is_head,
                                })
                            })
                            .filter(|v| !v.is_null())
                            .collect();

                        Some(serde_json::json!({
                            "hash": hash,
                            "shortHash": short_hash,
                            "message": message,
                            "author": author,
                            "email": email,
                            "date": date,
                            "timestamp": chrono::DateTime::parse_from_rfc3339(date)
                                .map(|dt| dt.timestamp())
                                .unwrap_or(0),
                            "parents": parents,
                            "refs": refs,
                            "isMerge": parents.len() > 1,
                        }))
                    } else {
                        None
                    }
                })
                .collect();
            Ok(Json(serde_json::json!({ "commits": commits })))
        }
        Err(_) => Ok(Json(serde_json::json!({ "commits": [] }))),
    }
}

/// Stage files.
pub async fn git_stage(Json(req): Json<GitStageRequest>) -> AppResult<Json<serde_json::Value>> {
    for file in &req.files {
        let _ = Command::new("git")
            .args(["-C", &req.path, "add", file])
            .output();
    }

    Ok(Json(serde_json::json!({ "success": true })))
}

/// Unstage files.
pub async fn git_unstage(Json(req): Json<GitStageRequest>) -> AppResult<Json<serde_json::Value>> {
    for file in &req.files {
        let _ = Command::new("git")
            .args(["-C", &req.path, "restore", "--staged", file])
            .output();
    }

    Ok(Json(serde_json::json!({ "success": true })))
}

/// Stage all files.
pub async fn git_stage_all(Json(req): Json<GitPathRequest>) -> AppResult<Json<serde_json::Value>> {
    let _ = Command::new("git")
        .args(["-C", &req.path, "add", "-A"])
        .output();

    Ok(Json(serde_json::json!({ "success": true })))
}

/// Unstage all files.
pub async fn git_unstage_all(
    Json(req): Json<GitPathRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let _ = Command::new("git")
        .args(["-C", &req.path, "reset", "HEAD"])
        .output();

    Ok(Json(serde_json::json!({ "success": true })))
}

/// Commit changes.
pub async fn git_commit(Json(req): Json<GitCommitRequest>) -> AppResult<Json<serde_json::Value>> {
    let output = Command::new("git")
        .args(["-C", &req.path, "commit", "-m", &req.message])
        .output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            Ok(Json(serde_json::json!({ "success": success })))
        }
        Err(_) => Ok(Json(serde_json::json!({ "success": false }))),
    }
}

/// Checkout branch.
pub async fn git_checkout(
    Json(req): Json<GitCheckoutRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let output = Command::new("git")
        .args(["-C", &req.path, "checkout", &req.branch])
        .output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Ok(Json(
                serde_json::json!({ "success": success, "error": if success { serde_json::Value::Null } else { serde_json::json!(stderr) } }),
            ))
        }
        Err(e) => Ok(Json(
            serde_json::json!({ "success": false, "error": e.to_string() }),
        )),
    }
}

/// Push to remote.
pub async fn git_push(Json(req): Json<GitPathRequest>) -> AppResult<Json<serde_json::Value>> {
    let output = Command::new("git").args(["-C", &req.path, "push"]).output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            Ok(Json(serde_json::json!({
                "success": success,
                "message": if success { stdout } else { stderr.clone() },
                "error": if success { serde_json::Value::Null } else { serde_json::json!(stderr) }
            })))
        }
        Err(e) => Ok(Json(
            serde_json::json!({ "success": false, "error": e.to_string() }),
        )),
    }
}

/// Pull from remote.
pub async fn git_pull(Json(req): Json<GitPathRequest>) -> AppResult<Json<serde_json::Value>> {
    let output = Command::new("git").args(["-C", &req.path, "pull"]).output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            Ok(Json(serde_json::json!({
                "success": success,
                "message": if success { stdout } else { stderr.clone() },
                "error": if success { serde_json::Value::Null } else { serde_json::json!(stderr) }
            })))
        }
        Err(e) => Ok(Json(
            serde_json::json!({ "success": false, "error": e.to_string() }),
        )),
    }
}

/// Fetch from remote.
pub async fn git_fetch(Json(req): Json<GitPathRequest>) -> AppResult<Json<serde_json::Value>> {
    let output = Command::new("git")
        .args(["-C", &req.path, "fetch", "--all", "--prune"])
        .output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Ok(Json(serde_json::json!({
                "success": success,
                "error": if success { serde_json::Value::Null } else { serde_json::json!(stderr) }
            })))
        }
        Err(e) => Ok(Json(
            serde_json::json!({ "success": false, "error": e.to_string() }),
        )),
    }
}

/// Discard changes.
pub async fn git_discard(Json(req): Json<GitStageRequest>) -> AppResult<Json<serde_json::Value>> {
    let mut errors = vec![];

    for file in &req.files {
        // First try to restore tracked files
        let restore_output = Command::new("git")
            .args(["-C", &req.path, "checkout", "--", file])
            .output();

        // If that fails (e.g., untracked file), try to clean
        if let Ok(output) = restore_output {
            if !output.status.success() {
                // Try removing untracked file
                let clean_output = Command::new("git")
                    .args(["-C", &req.path, "clean", "-fd", "--", file])
                    .output();

                if let Ok(clean_out) = clean_output
                    && !clean_out.status.success()
                {
                    errors.push(format!("Failed to discard {}", file));
                }
            }
        } else {
            errors.push(format!("Failed to discard {}", file));
        }
    }

    Ok(Json(serde_json::json!({
        "success": errors.is_empty(),
        "errors": errors
    })))
}

/// Create a new branch.
pub async fn git_create_branch(
    Json(req): Json<GitCreateBranchRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let mut args = vec!["-C".to_string(), req.path.clone()];

    if req.checkout {
        args.extend(["checkout".to_string(), "-b".to_string(), req.name.clone()]);
    } else {
        args.extend(["branch".to_string(), req.name.clone()]);
    }

    if let Some(ref start) = req.start_point {
        args.push(start.clone());
    }

    let output = Command::new("git").args(&args).output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Ok(Json(serde_json::json!({
                "success": success,
                "error": if success { serde_json::Value::Null } else { serde_json::json!(stderr) }
            })))
        }
        Err(e) => Ok(Json(
            serde_json::json!({ "success": false, "error": e.to_string() }),
        )),
    }
}

/// Delete a branch.
pub async fn git_delete_branch(
    Json(req): Json<GitDeleteBranchRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let delete_flag = if req.force { "-D" } else { "-d" };

    let output = Command::new("git")
        .args(["-C", &req.path, "branch", delete_flag, &req.name])
        .output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Ok(Json(serde_json::json!({
                "success": success,
                "error": if success { serde_json::Value::Null } else { serde_json::json!(stderr) }
            })))
        }
        Err(e) => Ok(Json(
            serde_json::json!({ "success": false, "error": e.to_string() }),
        )),
    }
}

/// Merge a branch.
pub async fn git_merge(Json(req): Json<GitMergeRequest>) -> AppResult<Json<serde_json::Value>> {
    let mut args = vec!["-C".to_string(), req.path.clone(), "merge".to_string()];

    if req.no_ff {
        args.push("--no-ff".to_string());
    }

    if let Some(ref msg) = req.message {
        args.push("-m".to_string());
        args.push(msg.clone());
    }

    args.push(req.branch.clone());

    let output = Command::new("git").args(&args).output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();

            // Check if there are conflicts
            let has_conflicts = stderr.contains("CONFLICT") || stdout.contains("CONFLICT");

            Ok(Json(serde_json::json!({
                "success": success && !has_conflicts,
                "hasConflicts": has_conflicts,
                "message": stdout,
                "error": if success && !has_conflicts { serde_json::Value::Null } else { serde_json::json!(stderr) }
            })))
        }
        Err(e) => Ok(Json(
            serde_json::json!({ "success": false, "error": e.to_string() }),
        )),
    }
}

/// List stashes.
pub async fn git_stash_list(
    Query(query): Query<GitPathQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let output = Command::new("git")
        .args(["-C", &query.path, "stash", "list", "--format=%gd|%H|%s|%aI"])
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stashes: Vec<serde_json::Value> = stdout
                .lines()
                .enumerate()
                .filter_map(|(idx, line)| {
                    let parts: Vec<&str> = line.splitn(4, '|').collect();
                    if parts.len() >= 4 {
                        let stash_ref = parts[0];
                        let oid = parts[1];
                        let message = parts[2];
                        let date = parts[3];

                        // Extract branch from message like "WIP on main: abc123 message"
                        let branch = message
                            .strip_prefix("WIP on ")
                            .or_else(|| message.strip_prefix("On "))
                            .and_then(|s| s.split(':').next())
                            .map(|s| s.to_string());

                        let timestamp = chrono::DateTime::parse_from_rfc3339(date)
                            .map(|dt| dt.timestamp())
                            .unwrap_or(0);

                        Some(serde_json::json!({
                            "index": idx,
                            "ref": stash_ref,
                            "oid": oid,
                            "shortOid": &oid[..8.min(oid.len())],
                            "message": message,
                            "branch": branch,
                            "date": date,
                            "timestamp": timestamp,
                        }))
                    } else {
                        None
                    }
                })
                .collect();
            Ok(Json(serde_json::json!({ "stashes": stashes })))
        }
        Err(_) => Ok(Json(serde_json::json!({ "stashes": [] }))),
    }
}

/// Create a stash.
pub async fn git_stash_create(
    Json(req): Json<GitStashCreateRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let mut args = vec!["-C", &req.path, "stash", "push"];

    if req.include_untracked {
        args.push("-u");
    }

    if let Some(ref msg) = req.message {
        args.push("-m");
        args.push(msg);
    }

    let output = Command::new("git").args(&args).output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();

            // Check if there was nothing to stash
            let no_changes =
                stdout.contains("No local changes") || stderr.contains("No local changes");

            Ok(Json(serde_json::json!({
                "success": success,
                "noChanges": no_changes,
                "message": stdout,
                "error": if success { serde_json::Value::Null } else { serde_json::json!(stderr) }
            })))
        }
        Err(e) => Ok(Json(
            serde_json::json!({ "success": false, "error": e.to_string() }),
        )),
    }
}

/// Apply a stash.
pub async fn git_stash_apply(
    Json(req): Json<GitStashIndexRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let stash_ref = format!("stash@{{{}}}", req.index);

    let output = Command::new("git")
        .args(["-C", &req.path, "stash", "apply", &stash_ref])
        .output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let has_conflicts = stderr.contains("CONFLICT");

            Ok(Json(serde_json::json!({
                "success": success && !has_conflicts,
                "hasConflicts": has_conflicts,
                "error": if success && !has_conflicts { serde_json::Value::Null } else { serde_json::json!(stderr) }
            })))
        }
        Err(e) => Ok(Json(
            serde_json::json!({ "success": false, "error": e.to_string() }),
        )),
    }
}

/// Pop a stash.
pub async fn git_stash_pop(
    Json(req): Json<GitStashIndexRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let stash_ref = format!("stash@{{{}}}", req.index);

    let output = Command::new("git")
        .args(["-C", &req.path, "stash", "pop", &stash_ref])
        .output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let has_conflicts = stderr.contains("CONFLICT");

            Ok(Json(serde_json::json!({
                "success": success && !has_conflicts,
                "hasConflicts": has_conflicts,
                "error": if success && !has_conflicts { serde_json::Value::Null } else { serde_json::json!(stderr) }
            })))
        }
        Err(e) => Ok(Json(
            serde_json::json!({ "success": false, "error": e.to_string() }),
        )),
    }
}

/// Drop a stash.
pub async fn git_stash_drop(
    Json(req): Json<GitStashIndexRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let stash_ref = format!("stash@{{{}}}", req.index);

    let output = Command::new("git")
        .args(["-C", &req.path, "stash", "drop", &stash_ref])
        .output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Ok(Json(serde_json::json!({
                "success": success,
                "error": if success { serde_json::Value::Null } else { serde_json::json!(stderr) }
            })))
        }
        Err(e) => Ok(Json(
            serde_json::json!({ "success": false, "error": e.to_string() }),
        )),
    }
}
