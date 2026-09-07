//! Allowlist-safe Weights & Biases publisher for Proof [`PublicEvidence`].
//!
//! The official `wandb` SDK is **forbidden** here: `wandb.init()` (audited at
//! v0.28.0) unconditionally uploads telemetry (`_wandb.t`: python version,
//! CLI version, platform, imported modules) through `UpsertBucket`, and no
//! setting suppresses it. This crate instead drives the artifact write path
//! directly: plain GraphQL over HTTP Basic auth (`api:<key>`) at
//! `{base}/graphql`, so nothing leaves the process beyond the seven public
//! fields of the evidence document, the fixed `cortex-proof-wandb/1`
//! user agent, and the operator's entity/project names.
//!
//! Sequence: `createArtifact` (dedup on `COMMITTED`) → `createArtifactManifest`
//! (placeholder) → `createArtifactFiles` + PUT `record.json` →
//! `createArtifactManifest` (real digest, `includeUpload`) + PUT manifest →
//! `commitArtifact` → readback `artifact(name)` and compare state and digest.
//!
//! **Unverified:** the client schema declares `runName: String!` on
//! `createArtifactManifest`; whether the server accepts `null` has not been
//! probed. With no configured `run_name` this crate sends `null` and fails
//! closed on any GraphQL error. An operator may pre-create a bare run and set
//! `run_name`; runs are never created here.
//!
//! The live probe test is opt-in via `CORTEX_TEST_WANDB_CONFIG` and `--ignored`.

#![forbid(unsafe_code)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use md5::{Digest, Md5};
use proof_autonomy::commitment;
use proof_research::{EvidencePublisher, PublicEvidence, ResearchError};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};

pub const USER_AGENT: &str = "cortex-proof-wandb/1";
pub const ARTIFACT_TYPE: &str = "proof-evidence";
pub const RECORD_NAME: &str = "record.json";
const MANIFEST_NAME: &str = "wandb_manifest.json";

/// Operator configuration. The API key lives in a private file, never inline.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WandbConfig {
    /// `https://api.wandb.ai` or a self-hosted origin; `http` only for loopback
    /// and only with `allow_loopback_http`.
    pub base_url: String,
    pub entity: String,
    pub project: String,
    /// Mode `0600` file containing the API key; read once at construction.
    pub api_key_file: PathBuf,
    /// Optional pre-existing bare run to attach manifests to (see crate doc).
    #[serde(default)]
    pub run_name: Option<String>,
    #[serde(default = "default_timeout", with = "secs")]
    pub timeout: Duration,
    #[serde(default)]
    pub allow_loopback_http: bool,
}

fn default_timeout() -> Duration {
    Duration::from_secs(20)
}

mod secs {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        u64::deserialize(d).map(Duration::from_secs)
    }
}

/// Construction errors carry no secret material by construction.
#[derive(Debug, thiserror::Error)]
pub enum WandbError {
    #[error("wandb base_url must be https (or loopback http when explicitly allowed)")]
    BaseUrl,
    #[error("wandb entity/project must be non-empty simple names")]
    Names,
    #[error("wandb api_key_file must be a private (0600) file with one token")]
    ApiKeyFile,
    #[error("wandb http client could not be built")]
    Client,
}

/// Publishes one `record.json` per evidence digest as a W&B artifact.
pub struct WandbPublisher {
    client: reqwest::Client,
    graphql: Url,
    entity: String,
    project: String,
    run_name: Option<String>,
    api_key: String,
}

impl std::fmt::Debug for WandbPublisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WandbPublisher")
            .field("graphql", &self.graphql.as_str())
            .field("entity", &self.entity)
            .field("project", &self.project)
            .field("run_name", &self.run_name)
            .finish_non_exhaustive()
    }
}

fn simple_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn b64_md5(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(Md5::digest(bytes))
}

/// Hex MD5 the server derives for a single-entry `wandb-storage-policy` manifest.
#[must_use]
pub fn manifest_digest(record_b64_md5: &str) -> String {
    let mut h = Md5::new();
    h.update(b"wandb-artifact-manifest-v1\n");
    h.update(RECORD_NAME.as_bytes());
    h.update(b":");
    h.update(record_b64_md5.as_bytes());
    h.update(b"\n");
    hex::encode(h.finalize())
}

/// Canonical JSON bytes of exactly the seven public fields.
///
/// # Errors
/// Encoding failure (non-finite float).
pub fn record_bytes(document: &PublicEvidence) -> Result<Vec<u8>, ResearchError> {
    let value = serde_json::to_value(document).map_err(|_| ResearchError::Publication)?;
    Ok(proof_task::canonical_json(&value).into_bytes())
}

