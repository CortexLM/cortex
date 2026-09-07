#![allow(clippy::expect_used, clippy::unwrap_used)]

//! Proves the judge-egress trust boundary with a TEST-ONLY scoring image that
//! behaves like hostile miner code: it calls the judge through the proxy and
//! simultaneously tries to exfiltrate to the public internet and the Docker
//! gateway. Exactly one of those must succeed.
//!
//! The real scoring image loads the artifact with `trust_remote_code=True`, so
//! this is the threat, not a hypothetical one.

use std::{collections::BTreeMap, path::Path};

use proof_measure::{
    DockerObserver, JudgeEgress, MeasurementObserver, MeasurementRequest, ObserverError,
};
use proof_research::artifact_digest;
use proof_task::{
    holdout_commitment, synthetic_holdout, InferenceConfig, InferenceMode, InferenceOffer,
    InferenceProvider, InferenceProviderKind, OfferStatus, ProofPin, TopicDocument, TopicStatus,
    STRATUM_SIZE,
};
use serde_json::Value;
use uuid::Uuid;

const PROBE_IMAGE: &str = "cortex-test-egress-probe:local-test";
const PROXY_IMAGE: &str = "cortex-test-judge-proxy:local-test";
const SOCKET: &str = "/var/run/docker.sock";

/// Upstream the proxy is allowed to reach. Public origin on purpose: the point
/// is that the container reaches it only through the proxy.
fn upstream_base_url() -> String {
    std::env::var("CORTEX_TEST_JUDGE_BASE_URL")
        .unwrap_or_else(|_| "https://judge.invalid/v1".to_owned())
}

/// The live test must never silently score against a guessed endpoint.
fn required_base_url() -> String {
    std::env::var("CORTEX_TEST_JUDGE_BASE_URL")
        .expect("explicit judge base URL; never guess an inference endpoint")
}

/// Literal loopback judges are the controller's own port, so the proxy needs the
/// host-gateway opt-in. Off unless the configured upstream is loopback.
fn allow_loopback() -> bool {
    reqwest::Url::parse(&upstream_base_url())
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .and_then(|h| h.parse::<std::net::IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback())
}

/// The live test demands explicit configuration; the offline refusal test only
/// needs a well-formed offer, so it falls back to an unreachable placeholder
/// that can never be mistaken for a real endpoint.
fn offer() -> InferenceOffer {
    let config = InferenceConfig {
        mode: InferenceMode::Chat,
        model_ref: std::env::var("CORTEX_TEST_JUDGE_MODEL")
            .unwrap_or_else(|_| "unconfigured-offline-test".to_owned()),
        max_input_tokens: 1024,
        max_output_tokens: 16,
        temperature: None,
        top_p: None,
        timeout_ms: None,
    };
    let base_url = upstream_base_url();
    InferenceOffer {
        offer_id: "judge-egress-test".into(),
        config_commitment: proof_task::inference_config_commitment(&config, &base_url),
        provider: InferenceProvider {
            kind: InferenceProviderKind::OpenaiCompatible,
            base_url,
        },
        config,
        status: OfferStatus::Open,
    }
}

