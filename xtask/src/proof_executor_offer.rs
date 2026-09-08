//! Proof eval executor offer ceremony helper.
//!
//! Builds the operator `EvalExecutorOffer` document (the `1x` Lium template
//! the digest-pinned `proof-eval` image is rented on, plus its proof
//! deadline), computes `config_commitment`, and validates it against
//! `config/proof-pin.toml` before writing. The document is operator state:
//! `PROOF_EVAL_EXECUTOR_OFFER_FILE` at boot, `POST /v1/admin/proof/executor`
//! to rotate. It never enters git and it never carries a secret.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use proof_executor::{EvalExecutorOffer, OfferStatus};
use proof_task::ProofPin;

/// Arguments for the executor offer ceremony.
#[derive(Debug)]
pub struct ExecutorOfferArgs {
    /// Workspace root; `--out` is refused when it would touch a tracked (or
    /// trackable) path under it.
    pub repo_root: PathBuf,
    /// Pin the offer must validate against (`config/proof-pin.toml`).
    pub pin: PathBuf,
    /// Immutable slug for this executor.
    pub offer_id: String,
    /// Lium template id, or the digest-scoped template name
    /// (`proof-eval-<12 hex>`). Defaults to the pin's digest-scoped name.
    pub template_id: Option<String>,
    /// Machine shape. Must be the pin `gpu_class` (`1x`).
    pub machine_shape: String,
    /// Proof deadline in seconds (`<=` pin ceiling).
    pub max_proof_deadline_s: u64,
    /// Bind the offer to the pin's eval image digest.
    pub bind_digest: bool,
    /// Publish closed (host cannot score until reopened).
    pub closed: bool,
    /// Where to write the JSON (never inside the repo). Stdout when omitted.
    pub out: Option<PathBuf>,
}

/// Build, validate, and emit the offer.
///
/// # Errors
///
/// Unreadable / invalid pin, or an offer the pin refuses (wrong shape,
/// deadline over the ceiling, template outside the allowlist, …).
pub fn run(args: &ExecutorOfferArgs) -> Result<(), String> {
    let body = fs::read_to_string(&args.pin)
        .map_err(|e| format!("read pin {}: {e}", args.pin.display()))?;
    let pin = ProofPin::from_toml(&body).map_err(|e| e.to_string())?;
    pin.validate().map_err(|e| e.to_string())?;
    let offer = build(&pin, args)?;
    let json = serde_json::to_string_pretty(&offer).map_err(|e| e.to_string())?;
    match &args.out {
        Some(out) => {
            refuse_repo_path(out, &args.repo_root)?;
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
            }
            fs::write(out, format!("{json}\n"))
                .map_err(|e| format!("write {}: {e}", out.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(out, fs::Permissions::from_mode(0o600));
            }
            println!(
                "executor offer {} ({}, {}, {}s) → {}",
                offer.offer_id,
                offer.lium_template_id,
                offer.machine_shape,
                offer.max_proof_deadline_s,
                out.display()
            );
        }
        None => println!("{json}"),
    }
    Ok(())
}

fn build(pin: &ProofPin, args: &ExecutorOfferArgs) -> Result<EvalExecutorOffer, String> {
    let hex = pin.eval_image_digest.trim().trim_start_matches("sha256:");
    let template_id = match args.template_id.as_deref().map(str::trim) {
        Some(t) if !t.is_empty() => t.to_owned(),
        _ => {
            let prefix = hex.get(..12).ok_or(
                "pin has no eval_image_digest; pass --template-id explicitly (pre-launch pin)",
            )?;
            format!("proof-eval-{prefix}")
        }
    };
    let mut offer = EvalExecutorOffer {
        offer_id: args.offer_id.trim().to_owned(),
        lium_template_id: template_id,
        machine_shape: args.machine_shape.trim().to_owned(),
        max_proof_deadline_s: args.max_proof_deadline_s,
        eval_image_digest: if args.bind_digest {
            pin.eval_image_digest.trim().to_owned()
        } else {
            String::new()
        },
        config_commitment: String::new(),
        status: if args.closed {
            OfferStatus::Closed
        } else {
            OfferStatus::Open
        },
    };
    offer.config_commitment = offer.expected_commitment();
    offer.validate(pin).map_err(|e| e.to_string())?;
    Ok(offer)
}

