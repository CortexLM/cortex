//! Relearn Agent episode ceremony helper.
//!
//! Selects a holdout episode set from an operator catalogue (or a local
//! synthetic one), writes the frozen episodes to an operator file, and prints
//! the commitment that belongs in `config/relearn-agent-pin.toml`.
//!
//! The episodes never enter git. The commitment does. Production salts and
//! catalogues stay off git — a documented salt plus a public catalogue would
//! let miners reconstruct the scored set.
//!
//! Selection is `sha256(domain || salt || id)`, lowest digests first. The
//! domain and salt are distinct from every other challenge's ceremony.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use relearn_agent_task::{episode_commitment, AgentEpisode};
use sha2::{Digest, Sha256};

/// Domain tag for episode selection. Distinct from the commitment domain.
const SELECT_DOMAIN: &[u8] = b"base-relearn-agent-holdout-select-v1";

/// Salts that must never select a live set.
const REFUSED_SALTS: &[&str] = &["cortex-t2i-dev-holdout-v0", "cortex-agent-dev-holdout-v0"];

/// Arguments for the episode ceremony.
#[derive(Debug)]
pub struct HoldoutArgs {
    /// Catalogue JSON array of [`AgentEpisode`]. Ignored when `synthetic` is set.
    pub catalog: Option<PathBuf>,
    /// Selection salt. Dev salts are local-only; production salts stay off git.
    pub salt: String,
    /// Holdout episode count.
    pub size: usize,
    /// Episode ids already published in the pin's public split.
    pub exclude: Vec<u32>,
    /// Build a local-only synthetic catalogue (never a production source).
    pub synthetic: bool,
    /// Where to write the episodes (never inside the repo).
    pub out: Option<PathBuf>,
}

/// Run the ceremony helper.
///
/// # Errors
///
/// Empty or refused salt, a missing catalogue, or a size the catalogue cannot
/// satisfy.
pub fn run(args: &HoldoutArgs) -> Result<(), String> {
    let salt = args.salt.trim();
    if salt.is_empty() {
        return Err("--salt must not be empty".into());
    }
    if REFUSED_SALTS.contains(&salt) {
        return Err(format!(
            "refusing the documented dev salt {salt:?}; use a private Relearn Agent salt"
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
    let episodes = select(&rows, salt, args.size, &excluded)?;
    let commitment = episode_commitment(&episodes);

    if let Some(out) = &args.out {
        refuse_repo_path(out)?;
        let body = serde_json::to_string_pretty(&episodes)
            .map_err(|e| format!("serialize episodes: {e}"))?;
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        fs::write(out, format!("{body}\n")).map_err(|e| format!("write {}: {e}", out.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(out, fs::Permissions::from_mode(0o600));
        }
        println!("wrote {} episodes to {}", episodes.len(), out.display());
    }

    println!("holdout_size = {}", episodes.len());
    println!("holdout_commitment = \"{commitment}\"");
    Ok(())
}

fn refuse_repo_path(out: &Path) -> Result<(), String> {
    let text = out.to_string_lossy();
    if text.contains("/config/") || text.starts_with("config/") || text.contains("/docs/") {
        return Err(format!(
            "refusing to write episodes under a tracked path: {text}"
        ));
    }
    Ok(())
}

fn read_catalog(path: &Path) -> Result<Vec<AgentEpisode>, String> {
    let body = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&body).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// Local/CI catalogue. Production must use a private operator catalogue.
fn synthetic_catalog() -> Vec<AgentEpisode> {
    (1..=200)
        .map(|id| {
            AgentEpisode::synthetic(
                id,
                format!(
                    "synthetic agent episode {id}: recover a figure that only appears after \
                     inspecting the attached record"
                ),
            )
        })
        .collect()
}

fn select(
    rows: &[AgentEpisode],
    salt: &str,
    size: usize,
    excluded: &BTreeSet<u32>,
) -> Result<Vec<AgentEpisode>, String> {
    let mut ranked: Vec<([u8; 32], AgentEpisode)> = rows
        .iter()
        .filter(|e| {
            !excluded.contains(&e.id)
                && !e.goal.trim().is_empty()
                && !e.tools.is_empty()
                && !e.steps.is_empty()
        })
        .map(|e| (rank(salt, e.id), e.clone()))
        .collect();
    if ranked.len() < size {
        return Err(format!(
            "catalogue has {} eligible episodes after exclusions, need {size}",
            ranked.len()
        ));
    }
    ranked.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
    let mut out: Vec<AgentEpisode> = ranked.into_iter().take(size).map(|(_, e)| e).collect();
    out.sort_by_key(|e| e.id);
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
        assert!(picked.iter().all(|e| e.id > 190));
    }

    /// Every selected episode must require a tool call, or the set cannot
    /// separate an agent from a model that memorised the answers.
    #[test]
    fn every_selected_episode_needs_the_environment() {
        let rows = synthetic_catalog();
        let picked = select(&rows, "salt", 50, &BTreeSet::new()).expect("picked");
        assert!(picked
            .iter()
            .all(|e| !e.steps.is_empty() && !e.tools.is_empty()));
    }

    #[test]
    fn refuses_the_documented_dev_salts() {
        for salt in REFUSED_SALTS {
            let err = run(&HoldoutArgs {
                catalog: None,
                salt: (*salt).to_owned(),
                size: 10,
                exclude: Vec::new(),
                synthetic: true,
                out: None,
            })
            .expect_err("dev salt");
            assert!(err.contains("dev salt"), "{err}");
        }
    }

    #[test]
    fn tracked_output_paths_are_refused() {
        assert!(refuse_repo_path(Path::new("config/episodes.json")).is_err());
        refuse_repo_path(Path::new("/root/.base-secrets/relearn-agent-episodes.json")).expect("ok");
    }
}
