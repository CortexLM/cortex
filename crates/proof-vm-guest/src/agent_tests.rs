//! The guest agent end to end over `handle` / `serve_connection`, with shell
//! scripts standing in for operator adaptors. No vsock, no container
//! runtime, no network beyond loopback.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use proof_rlm::fixtures::{experiment_request, request, rules, topic};
use proof_rlm::{CustomRunRequest, RuleSet, VmJob, VmJobOutput};
use proof_vm_proto::guest::{read_frame, write_frame, HostToRlm, RlmToHost, StagedFile};
use proof_vm_proto::tar::fixtures::{archive, member};
use proof_vm_proto::API_VERSION;
use sha2::{Digest, Sha256};

use crate::runner::env;
use crate::{GuestAgent, GuestConfig};

const RUNNER: &str = "placeholder_in_guest_runner";
const SECRET: &str = "owner-key-not-a-real-secret-0123456789";
/// The miner's own value in the BYOK tests. Never an owner key.
const MINER_KEY: &str = "miner-supplied-value-not-a-real-key";

fn root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("proof-vm-guest-agent-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("dir");
    d
}

fn agent(r: &Path) -> std::sync::Arc<GuestAgent> {
    let mut cfg = crate::under(r, &GuestConfig::defaults());
    cfg.allow_plain_http = true;
    GuestAgent::new(cfg)
}

/// Install `script` as `<runners>/<RUNNER>/<entry>`.
fn install(r: &Path, entry: &str, script: &str) {
    let dir = r.join("runners").join(RUNNER);
    std::fs::create_dir_all(&dir).expect("adaptor dir");
    let path = dir.join(entry);
    std::fs::write(&path, format!("#!/bin/sh\nset -eu\n{script}\n")).expect("script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

/// A pack tar + its pin.
fn pack() -> (Vec<u8>, String) {
    let tar = archive(&[member(
        "pack/tasks/one/task.toml",
        b'0',
        b"name = \"one\"\n",
    )]);
    (
        tar.clone(),
        format!("sha256:{}", hex::encode(Sha256::digest(&tar))),
    )
}

/// `experiment_request` whose pack pin is `digest`.
fn req_for(digest: &str) -> CustomRunRequest {
    let mut req = experiment_request(None);
    req.constraints.params.insert(
        proof_experiment::PARAM_PACK_DIGEST.into(),
        digest.to_owned(),
    );
    req.artifact_uri = None;
    req
}

async fn hello(a: &GuestAgent) {
    let ready = a
        .handle(HostToRlm::Hello {
            api_version: API_VERSION,
            topic_id: "topic-a".into(),
            vm_id: "topic-a-x0001".into(),
        })
        .await;
    assert!(matches!(ready, RlmToHost::Ready { api_version, .. } if api_version == API_VERSION));
}

async fn stage(a: &GuestAgent, tar: &[u8], digest: &str) {
    let staged = a
        .handle(HostToRlm::StageSecrets {
            files: vec![StagedFile::new(
                "inference_key",
                format!("{SECRET}\n").as_bytes(),
            )],
        })
        .await;
    assert_eq!(staged, RlmToHost::Staged { count: 1 });
    let staged = a
        .handle(HostToRlm::StagePack {
            digest: digest.to_owned(),
            pack_tar: StagedFile::new("pack.tar", tar),
        })
        .await;
    assert_eq!(
        staged,
        RlmToHost::PackStaged {
            digest: digest.to_owned(),
            bytes: tar.len() as u64
        }
    );
}

fn failed(answer: RlmToHost) -> String {
    match answer {
        RlmToHost::Failed { error } => error,
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// Serve `body` once over loopback HTTP; returns the URL.
async fn serve_once(body: Vec<u8>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.expect("accept");
        let mut buf = [0u8; 4096];
        let _ = s.read(&mut buf).await;
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = s.write_all(head.as_bytes()).await;
        let _ = s.write_all(&body).await;
        let _ = s.shutdown().await;
    });
    format!("http://{addr}/recipe.tar")
}

/// The no-placeholder guarantee: a job whose topic selects no runner, or
/// whose runner is not installed, or whose pack is not staged, fails — the
/// agent never answers `Done` with an invented value.
#[tokio::test]
async fn nothing_is_defaulted_no_runner_no_adaptor_no_pack_is_a_failed_job() {
    let r = root("fail-closed");
    let a = agent(&r);
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline { request: request() }),
        })
        .await,
    );
    assert!(err.contains("no hello"), "{err}");
    hello(&a).await;
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline { request: request() }),
        })
        .await,
    );
    assert!(err.contains("selects no in-guest runner"), "{err}");
    assert!(err.contains("never reports a placeholder value"), "{err}");
    let (tar, digest) = pack();
    let req = req_for(&digest);
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline {
                request: req.clone(),
            }),
        })
        .await,
    );
    assert!(err.contains("not installed in this guest image"), "{err}");
    install(
        &r,
        "run",
        "echo '{\"primary_value\": 1.0}' > \"$PROOF_OUTPUT_DIR/report.json\"",
    );
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline {
                request: req.clone(),
            }),
        })
        .await,
    );
    assert!(err.contains("no experiment pack staged"), "{err}");
    let mut other = req.clone();
    other.topic_id = "topic-b".into();
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline { request: other }),
        })
        .await,
    );
    assert!(err.contains("bound to"), "{err}");
    let wrong = failed(
        a.handle(HostToRlm::StagePack {
            digest: format!("sha256:{}", "00".repeat(32)),
            pack_tar: StagedFile::new("pack.tar", &tar),
        })
        .await,
    );
    assert!(wrong.contains("hashes to"), "{wrong}");
    let bad_version = failed(
        a.handle(HostToRlm::Hello {
            api_version: 99,
            topic_id: "topic-a".into(),
            vm_id: "v".into(),
        })
        .await,
    );
    assert!(bad_version.contains("api_version"), "{bad_version}");
    let _ = std::fs::remove_dir_all(&r);
}

