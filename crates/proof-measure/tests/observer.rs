#![allow(clippy::expect_used, clippy::unwrap_used)]

//! Exercises `DockerObserver` against the TEST-ONLY image built from
//! `tests/fixtures/Dockerfile` (`cortex.test.observer=1`). That image is not
//! the real scoring image; it only honours the CLI contract deterministically.

use std::{collections::BTreeMap, path::Path};

use proof_measure::{DockerObserver, MeasurementObserver, MeasurementRequest, ObserverError};
use proof_research::artifact_digest;
use proof_task::{
    holdout_commitment, synthetic_holdout, InferenceConfig, InferenceMode, InferenceOffer,
    InferenceProvider, InferenceProviderKind, OfferStatus, ProofPin, TopicDocument, TopicStatus,
    STRATUM_SIZE,
};
use uuid::Uuid;

const IMAGE: &str = "cortex-test-observer:local-test";
const SOCKET: &str = "/var/run/docker.sock";

fn offer() -> InferenceOffer {
    let config = InferenceConfig {
        mode: InferenceMode::Chat,
        model_ref: "local-test-only".into(),
        max_input_tokens: 1024,
        max_output_tokens: 64,
        temperature: None,
        top_p: None,
        timeout_ms: None,
    };
    let base_url = "http://127.0.0.1:1/v1".to_owned();
    InferenceOffer {
        offer_id: "local-test-only".into(),
        config_commitment: proof_task::inference_config_commitment(&config, &base_url),
        provider: InferenceProvider {
            kind: InferenceProviderKind::OpenaiCompatible,
            base_url,
        },
        config,
        status: OfferStatus::Open,
    }
}

/// Single-file ustar tar the observer stages as `/run/proof/in/artifact/…`.
fn artifact_tar(nll: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, bytes, typeflag) in [
        ("artifact/", &b""[..], b'5'),
        ("artifact/nll.txt", nll.as_bytes(), b'0'),
    ] {
        let mut header = [0_u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..108].copy_from_slice(b"0000755\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", bytes.len()).as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[148..156].copy_from_slice(b"        ");
        header[156] = typeflag;
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let sum = header.iter().map(|b| u32::from(*b)).sum::<u32>();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(bytes);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.resize(out.len() + 1024, 0);
    out
}

struct Fixture {
    observer: DockerObserver,
    topic: TopicDocument,
    pin: ProofPin,
    _store: tempdir::Dir,
}

mod tempdir {
    pub struct Dir(pub std::path::PathBuf);
    impl Dir {
        pub fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("cortex-measure-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

async fn fixture(prime_store: bool) -> Fixture {
    let records = synthetic_holdout(STRATUM_SIZE, 1);
    let store = tempdir::Dir::new();
    if prime_store {
        for record in &records {
            std::fs::write(store.0.join(&record.content_sha256), b"shard text").unwrap();
        }
    }
    let image_id = image_id().await;
    let mut pin = ProofPin {
        eval_image_digest: image_id,
        ..ProofPin::default()
    };
    pin.inference.model = "local-test-only".into();
    let topic = TopicDocument {
        id: "observer-test-only".into(),
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
    .expect("test observer image present locally");
    Fixture {
        observer,
        topic,
        pin,
        _store: store,
    }
}

async fn image_id() -> String {
    let client = reqwest::Client::builder()
        .unix_socket(SOCKET)
        .build()
        .unwrap();
    let value: serde_json::Value = client
        .get(format!("http://localhost/images/{IMAGE}/json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    value["Id"].as_str().unwrap().to_owned()
}

fn request(f: &Fixture, nll: &str) -> MeasurementRequest {
    let artifact = artifact_tar(nll);
    MeasurementRequest {
        experiment_id: Uuid::new_v4(),
        intent_id: Uuid::new_v4(),
        run_index: 0,
        seed: 5,
        script_digest: "a".repeat(64),
        artifact_digest: artifact_digest(&artifact),
        artifact,
        topic: f.topic.clone(),
        pin: f.pin.clone(),
        deadline_ms: i64::MAX,
        timeout_ms: 30_000,
    }
}

#[tokio::test]
async fn unpinned_or_relative_inputs_never_reach_the_daemon() {
    let store = tempdir::Dir::new();
    assert_eq!(
        DockerObserver::connect(
            Path::new(SOCKET),
            "python:latest",
            store.0.clone(),
            BTreeMap::new(),
            offer()
        )
        .await
        .err(),
        Some(ObserverError::Target)
    );
    assert_eq!(
        DockerObserver::connect(
            Path::new("docker.sock"),
            &format!("sha256:{}", "a".repeat(64)),
            store.0.clone(),
            BTreeMap::new(),
            offer()
        )
        .await
        .err(),
        Some(ObserverError::Target)
    );
}

#[tokio::test]
#[ignore = "requires local Docker and the TEST-ONLY cortex-test-observer:local-test image"]
async fn test_image_measures_staged_artifact_and_reports_flops() {
    let f = fixture(true).await;
    let observation = f.observer.measure(request(&f, "1.25")).await.unwrap();
    assert_eq!(observation.flops_used, Some(1_000_000));
    assert!((observation.metrics.holdout_nll - 1.25).abs() < 1e-9);
    assert_eq!(observation.metrics.split_nll.len(), 5);
    assert_eq!(observation.observer_image, f.pin.eval_image_digest);
    assert_eq!(artifact_digest(&observation.log), observation.log_digest);
    let mut wrong = request(&f, "1.25");
    wrong.pin.eval_image_digest = format!("sha256:{}", "b".repeat(64));
    assert_eq!(
        f.observer.measure(wrong).await.err(),
        Some(ObserverError::Request)
    );
}

#[tokio::test]
#[ignore = "requires local Docker and the TEST-ONLY cortex-test-observer:local-test image"]
async fn missing_holdout_shards_or_commitment_refuse_measurement() {
    let f = fixture(false).await;
    assert_eq!(
        f.observer.measure(request(&f, "1.0")).await.err(),
        Some(ObserverError::Document)
    );
    let mut sealed = request(&f, "1.0");
    sealed.topic.holdout_commitment = "c".repeat(64);
    assert_eq!(
        f.observer.measure(sealed).await.err(),
        Some(ObserverError::Holdout)
    );
}
