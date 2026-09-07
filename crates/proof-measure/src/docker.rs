use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use proof_eval::{ProofEvalDocument, PROOF_METRICS_SCHEMA};
use proof_harvest::{HarvestRequest, PROGRAM};
use proof_research::artifact_digest;
use proof_task::{
    resolve_inference, verify_holdout, HoldoutRecord, InferenceOffer, TopicDocument, CHALLENGE_ID,
};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};

use crate::{
    judge::{network_body, network_name, proxy_body, proxy_name, ready_deadline, JudgeUpstream},
    validate_document, MeasurementObserver, MeasurementRequest, Observation, ObserverError,
};

/// Controller-owned judge egress for the scoring container.
///
/// The scoring image executes untrusted miner code beside the operator holdout,
/// so it is never given plain internet. Instead the run gets a private
/// `internal` network whose only other member is a pinned proxy that reaches
/// exactly one upstream origin and holds the API key. Leave this unset to keep
/// the container fully network-free.
#[derive(Debug, Clone)]
pub struct JudgeEgress {
    /// Locally present, digest-pinned proxy image.
    pub proxy_image: String,
    /// Absolute `0600` file holding the judge API key. It is read by the
    /// controller and staged into the proxy; it is never bind-mounted, and the
    /// key never reaches the scoring container.
    pub api_key_file: PathBuf,
    /// Allow a controller-side judge on literal loopback. Only the proxy gets
    /// the host-gateway mapping needed to reach it; the scoring container never
    /// has a route to the host.
    pub allow_loopback_upstream: bool,
}

const INPUT: &str = "/run/proof/in";
const OUTPUT: &str = "/run/proof/out";
const HOLDOUT_MOUNT: &str = "/opt/proof-eval/holdout";
const MAX_LOG: usize = 1024 * 1024;
const POLL: Duration = Duration::from_millis(100);

/// Controller-launched scoring container over a pinned, locally present
/// image. It never pulls, never has network, mounts only the operator holdout
/// store (read-only), and is the only process that reads the captured artifact.
#[derive(Clone)]
pub struct DockerObserver {
    http: Client,
    /// `sha256:…` the pin names; must equal the requesting pin's digest.
    pin_digest: String,
    /// Daemon image ID the reference resolved to.
    image_id: String,
    holdout_store: PathBuf,
    holdouts: BTreeMap<String, Vec<HoldoutRecord>>,
    offer: InferenceOffer,
    /// `None` keeps the scoring container network-free; the live image then
    /// refuses to score, which is the correct fail-closed outcome.
    egress: Option<(JudgeEgress, String)>,
}

