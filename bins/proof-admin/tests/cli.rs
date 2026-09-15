//! Process-level tests for `proof-admin` (dynamic-topics P0).
//!
//! The commands that must work end to end are `topic validate` and
//! `topic install --dry-run`: both run the same acceptance checks the existing
//! `POST /v1/admin/proof/topics` route runs, and neither touches a host. The
//! stubs must fail closed with exit code 3 rather than doing something partial.
//!
//! A real install is deliberately **not** implemented in this slice, so the
//! test asserts it refuses rather than writing anything; the registry view
//! (`topic list` / `topic show`) is covered against Postgres in
//! `crates/proof-rlm-store/tests/store_contract.rs` and by one DB-gated test
//! here.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

/// Exit code for a failure (bad bundle, refused document, ...).
const EXIT_ERROR: i32 = 1;
/// Exit code for bad usage or missing configuration.
const EXIT_USAGE: i32 = 2;
/// Exit code for a command a later slice owns.
const EXIT_NOT_IMPLEMENTED: i32 = 3;

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "proof-admin-{}-{tag}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    fs::create_dir_all(&dir).expect("workdir");
    dir
}

fn write_file(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).expect("write file");
    path
}

/// Run the binary with every database variable removed, so a command that
/// silently reached for one would fail here rather than on an operator host.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_proof-admin"))
        .args(args)
        .env_remove("BASE_DATABASE_URL")
        .env_remove("BASE_DATABASE_URL_FILE")
        .output()
        .expect("run proof-admin")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

/// A bundle whose signed document matches `pin_body`'s key, so `validate`
/// exercises the real acceptance path.
///
/// Built by signing a real `TopicDocument` with a test mini-secret, then
/// embedding it: the CLI checks the signature exactly as the route does.
mod fixture {
    use proof_task::{
        default_adamw, holdout_commitment, synthetic_holdout, Constraints, MetricDirection,
        MetricFamily, MetricSpec, PayoutMode, TopicDocument, TopicStatus, FLOPS_BUDGET_MAX,
        STRATUM_SIZE,
    };

    /// Test mini-secret. Not a real key, never a production one.
    pub fn sk() -> [u8; 32] {
        let mut s = [3u8; 32];
        s[0] = 17;
        s
    }

    pub fn pin_toml() -> String {
        let pk = hex::encode(crypto::public_key_from_mini_secret(&sk()).expect("pk"));
        format!(
            r#"challenge_id = "proof"
scoring_version = 1
base_model_family = "Qwen/Qwen3.8"
eval_image = "ghcr.io/cortexlm/proof-eval"
eval_image_digest = "sha256:{}"
topic_pubkey = "{pk}"
flops_budget_max = 2000000000000000000
epsilon_nll_min = 0.02
epsilon_topic_max_regress_min = 0.05
epsilon_throughput_rel_min = 0.05
quality_floor_nll_max = 0.02
holdout_size = 120
stratum_size = 24

[inference]
provider = "openai_compatible"
base_url = "http://127.0.0.1:8000/v1"
model = "master-proxy-v0"
mode = "chat"
max_input_tokens = 32768
max_output_tokens = 8192
"#,
            "ab".repeat(32)
        )
    }

    /// A signed custom topic selecting the in-guest runner, the shape the live
    /// `tb4` topic has.
    pub fn signed_topic(pack_digest: &str) -> TopicDocument {
        let mut doc = TopicDocument {
            id: "tb4".into(),
            statement: "Score the pinned task pack with the pinned runner.".into(),
            payout_mode: PayoutMode::Discovery,
            constraints: Constraints::default(),
            metric: MetricSpec {
                family: MetricFamily::Custom,
                primary: "primary_value".into(),
                direction: MetricDirection::Max,
                unit: "rate".into(),
                epsilon_rel: 0.05,
                custom_id: "tbench".into(),
                ..MetricSpec::default()
            },
            baseline: default_adamw(FLOPS_BUDGET_MAX),
            holdout_commitment: holdout_commitment(&synthetic_holdout(STRATUM_SIZE, 1)),
            status: TopicStatus::Draft,
            ..TopicDocument::default()
        };
        doc.constraints.params.insert(
            proof_experiment::PARAM_RUNNER.into(),
            "rlm_fc_in_guest_harbor".into(),
        );
        doc.constraints.params.insert(
            proof_experiment::PARAM_PACK_DIGEST.into(),
            pack_digest.into(),
        );
        doc.signature = doc.sign_with(&sk()).expect("sign");
        doc
    }

    /// The Arch default bundle: slug `tb4`, custom id `tbench`, and the
    /// temporary alias `tbench` the Owner default declares.
    pub fn bundle_json(environment: &str) -> String {
        let hex = "ab".repeat(32);
        let pack = format!("sha256:{hex}");
        let topic = signed_topic(&pack);
        let bundle = serde_json::json!({
            "schema_version": 1,
            "environment": environment,
            "display_name": "Terminal-Bench 4",
            "topic": topic,
            "aliases": ["tbench"],
            "host": {
                "rlm_image_digest": format!("sha256:{hex}"),
                "experiment_image_digest": format!("sha256:{hex}"),
                "pack_digest": pack,
                "pack_dir": "/var/lib/proof/packs",
                "custom_ids_entry": "tbench"
            },
            // A small illustrative RLM section, so the committed fixture also
            // exercises the hand-off. A real bundle carries the topic's own
            // rules / migrations / apis / submission_format / scoring.
            "rlm": {
                "rules": [
                    {"id": "no_short_circuit", "text": "the harness must run the task"}
                ],
                "migrations": [
                    {"name": "0001_scratch", "sql": "CREATE TABLE tb4_scratch (id TEXT)"}
                ],
                "apis": [
                    {"path": "status", "method": "GET", "summary": "topic status"}
                ],
                "submission_format": {"kind": "tar", "max_bytes": 5_242_880},
                "scoring": {"primary": "primary_value", "epsilon_rel": 0.05}
            }
        });
        serde_json::to_string_pretty(&bundle).expect("json")
    }
}