/// The happy path through `run`: the contract reaches the adaptor, the
/// report becomes a bound `CustomRunReport`, secrets are redacted from logs
/// and evidence, and `Evaluate` needs (and verifies) the miner's artefact.
#[tokio::test]
async fn an_installed_adaptor_runs_under_the_contract_and_secrets_never_travel() {
    let r = root("run");
    let a = agent(&r);
    hello(&a).await;
    let (tar, digest) = pack();
    stage(&a, &tar, &digest).await;
    install(
        &r,
        "run",
        r#"
test -f "$PROOF_PACK_DIR/pack/tasks/one/task.toml"
key=$(cat "$PROOF_SECRETS_DIR/inference_key")
echo "job=$PROOF_JOB runner=$PROOF_RUNNER_ID model=$PROOF_MODEL_PIN param=$PROOF_PARAM_PARAM_A files=$PROOF_SECRET_FILES key=$key"
env | grep -c '^PROOF_' >&2
cat > "$PROOF_OUTPUT_DIR/report.json" <<EOF
{"primary_value": 0.73, "claim_holds": true, "flops_used": 42, "evidence": {"note": "used $key", "rows": [1,2]}}
EOF
"#,
    );
    let req = req_for(&digest);
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline {
                request: req.clone(),
            }),
        })
        .await;
    let RlmToHost::Done {
        output: VmJobOutput::Baseline(report),
    } = out
    else {
        panic!("expected a baseline report, got {out:?}");
    };
    report.verify(&req).expect("bound to the request");
    assert!((report.primary_value - 0.73).abs() < 1e-12);
    assert!(report.claim_holds);
    assert_eq!(report.flops_used, Some(42));
    assert_eq!(report.evidence["runner"], serde_json::json!(RUNNER));
    assert_eq!(report.evidence["pack_digest"], serde_json::json!(digest));
    assert_eq!(report.evidence["exit_code"], serde_json::json!(0));
    assert_eq!(
        report.evidence["note"],
        serde_json::json!("used [REDACTED]"),
        "the secret is blanked in evidence"
    );
    let dump = serde_json::to_string(&report).expect("json");
    assert!(!dump.contains(SECRET), "{dump}");

    // Evaluate: the artefact is required, fetched, verified, unpacked, and
    // exposed; the log tail is redacted too.
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Evaluate {
                request: req.clone(),
                checklist_digest: "c".into(),
                rules_version: 1,
            }),
        })
        .await,
    );
    assert!(err.contains("needs the miner's artifact_uri"), "{err}");
    let artefact = archive(&[member("recipe/run.sh", b'0', b"echo hi\n")]);
    let mut eval = req.clone();
    eval.artifact_digest = hex::encode(Sha256::digest(&artefact));
    eval.artifact_uri = Some(serve_once(artefact.clone()).await);
    install(
        &r,
        "run",
        r#"
test -f "$PROOF_ARTIFACT_DIR/recipe/run.sh"
test -f "$PROOF_WORK_DIR/artifact.tar"
echo "secret in log: $(cat "$PROOF_SECRETS_DIR/inference_key")"
echo '{"primary_value": 0.5, "flops_used": 7}' > "$PROOF_OUTPUT_DIR/report.json"
"#,
    );
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Evaluate {
                request: eval.clone(),
                checklist_digest: "c".into(),
                rules_version: 1,
            }),
        })
        .await;
    let RlmToHost::Done {
        output: VmJobOutput::Evaluated(run),
    } = out
    else {
        panic!("expected an evaluated run, got {out:?}");
    };
    run.report.verify(&eval).expect("bound");
    assert!(!run.report.claim_holds, "claim_holds defaults to false");
    assert_eq!(run.report.flops_used, Some(7));
    assert_eq!(run.logs.len(), 1);
    let log = String::from_utf8_lossy(&run.logs[0].bytes);
    assert!(log.contains("secret in log: [REDACTED]"), "{log}");
    assert!(!log.contains(SECRET));

    // The served bytes do not hash to the paid digest: refused, no run.
    let mut mismatch = eval.clone();
    mismatch.artifact_digest = "00".repeat(32);
    mismatch.artifact_uri = Some(serve_once(artefact).await);
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Evaluate {
                request: mismatch,
                checklist_digest: "c".into(),
                rules_version: 1,
            }),
        })
        .await,
    );
    assert!(err.contains("refusing to run a substitute"), "{err}");
    let _ = std::fs::remove_dir_all(&r);
}

