//! Process-level Proof submit → sim score.
//!
//! Spawns `proof-challenge --force-sim` with disposable synthetic
//! topic / holdout / baseline / offer files. No Lium, no compose, no secrets.
//! Topic ids match the staging open set.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use proof_eval::{sim_document, BaselineMeasurement, BASELINE_SKILL};
use proof_task::{
    default_adamw, holdout_commitment, inference_config_commitment, synthetic_holdout, Constraints,
    InferenceConfig, InferenceMode, InferenceOffer, InferenceProvider, InferenceProviderKind,
    MetricDirection, MetricFamily, MetricSpec, OfferStatus, ProofPin, TopicDocument, TopicStatus,
    EVAL_IMAGE, FLOPS_BUDGET_MAX, HOLDOUT_SIZE, METRIC_TOKENS_PER_SEC, PRIMARY_HOLDOUT_NLL,
    STRATUM_SIZE,
};
use serde_json::Value;
use tokio::process::Command;

const DT: &str = "dt-no-ib-v0";
const MUON: &str = "muon-vs-adamw-10m-v0";
const OFFER_ID: &str = "openrouter-glm53flash-v0";

fn sk() -> [u8; 32] {
    let mut s = [3u8; 32];
    s[0] = 17;
    s
}

fn pk_hex() -> String {
    hex::encode(crypto::public_key_from_mini_secret(&sk()).expect("pk"))
}

fn digest(label: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(label.as_bytes());
    hex::encode(h.finalize())
}

fn pin() -> ProofPin {
    let mut p = ProofPin {
        topic_pubkey: pk_hex(),
        ..ProofPin::default()
    };
    p.inference.model = "master-proxy-v0".into();
    p
}

fn offer() -> InferenceOffer {
    let config = InferenceConfig {
        mode: InferenceMode::Chat,
        model_ref: "master-proxy-v0".into(),
        max_input_tokens: 32_768,
        max_output_tokens: 8_192,
        temperature: Some(0.0),
        top_p: None,
        timeout_ms: None,
    };
    InferenceOffer {
        offer_id: OFFER_ID.into(),
        provider: InferenceProvider {
            kind: InferenceProviderKind::OpenaiCompatible,
            base_url: "http://127.0.0.1:8000/v1".into(),
        },
        config_commitment: inference_config_commitment(&config, "http://127.0.0.1:8000/v1"),
        config,
        status: OfferStatus::Open,
    }
}

fn dt_topic() -> TopicDocument {
    let mut baseline = default_adamw(FLOPS_BUDGET_MAX);
    baseline.optimizer = "nccl-ib-reference".into();
    baseline.wall_budget_s = 14_400;
    baseline.script_sha256 = "11".repeat(32);
    TopicDocument {
        id: DT.into(),
        statement: "No IB/NVLink; 12.5 Gbit/s cap; beat sealed comms baseline.".into(),
        payout_mode: proof_task::PayoutMode::Wta,
        constraints: Constraints {
            no_infiniband: true,
            no_nvlink: true,
            no_nccl_fast_fabric: true,
            max_inter_node_gbps: Some(12.5),
            ..Constraints::default()
        },
        metric: MetricSpec {
            family: MetricFamily::Throughput,
            primary: METRIC_TOKENS_PER_SEC.into(),
            direction: MetricDirection::Max,
            unit: "tokens_per_second".into(),
            epsilon_rel: 0.05,
            quality_floor_nll: 0.02,
            wall_budget_s: 14_400,
            custom_id: String::new(),
        },
        baseline,
        holdout_size: HOLDOUT_SIZE,
        status: TopicStatus::Open,
        ..TopicDocument::default()
    }
}

fn muon_topic() -> TopicDocument {
    let mut baseline = default_adamw(FLOPS_BUDGET_MAX);
    baseline.script_sha256 = "11".repeat(32);
    TopicDocument {
        id: MUON.into(),
        statement:
            "Beat sealed AdamW holdout NLL with Muon at ~10M params under the same FLOP budget."
                .into(),
        payout_mode: proof_task::PayoutMode::Wta,
        metric: MetricSpec {
            family: MetricFamily::Nll,
            primary: PRIMARY_HOLDOUT_NLL.into(),
            direction: MetricDirection::Min,
            unit: "nll".into(),
            epsilon_rel: 0.0,
            quality_floor_nll: 0.0,
            wall_budget_s: 0,
            custom_id: String::new(),
        },
        baseline,
        holdout_size: HOLDOUT_SIZE,
        status: TopicStatus::Open,
        ..TopicDocument::default()
    }
}