impl DockerObserver {
    /// `image` is `sha256:<id>` or `<repo>@sha256:<digest>`; both resolve
    /// locally only. `holdouts` is operator state keyed by topic id.
    ///
    /// # Errors
    /// Unpinned reference, absent local image, relative paths or daemon failure.
    pub async fn connect(
        socket: &Path,
        image: &str,
        holdout_store: PathBuf,
        holdouts: BTreeMap<String, Vec<HoldoutRecord>>,
        offer: InferenceOffer,
    ) -> Result<Self, ObserverError> {
        let pin_digest = image
            .rsplit_once('@')
            .map_or(image, |(_, digest)| digest)
            .to_owned();
        if !pin_digest
            .strip_prefix("sha256:")
            .is_some_and(proof_autonomy::is_digest)
            || !socket.is_absolute()
            || !holdout_store.is_absolute()
            || !holdout_store.is_dir()
        {
            return Err(ObserverError::Target);
        }
        let http = Client::builder()
            .unix_socket(socket)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| ObserverError::Target)?;
        let mut observer = Self {
            http,
            pin_digest,
            image_id: String::new(),
            holdout_store,
            holdouts,
            offer,
            egress: None,
        };
        let inspected = observer
            .json(Method::GET, &format!("/images/{image}/json"), None)
            .await?;
        inspected
            .get("Id")
            .and_then(Value::as_str)
            .filter(|id| id.starts_with("sha256:"))
            .ok_or(ObserverError::Target)?
            .clone_into(&mut observer.image_id);
        Ok(observer)
    }

    /// Allow the scoring container to reach the pinned judge, and only that.
    ///
    /// The proxy image must already be present locally and the key file must be
    /// an absolute regular file; the key is bound into the proxy alone and is
    /// never placed in the scoring container's environment.
    ///
    /// # Errors
    /// Unpinned/absent proxy image, non-absolute or non-regular key file, or an
    /// offer whose `base_url` is not a public HTTP(S) origin.
    pub async fn with_judge_egress(mut self, egress: JudgeEgress) -> Result<Self, ObserverError> {
        if !egress.api_key_file.is_absolute()
            || !std::fs::metadata(&egress.api_key_file)
                .map_err(|_| ObserverError::Target)?
                .is_file()
        {
            return Err(ObserverError::Target);
        }
        // Refuse now rather than at first score if the offer is unreachable.
        JudgeUpstream::parse_allowing_loopback(
            &self.offer.provider.base_url,
            egress.allow_loopback_upstream,
        )?;
        let inspected = self
            .json(
                Method::GET,
                &format!("/images/{}/json", egress.proxy_image),
                None,
            )
            .await?;
        let image_id = inspected
            .get("Id")
            .and_then(Value::as_str)
            .filter(|id| id.starts_with("sha256:"))
            .ok_or(ObserverError::Target)?
            .to_owned();
        self.egress = Some((egress, image_id));
        Ok(self)
    }

    fn name(request: &MeasurementRequest) -> String {
        format!(
            "base-proof-measure-{}-{}",
            request.intent_id, request.run_index
        )
    }

    fn harvest_request(
        &self,
        request: &MeasurementRequest,
        holdout: Vec<HoldoutRecord>,
    ) -> Result<HarvestRequest, ObserverError> {
        let topic = &request.topic;
        let resolved = resolve_inference(
            &request.pin,
            Some(&topic.inference),
            None,
            Some(&self.offer),
        );
        if !resolved.ready_to_score() {
            return Err(ObserverError::Request);
        }
        Ok(HarvestRequest {
            schema_version: PROOF_METRICS_SCHEMA,
            challenge_id: CHALLENGE_ID.to_owned(),
            submission_digest: request.submission_digest()?,
            artifact_digest: request.artifact_digest.clone(),
            topic_id: topic.id.clone(),
            family: topic.metric.family.as_str().to_owned(),
            inference_offer_id: self.offer.offer_id.clone(),
            provider_kind: resolved.provider.as_str().to_owned(),
            // With egress the container is told the proxy's address, never the
            // real judge origin; without it the original URL is unreachable
            // anyway, so the image fails closed.
            base_url: match &self.egress {
                Some((egress, _)) => JudgeUpstream::parse_allowing_loopback(
                    &resolved.base_url,
                    egress.allow_loopback_upstream,
                )?
                .rewritten(&resolved.base_url),
                None => resolved.base_url,
            },
            mode: resolved.mode.as_str().to_owned(),
            model_ref: resolved.model,
            max_input_tokens: resolved
                .max_input_tokens
                .min(self.offer.config.max_input_tokens),
            max_output_tokens: resolved
                .max_output_tokens
                .min(self.offer.config.max_output_tokens),
            config_commitment: self.offer.config_commitment.clone(),
            eval_image_digest: request.pin.eval_image_digest.clone(),
            holdout_commitment: topic.holdout_commitment.clone(),
            constraints: topic.constraints,
            flops_budget: topic.flops_budget,
            wall_budget_s: topic.metric.wall_budget_s,
            claim: format!(
                "controller reproduction seed={} script={}",
                request.seed, request.script_digest
            ),
            holdout,
        })
    }

    async fn run(
        &self,
        name: &str,
        request: &MeasurementRequest,
    ) -> Result<Vec<u8>, ObserverError> {
        let holdout = self.holdout(&request.topic)?;
        let harvest = serde_json::to_vec(&self.harvest_request(request, holdout)?)
            .map_err(|_| ObserverError::Request)?;
        self.start_egress(name, request).await?;
        let mut env = vec![
            format!("PROOF_HOLDOUT_STORE={HOLDOUT_MOUNT}"),
            format!("PROOF_ARTIFACT_DIR={INPUT}/artifact"),
        ];
        if self.egress.is_some() {
            env.push("PROOF_JUDGE_PROXY=1".to_owned());
        }
        let body = json!({
            "Image": self.image_id,
            "Cmd": ["score", "--request", format!("{INPUT}/request.json"), "--out", format!("{OUTPUT}/metrics.json")],
            "Labels": {"cortex.proof.measure": request.intent_id.to_string(),
                       "cortex.proof.experiment": request.experiment_id.to_string()},
            "Env": env,
            "WorkingDir": OUTPUT, "NetworkDisabled": self.egress.is_none(), "AttachStdout": true, "AttachStderr": true,
            "Volumes": {INPUT: {}},
            "HostConfig": {
                "ReadonlyRootfs": true, "CapDrop": ["ALL"], "SecurityOpt": ["no-new-privileges:true"],
                // Either no network at all, or a private internal network whose
                // only peer is the judge proxy. Never a routable network.
                "NetworkMode": self.egress.as_ref().map_or_else(|| "none".to_owned(), |_| network_name(name)),
                "Dns": [], "DnsSearch": [], "ExtraHosts": [],
                "PidsLimit": 256, "IpcMode": "private", "RestartPolicy": {"Name": "no"},
                "Memory": 4_294_967_296_u64, "MemorySwap": 4_294_967_296_u64,
                "Binds": [format!("{}:{HOLDOUT_MOUNT}:ro", self.holdout_store.display())],
                "Tmpfs": {OUTPUT: "rw,noexec,nosuid,nodev,size=16m,mode=1777", "/tmp": "rw,noexec,nosuid,nodev,size=256m,mode=1777"},
                "LogConfig": {"Type": "local", "Config": {"max-size": "1m", "max-file": "1", "compress": "false"}}
            }
        });
        self.json(
            Method::POST,
            &format!("/containers/create?name={name}"),
            Some(body),
        )
        .await?;
        let archive = format!("/containers/{name}/archive?path={INPUT}");
        self.put(&archive, &request.artifact).await?;
        self.put(&archive, &ustar("request.json", &harvest)).await?;
        self.json(Method::POST, &format!("/containers/{name}/start"), None)
            .await?;
        let started = tokio::time::Instant::now();
        let timeout = Duration::from_millis(request.timeout_ms);
        loop {
            let state = self
                .json(Method::GET, &format!("/containers/{name}/json"), None)
                .await?;
            if state.get("Image").and_then(Value::as_str) != Some(&self.image_id) {
                return Err(ObserverError::Target);
            }
            match state.pointer("/State/Status").and_then(Value::as_str) {
                Some("exited") => break,
                Some("created" | "running") => {}
                _ => return Err(ObserverError::Target),
            }
            let now = i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| ObserverError::Deadline)?
                    .as_millis(),
            )
            .map_err(|_| ObserverError::Deadline)?;
            if started.elapsed() > timeout || now >= request.deadline_ms {
                let _ = self
                    .request(Method::POST, &format!("/containers/{name}/kill"), None)
                    .await;
                return Err(ObserverError::Deadline);
            }
            tokio::time::sleep(POLL).await;
        }
        let (status, frames) = self
            .request(
                Method::GET,
                &format!("/containers/{name}/logs?stdout=1&stderr=1"),
                None,
            )
            .await?;
        if !status.is_success() {
            return Err(ObserverError::Target);
        }
        unframe(&frames)
    }

    /// Bring up the per-run internal network and the judge proxy on it. The
    /// proxy is dual-homed: the default bridge carries its upstream call, the
    /// internal network carries the scoring container's. The scoring container
    /// itself only ever joins the internal one.
    async fn start_egress(
        &self,
        name: &str,
        request: &MeasurementRequest,
    ) -> Result<(), ObserverError> {
        let Some((egress, proxy_image)) = &self.egress else {
            return Ok(());
        };
        let upstream = JudgeUpstream::parse_allowing_loopback(
            &self.offer.provider.base_url,
            egress.allow_loopback_upstream,
        )?;
        let labels = json!({
            "cortex.proof.measure": request.intent_id.to_string(),
            "cortex.proof.experiment": request.experiment_id.to_string(),
        });
        let network = network_name(name);
        let proxy = proxy_name(name);
        self.json(
            Method::POST,
            "/networks/create",
            Some(network_body(&network, &labels)),
        )
        .await?;
        // The proxy is created on the default bridge, which carries its
        // upstream call; the internal network is attached below and carries the
        // scoring container's. The scoring container joins only the latter.
        self.json(
            Method::POST,
            &format!("/containers/create?name={proxy}"),
            Some(proxy_body(
                proxy_image,
                &egress.api_key_file,
                &labels,
                egress.allow_loopback_upstream,
            )?),
        )
        .await?;
        // Stage upstream address and credential together into the proxy's
        // private volume. Reading the key here keeps the operator file `0600`
        // root-owned while the proxy runs as `nobody`, and keeps both values out
        // of any environment the workload or `docker inspect` can read.
        let key =
            std::fs::read_to_string(&egress.api_key_file).map_err(|_| ObserverError::Target)?;
        let key = key.trim();
        if key.is_empty() || key.len() > 4096 {
            return Err(ObserverError::Target);
        }
        self.put(
            &format!("/containers/{proxy}/archive?path={}", crate::judge::KEY_DIR),
            &ustar_mode(
                "judge.json",
                &crate::judge::proxy_config(&upstream, key),
                *b"0000400\0",
                65534,
            ),
        )
        .await?;
        // Alias the proxy to the hostname baked into the rewritten base_url.
        self.json(
            Method::POST,
            &format!("/networks/{network}/connect"),
            Some(json!({
                "Container": proxy,
                "EndpointConfig": {"Aliases": [crate::judge::JUDGE_HOST]}
            })),
        )
        .await?;
        self.json(Method::POST, &format!("/containers/{proxy}/start"), None)
            .await?;
        let (poll, timeout) = ready_deadline();
        let started = tokio::time::Instant::now();
        loop {
            let state = self
                .json(Method::GET, &format!("/containers/{proxy}/json"), None)
                .await?;
            if state.pointer("/State/Running").and_then(Value::as_bool) == Some(true) {
                return Ok(());
            }
            if started.elapsed() > timeout {
                return Err(ObserverError::Target);
            }
            tokio::time::sleep(poll).await;
        }
    }

    /// Remove the proxy and network regardless of how the run ended; a leaked
    /// internal network would otherwise outlive the measurement.
    async fn stop_egress(&self, name: &str) -> Result<(), ObserverError> {
        if self.egress.is_none() {
            return Ok(());
        }
        self.remove(&proxy_name(name)).await?;
        let (status, _) = self
            .request(
                Method::DELETE,
                &format!("/networks/{}", network_name(name)),
                None,
            )
            .await?;
        if !status.is_success() && status != StatusCode::NOT_FOUND {
            return Err(ObserverError::Target);
        }
        Ok(())
    }

    async fn remove(&self, name: &str) -> Result<(), ObserverError> {
        let (status, _) = self
            .request(
                Method::DELETE,
                &format!("/containers/{name}?force=true&v=true"),
                None,
            )
            .await?;
        if !status.is_success() && status != StatusCode::NOT_FOUND {
            return Err(ObserverError::Target);
        }
        Ok(())
    }

    async fn put(&self, path: &str, tar: &[u8]) -> Result<(), ObserverError> {
        let response = self
            .http
            .put(format!("http://localhost{path}"))
            .header("Content-Type", "application/x-tar")
            .body(tar.to_vec())
            .send()
            .await
            .map_err(|_| ObserverError::Target)?;
        if !response.status().is_success() {
            return Err(ObserverError::Target);
        }
        Ok(())
    }

    async fn json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ObserverError> {
        let (status, bytes) = self.request(method, path, body).await?;
        if !status.is_success() {
            return Err(ObserverError::Target);
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|_| ObserverError::Target)
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, Vec<u8>), ObserverError> {
        let mut request = self.http.request(method, format!("http://localhost{path}"));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request.send().await.map_err(|_| ObserverError::Target)?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ObserverError::Target)? {
            if bytes.len() + chunk.len() > MAX_LOG {
                return Err(ObserverError::LogLimit);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((status, bytes))
    }
}