/// Upload path: the host injects vault bytes over vsock; the guest skips
/// HTTP and still verifies digest + tar before the adaptor runs.
#[tokio::test]
async fn a_staged_inject_skips_http_and_verifies_the_digest() {
    let r = root("inject");
    let a = agent(&r);
    hello(&a).await;
    let (pack_tar, pack_digest) = pack();
    stage(&a, &pack_tar, &pack_digest).await;
    let artefact = archive(&[member("recipe/run.sh", b'0', b"echo hi\n")]);
    let digest = hex::encode(Sha256::digest(&artefact));
    let mut eval = req_for(&pack_digest);
    eval.artifact_digest = digest.clone();
    eval.artifact_uri = Some(format!("proof-artefact://{digest}"));
    install(
        &r,
        "run",
        r#"
test -f "$PROOF_ARTIFACT_DIR/recipe/run.sh"
test -f "$PROOF_WORK_DIR/artifact.tar"
echo '{"primary_value": 0.61, "flops_used": 3}' > "$PROOF_OUTPUT_DIR/report.json"
"#,
    );
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Evaluate {
                request: eval.clone(),
                checklist_digest: "c".into(),
                rules_version: 1,
            }),
        })
        .await,
    );
    assert!(
        err.contains("needs a host inject") || err.contains("proof-artefact"),
        "{err}"
    );
    let staged = a
        .handle(HostToRlm::StageArtifact {
            digest: digest.clone(),
            artifact_tar: StagedFile::new("artifact.tar", &artefact),
        })
        .await;
    assert_eq!(
        staged,
        RlmToHost::ArtifactStaged {
            digest: digest.clone(),
            bytes: artefact.len() as u64
        }
    );
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Evaluate {
                request: eval.clone(),
                checklist_digest: "c".into(),
                rules_version: 1,
            }),
        })
        .await;
    let RlmToHost::Done {
        output: VmJobOutput::Evaluated(run),
    } = out
    else {
        panic!("expected an evaluated run, got {out:?}");
    };
    run.report.verify(&eval).expect("bound");
    assert!((run.report.primary_value - 0.61).abs() < 1e-12);
    let mismatch = a
        .handle(HostToRlm::StageArtifact {
            digest: "00".repeat(32),
            artifact_tar: StagedFile::new("artifact.tar", &artefact),
        })
        .await;
    assert!(
        matches!(mismatch, RlmToHost::Failed { ref error } if error.contains("does not verify")),
        "{mismatch:?}"
    );
    let _ = std::fs::remove_dir_all(&r);
}

/// A run that writes no report, a non-finite value, or outlives its deadline
/// is a failed job — with the (redacted) tail as the reason, never a number.
#[tokio::test]
async fn bad_reports_and_deadline_cuts_fail_the_job() {
    let r = root("bad");
    let a = agent(&r);
    hello(&a).await;
    let (tar, digest) = pack();
    stage(&a, &tar, &digest).await;
    let req = req_for(&digest);
    let job = || {
        Box::new(VmJob::Baseline {
            request: req.clone(),
        })
    };
    install(
        &r,
        "run",
        "echo \"nothing written for $(cat \\\"$PROOF_SECRETS_DIR/inference_key\\\")\"; exit 3",
    );
    let err = failed(a.handle(HostToRlm::Run { job: job() }).await);
    assert!(err.contains("wrote no report.json"), "{err}");
    assert!(err.contains("exit Some(3)"), "{err}");
    assert!(!err.contains(SECRET), "{err}");
    install(
        &r,
        "run",
        "echo '{\"primary_value\": \"NaN\"}' > \"$PROOF_OUTPUT_DIR/report.json\"",
    );
    let err = failed(a.handle(HostToRlm::Run { job: job() }).await);
    assert!(err.contains("did not parse"), "{err}");
    install(
        &r,
        "run",
        "echo '{\"primary_value\": 1e999}' > \"$PROOF_OUTPUT_DIR/report.json\"",
    );
    let err = failed(a.handle(HostToRlm::Run { job: job() }).await);
    assert!(
        err.contains("did not parse") || err.contains("not finite"),
        "{err}"
    );
    install(
        &r,
        "run",
        "sleep 30; echo '{\"primary_value\": 1.0}' > \"$PROOF_OUTPUT_DIR/report.json\"",
    );
    let mut short = req.clone();
    short.sandbox.deadline_s = 6;
    let started = std::time::Instant::now();
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline { request: short }),
        })
        .await,
    );
    assert!(err.contains("cut at the deadline of 6s"), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    let _ = std::fs::remove_dir_all(&r);
}

