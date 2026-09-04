//! Sign a Proof topic document with the `proof` row mini-secret.
//!
//! Accepts a YAML or JSON draft (`statement`, `validation`, `payout_mode`,
//! metric, …). Fills `holdout_commitment` from `--holdout` or `--synthetic`,
//! then signs. The signature is sr25519 over canonical JSON under
//! `base-proof-topic-v1`. The secret stays off git; this helper only writes
//! the signed document (never under `config/` or `docs/`).

use std::fs;
use std::path::{Path, PathBuf};

use proof_task::{
    holdout_commitment, synthetic_holdout, TopicDocument, HOLDOUT_SIZE, STRATUM_SIZE,
};

/// Arguments for the topic-signing helper.
#[derive(Debug)]
pub struct TopicArgs {
    /// Unsigned YAML/JSON draft (or previously signed JSON).
    pub input: PathBuf,
    /// Mini-secret file: raw 32 bytes or hex text. Never commit.
    pub secret: PathBuf,
    /// Where to write the signed JSON. Stdout when omitted.
    pub out: Option<PathBuf>,
    /// Holdout records used to fill a missing commitment.
    pub holdout: Option<PathBuf>,
    /// Build a synthetic holdout commitment (dev only).
    pub synthetic: bool,
}

/// Sign the topic document.
///
/// # Errors
///
/// Missing files, a malformed secret, or a tracked output path.
pub fn run(args: &TopicArgs) -> Result<(), String> {
    let body = fs::read_to_string(&args.input)
        .map_err(|e| format!("read {}: {e}", args.input.display()))?;
    let mut topic =
        parse_draft(&body).map_err(|e| format!("parse {}: {e}", args.input.display()))?;
    fill_holdout(&mut topic, args)?;
    let secret = load_mini_secret(&args.secret)?;
    topic.signature = topic.sign_with(&secret).map_err(|e| e.to_string())?;
    let out_body =
        serde_json::to_string_pretty(&topic).map_err(|e| format!("serialize topic: {e}"))?;
    if let Some(out) = &args.out {
        refuse_repo_path(out)?;
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        fs::write(out, format!("{out_body}\n"))
            .map_err(|e| format!("write {}: {e}", out.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(out, fs::Permissions::from_mode(0o600));
        }
        println!("signed topic {} → {}", topic.id, out.display());
    } else {
        println!("{out_body}");
    }
    Ok(())
}

fn parse_draft(body: &str) -> Result<TopicDocument, String> {
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return serde_json::from_str(trimmed).map_err(|e| e.to_string());
    }
    serde_yaml::from_str(trimmed).map_err(|e| e.to_string())
}

fn fill_holdout(topic: &mut TopicDocument, args: &TopicArgs) -> Result<(), String> {
    if is_hex64(&topic.holdout_commitment) {
        if topic.holdout_size == 0 {
            topic.holdout_size = HOLDOUT_SIZE;
        }
        return Ok(());
    }
    let recs = if args.synthetic {
        synthetic_holdout(STRATUM_SIZE, 1)
    } else if let Some(path) = &args.holdout {
        let body = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        serde_json::from_str(&body).map_err(|e| format!("parse holdout {}: {e}", path.display()))?
    } else {
        return Err(
            "holdout_commitment is empty: pass --synthetic or --holdout so the helper can fill it"
                .into(),
        );
    };
    topic.holdout_commitment = holdout_commitment(&recs);
    topic.holdout_size = HOLDOUT_SIZE;
    Ok(())
}

fn is_hex64(s: &str) -> bool {
    let t = s.trim();
    t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())
}

fn refuse_repo_path(out: &Path) -> Result<(), String> {
    let text = out.to_string_lossy();
    if text.contains("/config/") || text.starts_with("config/") || text.contains("/docs/") {
        return Err(format!(
            "refusing to write a signed topic under a tracked path: {text}"
        ));
    }
    Ok(())
}

fn load_mini_secret(path: &Path) -> Result<[u8; 32], String> {
    let raw = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if raw.len() == 32 {
        return <[u8; 32]>::try_from(raw).map_err(|_| "secret must be 32 bytes".into());
    }
    let text = std::str::from_utf8(&raw)
        .map_err(|_| format!("{} is not utf-8 hex", path.display()))?
        .trim();
    let hex = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    let bytes = hex::decode(hex).map_err(|e| format!("decode {}: {e}", path.display()))?;
    <[u8; 32]>::try_from(bytes).map_err(|_| format!("{} must decode to 32 bytes", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_output_paths_are_refused() {
        assert!(refuse_repo_path(Path::new("config/topics.json")).is_err());
        refuse_repo_path(Path::new("/root/.base-secrets/proof/topics.json")).expect("ok");
    }

    #[test]
    fn yaml_draft_parses_payout_mode_and_validation() {
        let body = r#"
id: agent-harness-improve-v0
statement: "Improve the agent harness."
payout_mode: discovery
validation:
  score_on: "Holdout harness success rate vs sealed baseline"
  accept_if: "Reproduced under budget; no contamination; beat baseline by epsilon"
  reject_if: "Unreproduced claim; eval short-circuit"
metric:
  family: custom
  custom_id: harness_success_rate
  primary: success_rate
  direction: max
  epsilon_rel: 0.05
flops_budget: 2000000000000000000
status: draft
"#;
        let doc = parse_draft(body).expect("yaml");
        assert_eq!(doc.id, "agent-harness-improve-v0");
        assert_eq!(doc.payout_mode, proof_task::PayoutMode::Discovery);
        assert!(doc.validation.score_on.contains("success rate"));
        assert_eq!(doc.metric.custom_id, "harness_success_rate");
        assert_eq!(doc.status, proof_task::TopicStatus::Draft);
    }
}
