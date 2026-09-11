//! HTTP client for the public Cortex gateway.
//!
//! Every miner-facing route is proxied by the gateway under
//! `/challenge/{challenge_id}/v1/...`, so one base URL covers both live
//! challenges.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::doc_markdown)]

use std::time::Duration;

use serde_json::Value;

/// Public gateway miners and validators talk to.
///
/// Docs, `--help`, and `scripts/install-ctx.sh` all name this host. Override
/// with `--gateway` only when you run your own stack.
pub const DEFAULT_GATEWAY: &str = "https://gateway.cortex.foundation";

/// GET / status / topic-list timeout. These routes return immediately.
pub const DEFAULT_GET_TIMEOUT_SECS: u64 = 60;

/// Proof POST / multipart default. Evaluate is **synchronous** and can run
/// for minutes (tbench); a client-wide 60 s timeout drops the TCP stream,
/// the gateway cancels upstream, and the host is left with an orphan
/// experiment VM and no score. `0` (via [`Client::with_submit_timeout_secs`])
/// waits until the host answers.
pub const DEFAULT_SUBMIT_TIMEOUT_SECS: u64 = 7200;

/// Clear fail-closed message when a miner key would go out over cleartext HTTP.
const KEYED_HTTP_REFUSAL: &str =
    "refusing to send X-Lium-Api-Key over http:// — keyed calls require an https:// gateway";

/// Same floor as [`KEYED_HTTP_REFUSAL`] for Proof BYOK `env` on the body.
const ENV_HTTP_REFUSAL: &str =
    "refusing to send submit env over http:// — miner BYOK requires an https:// gateway";

fn body_has_env(body: &Value) -> bool {
    body.get("env")
        .and_then(Value::as_object)
        .is_some_and(|m| !m.is_empty())
}

fn is_https_url(url: &str) -> bool {
    url.get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
}

fn is_http_url(url: &str) -> bool {
    url.get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
}

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
    /// Proof submit POST / multipart timeout. `None` = wait until the host
    /// answers (`--submit-timeout-secs 0` / `CTX_PROOF_SUBMIT_TIMEOUT_SECS=0`).
    submit_timeout: Option<Duration>,
}

impl Client {
    /// Build a client for one gateway base URL.
    ///
    /// A Lium API key is attached only on `https://`. `http://` plus a key is
    /// refused so the header never goes out in cleartext. Redirects are never
    /// followed, so a 302 to `http://` cannot resend `X-Lium-Api-Key`.
    ///
    /// Proof submits use [`DEFAULT_SUBMIT_TIMEOUT_SECS`]. There is **no**
    /// client-wide reqwest timeout: tbench evaluate is synchronous.
    pub fn new(gateway: &str, lium_key: Option<String>) -> Result<Self, String> {
        Self::with_submit_timeout_secs(gateway, lium_key, DEFAULT_SUBMIT_TIMEOUT_SECS)
    }

    /// [`Self::new`] with an explicit Proof submit wait.
    ///
    /// `submit_timeout_secs == 0` waits until the host answers. GET routes
    /// stay on [`DEFAULT_GET_TIMEOUT_SECS`].
    pub fn with_submit_timeout_secs(
        gateway: &str,
        lium_key: Option<String>,
        submit_timeout_secs: u64,
    ) -> Result<Self, String> {
        let base = gateway.trim().trim_end_matches('/').to_owned();
        if !(is_https_url(&base) || is_http_url(&base)) {
            return Err(format!(
                "gateway must be an http(s) URL, got {base:?} (default is {DEFAULT_GATEWAY})"
            ));
        }
        let lium_key = lium_key.filter(|k| !k.trim().is_empty());
        if lium_key.is_some() && !is_https_url(&base) {
            return Err(KEYED_HTTP_REFUSAL.to_owned());
        }
        // Default reqwest policy resends headers (including X-Lium-Api-Key) to
        // any Location, including http://. Never auto-follow.
        // No client-wide `.timeout(...)`: GET and Proof POST set per-request
        // budgets. A total timeout of 60 s is what orphaned metal experiment
        // VMs (Broken pipe at t=60, no score).
        let http = reqwest::Client::builder()
            .user_agent(concat!("ctx/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Self {
            base,
            lium_key,
            http,
            submit_timeout: (submit_timeout_secs > 0)
                .then(|| Duration::from_secs(submit_timeout_secs)),
        })
    }

    /// The resolved gateway base URL.
    #[must_use]
    pub fn gateway(&self) -> &str {
        &self.base
    }

