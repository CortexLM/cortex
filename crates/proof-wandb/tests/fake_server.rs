//! In-process fake W&B: asserts the exact request set, dedup, readback and
//! GraphQL failure handling, and that no telemetry-shaped field ever leaves.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::{post, put};
use axum::{Json, Router};
use proof_autonomy::commitment;
use proof_research::{EvidencePublisher, PublicEvidence};
use proof_wandb::{manifest_digest, record_bytes, WandbConfig, WandbPublisher, USER_AGENT};
use serde_json::{json, Value};

const KEY: &str = "deadbeefsecretkeydeadbeefsecretkey000000";
const RECORD_MD5: &str = "GVCCT9Hv5F/lfbxs4MFIeA==";

#[derive(Clone, Copy)]
enum Mode {
    Happy,
    Dedup,
    Mismatch,
    GraphqlError,
}

#[derive(Debug)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

#[derive(Clone)]
struct Fake {
    mode: Mode,
    seen: Arc<Mutex<Vec<Seen>>>,
}

fn record(state: &Fake, path: &str, headers: &HeaderMap, body: Value) {
    let headers = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("?").to_owned()))
        .collect();
    if let Ok(mut seen) = state.seen.lock() {
        seen.push(Seen {
            path: path.to_owned(),
            headers,
            body,
        });
    }
}

async fn graphql(
    State(fake): State<Fake>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    record(&fake, "/graphql", &headers, body.clone());
    let query = body["query"].as_str().unwrap_or("");
    if matches!(fake.mode, Mode::GraphqlError) {
        return Json(json!({ "errors": [{ "message": "boom" }] }));
    }
    let base = fake
        .seen
        .lock()
        .map(|s| s[0].headers.clone())
        .unwrap_or_default();
    let host = base
        .iter()
        .find(|(k, _)| k == "host")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let upload = |name: &str| json!({ "uploadUrl": format!("http://{host}/upload/{name}"), "uploadHeaders": ["Content-Type: application/octet-stream"] });
    let digest = if matches!(fake.mode, Mode::Mismatch) {
        "00000000000000000000000000000000".to_owned()
    } else {
        manifest_digest(RECORD_MD5)
    };
    let data = if query.starts_with("mutation createArtifact(") {
        let state = if matches!(fake.mode, Mode::Dedup) {
            "COMMITTED"
        } else {
            "PENDING"
        };
        json!({ "createArtifact": { "artifact": { "id": "art1", "state": state, "artifactSequence": { "latestArtifact": null } } } })
    } else if query.starts_with("mutation createArtifactManifest(") {
        json!({ "createArtifactManifest": { "artifactManifest": { "id": "man1", "file": upload("wandb_manifest.json") } } })
    } else if query.starts_with("mutation createArtifactFiles(") {
        json!({ "createArtifactFiles": { "files": { "edges": [{ "node": upload("record.json") }] } } })
    } else if query.starts_with("mutation commitArtifact(") {
        json!({ "commitArtifact": { "artifact": { "id": "art1", "digest": digest } } })
    } else if query.starts_with("query artifact(") {
        json!({ "artifact": { "state": "COMMITTED", "digest": digest } })
    } else {
        return Json(json!({ "errors": [{ "message": "unknown operation" }] }));
    };
    Json(json!({ "data": data }))
}

async fn upload(State(fake): State<Fake>, headers: HeaderMap, body: Bytes) -> &'static str {
    let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
    record(&fake, "/upload", &headers, body);
    ""
}