fn seal(
    pin: &ProofPin,
    mut topic: TopicDocument,
) -> (
    TopicDocument,
    BaselineMeasurement,
    Vec<proof_task::HoldoutRecord>,
) {
    let recs = synthetic_holdout(STRATUM_SIZE, 1);
    topic.holdout_commitment = holdout_commitment(&recs);
    let doc = sim_document(pin, &topic, "base", "base-art", BASELINE_SKILL, true);
    let meas = BaselineMeasurement {
        eval_image_digest: pin.eval_image_digest.clone(),
        topic_id: topic.id.clone(),
        holdout_commitment: topic.holdout_commitment.clone(),
        holdout_nll: doc.harness.holdout_nll,
        split_nll: doc.harness.split_nll.clone(),
        tokens_per_sec: doc.harness.tokens_per_sec,
        step_latency_ms: doc.harness.step_latency_ms,
        custom_value: doc.harness.custom_value,
    };
    topic.baseline.metrics_commitment = meas.commitment();
    topic.signature = topic.sign_with(&sk()).expect("sign");
    topic.validate(pin, &[]).expect("valid");
    topic.verify_signature(pin).expect("sig");
    (topic, meas, recs)
}

fn write_pin(dir: &Path, pin: &ProofPin) -> PathBuf {
    let path = dir.join("pin.toml");
    let body = format!(
        r#"challenge_id = "proof"
scoring_version = 1
base_model_family = "Qwen/Qwen3.8"
proxy_model = ""
proxy_models = []
inference_config_schema_version = 1
allowed_modes = ["chat", "completions", "embeddings"]
max_input_tokens_ceiling = 32768
max_output_tokens_ceiling = 8192
inference_offer_commitment_alg = "sha256"
eval_image = "{EVAL_IMAGE}"
eval_image_digest = "{digest}"
proof_git = "https://github.com/CortexLM/cortex"
proof_git_sha = ""
topic_pubkey = "{pk}"
flops_budget_max = 2000000000000000000
epsilon_nll_min = 0.02
epsilon_topic_max_regress_min = 0.05
epsilon_throughput_rel_min = 0.05
quality_floor_nll_max = 0.02
holdout_size = 120
stratum_size = 24

[inference]
provider = "openai_compatible"
base_url = ""
model = "master-proxy-v0"
mode = "chat"
max_input_tokens = 32768
max_output_tokens = 8192
"#,
        digest = pin.eval_image_digest,
        pk = pin.topic_pubkey,
    );
    fs::write(&path, body).expect("pin");
    path
}

fn workdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "proof-submit-e2e-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("dir");
    dir
}

struct Host {
    child: tokio::process::Child,
    base: String,
    dir: PathBuf,
}

