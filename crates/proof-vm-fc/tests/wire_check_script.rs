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
    // A non-numeric declaration is refused before any request.
    for bad in ["abc", "-1"] {
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
            text.contains("--declared-flops must be a non-negative integer"),
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

/// A live probe (`--expect 2xx`) is judged on the bytes the RLM VM fetches,
/// so it must carry the sha256 of a real artefact: no digest, a digest of
/// nothing (empty input / empty tar), an empty or compressed file, or a URI
/// the VM cannot reach are all refused before any request. With a real tar
/// and a reachable-looking URI the request goes out (and fails here only
/// because there is no CP on port 9).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn a_live_probe_needs_a_real_artefact_never_a_digest_of_nothing() {
    if !tools_present()
        || !Command::new("sh")
            .args(["-c", "command -v tar"])
            .output()
            .is_ok_and(|o| o.status.success())
    {
        eprintln!("skipping: bash / curl / python3 / tar not all present");
        return;
    }
    let dir = std::env::temp_dir().join(format!("proof-vm-wire-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("recipe")).expect("dir");
    std::fs::write(dir.join("recipe/run.sh"), "#!/bin/sh\necho hi\n").expect("file");
    let real = dir.join("recipe.tar");
    let status = Command::new("tar")
        .args([
            "-cf",
            &real.to_string_lossy(),
            "-C",
            &dir.to_string_lossy(),
            "recipe",
        ])
        .status()
        .expect("tar");
    assert!(status.success());
    let empty_tar = dir.join("empty.tar");
    std::fs::write(&empty_tar, vec![0u8; 10_240]).expect("empty tar");
    let gz = dir.join("recipe.tar.gz");
    std::fs::write(&gz, [0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0, 0, 3, 1, 2, 3]).expect("gz");
    let empty_input_digest = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let base = |extra: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = [
            "submit-probe",
            "--cp",
            "http://127.0.0.1:9",
            "--topic",
            "x",
            "--expect",
            "201",
            "--allow-live-run",
            "--declared-flops",
            "5",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        v.extend(extra.iter().map(ToString::to_string));
        v
    };
    let real_s = real.to_string_lossy().into_owned();
    let empty_s = empty_tar.to_string_lossy().into_owned();
    let gz_s = gz.to_string_lossy().into_owned();
    let cases: Vec<(Vec<String>, i32, &str)> = vec![
        (base(&[]), 2, "a live run needs the digest"),
        (
            base(&["--artifact-digest", empty_input_digest]),
            2,
            "sha256 of nothing",
        ),
        (base(&["--artifact-file", &empty_s]), 2, "no file content"),
        (base(&["--artifact-file", &gz_s]), 2, "gzip-compressed"),
        (
            base(&[
                "--artifact-file",
                &real_s,
                "--artifact-uri",
                "http://127.0.0.1:8000/recipe.tar",
            ]),
            2,
            "not reachable from the RLM VM",
        ),
        (
            base(&["--artifact-file", &real_s]),
            2,
            "not reachable from the RLM VM",
        ),
        (
            base(&[
                "--artifact-file",
                &real_s,
                "--artifact-digest",
                empty_input_digest,
            ]),
            2,
            "is not the sha256 of --artifact-file",
        ),
    ];
    for (argv, want_code, want_text) in cases {
        let (code, text) = tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            run_script_env(&refs, &[])
        })
        .await
        .expect("join");
        assert_eq!(code, want_code, "{want_text}:\n{text}");
        assert!(text.contains(want_text), "{want_text}:\n{text}");
        assert!(
            !text.contains("POST http"),
            "{want_text}: no request may go out:\n{text}"
        );
    }
    // Real tar, a URI off loopback, fetch check skipped: the probe proceeds
    // to the CP with the file's digest — and reports the dead CP, not a pass.
    let argv = base(&[
        "--artifact-file",
        &real_s,
        "--artifact-uri",
        "https://artefacts.example.test/recipe.tar",
        "--no-fetch-check",
    ]);
    let (code, text) = tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        run_script_env(&refs, &[])
    })
    .await
    .expect("join");
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("uncompressed tar"), "{text}");
    assert!(text.contains("expected HTTP 201, got 000"), "{text}");
    // The fetch check itself: a URL nothing serves is a stop before the POST.
    let argv = base(&[
        "--artifact-file",
        &real_s,
        "--artifact-uri",
        "https://artefacts.example.test/recipe.tar",
    ]);
    let (code, text) = tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        run_script_env(&refs, &[])
    })
    .await
    .expect("join");
    assert_eq!(code, 1, "{text}");
    assert!(
        text.contains("artefact fetch from this host failed"),
        "{text}"
    );
    assert!(
        !text.contains("POST http"),
        "no request after a failed fetch:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
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