/// Absolute, symlink-resolved form of `out`, resolving through the deepest
/// existing ancestor so a not-yet-created file still normalizes.
fn resolve_out(out: &Path) -> Result<PathBuf, String> {
    let absolute = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("cwd: {e}"))?
            .join(out)
    };
    let mut existing = absolute.clone();
    let mut tail = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            break;
        };
        tail.push(name.to_owned());
        existing = existing
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| format!("no existing ancestor for {}", absolute.display()))?;
    }
    let mut resolved = existing
        .canonicalize()
        .map_err(|e| format!("resolve {}: {e}", existing.display()))?;
    for name in tail.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

/// `git <args>` in `repo_root`; `Ok(true)` on exit 0, `Ok(false)` on a
/// non-zero exit, `Err` when git itself cannot be run.
fn git_ok(repo_root: &Path, args: &[&str]) -> Result<bool, String> {
    Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .map_err(|e| format!("git {}: {e}", args.join(" ")))
}

/// A live offer is operator state: it must never land on a path git would
/// see. Outside the workspace anything goes. Inside it, the path must be
/// neither tracked (`git ls-files`) nor trackable (not matched by
/// `.gitignore`, e.g. `deploy/secrets/**` is fine, `README.md` or a new
/// `offer.json` at the root is not). When git cannot answer, refuse.
fn refuse_repo_path(out: &Path, repo_root: &Path) -> Result<(), String> {
    let resolved = resolve_out(out)?;
    let root = repo_root
        .canonicalize()
        .map_err(|e| format!("resolve workspace root {}: {e}", repo_root.display()))?;
    let Ok(rel) = resolved.strip_prefix(&root) else {
        return Ok(());
    };
    let rel = rel.to_string_lossy().into_owned();
    if rel.is_empty() {
        return Err("refusing to write a live executor offer over the workspace root".into());
    }
    if git_ok(&root, &["ls-files", "--error-unmatch", "--", &rel])? {
        return Err(format!(
            "refusing to write a live executor offer over tracked path {rel}"
        ));
    }
    if !git_ok(&root, &["check-ignore", "-q", "--", &rel])? {
        return Err(format!(
            "refusing to write a live executor offer under the workspace at {rel}: the path is \
             not gitignored and would be committed (use deploy/secrets/ or a path outside the repo)"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed_pin() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/proof-pin.toml")
    }

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .canonicalize()
            .expect("workspace root")
    }

    fn args(shape: &str, deadline: u64) -> ExecutorOfferArgs {
        ExecutorOfferArgs {
            repo_root: repo_root(),
            pin: committed_pin(),
            offer_id: "lium-1x-v0".into(),
            template_id: None,
            machine_shape: shape.into(),
            max_proof_deadline_s: deadline,
            bind_digest: true,
            closed: false,
            out: None,
        }
    }

    fn pin() -> ProofPin {
        let p =
            ProofPin::from_toml(&fs::read_to_string(committed_pin()).expect("pin")).expect("parse");
        p.validate().expect("valid");
        p
    }

    #[test]
    fn builds_a_valid_one_gpu_offer_on_the_committed_pin() {
        let p = pin();
        let offer = build(&p, &args("1x", 7_200)).expect("offer");
        assert_eq!(offer.machine_shape, "1x");
        assert!(offer.lium_template_id.starts_with("proof-eval-"));
        assert_eq!(offer.eval_image_digest, p.eval_image_digest);
        assert_eq!(offer.config_commitment, offer.expected_commitment());
        assert!(offer.is_open());
        offer.validate(&p).expect("validates");
        let round: EvalExecutorOffer =
            serde_json::from_str(&serde_json::to_string(&offer).expect("json")).expect("parse");
        assert_eq!(round, offer);
    }

    #[test]
    fn refuses_a_shape_or_deadline_the_pin_does_not_allow() {
        let p = pin();
        let err = build(&p, &args("8x", 7_200)).expect_err("8x");
        assert!(err.contains("machine_shape"), "{err}");
        let err = build(&p, &args("1x", 7_201)).expect_err("over ceiling");
        assert!(err.contains("max_proof_deadline_s"), "{err}");
        let mut unbound = args("1x", 600);
        unbound.template_id = Some("prism-recipe-v10".into());
        let err = build(&p, &unbound).expect_err("not digest-bound");
        assert!(err.contains("pinned eval image digest prefix"), "{err}");
        let digest_hex = p.eval_image_digest.trim_start_matches("sha256:");
        let mut off_list = args("1x", 600);
        off_list.template_id = Some(format!("other-{}", &digest_hex[..12]));
        let err = build(&p, &off_list).expect_err("allowlist");
        assert!(err.contains("allowed_lium_template_prefixes"), "{err}");
        let mut raw = args("1x", 600);
        raw.template_id = Some("f2f5e84c-3b09-4090-be83-1913eabd009e".into());
        let err = build(&p, &raw).expect_err("raw uuid");
        assert!(err.contains("raw Lium template id"), "{err}");
    }

    #[test]
    fn closed_offers_build() {
        let p = pin();
        let mut closed = args("1x", 600);
        closed.closed = true;
        assert!(!build(&p, &closed).expect("closed").is_open());
    }

    /// Any git-tracked path — not just names under config/ or docs/ — is
    /// refused, and so is an untracked path git would pick up. Only
    /// gitignored trees (`deploy/secrets/**`) and paths outside the
    /// workspace are writable.
    #[test]
    fn tracked_or_trackable_workspace_paths_are_refused() {
        let root = repo_root();
        for tracked in [
            "README.md",
            "Cargo.toml",
            "config/proof-pin.toml",
            "docs/PROOF.md",
            "xtask/src/proof_executor_offer.rs",
            "deploy/secrets/README.md",
        ] {
            let err = refuse_repo_path(&root.join(tracked), &root).expect_err(tracked);
            assert!(err.contains("tracked path"), "{tracked}: {err}");
            // Relative form (as typed on the command line) is resolved too.
            let cwd = std::env::current_dir().expect("cwd");
            if cwd == root {
                assert!(
                    refuse_repo_path(Path::new(tracked), &root).is_err(),
                    "{tracked}"
                );
            }
        }
        for trackable in [
            "eval_executor_offer.json",
            "docs/executor.json",
            "config/new-dir/offer.json",
            "crates/proof-executor/offer.json",
        ] {
            let err = refuse_repo_path(&root.join(trackable), &root).expect_err(trackable);
            assert!(err.contains("not gitignored"), "{trackable}: {err}");
        }
        refuse_repo_path(
            &root.join("deploy/secrets/proof/eval_executor_offer.json"),
            &root,
        )
        .expect("gitignored operator tree inside the workspace");
        refuse_repo_path(&root.join("deploy/env/proof-challenge.env"), &root)
            .expect("gitignored env file");
        refuse_repo_path(
            Path::new("/root/.base-secrets/proof/eval_executor_offer.json"),
            &root,
        )
        .expect("outside the workspace");
        assert!(refuse_repo_path(&root, &root).is_err(), "the root itself");
    }

    /// The end-to-end command refuses to touch a tracked file and leaves its
    /// bytes intact.
    #[test]
    fn run_never_overwrites_a_tracked_file() {
        let root = repo_root();
        let readme = root.join("README.md");
        let before = fs::read(&readme).expect("README");
        let mut a = args("1x", 600);
        a.out = Some(readme.clone());
        let err = run(&a).expect_err("tracked README");
        assert!(err.contains("tracked path"), "{err}");
        assert_eq!(
            fs::read(&readme).expect("README"),
            before,
            "README must be untouched"
        );

        let mut b = args("1x", 600);
        b.out = Some(root.join("brand-new-offer.json"));
        let err = run(&b).expect_err("would be committed");
        assert!(err.contains("not gitignored"), "{err}");
        assert!(!root.join("brand-new-offer.json").exists());
    }
}
