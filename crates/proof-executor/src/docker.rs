use std::{path::Path, time::Duration};

use docker_engine::RunSpec;
use proof_autonomy::is_digest;
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{store::Intent, Failure};

const SUPERVISOR: &str = include_str!("supervisor.py");
/// Where the workload leaves its model artifact for the trusted observer.
pub const ARTIFACT_DIR: &str = "/work/artifact";
const MAX_ARTIFACT_BYTES: usize = proof_measure::MAX_ARTIFACT_BYTES;

/// Local CPU-only sandbox. It never pulls images, mounts host files, forwards
/// environment/credentials, exposes Docker, or attaches to a provider-supplied name.
#[derive(Clone)]
pub struct DockerSandbox {
    http: Client,
    pub(crate) engine_id: String,
    pub(crate) image_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunObservation {
    pub schema_version: u32,
    pub seed: Option<u64>,
    pub script_digest: Option<String>,
    pub exit_code: Option<i32>,
    pub wall_ms: u64,
    pub failure: Option<String>,
    pub log: Vec<u8>,
    pub flops_used: Option<u64>,
    pub metrics: Option<Value>,
}

impl DockerSandbox {
    /// The Unix socket must belong to the trusted local daemon, not an agent.
    ///
    /// # Errors
    /// Non-digest image, missing exact local image, wrong daemon or API failure.
    pub async fn connect(socket: &Path, image_id: &str) -> Result<Self, Failure> {
        if !image_id.strip_prefix("sha256:").is_some_and(is_digest) || !socket.is_absolute() {
            return Err(Failure::Target);
        }
        let http = Client::builder()
            .unix_socket(socket)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| Failure::Target)?;
        let mut target = Self {
            http,
            engine_id: String::new(),
            image_id: image_id.into(),
        };
        let image = target
            .json(Method::GET, &format!("/images/{image_id}/json"), None)
            .await?;
        if image.get("Id").and_then(Value::as_str) != Some(image_id) {
            return Err(Failure::Target);
        }
        let info = target.json(Method::GET, "/info", None).await?;
        target.engine_id = info
            .get("ID")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or(Failure::Target)?
            .into();
        Ok(target)
    }

    pub(crate) fn name(id: Uuid, index: usize) -> String {
        format!("base-proof-exec-{id}-{index}")
    }

