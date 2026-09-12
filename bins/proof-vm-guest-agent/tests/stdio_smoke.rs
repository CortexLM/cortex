//! The single-task smoke driver (`deploy/scripts/proof-experiment-smoke.py`)
//! against the **real** guest agent binary over its `--stdio` frames.
//!
//! This is the CI half of the metal test path: the same driver an operator
//! points at the KVM host runs here with a fake adaptor (the guest contract)
//! and with the in-repo Harbor reference adaptor over a fake `harbor` /
//! `docker` on PATH (the adaptor contract). Nothing here needs Firecracker,
//! Docker, Harbor, or a key — and the fake key that stands in for miner BYOK
//! must never appear in what the driver prints.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const FAKE_KEY: &str = "sk-or-test-key-never-printed-0123456789";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "proof-guest-smoke-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn exe(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// An uncompressed tar of `members` (`path → bytes`), like `tar -cf`.
fn tar_of(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, bytes) in members {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, *bytes).unwrap();
    }
    builder.into_inner().unwrap()
}

/// A minimal custom-family topic document whose params select `runner`.
fn topic_json(runner: &str, extra: &[(&str, &str)]) -> String {
    let mut params: BTreeMap<&str, &str> = BTreeMap::from([
        ("baseline_runner", runner),
        // Overwritten by the driver with the real tar digest (--pack-tar).
        (
            "experiment_pack_digest",
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        ),
        ("tasks_dir", "tasks"),
        ("miner_byok", "OPENROUTER_API_KEY"),
        ("model", "openrouter/vendor/model"),
        ("n_concurrent", "2"),
    ]);
    params.extend(extra.iter().copied());
    serde_json::json!({
        "schema_version": 1,
        "id": "smoke-topic",
        "statement": "smoke",
        "status": "open",
        "constraints": {
            "firecracker_required": true,
            "model_pin": "vendor/model",
            "task_slice": "any-label",
            "params": params,
        },
        "metric": {"family": "custom", "primary": "success_rate", "direction": "max",
                   "epsilon_rel": 0.05, "custom_id": "smoke_topic"},
        "baseline": {"seed": 7},
        "eval_executor": {"max_proof_deadline_s": 600},
        "flops_budget": 1,
    })
    .to_string()
}

struct Fixture {
    root: PathBuf,
    topic: PathBuf,
    pack: PathBuf,
    recipe: PathBuf,
}

fn fixture(tag: &str, runner: &str, extra: &[(&str, &str)]) -> Fixture {
    let root = tmp(tag);
    let topic = root.join("topic.json");
    fs::write(&topic, topic_json(runner, extra)).unwrap();
    let pack = root.join("pack.tar");
    fs::write(
        &pack,
        tar_of(&[
            ("tasks/task-a/task.toml", b"[task]\nname = \"task-a\"\n"),
            ("tasks/task-b/task.toml", b"[task]\nname = \"task-b\"\n"),
        ]),
    )
    .unwrap();
    let recipe = root.join("recipe.tar");
    fs::write(
        &recipe,
        tar_of(&[
            (
                "recipe/harness.json",
                br#"{"kind":"python","import_path":"agent.agent:Agent"}"#,
            ),
            (
                "recipe/agent/agent.py",
                b"class Agent:\n    async def run(self, instruction, environment=None, context=None):\n        return 'ok'\n",
            ),
        ]),
    )
    .unwrap();
    Fixture {
        root,
        topic,
        pack,
        recipe,
    }
}