/// Two distinct signed param names that normalise to the same
/// `PROOF_PARAM_*` variable (`foo-bar` / `foo_bar`) are refused **before the
/// adaptor runs** — a run never executes with one signed input silently
/// replaced by another — while a lone hyphenated name still maps.
#[tokio::test]
async fn colliding_param_names_are_refused_before_the_adaptor_runs() {
    let r = root("collide");
    let a = agent(&r);
    hello(&a).await;
    let (tar, digest) = pack();
    stage(&a, &tar, &digest).await;
    install(
        &r,
        "run",
        r#"
touch "$PROOF_WORK_DIR/adaptor-ran"
echo "{\"primary_value\": 1.0, \"evidence\": {\"foo_bar\": \"$PROOF_PARAM_FOO_BAR\"}}" > "$PROOF_OUTPUT_DIR/report.json"
"#,
    );
    let mut control = req_for(&digest);
    control
        .constraints
        .params
        .insert("foo-bar".into(), "dash-value".into());
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline {
                request: control.clone(),
            }),
        })
        .await;
    let RlmToHost::Done {
        output: VmJobOutput::Baseline(report),
    } = out
    else {
        panic!("expected a baseline report, got {out:?}");
    };
    assert_eq!(
        report.evidence["foo_bar"],
        serde_json::json!("dash-value"),
        "a lone hyphenated name maps to PROOF_PARAM_FOO_BAR"
    );

    let mut colliding = control.clone();
    colliding
        .constraints
        .params
        .insert("foo_bar".into(), "underscore-value".into());
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline {
                request: colliding.clone(),
            }),
        })
        .await,
    );
    assert!(err.contains("\"foo-bar\" and \"foo_bar\""), "{err}");
    assert!(err.contains("both map to PROOF_PARAM_FOO_BAR"), "{err}");
    assert!(err.contains("re-sign the topic"), "{err}");
    let ran: Vec<_> = walkdir(&r.join("work"))
        .into_iter()
        .filter(|p| p.ends_with("adaptor-ran"))
        .collect();
    assert_eq!(
        ran.len(),
        1,
        "only the control run reached the adaptor: {ran:?}"
    );

    // The same refusal guards inspection and rule proposals.
    install(
        &r,
        "inspect",
        "echo '[]' > \"$PROOF_OUTPUT_DIR/checklist.json\"",
    );
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Inspect {
                request: colliding,
                rules: rules(),
            }),
        })
        .await,
    );
    assert!(err.contains("both map to PROOF_PARAM_FOO_BAR"), "{err}");
    install(
        &r,
        "propose_rules",
        "echo '[{\"id\": \"r_x\", \"text\": \"x\"}]' > \"$PROOF_OUTPUT_DIR/rules.json\"",
    );
    let mut t = topic();
    t.constraints
        .params
        .insert(proof_experiment::PARAM_RUNNER.into(), RUNNER.into());
    t.constraints
        .params
        .insert(proof_experiment::PARAM_PACK_DIGEST.into(), digest.clone());
    t.constraints.params.insert("foo-bar".into(), "a".into());
    t.constraints.params.insert("foo_bar".into(), "b".into());
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::ProposeRules {
                topic: Box::new(t),
                current_version: None,
            }),
        })
        .await,
    );
    assert!(err.contains("both map to PROOF_PARAM_FOO_BAR"), "{err}");
    let _ = std::fs::remove_dir_all(&r);
}

/// The generic run policy is shape-checked in the guest too: a malformed
/// signed knob (`agent_exception_policy = zer0`, `n_tasks = 0`) fails the
/// job **before the adaptor runs** (no spend), while a well-formed one —
/// the single-task smoke shape included — reaches the adaptor verbatim as
/// `PROOF_PARAM_*` and nothing else about the contract changes.
#[tokio::test]
async fn a_malformed_run_policy_never_reaches_the_adaptor() {
    let r = root("policy");
    let a = agent(&r);
    hello(&a).await;
    let (tar, digest) = pack();
    stage(&a, &tar, &digest).await;
    install(
        &r,
        "run",
        r#"
touch "$PROOF_WORK_DIR/adaptor-ran"
echo "{\"primary_value\": 0.5, \"evidence\": {\"tasks\": \"$PROOF_PARAM_TASKS\", \"policy\": \"${PROOF_PARAM_AGENT_EXCEPTION_POLICY:-unset}\", \"exec_timeout\": \"${PROOF_PARAM_EXEC_TIMEOUT_S:-unset}\"}}" > "$PROOF_OUTPUT_DIR/report.json"
"#,
    );
    for (key, value, why) in [
        (
            proof_experiment::policy::PARAM_AGENT_EXCEPTION_POLICY,
            "zer0",
            "\"fail\" (default) or \"zero\"",
        ),
        (
            proof_experiment::policy::PARAM_N_TASKS,
            "0",
            "positive integer",
        ),
        (
            proof_experiment::policy::PARAM_TASKS,
            "../escape",
            "item names",
        ),
        (
            proof_experiment::policy::PARAM_EXEC_TIMEOUT_S,
            "soon",
            "positive integer of seconds",
        ),
    ] {
        let mut bad = req_for(&digest);
        bad.constraints.params.insert(key.into(), value.into());
        let err = failed(
            a.handle(HostToRlm::Run {
                job: Box::new(VmJob::Baseline { request: bad }),
            })
            .await,
        );
        assert!(err.contains(key), "{key}={value}: {err}");
        assert!(err.contains(why), "{key}={value}: {err}");
        assert!(err.contains("re-sign the topic"), "{err}");
    }
    assert!(
        walkdir(&r.join("work"))
            .iter()
            .all(|p| !p.ends_with("adaptor-ran")),
        "no malformed policy reached the adaptor"
    );

    let mut smoke = req_for(&digest);
    smoke
        .constraints
        .params
        .insert(proof_experiment::policy::PARAM_TASKS.into(), "one".into());
    smoke.constraints.params.insert(
        proof_experiment::policy::PARAM_AGENT_EXCEPTION_POLICY.into(),
        "zero".into(),
    );
    smoke.constraints.params.insert(
        proof_experiment::policy::PARAM_EXEC_TIMEOUT_S.into(),
        "900".into(),
    );
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline { request: smoke }),
        })
        .await;
    let RlmToHost::Done {
        output: VmJobOutput::Baseline(report),
    } = out
    else {
        panic!("expected a baseline report, got {out:?}");
    };
    assert_eq!(report.evidence["tasks"], serde_json::json!("one"));
    assert_eq!(report.evidence["policy"], serde_json::json!("zero"));
    assert_eq!(report.evidence["exec_timeout"], serde_json::json!("900"));
    let _ = std::fs::remove_dir_all(&r);
}

fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// An adaptor that floods stdout and stderr (well past the 64 KiB tail, on
/// both streams at once) still completes: the guest drains both streams
/// concurrently into rolling tails — bounded while the process runs, not
/// only after it exits — and the log the host sees is the documented tail
/// with a marker for what was cut. The `Tail` itself never holds more than
/// its cap, whatever is pushed through it.
#[tokio::test]
async fn adaptor_output_is_bounded_while_draining_not_after() {
    use crate::runner::{Tail, MAX_TAIL_BYTES, STREAM_TAIL_BYTES};
    assert_eq!(STREAM_TAIL_BYTES * 2, MAX_TAIL_BYTES);
    let mut t = Tail::new(1_000);
    t.push(&[b'a'; 600]);
    assert_eq!(t.bytes().len(), 600);
    assert_eq!(t.dropped(), 0);
    t.push(&[b'b'; 600]);
    assert_eq!(t.bytes().len(), 1_000, "capped");
    assert_eq!(t.dropped(), 200);
    assert!(t.bytes().starts_with(&[b'a'; 400]));
    assert!(t.bytes().ends_with(&[b'b'; 600]));
    t.push(&[b'c'; 5_000]);
    assert_eq!(
        t.bytes().len(),
        1_000,
        "one chunk over the cap keeps its last cap bytes"
    );
    assert_eq!(t.dropped(), 200 + 1_000 + 4_000);
    assert!(t.bytes().iter().all(|b| *b == b'c'));
    assert!(t
        .text()
        .starts_with("[... 5200 earlier bytes dropped ...]\n"));
    assert_eq!(Tail::new(8).text(), "");

    let r = root("flood");
    let a = agent(&r);
    hello(&a).await;
    let (tar, digest) = pack();
    stage(&a, &tar, &digest).await;
    // 8 MiB on each stream, stderr first (a sequential drain waiting on
    // stdout EOF would deadlock on the full stderr pipe until the deadline).
    install(
        &r,
        "run",
        r#"
head -c 8388608 /dev/zero | tr '\0' 'e' >&2
head -c 8388608 /dev/zero | tr '\0' 'o'
echo "LAST-STDOUT-LINE"
echo "LAST-STDERR-LINE" >&2
echo '{"primary_value": 0.25}' > "$PROOF_OUTPUT_DIR/report.json"
"#,
    );
    let mut req = req_for(&digest);
    req.sandbox.deadline_s = 60;
    let started = std::time::Instant::now();
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline { request: req }),
        })
        .await;
    let RlmToHost::Done {
        output: VmJobOutput::Baseline(report),
    } = out
    else {
        panic!("expected a baseline report, got {out:?}");
    };
    assert!((report.primary_value - 0.25).abs() < 1e-12);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "drained concurrently, no pipe deadlock: {:?}",
        started.elapsed()
    );
    // The evaluate path carries the log; drive the same script through it
    // to read the tail the host would see.
    let artefact = archive(&[member("recipe/run.sh", b'0', b"echo hi\n")]);
    let mut eval = req_for(&digest);
    eval.artifact_digest = hex::encode(Sha256::digest(&artefact));
    eval.artifact_uri = Some(serve_once(artefact).await);
    eval.sandbox.deadline_s = 60;
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Evaluate {
                request: eval,
                checklist_digest: "c".into(),
                rules_version: 1,
            }),
        })
        .await;
    let RlmToHost::Done {
        output: VmJobOutput::Evaluated(run),
    } = out
    else {
        panic!("expected an evaluated run, got {out:?}");
    };
    let log = String::from_utf8_lossy(&run.logs[0].bytes);
    assert!(
        run.logs[0].bytes.len() <= MAX_TAIL_BYTES,
        "the documented tail limit holds: {}",
        run.logs[0].bytes.len()
    );
    assert!(
        log.contains("LAST-STDERR-LINE"),
        "the end of stderr survives"
    );
    assert!(log.contains("earlier bytes dropped"), "{}", &log[..200]);
    let _ = std::fs::remove_dir_all(&r);
}

