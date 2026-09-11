//! Reconstruction tests: a planted `work/*/output/report.json` becomes
//! `Evaluated` / `Baseline` with the adaptor's `primary_value`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use proof_rlm::fixtures::{experiment_request, request};
use proof_rlm::{VmJob, VmJobOutput};
use proof_vm_agent::HvError;
use proof_vm_proto::guest::{encode_frame, read_frame, RlmToHost};
use tokio::io::AsyncWriteExt;

use super::{
    from_work_tree, harvest_from_jail, recv_job_or_harvest, try_from_jail, HARBOR_JOBS,
    HARVEST_WORK, POLL_INTERVAL, SCRATCH_TREE,
};

fn tree(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("proof-fc-harvest-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("dir");
    d
}

fn plant(work: &Path, kind: &str, seq: &str, body: &str) {
    let dir = work.join(format!("{seq}-{kind}")).join("output");
    std::fs::create_dir_all(&dir).expect("output");
    std::fs::write(dir.join("report.json"), body).expect("report");
}

fn plant_trial(work: &Path, seq_kind: &str, job: &str, trial: &str, reward: &str) {
    let dir = work
        .join(seq_kind)
        .join(HARBOR_JOBS)
        .join(job)
        .join(trial)
        .join("verifier");
    std::fs::create_dir_all(&dir).expect("trial");
    let trial_dir = dir.parent().expect("trial dir");
    std::fs::write(
        trial_dir.join("result.json"),
        format!(
            r#"{{"trial_name":"{trial}","verifier_result":{{"rewards":{{"reward":{reward}}}}}}}"#
        ),
    )
    .expect("result.json");
    std::fs::write(dir.join("reward.txt"), reward).expect("reward.txt");
}

/// Guest report that claims Harbor finished (n15 shape: 6/6, atrx at 0).
fn harbor_report_n2(primary: &str) -> String {
    format!(
        r#"{{"primary_value": {primary}, "claim_holds": false, "evidence": {{"n_measured": 2, "harbor_exit": 0, "trials": [{{"name": "task-hard", "reward": 0.0}}, {{"name": "atrx", "reward": 0.0}}]}}}}"#
    )
}

#[test]
fn harvest_on_vsock_fail_returns_evaluated_with_report_primary_value() {
    let root = tree("eval");
    let work = root.join("work");
    plant(
        &work,
        "evaluate",
        "0001",
        r#"{"primary_value": 0.73, "claim_holds": true, "flops_used": 42, "evidence": {"harbor_exit": 0}}"#,
    );
    let job = VmJob::Evaluate {
        request: experiment_request(None),
        checklist_digest: "c".into(),
        rules_version: 1,
    };
    let out = from_work_tree(&work, &root, &job).expect("harvest");
    let VmJobOutput::Evaluated(run) = out else {
        panic!("expected Evaluated, got {out:?}");
    };
    assert!((run.report.primary_value - 0.73).abs() < 1e-12);
    assert_eq!(run.report.flops_used, Some(42));
    assert!(run.report.sandboxed);
    assert!(run.report.claim_holds);
    assert_eq!(
        run.report
            .evidence
            .get("host_harvest")
            .and_then(|v| v.as_str()),
        Some("vsock_done_drop")
    );
    run.report.verify(&experiment_request(None)).expect("bound");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_baseline_report_becomes_baseline_and_an_older_dir_is_ignored() {
    let root = tree("base");
    let work = root.join("work");
    plant(&work, "baseline", "0001", r#"{"primary_value": 0.1}"#);
    plant(
        &work,
        "baseline",
        "0002",
        r#"{"primary_value": 0.9, "claim_holds": true}"#,
    );
    let job = VmJob::Baseline { request: request() };
    let out = from_work_tree(&work, &root, &job).expect("harvest");
    let VmJobOutput::Baseline(report) = out else {
        panic!("expected Baseline, got {out:?}");
    };
    assert!((report.primary_value - 0.9).abs() < 1e-12);
    assert!(from_work_tree(
        &work,
        &root,
        &VmJob::Archive {
            topic_id: "t".into()
        }
    )
    .is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_missing_or_non_finite_report_does_not_invent_a_score() {
    let root = tree("miss");
    let work = root.join("work");
    std::fs::create_dir_all(work.join("0001-evaluate").join("output")).expect("dir");
    let job = VmJob::Evaluate {
        request: request(),
        checklist_digest: "c".into(),
        rules_version: 1,
    };
    assert!(from_work_tree(&work, &root, &job).is_err());
    plant(&work, "evaluate", "0001", r#"{"primary_value": "NaN"}"#);
    assert!(from_work_tree(&work, &root, &job).is_err());
    plant(&work, "evaluate", "0001", r#"{"primary_value": 1e999}"#);
    assert!(from_work_tree(&work, &root, &job).is_err());
    assert!(try_from_jail(&root, &root, &job).is_none());
    let overlay = root.join(SCRATCH_TREE).join("work");
    plant(&overlay, "evaluate", "0001", r#"{"primary_value": 0.5}"#);
    let got = try_from_jail(&root, &root, &job).expect("overlay");
    let VmJobOutput::Evaluated(run) = got else {
        panic!("{got:?}");
    };
    assert!((run.report.primary_value - 0.5).abs() < 1e-12);
    let _ = std::fs::remove_dir_all(&root);
}

fn evaluate_job() -> VmJob {
    VmJob::Evaluate {
        request: experiment_request(None),
        checklist_digest: "c".into(),
        rules_version: 1,
    }
}

fn archived() -> RlmToHost {
    RlmToHost::Done {
        output: VmJobOutput::Archived,
    }
}

/// Recreating `recv` on each 2s poll would restart this sleep and miss the
/// deadline. One pinned future must still deliver `Done`.
#[tokio::test(start_paused = true)]
async fn a_slow_vsock_done_is_not_restarted_on_scratch_poll() {
    let root = tree("slow");
    let job = VmJob::Archive {
        topic_id: "t".into(),
    };
    let got = recv_job_or_harvest(
        async {
            tokio::time::sleep(POLL_INTERVAL + Duration::from_secs(1)).await;
            Ok(archived())
        },
        &root,
        &root,
        &job,
        Duration::from_secs(8),
    )
    .await
    .expect("done");
    assert_eq!(got, archived());
    let _ = std::fs::remove_dir_all(&root);
}

/// Half a length prefix sitting in the stream must not be thrown away when
/// scratch is polled — dropping `read_exact` and starting again mis-parses
/// the next four bytes as a length.
#[tokio::test(start_paused = true)]
async fn a_partial_vsock_frame_survives_scratch_poll_timeouts() {
    let root = tree("partial");
    let job = VmJob::Archive {
        topic_id: "t".into(),
    };
    let frame = encode_frame(&archived()).expect("frame");
    assert!(frame.len() > 4, "need a split past the length prefix");
    let (mut tx, mut rx) = tokio::io::duplex(4096);
    let writer = tokio::spawn(async move {
        tx.write_all(&frame[..2]).await.expect("prefix");
        tx.flush().await.expect("flush");
        tokio::time::sleep(POLL_INTERVAL + Duration::from_millis(50)).await;
        tx.write_all(&frame[2..]).await.expect("rest");
    });
    let got = recv_job_or_harvest(
        async move {
            read_frame(&mut rx)
                .await
                .map_err(|e| HvError::Guest(e.to_string()))
        },
        &root,
        &root,
        &job,
        Duration::from_secs(30),
    )
    .await
    .expect("frame");
    assert_eq!(got, archived());
    writer.await.expect("writer");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(start_paused = true)]
async fn a_hanging_vsock_harvests_after_done_grace() {
    let root = tree("hang");
    let overlay = root.join(SCRATCH_TREE).join("work");
    plant(
        &overlay,
        "evaluate",
        "0001",
        r#"{"primary_value": 0.73, "claim_holds": true}"#,
    );
    let got = recv_job_or_harvest(
        std::future::pending::<Result<RlmToHost, HvError>>(),
        &root,
        &root,
        &evaluate_job(),
        Duration::from_secs(30),
    )
    .await
    .expect("harvest");
    let RlmToHost::Done {
        output: VmJobOutput::Evaluated(run),
    } = got
    else {
        panic!("expected Evaluated, got {got:?}");
    };
    assert!((run.report.primary_value - 0.73).abs() < 1e-12);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_dead_vsock_harvests_immediately_when_report_is_on_disk() {
    let root = tree("dead");
    let overlay = root.join(SCRATCH_TREE).join("work");
    plant(
        &overlay,
        "evaluate",
        "0001",
        r#"{"primary_value": 0.73, "claim_holds": true}"#,
    );
    let got = recv_job_or_harvest(
        async { Err(HvError::Guest("broken pipe".into())) },
        &root,
        &root,
        &evaluate_job(),
        Duration::from_secs(30),
    )
    .await
    .expect("harvest");
    let RlmToHost::Done {
        output: VmJobOutput::Evaluated(run),
    } = got
    else {
        panic!("expected Evaluated, got {got:?}");
    };
    assert!((run.report.primary_value - 0.73).abs() < 1e-12);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn harvest_work_lagging_guest_trial_files_is_refused() {
    let root = tree("lag");
    let overlay = root.join(SCRATCH_TREE).join("work");
    let dump = root.join(HARVEST_WORK);
    plant(&overlay, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&overlay, "0001-evaluate", "run", "task-hard", "0");
    plant_trial(&overlay, "0001-evaluate", "run", "atrx", "0");
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    let atrx = dump
        .join("0001-evaluate")
        .join(HARBOR_JOBS)
        .join("run")
        .join("atrx")
        .join("verifier");
    std::fs::create_dir_all(&atrx).expect("empty atrx verifier");
    let err = harvest_from_jail(&root, &root, &evaluate_job()).expect_err("incomplete dump");
    let msg = err.to_string();
    assert!(
        msg.contains("harvest-work incomplete"),
        "expected fail-closed, got {msg}"
    );
    assert!(try_from_jail(&root, &root, &evaluate_job()).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn complete_overlay_without_a_dump_still_harvests() {
    let root = tree("overlay-only");
    let overlay = root.join(SCRATCH_TREE).join("work");
    plant(&overlay, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&overlay, "0001-evaluate", "run", "task-hard", "0");
    plant_trial(&overlay, "0001-evaluate", "run", "atrx", "0");
    let out = harvest_from_jail(&root, &root, &evaluate_job()).expect("overlay harvest");
    let VmJobOutput::Evaluated(run) = out else {
        panic!("{out:?}");
    };
    assert!((run.report.primary_value - 0.0).abs() < 1e-12);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dump_only_missing_trial_reward_txt_is_refused() {
    let root = tree("dump-miss");
    let dump = root.join(HARVEST_WORK);
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    let atrx = dump
        .join("0001-evaluate")
        .join(HARBOR_JOBS)
        .join("run")
        .join("atrx");
    std::fs::create_dir_all(atrx.join("verifier")).expect("empty atrx");
    std::fs::write(atrx.join("result.json"), r#"{"trial_name":"atrx"}"#).expect("result");
    let err = from_work_tree(&dump, &root, &evaluate_job()).expect_err("incomplete dump");
    let msg = err.to_string();
    assert!(
        msg.contains("harvest-work incomplete"),
        "expected fail-closed, got {msg}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dump_only_complete_trials_matching_n_measured_harvests() {
    let root = tree("dump-ok");
    let dump = root.join(HARVEST_WORK);
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    plant_trial(&dump, "0001-evaluate", "run", "atrx", "0");
    let out = from_work_tree(&dump, &root, &evaluate_job()).expect("complete dump");
    let VmJobOutput::Evaluated(run) = out else {
        panic!("{out:?}");
    };
    assert!((run.report.primary_value - 0.0).abs() < 1e-12);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn vsock_drop_does_not_publish_when_harvest_work_lags_guest() {
    let root = tree("nosync");
    let overlay = root.join(SCRATCH_TREE).join("work");
    let dump = root.join(HARVEST_WORK);
    plant(&overlay, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&overlay, "0001-evaluate", "run", "task-hard", "0");
    plant_trial(&overlay, "0001-evaluate", "run", "atrx", "0");
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    std::fs::create_dir_all(
        dump.join("0001-evaluate")
            .join(HARBOR_JOBS)
            .join("run")
            .join("atrx")
            .join("verifier"),
    )
    .expect("empty atrx verifier");
    let err = recv_job_or_harvest(
        async { Err(HvError::Guest("broken pipe".into())) },
        &root,
        &root,
        &evaluate_job(),
        Duration::from_secs(30),
    )
    .await
    .expect_err("must not publish");
    let msg = err.to_string();
    assert!(
        msg.contains("harvest-work incomplete"),
        "expected harvest fail-closed, got {msg}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
