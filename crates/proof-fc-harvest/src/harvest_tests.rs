//! Reconstruction tests: a planted `work/*/output/report.json` becomes
//! `Evaluated` / `Baseline` with the adaptor's `primary_value`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use proof_rlm::fixtures::{experiment_request, request};
use proof_rlm::{CustomRunReport, RunOutcome, VmJob, VmJobOutput, RUN_REPORT_SCHEMA};
use proof_vm_agent::HvError;
use proof_vm_proto::guest::{encode_frame, read_frame, RlmToHost};
use tokio::io::AsyncWriteExt;

use super::{
    copy_regular_nofollow, copy_tree, from_work_tree, harvest_from_jail, recv_job_or_harvest,
    try_from_jail, HARBOR_JOBS, HARVEST_WORK, POLL_INTERVAL, SCRATCH_IN_JAIL, SCRATCH_TREE,
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

fn plant_job_snapshot(work: &Path, seq_kind: &str, job: &str, n_running: u64, finished_at: &str) {
    let p = work
        .join(seq_kind)
        .join(HARBOR_JOBS)
        .join(job)
        .join("result.json");
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("job");
    }
    let finished = if finished_at == "null" {
        "null".to_owned()
    } else {
        format!("\"{finished_at}\"")
    };
    std::fs::write(
        p,
        format!(
            r#"{{"n_running":{n_running},"n_completed":{},"finished_at":{finished}}}"#,
            if n_running == 0 { 6 } else { 5 }
        ),
    )
    .expect("job result");
}

fn plant_job_snapshot_stats(
    work: &Path,
    seq_kind: &str,
    job: &str,
    n_running_trials: u64,
    finished_at: &str,
) {
    let p = work
        .join(seq_kind)
        .join(HARBOR_JOBS)
        .join(job)
        .join("result.json");
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("job");
    }
    let finished = if finished_at == "null" {
        "null".to_owned()
    } else {
        format!("\"{finished_at}\"")
    };
    std::fs::write(
        p,
        format!(
            r#"{{"stats":{{"n_running_trials":{n_running_trials},"n_completed":{}}},"finished_at":{finished}}}"#,
            if n_running_trials == 0 { 6 } else { 5 }
        ),
    )
    .expect("job stats snapshot");
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

