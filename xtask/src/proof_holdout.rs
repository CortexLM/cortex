//! Proof per-topic holdout ceremony helper.
//!
//! Selects a stratified holdout set (24 records × 5 scored splits), writes
//! the frozen records to an operator file, and prints the commitment that
//! belongs in the signed topic document — never in `config/proof-pin.toml`.
//!
//! Records never enter git. The commitment does. Production salts stay off
//! git. A documented salt from another challenge is refused so a Proof
//! holdout cannot be reconstructed from a public Relearn / Image / Agent
//! ceremony.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use proof_task::{
    holdout_commitment, synthetic_holdout, HoldoutRecord, HoldoutSplit, HOLDOUT_SIZE, STRATUM_SIZE,
};
use sha2::{Digest, Sha256};

/// Domain tag for Proof holdout selection. Distinct from the commitment domain.
const SELECT_DOMAIN: &[u8] = b"base-proof-holdout-select-v1";

/// Salts that must never select a live Proof set.
const REFUSED_SALTS: &[&str] = &[
    "cortex-t2i-dev-holdout-v0",
    "cortex-relearn-dev-holdout-v0",
    "cortex-agent-dev-holdout-v0",
];

/// Arguments for the per-topic holdout ceremony.
#[derive(Debug)]
pub struct HoldoutArgs {
    /// Topic id the records will be scored under (operator file key).
    pub topic_id: String,
    /// Catalogue JSON array of [`HoldoutRecord`]. Ignored when `synthetic` is set.
    pub catalog: Option<PathBuf>,
    /// Selection salt. Dev salts from other challenges are refused.
    pub salt: String,
    /// Holdout record count (must stay stratified: multiple of 5).
    pub size: usize,
    /// Record ids already published (repeatable).
    pub exclude: Vec<u32>,
    /// Build a local-only synthetic catalogue (never a production source).
    pub synthetic: bool,
    /// Where to write the records (never inside the repo).
    pub out: Option<PathBuf>,
}

/// Run the ceremony helper.
///
/// # Errors
///
/// Empty topic id, empty or refused salt, a missing catalogue, or a size the
/// catalogue cannot satisfy as a stratified set.
pub fn run(args: &HoldoutArgs) -> Result<(), String> {
    let topic_id = args.topic_id.trim();
    if topic_id.is_empty() {
        return Err("--topic-id must not be empty".into());
    }
    let salt = args.salt.trim();
    if salt.is_empty() {
        return Err("--salt must not be empty".into());
    }
    if REFUSED_SALTS.contains(&salt) {
        return Err(format!(
            "refusing another challenge's documented salt {salt:?}; use a private Proof salt"
        ));
    }
    if args.size == 0 || args.size % HoldoutSplit::SCORED.len() != 0 {
        return Err(format!(
            "--size must be a positive multiple of {} (got {})",
            HoldoutSplit::SCORED.len(),
            args.size
        ));
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
    let records = select(&rows, salt, args.size, &excluded)?;
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
            "wrote {} records for topic {topic_id} to {}",
            records.len(),
            out.display()
        );
    }

    println!("topic_id = {topic_id}");
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

fn read_catalog(path: &Path) -> Result<Vec<HoldoutRecord>, String> {
    let body = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&body).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// Local/CI catalogue. Production must use a private operator catalogue.
fn synthetic_catalog() -> Vec<HoldoutRecord> {
    synthetic_holdout(80, 1)
}

fn select(
    rows: &[HoldoutRecord],
    salt: &str,
    size: usize,
    excluded: &BTreeSet<u32>,
) -> Result<Vec<HoldoutRecord>, String> {
    let per_split = size / HoldoutSplit::SCORED.len();
    let mut out = Vec::new();
    for split in HoldoutSplit::SCORED {
        let mut ranked: Vec<([u8; 32], HoldoutRecord)> = rows
            .iter()
            .filter(|r| r.split == split && !excluded.contains(&r.id))
            .map(|r| (rank(salt, r.id), r.clone()))
            .collect();
        if ranked.len() < per_split {
            return Err(format!(
                "catalogue has {} eligible {} records after exclusions, need {per_split}",
                ranked.len(),
                split.as_str()
            ));
        }
        ranked.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
        out.extend(ranked.into_iter().take(per_split).map(|(_, r)| r));
    }
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
    fn selection_is_deterministic_and_stratified() {
        let rows = synthetic_catalog();
        let a = select(&rows, "salt-a", HOLDOUT_SIZE, &BTreeSet::new()).expect("a");
        let b = select(&rows, "salt-a", HOLDOUT_SIZE, &BTreeSet::new()).expect("b");
        let c = select(&rows, "salt-b", HOLDOUT_SIZE, &BTreeSet::new()).expect("c");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), HOLDOUT_SIZE);
        for split in HoldoutSplit::SCORED {
            assert_eq!(
                a.iter().filter(|r| r.split == split).count(),
                STRATUM_SIZE,
                "{}",
                split.as_str()
            );
        }
    }

    #[test]
    fn refuses_other_challenges_documented_salts() {
        for salt in REFUSED_SALTS {
            let err = run(&HoldoutArgs {
                topic_id: "dt-no-ib-v0".into(),
                catalog: None,
                salt: (*salt).to_owned(),
                size: HOLDOUT_SIZE,
                exclude: Vec::new(),
                synthetic: true,
                out: None,
            })
            .expect_err("other challenge salt");
            assert!(err.contains("documented salt"), "{err}");
        }
    }

    #[test]
    fn tracked_output_paths_are_refused() {
        assert!(refuse_repo_path(Path::new("config/holdout.json")).is_err());
        refuse_repo_path(Path::new("/root/.base-secrets/proof/dt-no-ib-v0.json")).expect("ok");
    }

    #[test]
    fn empty_topic_id_is_refused() {
        let err = run(&HoldoutArgs {
            topic_id: "  ".into(),
            catalog: None,
            salt: "private-proof-salt".into(),
            size: HOLDOUT_SIZE,
            exclude: Vec::new(),
            synthetic: true,
            out: None,
        })
        .expect_err("empty topic");
        assert!(err.contains("topic-id"), "{err}");
    }
}
