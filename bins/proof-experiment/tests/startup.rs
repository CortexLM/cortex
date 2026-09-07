#![allow(clippy::unwrap_used, clippy::expect_used)]
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};
use tokio::process::Command;

const IMAGE: &str = "sha256:0104307df448338d8475c7cf8152e5e0655e211fd1c04b2bdc94e6758a7e7293";
const SOCKET: &str = "/var/run/docker.sock";

struct Fixture {
    database: db::TestPool,
    root: tempfile::TempDir,
    config: Value,
    app_url: String,
    owner_url: String,
}

impl Fixture {
    async fn new() -> Option<Self> {
        let owner = std::env::var("DATABASE_URL").ok()?;
        if !Path::new(SOCKET).exists() {
            eprintln!("Docker integration test skipped: {SOCKET} is absent");
            return None;
        }
        let database = db::test_pool_with_url(&owner).await.unwrap();
        let options = format!("?options=-c%20search_path%3D{},public", database.schema());
        let app_url = format!("{}{}", db::app_role_database_url(&owner).unwrap(), options);
        let owner_url = format!("{owner}{options}");
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let signer = [0xff; 32];
        let public = hex::encode(challenge_keys::public_key_from_secret(&signer).unwrap());
        let pin = include_str!("../../../config/proof-pin.toml")
            .lines()
            .map(|line| {
                if line.starts_with("topic_pubkey = ") {
                    format!("topic_pubkey = \"{public}\"")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let pin_path = private_write(root.path(), "pin", pin.as_bytes());
        let secret_path = private_write(root.path(), "secret", hex::encode(signer).as_bytes());
        let db_path = private_write(root.path(), "database", app_url.as_bytes());
        let model = private_write(root.path(), "model", b"not-used-in-check-mode");
        let holdout = root.path().join("holdout");
        fs::create_dir(&holdout).unwrap();
        fs::set_permissions(&holdout, fs::Permissions::from_mode(0o700)).unwrap();
        let config = json!({
            "schema_version":1, "database_url_file":db_path, "proof_pin_file":pin_path,
            "proof_secret_file":secret_path,
            "chain_endpoints":"http://127.0.0.1:1", "netuid":7,
            "publisher":{"kind":"none"},
            "docker_socket":SOCKET, "image_id":IMAGE, "slots":1, "lifetime_seconds":3600,
            "holdout_store":holdout,
            "policy":"Synthetic check fixture, never launch.",
            "headless":{
                "node":"/usr/bin/false", "loader":model, "entrypoint":model,
                "tsconfig":model, "model_config_file":model, "private_root":root.path(),
                "kernel_python":"/usr/bin/false", "runtime_pythonpath":root.path(),
                "kernel":{"image":IMAGE,
                    "memoryMb":256, "workspaceMb":16, "cpus":1, "pids":64, "seconds":30},
                "budget":{"maxDepth":1,"maxChildren":1,"maxConcurrentCalls":1,
                    "maxCalls":1,"maxReservedTokens":100,"maxReservedMicroUsd":100,"timeoutMs":30000}
            }
        });
        Some(Self {
            database,
            root,
            config,
            app_url,
            owner_url,
        })
    }
    fn command(&self) -> Command {
        let path = private_write(
            self.root.path(),
            "config",
            &serde_json::to_vec(&self.config).unwrap(),
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_proof-experiment"));
        command
            .env_clear()
            .arg("--config")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }
    async fn check(&self) -> Output {
        tokio::time::timeout(
            Duration::from_secs(20),
            self.command().arg("--check").output(),
        )
        .await
        .unwrap()
        .unwrap()
    }
    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(self.database.pool())
            .await
            .unwrap()
    }
}

fn private_write(parent: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = parent.join(name);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

#[tokio::test]
async fn check_validates_daemon_and_restricted_db_but_never_launches() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let result = f.check().await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("no worker started"));
    assert_eq!(f.count("proof_local_slot").await, 1);
    assert_eq!(f.count("proof_local_event").await, 0);
    assert_eq!(f.count("proof_experiment").await, 0);
    f.database.drop_schema().await.unwrap();
}

#[tokio::test]
async fn startup_rejects_owner_connection_unpinned_image_and_unknown_fields() {
    let Some(mut f) = Fixture::new().await else {
        return;
    };
    private_write(f.root.path(), "database", f.owner_url.as_bytes());
    let result = f.check().await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("restricted privileges"));
    assert!(!String::from_utf8_lossy(&result.stderr).contains(&f.owner_url));
    private_write(f.root.path(), "database", f.app_url.as_bytes());
    f.config["image_id"] = json!("cortex-atlas-kernel:local-test");
    let result = f.check().await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("pinned image"));
    f.config["image_id"] = json!(format!("sha256:{}", "a".repeat(64)));
    let result = f.check().await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("pinned image"));
    f.config["image_id"] = json!(IMAGE);
    f.config["unexpected"] = json!(true);
    let result = f.check().await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("invalid experiment configuration"));
    f.config.as_object_mut().unwrap().remove("unexpected");
    f.config["publisher"] = json!({"kind":"gateway","gateway_url":"http://example.com"});
    let result = f.check().await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("evidence gateway destination"));
    f.config["publisher"] = json!({"kind":"gateway","gateway_url":"http://127.0.0.1:1"});
    let result = f.check().await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    private_write(f.root.path(), "secret", hex::encode([0xee; 32]).as_bytes());
    let result = f.check().await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("differs from topic trust pin"));
    f.database.drop_schema().await.unwrap();
}

