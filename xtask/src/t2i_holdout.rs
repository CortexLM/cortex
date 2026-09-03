//! Relearn T2I holdout ceremony helper.
//!
//! Selects a holdout slice of Qwen-Image-Bench prompts, writes the frozen
//! records to an operator file, and prints the commitment that belongs in
//! `config/relearn-t2i-pin.toml`.
//!
//! The records never enter git. The commitment does, and
//! `relearn_t2i_task::verify_holdout_prompts` checks one against the other at
//! boot, so an edited or wrong holdout file makes the service refuse
//! submissions rather than quietly scoring against the public split.
//!
//! Selection is `sha256(domain || salt || id)`, lowest digests first. That is
//! deterministic given the salt, which is what lets an operator regenerate the
//! same holdout on a new host without shipping the file around — and it is why
//! the production salt must stay off git.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use relearn_t2i_task::{frozen_prompt_commitment, FrozenPrompt};
use sha2::{Digest, Sha256};

/// Domain tag for holdout selection. Distinct from the commitment domain.
const SELECT_DOMAIN: &[u8] = b"base-relearn-t2i-holdout-select-v1";

/// Arguments for the holdout ceremony.
#[derive(Debug)]
pub struct HoldoutArgs {
    /// Bench prompt file (`qwen_image_bench_hf_v0518.jsonl` from the dataset).
    pub bench: PathBuf,
    /// Selection salt. Dev salts are fine locally; production salts stay off git.
    pub salt: String,
    /// Holdout prompt count.
    pub size: usize,
    /// Prompt ids already published in the pin's public split.
    pub exclude: Vec<u32>,
    /// Where to write the records (never inside the repo).
    pub out: Option<PathBuf>,
}

#[derive(Debug, serde::Deserialize)]
struct BenchRow {
    #[serde(rename = "ID")]
    id: u32,
    prompt_en: String,
}

/// Run the ceremony helper.
///
/// # Errors
///
/// Missing or malformed bench file, an empty salt, or a size the bench cannot
/// satisfy after exclusions.
pub fn run(args: &HoldoutArgs) -> Result<(), String> {
    if args.salt.trim().is_empty() {
        return Err("--salt must not be empty".into());
    }
    let rows = read_bench(&args.bench)?;
    let excluded: BTreeSet<u32> = args.exclude.iter().copied().collect();
    let records = select(&rows, &args.salt, args.size, &excluded)?;
    let commitment = frozen_prompt_commitment(&records);

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

    // Ids are deliberately not printed: stdout ends up in CI logs and shells.
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

fn read_bench(path: &Path) -> Result<Vec<BenchRow>, String> {
    let body = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut rows = Vec::new();
    for (i, line) in body.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let row: BenchRow = serde_json::from_str(t)
            .map_err(|e| format!("{}:{}: {e}", path.display(), i.saturating_add(1)))?;
        rows.push(row);
    }
    if rows.is_empty() {
        return Err(format!("{} contained no prompt rows", path.display()));
    }
    Ok(rows)
}

fn select(
    rows: &[BenchRow],
    salt: &str,
    size: usize,
    excluded: &BTreeSet<u32>,
) -> Result<Vec<FrozenPrompt>, String> {
    let mut ranked: Vec<([u8; 32], u32, &str)> = rows
        .iter()
        .filter(|r| !excluded.contains(&r.id) && !r.prompt_en.trim().is_empty())
        .map(|r| (rank(salt, r.id), r.id, r.prompt_en.as_str()))
        .collect();
    if ranked.len() < size {
        return Err(format!(
            "bench has {} eligible prompts after exclusions, need {size}",
            ranked.len()
        ));
    }
    ranked.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut out: Vec<FrozenPrompt> = ranked
        .into_iter()
        .take(size)
        .map(|(_, id, text)| FrozenPrompt {
            id,
            text: text.to_owned(),
            upsampled_json: None,
        })
        .collect();
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

    fn rows() -> Vec<BenchRow> {
        (1..=100)
            .map(|id| BenchRow {
                id,
                prompt_en: format!("prompt {id}"),
            })
            .collect()
    }

    #[test]
    fn selection_is_deterministic_per_salt() {
        let a = select(&rows(), "salt-a", 10, &BTreeSet::new()).expect("a");
        let b = select(&rows(), "salt-a", 10, &BTreeSet::new()).expect("b");
        let c = select(&rows(), "salt-b", 10, &BTreeSet::new()).expect("c");
        assert_eq!(a, b);
        assert_ne!(a, c, "a different salt must rotate the holdout");
        assert_eq!(a.len(), 10);
    }

    #[test]
    fn selection_honors_exclusions() {
        let all: BTreeSet<u32> = (1..=95).collect();
        let picked = select(&rows(), "salt", 5, &all).expect("picked");
        assert!(picked.iter().all(|r| r.id > 95));
    }

    #[test]
    fn oversized_request_fails_instead_of_reusing_public_ids() {
        let excluded: BTreeSet<u32> = (1..=99).collect();
        assert!(select(&rows(), "salt", 5, &excluded).is_err());
    }

    #[test]
    fn records_are_id_sorted_so_the_commitment_is_stable() {
        let picked = select(&rows(), "salt", 12, &BTreeSet::new()).expect("picked");
        let mut sorted = picked.clone();
        sorted.sort_by_key(|r| r.id);
        assert_eq!(picked, sorted);
        assert_eq!(
            frozen_prompt_commitment(&picked),
            frozen_prompt_commitment(&sorted)
        );
    }

    #[test]
    fn tracked_output_paths_are_refused() {
        assert!(refuse_repo_path(Path::new("config/holdout.json")).is_err());
        assert!(refuse_repo_path(Path::new("/repo/docs/holdout.json")).is_err());
        refuse_repo_path(Path::new("/root/.base-secrets/relearn-t2i-holdout.json"))
            .expect("secret path ok");
    }
}
