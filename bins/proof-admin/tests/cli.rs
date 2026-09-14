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

    /// The Arch default bundle: slug `tb4`, custom id `tbench`.
    pub fn bundle_json(environment: &str) -> String {
        let hex = "ab".repeat(32);
        let pack = format!("sha256:{hex}");
        let topic = signed_topic(&pack);
        let bundle = serde_json::json!({
            "schema_version": 1,
            "environment": environment,
            "display_name": "Terminal-Bench 4",
            "topic": topic,
            "host": {
                "rlm_image_digest": format!("sha256:{hex}"),
                "experiment_image_digest": format!("sha256:{hex}"),
                "pack_digest": pack,
                "pack_dir": "/var/lib/proof/packs",
                "custom_ids_entry": "tbench"
            }
        });
        serde_json::to_string_pretty(&bundle).expect("json")
    }
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

/// A real install is out of scope for this slice: it must refuse loudly rather
/// than write anything, and it must not need a database to say so.
#[test]
fn a_real_install_is_not_implemented_and_changes_nothing() {
    let dir = workdir("no-real-install");
    let bundle = write_file(&dir, "tb4.json", &fixture::bundle_json("metal"));
    let pin = write_file(&dir, "pin.toml", &fixture::pin_toml());
    let out = run(&[
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "metal",
        "--pin",
        pin.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), EXIT_NOT_IMPLEMENTED, "stderr={}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("not implemented in this slice"), "{err}");
    assert!(err.contains("Nothing was changed"), "{err}");
    assert!(stdout(&out).is_empty(), "a stub prints nothing to stdout");
    fs::remove_dir_all(&dir).ok();
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
fn enable_disable_and_seal_fail_closed_with_exit_3() {
    for args in [
        vec!["topic", "enable", "tb4"],
        vec!["topic", "disable", "tb4"],
        vec!["topic", "seal", "tb4", "--value", "0.42"],
    ] {
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
}

#[test]
fn help_lists_every_p0_subcommand_and_says_what_is_not_implemented() {
    let out = run(&["topic", "--help"]);
    assert_eq!(code(&out), 0);
    let body = stdout(&out);
    for sub in [
        "validate", "install", "list", "show", "enable", "disable", "seal",
    ] {
        assert!(body.contains(sub), "missing subcommand {sub} in:\n{body}");
    }
    assert!(
        body.contains("--dry-run"),
        "the dry-run flag must be discoverable:\n{body}"
    );
    assert!(
        body.to_lowercase()
            .contains("not implemented in this slice"),
        "the stubs must say so in help:\n{body}"
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
