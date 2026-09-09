//! The operator bake tooling under `deploy/guest/` stays runnable: scripts
//! parse, the bake plans without root and refuses what it must, and the
//! example adaptor's summariser reads Harbor-shaped trial results. Nothing
//! here builds an image or runs a container.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("proof-bake-tooling-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("dir");
    d
}

fn exe(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn bake(args: &[&str]) -> (bool, String) {
    let out = Command::new("bash")
        .arg(repo().join("deploy/guest/bake-rootfs.sh"))
        .args(args)
        .output()
        .expect("run bake");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn guest_scripts_parse() {
    for (shell, file) in [
        ("bash", "deploy/guest/bake-rootfs.sh"),
        ("bash", "deploy/guest/runners/harbor-podman/run"),
        ("sh", "deploy/guest/init.sh"),
        ("sh", "deploy/guest/agent-loop.sh"),
    ] {
        let status = Command::new(shell)
            .arg("-n")
            .arg(repo().join(file))
            .status()
            .expect("shell");
        assert!(status.success(), "{file} does not parse under {shell} -n");
    }
    let status = Command::new("python3")
        .args(["-m", "py_compile"])
        .arg(repo().join("deploy/guest/runners/harbor-podman/summarize.py"))
        .status()
        .expect("python3");
    assert!(status.success(), "summarize.py does not compile");
}

/// The bake plans without root and without network, refuses a floating
/// Harbor install, a malformed runner id, and an adaptor without `run`, and
/// never prints a digest it did not compute.
#[test]
fn bake_dry_run_plans_and_refuses_what_it_must() {
    let d = tmp("dryrun");
    let agent = d.join("proof-vm-guest-agent");
    exe(&agent, "#!/bin/sh\nexit 0\n");
    let adaptor = d.join("adaptor");
    std::fs::create_dir_all(&adaptor).expect("adaptor dir");
    exe(&adaptor.join("run"), "#!/bin/sh\nexit 0\n");
    let agent_s = agent.display().to_string();
    let spec = format!("operator_runner_v0={}", adaptor.display());

    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--runner",
        &spec,
        "--with-harbor",
        "--harbor-version",
        "0.13.2",
        "--resolver",
        "1.1.1.1",
        "--dry-run",
    ]);
    assert!(ok, "{text}");
    assert!(
        text.contains("runners          operator_runner_v0"),
        "{text}"
    );
    assert!(text.contains("harbor==0.13.2"), "{text}");
    assert!(
        text.contains("rootless (crun, fuse-overlayfs, cgroupfs)"),
        "{text}"
    );
    assert!(text.contains("tree budget 2560 MiB"), "{text}");
    assert!(text.contains("dry run: nothing built"), "{text}");
    assert!(
        !text.contains("digest: sha256:"),
        "a dry run computes no digest: {text}"
    );

    let (ok, text) = bake(&["--guest-agent", &agent_s, "--with-harbor", "--dry-run"]);
    assert!(!ok);
    assert!(text.contains("--harbor-version"), "{text}");

    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--runner",
        &format!("Bad Id={}", adaptor.display()),
        "--dry-run",
    ]);
    assert!(!ok);
    assert!(text.contains("must match"), "{text}");

    let empty = d.join("empty-adaptor");
    std::fs::create_dir_all(&empty).expect("dir");
    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--runner",
        &format!("operator_runner_v0={}", empty.display()),
        "--dry-run",
    ]);
    assert!(!ok);
    assert!(text.contains("is not executable"), "{text}");

    let (ok, text) = bake(&["--dry-run"]);
    assert!(!ok);
    assert!(text.contains("--guest-agent is required"), "{text}");

    let (ok, text) = bake(&["--guest-agent", &agent_s, "--run-as-uid", "0", "--dry-run"]);
    assert!(!ok);
    assert!(text.contains("unprivileged uid"), "{text}");

    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--size-mib",
        "2000",
        "--budget-mib",
        "2560",
        "--dry-run",
    ]);
    assert!(!ok);
    assert!(text.contains("must exceed"), "{text}");

    // The kernel-config check names every missing symbol.
    let cfg = d.join("kernel.config");
    std::fs::write(&cfg, "CONFIG_USER_NS=y\nCONFIG_VIRTIO_VSOCKETS=y\n").expect("cfg");
    let (ok, text) = bake(&[
        "--guest-agent",
        &agent_s,
        "--check-kernel-config",
        &cfg.display().to_string(),
        "--dry-run",
    ]);
    assert!(!ok);
    assert!(
        text.contains("kernel config lacks") && text.contains("CONFIG_FUSE_FS"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// The example adaptor's summariser reads Harbor's per-trial `result.json`
/// (`verifier_result.rewards.reward`, `exception_info`) into the report
/// contract; an errored trial counts 0 and FLOPs appear only when the topic
/// supplies an accounting figure.
#[test]
fn harbor_summariser_reads_trial_results_into_the_report_contract() {
    let d = tmp("summarise");
    let jobs = d.join("jobs");
    for (job, trial, body) in [
        (
            "task-a",
            "task-a__agent__attempt-1",
            r#"{"task_name":"task-a","verifier_result":{"rewards":{"reward":1.0}},"exception_info":null}"#,
        ),
        (
            "task-b",
            "task-b__agent__attempt-1",
            r#"{"task_name":"task-b","verifier_result":{"rewards":{"reward":0.5}},"exception_info":null}"#,
        ),
        (
            "task-c",
            "task-c__agent__attempt-1",
            r#"{"task_name":"task-c","verifier_result":null,"exception_info":{"exception_type":"AgentTimeoutError"}}"#,
        ),
        ("task-d", "task-d__agent__attempt-1", "not json"),
    ] {
        let dir = jobs.join(job).join(trial);
        std::fs::create_dir_all(&dir).expect("trial dir");
        std::fs::write(dir.join("result.json"), body).expect("result");
    }
    let report = d.join("report.json");
    let summarise = |fpt: &str| {
        Command::new("python3")
            .arg(repo().join("deploy/guest/runners/harbor-podman/summarize.py"))
            .arg(&jobs)
            .arg(&report)
            .arg(fpt)
            .output()
            .expect("python3")
    };
    let out = summarise("");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("report")).expect("json");
    assert!(
        (doc["primary_value"].as_f64().unwrap() - 0.375).abs() < 1e-12,
        "{doc}"
    );
    assert_eq!(doc["claim_holds"], serde_json::json!(false));
    assert!(
        doc.get("flops_used").is_none(),
        "no accounting param, no FLOP figure: {doc}"
    );
    assert_eq!(doc["evidence"]["n_trials"], serde_json::json!(4));
    let trials = doc["evidence"]["trials"].as_array().unwrap();
    assert_eq!(trials[2]["error"], serde_json::json!("exception"));
    assert!(trials[3]["error"].as_str().unwrap().contains("unreadable"));
    let parsed: proof_vm_guest::RunnerReport =
        serde_json::from_value(doc).expect("matches the agent's RunnerReport");
    assert!(parsed.primary_value.is_finite());

    let out = summarise("250");
    assert!(out.status.success());
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("report")).expect("json");
    assert_eq!(doc["flops_used"], serde_json::json!(1000));

    let empty = d.join("no-jobs");
    std::fs::create_dir_all(&empty).expect("dir");
    let out = Command::new("python3")
        .arg(repo().join("deploy/guest/runners/harbor-podman/summarize.py"))
        .arg(&empty)
        .arg(&report)
        .output()
        .expect("python3");
    assert!(!out.status.success(), "no trials is not a report");
    let _ = std::fs::remove_dir_all(&d);
}