#[async_trait]
impl MeasurementObserver for DockerObserver {
    fn holdout(&self, topic: &TopicDocument) -> Result<Vec<HoldoutRecord>, ObserverError> {
        let records = self.holdouts.get(&topic.id).ok_or(ObserverError::Holdout)?;
        verify_holdout(records, &topic.holdout_commitment, topic.holdout_size)
            .map_err(|_| ObserverError::Holdout)?;
        Ok(records.clone())
    }

    async fn measure(&self, request: MeasurementRequest) -> Result<Observation, ObserverError> {
        request.validate()?;
        if request.pin.eval_image_digest.trim() != self.pin_digest {
            return Err(ObserverError::Request);
        }
        let name = Self::name(&request);
        // A stale container, proxy or network under this name is never reused.
        self.remove(&name).await?;
        self.stop_egress(&name).await?;
        let result = self.run(&name, &request).await;
        let removed = self.remove(&name).await;
        // Always tear the egress down, even when the run or removal failed.
        let egress_removed = self.stop_egress(&name).await;
        let log = result?;
        removed?;
        egress_removed?;
        let stdout = String::from_utf8_lossy(&log);
        if !PROGRAM.ran_to_completion(&stdout) {
            return Err(ObserverError::Document);
        }
        let document = PROGRAM
            .extract_document(&stdout)
            .and_then(|body| ProofEvalDocument::from_json(body).ok())
            .ok_or(ObserverError::Document)?;
        validate_document(&document, &request)?;
        Ok(Observation {
            metrics: document.harness,
            flops_used: (document.agent.flops_used > 0).then_some(document.agent.flops_used),
            verdict: document.agent,
            observer_image: self.pin_digest.clone(),
            log_digest: artifact_digest(&log),
            log,
        })
    }
}

