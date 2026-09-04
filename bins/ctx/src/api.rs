//! HTTP client for the public Cortex gateway.
//!
//! Every miner-facing route is proxied by the gateway under
//! `/challenge/{challenge_id}/v1/...`, so one base URL covers both live
//! challenges.

use std::time::Duration;

use serde_json::Value;

/// Public gateway miners and validators talk to.
///
/// Docs, `--help`, and `scripts/install-ctx.sh` all name this host. Override
/// with `--gateway` only when you run your own stack.
pub const DEFAULT_GATEWAY: &str = "https://network.cortex.foundation";

/// Per-request timeout. Submits rent nothing synchronously, so this is short.
const TIMEOUT_SECS: u64 = 60;

/// One gateway reply: HTTP status plus a decoded JSON body.
pub struct Reply {
    /// HTTP status code.
    pub status: u16,
    /// Decoded body, or a string when the body was not JSON.
    pub body: Value,
}

impl Reply {
    /// Whether the gateway answered 2xx.
    #[must_use]
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Best-effort error string: the service's `error` field, else the body.
    #[must_use]
    pub fn message(&self) -> String {
        self.body
            .get("error")
            .and_then(Value::as_str)
            .map_or_else(|| self.body.to_string(), ToOwned::to_owned)
    }
}

/// Gateway client. Holds the miner's optional Lium key and never logs it.
pub struct Client {
    base: String,
    lium_key: Option<String>,
    http: reqwest::Client,
}

impl Client {
    /// Build a client for one gateway base URL.
    pub fn new(gateway: &str, lium_key: Option<String>) -> Result<Self, String> {
        let base = gateway.trim().trim_end_matches('/').to_owned();
        if !(base.starts_with("https://") || base.starts_with("http://")) {
            return Err(format!(
                "gateway must be an http(s) URL, got {base:?} (default is {DEFAULT_GATEWAY})"
            ));
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .user_agent(concat!("ctx/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Self {
            base,
            lium_key: lium_key.filter(|k| !k.trim().is_empty()),
            http,
        })
    }

    /// The resolved gateway base URL.
    #[must_use]
    pub fn gateway(&self) -> &str {
        &self.base
    }

    /// GET a gateway path (`/v1/...` or `/challenge/...`).
    pub async fn get(&self, path: &str) -> Result<Reply, String> {
        self.send(self.http.get(self.url(path))).await
    }

    /// POST JSON to a gateway path.
    pub async fn post(&self, path: &str, body: &Value) -> Result<Reply, String> {
        self.send(self.http.post(self.url(path)).json(body)).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Reply, String> {
        // Miner BYOK. Accepted and never logged by the challenge services, and
        // never printed by this CLI.
        let req = match &self.lium_key {
            Some(key) => req.header("X-Lium-Api-Key", key),
            None => req,
        };
        let resp = req.send().await.map_err(|e| {
            format!(
                "request to {} failed: {e}\n  check the gateway is reachable: {}/v1/weights/latest",
                self.base, self.base
            )
        })?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("read response body: {e}"))?;
        let body = serde_json::from_str::<Value>(&text)
            .unwrap_or_else(|_| Value::String(text.trim().to_owned()));
        Ok(Reply { status, body })
    }
}

/// Path of a challenge route behind the gateway proxy.
#[must_use]
pub fn challenge_path(challenge_id: &str, suffix: &str) -> String {
    format!("/challenge/{challenge_id}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_gateway_is_the_public_host() {
        assert_eq!(DEFAULT_GATEWAY, "https://network.cortex.foundation");
        assert!(!DEFAULT_GATEWAY.ends_with('/'));
    }

    #[test]
    fn trailing_slash_does_not_double_up() {
        let c = Client::new("https://network.cortex.foundation/", None).expect("client");
        assert_eq!(c.gateway(), "https://network.cortex.foundation");
        assert_eq!(
            c.url("/v1/weights/latest"),
            "https://network.cortex.foundation/v1/weights/latest"
        );
    }

    #[test]
    fn non_http_gateway_is_rejected() {
        assert!(Client::new("network.cortex.foundation", None).is_err());
    }

    #[test]
    fn blank_lium_key_is_dropped() {
        let c = Client::new(DEFAULT_GATEWAY, Some("   ".into())).expect("client");
        assert!(c.lium_key.is_none());
    }

    #[test]
    fn challenge_paths_are_proxy_paths() {
        assert_eq!(
            challenge_path("proof", "/v1/status"),
            "/challenge/proof/v1/status"
        );
    }

    #[test]
    fn an_error_reply_surfaces_the_service_message() {
        let r = Reply {
            status: 503,
            body: serde_json::json!({"error": "scoring unconfigured"}),
        };
        assert!(!r.ok());
        assert_eq!(r.message(), "scoring unconfigured");
    }
}