async fn start(mode: Mode) -> (Fake, WandbPublisher, tempfile::TempDir) {
    let fake = Fake {
        mode,
        seen: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/graphql", post(graphql))
        .route("/upload/{name}", put(upload))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let key_path = dir.path().join("key");
    std::fs::write(&key_path, format!("{KEY}\n")).unwrap();
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let publisher = WandbPublisher::new(WandbConfig {
        base_url: format!("http://{addr}/"),
        entity: "cortex".into(),
        project: "proof".into(),
        api_key_file: key_path,
        run_name: None,
        timeout: Duration::from_secs(5),
        allow_loopback_http: true,
    })
    .unwrap();
    (fake, publisher, dir)
}

fn doc() -> PublicEvidence {
    PublicEvidence {
        schema_version: 1,
        evidence_digest: "0123456789abcdef".repeat(4),
        recipe_digest: "fedcba9876543210".repeat(4),
        repetitions: 3,
        primary_mean: 0.5,
        primary_standard_error: 0.01,
        passed: true,
    }
}

/// Every header the client may send, and nothing else.
fn assert_allowlisted(seen: &[Seen]) {
    let allowed: BTreeSet<&str> = [
        "host",
        "user-agent",
        "authorization",
        "content-type",
        "content-length",
        "accept",
        "accept-encoding",
    ]
    .into();
    for req in seen {
        let names: BTreeSet<&str> = req.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.is_subset(&allowed), "unexpected headers {names:?}");
        let ua = req
            .headers
            .iter()
            .find(|(k, _)| k == "user-agent")
            .map(|(_, v)| v.as_str());
        assert_eq!(ua, Some(USER_AGENT));
        let dump = format!("{:?} {}", req.headers, req.body).to_ascii_lowercase();
        for forbidden in [
            "python",
            "platform",
            "hostname",
            "uname",
            "username",
            "program",
            "telemetry",
            KEY,
        ] {
            assert!(
                !dump.contains(forbidden),
                "{forbidden} leaked in {}",
                req.path
            );
        }
    }
}

#[tokio::test]
async fn happy_path_sends_exact_sequence_and_confirms_digest() {
    let (fake, publisher, _dir) = start(Mode::Happy).await;
    let receipt = publisher.publish(&doc()).await.unwrap();
    assert_eq!(receipt, commitment(&doc()).unwrap());
    let seen = fake.seen.lock().unwrap();
    assert_allowlisted(&seen);
    let record = String::from_utf8(record_bytes(&doc()).unwrap()).unwrap();
    let alias = doc().evidence_digest;
    let collection = format!("proof-evidence-{}", &alias[..16]);
    let ops: Vec<(&str, Value)> = seen
        .iter()
        .map(|s| (s.path.as_str(), s.body["variables"].clone()))
        .collect();
    let auth = seen[0]
        .headers
        .iter()
        .find(|(k, _)| k == "authorization")
        .unwrap()
        .1
        .clone();
    assert_eq!(auth, format!("Basic {}", base64_std(format!("api:{KEY}"))));
    let manifest_common = json!({
        "artifactID": "art1", "name": "wandb_manifest.json", "entityName": "cortex",
        "projectName": "proof", "runName": null, "type": "FULL",
    });
    let mut m1 = manifest_common.clone();
    m1["digest"] = json!("");
    m1["includeUpload"] = json!(false);
    let manifest_json = serde_json::json!({
        "contents": { "record.json": { "digest": RECORD_MD5, "size": record.len() } },
        "storagePolicy": "wandb-storage-policy", "storagePolicyConfig": {}, "version": 1,
    });
    let mut m2 = manifest_common;
    m2["digest"] = json!(base64_std(md5_of(&proof_task::canonical_json(
        &manifest_json
    ))));
    m2["includeUpload"] = json!(true);
    let expected = vec![
        (
            "/graphql",
            json!({ "input": {
                "artifactTypeName": "proof-evidence", "artifactCollectionName": collection,
                "entityName": "cortex", "projectName": "proof", "runName": null,
                "digest": manifest_digest(RECORD_MD5), "digestAlgorithm": "MANIFEST_MD5",
                "clientID": alias, "sequenceClientID": alias, "enableDigestDeduplication": true,
                "metadata": record, "description": format!("Cortex Proof public evidence {alias}"),
                "aliases": [{ "artifactCollectionName": collection, "alias": alias }],
            }}),
        ),
        ("/graphql", json!({ "input": m1 })),
        (
            "/graphql",
            json!({ "input": {
                "artifactFiles": [{ "artifactID": "art1", "name": "record.json", "md5": RECORD_MD5, "artifactManifestID": "man1" }],
                "storageLayout": "V2",
            }}),
        ),
        ("/upload", Value::Null),
        ("/graphql", json!({ "input": m2 })),
        ("/upload", Value::Null),
        ("/graphql", json!({ "input": { "artifactID": "art1" } })),
        (
            "/graphql",
            json!({ "name": format!("cortex/proof/{collection}:{alias}") }),
        ),
    ];
    assert_eq!(ops, expected);
    assert_eq!(
        seen[3].body,
        serde_json::from_str::<Value>(&record).unwrap()
    );
    assert_eq!(seen[5].body, manifest_json);
}