    pub(crate) async fn create(
        &self,
        intent: &Intent,
        index: usize,
        mut payload: Value,
    ) -> Result<(), Failure> {
        payload["deadline_ms"] = json!(intent.deadline_ms);
        let name = Self::name(intent.id, index);
        let spec = RunSpec::design_hardened(
            name.clone(),
            self.image_id.clone(),
            vec![
                "/usr/local/bin/python".into(),
                "-I".into(),
                "-u".into(),
                "-c".into(),
                SUPERVISOR.into(),
                serde_json::to_string(&payload).map_err(|_| Failure::Input)?,
            ],
        );
        let body = json!({
            "Image": spec.image, "Entrypoint": spec.cmd, "Cmd": [],
            "Labels": labels(intent), "User": "0:0", "WorkingDir": "/",
            "NetworkDisabled": true, "AttachStdout": true, "AttachStderr": true,
            // Anonymous volume, not tmpfs: the artifact must survive container
            // exit so the controller can archive it before removal.
            "Volumes": {ARTIFACT_DIR: {}},
            "HostConfig": {
                "ReadonlyRootfs": spec.readonly_rootfs, "CapDrop": ["ALL"],
                // Only the trusted PID-1 supervisor can switch the workload uid.
                "CapAdd": ["SETUID", "SETGID"],
                "SecurityOpt": ["no-new-privileges:true"], "NetworkMode": "none",
                "PidsLimit": 64, "Memory": 268_435_456, "MemorySwap": 268_435_456,
                "NanoCpus": 1_000_000_000, "IpcMode": "private", "RestartPolicy": {"Name": "no"},
                "Tmpfs": {"/work": "rw,noexec,nosuid,nodev,size=32m,mode=1777",
                          "/run/proof": "rw,noexec,nosuid,nodev,size=1m,mode=0755"},
                "LogConfig": {"Type": "local", "Config": {"max-size": "1m", "max-file": "1", "compress": "false"}}
            }
        });
        self.json(
            Method::POST,
            &format!("/containers/create?name={name}"),
            Some(body),
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn start(&self, intent: &Intent, index: usize) -> Result<(), Failure> {
        self.inspect(intent, index).await?.ok_or(Failure::Target)?;
        self.json(
            Method::POST,
            &format!("/containers/{}/start", Self::name(intent.id, index)),
            None,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn inspect(
        &self,
        intent: &Intent,
        index: usize,
    ) -> Result<Option<Value>, Failure> {
        let path = format!("/containers/{}/json", Self::name(intent.id, index));
        let (status, bytes) = self.request(Method::GET, &path, None).await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let value = decode(status, &bytes)?;
        if value.get("Image").and_then(Value::as_str) != Some(&self.image_id)
            || value.pointer("/Config/Labels") != Some(&labels(intent))
        {
            return Err(Failure::Target);
        }
        Ok(Some(value))
    }

    pub(crate) async fn stopped(&self, intent: &Intent, index: usize) -> Result<bool, Failure> {
        let value = self.inspect(intent, index).await?.ok_or(Failure::Target)?;
        match value.pointer("/State/Status").and_then(Value::as_str) {
            Some("exited") => Ok(true),
            Some("created" | "running") => Ok(false),
            _ => Err(Failure::Target),
        }
    }

    pub(crate) async fn logs(&self, intent: &Intent, index: usize) -> Result<Vec<u8>, Failure> {
        let path = format!(
            "/containers/{}/logs?stdout=1&stderr=1",
            Self::name(intent.id, index)
        );
        let (status, frames) = self.request(Method::GET, &path, None).await?;
        if !status.is_success() {
            return Err(Failure::Target);
        }
        unframe(&frames)
    }

    /// Tar of the workload's artifact directory, read by the controller only
    /// after the container stopped. Bounded; oversize is a retained failure.
    pub(crate) async fn artifact(&self, intent: &Intent, index: usize) -> Result<Vec<u8>, Failure> {
        if !self.stopped(intent, index).await? {
            return Err(Failure::Target);
        }
        let path = format!(
            "/containers/{}/archive?path={ARTIFACT_DIR}",
            Self::name(intent.id, index)
        );
        let response = self
            .http
            .get(format!("http://localhost{path}"))
            .send()
            .await
            .map_err(|_| Failure::Target)?;
        if !response.status().is_success() {
            return Err(Failure::Target);
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Target)? {
            if bytes.len() + chunk.len() > MAX_ARTIFACT_BYTES {
                return Err(Failure::LogLimit);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    pub(crate) async fn interrupt(&self, intent: &Intent, index: usize) -> Result<(), Failure> {
        if self.inspect(intent, index).await?.is_none() || self.stopped(intent, index).await? {
            return Ok(());
        }
        let path = format!(
            "/containers/{}/kill?signal=SIGTERM",
            Self::name(intent.id, index)
        );
        let (status, _) = self.request(Method::POST, &path, None).await?;
        if !status.is_success() {
            return Err(Failure::Target);
        }
        // The supervisor catches SIGTERM, emits its bounded receipt and exits
        // PID 1. A refused signal never counts as a confirmed target stop.
        for _ in 0..20 {
            if self.stopped(intent, index).await? {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Err(Failure::StopUnconfirmed)
    }

    /// Missing is only absence at the observation time. Reconciliation does not
    /// release an uncertain intent until its original deadline has passed.
    pub(crate) async fn remove(&self, intent: &Intent, index: usize) -> Result<(), Failure> {
        if self.inspect(intent, index).await?.is_none() {
            return Ok(());
        }
        let path = format!(
            "/containers/{}?force=true&v=true",
            Self::name(intent.id, index)
        );
        let (status, _) = self.request(Method::DELETE, &path, None).await?;
        if !status.is_success() || self.inspect(intent, index).await?.is_some() {
            return Err(Failure::StopUnconfirmed);
        }
        Ok(())
    }

    async fn json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, Failure> {
        let (status, bytes) = self.request(method, path, body).await?;
        decode(status, &bytes)
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, Vec<u8>), Failure> {
        let mut request = self.http.request(method, format!("http://localhost{path}"));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request.send().await.map_err(|_| Failure::Target)?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Target)? {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err(Failure::LogLimit);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((status, bytes))
    }
}

fn labels(intent: &Intent) -> Value {
    json!({"cortex.proof.execution": intent.id.to_string(),
        "cortex.proof.experiment": intent.experiment_id.to_string(),
        "cortex.proof.fence": intent.controller_fence.to_string(),
        "cortex.proof.operation": intent.operation_key})
}

fn decode(status: StatusCode, bytes: &[u8]) -> Result<Value, Failure> {
    if !status.is_success() {
        return Err(Failure::Target);
    }
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(bytes).map_err(|_| Failure::Target)
}

fn unframe(mut frames: &[u8]) -> Result<Vec<u8>, Failure> {
    let mut bytes = Vec::new();
    while !frames.is_empty() {
        if frames.len() < 8 || ![1, 2].contains(&frames[0]) || frames[1..4] != [0; 3] {
            return Err(Failure::Target);
        }
        let len =
            u32::from_be_bytes(frames[4..8].try_into().map_err(|_| Failure::Target)?) as usize;
        let data = frames.get(8..8 + len).ok_or(Failure::Target)?;
        bytes.extend_from_slice(data);
        frames = &frames[8 + len..];
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "docker_tests.rs"]
mod tests;
