//! The miner artefact, fetched **inside** the VM and judged on bytes: the
//! file served at `artifact_uri` must hash to the request's
//! `artifact_digest` and be an uncompressed tar with content
//! (`proof_vm_proto::tar::verify_artifact`). Anything else is a failed job —
//! never a substitute tree, never a re-tar.

use std::path::{Path, PathBuf};
use std::time::Duration;

use proof_rlm::CustomRunRequest;
use proof_vm_proto::tar::verify_artifact;

use crate::staging::unpack_tar;

/// Largest artefact the guest fetches (matches the host's relay cap).
pub const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;
/// Wall-clock for one fetch.
pub const FETCH_TIMEOUT: Duration = Duration::from_mins(5);
/// The verbatim bytes are kept beside the tree for adaptors that forward them.
pub const ARTIFACT_TAR_NAME: &str = "artifact.tar";
/// Unpacked tree directory name under the job's work dir.
pub const ARTIFACT_DIR_NAME: &str = "artifact";

fn scheme_ok(uri: &str, allow_plain_http: bool) -> Result<(), String> {
    if uri.starts_with("https://") || (allow_plain_http && uri.starts_with("http://")) {
        Ok(())
    } else {
        Err(format!(
            "artifact_uri {uri:?} must be https:// (plain http only when the guest allows it)"
        ))
    }
}

/// Fetch, verify, and unpack the request's artefact under `work`. `Ok(None)`
/// when the request carries no locator (a baseline with no artefact).
pub async fn fetch_artifact(
    request: &CustomRunRequest,
    work: &Path,
    allow_plain_http: bool,
) -> Result<Option<PathBuf>, String> {
    let Some(uri) = request.artifact_uri.as_deref().map(str::trim) else {
        return Ok(None);
    };
    scheme_ok(uri, allow_plain_http)?;
    let bytes = download(uri).await?;
    verify_artifact(&bytes, &request.artifact_digest)
        .map_err(|e| format!("artifact fetch from {uri}: {e}; refusing to run a substitute"))?;
    std::fs::create_dir_all(work).map_err(|e| format!("mkdir {}: {e}", work.display()))?;
    std::fs::write(work.join(ARTIFACT_TAR_NAME), &bytes)
        .map_err(|e| format!("keep artifact bytes: {e}"))?;
    let dir = work.join(ARTIFACT_DIR_NAME);
    unpack_tar(&bytes, &dir)?;
    Ok(Some(dir))
}

/// GET `uri` with a size cap and a deadline; the bytes as served.
pub async fn download(uri: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .get(uri)
        .send()
        .await
        .map_err(|e| format!("artifact fetch failed: {uri}: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!(
            "artifact fetch failed: {uri}: HTTP {}",
            resp.status()
        ));
    }
    if resp
        .content_length()
        .is_some_and(|n| n > MAX_ARTIFACT_BYTES as u64)
    {
        return Err(format!(
            "artifact at {uri} is larger than {MAX_ARTIFACT_BYTES} bytes"
        ));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("artifact fetch failed: {uri}: {}", e.without_url()))?;
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "artifact at {uri} is larger than {MAX_ARTIFACT_BYTES} bytes"
        ));
    }
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_by_default_and_no_locator_is_no_artifact() {
        scheme_ok("https://artefacts.example.test/recipe.tar", false).expect("https");
        assert!(scheme_ok("http://10.0.0.5/recipe.tar", false).is_err());
        scheme_ok("http://10.0.0.5/recipe.tar", true).expect("staging opt-in");
        assert!(scheme_ok("ftp://x/recipe.tar", true).is_err());
        assert!(scheme_ok("file:///etc/passwd", true).is_err());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        let mut req = proof_rlm::fixtures::request();
        req.artifact_uri = None;
        let work =
            std::env::temp_dir().join(format!("proof-vm-guest-fetch-{}", std::process::id()));
        assert_eq!(
            rt.block_on(fetch_artifact(&req, &work, false))
                .expect("no locator"),
            None
        );
        req.artifact_uri = Some("http://127.0.0.1:9/recipe.tar".into());
        let err = rt
            .block_on(fetch_artifact(&req, &work, false))
            .expect_err("plain http refused before any request");
        assert!(err.contains("must be https://"), "{err}");
        assert_eq!(MAX_ARTIFACT_BYTES, 64 * 1024 * 1024);
    }
}