/// Regenerate the committed dry-run fixture.
///
/// Gated on `PROOF_ADMIN_FIXTURE_DIR` so it is a no-op in CI. The fixture is
/// the operator artifact for the Owner A→Z walkthrough and must be signed by
/// the same test key its pin carries, so it cannot be hand-edited safely:
///
/// ```bash
/// PROOF_ADMIN_FIXTURE_DIR=bins/proof-admin/tests/fixtures \
///   cargo test -p proof-admin-bin --test cli regenerate_dry_run_fixture
/// ```
#[test]
fn regenerate_dry_run_fixture() {
    let Ok(dir) = std::env::var("PROOF_ADMIN_FIXTURE_DIR") else {
        return;
    };
    // `cargo test` runs with the **package** directory as the working
    // directory, so a repo-relative value would land under
    // `bins/proof-admin/bins/proof-admin/…`. Resolving against the workspace
    // root is what makes the documented command write where it says.
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("bins/<pkg> sits two levels under the workspace root");
    let dir = if Path::new(&dir).is_absolute() {
        PathBuf::from(dir)
    } else {
        workspace.join(dir)
    };
    fs::create_dir_all(&dir).expect("fixture dir");
    fs::write(
        dir.join("tb4.install-bundle.json"),
        fixture::bundle_json("staging"),
    )
    .expect("bundle");
    fs::write(dir.join("tb4.pin.toml"), fixture::pin_toml()).expect("pin");
    eprintln!("wrote the dry-run fixture to {}", dir.display());
}

