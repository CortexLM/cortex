//! Process-level tests for `proof-admin` (dynamic-topics P0).
//!
//! Everything here runs without a database, a network, or a metal key:
//! `topic validate` and `topic install --dry-run` are the two commands P0
//! promises to make work end to end, and the stubs must fail closed with
//! exit code 3 rather than doing something partial.
//!
//! The real install path is covered against Postgres in
//! `crates/db/tests/topics.rs` (schema and upsert) and by the same
//! `upsert_topic` call this binary makes; these tests assert that the binary
//! never *reaches* a database unless it was asked to and given one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Exit code for a failure (bad bundle, missing database, ...).
const EXIT_ERROR: i32 = 1;
/// Exit code for bad usage or missing configuration.
const EXIT_USAGE: i32 = 2;
/// Exit code for a command a later slice owns.
const EXIT_NOT_IMPLEMENTED: i32 = 3;

const HEX: &str = "abababababababababababababababababababababababababababababababab";

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

fn write_bundle(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).expect("write bundle");
    path
}

/// The Arch default bundle: slug `tb4`, alias `tbench`.
fn tb4_json(environment: &str) -> String {
    format!(
        r#"{{
  "schema_version": 1,
  "topic_id": "tb4",
  "display_name": "Terminal-Bench 4",
  "version": 1,
  "environment": "{environment}",
  "aliases": ["tbench"],
  "runner_id": "rlm_fc_in_guest_harbor",
  "pin_rlm": "sha256:{HEX}",
  "pin_experiment": "sha256:{HEX}",
  "pack_digest": "sha256:{HEX}",
  "n_concurrent": 2,
  "config": {{"task_slice": "tb4-first-15"}}
}}"#
    )
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

/// Run the binary against a real Postgres URL.
fn run_with_db(args: &[&str], database_url: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_proof-admin"))
        .args(args)
        .env("BASE_DATABASE_URL", database_url)
        .env_remove("BASE_DATABASE_URL_FILE")
        .output()
        .expect("run proof-admin")
}

/// Returns `None` when `DATABASE_URL` is unset so default CI (no Postgres)
/// skips, matching the gating in `crates/db/tests`.
fn owner_url() -> Option<String> {
    std::env::var("DATABASE_URL")
        .ok()
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
}

