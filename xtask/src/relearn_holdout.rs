//! Relearn holdout ceremony helper.
//!
//! Selects a holdout slice from an operator catalog (or a local synthetic
//! catalog), writes the frozen records to an operator file, and prints the
//! commitment that belongs in `config/relearn-pin.toml`.
//!
//! The records never enter git. The commitment does. Production salts and
//! catalogs stay off git — a documented salt plus a public catalog would let
//! miners reconstruct the scored split.
//!
//! Selection is `sha256(domain || salt || id)`, lowest digests first. The
//! domain and salt are distinct from the T2I ceremony.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use relearn_challenge_task::{holdout_commitment, HoldoutItem, HoldoutTask};
use sha2::{Digest, Sha256};

/// Domain tag for holdout selection. Distinct from T2I and from the commitment domain.
const SELECT_DOMAIN: &[u8] = b"base-relearn-holdout-select-v1";

/// Arguments for the holdout ceremony.
#[derive(Debug)]
pub struct HoldoutArgs {
    /// Catalog JSON array of [`HoldoutItem`]. Ignored when `synthetic` is set.
    pub catalog: Option<PathBuf>,
    /// Selection salt. Dev salts are local-only; production salts stay off git.
    pub salt: String,
    /// Holdout item count.
    pub size: usize,
    /// Item ids already published in the pin's public split.
    pub exclude: Vec<u32>,
    /// Build a local-only synthetic catalog (never a production source).
    pub synthetic: bool,
    /// Where to write the records (never inside the repo).
    pub out: Option<PathBuf>,
}

/// Run the ceremony helper.
///
/// # Errors
///
/// Empty salt, a missing catalog, or a size the catalog cannot satisfy.
pub fn run(args: &HoldoutArgs) -> Result<(), String> {
    if args.salt.trim().is_empty() {
        return Err("--salt must not be empty".into());
    }
    if args.salt.trim() == "cortex-t2i-dev-holdout-v0" {
        return Err("refusing the documented T2I/dev salt; use a Relearn-specific salt".into());
    }
    let rows = if args.synthetic {
        synthetic_catalog()
    } else {
        let path = args
            .catalog
            .as_ref()
            .ok_or("--catalog is required unless --synthetic")?;
        read_catalog(path)?
    };
    let excluded: BTreeSet<u32> = args.exclude.iter().copied().collect();
    let records = select(&rows, &args.salt, args.size, &excluded)?;
    let commitment = holdout_commitment(&records);

    if let Some(out) = &args.out {
        refuse_repo_path(out)?;
        let body = serde_json::to_string_pretty(&records)
            .map_err(|e| format!("serialize records: {e}"))?;
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        fs::write(out, format!("{body}\n")).map_err(|e| format!("write {}: {e}", out.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(out, fs::Permissions::from_mode(0o600));
        }
        println!(
            "wrote {} holdout records to {}",
            records.len(),
            out.display()
        );
    }

    println!("holdout_size = {}", records.len());
    println!("holdout_commitment = \"{commitment}\"");
    Ok(())
}

fn refuse_repo_path(out: &Path) -> Result<(), String> {
    let text = out.to_string_lossy();
    if text.contains("/config/") || text.starts_with("config/") || text.contains("/docs/") {
        return Err(format!(
            "refusing to write holdout records under a tracked path: {text}"
        ));
    }
    Ok(())
}

fn read_catalog(path: &Path) -> Result<Vec<HoldoutItem>, String> {
    let body = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&body).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// Local/CI catalog. Production must use a private operator catalog.
fn synthetic_catalog() -> Vec<HoldoutItem> {
    (1..=200)
        .map(|id| {
            let (task, image_hash) = match id % 5 {
                1 => (HoldoutTask::Captioning, format!("{:064x}", id + 10_000)),
                2 => (HoldoutTask::Vqa, format!("{:064x}", id + 20_000)),
                3 => (HoldoutTask::Ocr, format!("{:064x}", id + 30_000)),
                4 => (HoldoutTask::Spatial, format!("{:064x}", id + 40_000)),
                _ => (HoldoutTask::Text, String::new()),
            };
            HoldoutItem {
                id,
                prompt: format!(
                    "synthetic relearn item {id} describing a unique scene for family {}",
                    task.as_str()
                ),
                dataset_id: "synthetic-dev".into(),
                task,
                image_hash,
            }
        })
        .collect()
}

fn select(
    rows: &[HoldoutItem],
    salt: &str,
    size: usize,
    excluded: &BTreeSet<u32>,
) -> Result<Vec<HoldoutItem>, String> {
    let mut ranked: Vec<([u8; 32], HoldoutItem)> = rows
        .iter()
        .filter(|r| !excluded.contains(&r.id) && !r.prompt.trim().is_empty())
        .map(|r| (rank(salt, r.id), r.clone()))
        .collect();
    if ranked.len() < size {
        return Err(format!(
            "catalog has {} eligible items after exclusions, need {size}",
            ranked.len()
        ));
    }
    ranked.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
    let mut out: Vec<HoldoutItem> = ranked.into_iter().take(size).map(|(_, r)| r).collect();
    out.sort_by_key(|r| r.id);
    Ok(out)
}

fn rank(salt: &str, id: u32) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(SELECT_DOMAIN);
    h.update([0xff]);
    h.update(salt.as_bytes());
    h.update([0xff]);
    h.update(id.to_le_bytes());
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_deterministic_per_salt() {
        let rows = synthetic_catalog();
        let a = select(&rows, "salt-a", 10, &BTreeSet::new()).expect("a");
        let b = select(&rows, "salt-a", 10, &BTreeSet::new()).expect("b");
        let c = select(&rows, "salt-b", 10, &BTreeSet::new()).expect("c");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 10);
    }

    #[test]
    fn selection_honors_exclusions() {
        let rows = synthetic_catalog();
        let all: BTreeSet<u32> = (1..=190).collect();
        let picked = select(&rows, "salt", 10, &all).expect("picked");
        assert!(picked.iter().all(|r| r.id > 190));
    }

    #[test]
    fn refuses_the_t2i_dev_salt() {
        let err = run(&HoldoutArgs {
            catalog: None,
            salt: "cortex-t2i-dev-holdout-v0".into(),
            size: 10,
            exclude: Vec::new(),
            synthetic: true,
            out: None,
        })
        .expect_err("t2i salt");
        assert!(err.contains("T2I"), "{err}");
    }

    #[test]
    fn tracked_output_paths_are_refused() {
        assert!(refuse_repo_path(Path::new("config/holdout.json")).is_err());
        refuse_repo_path(Path::new("/root/.base-secrets/relearn-holdout.json")).expect("ok");
    }
}
