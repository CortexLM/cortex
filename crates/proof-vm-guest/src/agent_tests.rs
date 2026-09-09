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