/// Single-file ustar archive; enough for the request document.
fn ustar(name: &str, bytes: &[u8]) -> Vec<u8> {
    ustar_mode(name, bytes, *b"0000444\0", 0)
}

/// Same, with an explicit mode and owner so a staged secret is readable only by
/// the unprivileged user the proxy runs as.
fn ustar_mode(name: &str, bytes: &[u8], mode: [u8; 8], owner: u32) -> Vec<u8> {
    let mut header = [0_u8; 512];
    header[..name.len().min(100)].copy_from_slice(&name.as_bytes()[..name.len().min(100)]);
    header[100..108].copy_from_slice(&mode);
    let id = format!("{owner:07o}\0");
    header[108..116].copy_from_slice(id.as_bytes());
    header[116..124].copy_from_slice(id.as_bytes());
    let size = format!("{:011o}\0", bytes.len());
    header[124..136].copy_from_slice(size.as_bytes());
    header[136..148].copy_from_slice(b"00000000000\0");
    header[148..156].copy_from_slice(b"        ");
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum = header.iter().map(|b| u32::from(*b)).sum::<u32>();
    header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    let mut tar = header.to_vec();
    tar.extend_from_slice(bytes);
    tar.resize(tar.len().div_ceil(512) * 512 + 1024, 0);
    tar
}

