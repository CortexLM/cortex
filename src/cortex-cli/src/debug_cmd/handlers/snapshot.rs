//! Snapshot command handler.

use anyhow::{Context, Result, bail};

use cortex_engine::list_sessions;
use cortex_engine::rollout::get_rollout_path;
use cortex_protocol::ConversationId;

use crate::debug_cmd::commands::SnapshotArgs;
use crate::debug_cmd::types::{SnapshotDebugOutput, SnapshotInfo};
use crate::debug_cmd::utils::{format_size, get_cortex_home};

/// Run the snapshot debug command.
pub async fn run_snapshot(args: SnapshotArgs) -> Result<()> {
    let cortex_home = get_cortex_home();
    let snapshots_dir = cortex_home.join("snapshots");
    let cwd = std::env::current_dir()?;

    // Handle --create flag: create a new snapshot
    if args.create {
        use cortex_snapshot::SnapshotManager;

        let data_dir = cortex_home.clone();
        let mut manager = SnapshotManager::new(&cwd, &data_dir);

        let snapshot = manager
            .create_with_metadata(args.description.as_deref(), None, None)
            .await
            .context("Failed to create snapshot")?;

        if args.json {
            let output = serde_json::json!({
                "action": "create",
                "snapshot_id": snapshot.id,
                "tree_hash": snapshot.tree_hash,
                "created_at": snapshot.created_at.to_rfc3339(),
                "description": snapshot.description,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!("Created snapshot: {}", snapshot.id);
            println!("  Tree hash: {}", snapshot.tree_hash);
            println!("  Created at: {}", snapshot.created_at.to_rfc3339());
            if let Some(desc) = &snapshot.description {
                println!("  Description: {}", desc);
            }
        }
        return Ok(());
    }

    // Handle --restore flag: restore workspace to a snapshot
    if args.restore {
        let snapshot_id = args
            .snapshot_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("--snapshot-id is required for restore operation"))?;

        // For now, just indicate that restore would happen
        // Full implementation would require loading snapshot metadata
        if args.json {
            let output = serde_json::json!({
                "action": "restore",
                "snapshot_id": snapshot_id,
                "status": "not_implemented",
                "message": "Snapshot restore from CLI is not yet fully implemented. Use the TUI for restore operations.",
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!("Restore snapshot: {}", snapshot_id);
            println!("  Status: Not fully implemented in CLI");
            println!("  Note: Use the TUI for full snapshot restore operations.");
        }
        return Ok(());
    }

    // Handle --delete flag: delete a snapshot
    if args.delete {
        let snapshot_id = args
            .snapshot_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("--snapshot-id is required for delete operation"))?;

        // Find and delete the snapshot file
        let snapshot_path = snapshots_dir.join(snapshot_id);
        if snapshot_path.exists() {
            std::fs::remove_file(&snapshot_path)
                .with_context(|| format!("Failed to delete snapshot: {}", snapshot_id))?;

            if args.json {
                let output = serde_json::json!({
                    "action": "delete",
                    "snapshot_id": snapshot_id,
                    "status": "deleted",
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!("Deleted snapshot: {}", snapshot_id);
            }
        } else {
            bail!("Snapshot not found: {}", snapshot_id);
        }
        return Ok(());
    }

    // Default behavior: show snapshot status
    let snapshots_dir_exists = snapshots_dir.exists();
    let mut snapshot_count = 0;
    let mut total_size_bytes = 0u64;
    let mut session_snapshots = None;

    if snapshots_dir_exists {
        // Count all snapshots
        if let Ok(entries) = std::fs::read_dir(&snapshots_dir) {
            for entry in entries.flatten() {
                if entry.path().is_file() {
                    snapshot_count += 1;
                    if let Ok(meta) = entry.metadata() {
                        total_size_bytes += meta.len();
                    }
                }
            }
        }

        // Get session-specific snapshots
        if let Some(ref session_id) = args.session {
            // Validate session ID and check if session exists
            let conversation_id: ConversationId = match session_id.parse() {
                Ok(id) => id,
                Err(_) => {
                    // If parsing failed, check if it's a short ID (8 chars)
                    if session_id.len() == 8 {
                        // Try to find a session with matching prefix
                        let sessions = list_sessions(&cortex_home)?;
                        let matching: Vec<_> = sessions
                            .iter()
                            .filter(|s| s.id.starts_with(session_id))
                            .collect();

                        match matching.len() {
                            0 => bail!("No session found with ID prefix: {session_id}"),
                            1 => matching[0].id.parse().map_err(|_| {
                                anyhow::anyhow!("Internal error: invalid session ID format")
                            })?,
                            _ => bail!(
                                "Ambiguous session ID prefix '{}' matches {} sessions. Please provide more characters.",
                                session_id,
                                matching.len()
                            ),
                        }
                    } else {
                        bail!(
                            "Invalid session ID: {session_id}. Expected full UUID or 8-character prefix."
                        );
                    }
                }
            };

            // Check if session rollout file exists
            let rollout_path = get_rollout_path(&cortex_home, &conversation_id);
            if !rollout_path.exists() {
                bail!("Session not found: {session_id}");
            }

            // Session exists, now search for its snapshots
            let mut snapshots = Vec::new();
            let conversation_id_str = conversation_id.to_string();
            if let Ok(entries) = std::fs::read_dir(&snapshots_dir) {
                for entry in entries.flatten() {
                    let filename = entry.file_name().to_string_lossy().to_string();
                    // Match against the validated full conversation ID
                    if filename.contains(&conversation_id_str)
                        && let Ok(meta) = entry.metadata()
                    {
                        let modified = meta
                            .modified()
                            .ok()
                            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
                            .unwrap_or_else(|| "unknown".to_string());

                        snapshots.push(SnapshotInfo {
                            id: filename,
                            timestamp: modified,
                            size_bytes: meta.len(),
                        });
                    }
                }
            }
            snapshots.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            session_snapshots = Some(snapshots);
        }
    }

    let output = SnapshotDebugOutput {
        snapshots_dir: snapshots_dir.clone(),
        snapshots_dir_exists,
        snapshot_count,
        session_snapshots,
        total_size_bytes,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Snapshot Status");
        println!("{}", "=".repeat(50));
        println!("  Directory: {}", output.snapshots_dir.display());
        println!(
            "  Exists:    {}",
            if output.snapshots_dir_exists {
                "yes"
            } else {
                "no"
            }
        );
        println!("  Count:     {}", output.snapshot_count);
        println!("  Total Size: {}", format_size(output.total_size_bytes));

        // Provide guidance when snapshots directory doesn't exist
        if !output.snapshots_dir_exists {
            println!();
            println!("Note: Snapshots are created automatically during sessions when the agent");
            println!(
                "      makes changes to your workspace. Start a new session to create snapshots."
            );
        }

        if let Some(ref snapshots) = output.session_snapshots {
            println!();
            println!("Session Snapshots");
            println!("{}", "-".repeat(40));
            if snapshots.is_empty() {
                println!("  (no snapshots found for session)");
            } else {
                for snap in snapshots {
                    println!(
                        "  {} ({}) - {}",
                        snap.id,
                        format_size(snap.size_bytes),
                        snap.timestamp
                    );
                }
            }
        }

        if args.diff {
            println!();
            println!("Diff");
            println!("{}", "-".repeat(40));
            println!("  (snapshot diff not yet implemented)");
        }
    }

    Ok(())
}
