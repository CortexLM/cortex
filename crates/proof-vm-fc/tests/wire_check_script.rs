//! The operator harness `deploy/scripts/proof-vm-wire-check.sh` against the
//! in-process fake agent: the `agent` and `boot-probe` subcommands must speak
//! the JSON the agent router speaks (create / attach / 409 / teardown / 404)
//! and never leak the bearer. No Firecracker, no VM, loopback only — what CI
//! runs. Skipped where bash, curl, or python3 are missing.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use proof_rlm::fixtures::pinned_template;
use proof_rlm::RetainPolicy;
use proof_vm_agent::fixtures::{token_file, FakeAgent, FakeHypervisor};

const TOKEN: &str = "wire-check-script-bearer-not-a-real-secret";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn tools_present() -> bool {
    ["bash", "curl", "python3"].iter().all(|tool| {
        Command::new("sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// A CP env file for the script plus the "container" secrets dir it maps to.
fn cp_env(tag: &str, agent_url: &str, digest: &str) -> (PathBuf, PathBuf) {
    let dir =
        std::env::temp_dir().join(format!("proof-vm-wire-script-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let secrets = dir.join("secrets");
    std::fs::create_dir_all(&secrets).expect("secrets dir");
    std::fs::write(secrets.join("vm_orchestrator_token"), format!("{TOKEN}\n")).expect("token");
    let env = dir.join("proof-challenge.env");
    std::fs::write(
        &env,
        format!(
            "# test env\nPROOF_VM_ORCHESTRATOR_URL={agent_url}\n\
             PROOF_VM_ORCHESTRATOR_TOKEN_FILE=/run/base/proof/vm_orchestrator_token\n\
             PROOF_RLM_VM_IMAGE_DIGEST={digest}\n\
             PROOF_VM_RUNNER_CUSTOM_IDS=wire_metric\n"
        ),
    )
    .expect("env");
    (env, secrets)
}

fn run_script_env(args: &[&str], env: &[(&str, &str)]) -> (i32, String) {
    let out = Command::new("bash")
        .arg(repo_root().join("deploy/scripts/proof-vm-wire-check.sh"))
        .args(args)
        .envs(env.iter().copied())
        .current_dir(repo_root())
        .output()
        .expect("run script");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().unwrap_or(-1), text)
}

fn run_script(args: &[&str]) -> (bool, String) {
    let (code, text) = run_script_env(args, &[]);
    (code == 0, text)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_and_boot_probe_speak_the_router_json_and_never_print_the_bearer() {
    if !tools_present() {
        eprintln!("skipping: bash / curl / python3 not all present");
        return;
    }
    let agent_token = token_file("wire-script", TOKEN);
    let agent = FakeAgent::serve(FakeHypervisor::new(0.8), &agent_token).await;
    let digest = pinned_template().image_digest;
    let (env, secrets) = cp_env("ok", &agent.url(), &digest);
    let env_s = env.to_string_lossy().into_owned();
    let map = format!("/run/base/proof={}", secrets.display());

    let (ok, text) = tokio::task::spawn_blocking({
        let env_s = env_s.clone();
        let map = map.clone();
        move || run_script(&["env", "--env-file", &env_s, "--path-map", &map])
    })
    .await
    .expect("join");
    assert!(ok, "env check failed:\n{text}");
    assert!(text.contains("bearer file present and non-empty"), "{text}");
    assert!(text.contains("sha256 pin"), "{text}");
    assert!(!text.contains(TOKEN), "bearer printed:\n{text}");

    let (ok, text) = tokio::task::spawn_blocking({
        let env_s = env_s.clone();
        let map = map.clone();
        move || run_script(&["agent", "--env-file", &env_s, "--path-map", &map])
    })
    .await
    .expect("join");
    assert!(ok, "agent check failed:\n{text}");
    assert!(
        text.contains("agent ready (hypervisor=fake vms=0)"),
        "{text}"
    );
    assert!(text.contains("no bearer → 401"), "{text}");
    assert!(text.contains("wrong bearer → 401"), "{text}");
    assert!(!text.contains(TOKEN), "bearer printed:\n{text}");

    let (ok, text) = tokio::task::spawn_blocking({
        let env_s = env_s.clone();
        let map = map.clone();
        move || {
            run_script(&[
                "boot-probe",
                "--env-file",
                &env_s,
                "--path-map",
                &map,
                "--probe-topic",
                "wire-probe-script",
            ])
        }
    })
    .await
    .expect("join");
    assert!(ok, "boot-probe failed:\n{text}");
    for line in [
        "create bound vm wire-probe-script-",
        "agent booted the pinned image",
        "attach returns the same vm",
        "second create for the topic → 409 already_exists",
        "teardown naming another topic → 409 topic_mismatch",
        "teardown destroyed wire-probe-script-",
        "attach after destroy → 404",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
    assert!(!text.contains(TOKEN), "bearer printed:\n{text}");
    let hv = &agent.hypervisor;
    assert_eq!(
        hv.boots().len(),
        1,
        "one VM booted, the 409 create booted none"
    );
    assert_eq!(hv.boots()[0].topic_id, "wire-probe-script");
    assert_eq!(hv.boots()[0].image_digest, digest);
    assert_eq!(hv.teardowns().len(), 1);
    assert_eq!(hv.teardowns()[0].1, RetainPolicy::Destroy);
    assert!(
        agent.state.running().await.is_empty(),
        "nothing outlives the probe"
    );

    // A stopped agent is reported, not swallowed.
    agent.stop();
    let (ok, text) = tokio::task::spawn_blocking(move || {
        run_script(&["agent", "--env-file", &env_s, "--path-map", &map])
    })
    .await
    .expect("join");
    assert!(!ok, "a dead agent must fail the check:\n{text}");
    assert!(text.contains("agent health → HTTP 000"), "{text}");
    let _ = std::fs::remove_dir_all(env.parent().expect("dir"));
}

/// The agent committed the create but its answer never arrived (timeout,
/// dropped connection): the probe must find the VM by topic and destroy it,
/// never leave it running, and a retry on the same topic must not be blocked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_create_answer_is_reconciled_by_topic_and_destroyed() {
    if !tools_present() {
        eprintln!("skipping: bash / curl / python3 not all present");
        return;
    }
    let agent_token = token_file("wire-script-lost", TOKEN);
    let agent = FakeAgent::serve(FakeHypervisor::new(0.8), &agent_token).await;
    let digest = pinned_template().image_digest;
    let (env, secrets) = cp_env("lost", &agent.url(), &digest);
    let env_s = env.to_string_lossy().into_owned();
    let map = format!("/run/base/proof={}", secrets.display());
    let probe_args = |env_s: &str, map: &str| -> Vec<String> {
        [
            "boot-probe",
            "--env-file",
            env_s,
            "--path-map",
            map,
            "--probe-topic",
            "wire-probe-lost",
        ]
        .iter()
        .map(ToString::to_string)
        .collect()
    };

    let (code, text) = tokio::task::spawn_blocking({
        let argv = probe_args(&env_s, &map);
        move || {
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            run_script_env(
                &refs,
                &[("PROOF_VM_WIRE_CHECK_FAULT", "lose-create-answer")],
            )
        }
    })
    .await
    .expect("join");
    assert_eq!(code, 1, "a lost answer is a FAIL, not a pass:\n{text}");
    assert!(text.contains("create → HTTP 000"), "{text}");
    assert!(
        text.contains("reconcile: agent reports vm wire-probe-lost-"),
        "{text}"
    );
    assert!(
        text.contains("destroyed; topic wire-probe-lost free"),
        "{text}"
    );
    assert!(
        !text.contains("left topic wire-probe-lost in flight"),
        "the in-line reconcile must clear the topic before exit:\n{text}"
    );
    let hv = &agent.hypervisor;
    assert_eq!(hv.boots().len(), 1, "the lost create still booted a VM");
    assert_eq!(hv.boots()[0].topic_id, "wire-probe-lost");
    assert_eq!(hv.teardowns().len(), 1, "and it was destroyed");
    assert_eq!(hv.teardowns()[0].1, RetainPolicy::Destroy);
    assert!(
        agent.state.running().await.is_empty(),
        "nothing outlives an ambiguous create"
    );

    let (code, text) = tokio::task::spawn_blocking({
        let argv = probe_args(&env_s, &map);
        move || {
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            run_script_env(&refs, &[])
        }
    })
    .await
    .expect("join");
    assert_eq!(code, 0, "retry after reconcile:\n{text}");
    assert_eq!(hv.boots().len(), 2);
    assert_eq!(hv.teardowns().len(), 2);
    assert!(agent.state.running().await.is_empty());
    let _ = std::fs::remove_dir_all(env.parent().expect("dir"));
}

/// DNS is case-insensitive, so is the guard: a production origin is refused
/// however it is spelled, before any authenticated request, on every probe
/// that would send one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_hosts_are_refused_case_insensitively_before_any_request() {
    if !tools_present() {
        eprintln!("skipping: bash / curl / python3 not all present");
        return;
    }
    for cp in [
        "https://NETWORK.CORTEX.FOUNDATION/challenge/proof",
        "http://user@Chain.JoinBase.AI:8080/challenge/proof",
        "https://api.cortex.foundation./v1",
        "https://sub.network.cortex.foundation:443/challenge/proof",
    ] {
        let (code, text) = tokio::task::spawn_blocking(move || {
            run_script_env(
                &[
                    "submit-probe",
                    "--cp",
                    cp,
                    "--topic",
                    "x",
                    "--expect",
                    "400",
                ],
                &[],
            )
        })
        .await
        .expect("join");
        assert_eq!(code, 2, "{cp} must be refused:\n{text}");
        assert!(text.contains("refusing production host"), "{cp}: {text}");
        assert!(
            !text.contains("POST"),
            "{cp}: no request may go out:\n{text}"
        );
    }
    // A zero (or non-numeric) declaration can only end rejected
    // (flops_under_declared): refused before any request, live run or not.
    for bad in ["0", "abc", "-1"] {
        let (code, text) = tokio::task::spawn_blocking(move || {
            run_script_env(
                &[
                    "submit-probe",
                    "--cp",
                    "http://127.0.0.1:9",
                    "--topic",
                    "x",
                    "--expect",
                    "201",
                    "--allow-live-run",
                    "--declared-flops",
                    bad,
                ],
                &[],
            )
        })
        .await
        .expect("join");
        assert_eq!(code, 1, "--declared-flops {bad} must be refused:\n{text}");
        assert!(
            text.contains("--declared-flops must be a positive integer"),
            "{bad}: {text}"
        );
        assert!(
            !text.contains("POST"),
            "{bad}: no request may go out:\n{text}"
        );
    }

    // The agent URL is refused at env load, before boot-probe reads the bearer.
    let (secrets, _) = {
        let (env, secrets) = cp_env(
            "prod-agent",
            "https://NETWORK.Cortex.Foundation:8200",
            &pinned_template().image_digest,
        );
        (secrets, env)
    };
    let env_s = secrets
        .parent()
        .expect("dir")
        .join("proof-challenge.env")
        .to_string_lossy()
        .into_owned();
    let map = format!("/run/base/proof={}", secrets.display());
    let (code, text) = tokio::task::spawn_blocking(move || {
        run_script_env(
            &["boot-probe", "--env-file", &env_s, "--path-map", &map],
            &[],
        )
    })
    .await
    .expect("join");
    assert_eq!(code, 2, "{text}");
    assert!(text.contains("refusing production host"), "{text}");
    let _ = std::fs::remove_dir_all(secrets.parent().expect("dir"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn env_check_fails_closed_on_unpinned_digest_and_empty_bearer() {
    if !tools_present() {
        eprintln!("skipping: bash / curl / python3 not all present");
        return;
    }
    let (env, secrets) = cp_env("unpinned", "https://kvm.example.invalid:8200", "");
    let env_s = env.to_string_lossy().into_owned();
    let map = format!("/run/base/proof={}", secrets.display());
    let (ok, text) = tokio::task::spawn_blocking({
        let env_s = env_s.clone();
        let map = map.clone();
        move || run_script(&["env", "--env-file", &env_s, "--path-map", &map])
    })
    .await
    .expect("join");
    assert!(!ok, "unpinned digest must fail:\n{text}");
    assert!(text.contains("PROOF_RLM_VM_IMAGE_DIGEST unset"), "{text}");
    assert!(text.contains("DO NOT INVENT ONE"), "{text}");
    assert!(
        text.contains("PROOF_VM_ORCHESTRATOR_URL is https"),
        "{text}"
    );

    std::fs::write(secrets.join("vm_orchestrator_token"), "\n").expect("empty token");
    let (ok, text) = tokio::task::spawn_blocking(move || {
        run_script(&["env", "--env-file", &env_s, "--path-map", &map])
    })
    .await
    .expect("join");
    assert!(!ok, "{text}");
    assert!(
        text.contains("is empty → ready() 503 naming PROOF_VM_ORCHESTRATOR_TOKEN_FILE"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(env.parent().expect("dir"));
}