/// The reported install state must be the **persisted** state.
///
/// A re-install deliberately leaves `enabled` alone, so a topic that was
/// already live stays live. An operator (or an automation reading `--json`)
/// told it is disabled would be exactly the mistake that produces a surprise
/// on a live host.
#[tokio::test]
async fn a_reinstall_reports_the_persisted_state_not_a_guess() {
    let Some(url) = owner_url() else {
        return;
    };
    let tp = match db::test_pool_with_url(&url).await {
        Ok(tp) => tp,
        Err(e) => panic!("test_pool: {e}"),
    };
    // The binary talks to this schema through `search_path`, so hand it a URL
    // whose connections land in the isolated test schema.
    let schema = tp.schema().to_owned();
    let scoped = format!("{url}?options=-c%20search_path%3D{schema}%2Cpublic");

    let dir = workdir("reinstall");
    let bundle = write_bundle(&dir, "tb4.json", &tb4_json("metal"));
    let install = |json: bool| {
        let mut args = vec![
            "topic",
            "install",
            "--bundle",
            bundle.to_str().unwrap(),
            "--env",
            "metal",
        ];
        if json {
            args.insert(0, "--json");
        }
        run_with_db(&args, &scoped)
    };

    // First install: the row does not exist, so it is reported disabled.
    let out = install(true);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let first: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(first["enabled"], false, "{first}");

    // An operator enables it (the enable path is a later slice, so the test
    // writes the column directly).
    sqlx::query("UPDATE proof_topic SET enabled = TRUE WHERE topic_id = 'tb4'")
        .execute(tp.pool())
        .await
        .expect("enable");

    // Re-install: the row stays enabled, and the output must say so.
    let out = install(true);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let second: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(
        second["enabled"], true,
        "a re-install must report the persisted state: {second}"
    );

    // The human output agrees with the JSON output.
    let out = install(false);
    let text = stdout(&out);
    assert!(
        text.contains("still ENABLED") && !text.contains("(DISABLED)"),
        "human output must not claim a live topic is disabled:\n{text}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    tp.drop_schema().await.expect("drop");
}

#[test]
fn validate_accepts_the_arch_default_bundle_and_writes_nothing() {
    let dir = workdir("validate-ok");
    let bundle = write_bundle(&dir, "tb4.json", &tb4_json("metal"));
    let out = run(&["topic", "validate", "--bundle", bundle.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let body = stdout(&out);
    for needle in [
        "is valid",
        "topic_id       tb4",
        "environment    metal",
        "aliases        tbench",
        "bundle_digest  sha256:",
        "Nothing was written",
    ] {
        assert!(body.contains(needle), "missing {needle:?} in:\n{body}");
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn validate_reports_the_offending_key_and_writes_nothing() {
    let dir = workdir("validate-bad");
    // An unknown key is refused rather than ignored: a binding this build
    // cannot name is a binding nothing enforces.
    let unknown = tb4_json("metal").replace("\"version\": 1,", "\"task_slice\": \"x\",");
    let bundle = write_bundle(&dir, "unknown.json", &unknown);
    let out = run(&["topic", "validate", "--bundle", bundle.to_str().unwrap()]);
    assert_eq!(code(&out), EXIT_ERROR, "stderr={}", stderr(&out));
    assert!(
        stderr(&out).contains("task_slice"),
        "stderr must name the unknown key: {}",
        stderr(&out)
    );
    assert!(
        stdout(&out).is_empty(),
        "a failure prints nothing to stdout"
    );

    // A runner with no pack cannot be installed, so it does not validate.
    let no_pack = tb4_json("metal").replace(&format!(",\n  \"pack_digest\": \"sha256:{HEX}\""), "");
    let bundle = write_bundle(&dir, "no-pack.json", &no_pack);
    let out = run(&["topic", "validate", "--bundle", bundle.to_str().unwrap()]);
    assert_eq!(code(&out), EXIT_ERROR);
    assert!(
        stderr(&out).contains("pack_digest is required"),
        "stderr={}",
        stderr(&out)
    );

    // An invented digest is refused by name.
    let bad_digest = tb4_json("metal").replace(&format!("sha256:{HEX}"), "sha256:abc");
    let bundle = write_bundle(&dir, "bad-digest.json", &bad_digest);
    let out = run(&["topic", "validate", "--bundle", bundle.to_str().unwrap()]);
    assert_eq!(code(&out), EXIT_ERROR);
    assert!(
        stderr(&out).contains("64 lowercase hex"),
        "stderr={}",
        stderr(&out)
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn validate_json_output_is_machine_readable() {
    let dir = workdir("validate-json");
    let bundle = write_bundle(&dir, "tb4.json", &tb4_json("metal"));
    let out = run(&[
        "--json",
        "topic",
        "validate",
        "--bundle",
        bundle.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("validate --json is JSON");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["topic_id"], "tb4");
    assert_eq!(parsed["environment"], "metal");
    assert_eq!(parsed["aliases"][0], "tbench");
    assert!(
        parsed["bundle_digest"]
            .as_str()
            .unwrap_or_default()
            .starts_with("sha256:"),
        "{parsed}"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn dry_run_install_resolves_a_disabled_plan_without_a_database() {
    let dir = workdir("dry-run");
    let bundle = write_bundle(&dir, "tb4.json", &tb4_json("metal"));
    let out = run(&[
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "metal",
        "--dry-run",
    ]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let body = stdout(&out);
    for needle in [
        "topic install plan",
        "topic_id          tb4",
        "environment       metal",
        "aliases           tbench",
        "runner_id         rlm_fc_in_guest_harbor",
        "n_concurrent      2",
        "enabled           false",
        "nothing was written and no database was touched",
    ] {
        assert!(body.contains(needle), "missing {needle:?} in:\n{body}");
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn dry_run_install_json_matches_the_plan_shape() {
    let dir = workdir("dry-run-json");
    let bundle = write_bundle(&dir, "tb4.json", &tb4_json("staging"));
    let out = run(&[
        "--json",
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "staging",
        "--dry-run",
    ]);
    assert_eq!(code(&out), 0, "stderr={}", stderr(&out));
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("dry run --json is JSON");
    assert_eq!(parsed["topic_id"], "tb4");
    assert_eq!(parsed["environment"], "staging");
    assert_eq!(parsed["enabled"], false, "a plan never enables");
    assert_eq!(parsed["n_concurrent"], 2);
    assert_eq!(parsed["schema_version"], 1);
    assert!(parsed["bundle_digest"].as_str().is_some(), "{parsed}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn install_refuses_an_environment_the_bundle_does_not_declare() {
    let dir = workdir("env-mismatch");
    let bundle = write_bundle(&dir, "tb4.json", &tb4_json("metal"));
    let out = run(&[
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "staging",
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

#[test]
fn a_real_install_without_a_database_is_a_usage_error_not_a_write() {
    let dir = workdir("no-db");
    let bundle = write_bundle(&dir, "tb4.json", &tb4_json("metal"));
    let out = run(&[
        "topic",
        "install",
        "--bundle",
        bundle.to_str().unwrap(),
        "--env",
        "metal",
    ]);
    assert_eq!(code(&out), EXIT_USAGE, "stderr={}", stderr(&out));
    assert!(
        stderr(&out).contains("needs a database"),
        "stderr={}",
        stderr(&out)
    );
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
    let bundle = write_bundle(&dir, "tb4.json", &tb4_json("metal"));
    let url_file = dir.join("url.txt");
    fs::write(&url_file, "postgres://example/db").expect("write url file");
    let out = Command::new(env!("CARGO_BIN_EXE_proof-admin"))
        .args([
            "topic",
            "install",
            "--bundle",
            bundle.to_str().unwrap(),
            "--env",
            "metal",
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
fn validate_refuses_a_noncanonical_digest_before_an_install_can_fail() {
    let dir = workdir("digest-strict");
    // The row's CHECK is `^sha256:[0-9a-f]{64}$`. An uppercase or padded pin
    // must be a validate-time reject, not a surprise on the host that matters.
    for (label, replacement) in [
        (
            "uppercase hex",
            format!("sha256:{}", HEX.to_ascii_uppercase()),
        ),
        ("padded", format!("sha256: {HEX}")),
    ] {
        let body = tb4_json("metal").replace(&format!("sha256:{HEX}"), &replacement);
        let bundle = write_bundle(&dir, &format!("{}.json", label.replace(' ', "-")), &body);
        let out = run(&["topic", "validate", "--bundle", bundle.to_str().unwrap()]);
        assert_eq!(code(&out), EXIT_ERROR, "{label}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("64 lowercase hex"),
            "{label}: stderr={}",
            stderr(&out)
        );
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn validate_refuses_a_numeric_column_overflow_instead_of_clamping() {
    let dir = workdir("overflow");
    // `version` and `n_concurrent` land in INTEGER columns; a value that does
    // not fit is a reject, never a silent rewrite of what was validated.
    for (label, from, to) in [
        ("version", "\"version\": 1,", "\"version\": 4294967295,"),
        (
            "concurrency",
            "\"n_concurrent\": 2",
            "\"n_concurrent\": 4294967295",
        ),
    ] {
        let body = tb4_json("metal").replace(from, to);
        let bundle = write_bundle(&dir, &format!("{label}.json"), &body);
        let out = run(&["topic", "validate", "--bundle", bundle.to_str().unwrap()]);
        assert_eq!(code(&out), EXIT_ERROR, "{label}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("refused rather than clamped"),
            "{label}: stderr={}",
            stderr(&out)
        );
    }
    fs::remove_dir_all(&dir).ok();
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