fn run_driver(fx: &Fixture, args: &[&str], key: Option<&str>) -> (bool, String, String) {
    let mut cmd = Command::new("python3");
    cmd.arg(repo().join("deploy/scripts/proof-experiment-smoke.py"))
        .args([
            "--driver",
            "agent",
            "--guest-agent",
            env!("CARGO_BIN_EXE_proof-vm-guest-agent"),
            "--topic-json",
        ])
        .arg(&fx.topic)
        .arg("--pack-tar")
        .arg(&fx.pack)
        .args(args)
        .env_remove("OPENROUTER_API_KEY")
        .env("RUST_LOG", "warn");
    if let Some(k) = key {
        cmd.env("OPENROUTER_API_KEY", k);
    }
    let out = cmd.output().expect("run the smoke driver");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The guest contract end to end: hello → stage_pack → stage_artifact → run
/// over real frames, one task selected by the topic shape, the miner key
/// exported to the adaptor and never printed.
#[test]
fn one_task_evaluate_through_the_real_guest_agent() {
    let fx = fixture("contract", "smoke_runner", &[]);
    let adaptor = fx.root.join("adaptor");
    fs::create_dir_all(&adaptor).unwrap();
    exe(
        &adaptor.join("run"),
        r#"#!/bin/sh
set -eu
[ -d "$PROOF_PACK_DIR/$PROOF_PARAM_TASKS_DIR/task-a" ] || { echo "pack not staged" >&2; exit 2; }
[ -f "$PROOF_ARTIFACT_DIR/recipe/agent/agent.py" ] || { echo "artefact not staged" >&2; exit 2; }
[ -r "$PROOF_MINER_ENV_DIR/OPENROUTER_API_KEY" ] || { echo "byok file missing" >&2; exit 2; }
[ "$OPENROUTER_API_KEY" = "$(cat "$PROOF_MINER_ENV_DIR/OPENROUTER_API_KEY")" ] || { echo "byok mismatch" >&2; exit 2; }
echo "adaptor saw key $OPENROUTER_API_KEY" >&2
printf '{"primary_value": 0.5, "claim_holds": true, "evidence": {"tasks": "%s", "policy": "%s", "exec_timeout": "%s", "slice": "%s", "n_scored": 1}}\n' \
  "$PROOF_PARAM_TASKS" "${PROOF_PARAM_AGENT_EXCEPTION_POLICY:-unset}" "${PROOF_PARAM_EXEC_TIMEOUT_S:-unset}" "$PROOF_TASK_SLICE" > "$PROOF_OUTPUT_DIR/report.json"
"#,
    );
    let out = fx.root.join("outcome.json");
    let (ok, stdout, stderr) = run_driver(
        &fx,
        &[
            "--runner-dir",
            adaptor.to_str().unwrap(),
            "--job",
            "evaluate",
            "--tasks",
            "task-a",
            "--set",
            "agent_exception_policy=zero",
            "--set",
            "exec_timeout_s=900",
            "--artifact-tar",
            fx.recipe.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        Some(FAKE_KEY),
    );
    assert!(ok, "driver failed\nstdout:\n{stdout}\nstderr:\n{stderr}");
    for frame in [
        "hello → ready",
        "stage_pack → pack_staged",
        "stage_artifact → artifact_staged",
        "run → done",
    ] {
        assert!(stderr.contains(frame), "missing {frame:?} in\n{stderr}");
    }
    let outcome: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
    let report = &outcome["report"];
    assert_eq!(report["primary_value"], serde_json::json!(0.5));
    assert_eq!(
        report["sandboxed"],
        serde_json::json!(true),
        "the guest states where it ran"
    );
    assert_eq!(report["evidence"]["tasks"], serde_json::json!("task-a"));
    assert_eq!(report["evidence"]["policy"], serde_json::json!("zero"));
    assert_eq!(report["evidence"]["exec_timeout"], serde_json::json!("900"));
    assert_eq!(report["evidence"]["slice"], serde_json::json!("any-label"));
    assert_eq!(
        report["evidence"]["runner"],
        serde_json::json!("smoke_runner")
    );
    assert_eq!(report["topic_id"], serde_json::json!("smoke-topic"));
    let printed = format!("{stdout}{stderr}{}", fs::read_to_string(&out).unwrap());
    assert!(
        !printed.contains(FAKE_KEY),
        "the miner key must never appear in driver output or the guest's redacted tail"
    );
    assert!(
        printed.contains("adaptor saw key [REDACTED]") || !printed.contains("adaptor saw key"),
        "the guest redacts the key from the adaptor tail: {stderr}"
    );
    let _ = fs::remove_dir_all(&fx.root);
}

/// A malformed generic knob is refused by the guest **before** the adaptor
/// runs, and the driver reports it as a failure (exit 1), not a score.
#[test]
fn a_malformed_policy_fails_before_the_adaptor_and_dry_run_prints_the_job() {
    let fx = fixture("refuse", "smoke_runner", &[]);
    let adaptor = fx.root.join("adaptor");
    fs::create_dir_all(&adaptor).unwrap();
    let marker = fx.root.join("adaptor-ran");
    exe(
        &adaptor.join("run"),
        &format!(
            "#!/bin/sh\ntouch {}\nprintf '{{\"primary_value\": 1.0}}' > \"$PROOF_OUTPUT_DIR/report.json\"\n",
            marker.display()
        ),
    );
    let (ok, stdout, stderr) = run_driver(
        &fx,
        &[
            "--runner-dir",
            adaptor.to_str().unwrap(),
            "--job",
            "evaluate",
            "--tasks",
            "task-a",
            "--set",
            "agent_exception_policy=zer0",
            "--artifact-tar",
            fx.recipe.to_str().unwrap(),
        ],
        Some(FAKE_KEY),
    );
    assert!(!ok, "a typo in a signed knob must not score\n{stdout}");
    assert!(stderr.contains("Failed"), "{stderr}");
    assert!(stderr.contains("agent_exception_policy"), "{stderr}");
    assert!(!marker.exists(), "the adaptor never ran");

    // Without the topic's BYOK in the environment the driver refuses (exit 2)
    // before spawning anything; a dry run prints the derived job instead.
    let (ok, _, stderr) = run_driver(
        &fx,
        &[
            "--runner-dir",
            adaptor.to_str().unwrap(),
            "--job",
            "evaluate",
            "--tasks",
            "task-a",
            "--artifact-tar",
            fx.recipe.to_str().unwrap(),
        ],
        None,
    );
    assert!(!ok);
    assert!(
        stderr.contains("requires miner BYOK OPENROUTER_API_KEY"),
        "{stderr}"
    );
    let (ok, stdout, _) = run_driver(
        &fx,
        &[
            "--runner-dir",
            adaptor.to_str().unwrap(),
            "--job",
            "evaluate",
            "--n-tasks",
            "1",
            "--artifact-tar",
            fx.recipe.to_str().unwrap(),
            "--dry-run",
        ],
        Some(FAKE_KEY),
    );
    assert!(ok, "dry run");
    let dump: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        dump["frames"],
        serde_json::json!(["hello", "stage_pack", "stage_artifact", "run"])
    );
    assert_eq!(dump["job"]["job"], serde_json::json!("evaluate"));
    assert_eq!(
        dump["job"]["request"]["constraints"]["params"]["n_tasks"],
        serde_json::json!("1")
    );
    assert_eq!(
        dump["job"]["request"]["miner_env"]["OPENROUTER_API_KEY"],
        serde_json::json!("[REDACTED]")
    );
    assert!(!stdout.contains(FAKE_KEY));
    let _ = fs::remove_dir_all(&fx.root);
}

/// The in-repo Harbor reference adaptor through the same frames: the topic's
/// `tasks` selection reaches `filter_tasks.py`, Harbor is invoked on the
/// filtered copy with the full model id, the miner key reaches the harness,
/// and summarize scores the one measured trial.
#[test]
fn the_reference_adaptor_scores_one_task_over_a_fake_harbor() {
    let fx = fixture("harbor", "rlm_fc_in_guest_harbor", &[]);
    let bin = fx.root.join("fakebin");
    fs::create_dir_all(&bin).unwrap();
    exe(
        &bin.join("harbor"),
        r#"#!/bin/bash
set -euo pipefail
path=""; jobs=""
printf '%s\n' "$*" > "$PROOF_WORK_DIR/harbor.argv"
printf '%s\n' "${OPENROUTER_API_KEY:+key-present}" > "$PROOF_WORK_DIR/harbor.key"
while [ $# -gt 0 ]; do
  case "$1" in
    --path|-p) path="$2"; shift 2 ;;
    --jobs-dir) jobs="$2"; shift 2 ;;
    *) shift ;;
  esac
done
for t in "$path"/*/; do
  name="$(basename "$t")"
  job="$jobs/job1/${name}__1"
  mkdir -p "$job/verifier"
  printf '{"trial_name": "%s__1", "verifier_result": {"rewards": {"reward": 1.0}}}\n' "$name" > "$job/result.json"
  printf '1.0\n' > "$job/verifier/reward.txt"
done
"#,
    );
    exe(
        &bin.join("docker"),
        "#!/bin/sh\n[ \"$1\" = info ] && exit 0\nexit 1\n",
    );
    let out = fx.root.join("outcome.json");
    let work = fx.root.join("work");
    let (ok, stdout, stderr) = run_driver(
        &fx,
        &[
            "--runner-dir",
            repo()
                .join("deploy/guest/runners/rlm_fc_in_guest_harbor")
                .to_str()
                .unwrap(),
            "--path-prepend",
            bin.to_str().unwrap(),
            "--job",
            "evaluate",
            "--tasks",
            "task-a",
            "--set",
            "exec_timeout_s=900",
            "--artifact-tar",
            fx.recipe.to_str().unwrap(),
            "--work-root",
            work.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        Some(FAKE_KEY),
    );
    assert!(ok, "driver failed\nstdout:\n{stdout}\nstderr:\n{stderr}");
    let outcome: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
    let ev = &outcome["report"]["evidence"];
    assert_eq!(outcome["report"]["primary_value"], serde_json::json!(1.0));
    assert_eq!(ev["n_scored"], serde_json::json!(1));
    assert_eq!(ev["harness_kind"], serde_json::json!("python"));
    assert_eq!(ev["trials"][0]["name"], serde_json::json!("task-a__1"));
    let job_work = work.join("work/0001-evaluate");
    let argv = fs::read_to_string(job_work.join("harbor.argv")).unwrap();
    assert!(
        argv.contains("tasks-filtered"),
        "Harbor ran on the filtered copy: {argv}"
    );
    assert!(
        argv.contains("-a proof_python_agent:ProofPythonAgent"),
        "{argv}"
    );
    assert!(argv.contains("-m openrouter/vendor/model"), "{argv}");
    assert_eq!(
        fs::read_to_string(job_work.join("harbor.key"))
            .unwrap()
            .trim(),
        "key-present"
    );
    let filter: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(job_work.join("tasks-filtered/.proof-task-filter.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(filter["source"], serde_json::json!("params.tasks"));
    assert_eq!(filter["n_kept"], serde_json::json!(1));
    assert_eq!(filter["n_available"], serde_json::json!(2));
    let printed = format!("{stdout}{stderr}{}", fs::read_to_string(&out).unwrap());
    assert!(!printed.contains(FAKE_KEY));
    let _ = fs::remove_dir_all(&fx.root);
}