/// Inspection ticks every rule through `inspect` (unanswered rules are red),
/// and rule proposals come from `propose_rules` or, without one, from the
/// signed checklist itself.
#[tokio::test]
async fn inspection_and_rule_proposals_go_through_the_adaptor_or_the_signed_topic() {
    let r = root("inspect");
    let a = agent(&r);
    hello(&a).await;
    let (tar, digest) = pack();
    stage(&a, &tar, &digest).await;
    let req = req_for(&digest);
    let set: RuleSet = rules();
    install(&r, "run", "exit 0");
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Inspect {
                request: req.clone(),
                rules: set.clone(),
            }),
        })
        .await,
    );
    assert!(err.contains("has no inspect entrypoint"), "{err}");
    install(
        &r,
        "inspect",
        r#"
test -f "$PROOF_RULES_FILE"
test "$PROOF_JOB" = inspect
cat > "$PROOF_OUTPUT_DIR/checklist.json" <<EOF
[{"id": "rule_a", "pass": true, "evidence": "saw it"}, {"id": "rule_b", "pass": false, "evidence": "key $(cat "$PROOF_SECRETS_DIR/inference_key")"}]
EOF
"#,
    );
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Inspect {
                request: req.clone(),
                rules: set.clone(),
            }),
        })
        .await;
    let RlmToHost::Done {
        output: VmJobOutput::Inspected(inspected),
    } = out
    else {
        panic!("expected an inspection, got {out:?}");
    };
    assert!(
        !inspected.checklist.is_green(&set),
        "rule_b failed, rule_c unanswered"
    );
    assert_eq!(inspected.checklist.items.len(), 3);
    let by_id = |id: &str| {
        inspected
            .checklist
            .items
            .iter()
            .find(|i| i.id == id)
            .expect(id)
    };
    assert!(by_id("rule_a").pass);
    assert!(!by_id("rule_b").pass);
    assert!(by_id("rule_b").evidence.contains("[REDACTED]"));
    assert!(!by_id("rule_c").pass);
    assert!(by_id("rule_c").evidence.contains("no verdict"));
    assert!(inspected.artifact.is_empty(), "no locator, no tree");

    let t = topic();
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::ProposeRules {
                topic: Box::new(t.clone()),
                current_version: None,
            }),
        })
        .await;
    let RlmToHost::Done {
        output: VmJobOutput::Rules(proposed),
    } = out
    else {
        panic!("expected rules, got {out:?}");
    };
    assert_eq!(proposed, t.checklist, "no adaptor: the signed vector");
    let mut selecting = t.clone();
    selecting
        .constraints
        .params
        .insert(proof_experiment::PARAM_RUNNER.into(), RUNNER.into());
    selecting
        .constraints
        .params
        .insert(proof_experiment::PARAM_PACK_DIGEST.into(), digest.clone());
    install(
        &r,
        "propose_rules",
        r#"
test -f "$PROOF_TOPIC_FILE"
echo '[{"id": "rlm_rule_x", "text": "an adaptor-written rule"}]' > "$PROOF_OUTPUT_DIR/rules.json"
"#,
    );
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::ProposeRules {
                topic: Box::new(selecting.clone()),
                current_version: Some(1),
            }),
        })
        .await;
    let RlmToHost::Done {
        output: VmJobOutput::Rules(proposed),
    } = out
    else {
        panic!("expected rules, got {out:?}");
    };
    assert_eq!(proposed.len(), 1);
    assert_eq!(proposed[0].id, "rlm_rule_x");
    install(
        &r,
        "propose_rules",
        "echo '[{\"id\": \"Bad Id\", \"text\": \"x\"}]' > \"$PROOF_OUTPUT_DIR/rules.json\"",
    );
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::ProposeRules {
                topic: Box::new(selecting),
                current_version: Some(1),
            }),
        })
        .await,
    );
    assert!(err.contains("rules.json checklist[Bad Id]"), "{err}");
    let archived = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Archive {
                topic_id: "topic-a".into(),
            }),
        })
        .await;
    assert_eq!(
        archived,
        RlmToHost::Done {
            output: VmJobOutput::Archived
        }
    );
    let _ = std::fs::remove_dir_all(&r);
}

/// A leftover unsyncable entry under the shared work root (earlier failed
/// job on a reused topic VM) must not turn Archive — or any later success
/// that does not own that path — into Failed.
#[tokio::test]
async fn archive_and_later_jobs_ignore_stale_unsyncable_siblings() {
    let r = root("stale-sibling");
    let a = agent(&r);
    hello(&a).await;
    let work = r.join("work");
    std::fs::create_dir_all(&work).expect("work root");
    std::os::unix::fs::symlink("/no/such-proof-stale-entry", work.join("dangling"))
        .expect("dangling sibling");
    let archived = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Archive {
                topic_id: "topic-a".into(),
            }),
        })
        .await;
    assert_eq!(
        archived,
        RlmToHost::Done {
            output: VmJobOutput::Archived
        },
        "Archive creates no work; a stale sibling must not be synced"
    );
    let (tar, digest) = pack();
    stage(&a, &tar, &digest).await;
    install(
        &r,
        "run",
        "echo '{\"primary_value\": 0.5}' > \"$PROOF_OUTPUT_DIR/report.json\"",
    );
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline {
                request: req_for(&digest),
            }),
        })
        .await;
    assert!(
        matches!(out, RlmToHost::Done { .. }),
        "paid success must flush only its job dir, not the dangling sibling: {out:?}"
    );
    let _ = std::fs::remove_dir_all(&r);
}

/// Framing over a stream, as the host drives it: hello, staging, a job, EOF.
#[tokio::test]
async fn serve_connection_speaks_frames_until_the_host_hangs_up() {
    let r = root("frames");
    let a = agent(&r);
    let (mut host, guest) = tokio::io::duplex(1 << 20);
    let server = {
        let a = a.clone();
        tokio::spawn(async move { a.serve_connection(guest).await })
    };
    write_frame(
        &mut host,
        &HostToRlm::Hello {
            api_version: API_VERSION,
            topic_id: "topic-a".into(),
            vm_id: "topic-a-x0001".into(),
        },
    )
    .await
    .expect("hello");
    let ready: RlmToHost = read_frame(&mut host).await.expect("ready");
    assert!(
        matches!(ready, RlmToHost::Ready { ref agent, .. } if agent.starts_with("proof-vm-guest-agent/"))
    );
    write_frame(
        &mut host,
        &HostToRlm::Run {
            job: Box::new(VmJob::Archive {
                topic_id: "topic-a".into(),
            }),
        },
    )
    .await
    .expect("job");
    let done: RlmToHost = read_frame(&mut host).await.expect("done");
    assert_eq!(
        done,
        RlmToHost::Done {
            output: VmJobOutput::Archived
        }
    );
    drop(host);
    server.await.expect("join").expect("clean eof");
    assert_eq!(env::PARAM_PREFIX, "PROOF_PARAM_");
    assert_eq!(crate::runner::JobKind::Evaluate.entrypoint(), "run");
    let _ = std::fs::remove_dir_all(&r);
}