#[tokio::test]
async fn service_polls_and_stops_on_sigterm_without_experiments() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut child = f.command().spawn().unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(child.try_wait().unwrap().is_none(), "service exited early");
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()).unwrap(),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(f.count("proof_experiment").await, 0);
    assert_eq!(f.count("proof_local_event").await, 0);
    f.database.drop_schema().await.unwrap();
}

/// The observer is the difference between a service that can produce rewardable
/// evidence and one that always fails closed, so `--check` must report which it
/// is and must refuse a misconfigured observer at startup.
#[tokio::test]
async fn check_reports_fail_closed_without_an_observer_and_builds_one_with_it() {
    let Some(mut f) = Fixture::new().await else {
        return;
    };
    // Default fixture has no observer: say so, loudly.
    let out = String::from_utf8_lossy(&f.check().await.stdout).into_owned();
    assert!(
        out.contains("NO observer") && out.contains("UnobservedMeasurements"),
        "check must disclose fail-closed scoring, got {out}"
    );

    // A configured observer must actually resolve the pinned image, the holdout
    // records and the offer.
    let holdouts = private_write(
        f.root.path(),
        "holdouts.json",
        serde_json::to_vec(&json!({
            "t": proof_task::synthetic_holdout(proof_task::STRATUM_SIZE, 1),
        }))
        .unwrap()
        .as_slice(),
    );
    let offer = private_write(
        f.root.path(),
        "offer.json",
        serde_json::to_vec(&json!({
            "offer_id": "startup-check",
            "config_commitment": "0".repeat(64),
            "provider": {"kind": "openai_compatible", "base_url": "https://judge.example/v1"},
            "config": {"mode": "chat", "model_ref": "m", "max_input_tokens": 1024,
                       "max_output_tokens": 16, "temperature": null, "top_p": null,
                       "timeout_ms": null},
            "status": "open",
        }))
        .unwrap()
        .as_slice(),
    );
    f.config["observer"] = json!({
        "image": IMAGE, "holdouts_file": holdouts, "offer_file": offer, "judge": null,
    });
    let out = f.check().await;
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && text.contains("trusted observer configured"),
        "observer must be constructed: {} {}",
        text,
        String::from_utf8_lossy(&out.stderr)
    );

    // An unpinned scoring image must be refused rather than silently ignored.
    f.config["observer"]["image"] = json!("python:latest");
    let out = f.check().await;
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("pinned scoring image"));
    f.database.drop_schema().await.unwrap();
}