    /// GET a gateway path (`/v1/...` or `/challenge/...`).
    pub async fn get(&self, path: &str) -> Result<Reply, String> {
        self.send(
            self.http
                .get(self.url(path))
                .timeout(Duration::from_secs(DEFAULT_GET_TIMEOUT_SECS)),
        )
        .await
    }

    /// POST JSON or multipart to a gateway path.
    ///
    /// A non-empty submit `env` (miner BYOK) is refused on `http://` so the
    /// values never go out in cleartext — same floor as `X-Lium-Api-Key`.
    /// Proof `/v1/submissions` uses the submit timeout; other POSTs use the
    /// GET budget.
    pub async fn post(&self, path: &str, body: &Value) -> Result<Reply, String> {
        if body_has_env(body) && !is_https_url(&self.base) {
            return Err(ENV_HTTP_REFUSAL.to_owned());
        }
        self.send(self.with_post_timeout(path, self.http.post(self.url(path)).json(body)))
            .await
    }

    /// POST multipart fields plus an `artifact` part (uncompressed tar).
    pub async fn post_multipart(
        &self,
        path: &str,
        fields: &Value,
        artifact: Vec<u8>,
    ) -> Result<Reply, String> {
        if body_has_env(fields) && !is_https_url(&self.base) {
            return Err(ENV_HTTP_REFUSAL.to_owned());
        }
        let mut form = reqwest::multipart::Form::new();
        if let Some(obj) = fields.as_object() {
            for (k, v) in obj {
                let text = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                form = form.text(k.clone(), text);
            }
        }
        let part = reqwest::multipart::Part::bytes(artifact)
            .file_name("artifact.tar")
            .mime_str("application/octet-stream")
            .map_err(|e| format!("multipart: {e}"))?;
        self.send(
            self.with_post_timeout(
                path,
                self.http
                    .post(self.url(path))
                    .multipart(form.part("artifact", part)),
            ),
        )
        .await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn with_post_timeout(
        &self,
        path: &str,
        req: reqwest::RequestBuilder,
    ) -> reqwest::RequestBuilder {
        match self.post_timeout(path) {
            Some(d) => req.timeout(d),
            None => req,
        }
    }

    fn post_timeout(&self, path: &str) -> Option<Duration> {
        if is_proof_submit_path(path) {
            self.submit_timeout
        } else {
            Some(Duration::from_secs(DEFAULT_GET_TIMEOUT_SECS))
        }
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Reply, String> {
        // Miner BYOK. Accepted and never logged by the challenge services, and
        // never printed by this CLI. HTTPS is required whenever a key is set
        // (`Client::new` already refuses http + key); this guard is fail-closed
        // if a future constructor forgets that check.
        if self.lium_key.is_some() && !is_https_url(&self.base) {
            return Err(KEYED_HTTP_REFUSAL.to_owned());
        }
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

fn is_proof_submit_path(path: &str) -> bool {
    let p = path.trim();
    let p = p.split_once('?').map_or(p, |(head, _)| head);
    p.ends_with("/v1/submissions")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_gateway_is_the_public_host() {
        assert_eq!(DEFAULT_GATEWAY, "https://gateway.cortex.foundation");
        assert!(!DEFAULT_GATEWAY.ends_with('/'));
    }

    #[test]
    fn trailing_slash_does_not_double_up() {
        let c = Client::new("https://gateway.cortex.foundation/", None).expect("client");
        assert_eq!(c.gateway(), "https://gateway.cortex.foundation");
        assert_eq!(
            c.url("/v1/weights/latest"),
            "https://gateway.cortex.foundation/v1/weights/latest"
        );
    }

    #[test]
    fn non_http_gateway_is_rejected() {
        assert!(Client::new("gateway.cortex.foundation", None).is_err());
    }

    #[test]
    fn blank_lium_key_is_dropped() {
        let c = Client::new(DEFAULT_GATEWAY, Some("   ".into())).expect("client");
        assert!(c.lium_key.is_none());
    }

    #[test]
    fn http_gateway_without_key_is_allowed() {
        let c = Client::new("http://127.0.0.1:8090", None).expect("http without key");
        assert_eq!(c.gateway(), "http://127.0.0.1:8090");
        assert!(c.lium_key.is_none());
    }

    #[tokio::test]
    async fn http_gateway_refuses_submit_env() {
        let c = Client::new("http://127.0.0.1:8090", None).expect("http without key");
        let body = serde_json::json!({"env": {"OPENROUTER_API_KEY": "sk-or-test"}});
        let Err(err) = c.post("/challenge/proof/v1/submissions", &body).await else {
            panic!("http + env must fail closed");
        };
        assert!(
            err.contains("https://"),
            "error must name https as the requirement: {err}"
        );
        assert!(
            !err.contains("sk-or-test"),
            "must not echo the API key: {err}"
        );
    }

    #[test]
    fn http_gateway_with_lium_key_is_refused() {
        let Err(err) = Client::new("http://127.0.0.1:8090", Some("sk-test-key".into())) else {
            panic!("http + key must fail closed");
        };
        assert!(
            err.contains("https://"),
            "error must name https as the requirement: {err}"
        );
        assert!(
            err.contains("X-Lium-Api-Key") || err.contains("http://"),
            "{err}"
        );
        assert!(
            !err.contains("sk-test-key"),
            "must not echo the API key: {err}"
        );
    }

    #[test]
    fn http_scheme_is_matched_case_insensitively() {
        let Err(err) = Client::new("HTTP://127.0.0.1:8090", Some("sk-test-key".into())) else {
            panic!("HTTP:// + key must fail closed");
        };
        assert!(!err.contains("sk-test-key"), "must not echo the API key");
    }

    #[test]
    fn https_gateway_accepts_lium_key() {
        let c = Client::new(DEFAULT_GATEWAY, Some("sk-test-key".into())).expect("https + key");
        assert!(c.lium_key.is_some());
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

    /// A 302 to a second origin must not be followed. Auto-follow would resend
    /// `X-Lium-Api-Key` to whatever `Location` the gateway returns, including
    /// `http://`. Keyed clients require `https://` (not locally mockable here);
    /// the no-redirect policy is the same builder used for keyed calls.
    #[tokio::test]
    async fn does_not_follow_redirect_to_http_target() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let target = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("leaked"))
            .expect(0)
            .mount(&target)
            .await;

        let source = MockServer::start().await;
        let location = format!("{}/leaked", target.uri());
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", location))
            .mount(&source)
            .await;

        let c = Client::new(&source.uri(), None).expect("http without key");
        let reply = c.get("/v1/weights/latest").await.expect("response");
        assert_eq!(
            reply.status, 302,
            "must surface the 302 instead of following it"
        );
        drop(source);
        drop(target);
    }

    #[test]
    fn http_client_is_built_without_a_total_timeout() {
        let src = include_str!("lib.rs");
        let builder = src
            .split("reqwest::Client::builder()")
            .nth(1)
            .expect("builder");
        let builder = builder.split(".build()").next().expect("build");
        assert!(
            !builder.contains(".timeout("),
            "do not set a client-wide reqwest timeout (tbench evaluate is sync): {builder}"
        );
        assert_eq!(DEFAULT_GET_TIMEOUT_SECS, 60);
        assert_eq!(DEFAULT_SUBMIT_TIMEOUT_SECS, 7200);
    }

    #[test]
    fn proof_submit_timeout_zero_means_wait() {
        let c = Client::with_submit_timeout_secs(DEFAULT_GATEWAY, None, 0).expect("client");
        assert!(c.submit_timeout.is_none());
        let c = Client::new(DEFAULT_GATEWAY, None).expect("client");
        assert_eq!(
            c.submit_timeout,
            Some(Duration::from_secs(DEFAULT_SUBMIT_TIMEOUT_SECS))
        );
        assert!(is_proof_submit_path("/challenge/proof/v1/submissions"));
        assert!(is_proof_submit_path("/v1/submissions"));
        assert!(!is_proof_submit_path("/challenge/bounty/v1/reports"));
        assert!(!is_proof_submit_path("/v1/weights/latest"));
        assert!(!is_proof_submit_path("/challenge/proof/v1/submissions/pf"));
    }

    /// A Proof POST that takes longer than the old 60 s client-wide timeout
    /// would have been cut. This mock only waits 1.2 s (CI), but the client
    /// is built with **no** total timeout — the 7200 s budget is per-request.
    #[tokio::test]
    async fn proof_post_is_not_bound_by_a_client_wide_timeout() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/challenge/proof/v1/submissions"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_delay(Duration::from_millis(1200))
                    .set_body_json(serde_json::json!({"id": "ok", "state": "awaiting_admin"})),
            )
            .mount(&server)
            .await;

        let c = Client::with_submit_timeout_secs(&server.uri(), None, 5).expect("client");
        let reply = c
            .post(
                "/challenge/proof/v1/submissions",
                &serde_json::json!({"claim": "x"}),
            )
            .await
            .expect("response");
        assert_eq!(reply.status, 201, "{:?}", reply.body);
        drop(server);
    }
}