fn unframe(mut frames: &[u8]) -> Result<Vec<u8>, ObserverError> {
    let mut bytes = Vec::new();
    while !frames.is_empty() {
        if frames.len() < 8 || ![1, 2].contains(&frames[0]) || frames[1..4] != [0; 3] {
            return Err(ObserverError::Target);
        }
        let len = u32::from_be_bytes(frames[4..8].try_into().map_err(|_| ObserverError::Target)?)
            as usize;
        let data = frames.get(8..8 + len).ok_or(ObserverError::Target)?;
        bytes.extend_from_slice(data);
        frames = &frames[8 + len..];
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ustar_header_checksum_and_padding_are_valid() {
        let tar = ustar("request.json", b"{}");
        assert_eq!(tar.len(), 512 + 512 + 1024);
        assert_eq!(&tar[257..262], b"ustar");
        let mut header = tar[..512].to_vec();
        header[148..156].copy_from_slice(b"        ");
        let expected = header.iter().map(|b| u32::from(*b)).sum::<u32>();
        let stored = std::str::from_utf8(&tar[148..154]).unwrap_or("");
        assert_eq!(u32::from_str_radix(stored, 8).ok(), Some(expected));
        assert_eq!(&tar[512..514], b"{}");
    }

    #[test]
    fn unframe_rejects_truncated_and_foreign_streams() {
        assert!(unframe(&[1, 0, 0, 0, 0, 0, 0, 9, b'a']).is_err());
        assert!(unframe(&[3, 0, 0, 0, 0, 0, 0, 1, b'a']).is_err());
        assert_eq!(
            unframe(&[1, 0, 0, 0, 0, 0, 0, 2, b'o', b'k']).ok(),
            Some(b"ok".to_vec())
        );
    }
}