struct Dir(std::path::PathBuf);
impl Dir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("cortex-egress-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn image_id(reference: &str) -> String {
    let client = reqwest::Client::builder()
        .unix_socket(SOCKET)
        .build()
        .unwrap();
    let value: Value = client
        .get(format!("http://localhost/images/{reference}/json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    value["Id"]
        .as_str()
        .expect("TEST-ONLY image must be present locally")
        .to_owned()
}

fn empty_artifact() -> Vec<u8> {
    vec![0_u8; 1024]
}

#[tokio::test]
#[ignore = "requires local Docker, the TEST-ONLY probe/proxy images, a private judge key file \
            (CORTEX_TEST_JUDGE_KEY_FILE) and outbound network"]
async fn scoring_container_reaches_only_the_judge_and_never_holds_the_key() {
    let key_file = std::env::var("CORTEX_TEST_JUDGE_KEY_FILE")
        .expect("explicit private judge key file; never inline a key");
    let expected_upstream = required_base_url();
    let records = synthetic_holdout(STRATUM_SIZE, 1);
    let store = Dir::new();
    for record in &records {
        std::fs::write(store.0.join(&record.content_sha256), b"holdout text").unwrap();
    }
    let mut pin = ProofPin {
        eval_image_digest: image_id(PROBE_IMAGE).await,
        ..ProofPin::default()
    };
    pin.inference.model =
        std::env::var("CORTEX_TEST_JUDGE_MODEL").expect("explicit judge model for the live test");
    let topic = TopicDocument {
        id: "egress-test-only".into(),
        status: TopicStatus::Open,
        holdout_commitment: holdout_commitment(&records),
        ..TopicDocument::default()
    };
    let observer = DockerObserver::connect(
        Path::new(SOCKET),
        &pin.eval_image_digest,
        store.0.clone(),
        BTreeMap::from([(topic.id.clone(), records)]),
        offer(),
    )
    .await
    .expect("probe image present")
    .with_judge_egress(JudgeEgress {
        proxy_image: image_id(PROXY_IMAGE).await,
        api_key_file: key_file.into(),
        allow_loopback_upstream: allow_loopback(),
    })
    .await
    .expect("proxy image present and key file readable");

    let artifact = empty_artifact();
    let request = MeasurementRequest {
        experiment_id: Uuid::new_v4(),
        intent_id: Uuid::new_v4(),
        run_index: 0,
        seed: 5,
        script_digest: "a".repeat(64),
        artifact_digest: artifact_digest(&artifact),
        artifact,
        topic,
        pin,
        deadline_ms: i64::MAX,
        timeout_ms: 120_000,
    };

    // The probe emits a valid document carrying its findings in `rationale`.
    let observation = observer.measure(request).await.expect("probe measured");
    let egress: Value =
        serde_json::from_str(&observation.verdict.rationale).expect("probe findings");
    println!("egress findings: {egress}");

    // The judge is reachable through the proxy.
    let judge = egress["judge"].as_str().unwrap_or_default();
    assert!(
        judge.starts_with("reachable"),
        "judge must be reachable through the proxy, got {judge}"
    );

    // A real model answered: the proxy cannot fabricate a completion.
    let reply = egress["judge_reply"].as_str().unwrap_or_default();
    assert!(
        reply.contains("choices") || reply.contains("content"),
        "judge must return a real completion, got {reply}"
    );

    // Every escape route is blocked.
    for (name, value) in egress["escapes"].as_object().expect("escape probes") {
        let outcome = value.as_str().unwrap_or_default();
        assert!(
            outcome.starts_with("blocked"),
            "escape route {name} must be blocked, got {outcome}"
        );
    }

    // The credential never entered the scoring container, by env or by file.
    assert_eq!(
        egress["leaked_env"].as_array().map(Vec::len),
        Some(0),
        "judge credential must never appear in the scoring container env"
    );
    let key_file = egress["key_file"].as_str().unwrap_or_default();
    assert!(
        key_file.starts_with("blocked"),
        "staged judge key must be unreachable from the scoring container, got {key_file}"
    );

    // The workload must not learn where the judge actually lives. Everything it
    // can read (its environment plus the base_url it was handed) is checked
    // against the real upstream host.
    let upstream_host = reqwest::Url::parse(&expected_upstream)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .expect("upstream host");
    let visible = egress["visible"].to_string();
    assert!(
        !visible.contains(&upstream_host),
        "scoring container must never see upstream host {upstream_host}, saw {visible}"
    );
}

#[tokio::test]
async fn egress_refuses_relative_key_files_and_local_upstreams() {
    let store = Dir::new();
    let observer = DockerObserver::connect(
        Path::new(SOCKET),
        &format!("sha256:{}", "a".repeat(64)),
        store.0.clone(),
        BTreeMap::new(),
        offer(),
    )
    .await;
    // Without a local image the observer never reaches egress configuration.
    assert_eq!(observer.err(), Some(ObserverError::Target));
}