impl Host {
    async fn spawn() -> Self {
        let dir = workdir();
        let p = pin();
        let (dt, dt_meas, dt_recs) = seal(&p, dt_topic());
        let (muon, muon_meas, muon_recs) = seal(&p, muon_topic());
        fs::write(
            dir.join("topics.json"),
            serde_json::to_vec(&[&dt, &muon]).expect("topics"),
        )
        .expect("write topics");
        fs::write(
            dir.join("holdouts.json"),
            serde_json::to_vec(&serde_json::json!({
                DT: dt_recs,
                MUON: muon_recs,
            }))
            .expect("holdouts"),
        )
        .expect("write holdouts");
        fs::write(
            dir.join("baselines.json"),
            serde_json::to_vec(&serde_json::json!({
                DT: dt_meas,
                MUON: muon_meas,
            }))
            .expect("baselines"),
        )
        .expect("write baselines");
        fs::write(
            dir.join("offer.json"),
            serde_json::to_vec(&offer()).expect("offer"),
        )
        .expect("write offer");
        let pin_path = write_pin(&dir, &p);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind probe");
        let addr = listener.local_addr().expect("addr");
        drop(listener);

        let mut child = Command::new(env!("CARGO_BIN_EXE_proof-challenge"))
            .arg("--bind")
            .arg(addr.to_string())
            .arg("--force-sim")
            .arg("--pin-file")
            .arg(&pin_path)
            .arg("--topics-file")
            .arg(dir.join("topics.json"))
            .arg("--holdout-file")
            .arg(dir.join("holdouts.json"))
            .arg("--baseline-file")
            .arg(dir.join("baselines.json"))
            .arg("--inference-offer-file")
            .arg(dir.join("offer.json"))
            .env("PROOF_FORCE_SIM", "true")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn proof-challenge");

        let base = format!("http://{addr}");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("client");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if tokio::time::Instant::now() > deadline {
                let _ = child.start_kill();
                let out = child.wait_with_output().await.expect("wait");
                panic!(
                    "proof-challenge did not become healthy\nstderr: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            if client
                .get(format!("{base}/health"))
                .send()
                .await
                .ok()
                .is_some_and(|r| r.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Self { child, base, dir }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

async fn json(method: reqwest::Method, url: &str, body: Option<Value>) -> (u16, Value) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("client");
    let mut req = client.request(method, url);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.expect("http");
    let status = resp.status().as_u16();
    let v = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, v)
}

fn miner_sk() -> [u8; 32] {
    let mut s = [0x11u8; 32];
    s[0] = 0x42;
    s
}

fn submit_body(topic_id: &str, extra: &Value) -> Value {
    let mut v = serde_json::json!({
        "artifact_digest": digest(topic_id),
        "claim": "e2e sim claim + artifact + declared_flops",
        "declared_flops": 1_000_000u64,
        "topic_id": topic_id,
        "manifest": { "train_dataset_ids": ["e2e-mix-v0"] },
    });
    if let Some(obj) = extra.as_object() {
        if let Some(dst) = v.as_object_mut() {
            for (k, val) in obj {
                dst.insert(k.clone(), val.clone());
            }
        }
    }
    if extra.get("hotkey_signature").is_none() {
        proof_submit::attach_to_json(&mut v, &miner_sk()).expect("sign");
    }
    v
}

#[tokio::test]
async fn force_sim_binary_scores_staging_topic_ids() {
    let host = Host::spawn().await;
    let (st, status) = json(
        reqwest::Method::GET,
        &format!("{}/v1/status", host.base),
        None,
    )
    .await;
    assert_eq!(st, 200, "{status}");
    assert_eq!(status["challenge_id"], "proof");
    assert_eq!(status["can_score"], true, "{status}");
    assert_eq!(status["eval_backend"], "sim", "{status}");
    assert_eq!(status["force_sim"], true, "{status}");
    assert_eq!(status["sim_stub_win"], true, "{status}");
    assert_eq!(status["baseline_sealed"], true, "{status}");
    assert_eq!(status["inference_offer"]["offer_id"], OFFER_ID);
    let open = status["open_topics"].as_array().expect("open_topics");
    let ids: Vec<&str> = open.iter().filter_map(Value::as_str).collect();
    assert!(ids.contains(&DT), "{status}");
    assert!(ids.contains(&MUON), "{status}");
    assert!(!status.to_string().contains("api_key"), "{status}");
    assert!(!status.to_string().contains("8000"), "{status}");

    let (st, topics) = json(
        reqwest::Method::GET,
        &format!("{}/v1/proof/topics", host.base),
        None,
    )
    .await;
    assert_eq!(st, 200, "{topics}");
    assert!(!topics.to_string().contains("content_sha256"), "{topics}");

    for topic_id in [DT, MUON] {
        let (st, created) = json(
            reqwest::Method::POST,
            &format!("{}/v1/submissions", host.base),
            Some(submit_body(topic_id, &serde_json::json!({}))),
        )
        .await;
        assert_eq!(st, 201, "{topic_id}: {created}");
        assert!(
            created["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("pf_")),
            "{created}"
        );
        assert_eq!(created["topic_id"], topic_id);
        assert_eq!(created["eval_backend"], "sim");
        assert_eq!(created["state"], "awaiting_admin", "{created}");
        assert_eq!(created["eligible"], true, "{created}");

        let id = created["id"].as_str().expect("id");
        let (st, row) = json(
            reqwest::Method::GET,
            &format!("{}/v1/submissions/{id}", host.base),
            None,
        )
        .await;
        assert_eq!(st, 200, "{row}");
        assert_eq!(row["declared_flops"], 1_000_000);
        assert!(row["verdict"]["agent"].is_object(), "judge missing: {row}");
        assert!(
            row["verdict"]["harness"]["holdout_nll"].is_number(),
            "{row}"
        );
        assert_eq!(row["verdict"]["pass"], true, "{row}");
        assert_eq!(row["verdict"]["agent"]["rationale"], "sim stub win");
        assert!(
            row["receipt_json"]
                .as_str()
                .is_some_and(|s| s.contains("sim")),
            "{row}"
        );
    }

    let (st, bad) = json(
        reqwest::Method::POST,
        &format!("{}/v1/submissions", host.base),
        // A real (non-empty-input) artefact digest: the digest of nothing is
        // its own 400, and this probe is about the missing topic id.
        Some(submit_body(
            "",
            &serde_json::json!({ "topic_id": "", "artifact_digest": digest("no-topic-probe") }),
        )),
    )
    .await;
    assert_eq!(st, 400, "{bad}");
    assert_eq!(bad["error"], "topic_id is required");
}
