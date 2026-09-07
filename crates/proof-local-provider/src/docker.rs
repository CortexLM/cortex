use std::{path::Path, time::Duration};

use proof_autonomy::is_digest;
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::LocalError;

/// Read-mostly view of the trusted local daemon. It never creates containers;
/// experiment containers are created only by `proof-executor` under a journaled
/// intent. This client observes and force-removes them by experiment label.
#[derive(Clone)]
pub(crate) struct Daemon {
    http: Client,
    pub(crate) engine_id: String,
    pub(crate) image_id: String,
}

pub(crate) const EXPERIMENT_LABEL: &str = "cortex.proof.experiment";

impl Daemon {
    pub(crate) async fn connect(socket: &Path, image_id: &str) -> Result<Self, LocalError> {
        if !image_id.strip_prefix("sha256:").is_some_and(is_digest) || !socket.is_absolute() {
            return Err(LocalError::Target);
        }
        let http = Client::builder()
            .unix_socket(socket)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| LocalError::Target)?;
        let mut daemon = Self {
            http,
            engine_id: String::new(),
            image_id: image_id.into(),
        };
        daemon.engine_id = daemon.observe_engine().await?;
        Ok(daemon)
    }

    /// Re-observe the exact image and daemon identity; a swapped daemon or
    /// retagged image is refused rather than silently adopted.
    pub(crate) async fn observe_engine(&self) -> Result<String, LocalError> {
        let image = self
            .json(Method::GET, &format!("/images/{}/json", self.image_id), &[])
            .await?;
        if image.get("Id").and_then(Value::as_str) != Some(self.image_id.as_str()) {
            return Err(LocalError::Target);
        }
        let info = self.json(Method::GET, "/info", &[]).await?;
        let engine = info
            .get("ID")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or(LocalError::Target)?;
        if !self.engine_id.is_empty() && engine != self.engine_id {
            return Err(LocalError::Target);
        }
        Ok(engine.into())
    }

    /// Container ids carrying this experiment's label, including stopped ones,
    /// with whether each was created from the pinned image. A foreign
    /// container with a colliding label is reported but never removed.
    pub(crate) async fn labelled(
        &self,
        experiment: Uuid,
    ) -> Result<Vec<(String, bool)>, LocalError> {
        let filters = json!({"label": [format!("{EXPERIMENT_LABEL}={experiment}")]}).to_string();
        let list = self
            .json(
                Method::GET,
                "/containers/json",
                &[("all", "true"), ("filters", filters.as_str())],
            )
            .await?;
        let mut ids = Vec::new();
        for entry in list.as_array().ok_or(LocalError::Target)? {
            let id = entry
                .get("Id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or(LocalError::Target)?;
            let pinned =
                entry.get("ImageID").and_then(Value::as_str) == Some(self.image_id.as_str());
            ids.push((id.into(), pinned));
        }
        Ok(ids)
    }

    pub(crate) async fn force_remove(&self, id: &str) -> Result<(), LocalError> {
        let (status, _) = self
            .request(
                Method::DELETE,
                &format!("/containers/{id}"),
                &[("force", "true")],
            )
            .await?;
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(LocalError::Target)
    }

    async fn json(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Value, LocalError> {
        let (status, bytes) = self.request(method, path, query).await?;
        if !status.is_success() {
            return Err(LocalError::Target);
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|_| LocalError::Target)
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<(StatusCode, Vec<u8>), LocalError> {
        let mut response = self
            .http
            .request(method, format!("http://localhost{path}"))
            .query(query)
            .send()
            .await
            .map_err(|_| LocalError::Target)?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| LocalError::Target)? {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err(LocalError::Target);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((status, bytes))
    }
}
