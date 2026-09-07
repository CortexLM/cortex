#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::{SystemTime, UNIX_EPOCH};

use proof_autonomy::commitment;
use proof_research::artifact_digest;
use sqlx::types::Json;

use super::*;
use crate::{Invocation, Plan};

const IMAGE: &str = "sha256:0104307df448338d8475c7cf8152e5e0655e211fd1c04b2bdc94e6758a7e7293";

fn intent(target: &DockerSandbox, script: &str, timeout_ms: u64) -> Intent {
    let plan = Plan {
        schema_version: 1,
        recipe_digest: None,
        kernel_source: None,
        runs: vec![Invocation::Script {
            script_digest: artifact_digest(script.as_bytes()),
            seed: Some(7),
            timeout_ms,
        }],
    };
    Intent {
        id: Uuid::new_v4(),
        experiment_id: Uuid::new_v4(),
        operation_key: commitment(&plan).unwrap(),
        controller_fence: 1,
        owner_id: Uuid::new_v4(),
        resource_id: "local-unit-target".into(),
        engine_id: target.engine_id.clone(),
        image_id: target.image_id.clone(),
        plan: Json(plan),
        deadline_ms: i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
            + 10_000,
        state: "dispatched".into(),
    }
}

#[test]
fn docker_log_frames_are_bounded_and_strict() {
    assert_eq!(
        unframe(&[1, 0, 0, 0, 0, 0, 0, 2, b'o', b'k']).unwrap(),
        b"ok"
    );
    for bytes in [
        &b"junk"[..],
        &[1, 0, 0, 0, 0, 0, 0, 255],
        &[9, 0, 0, 0, 0, 0, 0, 0],
    ] {
        assert!(unframe(bytes).is_err());
    }
}

#[tokio::test]
#[ignore = "requires exact pinned local Docker; proves target timeout with no controller polling"]
async fn target_deadline_kills_detached_descendants_without_controller() {
    let target = DockerSandbox::connect(Path::new("/var/run/docker.sock"), IMAGE)
        .await
        .unwrap();
    let script = "import os,time,signal\nif os.fork()==0:\n os.setsid()\n signal.signal(signal.SIGTERM,signal.SIG_IGN)\n while True: time.sleep(.01)\nprint('descendant-started',flush=True)\nwhile True: time.sleep(.01)\n";
    let intent = intent(&target, script, 150);
    let mut payload = serde_json::to_value(&intent.plan.runs[0]).unwrap();
    payload["script"] = json!(script);
    target.create(&intent, 0, payload).await.unwrap();
    target.start(&intent, 0).await.unwrap();
    // No polling, SIGTERM, host timeout process, or force-remove until after the
    // target alone must have exited. PID namespace death kills setsid children.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let stopped = target.stopped(&intent, 0).await.unwrap();
    let logs = target.logs(&intent, 0).await.unwrap();
    target.remove(&intent, 0).await.unwrap();
    assert!(stopped);
    let receipt: RunObservation = serde_json::from_slice(&logs).unwrap();
    assert_eq!(receipt.failure.as_deref(), Some("deadline"));
    assert!(receipt.wall_ms >= 150 && receipt.wall_ms < 500);
    assert!(String::from_utf8_lossy(&receipt.log).contains("descendant-started"));
    assert_eq!(receipt.exit_code, None);
    assert!(target.inspect(&intent, 0).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires exact pinned local Docker; verifies protected supervisor channel"]
async fn workload_cannot_forge_supervisor_receipts_or_change_source() {
    let target = DockerSandbox::connect(Path::new("/var/run/docker.sock"), IMAGE)
        .await
        .unwrap();
    let script = "import os,socket\nfor p in ['/proc/1/fd/1','/run/proof/program.py']:\n try:\n  fd=os.open(p,os.O_WRONLY)\n except PermissionError:\n  pass\n else:\n  os.close(fd)\n  raise RuntimeError('supervisor mutable')\ns=socket.socket();s.settimeout(.1)\ntry:\n s.connect(('198.51.100.1',9))\nexcept OSError:\n pass\nelse:\n raise RuntimeError('network enabled')\nprint('{\"exit_code\":0,\"metrics\":{},\"flops_used\":999}')\n";
    let intent = intent(&target, script, 3000);
    let mut payload = serde_json::to_value(&intent.plan.runs[0]).unwrap();
    payload["script"] = json!(script);
    target.create(&intent, 0, payload).await.unwrap();
    let config = target.inspect(&intent, 0).await.unwrap().unwrap();
    target.start(&intent, 0).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let logs = target.logs(&intent, 0).await.unwrap();
    target.remove(&intent, 0).await.unwrap();
    assert_eq!(
        config.pointer("/HostConfig/NetworkMode"),
        Some(&json!("none"))
    );
    assert_eq!(config.pointer("/HostConfig/PidsLimit"), Some(&json!(64)));
    assert_eq!(
        config.pointer("/HostConfig/Privileged"),
        Some(&json!(false))
    );
    assert_eq!(
        config.pointer("/HostConfig/ReadonlyRootfs"),
        Some(&json!(true))
    );
    assert_eq!(config["HostConfig"]["Tmpfs"].as_object().unwrap().len(), 2);
    assert!(config["HostConfig"]["Binds"].is_null());
    // Exactly one anonymous artifact volume; never a host path.
    let mounts = config["Mounts"].as_array().unwrap();
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0]["Type"], json!("volume"));
    assert_eq!(mounts[0]["Destination"], json!(crate::ARTIFACT_DIR));
    let receipt: RunObservation = serde_json::from_slice(&logs).unwrap();
    assert_eq!(receipt.exit_code, Some(0), "{receipt:?}");
    assert!(receipt.metrics.is_none() && receipt.flops_used.is_none());
    assert!(String::from_utf8_lossy(&receipt.log).contains("999"));
}

#[tokio::test]
#[ignore = "requires pinned local Docker; simulates delayed dispatch after grant expiry"]
async fn delayed_start_after_absolute_deadline_never_executes_script() {
    let target = DockerSandbox::connect(Path::new("/var/run/docker.sock"), IMAGE)
        .await
        .unwrap();
    let script = "print('must-not-execute',flush=True)\n";
    let mut intent = intent(&target, script, 25_000);
    intent.deadline_ms = 1;
    let mut payload = serde_json::to_value(&intent.plan.runs[0]).unwrap();
    payload["script"] = json!(script);
    target.create(&intent, 0, payload).await.unwrap();
    target.start(&intent, 0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let stopped = target.stopped(&intent, 0).await.unwrap();
    let logs = target.logs(&intent, 0).await.unwrap();
    target.remove(&intent, 0).await.unwrap();
    assert!(stopped);
    let receipt: RunObservation = serde_json::from_slice(&logs).unwrap();
    assert_eq!(receipt.failure.as_deref(), Some("deadline"));
    assert!(
        receipt.script_digest.is_none() && receipt.exit_code.is_none() && receipt.log.is_empty()
    );
}