impl WandbPublisher {
    /// Validate configuration, read the key once, and build the client.
    ///
    /// # Errors
    /// Non-https origin, bad names, unreadable or world-readable key file.
    pub fn new(config: WandbConfig) -> Result<Self, WandbError> {
        let base = Url::parse(&config.base_url).map_err(|_| WandbError::BaseUrl)?;
        let loopback = match base.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            Some(url::Host::Domain(d)) => d == "localhost",
            None => false,
        };
        let scheme_ok = base.scheme() == "https"
            || (base.scheme() == "http" && loopback && config.allow_loopback_http);
        if !scheme_ok || !base.query().is_none_or(str::is_empty) {
            return Err(WandbError::BaseUrl);
        }
        if !simple_name(&config.entity)
            || !simple_name(&config.project)
            || !config.run_name.as_deref().is_none_or(simple_name)
        {
            return Err(WandbError::Names);
        }
        let meta = std::fs::metadata(&config.api_key_file).map_err(|_| WandbError::ApiKeyFile)?;
        if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
            return Err(WandbError::ApiKeyFile);
        }
        let api_key = std::fs::read_to_string(&config.api_key_file)
            .map_err(|_| WandbError::ApiKeyFile)?
            .trim()
            .to_owned();
        if api_key.is_empty()
            || api_key.len() > 256
            || !api_key.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(WandbError::ApiKeyFile);
        }
        let graphql = base.join("graphql").map_err(|_| WandbError::BaseUrl)?;
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(config.timeout)
            .build()
            .map_err(|_| WandbError::Client)?;
        Ok(Self {
            client,
            graphql,
            entity: config.entity,
            project: config.project,
            run_name: config.run_name,
            api_key,
        })
    }

    async fn graphql(&self, query: &str, variables: Value) -> Result<Value, ResearchError> {
        let response = self
            .client
            .post(self.graphql.clone())
            .basic_auth("api", Some(&self.api_key))
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(|_| ResearchError::Publication)?;
        if !response.status().is_success() {
            return Err(ResearchError::Publication);
        }
        let body: Value = response
            .json()
            .await
            .map_err(|_| ResearchError::Publication)?;
        if body.get("errors").is_some_and(|e| !e.is_null()) {
            return Err(ResearchError::Publication);
        }
        body.get("data").cloned().ok_or(ResearchError::Publication)
    }

    async fn upload(&self, target: &Value, bytes: Vec<u8>) -> Result<(), ResearchError> {
        let Some(url) = target.get("uploadUrl").and_then(Value::as_str) else {
            // A null upload URL means the server already holds these bytes.
            return Ok(());
        };
        let url = Url::parse(url).map_err(|_| ResearchError::Publication)?;
        if url.scheme() != "https" && url.scheme() != self.graphql.scheme() {
            return Err(ResearchError::Publication);
        }
        let mut headers = HeaderMap::new();
        for line in target
            .get("uploadHeaders")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let (name, value) = line.split_once(':').ok_or(ResearchError::Publication)?;
            headers.insert(
                HeaderName::from_bytes(name.trim().as_bytes())
                    .map_err(|_| ResearchError::Publication)?,
                HeaderValue::from_str(value.trim()).map_err(|_| ResearchError::Publication)?,
            );
        }
        let response = self
            .client
            .put(url)
            .headers(headers)
            .body(bytes)
            .send()
            .await
            .map_err(|_| ResearchError::Publication)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(ResearchError::Publication)
        }
    }

    async fn create_manifest(
        &self,
        artifact_id: &str,
        digest: &str,
        include_upload: bool,
    ) -> Result<Value, ResearchError> {
        let data = self
            .graphql(
                "mutation createArtifactManifest($input: CreateArtifactManifestInput!) { \
                 createArtifactManifest(input: $input) { artifactManifest { id file { uploadUrl uploadHeaders } } } }",
                json!({ "input": {
                    "artifactID": artifact_id,
                    "name": MANIFEST_NAME,
                    "digest": digest,
                    "entityName": self.entity,
                    "projectName": self.project,
                    "runName": self.run_name,
                    "type": "FULL",
                    "includeUpload": include_upload,
                }}),
            )
            .await?;
        data.pointer("/createArtifactManifest/artifactManifest")
            .cloned()
            .ok_or(ResearchError::Publication)
    }

    async fn readback(&self, collection: &str, alias: &str) -> Result<String, ResearchError> {
        let data = self
            .graphql(
                "query artifact($name: String!) { artifact(name: $name) { state digest } }",
                json!({ "name": format!("{}/{}/{collection}:{alias}", self.entity, self.project) }),
            )
            .await?;
        let artifact = data.get("artifact").ok_or(ResearchError::Publication)?;
        match (
            artifact.get("state").and_then(Value::as_str),
            artifact.get("digest").and_then(Value::as_str),
        ) {
            (Some("COMMITTED"), Some(digest)) => Ok(digest.to_owned()),
            _ => Err(ResearchError::Publication),
        }
    }

    /// Run the artifact sequence and return the server-confirmed manifest digest.
    ///
    /// # Errors
    /// Any transport, GraphQL, upload, or readback mismatch fails closed.
    pub async fn publish_artifact(
        &self,
        document: &PublicEvidence,
    ) -> Result<String, ResearchError> {
        let alias = document.evidence_digest.as_str();
        if !proof_autonomy::is_digest(alias) {
            return Err(ResearchError::Publication);
        }
        let collection = format!("proof-evidence-{}", &alias[..16]);
        let record = record_bytes(document)?;
        let record_len = record.len();
        let record_md5 = b64_md5(&record);
        let expected = manifest_digest(&record_md5);
        let metadata = String::from_utf8(record.clone()).map_err(|_| ResearchError::Publication)?;

        let created = self
            .graphql(
                "mutation createArtifact($input: CreateArtifactInput!) { \
                 createArtifact(input: $input) { artifact { id state artifactSequence { latestArtifact { id } } } } }",
                json!({ "input": {
                    "artifactTypeName": ARTIFACT_TYPE,
                    "artifactCollectionName": collection,
                    "entityName": self.entity,
                    "projectName": self.project,
                    "runName": null,
                    "digest": expected,
                    "digestAlgorithm": "MANIFEST_MD5",
                    "clientID": alias,
                    "sequenceClientID": alias,
                    "enableDigestDeduplication": true,
                    "metadata": metadata,
                    "description": format!("Cortex Proof public evidence {alias}"),
                    "aliases": [{ "artifactCollectionName": collection, "alias": alias }],
                }}),
            )
            .await?;
        let artifact = created
            .pointer("/createArtifact/artifact")
            .ok_or(ResearchError::Publication)?;
        let artifact_id = artifact
            .get("id")
            .and_then(Value::as_str)
            .ok_or(ResearchError::Publication)?
            .to_owned();
        if artifact.get("state").and_then(Value::as_str) != Some("COMMITTED") {
            let manifest = self.create_manifest(&artifact_id, "", false).await?;
            let manifest_id = manifest
                .get("id")
                .and_then(Value::as_str)
                .ok_or(ResearchError::Publication)?
                .to_owned();
            let files = self
                .graphql(
                    "mutation createArtifactFiles($input: CreateArtifactFilesInput!) { \
                     createArtifactFiles(input: $input) { files { edges { node { uploadUrl uploadHeaders } } } } }",
                    json!({ "input": {
                        "artifactFiles": [{
                            "artifactID": artifact_id,
                            "name": RECORD_NAME,
                            "md5": record_md5,
                            "artifactManifestID": manifest_id,
                        }],
                        "storageLayout": "V2",
                    }}),
                )
                .await?;
            let target = files
                .pointer("/createArtifactFiles/files/edges/0/node")
                .ok_or(ResearchError::Publication)?;
            self.upload(target, record).await?;
            let manifest_json = proof_task::canonical_json(&json!({
                "version": 1,
                "storagePolicy": "wandb-storage-policy",
                "storagePolicyConfig": {},
                "contents": { RECORD_NAME: { "digest": record_md5, "size": record_len } },
            }))
            .into_bytes();
            let manifest = self
                .create_manifest(&artifact_id, &b64_md5(&manifest_json), true)
                .await?;
            let target = manifest.get("file").ok_or(ResearchError::Publication)?;
            self.upload(target, manifest_json).await?;
            self.graphql(
                "mutation commitArtifact($input: CommitArtifactInput!) { \
                 commitArtifact(input: $input) { artifact { id digest } } }",
                json!({ "input": { "artifactID": artifact_id } }),
            )
            .await?;
        }
        let confirmed = self.readback(&collection, alias).await?;
        if confirmed != expected {
            return Err(ResearchError::Publication);
        }
        Ok(confirmed)
    }
}

#[async_trait]
impl EvidencePublisher for WandbPublisher {
    /// Returns the evidence commitment the store checks against, only after
    /// the remote artifact was confirmed committed with the expected digest.
    async fn publish(&self, document: &PublicEvidence) -> Result<String, ResearchError> {
        self.publish_artifact(document).await?;
        Ok(commitment(document)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Vector from `python3` hashlib over the same canonical record bytes.
    #[test]
    fn manifest_digest_matches_python_vector() {
        let record = record_bytes(&doc()).unwrap_or_default();
        assert_eq!(b64_md5(&record), "GVCCT9Hv5F/lfbxs4MFIeA==");
        assert_eq!(
            manifest_digest("GVCCT9Hv5F/lfbxs4MFIeA=="),
            "40980c6616cabca04c5c2349cf14a4ea"
        );
    }
}