fn evaluate_job_other() -> VmJob {
    let mut request = experiment_request(None);
    request.submission_digest = "digest-b".into();
    request.artifact_digest = "cd".repeat(32);
    VmJob::Evaluate {
        request,
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
fn harvest_work_lagging_guest_trial_files_is_refreshed_from_overlay() {
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
    let out = harvest_from_jail(&root, &root, &evaluate_job()).expect("overlay refresh");
    let VmJobOutput::Evaluated(run) = out else {
        panic!("{out:?}");
    };
    assert!((run.report.primary_value - 0.0).abs() < 1e-12);
    let refreshed = dump
        .join("0001-evaluate")
        .join(HARBOR_JOBS)
        .join("run")
        .join("atrx")
        .join("result.json");
    assert!(
        refreshed.is_file() && std::fs::metadata(&refreshed).expect("meta").len() > 0,
        "stale dump must be replaced from overlay"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Adaptor-controlled `child/ancestor-link -> ..` must not recurse until
/// ELOOP and fail a completed overlay harvest.
#[test]
fn cyclic_dir_symlink_does_not_block_harvest_of_complete_overlay() {
    let root = tree("cycle");
    let overlay = root.join(SCRATCH_TREE).join("work");
    plant(&overlay, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&overlay, "0001-evaluate", "run", "task-hard", "0");
    plant_trial(&overlay, "0001-evaluate", "run", "atrx", "0");
    let child = overlay.join("child");
    std::fs::create_dir_all(&child).expect("child");
    std::os::unix::fs::symlink("..", child.join("ancestor-link")).expect("cycle");
    let dest = root.join("copy-dest");
    let started = std::time::Instant::now();
    copy_tree(&overlay, &dest).expect("copy must not follow the dir symlink");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "copy_tree must not walk a dir-symlink cycle"
    );
    assert!(
        dest.join("0001-evaluate")
            .join("output")
            .join("report.json")
            .is_file(),
        "complete tree must still be copied"
    );
    assert!(
        !dest.join("child").join("ancestor-link").exists(),
        "directory symlink must not be followed or recreated as a directory"
    );
    let out = harvest_from_jail(&root, &root, &evaluate_job()).expect("overlay harvest");
    let VmJobOutput::Evaluated(run) = out else {
        panic!("{out:?}");
    };
    assert!((run.report.primary_value - 0.0).abs() < 1e-12);
    let _ = std::fs::remove_dir_all(&root);
}

/// Adaptor-controlled regular-file symlink (`leak ->` a host secret) must
/// not be followed: `std::fs::copy` would otherwise copy target bytes into
/// `{jail}/harvest-work`. Skip the link; never dereference it.
#[test]
fn regular_file_symlink_is_not_followed_during_harvest_copy() {
    let root = tree("file-link");
    let overlay = root.join(SCRATCH_TREE).join("work");
    plant(&overlay, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&overlay, "0001-evaluate", "run", "task-hard", "0");
    plant_trial(&overlay, "0001-evaluate", "run", "atrx", "0");
    let marker = b"HARVEST-MUST-NOT-COPY-THIS-SECRET";
    let secret = root.join("host-secret");
    std::fs::write(&secret, marker).expect("secret");
    std::os::unix::fs::symlink(&secret, overlay.join("leak")).expect("file symlink");
    let outside = root.join("outside-secret");
    std::fs::write(&outside, marker).expect("relative secret");
    std::os::unix::fs::symlink("../outside-secret", overlay.join("escape")).expect("rel link");
    let dest = root.join("copy-dest");
    copy_tree(&overlay, &dest).expect("copy must skip file symlinks");
    assert!(
        dest.join("0001-evaluate")
            .join("output")
            .join("report.json")
            .is_file(),
        "complete tree must still be copied"
    );
    assert!(
        std::fs::symlink_metadata(dest.join("leak")).is_err(),
        "file symlink must not be copied or followed"
    );
    assert!(
        std::fs::symlink_metadata(dest.join("escape")).is_err(),
        "relative file symlink must not be copied or followed"
    );
    assert!(
        !tree_contains_bytes(&dest, marker),
        "harvest dest must not contain the symlink target bytes"
    );
    let out = harvest_from_jail(&root, &root, &evaluate_job()).expect("overlay harvest");
    let VmJobOutput::Evaluated(run) = out else {
        panic!("{out:?}");
    };
    assert!((run.report.primary_value - 0.0).abs() < 1e-12);
    let harvest = root.join(HARVEST_WORK);
    assert!(
        std::fs::symlink_metadata(harvest.join("leak")).is_err(),
        "harvest-work must not materialize the leak link"
    );
    assert!(
        !tree_contains_bytes(&harvest, marker),
        "harvest-work must not contain the symlink target bytes"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `O_NOFOLLOW` open path: a symlink at open time must not be followed,
/// even when the caller skipped a prior `symlink_metadata` check (TOCTOU).
/// Fail-closed: error, dest must not contain the target bytes.
#[test]
fn copy_regular_nofollow_does_not_follow_symlink_at_open() {
    let root = tree("nofollow-open");
    let marker = b"HARVEST-MUST-NOT-COPY-THIS-SECRET";
    let secret = root.join("host-secret");
    std::fs::write(&secret, marker).expect("secret");
    let src = root.join("entry");
    std::os::unix::fs::symlink(&secret, &src).expect("symlink");
    let dest = root.join("out");
    let err = copy_regular_nofollow(&src, &dest).expect_err("must not follow symlink");
    assert!(
        err.contains("symlink") || err.contains("ELOOP"),
        "expected ELOOP/symlink refuse, got {err}"
    );
    assert!(
        !dest.exists() || std::fs::read(&dest).is_ok_and(|b| b != marker),
        "dest must not contain the symlink target bytes"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn copy_regular_nofollow_copies_a_regular_file() {
    let root = tree("nofollow-reg");
    let src = root.join("entry");
    std::fs::write(&src, b"guest-bytes").expect("src");
    let dest = root.join("out");
    copy_regular_nofollow(&src, &dest).expect("regular file");
    assert_eq!(std::fs::read(&dest).expect("dest"), b"guest-bytes");
    let _ = std::fs::remove_dir_all(&root);
}

/// FIFO at open must fail closed without blocking (`O_NONBLOCK` + `fstat`).
#[test]
fn copy_regular_nofollow_rejects_fifo_without_blocking() {
    let root = tree("nofollow-fifo");
    let fifo = root.join("pipe");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo");
    assert!(status.success(), "mkfifo");
    let dest = root.join("out");
    let started = std::time::Instant::now();
    let err = copy_regular_nofollow(&fifo, &dest).expect_err("fifo");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "must not block on a FIFO with no writer"
    );
    assert!(
        err.contains("regular file") || err.contains("open"),
        "expected special-file refuse, got {err}"
    );
    assert!(!dest.exists(), "FIFO must not create dest");
    let _ = std::fs::remove_dir_all(&root);
}

/// Models the check-then-use race as tightly as practical: a swapper thread
/// replaces a regular file with a symlink to a host secret while copies run.
/// `O_NOFOLLOW` must never land the secret bytes in dest (fail-closed on
/// `ELOOP` is OK; following the link is not).
#[test]
fn copy_regular_nofollow_toctou_swap_cannot_leak_host_bytes() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let root = tree("nofollow-toctou");
    let marker = b"HARVEST-MUST-NOT-COPY-THIS-SECRET";
    let secret = root.join("host-secret");
    std::fs::write(&secret, marker).expect("secret");
    let src = root.join("entry");
    std::fs::write(&src, b"benign").expect("regular");
    let dest_dir = root.join("outs");
    std::fs::create_dir_all(&dest_dir).expect("outs");

    let stop = Arc::new(AtomicBool::new(false));
    let swapper = {
        let stop = Arc::clone(&stop);
        let src = src.clone();
        let secret = secret.clone();
        std::thread::spawn(move || {
            let mut i = 0u32;
            while !stop.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(&src);
                if i.is_multiple_of(2) {
                    let _ = std::fs::write(&src, b"benign");
                } else {
                    let _ = std::os::unix::fs::symlink(&secret, &src);
                }
                i = i.wrapping_add(1);
            }
        })
    };

    let mut leaked = false;
    for i in 0..1500u32 {
        let dest = dest_dir.join(format!("{i}"));
        match copy_regular_nofollow(&src, &dest) {
            Ok(()) => {
                if std::fs::read(&dest).is_ok_and(|b| b.windows(marker.len()).any(|w| w == marker))
                {
                    leaked = true;
                    break;
                }
            }
            Err(_) => {
                if dest.exists()
                    && std::fs::read(&dest)
                        .is_ok_and(|b| b.windows(marker.len()).any(|w| w == marker))
                {
                    leaked = true;
                    break;
                }
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    let _ = swapper.join();
    assert!(
        !leaked,
        "O_NOFOLLOW copy must not follow a TOCTOU symlink swap into host-readable bytes"
    );
    let _ = std::fs::remove_dir_all(&root);
}

fn tree_contains_bytes(dir: &Path, needle: &[u8]) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for e in entries.flatten() {
        let p = e.path();
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if tree_contains_bytes(&p, needle) {
                return true;
            }
        } else if std::fs::read(&p).is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle)) {
            return true;
        }
    }
    false
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
async fn vsock_drop_refreshes_dump_from_overlay_when_guest_finished() {
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
    let got = recv_job_or_harvest(
        async { Err(HvError::Guest("broken pipe".into())) },
        &root,
        &root,
        &evaluate_job(),
        Duration::from_secs(30),
    )
    .await
    .expect("overlay refresh");
    let RlmToHost::Done {
        output: VmJobOutput::Evaluated(run),
    } = got
    else {
        panic!("expected Evaluated, got {got:?}");
    };
    assert!((run.report.primary_value - 0.0).abs() < 1e-12);
    assert!(
        dump.join("0001-evaluate")
            .join(HARBOR_JOBS)
            .join("run")
            .join("atrx")
            .join("result.json")
            .is_file(),
        "stale dump must be refreshed from overlay"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dump_only_stale_n_running_is_refused_even_with_trial_files() {
    let root = tree("nrun");
    let dump = root.join(HARVEST_WORK);
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(
        &dump,
        "0001-evaluate",
        "run",
        "atrx-vep-crispr__Xs5jyyz",
        "0",
    );
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    plant_job_snapshot(&dump, "0001-evaluate", "run", 1, "null");
    let err = from_work_tree(&dump, &root, &evaluate_job()).expect_err("stale snapshot");
    let msg = err.to_string();
    assert!(
        msg.contains("harvest-work incomplete"),
        "expected fail-closed, got {msg}"
    );
    assert!(
        msg.contains("n_running"),
        "expected n_running in refuse, got {msg}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dump_only_finished_snapshot_with_measured_rewards_harvests() {
    let root = tree("nfin");
    let dump = root.join(HARVEST_WORK);
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(
        &dump,
        "0001-evaluate",
        "run",
        "atrx-vep-crispr__Xs5jyyz",
        "0",
    );
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    plant_job_snapshot(&dump, "0001-evaluate", "run", 0, "2026-09-11T00:00:00Z");
    let out = from_work_tree(&dump, &root, &evaluate_job()).expect("finished snapshot");
    let VmJobOutput::Evaluated(run) = out else {
        panic!("{out:?}");
    };
    assert!((run.report.primary_value - 0.0).abs() < 1e-12);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn overlay_finished_dump_still_n_running_is_refreshed() {
    let root = tree("race");
    let overlay = root.join(SCRATCH_TREE).join("work");
    let dump = root.join(HARVEST_WORK);
    plant(&overlay, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(
        &overlay,
        "0001-evaluate",
        "run",
        "atrx-vep-crispr__Xs5jyyz",
        "0",
    );
    plant_trial(&overlay, "0001-evaluate", "run", "task-hard", "0");
    plant_job_snapshot(&overlay, "0001-evaluate", "run", 0, "2026-09-11T00:00:00Z");
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(
        &dump,
        "0001-evaluate",
        "run",
        "atrx-vep-crispr__Xs5jyyz",
        "0",
    );
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    plant_job_snapshot(&dump, "0001-evaluate", "run", 1, "null");
    let out = harvest_from_jail(&root, &root, &evaluate_job()).expect("refresh dump");
    let VmJobOutput::Evaluated(run) = out else {
        panic!("{out:?}");
    };
    assert!((run.report.primary_value - 0.0).abs() < 1e-12);
    let snap = std::fs::read_to_string(
        dump.join("0001-evaluate")
            .join(HARBOR_JOBS)
            .join("run")
            .join("result.json"),
    )
    .expect("refreshed snapshot");
    assert!(
        snap.contains("\"n_running\":0") || snap.contains("\"n_running_trials\":0"),
        "dump must not stay n_running=1, got {snap}"
    );
    assert!(
        !snap.contains("\"finished_at\":null"),
        "dump must not stay finished_at=null, got {snap}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn leftover_harvest_work_is_not_reused_for_a_later_job() {
    let root = tree("reuse");
    let dump = root.join(HARVEST_WORK);
    plant(
        &dump,
        "evaluate",
        "0001",
        r#"{"primary_value": 0.73, "claim_holds": true, "evidence": {"harbor_exit": 0}}"#,
    );
    assert!(
        harvest_from_jail(&root, &root, &evaluate_job_other()).is_err(),
        "leftover dump without overlay/image must not score job B"
    );
    assert!(try_from_jail(&root, &root, &evaluate_job_other()).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

fn plant_empty_atrx_trial(work: &Path) {
    let trial = work
        .join("0001-evaluate")
        .join(HARBOR_JOBS)
        .join("run")
        .join("atrx");
    std::fs::create_dir_all(trial.join("verifier")).expect("verifier");
    std::fs::write(trial.join("result.json"), "").expect("empty result");
    std::fs::write(trial.join("verifier").join("reward.txt"), "").expect("empty reward");
}

fn assert_incomplete_not_scored(err: &HvError, primary: &str) {
    let msg = err.to_string();
    assert!(
        msg.contains("harvest-work incomplete"),
        "expected fail-closed, got {msg}"
    );
    assert!(
        !msg.contains(primary),
        "must not publish the report score, got {msg}"
    );
}

#[test]
fn dump_only_empty_trial_files_are_refused() {
    let root = tree("empty");
    let dump = root.join(HARVEST_WORK);
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.73"));
    plant_empty_atrx_trial(&dump);
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    let err = from_work_tree(&dump, &root, &evaluate_job()).expect_err("empty files");
    assert_incomplete_not_scored(&err, "0.73");
    let _ = std::fs::remove_dir_all(&root);
}

/// Truncated `result.json` is not a finite `verifier_result.rewards.reward`.
#[test]
fn dump_only_truncated_result_json_is_refused() {
    let root = tree("trunc");
    let dump = root.join(HARVEST_WORK);
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.73"));
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    let trial = dump
        .join("0001-evaluate")
        .join(HARBOR_JOBS)
        .join("run")
        .join("atrx");
    std::fs::create_dir_all(trial.join("verifier")).expect("verifier");
    std::fs::write(trial.join("result.json"), r#"{"trial_name":"atrx""#).expect("truncated");
    std::fs::write(trial.join("verifier").join("reward.txt"), "0").expect("reward");
    let err = from_work_tree(&dump, &root, &evaluate_job()).expect_err("truncated json");
    assert_incomplete_not_scored(&err, "0.73");
    let _ = std::fs::remove_dir_all(&root);
}

fn e2fs_bin(names: &[&str]) -> Option<PathBuf> {
    for n in names {
        let p = Path::new(n);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    None
}

fn write_scratch_ext4(jail_root: &Path, work_tree: &Path) {
    let mkfs = e2fs_bin(&["/usr/sbin/mkfs.ext4", "/sbin/mkfs.ext4"])
        .expect("mkfs.ext4 required for scratch.ext4 harvest tests");
    let stage = jail_root.join("mkfs-src");
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).expect("stage");
    let status = std::process::Command::new("cp")
        .arg("-a")
        .arg(work_tree)
        .arg(stage.join("work"))
        .status()
        .expect("cp");
    assert!(status.success(), "cp work tree for mkfs");
    let img = jail_root.join(SCRATCH_IN_JAIL);
    let trunc = std::process::Command::new("truncate")
        .args(["-s", "16M"])
        .arg(&img)
        .status()
        .expect("truncate");
    assert!(trunc.success(), "truncate scratch.ext4");
    let out = std::process::Command::new(&mkfs)
        .args(["-F", "-d"])
        .arg(&stage)
        .arg(&img)
        .output()
        .expect("mkfs.ext4");
    assert!(
        out.status.success(),
        "mkfs.ext4 -d failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&stage);
}

/// P1: dump-only recovery of a scratch image with zero-byte trial files
/// must refuse, not publish `report.json`'s `primary_value`.
#[test]
fn dump_from_scratch_empty_trial_files_are_refused() {
    let root = tree("empty-img");
    let src = root.join("src-work");
    plant(&src, "evaluate", "0001", &harbor_report_n2("0.73"));
    plant_empty_atrx_trial(&src);
    plant_trial(&src, "0001-evaluate", "run", "task-hard", "0");
    write_scratch_ext4(&root, &src);
    let err = harvest_from_jail(&root, &root, &evaluate_job()).expect_err("empty files");
    assert_incomplete_not_scored(&err, "0.73");
    assert!(try_from_jail(&root, &root, &evaluate_job()).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

/// P1: an early nonempty `harvest-work` must not hide a later completed
/// `scratch.ext4`. Recovery invalidates the retained dump and re-dumps.
#[test]
fn retained_harvest_work_is_refreshed_from_scratch_ext4() {
    let root = tree("refresh");
    let stale = root.join(HARVEST_WORK);
    plant(
        &stale,
        "evaluate",
        "0001",
        r#"{"primary_value": 0.11, "claim_holds": false, "evidence": {"harbor_exit": 0}}"#,
    );
    let src = root.join("src-work");
    plant(
        &src,
        "evaluate",
        "0001",
        r#"{"primary_value": 0.73, "claim_holds": true, "evidence": {"harbor_exit": 0}}"#,
    );
    write_scratch_ext4(&root, &src);
    let out = harvest_from_jail(&root, &root, &evaluate_job()).expect("fresh dump");
    let VmJobOutput::Evaluated(run) = out else {
        panic!("{out:?}");
    };
    assert!(
        (run.report.primary_value - 0.73).abs() < 1e-12,
        "stale harvest-work 0.11 must not win over scratch.ext4 0.73, got {}",
        run.report.primary_value
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// P1: first dump before `report.json` exists; a later harvest must see the
/// report now in the image instead of the retained incomplete dump.
#[test]
fn early_incomplete_dump_does_not_hide_later_scratch_report() {
    let root = tree("early");
    let src = root.join("src-work");
    std::fs::create_dir_all(src.join("placeholder")).expect("empty work");
    std::fs::write(src.join("placeholder").join("keep"), b"x").expect("keep");
    write_scratch_ext4(&root, &src);
    assert!(
        harvest_from_jail(&root, &root, &evaluate_job()).is_err(),
        "no report yet"
    );
    assert!(root.join(HARVEST_WORK).is_dir(), "early dump retained");
    let later = root.join("src-work-later");
    plant(
        &later,
        "evaluate",
        "0001",
        r#"{"primary_value": 0.73, "claim_holds": true, "evidence": {"harbor_exit": 0}}"#,
    );
    write_scratch_ext4(&root, &later);
    let out = harvest_from_jail(&root, &root, &evaluate_job()).expect("re-dump");
    let VmJobOutput::Evaluated(run) = out else {
        panic!("{out:?}");
    };
    assert!((run.report.primary_value - 0.73).abs() < 1e-12);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dump_only_matches_result_json_trial_name() {
    let root = tree("logical");
    let dump = root.join(HARVEST_WORK);
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&dump, "0001-evaluate", "run", "generated-id-9f", "0");
    let result = dump
        .join("0001-evaluate")
        .join(HARBOR_JOBS)
        .join("run")
        .join("generated-id-9f")
        .join("result.json");
    std::fs::write(
        result,
        r#"{"trial_name":"atrx","verifier_result":{"rewards":{"reward":0.0}}}"#,
    )
    .expect("logical name");
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    let out = from_work_tree(&dump, &root, &evaluate_job()).expect("logical name");
    let VmJobOutput::Evaluated(run) = out else {
        panic!("{out:?}");
    };
    assert!((run.report.primary_value - 0.0).abs() < 1e-12);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dump_only_reward_txt_without_result_json_is_refused() {
    let root = tree("reward-only");
    let dump = root.join(HARVEST_WORK);
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    let atrx = dump
        .join("0001-evaluate")
        .join(HARBOR_JOBS)
        .join("run")
        .join("atrx");
    std::fs::create_dir_all(atrx.join("verifier")).expect("atrx");
    std::fs::write(atrx.join("verifier").join("reward.txt"), "0").expect("reward");
    let err = from_work_tree(&dump, &root, &evaluate_job()).expect_err("missing result.json");
    let msg = err.to_string();
    assert!(
        msg.contains("harvest-work incomplete"),
        "expected fail-closed, got {msg}"
    );
    assert!(
        msg.contains("result.json") || msg.contains("n_measured"),
        "must name the missing trial result, got {msg}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dump_only_stats_n_running_trials_is_refused() {
    let root = tree("nrun-stats");
    let dump = root.join(HARVEST_WORK);
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&dump, "0001-evaluate", "run", "atrx", "0");
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    plant_job_snapshot_stats(&dump, "0001-evaluate", "run", 1, "null");
    let err = from_work_tree(&dump, &root, &evaluate_job()).expect_err("stale stats");
    let msg = err.to_string();
    assert!(
        msg.contains("harvest-work incomplete"),
        "expected fail-closed, got {msg}"
    );
    assert!(
        msg.contains("n_running"),
        "expected n_running in refuse, got {msg}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

fn vsock_done_n2() -> RlmToHost {
    let req = experiment_request(None);
    let mut evidence = BTreeMap::new();
    evidence.insert("n_measured".into(), serde_json::json!(2));
    evidence.insert("harbor_exit".into(), serde_json::json!(0));
    evidence.insert(
        "trials".into(),
        serde_json::json!([
            {"name": "task-hard", "reward": 0.0},
            {"name": "atrx", "reward": 0.0}
        ]),
    );
    RlmToHost::Done {
        output: VmJobOutput::Evaluated(RunOutcome {
            report: CustomRunReport {
                schema_version: RUN_REPORT_SCHEMA,
                topic_id: req.topic_id,
                custom_id: req.custom_id,
                submission_digest: req.submission_digest,
                artifact_digest: req.artifact_digest,
                rules_version: req.rules_version,
                primary_value: 0.0,
                claim_holds: true,
                sandboxed: true,
                flops_used: None,
                evidence,
            },
            logs: vec![],
        }),
    }
}

/// Happy-path vsock Done must still refresh a Done≈finish dump (a21f):
/// atrx `result.json` missing, job-level `n_running=1`. Score stays the vsock
/// report (no `host_harvest`).
#[tokio::test]
async fn vsock_done_refreshes_stale_dump_keeps_vsock_score() {
    let root = tree("done-refresh");
    let overlay = root.join(SCRATCH_TREE).join("work");
    let dump = root.join(HARVEST_WORK);
    plant(&overlay, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&overlay, "0001-evaluate", "run", "task-hard", "0");
    plant_trial(&overlay, "0001-evaluate", "run", "atrx", "0");
    plant_job_snapshot(&overlay, "0001-evaluate", "run", 0, "2026-09-11T09:22:22Z");
    plant(&dump, "evaluate", "0001", &harbor_report_n2("0.0"));
    plant_trial(&dump, "0001-evaluate", "run", "task-hard", "0");
    let atrx = dump
        .join("0001-evaluate")
        .join(HARBOR_JOBS)
        .join("run")
        .join("atrx");
    std::fs::create_dir_all(atrx.join("verifier")).expect("atrx");
    std::fs::write(atrx.join("verifier").join("reward.txt"), "0").expect("reward");
    plant_job_snapshot(&dump, "0001-evaluate", "run", 1, "null");
    let done = vsock_done_n2();
    let got = recv_job_or_harvest(
        async { Ok(done) },
        &root,
        &root,
        &evaluate_job(),
        Duration::from_secs(30),
    )
    .await
    .expect("vsock Done");
    let RlmToHost::Done {
        output: VmJobOutput::Evaluated(run),
    } = got
    else {
        panic!("expected Evaluated, got {got:?}");
    };
    assert!((run.report.primary_value - 0.0).abs() < 1e-12);
    assert!(
        !run.report.evidence.contains_key("host_harvest"),
        "vsock Done must not be rewritten as a harvest reconstruct"
    );
    let refreshed = dump
        .join("0001-evaluate")
        .join(HARBOR_JOBS)
        .join("run")
        .join("atrx")
        .join("result.json");
    assert!(
        refreshed.is_file() && std::fs::metadata(&refreshed).expect("meta").len() > 0,
        "Done must copy overlay atrx result.json onto harvest-work"
    );
    let snap = std::fs::read_to_string(
        dump.join("0001-evaluate")
            .join(HARBOR_JOBS)
            .join("run")
            .join("result.json"),
    )
    .expect("job snapshot");
    assert!(
        !snap.contains("\"finished_at\":null"),
        "dump must not stay finished_at=null, got {snap}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
