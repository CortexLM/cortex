//! Sign a Proof topic document with the `proof` row mini-secret.
//!
//! The signature is sr25519 over canonical JSON under `base-proof-topic-v1`,
//! the same key that signs this challenge's weight leaves. The secret stays
//! off git; this helper only writes the signed document (never under
//! `config/` or `docs/`).

use std::fs;
use std::path::{Path, PathBuf};

use proof_task::TopicDocument;

/// Arguments for the topic-signing helper.
#[derive(Debug)]
pub struct TopicArgs {
    /// Unsigned (or previously signed) topic JSON.
    pub input: PathBuf,
    /// Mini-secret file: raw 32 bytes or hex text. Never commit.
    pub secret: PathBuf,
    /// Where to write the signed JSON. Stdout when omitted.
    pub out: Option<PathBuf>,
}

/// Sign the topic document.
///
/// # Errors
///
/// Missing files, a malformed secret, or a tracked output path.
pub fn run(args: &TopicArgs) -> Result<(), String> {
    let body = fs::read_to_string(&args.input)
        .map_err(|e| format!("read {}: {e}", args.input.display()))?;
    let mut topic: TopicDocument =
        serde_json::from_str(&body).map_err(|e| format!("parse {}: {e}", args.input.display()))?;
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
}
