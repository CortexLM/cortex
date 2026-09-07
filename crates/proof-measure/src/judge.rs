//! Judge egress for the scoring container.
//!
//! The scoring image runs untrusted miner code (`trust_remote_code=True`) in the
//! same process that reads the operator holdout, so it must never hold a route
//! to the internet or the judge credential. But `proof-eval` refuses to score
//! without a live judge call.
//!
//! Both hold at once: the scoring container joins an `internal` Docker network
//! with no route off the host, and a controller-owned proxy container is the
//! only other member. The proxy accepts exactly one upstream — the origin the
//! pinned `InferenceOffer` names — and injects the API key itself, so the key
//! never enters the scoring container's environment or its request. Anything
//! the workload aims elsewhere has nowhere to go.

use std::{path::Path, time::Duration};

use serde_json::{json, Value};

use crate::ObserverError;

/// Hostname the scoring container resolves the proxy by; the rewritten
/// `base_url` handed to `proof-eval` points here and nowhere else.
pub const JUDGE_HOST: &str = "proof-judge";
const PROXY_PORT: u16 = 8080;
/// Digest-pinned, locally present proxy image (no pull is ever attempted).
const READY_POLL: Duration = Duration::from_millis(100);
const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// A judge upstream reduced to what the proxy is allowed to reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeUpstream {
    /// Scheme + host + optional port; never a path.
    pub origin: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
}

impl JudgeUpstream {
    /// Parse the offer's `base_url` into a single reachable origin.
    ///
    /// # Errors
    /// Non-HTTP(S) schemes, missing host, credentials in the URL, or a
    /// loopback/link-local/private host, which would let the workload reach
    /// controller-side services instead of the judge.
    pub fn parse(base_url: &str) -> Result<Self, ObserverError> {
        Self::parse_allowing_loopback(base_url, false)
    }

    /// As [`Self::parse`], but an operator may opt into a loopback judge.
    ///
    /// A controller-side inference endpoint often listens on `127.0.0.1`. The
    /// scoring container still cannot reach it: only the proxy dials upstream,
    /// and it resolves loopback through the host gateway. The opt-in is narrow
    /// on purpose — with it enabled, the operator asserts the loopback port is
    /// the judge and not some other controller service.
    ///
    /// # Errors
    /// As [`Self::parse`]; private and link-local hosts stay refused either way.
    pub fn parse_allowing_loopback(
        base_url: &str,
        allow_loopback: bool,
    ) -> Result<Self, ObserverError> {
        let url = reqwest::Url::parse(base_url.trim()).map_err(|_| ObserverError::Request)?;
        let tls = match url.scheme() {
            "https" => true,
            "http" => false,
            _ => return Err(ObserverError::Request),
        };
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ObserverError::Request);
        }
        let host = url.host_str().ok_or(ObserverError::Request)?.to_owned();
        let loopback = is_loopback(&host);
        if is_local(&host) && !(allow_loopback && loopback) {
            return Err(ObserverError::Request);
        }
        // ponytail: rewritten loopback TLS needs a verified SNI-aware transport.
        if (!tls && !loopback) || (tls && loopback) {
            return Err(ObserverError::Request);
        }
        let port = url.port_or_known_default().ok_or(ObserverError::Request)?;
        Ok(Self {
            origin: format!(
                "{}://{host}{}",
                url.scheme(),
                url.port().map_or(String::new(), |p| format!(":{p}"))
            ),
            host,
            port,
            tls,
        })
    }

    /// The `base_url` the scoring container is given: same path shape, but
    /// pointed at the proxy. `proof-eval` appends `/chat/completions` etc.
    #[must_use]
    pub fn rewritten(&self, base_url: &str) -> String {
        let path = reqwest::Url::parse(base_url.trim()).ok().map_or_else(
            || "/v1".to_owned(),
            |u| u.path().trim_end_matches('/').to_owned(),
        );
        format!("http://{JUDGE_HOST}:{PROXY_PORT}{path}")
    }
}

/// Literal loopback only; a name that merely resolves to loopback is not
/// treated as such, so the opt-in cannot be widened by DNS.
fn is_loopback(host: &str) -> bool {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// Reject hosts that resolve back to the controller or its private network.
/// A literal IP is checked directly; a name that is not obviously local is
/// still confined by the proxy, which only ever dials the single upstream.
fn is_local(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_multicast()
                    || v4.is_broadcast()
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.segments()[0] & 0xfe00 == 0xfc00
                    || v6.is_unicast_link_local()
                    || v6.is_multicast()
                    || v6
                        .to_ipv4_mapped()
                        .is_some_and(|v4| is_local(&v4.to_string()))
            }
        };
    }
    let lower = host.trim_end_matches('.').to_ascii_lowercase();
    lower == "localhost"
        || [".localhost", ".internal", ".local"]
            .iter()
            .any(|suffix| lower.ends_with(suffix))
}

