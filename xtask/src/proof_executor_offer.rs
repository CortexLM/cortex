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

use proof_executor::{EvalExecutorOffer, OfferStatus};
use proof_task::ProofPin;

/// Arguments for the executor offer ceremony.
#[derive(Debug)]
pub struct ExecutorOfferArgs {
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
            refuse_repo_path(out)?;
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

fn refuse_repo_path(out: &Path) -> Result<(), String> {
    let text = out.to_string_lossy();
    if text.contains("/config/") || text.starts_with("config/") || text.contains("/docs/") {
        return Err(format!(
            "refusing to write a live executor offer under a tracked path: {text}"
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

    fn args(shape: &str, deadline: u64) -> ExecutorOfferArgs {
        ExecutorOfferArgs {
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
        let mut off_list = args("1x", 600);
        off_list.template_id = Some("prism-recipe-v10".into());
        let err = build(&p, &off_list).expect_err("allowlist");
        assert!(err.contains("allowed_lium_template_prefixes"), "{err}");
    }

    #[test]
    fn closed_offers_and_tracked_paths() {
        let p = pin();
        let mut closed = args("1x", 600);
        closed.closed = true;
        assert!(!build(&p, &closed).expect("closed").is_open());
        assert!(refuse_repo_path(Path::new("config/eval_executor_offer.json")).is_err());
        assert!(refuse_repo_path(Path::new("/x/docs/offer.json")).is_err());
        refuse_repo_path(Path::new(
            "/root/.base-secrets/proof/eval_executor_offer.json",
        ))
        .expect("operator path");
    }
}
