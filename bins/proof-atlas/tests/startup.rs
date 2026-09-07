#![allow(clippy::unwrap_used, clippy::expect_used)]
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Output, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::process::Command;
use tokio::sync::Notify;

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
        let key_path = private_write(root.path(), "signer", &signer);
        let pin_path = private_write(root.path(), "pin", pin.as_bytes());
        let db_path = private_write(root.path(), "database", app_url.as_bytes());
        let model = private_write(root.path(), "model", b"not-used-in-check-mode");
        let config = json!({
            "schema_version":1, "database_url_file":db_path,
            "proof_secret_file":key_path, "proof_pin_file":pin_path,
            "chain_endpoints":"http://127.0.0.1:1",
            "gateway_url":"http://127.0.0.1:1",
            "netuid":7, "anchor_block":0, "policy":"Synthetic check fixture, never launch.",
            "headless":{
                "node":"/usr/bin/false", "loader":model, "entrypoint":model,
                "tsconfig":model, "model_config_file":model, "private_root":root.path(),
                "kernel_python":"/usr/bin/false", "runtime_pythonpath":root.path(),
                "kernel":{"image":format!("sha256:{}", "a".repeat(64)),
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_proof-atlas"));
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
}

fn private_write(parent: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = parent.join(name);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

#[tokio::test]
async fn check_uses_canonical_migrations_but_never_launches_or_freezes() {
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
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_atlas_round")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    f.database.drop_schema().await.unwrap();
}

#[tokio::test]
async fn startup_rejects_owner_connection_wrong_signer_and_unknown_fields() {
    let Some(mut f) = Fixture::new().await else {
        return;
    };
    private_write(f.root.path(), "database", f.owner_url.as_bytes());
    let result = f.check().await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("restricted privileges"));
    assert!(!String::from_utf8_lossy(&result.stderr).contains(&f.owner_url));
    private_write(f.root.path(), "database", f.app_url.as_bytes());
    private_write(f.root.path(), "signer", &[7; 32]);
    let result = f.check().await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("differs from topic trust pin"));
    private_write(f.root.path(), "signer", &[0xff; 32]);
    f.config["unexpected"] = json!(true);
    let result = f.check().await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("invalid Atlas configuration"));
    f.database.drop_schema().await.unwrap();
}

#[tokio::test]
async fn service_waits_for_finality_and_stops_on_sigterm_without_agents() {
    let Some(mut f) = Fixture::new().await else {
        return;
    };
    let observed = Arc::new(Notify::new());
    let notify = observed.clone();
    let router = axum::Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let notify = notify.clone();
            async move {
                let result = match body["method"].as_str().unwrap() {
                    "chain_getFinalizedHead" => json!(format!("0x{}", "1".repeat(64))),
                    "chain_getHeader" => {
                        notify.notify_one();
                        json!({"number":"0x0"})
                    }
                    other => panic!("unexpected RPC method {other}"),
                };
                axum::Json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    f.config["chain_endpoints"] = json!(format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let child = f.command().spawn().unwrap();
    tokio::time::timeout(Duration::from_secs(10), observed.notified())
        .await
        .unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()).unwrap(),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_atlas_round")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    server.abort();
    let _ = server.await;
    f.database.drop_schema().await.unwrap();
}