/// After `handle(Run)` the `Done` write hits a broken pipe: the session is
/// still Ok. The guest does not open a new host vsock (the host never
/// listens on `v.sock_5000`); host harvest reads `report.json`.
#[tokio::test]
async fn a_broken_pipe_on_done_is_ok_host_harvest_recovers() {
    let r = root("retry");
    let a = agent(&r);
    hello(&a).await;
    let (mut host, guest) = tokio::io::duplex(1 << 20);
    let server = {
        let a = a.clone();
        tokio::spawn(async move { a.serve_connection(guest).await })
    };
    write_frame(
        &mut host,
        &HostToRlm::Run {
            job: Box::new(VmJob::Archive {
                topic_id: "topic-a".into(),
            }),
        },
    )
    .await
    .expect("job");
    drop(host);
    server
        .await
        .expect("join")
        .expect("ok; host harvest recovers");
    let _ = std::fs::remove_dir_all(&r);
}

/// The miner's own BYOK environment reaches their paid run: exported under
/// the name the signed topic declared, written to a 0600 file beside it, and
/// blanked out of everything the guest ships back. It is not part of the
/// adaptor contract's own `PROOF_…` namespace and it cannot rewrite it.
#[tokio::test]
async fn a_miner_byok_variable_reaches_the_paid_run_and_never_travels_back() {
    let r = root("byok");
    let a = agent(&r);
    hello(&a).await;
    let (tar, digest) = pack();
    stage(&a, &tar, &digest).await;
    install(
        &r,
        "run",
        r#"
: "${MINER_PROVIDED_API_KEY:?the miner key must be exported}"
from_file=$(cat "$PROOF_MINER_ENV_DIR/MINER_PROVIDED_API_KEY")
test "$from_file" = "$MINER_PROVIDED_API_KEY"
test "$PROOF_MINER_ENV_NAMES" = "MINER_PROVIDED_API_KEY"
echo "leaking $MINER_PROVIDED_API_KEY on stdout"
cat > "$PROOF_OUTPUT_DIR/report.json" <<EOF
{"primary_value": 0.5, "flops_used": 1, "evidence": {"note": "called with $MINER_PROVIDED_API_KEY"}}
EOF
"#,
    );
    let mut req = req_for(&digest);
    let mut env = proof_rlm::MinerEnv::new();
    env.insert("MINER_PROVIDED_API_KEY", MINER_KEY);
    req.miner_env = env;
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline {
                request: req.clone(),
            }),
        })
        .await;
    let RlmToHost::Done {
        output: VmJobOutput::Baseline(report),
    } = out
    else {
        panic!("expected a baseline report, got {out:?}");
    };
    report.verify(&req).expect("bound to the request");
    assert_eq!(
        report.evidence["note"],
        serde_json::json!("called with [REDACTED]"),
        "the miner's key is blanked in evidence like any other secret"
    );
    let dump = serde_json::to_string(&report).expect("json");
    assert!(!dump.contains(MINER_KEY), "{dump}");

    // The file the adaptor read is private, and it is not one of the owner
    // key files the host staged at boot.
    let file = r
        .join("secrets")
        .join(crate::staging::MINER_ENV_SUBDIR)
        .join("MINER_PROVIDED_API_KEY");
    let mode = std::fs::metadata(&file).expect("file").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(
        std::fs::read_to_string(&file).expect("value"),
        MINER_KEY,
        "written verbatim for an adaptor that would rather read a file"
    );
    assert_eq!(
        crate::staging::secret_names(&r.join("secrets")),
        vec!["inference_key"],
        "PROOF_SECRET_FILES stays the owner's list"
    );

    // Evaluate must stage the same way: PROOF_MINER_ENV_DIR is set and the
    // file is there. A missing directory variable is not a fail-closed.
    req.constraints.params.insert(
        proof_canon::PARAM_MINER_BYOK.into(),
        "MINER_PROVIDED_API_KEY".into(),
    );
    let artefact = archive(&[member("recipe/run.sh", b'0', b"echo hi\n")]);
    req.artifact_digest = hex::encode(Sha256::digest(&artefact));
    req.artifact_uri = Some(serve_once(artefact).await);
    install(
        &r,
        "run",
        r#"
: "${PROOF_MINER_ENV_DIR:?evaluate must stage a miner env dir}"
: "${MINER_PROVIDED_API_KEY:?the miner key must be exported}"
from_file=$(cat "$PROOF_MINER_ENV_DIR/MINER_PROVIDED_API_KEY")
test "$from_file" = "$MINER_PROVIDED_API_KEY"
echo '{"primary_value": 0.25, "flops_used": 1}' > "$PROOF_OUTPUT_DIR/report.json"
"#,
    );
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Evaluate {
                request: req.clone(),
                checklist_digest: "c".into(),
                rules_version: 1,
            }),
        })
        .await;
    let RlmToHost::Done {
        output: VmJobOutput::Evaluated(run),
    } = out
    else {
        panic!("expected an evaluated run, got {out:?}");
    };
    run.report.verify(&req).expect("bound to the request");
    assert_eq!(run.report.flops_used, Some(1));
    let _ = std::fs::remove_dir_all(&r);
}