/// Per-run network and proxy names, derived from the run identity so a stale
/// pair is always removed rather than reused.
#[must_use]
pub fn network_name(base: &str) -> String {
    format!("{base}-net")
}

#[must_use]
pub fn proxy_name(base: &str) -> String {
    format!("{base}-judge")
}

/// Create-network body: `Internal` removes the default route, so members reach
/// each other and nothing else. The proxy is dual-homed by a second attach.
#[must_use]
pub fn network_body(name: &str, labels: &Value) -> Value {
    json!({
        "Name": name,
        "Driver": "bridge",
        "Internal": true,
        "Attachable": false,
        "EnableIPv6": false,
        "CheckDuplicate": true,
        "Labels": labels,
    })
}

/// Directory the controller stages the proxy's private configuration into. It
/// is an anonymous volume, so nothing lands on a host path and it disappears
/// with the container.
pub const KEY_DIR: &str = "/run/proof";
/// Upstream origin and the API key travel in the same private file. Neither is
/// an environment variable, so `docker inspect` on either container reveals the
/// judge's address no more than its credential.
pub const CONFIG_PATH: &str = "/run/proof/judge.json";
/// Name the proxy resolves a controller-side (loopback) judge by, mapped to the
/// Docker host gateway. Only the proxy ever receives this mapping.
pub const HOST_GATEWAY: &str = "proof-judge-host";

/// The proxy's private configuration document: everything the workload must not
/// see. Rendered by the controller and staged straight into the volume.
#[must_use]
pub fn proxy_config(upstream: &JudgeUpstream, api_key: &str) -> Vec<u8> {
    // A loopback judge is the controller's own port, which inside the proxy
    // means the host gateway rather than the proxy itself.
    let dial_host = if is_loopback(&upstream.host) {
        HOST_GATEWAY
    } else {
        &upstream.host
    };
    let scheme = if upstream.tls { "https" } else { "http" };
    let document = json!({
        "url": format!("{scheme}://{dial_host}:{}", upstream.port),
        "sni_host": upstream.host,
        "tls": upstream.tls,
        "api_key": api_key,
    });
    document.to_string().into_bytes()
}

/// Proxy container body. It listens on the internal network and forwards only
/// to the upstream named in its staged configuration, adding the bearer token
/// the scoring container never receives. The proxy runs read-only,
/// capability-free and unprivileged.
///
/// Nothing secret is bind-mounted or passed as an environment variable: an
/// operator key file is `0600` root-owned while this process is `nobody`, so
/// the controller stages origin and key together through the archive API.
///
/// `allow_loopback` opens the host gateway so a controller-side judge on
/// `127.0.0.1` is reachable from the proxy only.
///
/// # Errors
/// Non-absolute key path, which would resolve inside the container.
pub fn proxy_body(
    image_id: &str,
    key_file: &Path,
    labels: &Value,
    allow_loopback: bool,
) -> Result<Value, ObserverError> {
    // Checked here so a misconfigured path is refused before the daemon call
    // that would stage the secret.
    if !key_file.is_absolute() {
        return Err(ObserverError::Request);
    }
    Ok(json!({
        "Image": image_id,
        "Labels": labels,
        // Anonymous volume, so the key directory is writable at create time (the
        // archive API cannot write a read-only rootfs) and is discarded with the
        // container. It is never a host path.
        "Volumes": {KEY_DIR: {}},
        // Only the listen port and the config path; the upstream address and
        // the credential stay inside the staged file, never in inspectable env.
        "Env": [
            format!("PROOF_JUDGE_LISTEN={PROXY_PORT}"),
            format!("PROOF_JUDGE_CONFIG={CONFIG_PATH}"),
        ],
        "User": "65534:65534",
        "HostConfig": {
            "ReadonlyRootfs": true,
            "CapDrop": ["ALL"],
            "SecurityOpt": ["no-new-privileges:true"],
            "PidsLimit": 64,
            "Memory": 268_435_456_u64,
            "MemorySwap": 268_435_456_u64,
            "RestartPolicy": {"Name": "no"},
            // Only the proxy learns the host gateway, and only when the operator
            // opted into a controller-side judge. The scoring container has no
            // route here at all.
            "ExtraHosts": if allow_loopback { json!([format!("{HOST_GATEWAY}:host-gateway")]) } else { json!([]) },
            // No tmpfs over KEY_DIR: the staged key lives in the container's own
            // writable layer, which a tmpfs mount would shadow at start.
            "Tmpfs": {"/tmp": "rw,noexec,nosuid,nodev,size=8m"},
            // The local driver refuses max-file=1 unless compression is off.
            "LogConfig": {"Type": "local", "Config": {"max-size": "1m", "max-file": "1", "compress": "false"}}
        }
    }))
}