#[tokio::test]
async fn committed_on_create_skips_uploads_but_still_reads_back() {
    let (fake, publisher, _dir) = start(Mode::Dedup).await;
    publisher.publish(&doc()).await.unwrap();
    let seen = fake.seen.lock().unwrap();
    assert_allowlisted(&seen);
    let paths: Vec<&str> = seen.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, ["/graphql", "/graphql"]);
    assert!(seen[1].body["query"]
        .as_str()
        .unwrap()
        .starts_with("query artifact("));
}

#[tokio::test]
async fn readback_digest_mismatch_fails_closed() {
    let (_fake, publisher, _dir) = start(Mode::Mismatch).await;
    let err = publisher.publish(&doc()).await.unwrap_err();
    assert!(matches!(err, proof_research::ResearchError::Publication));
}

#[tokio::test]
async fn graphql_error_fails_closed_and_redacts_key() {
    let (fake, publisher, _dir) = start(Mode::GraphqlError).await;
    let err = publisher.publish(&doc()).await.unwrap_err();
    let text = format!("{err} {err:?} {publisher:?}");
    assert!(!text.contains(KEY));
    assert_eq!(fake.seen.lock().unwrap().len(), 1);
}

#[test]
fn config_rejects_plain_http_and_open_key_files() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key");
    std::fs::write(&key, KEY).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let cfg = |base: &str, allow: bool| WandbConfig {
        base_url: base.into(),
        entity: "cortex".into(),
        project: "proof".into(),
        api_key_file: key.clone(),
        run_name: None,
        timeout: Duration::from_secs(1),
        allow_loopback_http: allow,
    };
    assert!(
        WandbPublisher::new(cfg("https://api.wandb.ai", false)).is_err(),
        "0644 key"
    );
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(WandbPublisher::new(cfg("https://api.wandb.ai", false)).is_ok());
    assert!(WandbPublisher::new(cfg("http://api.wandb.ai", true)).is_err());
    assert!(WandbPublisher::new(cfg("http://127.0.0.1:1", false)).is_err());
    assert!(WandbPublisher::new(cfg("http://127.0.0.1:1", true)).is_ok());
}

/// Live probe. Never runs by default; needs `--ignored` and a private config.
#[tokio::test]
#[ignore = "live wandb probe; set CORTEX_TEST_WANDB_CONFIG and run with --ignored"]
async fn live_probe_publishes_and_reads_back() {
    let Ok(path) = std::env::var("CORTEX_TEST_WANDB_CONFIG") else {
        return;
    };
    let cfg: WandbConfig = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let publisher = WandbPublisher::new(cfg).unwrap();
    let digest = publisher.publish_artifact(&doc()).await.unwrap();
    assert_eq!(digest, manifest_digest(RECORD_MD5));
}

fn md5_of(s: &str) -> Vec<u8> {
    use md5::Digest;
    md5::Md5::digest(s.as_bytes()).to_vec()
}

fn base64_std(bytes: impl AsRef<[u8]>) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