/// Fail-closed at the guest boundary too: a variable that would shadow the
/// adaptor contract or the base environment refuses the job before the
/// adaptor is spawned, whatever the control plane accepted.
#[tokio::test]
async fn a_byok_variable_never_rewrites_a_guest_fact() {
    let r = root("byok-shadow");
    let a = agent(&r);
    hello(&a).await;
    let (tar, digest) = pack();
    stage(&a, &tar, &digest).await;
    install(
        &r,
        "run",
        "echo '{\"primary_value\": 1.0, \"flops_used\": 1}' > \"$PROOF_OUTPUT_DIR/report.json\"",
    );
    for name in ["PATH", "PROOF_JOB", "PROOF_SECRETS_DIR", "lower_case"] {
        let mut req = req_for(&digest);
        let mut env = proof_rlm::MinerEnv::new();
        env.insert(name, MINER_KEY);
        req.miner_env = env;
        let err = failed(
            a.handle(HostToRlm::Run {
                job: Box::new(VmJob::Baseline { request: req }),
            })
            .await,
        );
        assert!(err.contains(name), "{err}");
        assert!(!err.contains(MINER_KEY), "a refusal never quotes it: {err}");
    }
    // Inspection ticks rules without spending, so it is handed no key at all.
    let mut req = req_for(&digest);
    let mut env = proof_rlm::MinerEnv::new();
    env.insert("MINER_PROVIDED_API_KEY", MINER_KEY);
    req.miner_env = env;
    install(
        &r,
        "inspect",
        r#"
test -z "${MINER_PROVIDED_API_KEY:-}"
test -z "${PROOF_MINER_ENV_DIR:-}"
echo '[]' > "$PROOF_OUTPUT_DIR/checklist.json"
"#,
    );
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Inspect {
                request: req,
                rules: rules(),
            }),
        })
        .await;
    assert!(
        matches!(out, RlmToHost::Done { .. }),
        "inspection runs without the key: {out:?}"
    );
    assert!(
        !r.join("secrets")
            .join(crate::staging::MINER_ENV_SUBDIR)
            .exists(),
        "no key file is written for an unpaid job"
    );

    // Evaluate on a miner_byok topic still exposes PROOF_MINER_ENV_DIR
    // when the request carried no values, so the adaptor fails on a
    // missing file rather than an unset directory variable.
    let artefact = archive(&[member("recipe/run.sh", b'0', b"echo hi\n")]);
    let mut empty = req_for(&digest);
    empty.constraints.params.insert(
        proof_canon::PARAM_MINER_BYOK.into(),
        "MINER_PROVIDED_API_KEY".into(),
    );
    empty.artifact_digest = hex::encode(Sha256::digest(&artefact));
    empty.artifact_uri = Some(serve_once(artefact).await);
    install(
        &r,
        "run",
        r#"
: "${PROOF_MINER_ENV_DIR:?evaluate miner_byok must set the dir even with empty miner_env}"
test ! -r "$PROOF_MINER_ENV_DIR/MINER_PROVIDED_API_KEY"
echo '{"primary_value": 0.1, "flops_used": 1}' > "$PROOF_OUTPUT_DIR/report.json"
"#,
    );
    let out = a
        .handle(HostToRlm::Run {
            job: Box::new(VmJob::Evaluate {
                request: empty,
                checklist_digest: "c".into(),
                rules_version: 1,
            }),
        })
        .await;
    assert!(
        matches!(out, RlmToHost::Done { .. }),
        "evaluate still runs with an empty miner env dir: {out:?}"
    );
    let _ = std::fs::remove_dir_all(&r);
}

/// A paid run must not answer `Done` when the pre-Done durability barrier
/// cannot open/sync the work tree (report.json sitting in page cache is not
/// enough).
#[tokio::test]
async fn paid_run_fails_closed_when_work_tree_cannot_be_synced() {
    let r = root("sync-fail");
    let a = agent(&r);
    hello(&a).await;
    let (tar, digest) = pack();
    stage(&a, &tar, &digest).await;
    install(
        &r,
        "run",
        r#"
echo '{"primary_value": 0.73}' > "$PROOF_OUTPUT_DIR/report.json"
touch "$PROOF_WORK_DIR/blocked"
chmod 000 "$PROOF_WORK_DIR/blocked"
"#,
    );
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline {
                request: req_for(&digest),
            }),
        })
        .await,
    );
    assert!(
        err.contains("sync"),
        "durability failure must prevent Done, got {err}"
    );
    let artefact = archive(&[member("recipe/run.sh", b'0', b"echo hi\n")]);
    let mut eval = req_for(&digest);
    eval.artifact_digest = hex::encode(Sha256::digest(&artefact));
    eval.artifact_uri = Some(serve_once(artefact).await);
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Evaluate {
                request: eval,
                checklist_digest: "c".into(),
                rules_version: 1,
            }),
        })
        .await,
    );
    assert!(
        err.contains("sync"),
        "evaluate must also refuse Done after a failed flush, got {err}"
    );
    let _ = std::fs::remove_dir_all(&r);
}

/// A deadline cut must still flush work/ before Failed. Metal tbench-x0004
/// retain looked empty until e2fsck replayed the journal; a chmod-000
/// file makes that persist fail closed instead of a timeout with nothing
/// on disk.
#[tokio::test]
async fn deadline_cut_still_persists_work_tree() {
    let r = root("sync-timeout");
    let a = agent(&r);
    hello(&a).await;
    let (tar, digest) = pack();
    stage(&a, &tar, &digest).await;
    install(
        &r,
        "run",
        r#"
echo harbor > "$PROOF_WORK_DIR/harbor.run.log"
touch "$PROOF_WORK_DIR/blocked"
chmod 000 "$PROOF_WORK_DIR/blocked"
sleep 30
"#,
    );
    let mut short = req_for(&digest);
    short.sandbox.deadline_s = 6;
    let err = failed(
        a.handle(HostToRlm::Run {
            job: Box::new(VmJob::Baseline { request: short }),
        })
        .await,
    );
    assert!(
        err.contains("sync"),
        "timeout must persist work before Failed, got {err}"
    );
    let _ = std::fs::remove_dir_all(&r);
}