/// How long to wait for the proxy to accept connections before refusing the run.
#[must_use]
pub fn ready_deadline() -> (Duration, Duration) {
    (READY_POLL, READY_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_parses_public_https_origins_without_paths() {
        let up = JudgeUpstream::parse("https://api.example.com/v1").expect("public https");
        assert_eq!(up.origin, "https://api.example.com");
        assert_eq!((up.port, up.tls), (443, true));
        let explicit = JudgeUpstream::parse("https://api.example.com:8443/v1").expect("port");
        assert_eq!(explicit.origin, "https://api.example.com:8443");
        assert_eq!(explicit.port, 8443);
    }

    #[test]
    fn upstream_refuses_local_credentialed_and_foreign_schemes() {
        for bad in [
            "http://127.0.0.1:8000/v1",
            "http://localhost/v1",
            "https://10.0.0.5/v1",
            "https://192.168.1.9/v1",
            "https://[::1]/v1",
            "https://judge.internal/v1",
            "file:///etc/passwd",
            "ftp://example.com/v1",
            "https://user:pass@api.example.com/v1",
            "http://api.example.com/v1",
            "https://api.example.com/v1?secret=value",
            "https://api.example.com/v1#fragment",
            "https://[fe80::1]/v1",
            "https://[::ffff:127.0.0.1]/v1",
            "https://224.0.0.1/v1",
            "https://localhost./v1",
            "https://judge.local/v1",
        ] {
            assert!(
                JudgeUpstream::parse(bad).is_err(),
                "must refuse upstream {bad}"
            );
        }
    }

    #[test]
    fn loopback_upstream_needs_the_explicit_opt_in_and_only_for_literal_ips() {
        assert!(JudgeUpstream::parse("http://127.0.0.1:20128/v1").is_err());
        let up = JudgeUpstream::parse_allowing_loopback("http://127.0.0.1:20128/v1", true)
            .expect("opted-in loopback judge");
        assert_eq!((up.port, up.tls), (20128, false));
        // The opt-in never widens to private or named hosts.
        for bad in [
            "https://10.0.0.5/v1",
            "http://localhost/v1",
            "https://127.0.0.1/v1",
        ] {
            assert!(
                JudgeUpstream::parse_allowing_loopback(bad, true).is_err(),
                "opt-in must not cover {bad}"
            );
        }
    }

    #[test]
    fn staged_config_carries_the_secret_and_redirects_loopback_to_the_gateway() {
        let public = JudgeUpstream::parse("https://api.example.com/v1").expect("public");
        let doc: Value =
            serde_json::from_slice(&proxy_config(&public, "secret-token")).expect("json");
        assert_eq!(doc["url"], json!("https://api.example.com:443"));
        assert_eq!(doc["api_key"], json!("secret-token"));

        let local = JudgeUpstream::parse_allowing_loopback("http://127.0.0.1:20128/v1", true)
            .expect("loopback");
        let doc: Value = serde_json::from_slice(&proxy_config(&local, "k")).expect("json");
        // Inside the proxy, the controller's loopback is the host gateway.
        assert_eq!(doc["url"], json!(format!("http://{HOST_GATEWAY}:20128")));
        assert_eq!(doc["sni_host"], json!("127.0.0.1"));
    }

    #[test]
    fn loopback_opt_in_maps_the_host_gateway_only_for_the_proxy() {
        let body = proxy_body(
            "sha256:aa",
            Path::new("/private/judge.key"),
            &json!({}),
            true,
        )
        .expect("body");
        assert_eq!(
            body["HostConfig"]["ExtraHosts"],
            json!([format!("{HOST_GATEWAY}:host-gateway")])
        );
    }

    #[test]
    fn rewritten_base_url_keeps_the_path_and_never_the_real_host() {
        let up = JudgeUpstream::parse("https://api.example.com/v1").expect("public https");
        let rewritten = up.rewritten("https://api.example.com/v1");
        assert_eq!(rewritten, format!("http://{JUDGE_HOST}:{PROXY_PORT}/v1"));
        assert!(!rewritten.contains("api.example.com"));
    }

    #[test]
    fn network_is_internal_and_proxy_never_exposes_the_key_in_env() {
        let labels = json!({"cortex.proof.measure": "run"});
        let net = network_body(&network_name("base"), &labels);
        assert_eq!(net["Internal"], json!(true));
        assert_eq!(net["Name"], json!("base-net"));

        let body = proxy_body("sha256:aa", Path::new("/private/judge.key"), &labels, false)
            .expect("absolute key path");
        // Neither the upstream address nor the credential may be inspectable.
        let env = body["Env"].to_string();
        assert!(!env.contains("api.example.com"), "env leaked the upstream");
        assert!(!env.to_lowercase().contains("key="), "env leaked a key");
        assert!(env.contains("PROOF_JUDGE_CONFIG=/run/proof/judge.json"));
        assert_eq!(body["HostConfig"]["ReadonlyRootfs"], json!(true));
        assert_eq!(body["HostConfig"]["CapDrop"], json!(["ALL"]));
        assert_eq!(body["HostConfig"]["ExtraHosts"], json!([]));
        assert!(proxy_body("sha256:aa", Path::new("relative.key"), &labels, false).is_err());
    }
}