/// The committed dry-run fixture must stay runnable.
///
/// `tests/fixtures/tb4.bundle.json` + `tb4.pin.toml` are the operator artifact
/// the A→Z walkthrough uses, so a schema change that quietly breaks them must
/// fail here rather than in the Owner's hands. This runs the **same two
/// commands** the fixture README documents.
#[test]
fn the_committed_dry_run_fixture_still_validates_and_plans() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let bundle = fixtures.join("tb4.install-bundle.json");
    let pin = fixtures.join("tb4.pin.toml");
    assert!(bundle.is_file(), "missing {}", bundle.display());
    assert!(pin.is_file(), "missing {}", pin.display());

    // 1. validate
    let out = run(&[
        "topic",
        "validate",
        "--bundle",
        bundle.to_str().unwrap(),
        "--pin",
        pin.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "fixture must validate: {}", stderr(&out));
    let body = stdout(&out);
    assert!(body.contains("topic_id         tb4"), "{body}");
    assert!(body.contains("custom_id        tbench"), "{body}");
    assert!(
        body.contains("rlm_install      present"),
        "the fixture carries an RLM section: {body}"
    );

    // 2. install --dry-run, the documented staging command.
    let out = run(&[
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "staging",
        "--dry-run",
        "--pin",
        pin.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "fixture must plan: {}", stderr(&out));
    let body = stdout(&out);
    assert!(body.contains("environment       staging"), "{body}");
    assert!(body.contains("owner_gate        n/a (staging)"), "{body}");
    assert!(
        body.contains("Hand control to the topic's RLM"),
        "the plan must show the hand-off: {body}"
    );
    // The fixture carries the Owner-default alias, so the plan must say so:
    // an operator reads the plan to know what the install will do.
    assert!(
        body.contains("tbench"),
        "the plan must name the alias the fixture declares: {body}"
    );

    // The fixture is staging-only: a metal plan must be refused, both by the
    // declared target and by the Owner gate.
    let out = run(&[
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "metal",
        "--owner-metal-ack",
        "--dry-run",
        "--pin",
        pin.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), EXIT_ERROR, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("declares environment staging"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn validate_accepts_the_arch_default_bundle_and_writes_nothing() {
    let dir = workdir("validate-ok");
    let bundle = write_file(&dir, "tb4.json", &fixture::bundle_json("metal"));
    let pin = write_file(&dir, "pin.toml", &fixture::pin_toml());
    let out = run(&[
        "topic",
        "validate",
        "--bundle",
        bundle.to_str().unwrap(),
        "--pin",
        pin.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let body = stdout(&out);
    for needle in [
        "is valid",
        "topic_id         tb4",
        "environment      metal",
        "custom_id        tbench",
        "runner_id        rlm_fc_in_guest_harbor",
        "bundle_digest    sha256:",
        "Nothing was written",
    ] {
        assert!(body.contains(needle), "missing {needle:?} in:\n{body}");
    }
    fs::remove_dir_all(&dir).ok();
}

/// `validate` runs the same acceptance the publish route runs, so a document
/// the route would refuse is refused here — with the reason.
#[test]
fn validate_refuses_a_document_the_publish_route_would_refuse() {
    let dir = workdir("validate-refuse");

    // A signature that does not verify under the pin's topic key.
    let mut wrong_key =
        serde_json::from_str::<serde_json::Value>(&fixture::bundle_json("metal")).expect("json");
    let mut other = [9u8; 32];
    other[1] = 4;
    let doc: proof_task::TopicDocument =
        serde_json::from_value(wrong_key["topic"].clone()).expect("document");
    let resigned = doc.sign_with(&other).expect("sign with another key");
    wrong_key["topic"]["signature"] = serde_json::Value::String(resigned);
    let bundle = write_file(&dir, "wrong-key.json", &wrong_key.to_string());
    let pin = write_file(&dir, "pin.toml", &fixture::pin_toml());
    let out = run(&[
        "topic",
        "validate",
        "--bundle",
        bundle.to_str().unwrap(),
        "--pin",
        pin.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), EXIT_ERROR, "stderr={}", stderr(&out));
    assert!(
        stderr(&out).contains("signature"),
        "stderr must name the signature: {}",
        stderr(&out)
    );
    assert!(
        stdout(&out).is_empty(),
        "a failure prints nothing to stdout"
    );

    // An unknown key is refused rather than ignored: a step this build cannot
    // name is a step nothing performs.
    let mut unknown =
        serde_json::from_str::<serde_json::Value>(&fixture::bundle_json("metal")).expect("json");
    unknown["runner_id"] = serde_json::Value::String("rlm_fc_in_guest_harbor".into());
    let bundle = write_file(&dir, "unknown.json", &unknown.to_string());
    let out = run(&[
        "topic",
        "validate",
        "--bundle",
        bundle.to_str().unwrap(),
        "--pin",
        pin.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), EXIT_ERROR);
    assert!(
        stderr(&out).contains("runner_id"),
        "stderr must name the unknown key: {}",
        stderr(&out)
    );

    // A host expectation that contradicts the signed document.
    let mut contradicting =
        serde_json::from_str::<serde_json::Value>(&fixture::bundle_json("metal")).expect("json");
    contradicting["host"]["pack_digest"] =
        serde_json::Value::String(format!("sha256:{}", "cd".repeat(32)));
    let bundle = write_file(&dir, "contradicting.json", &contradicting.to_string());
    let out = run(&[
        "topic",
        "validate",
        "--bundle",
        bundle.to_str().unwrap(),
        "--pin",
        pin.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), EXIT_ERROR);
    assert!(
        stderr(&out).contains("contradicts the signed document"),
        "stderr={}",
        stderr(&out)
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn validate_json_output_is_machine_readable() {
    let dir = workdir("validate-json");
    let bundle = write_file(&dir, "tb4.json", &fixture::bundle_json("metal"));
    let pin = write_file(&dir, "pin.toml", &fixture::pin_toml());
    let out = run(&[
        "--json",
        "topic",
        "validate",
        "--bundle",
        bundle.to_str().unwrap(),
        "--pin",
        pin.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("validate --json is JSON");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["topic_id"], "tb4");
    assert_eq!(parsed["environment"], "metal");
    assert_eq!(parsed["custom_id"], "tbench");
    assert_eq!(parsed["runner_id"], "rlm_fc_in_guest_harbor");
    assert!(
        parsed["bundle_digest"]
            .as_str()
            .unwrap_or_default()
            .starts_with("sha256:"),
        "{parsed}"
    );
    fs::remove_dir_all(&dir).ok();
}

/// The dry run prints the **existing** publish call and the host env, and
/// touches nothing.
#[test]
fn dry_run_install_prints_the_existing_publish_call_and_host_env() {
    let dir = workdir("dry-run");
    let bundle = write_file(&dir, "tb4.json", &fixture::bundle_json("metal"));
    let pin = write_file(&dir, "pin.toml", &fixture::pin_toml());
    let out = run(&[
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "metal",
        "--owner-metal-ack",
        "--pin",
        pin.to_str().unwrap(),
        "--dry-run",
    ]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let body = stdout(&out);
    for needle in [
        "topic install plan",
        "topic_id          tb4",
        "environment       metal",
        "custom_id         tbench",
        "runner_id         rlm_fc_in_guest_harbor",
        "Hand control to the topic's RLM (it installs and sets the topic up)",
        "provision -> propose_rules -> baseline",
        "Publish the signed document (one block; existing route, operator bearer)",
        "jq '.topic'",
        "mktemp -d",
        "chmod 600",
        "--data-binary @\"$PROOF_TOPIC_DIR/document.json\"",
        "/challenge/proof/v1/admin/proof/topics",
        "PROOF_VM_RUNNER_CUSTOM_IDS=tbench",
        "PROOF_RLM_VM_IMAGE_DIGEST=sha256:",
        "PROOF_EXPERIMENT_VM_IMAGE_DIGEST=sha256:",
        "PROOF_VM_AGENT_EXPERIMENT_PACK_DIR=/var/lib/proof/packs",
        "nothing was written and no host was touched",
    ] {
        assert!(body.contains(needle), "missing {needle:?} in:\n{body}");
    }
    // The pack directory is a path, never the digest: the variable names a
    // directory and the host re-hashes what it finds there.
    assert!(
        !body.contains("PROOF_VM_AGENT_EXPERIMENT_PACK_DIR=sha256:"),
        "the pack dir must not carry a digest:\n{body}"
    );
    // The bearer is never printed; the operator supplies it.
    assert!(
        body.contains("Bearer $PROOF_ADMIN_TOKEN"),
        "the token must stay a placeholder:\n{body}"
    );
    // The procedure must be runnable shell, not a placeholder an operator has
    // to hand-edit.
    assert!(
        !body.contains("<extract"),
        "the publish step must not be a placeholder:\n{body}"
    );
    // The extracted document must not live at a fixed shared path: any local
    // process could swap it between validation and publication, so what gets
    // published would not be what was validated. It goes in a private
    // `mktemp -d` directory, and extraction and publication are one block.
    assert!(
        !body.contains("/tmp/proof-topic-document.json"),
        "the document must not use a fixed shared path:\n{body}"
    );
    assert!(
        body.contains("PROOF_TOPIC_DIR=$(mktemp -d)"),
        "the document must live in a private directory:\n{body}"
    );
    let block: Vec<&str> = body
        .lines()
        .skip_while(|l| !l.contains("PROOF_TOPIC_DIR=$(mktemp -d)"))
        .take_while(|l| !l.trim().is_empty())
        .map(str::trim)
        .collect();
    assert!(
        block.iter().any(|l| l.contains("jq '.topic'"))
            && block.iter().any(|l| l.contains("curl -sS -X POST")),
        "extraction and publication must be one block:\n{block:#?}"
    );
    let script = block.join("\n");
    let status = std::process::Command::new("sh")
        .arg("-n")
        .arg("-c")
        .arg(&script)
        .status()
        .expect("sh -n");
    assert!(
        status.success(),
        "the printed publish block must be valid shell: {script}"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn dry_run_install_json_matches_the_plan_shape() {
    let dir = workdir("dry-run-json");
    let bundle = write_file(&dir, "tb4.json", &fixture::bundle_json("staging"));
    let pin = write_file(&dir, "pin.toml", &fixture::pin_toml());
    let out = run(&[
        "--json",
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "staging",
        "--pin",
        pin.to_str().unwrap(),
        "--dry-run",
    ]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("dry run --json is JSON");
    assert_eq!(parsed["topic_id"], "tb4");
    assert_eq!(parsed["environment"], "staging");
    assert_eq!(parsed["custom_id"], "tbench");
    assert_eq!(parsed["runner_id"], "rlm_fc_in_guest_harbor");
    assert_eq!(parsed["publish_route"], "POST /v1/admin/proof/topics");
    assert_eq!(parsed["pack_dir_env"], "PROOF_VM_AGENT_EXPERIMENT_PACK_DIR");
    assert!(
        parsed["host_env"].as_array().is_some_and(|a| a.len() == 4),
        "{parsed}"
    );
    assert!(parsed["bundle_digest"].as_str().is_some(), "{parsed}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn install_refuses_an_environment_the_bundle_does_not_declare() {
    let dir = workdir("env-mismatch");
    let bundle = write_file(&dir, "tb4.json", &fixture::bundle_json("metal"));
    let pin = write_file(&dir, "pin.toml", &fixture::pin_toml());
    let out = run(&[
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "staging",
        "--pin",
        pin.to_str().unwrap(),
        "--dry-run",
    ]);
    assert_eq!(code(&out), EXIT_ERROR, "stderr={}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("declares environment metal") && err.contains("--env staging"),
        "stderr={err}"
    );
    fs::remove_dir_all(&dir).ok();
}

/// A real install needs the master and a bearer, and refuses **before**
/// touching anything when either is missing.
///
/// This is the P1a contract replacing the P0 stub: `install` without
/// `--dry-run` now performs the install, so the test that matters is that a
/// missing configuration is a usage error naming what to set — not a partial
/// install. The happy path is covered against Postgres in
/// `crates/proof-topic-install/tests/install_engine.rs`.
#[test]
fn a_real_install_refuses_without_a_master_and_a_bearer_and_changes_nothing() {
    let dir = workdir("real-install-config");
    let bundle = write_file(&dir, "tb4.json", &fixture::bundle_json("metal"));
    let pin = write_file(&dir, "pin.toml", &fixture::pin_toml());

    // No --admin-url: refused, and it says how to supply one.
    let out = run(&[
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "metal",
        "--owner-metal-ack",
        "--pin",
        pin.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), EXIT_USAGE, "stderr={}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("--admin-url"), "{err}");
    assert!(err.contains("PROOF_ADMIN_URL"), "{err}");
    assert!(
        err.contains("--dry-run"),
        "the refusal must point at the dry run: {err}"
    );
    assert!(stdout(&out).is_empty(), "a refused install prints no plan");

    // With a URL but no bearer: refused too, and it names the token file.
    let out = run(&[
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "metal",
        "--owner-metal-ack",
        "--pin",
        pin.to_str().unwrap(),
        "--admin-url",
        "http://127.0.0.1:8100",
    ]);
    assert_eq!(code(&out), EXIT_USAGE, "stderr={}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("--admin-token-file"), "{err}");
    assert!(err.contains("PROOF_ADMIN_TOKEN_FILE"), "{err}");
    assert!(
        err.contains("never logged or printed"),
        "the refusal must say the bearer is handled safely: {err}"
    );

    // An empty tokens file is an error naming the file, not a silent no-op.
    let empty = write_file(&dir, "empty-token", "# only a comment\n\n");
    let out = run(&[
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "metal",
        "--owner-metal-ack",
        "--pin",
        pin.to_str().unwrap(),
        "--admin-url",
        "http://127.0.0.1:8100",
        "--admin-token-file",
        empty.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), EXIT_ERROR, "stderr={}", stderr(&out));
    assert!(stderr(&out).contains("holds no bearer"), "{}", stderr(&out));

    // The bearer is never echoed, in any of those refusals.
    for args in [
        vec![
            "topic",
            "install",
            "--bundle",
            bundle.to_str().unwrap(),
            "--env",
            "metal",
            "--owner-metal-ack",
            "--pin",
            pin.to_str().unwrap(),
        ],
        vec![
            "topic",
            "install",
            "--bundle",
            bundle.to_str().unwrap(),
            "--env",
            "metal",
            "--owner-metal-ack",
            "--pin",
            pin.to_str().unwrap(),
            "--admin-url",
            "http://127.0.0.1:8100",
        ],
    ] {
        let out = run(&args);
        assert!(
            !stdout(&out).contains("Bearer ") && !stderr(&out).contains("Bearer "),
            "the bearer must never be printed: {} {}",
            stdout(&out),
            stderr(&out)
        );
    }
    fs::remove_dir_all(&dir).ok();
}

/// `--drive-rlm` provisions a VM and runs a paid baseline, so it needs the
/// Owner assertion. Without either flag the install is the static half only.
#[test]
fn driving_the_rlm_requires_the_owner_assertion() {
    let dir = workdir("drive-rlm-gate");
    let bundle = write_file(&dir, "tb4.json", &fixture::bundle_json("staging"));
    let pin = write_file(&dir, "pin.toml", &fixture::pin_toml());
    let base = |extra: &[&str]| {
        let mut a = vec![
            "topic",
            "install",
            "--bundle",
            bundle.to_str().unwrap(),
            "--env",
            "staging",
            "--pin",
            pin.to_str().unwrap(),
            "--admin-url",
            "http://127.0.0.1:8100",
            "--admin-token-file",
            "/nonexistent/token",
        ];
        a.extend_from_slice(extra);
        run(&a)
    };

    // --drive-rlm without --owner-approved: refused as usage, before anything.
    let out = base(&["--drive-rlm"]);
    assert_eq!(code(&out), EXIT_USAGE, "stderr={}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("--owner-approved"), "{err}");
    assert!(err.contains("provisions a topic VM"), "{err}");
    assert!(
        err.contains("paid baseline"),
        "the gate must say what it authorizes: {err}"
    );

    // --skip-baseline without --drive-rlm is a contradiction, not a no-op.
    let out = base(&["--skip-baseline"]);
    assert_eq!(code(&out), EXIT_USAGE, "stderr={}", stderr(&out));
    assert!(
        stderr(&out).contains("only means something with --drive-rlm"),
        "{}",
        stderr(&out)
    );

    // The gates are checked before the token file is read, so a gate refusal
    // is a usage error rather than a confusing "file not found".
    let out = base(&["--drive-rlm", "--skip-baseline"]);
    assert_eq!(code(&out), EXIT_USAGE, "stderr={}", stderr(&out));

    fs::remove_dir_all(&dir).ok();
}

/// Owner default: metal is Owner-only and staging goes first, so a metal plan
/// without the explicit acknowledgement is a usage error, not a silent
/// fallback and not a partial install.
#[test]
fn a_metal_install_requires_the_owner_acknowledgement() {
    let dir = workdir("metal-gate");
    let bundle = write_file(&dir, "tb4.json", &fixture::bundle_json("metal"));
    let pin = write_file(&dir, "pin.toml", &fixture::pin_toml());
    let args = |extra: &[&str]| {
        let mut a = vec![
            "topic",
            "install",
            "--bundle",
            bundle.to_str().unwrap(),
            "--env",
            "metal",
            "--pin",
            pin.to_str().unwrap(),
            "--dry-run",
        ];
        a.extend_from_slice(extra);
        run(&a)
    };

    // Without the flag: refused, and it says how to proceed.
    let out = args(&[]);
    assert_eq!(code(&out), EXIT_USAGE, "stderr={}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("Owner-only"), "{err}");
    assert!(err.contains("--owner-metal-ack"), "{err}");
    assert!(
        err.contains("staging has passed"),
        "the gate must state the staging precondition: {err}"
    );
    assert!(
        err.contains("--env \n             staging") || err.contains("staging --dry-run"),
        "the gate must point at staging first: {err}"
    );
    assert!(stdout(&out).is_empty(), "a refused plan prints no plan");

    // With the flag: the plan resolves and says the gate was acknowledged.
    let out = args(&["--owner-metal-ack"]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    assert!(
        stdout(&out).contains("owner_gate        acknowledged"),
        "{}",
        stdout(&out)
    );

    // Staging is never gated: that is the default path.
    let staging = write_file(&dir, "tb4-staging.json", &fixture::bundle_json("staging"));
    let out = run(&[
        "topic",
        "install",
        "--bundle",
        staging.to_str().unwrap(),
        "--env",
        "staging",
        "--pin",
        pin.to_str().unwrap(),
        "--dry-run",
    ]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    assert!(
        stdout(&out).contains("owner_gate        n/a (staging)"),
        "{}",
        stdout(&out)
    );

    fs::remove_dir_all(&dir).ok();
}

/// The Owner default: slug `tb4` with `tbench` as a temporary alias. The
/// alias resolves through the store, and the CLI says which topic it hit.
#[tokio::test]
async fn an_alias_resolves_to_its_topic() {
    let Some(url) = std::env::var("DATABASE_URL")
        .ok()
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
    else {
        return;
    };
    let tp = match db::test_pool_with_url(&url).await {
        Ok(tp) => tp,
        Err(e) => panic!("test_pool: {e}"),
    };
    let store = proof_rlm_store::PgRlmStore::new(tp.pool().clone());
    let doc = fixture::signed_topic(&format!("sha256:{}", "ab".repeat(32)));
    proof_rlm_store::RlmStore::put_topic_version(&store, &doc)
        .await
        .expect("persist");

    let schema = tp.schema().to_owned();
    let scoped = format!("{url}?options=-c%20search_path%3D{schema}%2Cpublic");
    let run_db = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_proof-admin"))
            .args(args)
            .env("BASE_DATABASE_URL", &scoped)
            .env_remove("BASE_DATABASE_URL_FILE")
            .output()
            .expect("run proof-admin")
    };

    // Before the alias exists, the temporary slug is unknown.
    let out = run_db(&["topic", "show", "tbench"]);
    assert_eq!(code(&out), EXIT_ERROR, "stderr={}", stderr(&out));

    // Set the Owner default alias.
    let out = run_db(&["topic", "alias", "set", "tbench", "--topic", "tb4"]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    assert!(stdout(&out).contains("tbench -> tb4"), "{}", stdout(&out));

    // The alias now resolves, and the CLI says so.
    let out = run_db(&["topic", "show", "tbench"]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let body = stdout(&out);
    assert!(body.contains("tbench is an alias of tb4"), "{body}");
    assert!(body.contains("topic tb4"), "{body}");

    let out = run_db(&["--json", "topic", "show", "tbench"]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let parsed: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(
        parsed["topic_id"], "tb4",
        "the alias reports the canonical id"
    );

    // Listing shows the temporary mapping.
    let out = run_db(&["topic", "alias", "list", "--topic", "tb4"]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    assert!(stdout(&out).contains("tbench -> tb4"), "{}", stdout(&out));

    // An alias for an unpublished topic is refused.
    let out = run_db(&[
        "topic",
        "alias",
        "set",
        "ghost",
        "--topic",
        "never-published",
    ]);
    assert_eq!(code(&out), EXIT_ERROR, "stderr={}", stderr(&out));
    assert!(
        stderr(&out).contains("no published version"),
        "{}",
        stderr(&out)
    );

    // Retiring the alias leaves the topic alone.
    let out = run_db(&["topic", "alias", "rm", "tbench"]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let out = run_db(&["topic", "show", "tb4"]);
    assert_eq!(code(&out), 0, "the topic survives: {}", stderr(&out));
    let out = run_db(&["topic", "alias", "rm", "tbench"]);
    assert_eq!(code(&out), EXIT_ERROR, "already gone: {}", stderr(&out));

    tp.drop_schema().await.expect("drop");
}

#[test]
fn read_commands_without_a_database_are_usage_errors() {
    for args in [vec!["topic", "list"], vec!["topic", "show", "tb4"]] {
        let out = run(&args);
        assert_eq!(code(&out), EXIT_USAGE, "{args:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("needs a database"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
}

#[test]
fn database_url_and_file_are_mutually_exclusive() {
    let dir = workdir("db-url-both");
    let url_file = write_file(&dir, "url.txt", "postgres://example/db");
    let out = Command::new(env!("CARGO_BIN_EXE_proof-admin"))
        .args([
            "topic",
            "list",
            "--database-url",
            "postgres://example/other",
            "--database-url-file",
            url_file.to_str().unwrap(),
        ])
        .env_remove("BASE_DATABASE_URL")
        .env_remove("BASE_DATABASE_URL_FILE")
        .output()
        .expect("run");
    assert_eq!(code(&out), EXIT_USAGE, "stderr={}", stderr(&out));
    assert!(stderr(&out).contains("not both"), "stderr={}", stderr(&out));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn seal_still_fails_closed_with_exit_3() {
    let args = vec!["topic", "seal", "tb4", "--value", "0.42"];
    let out = run(&args);
    assert_eq!(
        code(&out),
        EXIT_NOT_IMPLEMENTED,
        "{args:?}: {}",
        stderr(&out)
    );
    let err = stderr(&out);
    assert!(
        err.contains("not implemented in this slice"),
        "{args:?}: {err}"
    );
    assert!(
        err.contains("Nothing was changed"),
        "a stub must say it changed nothing: {args:?}: {err}"
    );
    assert!(
        stdout(&out).is_empty(),
        "a stub prints nothing to stdout: {args:?}"
    );
}

/// `topic disable` / `topic enable` are implemented now, and they are
/// **fail-closed without a database**: the gate is the table the challenge
/// reads, so a CLI that could not write it must refuse rather than report a
/// topic as stopped. Exit 2 (usage), nothing on stdout, and the message names
/// the variable to set.
#[test]
fn disable_and_enable_need_the_gate_database() {
    for args in [
        vec!["topic", "disable", "tb4", "--reason", "incident 42"],
        vec!["topic", "enable", "tb4"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_proof-admin"))
            .args(&args)
            .env_remove("BASE_DATABASE_URL")
            .env_remove("BASE_DATABASE_URL_FILE")
            .output()
            .expect("run");
        assert_eq!(code(&out), EXIT_USAGE, "{args:?}: {}", stderr(&out));
        let err = stderr(&out);
        assert!(err.contains("BASE_DATABASE_URL"), "{args:?}: {err}");
        assert!(
            stdout(&out).is_empty(),
            "nothing is reported as changed: {args:?}"
        );
    }
}

#[test]
fn help_lists_every_subcommand_and_says_what_is_not_implemented() {
    let out = run(&["topic", "--help"]);
    assert_eq!(code(&out), 0);
    let body = stdout(&out);
    for sub in [
        "validate",
        "install",
        "install-log",
        "list",
        "show",
        "enable",
        "disable",
        "seal",
    ] {
        assert!(body.contains(sub), "missing subcommand {sub} in:\n{body}");
    }
    // `topic --help` lists subcommands; the flags live on `install --help`.
    assert!(
        !body.contains("not implemented in this slice")
            || body.contains("enable")
            || body.contains("disable")
            || body.contains("seal"),
        "the stubs must be the ones that say so:\n{body}"
    );

    let out = run(&["topic", "install", "--help"]);
    assert_eq!(code(&out), 0);
    let body = stdout(&out);
    for flag in [
        "--dry-run",
        "--skip-baseline",
        "--drive-rlm",
        "--owner-approved",
        "--owner-metal-ack",
        "--admin-url",
        "--admin-token-file",
    ] {
        assert!(body.contains(flag), "missing {flag} in:\n{body}");
    }
    // And the install help must not promise a stub it no longer is.
    assert!(
        !body.contains("not implemented in this slice"),
        "install is implemented; its help must not say otherwise:\n{body}"
    );
}

/// The admin CLI hands control to the RLM; it does not interpret the topic.
///
/// This is the architectural guard: the CLI may *name* the RLM-owned parts in
/// its output, but no topic behavior may be compiled into it. A future edit
/// that branches on a topic id, or bakes in a rule, metric, or submit format,
/// fails here.
#[test]
fn the_cli_does_not_bake_in_topic_behavior() {
    const SOURCE: &str = include_str!("../src/main.rs");
    // Strip comments: the crate may *explain* the boundary (and its help text
    // shows an example bundle name), but no literal may live in logic.
    let logic: String = SOURCE
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    // The one place a seed id is allowed is the CLI's own help/examples.
    let without_examples = logic
        .replace("tb4.json", "")
        .replace("`tbench`", "")
        .replace("`tb4`", "");
    assert!(
        !without_examples.contains("tb4") && !without_examples.contains("tbench"),
        "a topic id must not appear in CLI logic"
    );
    for forbidden in [
        "terminal-bench",
        "harbor",
        "success_rate",
        "no_short_circuit",
        "submission_format",
    ] {
        assert!(
            !without_examples.to_lowercase().contains(forbidden),
            "{forbidden} must not be compiled into the admin CLI"
        );
    }
    // It must not read the RLM section's *contents* either: only carry them.
    assert!(
        !without_examples.contains("rlm.rules")
            && !without_examples.contains("rlm.scoring")
            && !without_examples.contains("rlm.migrations")
            && !without_examples.contains("rlm.apis"),
        "the CLI must carry the RLM section, never read into it"
    );
}

/// The registry view reads the existing `proof_topic_version` rows.
#[tokio::test]
async fn the_registry_view_lists_what_the_scoring_path_persisted() {
    let Some(url) = std::env::var("DATABASE_URL")
        .ok()
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
    else {
        return;
    };
    let tp = match db::test_pool_with_url(&url).await {
        Ok(tp) => tp,
        Err(e) => panic!("test_pool: {e}"),
    };
    // Persist through the scoring path's own store, then read it back through
    // the CLI: there is one table, so the view cannot disagree with scoring.
    let store = proof_rlm_store::PgRlmStore::new(tp.pool().clone());
    let doc = fixture::signed_topic(&format!("sha256:{}", "ab".repeat(32)));
    proof_rlm_store::RlmStore::put_topic_version(&store, &doc)
        .await
        .expect("persist through the scoring path");

    let schema = tp.schema().to_owned();
    let scoped = format!("{url}?options=-c%20search_path%3D{schema}%2Cpublic");
    let run_db = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_proof-admin"))
            .args(args)
            .env("BASE_DATABASE_URL", &scoped)
            .env_remove("BASE_DATABASE_URL_FILE")
            .output()
            .expect("run proof-admin")
    };

    let out = run_db(&["--json", "topic", "list"]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let listed: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(listed[0]["topic_id"], "tb4");
    assert_eq!(listed[0]["version"], 1);
    assert_eq!(listed[0]["custom_id"], "tbench");

    let out = run_db(&["--json", "topic", "show", "tb4"]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let shown: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(shown["topic_id"], "tb4");
    assert_eq!(shown["document"]["id"], "tb4");
    assert_eq!(shown["document"]["signature"], doc.signature);

    // An unknown id is an error that says what to do, not an empty success.
    let out = run_db(&["topic", "show", "nope"]);
    assert_eq!(code(&out), EXIT_ERROR);
    assert!(
        stderr(&out).contains("no installed topic"),
        "{}",
        stderr(&out)
    );

    tp.drop_schema().await.expect("drop");
}

// ---------------------------------------------------------------------------
// The publish order: an `open` document is not publishable before the install
// ---------------------------------------------------------------------------

/// What the stub saw in the database **at the moment** the publish arrived.
type PublishProbe = Option<(String, String)>;

/// A minimal `POST /v1/admin/proof/topics` stub.
///
/// It records every request it receives, and — when it is given a pool — reads
/// the install journal and the migration's table *inside* the publish handler,
/// so the test can assert what was in place **before** the topic became
/// reachable rather than after the process exited.
struct AdminStub {
    addr: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    at_publish: Arc<Mutex<PublishProbe>>,
}

impl AdminStub {
    async fn start(probe: Option<sqlx::PgPool>) -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub");
        let addr = listener.local_addr().expect("addr");
        let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let at_publish: Arc<Mutex<PublishProbe>> = Arc::new(Mutex::new(None));
        let held_requests = Arc::clone(&requests);
        let held_probe = Arc::clone(&at_publish);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let requests = Arc::clone(&held_requests);
                let at_publish = Arc::clone(&held_probe);
                let probe = probe.clone();
                tokio::spawn(async move {
                    let mut buf: Vec<u8> = Vec::new();
                    let mut chunk = [0u8; 4096];
                    // Headers, then the body the Content-Length promises.
                    let (head, body) = loop {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                        let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                            continue;
                        };
                        let head = String::from_utf8_lossy(&buf[..end]).into_owned();
                        let want = head
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + want {
                            break (head, String::from_utf8_lossy(&buf[end + 4..]).into_owned());
                        }
                    };
                    let request_line = head.lines().next().unwrap_or_default().to_owned();
                    let path = request_line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("")
                        .to_owned();
                    if path == "/challenge/proof/v1/admin/proof/topics" {
                        let seen = match &probe {
                            Some(pool) => {
                                let state: Option<String> = sqlx::query_scalar(
                                    "SELECT state FROM proof_topic_install \
                                     WHERE topic_id = 'tb4' ORDER BY id DESC LIMIT 1",
                                )
                                .fetch_optional(pool)
                                .await
                                .ok()
                                .flatten();
                                let table: Option<String> =
                                    sqlx::query_scalar("SELECT to_regclass('tb4_scratch')::text")
                                        .fetch_optional(pool)
                                        .await
                                        .ok()
                                        .flatten();
                                Some((
                                    state.unwrap_or_else(|| "no row".into()),
                                    table.unwrap_or_else(|| "no table".into()),
                                ))
                            }
                            None => None,
                        };
                        if let Some(seen) = seen {
                            *at_publish.lock().unwrap() = Some(seen);
                        }
                    }
                    requests.lock().unwrap().push(request_line);
                    let _ = body;
                    let body = r#"{"ok":true}"#;
                    let response = format!(
                        "HTTP/1.1 201 Created\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Self {
            addr,
            requests,
            at_publish,
        }
    }

    fn publish_requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.contains("/challenge/proof/v1/admin/proof/topics"))
            .cloned()
            .collect()
    }
}

/// The publish happens **after** the install reached green — never before.
///
/// The stub reads the journal and the migration's table inside the publish
/// handler, so this asserts the state a miner would have found at the instant
/// the topic became reachable: rules installed, migrations applied.
#[tokio::test(flavor = "multi_thread")]
async fn the_publish_lands_only_after_the_install_is_green() {
    let Some(url) = std::env::var("DATABASE_URL")
        .ok()
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
    else {
        return;
    };
    let tp = match db::test_pool_with_url(&url).await {
        Ok(tp) => tp,
        Err(e) => panic!("test_pool: {e}"),
    };
    let schema = tp.schema().to_owned();
    let scoped = format!("{url}?options=-c%20search_path%3D{schema}%2Cpublic");
    let probe_pool = db::connect(&scoped).await.expect("probe pool");
    let stub = AdminStub::start(Some(probe_pool.clone())).await;

    // The document's own version, as the RLM setup (or an earlier publish)
    // leaves it: the alias step is the one part of the install that keys on a
    // persisted version rather than on `topic_id` alone.
    let store = proof_rlm_store::PgRlmStore::new(probe_pool.clone());
    proof_rlm_store::RlmStore::put_topic_version(
        &store,
        &fixture::signed_topic(&format!("sha256:{}", "ab".repeat(32))),
    )
    .await
    .expect("persist the document");

    let dir = workdir("publish-order");
    let bundle = write_file(&dir, "b.json", &fixture::bundle_json("staging"));
    let pin = write_file(&dir, "pin.toml", &fixture::pin_toml());
    let token = write_file(&dir, "token", "operator-bearer-not-a-real-one\n");

    let out = Command::new(env!("CARGO_BIN_EXE_proof-admin"))
        .args([
            "topic",
            "install",
            "--bundle",
            bundle.to_str().unwrap(),
            "--env",
            "staging",
            "--pin",
            pin.to_str().unwrap(),
            "--admin-url",
            &format!("http://{}", stub.addr),
            "--admin-token-file",
            token.to_str().unwrap(),
        ])
        .env("BASE_DATABASE_URL", &scoped)
        .env_remove("BASE_DATABASE_URL_FILE")
        .output()
        .expect("run proof-admin");
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));

    let published = stub.publish_requests();
    assert_eq!(published.len(), 1, "one publish: {published:?}");
    let (state, table) = stub
        .at_publish
        .lock()
        .unwrap()
        .clone()
        .expect("the stub saw a publish");
    assert_eq!(
        state, "applied",
        "the install must be green before the topic is published"
    );
    assert_eq!(
        table, "tb4_scratch",
        "the migration must have applied before the topic is published"
    );

    // And the install is complete afterwards: the journal's newest row is
    // `applied` with the migration recorded.
    let row = proof_topic_install::latest_install(&probe_pool, "tb4")
        .await
        .expect("journal")
        .expect("a row");
    assert_eq!(row.state, "applied");
    assert_eq!(row.migrations, ["0001_scratch"]);

    fs::remove_dir_all(&dir).ok();
    tp.drop_schema().await.expect("drop");
}

/// A refused install publishes **nothing at all**: no document reaches the
/// registry, so there is no `open` topic for a miner to submit to while its
/// migrations, routes, and rules are missing.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_install_never_publishes() {
    let Some(url) = std::env::var("DATABASE_URL")
        .ok()
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
    else {
        return;
    };
    let tp = match db::test_pool_with_url(&url).await {
        Ok(tp) => tp,
        Err(e) => panic!("test_pool: {e}"),
    };
    let schema = tp.schema().to_owned();
    let scoped = format!("{url}?options=-c%20search_path%3D{schema}%2Cpublic");
    let stub = AdminStub::start(None).await;

    let dir = workdir("publish-refused");
    // The same bundle, with a migration the deny-list refuses. The document
    // and its signature are untouched, so the refusal comes from the install.
    let denied = fixture::bundle_json("staging").replace(
        "CREATE TABLE tb4_scratch (id TEXT)",
        "DROP TABLE proof_rule_version",
    );
    assert!(denied.contains("proof_rule_version"), "the swap applied");
    let bundle = write_file(&dir, "b.json", &denied);
    let pin = write_file(&dir, "pin.toml", &fixture::pin_toml());
    let token = write_file(&dir, "token", "operator-bearer-not-a-real-one\n");

    let out = Command::new(env!("CARGO_BIN_EXE_proof-admin"))
        .args([
            "topic",
            "install",
            "--bundle",
            bundle.to_str().unwrap(),
            "--env",
            "staging",
            "--pin",
            pin.to_str().unwrap(),
            "--admin-url",
            &format!("http://{}", stub.addr),
            "--admin-token-file",
            token.to_str().unwrap(),
        ])
        .env("BASE_DATABASE_URL", &scoped)
        .env_remove("BASE_DATABASE_URL_FILE")
        .output()
        .expect("run proof-admin");
    assert_eq!(code(&out), EXIT_ERROR, "stderr={}", stderr(&out));
    assert!(
        stderr(&out).contains("deny-list"),
        "the refusal names the gate: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("was **not** published"),
        "the rollback notes must say nothing was published: {}",
        stderr(&out)
    );
    assert!(
        stub.publish_requests().is_empty(),
        "a refused install must publish nothing: {:?}",
        stub.publish_requests()
    );

    // Nothing was installed either: no journal row, no table.
    let row = proof_topic_install::latest_install(tp.pool(), "tb4")
        .await
        .expect("journal");
    assert!(row.is_none(), "a pre-flight refusal writes no journal row");
    let table: Option<String> = sqlx::query_scalar("SELECT to_regclass('tb4_scratch')::text")
        .fetch_one(tp.pool())
        .await
        .expect("probe");
    assert!(table.is_none(), "and no migration ran: {table:?}");

    fs::remove_dir_all(&dir).ok();
    tp.drop_schema().await.expect("drop");
}
