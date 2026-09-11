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
    HARVEST_WORK, POLL_INTERVAL, SCRATCH_IN_JAIL, SCRATCH_TREE,
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
fn overlay_finished_dump_still_n_running_is_refused() {
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
    let err = harvest_from_jail(&root, &root, &evaluate_job()).expect_err("stale dump");
    let msg = err.to_string();
    assert!(
        msg.contains("harvest-work incomplete"),
        "expected fail-closed, got {msg}"
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
